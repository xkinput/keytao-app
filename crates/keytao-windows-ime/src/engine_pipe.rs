//! Synchronous, deadline-bounded engine RPCs for the TSF calling thread.

use std::{
    io::Cursor,
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
            LocalFree, ERROR_ACCESS_DENIED, ERROR_IO_INCOMPLETE, ERROR_IO_PENDING, GENERIC_READ,
            GENERIC_WRITE, HANDLE, HLOCAL, WAIT_OBJECT_0, WAIT_TIMEOUT,
        },
        Security::{
            Authorization::{ConvertSidToStringSidW, GetSecurityInfo, SE_FILE_OBJECT},
            OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
        },
        Storage::FileSystem::{
            CreateFileW, ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, FILE_SHARE_MODE, OPEN_EXISTING,
            SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
        },
        System::{
            Pipes::GetNamedPipeServerProcessId,
            Threading::{
                CreateEventW, OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
            },
            IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
        },
    },
};

use crate::engine_backend::{EngineHandle, ImeEngineBackend};
use crate::engine_config::is_descendant;
use crate::pipe_security::{keytao_runtime_root, process_image, OwnedHandle};

#[path = "generation_activity.rs"]
mod generation_activity;
use generation_activity::{reaper_wait, GenerationActivity};

const REQUEST_TIMEOUT: Duration = Duration::from_millis(80);
const KEY_TIMEOUT: Duration = Duration::from_secs(2);
const BUILD_TIMEOUT: Duration = Duration::from_secs(30);
const GENERATION_POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Default)]
pub struct PipeEngine {
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    inner: Mutex<PipeState>,
    usable_session: Mutex<Option<Arc<()>>>,
    control: Mutex<Option<Connection>>,
    active: AtomicBool,
    reload_required: AtomicBool,
    reconnect_required: AtomicBool,
    consume_failed_key: AtomicBool,
    generation: AtomicU64,
    epoch: AtomicU64,
    activity: Mutex<GenerationActivity>,
    reaper: OnceLock<Sender<ReaperMessage>>,
}

impl Shared {
    fn track_schema(&self, schema_id: &str) {
        let mut usable = self.usable_session.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if schema_id.is_empty() || schema_id == ".default" {
            *usable = None;
        } else if usable.is_none() {
            *usable = keytao_core::default_user_data_dir()
                .map(|root| keytao_core::track_usable_input_session(&root));
        }
    }

    fn clear_usable_session(&self) {
        *self.usable_session.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

#[derive(Default)]
struct PipeState {
    connection: Option<Connection>,
    policy: Option<InputContextPolicy>,
    ascii_mode: Option<bool>,
    context_ascii_mode: Option<bool>,
}

impl PipeState {
    fn remember(&mut self, request: &Request, response: &Response) {
        let state = match response {
            Response::Key(Some(result)) => &result.state,
            Response::State(Some(state)) | Response::English(Ok(state)) => state,
            _ => return,
        };
        if let Request::SetInputPolicy { policy } = request {
            self.policy = Some(*policy);
        }
        match request {
            Request::SetContextAsciiMode { enabled } => self.context_ascii_mode = *enabled,
            Request::SetEnglishMode { .. } => self.context_ascii_mode = None,
            Request::ProcessKeys { .. } | Request::ProcessEnter
                if self.ascii_mode != Some(state.ascii_mode) =>
            {
                self.context_ascii_mode = None
            }
            _ => {}
        }
        if !state.schema_id.is_empty() {
            self.ascii_mode = Some(state.ascii_mode);
        }
    }

    fn settings(&self) -> Vec<Request> {
        let mut settings = Vec::new();
        if let Some(policy) = self.policy {
            settings.push(Request::SetInputPolicy { policy });
        }
        // Normal language/schema state is shared by the server, not a context override.
        if let Some(ascii) = self.context_ascii_mode {
            settings.push(Request::SetContextAsciiMode {
                enabled: Some(ascii),
            });
        }
        settings
    }
}

fn request_timeout(request: &Request) -> Duration {
    match request {
        Request::Generation => REQUEST_TIMEOUT,
        _ => KEY_TIMEOUT,
    }
}

#[derive(Debug)]
struct RequestError {
    message: String,
    written: bool,
    timed_out: bool,
}

impl RequestError {
    fn timeout() -> Self {
        Self {
            message: "engine pipe request timed out".into(),
            written: false,
            timed_out: true,
        }
    }

    fn consume_key(&self) -> bool {
        self.written && self.timed_out
    }
}

impl From<String> for RequestError {
    fn from(message: String) -> Self {
        Self {
            message,
            written: false,
            timed_out: false,
        }
    }
}

impl From<&str> for RequestError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

impl From<RequestError> for String {
    fn from(error: RequestError) -> Self {
        error.message
    }
}

impl PipeEngine {
    fn request(&self, request: Request) -> Option<Response> {
        self.request_with(
            request,
            |inner, deadline| self.reconnect(inner, deadline),
            |inner, request, deadline| Some(inner.connection.as_mut()?.request(request, deadline)),
        )
    }

    fn request_with(
        &self,
        request: Request,
        reconnect: impl FnOnce(&mut PipeState, Instant) -> Result<u64, String>,
        send: impl FnOnce(&mut PipeState, Request, Instant) -> Option<Result<Response, RequestError>>,
    ) -> Option<Response> {
        let started = Instant::now();
        let timeout = request_timeout(&request);
        let deadline = started + timeout;
        let key_request = matches!(
            &request,
            Request::ProcessKeys { .. } | Request::ProcessEnter
        );
        self.shared
            .consume_failed_key
            .store(false, Ordering::Release);
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
        if self.shared.reconnect_required.load(Ordering::Acquire) {
            let generation = match reconnect(&mut inner, deadline) {
                Ok(generation) => generation,
                Err(error) => {
                    self.shared.clear_usable_session();
                    self.shared.reload_required.store(true, Ordering::Release);
                    log_disconnect(&error);
                    return None;
                }
            };
            if self.shared.generation.load(Ordering::Acquire) != generation {
                self.shared.clear_usable_session();
                inner.connection = None;
                self.shared.reload_required.store(true, Ordering::Release);
                return None;
            }
            self.shared
                .reconnect_required
                .store(false, Ordering::Release);
        }
        let result = send(&mut inner, request.clone(), deadline)?;
        if self.shared.epoch.load(Ordering::Acquire) != epoch
            || !self.shared.active.load(Ordering::Acquire)
        {
            self.shared.clear_usable_session();
            inner.connection = None;
            return None;
        }
        match result {
            Ok(Response::Err(EngineError::NotReady)) => {
                self.shared.clear_usable_session();
                inner.connection = None;
                self.shared.reload_required.store(true, Ordering::Release);
                None
            }
            Ok(response) => {
                match &response {
                    Response::Key(Some(result)) => self.shared.track_schema(&result.state.schema_id),
                    Response::State(Some(state)) | Response::English(Ok(state)) => self.shared.track_schema(&state.schema_id),
                    Response::Key(None) | Response::State(None) | Response::Err(EngineError::Failed(_)) => self.shared.clear_usable_session(),
                    _ => {}
                }
                inner.remember(&request, &response);
                Some(response)
            }
            Err(error) => {
                self.shared.clear_usable_session();
                inner.connection = None;
                let consume = key_request && error.consume_key();
                self.shared
                    .consume_failed_key
                    .store(consume, Ordering::Release);
                if consume {
                    self.shared.epoch.fetch_add(1, Ordering::AcqRel);
                    self.shared
                        .reconnect_required
                        .store(true, Ordering::Release);
                } else {
                    self.shared.reload_required.store(true, Ordering::Release);
                }
                drop(inner);
                if key_request && error.timed_out {
                    keytao_core::rt_log!(
                        keytao_core::runtime_log::Level::Info,
                        "ipc",
                        "key_request_timeout",
                        key = format!("{request:?}"),
                        duration_ms = started.elapsed().as_millis(),
                        timeout_ms = timeout.as_millis(),
                        written = error.written
                    );
                }
                log_disconnect(&error.message);
                None
            }
        }
    }

    fn reconnect(&self, inner: &mut PipeState, deadline: Instant) -> Result<u64, String> {
        let (mut connection, control, _) = Connection::connect_until(
            self.shared
                .reaper
                .get()
                .ok_or("pipe reaper unavailable")?
                .clone(),
            deadline,
        )?;
        for setting in inner.settings() {
            match connection.request(setting, deadline)? {
                Response::State(Some(_)) | Response::English(Ok(_)) => {}
                _ => return Err("could not restore engine session settings".into()),
            }
        }
        let state = connection.request(Request::GetState, deadline)?;
        if !matches!(state, Response::State(Some(_))) {
            return Err("could not resync engine session state".into());
        }
        inner.remember(&Request::GetState, &state);
        if let Response::State(Some(state)) = &state {
            self.shared.track_schema(&state.schema_id);
        }
        let generation = connection.generation;
        *self
            .shared
            .control
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(control);
        inner.connection = Some(connection);
        Ok(generation)
    }
}

impl ImeEngineBackend for PipeEngine {
    fn build(&self) -> Result<EngineHandle, String> {
        self.shared.clear_usable_session();
        self.shared.active.store(false, Ordering::Release);
        let epoch = self.shared.epoch.fetch_add(1, Ordering::AcqRel) + 1;
        self.shared.reload_required.store(false, Ordering::Release);
        self.shared
            .reconnect_required
            .store(false, Ordering::Release);
        self.shared
            .consume_failed_key
            .store(false, Ordering::Release);
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
        inner.policy = None;
        inner.ascii_mode = None;
        inner.context_ascii_mode = None;
        self.shared.active.store(true, Ordering::Release);
        self.shared.track_schema(&handle.schema_id);
        drop(inner);
        let _ = self
            .shared
            .reaper
            .get()
            .unwrap()
            .send(ReaperMessage::Activity);
        Ok(handle)
    }

    fn reload_needed(&self) -> bool {
        let was_idle = self
            .shared
            .activity
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .record_key(Instant::now());
        if was_idle {
            // Check before State decides whether this key needs a new session.
            poll_generation(&self.shared, true);
            if let Some(reaper) = self.shared.reaper.get() {
                let _ = reaper.send(ReaperMessage::Activity);
            }
        }
        self.shared.reload_required.load(Ordering::Acquire)
    }

    fn drop_session(&self) {
        self.shared.clear_usable_session();
        self.shared.active.store(false, Ordering::Release);
        self.shared.epoch.fetch_add(1, Ordering::AcqRel);
        self.shared.reload_required.store(false, Ordering::Release);
        self.shared
            .reconnect_required
            .store(false, Ordering::Release);
        self.shared
            .consume_failed_key
            .store(false, Ordering::Release);
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

    fn consume_failed_key(&self) -> bool {
        self.shared.consume_failed_key.swap(false, Ordering::AcqRel)
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

    fn set_context_ascii_mode(&self, enabled: Option<bool>) -> Option<ImeState> {
        self.request(Request::SetContextAsciiMode { enabled })?
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
    reaper: Sender<ReaperMessage>,
    sequence: u64,
    generation: u64,
}

impl Connection {
    fn connect(reaper: Sender<ReaperMessage>) -> Result<(Self, Self, EngineHandle), String> {
        Self::connect_until(reaper, Instant::now() + BUILD_TIMEOUT)
    }

    fn connect_until(
        reaper: Sender<ReaperMessage>,
        deadline: Instant,
    ) -> Result<(Self, Self, EngineHandle), String> {
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

    fn open(reaper: Sender<ReaperMessage>, deadline: Instant) -> Result<Self, String> {
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
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
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

    fn request(&mut self, request: Request, deadline: Instant) -> Result<Response, RequestError> {
        let mut written = false;
        let result = (|| {
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
            written = true;
            let header = self.read_exact(4, deadline)?;
            let length = u32::from_le_bytes(header.as_slice().try_into().unwrap()) as usize;
            if length > MAX_FRAME_SIZE {
                return Err("engine response exceeds 1 MiB".into());
            }
            let mut frame = header;
            frame.extend(self.read_exact(length, deadline)?);
            read_response(&mut Cursor::new(frame), self.sequence)
                .map_err(|error| RequestError::from(format!("decode engine response: {error}")))
        })();
        result.map_err(|mut error| {
            error.written = written;
            error
        })
    }

    fn read_exact(&self, length: usize, deadline: Instant) -> Result<Vec<u8>, RequestError> {
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
    ) -> Result<(Vec<u8>, usize), RequestError> {
        if Instant::now() >= deadline {
            return Err(RequestError::timeout());
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
                return Err(format!("start pipe I/O: {error}").into());
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
                return Err(if wait == WAIT_TIMEOUT {
                    RequestError::timeout()
                } else {
                    format!("wait for pipe I/O failed: {wait:?}").into()
                });
            }
        }
        let mut count = 0;
        if let Err(error) =
            unsafe { GetOverlappedResult(self.pipe.0, &*pending.overlapped, &mut count, false) }
        {
            if error.code() == ERROR_IO_INCOMPLETE.to_hresult() {
                self.cancel_pending(pending);
            }
            return Err(format!("complete pipe I/O: {error}").into());
        }
        Ok((pending.buffer, count as usize))
    }

    fn cancel_pending(&self, pending: PendingIo) {
        unsafe {
            let _ = CancelIoEx(self.pipe.0, Some(&*pending.overlapped));
        }
        if let Err(error) = self.reaper.send(ReaperMessage::Pending(pending)) {
            // A failed reaper must leak rather than free pending I/O memory.
            std::mem::forget(error.0);
        }
    }
}

enum ReaperMessage {
    Activity,
    Pending(PendingIo),
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

fn start_reaper(shared: std::sync::Weak<Shared>) -> Result<Sender<ReaperMessage>, String> {
    crate::globals::pin_module().map_err(|error| format!("pin pipe worker module: {error}"))?;
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("keytao-pipe-io-reaper".into())
        .spawn(move || reap_pending(receiver, shared))
        .map_err(|error| format!("start pipe I/O reaper: {error}"))?;
    Ok(sender)
}

fn reap_pending(receiver: Receiver<ReaperMessage>, shared: std::sync::Weak<Shared>) {
    let mut pending = Vec::new();
    let mut disconnected = false;
    let mut check_at = Instant::now() + GENERATION_POLL_INTERVAL;
    loop {
        let now = Instant::now();
        let activity_remaining = shared.upgrade().and_then(|shared| {
            if !shared.active.load(Ordering::Acquire)
                || shared.reload_required.load(Ordering::Acquire)
            {
                return None;
            }
            let remaining = shared
                .activity
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remaining(now);
            remaining
        });
        let message = match reaper_wait(
            activity_remaining,
            check_at.saturating_duration_since(now),
            !pending.is_empty(),
        ) {
            Some(wait) => receiver.recv_timeout(wait),
            None => receiver
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        };
        match message {
            Ok(ReaperMessage::Pending(operation)) => pending.push(operation),
            Ok(ReaperMessage::Activity) => {
                check_at = Instant::now() + GENERATION_POLL_INTERVAL;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => disconnected = true,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        pending.retain(|operation| !operation.completed());
        if Instant::now() >= check_at {
            check_at = Instant::now() + GENERATION_POLL_INTERVAL;
            if let Some(shared) = shared.upgrade() {
                let active = shared
                    .activity
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remaining(Instant::now())
                    .is_some();
                if active {
                    poll_generation(&shared, false);
                }
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

fn poll_generation(shared: &Shared, synchronous: bool) {
    if !shared.active.load(Ordering::Acquire)
        || shared.reload_required.load(Ordering::Acquire)
        || shared.reconnect_required.load(Ordering::Acquire)
    {
        return;
    }
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    let mut control = if synchronous {
        shared
            .control
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    } else {
        let Ok(control) = shared.control.try_lock() else {
            return;
        };
        control
    };
    if !shared.active.load(Ordering::Acquire)
        || shared.reload_required.load(Ordering::Acquire)
        || shared.reconnect_required.load(Ordering::Acquire)
    {
        return;
    }
    let epoch = shared.epoch.load(Ordering::Acquire);
    let Some(connection) = control.as_mut() else {
        return;
    };
    let response = connection.request(Request::Generation, deadline);
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
    apply_generation_response(shared, epoch, response);
}

fn apply_generation_response(
    shared: &Shared,
    epoch: u64,
    response: Result<Response, RequestError>,
) {
    // Serialize the decision with key timeouts, which must recover without a State reload.
    let mut inner = shared.inner.lock().unwrap_or_else(|p| p.into_inner());
    if !shared.active.load(Ordering::Acquire)
        || shared.epoch.load(Ordering::Acquire) != epoch
        || shared.reconnect_required.load(Ordering::Acquire)
    {
        return;
    }
    match response {
        Ok(Response::Generation(generation)) => {
            if shared.generation.load(Ordering::Acquire) != generation {
                shared.clear_usable_session();
                shared.reload_required.store(true, Ordering::Release);
            }
        }
        Ok(Response::Err(EngineError::Busy)) => {}
        Ok(_) => {
            shared.clear_usable_session();
            inner.connection = None;
            shared.reload_required.store(true, Ordering::Release);
            log_disconnect("invalid engine generation response");
        }
        Err(error) => {
            shared.clear_usable_session();
            inner.connection = None;
            shared.reload_required.store(true, Ordering::Release);
            log_disconnect(&error.message);
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
    let image = match process_image(process.0) {
        Ok(image) => image,
        Err(error)
            if identity.is_appcontainer && error.code() == ERROR_ACCESS_DENIED.to_hresult() =>
        {
            return verify_pipe_owner(pipe, &identity.sid);
        }
        Err(error) => return Err(format!("read engine server image path: {error}")),
    };
    let root = keytao_runtime_root()?;
    let image = image
        .canonicalize()
        .map_err(|error| format!("resolve engine server image path: {error}"))?;
    if !is_descendant(&image, &root) {
        return Err("engine server image is outside the KeyTao runtime root".into());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> Request {
        Request::ProcessKeys {
            keys: vec![(b'0' as u32, 0)],
        }
    }

    fn engine() -> PipeEngine {
        let engine = PipeEngine::default();
        engine.shared.active.store(true, Ordering::Release);
        engine
    }

    fn written_timeout(engine: &PipeEngine) {
        assert!(engine
            .request_with(
                key(),
                |_, _| panic!("connected key must not reconnect"),
                |_, _, _| Some(Err(RequestError {
                    written: true,
                    ..RequestError::timeout()
                })),
            )
            .is_none());
        assert!(engine.shared.consume_failed_key.load(Ordering::Acquire));
        assert!(engine.shared.reconnect_required.load(Ordering::Acquire));
        assert!(!engine.reload_needed());
    }

    #[test]
    fn session_requests_allow_slow_lua_but_generation_stays_short() {
        for request in [
            key(),
            Request::ProcessEnter,
            Request::Reset,
            Request::GetState,
            Request::SetInputPolicy {
                policy: InputContextPolicy::default(),
            },
            Request::SetContextAsciiMode { enabled: None },
            Request::SelectCandidate { index: 0 },
            Request::ChangePage { backward: false },
        ] {
            assert_eq!(request_timeout(&request), Duration::from_secs(2));
        }
        assert_eq!(
            request_timeout(&Request::Generation),
            Duration::from_millis(80)
        );
    }

    #[test]
    fn only_written_timeouts_consume_failed_keys() {
        for (written, timed_out, expected) in [
            (true, true, true),
            (false, true, false),
            (true, false, false),
            (false, false, false),
        ] {
            let engine = engine();
            let error = RequestError {
                written,
                timed_out,
                message: "pipe failure".into(),
            };
            assert!(engine
                .request_with(key(), |_, _| unreachable!(), |_, _, _| Some(Err(error)))
                .is_none());
            assert_eq!(engine.consume_failed_key(), expected);
            assert_eq!(
                engine.shared.reconnect_required.load(Ordering::Acquire),
                expected
            );
        }
    }

    #[test]
    fn next_key_reconnects_then_processes_without_entering_reload() {
        let engine = engine();
        written_timeout(&engine);
        let reconnected = std::cell::Cell::new(false);
        let response = engine.request_with(key(), |_, _| {
            reconnected.set(true);
            Ok(0)
        }, |_, request, _| {
            assert!(reconnected.get(), "recover before sending the next key");
            assert!(matches!(request, Request::ProcessKeys { keys } if keys == vec![(b'0' as u32, 0)]));
            Some(Ok(Response::Key(Some(KeyProcessResult { accepted: true, state: ImeState::empty() }))))
        });
        assert!(
            response
                .unwrap()
                .into_key_result()
                .unwrap()
                .unwrap()
                .accepted
        );
        assert!(!engine.consume_failed_key());
        assert!(!engine.shared.reconnect_required.load(Ordering::Acquire));
        assert!(!engine.reload_needed());
    }

    #[test]
    fn reconnect_failure_before_next_key_write_passes_through_and_requires_rebuild() {
        for first_request in [key(), Request::SetContextAsciiMode { enabled: None }] {
            let engine = engine();
            written_timeout(&engine);
            assert!(engine
                .request_with(
                    first_request,
                    |_, deadline| {
                        assert!(deadline <= Instant::now() + KEY_TIMEOUT);
                        Err("connect failed".into())
                    },
                    |_, _, _| panic!("must not write after connect failure")
                )
                .is_none());
            assert!(!engine.consume_failed_key());
            assert!(engine.reload_needed());
            for request in [Request::GetState, Request::ProcessEnter, key()] {
                assert!(engine
                    .request_with(
                        request,
                        |_, _| panic!("must not reconnect again before background rebuild"),
                        |_, _, _| panic!("must not write before background rebuild")
                    )
                    .is_none());
            }
        }
    }

    #[test]
    fn reconnect_generation_change_requires_reload_before_next_key_write() {
        let engine = engine();
        engine.shared.generation.store(7, Ordering::Release);
        written_timeout(&engine);
        assert!(engine
            .request_with(
                key(),
                |_, _| Ok(8),
                |_, _, _| panic!("must not write a key after generation changed")
            )
            .is_none());
        assert!(engine.reload_needed());
        assert_eq!(engine.shared.generation.load(Ordering::Acquire), 7);
        assert!(!engine.consume_failed_key());
    }

    #[test]
    fn failed_key_consumption_is_taken_once_without_another_request() {
        let engine = engine();
        written_timeout(&engine);
        assert!(engine.consume_failed_key());
        assert!(
            !engine.consume_failed_key(),
            "a later key skipped before sending a request must pass through"
        );
    }

    #[test]
    fn recovery_uses_mode_after_key_toggle_instead_of_old_setter() {
        let mut inner = PipeState::default();
        let mut state = ImeState::empty();
        state.schema_id = "xmjd6".into();
        state.ascii_mode = true;
        inner.remember(
            &Request::SetEnglishMode { enabled: true },
            &Response::English(Ok(state.clone())),
        );
        state.ascii_mode = false;
        inner.remember(
            &key(),
            &Response::Key(Some(KeyProcessResult {
                state,
                accepted: true,
            })),
        );
        assert!(
            inner.settings().is_empty(),
            "use the server's current global mode"
        );
        inner.remember(
            &Request::SetContextAsciiMode {
                enabled: Some(false),
            },
            &Response::State(Some(ImeState {
                schema_id: "xmjd6".into(),
                ..ImeState::empty()
            })),
        );
        assert!(matches!(
            inner.settings().as_slice(),
            [Request::SetContextAsciiMode {
                enabled: Some(false)
            }]
        ));
        inner.remember(
            &key(),
            &Response::Key(Some(KeyProcessResult {
                state: ImeState {
                    schema_id: "xmjd6".into(),
                    ascii_mode: true,
                    ..ImeState::empty()
                },
                accepted: true,
            })),
        );
        assert!(
            inner.settings().is_empty(),
            "a key toggle clears the context override"
        );
        inner.remember(
            &Request::SetInputPolicy {
                policy: InputContextPolicy::sensitive(),
            },
            &Response::State(Some(ImeState::empty())),
        );
        assert!(
            matches!(inner.settings().first(), Some(Request::SetInputPolicy { policy }) if !policy.composing)
        );
    }

    #[test]
    fn old_generation_timeout_cannot_turn_key_recovery_into_reload() {
        let engine = engine();
        let epoch = engine.shared.epoch.load(Ordering::Acquire);
        written_timeout(&engine);
        apply_generation_response(&engine.shared, epoch, Err(RequestError::timeout()));
        assert!(!engine.reload_needed());
        assert!(engine.shared.reconnect_required.load(Ordering::Acquire));
        assert!(engine.consume_failed_key());
        assert!(engine
            .request_with(key(), |_, _| Ok(0), |_, _, _| Some(Ok(Response::Key(None))))
            .is_some());
        apply_generation_response(&engine.shared, epoch, Err(RequestError::timeout()));
        assert!(
            !engine.reload_needed(),
            "old poll must stay stale after reconnect"
        );
    }

    #[test]
    fn reset_reconnect_failure_does_not_reuse_previous_key_consumption() {
        let engine = engine();
        written_timeout(&engine);
        assert!(engine
            .request_with(
                Request::Reset,
                |_, _| Err("connect failed".into()),
                |_, _, _| panic!("must not write after connect failure")
            )
            .is_none());
        assert!(!engine.consume_failed_key());
        assert!(engine.reload_needed());
    }

    #[test]
    fn generation_failure_is_applied_after_current_key_releases_connection() {
        let engine = engine();
        let inner = engine.shared.inner.lock().unwrap();
        let shared = engine.shared.clone();
        let ready = Arc::new(std::sync::Barrier::new(2));
        let worker_ready = ready.clone();
        let (done, completed) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            worker_ready.wait();
            apply_generation_response(&shared, 0, Err(RequestError::timeout()));
            done.send(()).unwrap();
        });
        ready.wait();
        assert_eq!(
            completed.recv_timeout(Duration::from_millis(100)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        );
        drop(inner);
        worker.join().unwrap();
        assert!(engine.reload_needed());
    }
}
