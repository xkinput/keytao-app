//! Synchronous, deadline-bounded engine RPCs for the TSF calling thread.

use std::{
    io::Cursor,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

use keytao_core::{ImeState, InputContextPolicy, KeyProcessResult};
use keytao_ime_ipc::{
    read_response, write_frame, ClientRole, EngineError, Request, RequestEnvelope, Response,
    MAX_FRAME_SIZE, PROTOCOL_VERSION,
};
use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Foundation::{
            CloseHandle, LocalFree, ERROR_ACCESS_DENIED, ERROR_IO_INCOMPLETE, ERROR_IO_PENDING,
            GENERIC_READ, GENERIC_WRITE, HANDLE, HLOCAL, WAIT_OBJECT_0,
        },
        Security::{
            Authorization::{ConvertSidToStringSidW, GetSecurityInfo, SE_FILE_OBJECT},
            OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
        },
        Storage::FileSystem::{
            CreateFileW, ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, FILE_SHARE_MODE, OPEN_EXISTING,
        },
        System::{
            Pipes::GetNamedPipeServerProcessId,
            Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY},
            Threading::{
                CreateEventW, OpenProcess, QueryFullProcessImageNameW, WaitForSingleObject,
                PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
            },
            IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
        },
    },
};

use crate::engine_backend::{EngineHandle, ImeEngineBackend};

const REQUEST_TIMEOUT: Duration = Duration::from_millis(80);
const BUILD_TIMEOUT: Duration = Duration::from_secs(30);
const GENERATION_POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Default)]
pub struct PipeEngine {
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    inner: Mutex<PipeState>,
    control: Mutex<Option<Connection>>,
    active: AtomicBool,
    reload_required: AtomicBool,
    generation: AtomicU64,
    epoch: AtomicU64,
    reaper: OnceLock<Sender<PendingIo>>,
}

#[derive(Default)]
struct PipeState {
    connection: Option<Connection>,
}

impl PipeEngine {
    fn request(&self, request: Request) -> Option<Response> {
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        if !self.shared.active.load(Ordering::Acquire)
            || self.shared.reload_required.load(Ordering::Acquire)
        {
            return None;
        }
        let epoch = self.shared.epoch.load(Ordering::Acquire);
        // A background build or another callback must never hold up the STA.
        let mut inner = self.shared.inner.try_lock().ok()?;
        if !self.shared.active.load(Ordering::Acquire)
            || self.shared.reload_required.load(Ordering::Acquire)
        {
            return None;
        }
        let result = inner.connection.as_mut()?.request(request, deadline);
        if self.shared.epoch.load(Ordering::Acquire) != epoch
            || !self.shared.active.load(Ordering::Acquire)
        {
            inner.connection = None;
            return None;
        }
        match result {
            Ok(Response::Err(EngineError::NotReady)) => {
                inner.connection = None;
                self.shared.reload_required.store(true, Ordering::Release);
                None
            }
            Ok(response) => Some(response),
            Err(error) => {
                inner.connection = None;
                self.shared.reload_required.store(true, Ordering::Release);
                drop(inner);
                log_disconnect(&error);
                None
            }
        }
    }
}

impl ImeEngineBackend for PipeEngine {
    fn build(&self) -> Result<EngineHandle, String> {
        self.shared.active.store(false, Ordering::Release);
        let epoch = self.shared.epoch.fetch_add(1, Ordering::AcqRel) + 1;
        self.shared.reload_required.store(false, Ordering::Release);
        if self.shared.reaper.get().is_none() {
            let reaper = start_reaper(Arc::downgrade(&self.shared))?;
            let _ = self.shared.reaper.set(reaper);
        }
        let (connection, control, handle) =
            Connection::connect(self.shared.reaper.get().unwrap().clone())?;
        *self
            .shared
            .control
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(control);
        let mut inner = self
            .shared
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.shared.epoch.load(Ordering::Acquire) != epoch {
            return Err("pipe engine build was superseded".into());
        }
        self.shared
            .generation
            .store(connection.generation, Ordering::Release);
        inner.connection = Some(connection);
        self.shared.active.store(true, Ordering::Release);
        Ok(handle)
    }

    fn reload_needed(&self) -> bool {
        // The worker polls Generation; a key callback never adds a second RTT.
        self.shared.reload_required.load(Ordering::Acquire)
    }

    fn drop_session(&self) {
        self.shared.active.store(false, Ordering::Release);
        self.shared.epoch.fetch_add(1, Ordering::AcqRel);
        self.shared.reload_required.store(false, Ordering::Release);
        if let Ok(mut inner) = self.shared.inner.try_lock() {
            // Disconnect releases the server session without a second STA wait.
            inner.connection = None;
        }
        if let Ok(mut control) = self.shared.control.try_lock() {
            *control = None;
        }
    }

    fn process_key(&self, keysym: u32, mods: u32) -> Option<KeyProcessResult> {
        self.process_keys(&[(keysym, mods)])
    }

    fn process_keys(&self, keys: &[(u32, u32)]) -> Option<KeyProcessResult> {
        self.request(Request::ProcessKeys {
            keys: keys.to_vec(),
        })?
        .into_key_result()
        .ok()
        .flatten()
    }

    fn process_enter(&self) -> Option<KeyProcessResult> {
        self.request(Request::ProcessEnter)?
            .into_key_result()
            .ok()
            .flatten()
    }

    fn select_candidate_on_page(&self, index: usize) -> Option<ImeState> {
        self.request(Request::SelectCandidate { index })?
            .into_state()
            .ok()
            .flatten()
    }

    fn change_page(&self, backward: bool) -> Option<ImeState> {
        self.request(Request::ChangePage { backward })?
            .into_state()
            .ok()
            .flatten()
    }

    fn reset(&self) -> Option<ImeState> {
        self.request(Request::Reset)?.into_state().ok().flatten()
    }

    fn state(&self) -> ImeState {
        self.request(Request::GetState)
            .and_then(|response| response.into_state().ok().flatten())
            .unwrap_or_else(ImeState::empty)
    }

    fn set_input_policy(&self, policy: InputContextPolicy) -> Option<ImeState> {
        self.request(Request::SetInputPolicy { policy })?
            .into_state()
            .ok()
            .flatten()
    }

    fn set_english_mode(&self, enabled: bool) -> Result<ImeState, String> {
        match self.request(Request::SetEnglishMode { enabled }) {
            Some(Response::English(result)) => result,
            _ => Err("pipe engine is unavailable".into()),
        }
    }
}

struct Connection {
    pipe: Arc<OwnedHandle>,
    reaper: Sender<PendingIo>,
    sequence: u64,
    generation: u64,
}

impl Connection {
    fn connect(reaper: Sender<PendingIo>) -> Result<(Self, Self, EngineHandle), String> {
        let deadline = Instant::now() + BUILD_TIMEOUT;
        let mut connection = Self::open(reaper.clone(), deadline)?;
        let schema_id = match connection.request(Request::CreateSession, deadline)? {
            Response::Handle { schema_id } => schema_id,
            Response::Err(error) => return Err(format!("create engine session: {error:?}")),
            _ => return Err("invalid create-session response".into()),
        };
        connection.generation = match connection.request(Request::Generation, deadline)? {
            Response::Generation(generation) => generation,
            _ => return Err("invalid engine generation response".into()),
        };
        // Generation has a separate authenticated pipe and never owns a session.
        let control = Self::open(reaper, deadline)?;
        Ok((connection, control, EngineHandle { schema_id }))
    }

    fn open(reaper: Sender<PendingIo>, deadline: Instant) -> Result<Self, String> {
        let identity = crate::pipe_security::current_identity()?;
        let name = wide(&crate::engine_config::pipe_name(
            &identity.sid,
            identity.session_id,
        ));
        let pipe = unsafe {
            CreateFileW(
                PCWSTR(name.as_ptr()),
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_MODE(0),
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                HANDLE::default(),
            )
        }
        .map_err(|error| format!("connect to engine pipe: {error}"))?;
        let pipe = Arc::new(OwnedHandle(pipe));
        verify_server_image(pipe.0, &identity)?;
        let mut connection = Self {
            pipe,
            reaper,
            sequence: 0,
            generation: 0,
        };
        match connection.request(
            Request::Hello {
                protocol: PROTOCOL_VERSION,
                client_build: env!("CARGO_PKG_VERSION").into(),
                role: ClientRole::Ime,
                is_appcontainer: identity.is_appcontainer,
            },
            deadline,
        )? {
            Response::HelloOk { protocol, .. } if protocol == PROTOCOL_VERSION => {}
            Response::HelloErr(error) => return Err(format!("engine handshake: {error}")),
            _ => return Err("invalid engine handshake response".into()),
        }
        Ok(connection)
    }

    fn request(&mut self, request: Request, deadline: Instant) -> Result<Response, String> {
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or("IPC sequence exhausted")?;
        let mut frame = Vec::new();
        write_frame(
            &mut frame,
            &RequestEnvelope {
                seq: self.sequence,
                message: request,
            },
        )
        .map_err(|error| format!("encode engine request: {error}"))?;
        while !frame.is_empty() {
            let (buffer, count) = self.transfer(frame, false, deadline)?;
            if count == 0 {
                return Err("engine pipe disconnected while writing".into());
            }
            frame = buffer[count..].to_vec();
        }
        let header = self.read_exact(4, deadline)?;
        let length = u32::from_le_bytes(header.as_slice().try_into().unwrap()) as usize;
        if length > MAX_FRAME_SIZE {
            return Err("engine response exceeds 1 MiB".into());
        }
        let mut frame = header;
        frame.extend(self.read_exact(length, deadline)?);
        read_response(&mut Cursor::new(frame), self.sequence)
            .map_err(|error| format!("decode engine response: {error}"))
    }

    fn read_exact(&self, length: usize, deadline: Instant) -> Result<Vec<u8>, String> {
        let mut output = Vec::with_capacity(length);
        while output.len() < length {
            let (buffer, count) = self.transfer(vec![0; length - output.len()], true, deadline)?;
            if count == 0 {
                return Err("engine pipe disconnected while reading".into());
            }
            output.extend_from_slice(&buffer[..count]);
        }
        Ok(output)
    }

    fn transfer(
        &self,
        buffer: Vec<u8>,
        read: bool,
        deadline: Instant,
    ) -> Result<(Vec<u8>, usize), String> {
        if Instant::now() >= deadline {
            return Err("engine pipe request timed out".into());
        }
        let event = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
            .map_err(|error| format!("create pipe I/O event: {error}"))?;
        let mut pending = PendingIo {
            pipe: self.pipe.clone(),
            event: OwnedHandle(event),
            overlapped: Box::new(OVERLAPPED {
                hEvent: event,
                ..OVERLAPPED::default()
            }),
            buffer,
        };
        let started = unsafe {
            if read {
                ReadFile(
                    self.pipe.0,
                    Some(&mut pending.buffer),
                    None,
                    Some(&mut *pending.overlapped),
                )
            } else {
                WriteFile(
                    self.pipe.0,
                    Some(&pending.buffer),
                    None,
                    Some(&mut *pending.overlapped),
                )
            }
        };
        if let Err(error) = &started {
            if error.code() != ERROR_IO_PENDING.to_hresult() {
                return Err(format!("start pipe I/O: {error}"));
            }
        }
        if started.is_err() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let milliseconds = remaining.as_millis().min(u32::MAX as u128) as u32;
            let wait = unsafe { WaitForSingleObject(pending.event.0, milliseconds) };
            if wait != WAIT_OBJECT_0 {
                // CancelIoEx is nonblocking. Keep every kernel-visible allocation
                // alive until a background reaper observes actual completion.
                self.cancel_pending(pending);
                return Err("engine pipe request timed out or wait failed".into());
            }
        }
        let mut count = 0;
        if let Err(error) =
            unsafe { GetOverlappedResult(self.pipe.0, &*pending.overlapped, &mut count, false) }
        {
            if error.code() == ERROR_IO_INCOMPLETE.to_hresult() {
                self.cancel_pending(pending);
            }
            return Err(format!("complete pipe I/O: {error}"));
        }
        Ok((pending.buffer, count as usize))
    }

    fn cancel_pending(&self, pending: PendingIo) {
        unsafe {
            let _ = CancelIoEx(self.pipe.0, Some(&*pending.overlapped));
        }
        if let Err(error) = self.reaper.send(pending) {
            // A failed reaper must leak rather than free pending I/O memory.
            std::mem::forget(error.0);
        }
    }
}

struct OwnedHandle(HANDLE);

// SAFETY: These handles are owned, never dereferenced, and Windows permits their
// use and closure on a different thread from the one that opened them.
unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

struct PendingIo {
    pipe: Arc<OwnedHandle>,
    event: OwnedHandle,
    overlapped: Box<OVERLAPPED>,
    buffer: Vec<u8>,
}

// SAFETY: The buffer and OVERLAPPED live in stable heap allocations and are
// accessed only after the kernel reports completion, including cancellation.
unsafe impl Send for PendingIo {}

impl PendingIo {
    fn completed(&self) -> bool {
        let mut count = 0;
        match unsafe { GetOverlappedResult(self.pipe.0, &*self.overlapped, &mut count, false) } {
            Ok(()) => true,
            Err(error) => error.code() != ERROR_IO_INCOMPLETE.to_hresult(),
        }
    }
}

fn start_reaper(shared: std::sync::Weak<Shared>) -> Result<Sender<PendingIo>, String> {
    crate::globals::pin_module().map_err(|error| format!("pin pipe worker module: {error}"))?;
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("keytao-pipe-io-reaper".into())
        .spawn(move || reap_pending(receiver, shared))
        .map_err(|error| format!("start pipe I/O reaper: {error}"))?;
    Ok(sender)
}

fn reap_pending(receiver: Receiver<PendingIo>, shared: std::sync::Weak<Shared>) {
    let mut pending = Vec::new();
    let mut disconnected = false;
    let mut check_at = Instant::now() + GENERATION_POLL_INTERVAL;
    loop {
        let wait = check_at
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(100));
        match receiver.recv_timeout(wait) {
            Ok(operation) => pending.push(operation),
            Err(mpsc::RecvTimeoutError::Disconnected) => disconnected = true,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        pending.retain(|operation| !operation.completed());
        if Instant::now() >= check_at {
            check_at = Instant::now() + GENERATION_POLL_INTERVAL;
            if let Some(shared) = shared.upgrade() {
                poll_generation(&shared);
            }
        }
        if disconnected {
            if pending.is_empty() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn poll_generation(shared: &Shared) {
    if !shared.active.load(Ordering::Acquire) || shared.reload_required.load(Ordering::Acquire) {
        return;
    }
    let epoch = shared.epoch.load(Ordering::Acquire);
    let Ok(mut control) = shared.control.try_lock() else {
        return;
    };
    let Some(connection) = control.as_mut() else {
        return;
    };
    let response = connection.request(Request::Generation, Instant::now() + REQUEST_TIMEOUT);
    if shared.epoch.load(Ordering::Acquire) != epoch {
        *control = None;
        return;
    }
    if !matches!(
        response,
        Ok(Response::Generation(_) | Response::Err(EngineError::Busy))
    ) {
        *control = None;
    }
    drop(control);
    match response {
        Ok(Response::Generation(generation)) => {
            if shared.generation.load(Ordering::Acquire) != generation {
                shared.reload_required.store(true, Ordering::Release);
            }
        }
        Ok(Response::Err(EngineError::Busy)) => {}
        Ok(_) => {
            if let Ok(mut inner) = shared.inner.try_lock() {
                inner.connection = None;
            }
            shared.reload_required.store(true, Ordering::Release);
            log_disconnect("invalid engine generation response");
        }
        Err(error) => {
            if let Ok(mut inner) = shared.inner.try_lock() {
                inner.connection = None;
            }
            shared.reload_required.store(true, Ordering::Release);
            log_disconnect(&error);
        }
    }
}

fn verify_server_image(
    pipe: HANDLE,
    identity: &crate::pipe_security::Identity,
) -> Result<(), String> {
    let mut process_id = 0;
    unsafe { GetNamedPipeServerProcessId(pipe, &mut process_id) }
        .map_err(|error| format!("read engine server process ID: {error}"))?;
    let process = match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) }
    {
        Ok(process) => OwnedHandle(process),
        Err(error)
            if identity.is_appcontainer && error.code() == ERROR_ACCESS_DENIED.to_hresult() =>
        {
            return verify_pipe_owner(pipe, &identity.sid);
        }
        Err(error) => return Err(format!("open engine server process: {error}")),
    };
    let mut image = vec![0_u16; 32768];
    let mut length = image.len() as u32;
    if let Err(error) = unsafe {
        QueryFullProcessImageNameW(
            process.0,
            PROCESS_NAME_WIN32,
            PWSTR(image.as_mut_ptr()),
            &mut length,
        )
    } {
        if identity.is_appcontainer && error.code() == ERROR_ACCESS_DENIED.to_hresult() {
            return verify_pipe_owner(pipe, &identity.sid);
        }
        return Err(format!("read engine server image path: {error}"));
    }
    let image = PathBuf::from(String::from_utf16_lossy(&image[..length as usize]));
    let root = program_files_dir()?
        .join("KeyTao")
        .canonicalize()
        .map_err(|error| format!("resolve KeyTao installation directory: {error}"))?;
    let image = image
        .canonicalize()
        .map_err(|error| format!("resolve engine server image path: {error}"))?;
    if !is_descendant(&image, &root) {
        return Err("engine server image is outside Program Files\\KeyTao".into());
    }
    log_server_verification("exe_path");
    Ok(())
}

fn verify_pipe_owner(pipe: HANDLE, user_sid: &str) -> Result<(), String> {
    let mut owner = PSID::default();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    unsafe {
        GetSecurityInfo(
            pipe,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            None,
            None,
            Some(&mut descriptor),
        )
    }
    .ok()
    .map_err(|error| format!("read engine pipe owner: {error}"))?;
    let owner_sid = (|| {
        if owner.0.is_null() {
            return Err("engine pipe has no owner SID".into());
        }
        let mut sid_text = PWSTR::null();
        unsafe { ConvertSidToStringSidW(owner, &mut sid_text) }
            .map_err(|error| format!("encode engine pipe owner SID: {error}"))?;
        let sid = unsafe { sid_text.to_string() };
        unsafe { LocalFree(HLOCAL(sid_text.0.cast())) };
        sid.map_err(|error| format!("decode engine pipe owner SID: {error}"))
    })();
    unsafe { LocalFree(HLOCAL(descriptor.0)) };
    if owner_sid? != user_sid {
        return Err("engine pipe owner does not match the client user".into());
    }
    log_server_verification("pipe_owner_sid");
    Ok(())
}

fn log_server_verification(check: &str) {
    keytao_core::rt_log!(
        keytao_core::runtime_log::Level::Info,
        "ipc",
        "server_identity_verified",
        check = check
    );
}

fn program_files_dir() -> Result<PathBuf, String> {
    let mut value = vec![0_u16; 32768];
    let mut size = (value.len() * std::mem::size_of::<u16>()) as u32;
    // The native registry view is independent of the TSF host's bitness and environment.
    let result = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            windows::core::w!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion"),
            windows::core::w!("ProgramFilesDir"),
            RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
            None,
            Some(value.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if result.is_err() || size < 4 || size as usize > value.len() * 2 || size % 2 != 0 {
        return Err("cannot determine native Program Files directory".into());
    }
    let value = &value[..size as usize / 2];
    let length = value
        .iter()
        .position(|unit| *unit == 0)
        .ok_or("unterminated Program Files directory")?;
    let path = String::from_utf16(&value[..length])
        .map_err(|error| format!("decode Program Files directory: {error}"))?;
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err("Program Files directory is not absolute".into());
    }
    Ok(path)
}

fn is_descendant(image: &Path, root: &Path) -> bool {
    let image: Vec<_> = image.components().collect();
    let root: Vec<_> = root.components().collect();
    image.len() > root.len()
        && root.iter().zip(&image).all(|(left, right)| {
            left.as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
        })
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn log_disconnect(error: &str) {
    keytao_core::rt_log!(
        keytao_core::runtime_log::Level::Info,
        "ipc",
        "pipe_disconnected",
        msg = error
    );
}
