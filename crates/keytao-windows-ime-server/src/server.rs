use keytao_core::runtime_log::{self, Level};
use keytao_ime_ipc::{
    read_frame, read_response, write_frame, ClientRole, EngineError, Request, RequestEnvelope,
    Response, PROTOCOL_VERSION,
};
use keytao_windows_ime::{
    engine_backend::{ImeEngineBackend, InProcEngine},
    engine_status,
    pipe_security::{
        current_identity, keytao_install_root, pipe_client_identity, process_identity,
        process_image, Identity, OwnedHandle,
    },
};
use std::{
    fs::File,
    io::{self, Read, Write},
    os::windows::io::{AsRawHandle, FromRawHandle},
    path::PathBuf,
    sync::{Arc, Mutex, TryLockError},
    time::{Duration, Instant},
};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{
            LocalFree, ERROR_ACCESS_DENIED, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GENERIC_READ,
            GENERIC_WRITE, HANDLE, HLOCAL,
        },
        Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
            },
            PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
        },
        Storage::FileSystem::{
            CreateFileW, FlushFileBuffers, ReadFile, FILE_FLAG_FIRST_PIPE_INSTANCE,
            FILE_SHARE_MODE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION,
            SECURITY_SQOS_PRESENT,
        },
        System::{
            Pipes::{
                ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe,
                GetNamedPipeClientProcessId, GetNamedPipeServerProcessId, SetNamedPipeHandleState,
                PIPE_NOWAIT, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
            },
            Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
        },
    },
};

struct SecurityDescriptor(PSECURITY_DESCRIPTOR);
impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(HLOCAL(self.0 .0));
        }
    }
}

fn create_pipe(name: &str, sid: &str, first: bool) -> windows::core::Result<OwnedHandle> {
    let sddl: Vec<u16> = crate::engine_config::pipe_sddl(sid)
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
    }?;
    let descriptor = SecurityDescriptor(descriptor);
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0 .0,
        bInheritHandle: false.into(),
    };
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let mut flags = PIPE_ACCESS_DUPLEX;
    if first {
        flags |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            flags,
            PIPE_TYPE_BYTE | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            64 * 1024,
            64 * 1024,
            0,
            Some(&attributes),
        )
    };
    if handle.is_invalid() {
        return Err(windows::core::Error::from_win32());
    }
    Ok(OwnedHandle(handle))
}

fn acquire_listener(name: &str, identity: &Identity) -> Result<Option<OwnedHandle>, String> {
    match create_pipe(name, &identity.sid, true) {
        Ok(listener) => return Ok(Some(listener)),
        Err(error) if error.code() == ERROR_ACCESS_DENIED.to_hresult() => {}
        Err(error) => return Err(format!("create exclusive IME pipe: {error}")),
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    if !request_shutdown(name, identity, deadline)? {
        return Ok(None);
    }
    for _ in 0..30 {
        if Instant::now() >= deadline {
            break;
        }
        match create_pipe(name, &identity.sid, true) {
            Ok(listener) => return Ok(Some(listener)),
            Err(error) if error.code() == ERROR_ACCESS_DENIED.to_hresult() => {}
            Err(error) => return Err(format!("replace IME pipe: {error}")),
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("old IME server did not release its pipe within 3 seconds".into())
}

fn request_shutdown(name: &str, identity: &Identity, deadline: Instant) -> Result<bool, String> {
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let handle = loop {
        let result = unsafe {
            CreateFileW(
                PCWSTR(name.as_ptr()),
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_MODE(0),
                None,
                OPEN_EXISTING,
                SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                HANDLE::default(),
            )
        };
        match result {
            Ok(handle) => break handle,
            Err(error)
                if error.code() == ERROR_PIPE_BUSY.to_hresult() && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(format!("connect to old IME server: {error}")),
        }
    };
    let handle = OwnedHandle(handle);
    let mut pid = 0;
    unsafe { GetNamedPipeServerProcessId(handle.0, &mut pid) }.map_err(|e| e.to_string())?;
    let peer = process_identity(pid)?;
    if peer.session_id != identity.session_id {
        return Err("old IME server session does not match".into());
    }
    let image = installed_process(pid, &peer.sid, &identity.sid)?;
    let current_image = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|error| format!("resolve current server image: {error}"))?;
    if image
        .to_string_lossy()
        .eq_ignore_ascii_case(&current_image.to_string_lossy())
    {
        return Ok(false);
    }
    unsafe { SetNamedPipeHandleState(handle.0, Some(&PIPE_NOWAIT), None, None) }
        .map_err(|error| format!("set replacement pipe deadline mode: {error}"))?;
    let file = unsafe { File::from_raw_handle(handle.0 .0) };
    std::mem::forget(handle);
    let mut pipe = DeadlinePipe { file, deadline };
    let hello = RequestEnvelope {
        seq: 1,
        message: Request::Hello {
            protocol: PROTOCOL_VERSION - 1,
            client_build: env!("CARGO_PKG_VERSION").into(),
            role: ClientRole::App,
            is_appcontainer: false,
        },
    };
    write_frame(&mut pipe, &hello).map_err(|e| e.to_string())?;
    match read_response(&mut pipe, hello.seq).map_err(|e| e.to_string())? {
        Response::HelloOk { protocol, .. } if protocol == PROTOCOL_VERSION - 1 => {}
        response => return Err(format!("old IME server handshake failed: {response:?}")),
    }
    let shutdown = RequestEnvelope {
        seq: 2,
        message: Request::Shutdown,
    };
    write_frame(&mut pipe, &shutdown).map_err(|e| e.to_string())?;
    match read_response(&mut pipe, shutdown.seq).map_err(|e| e.to_string())? {
        Response::Ok => Ok(true),
        response => Err(format!("old IME server rejected replacement: {response:?}")),
    }
}

struct DeadlinePipe {
    file: File,
    deadline: Instant,
}

impl DeadlinePipe {
    fn check_deadline(&self) -> io::Result<()> {
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "server replacement timed out",
            ));
        }
        Ok(())
    }

    fn wait(&self) -> io::Result<()> {
        self.check_deadline()?;
        std::thread::sleep(Duration::from_millis(10));
        Ok(())
    }
}

impl Read for DeadlinePipe {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            self.check_deadline()?;
            let mut bytes_read = 0;
            // File::read turns ERROR_NO_DATA into EOF before we can retry it.
            let result = unsafe {
                ReadFile(
                    HANDLE(self.file.as_raw_handle()),
                    Some(buffer),
                    Some(&mut bytes_read),
                    None,
                )
            }
            .map_err(|error| error.code().0 & 0xffff);
            match crate::pipe_io::classify_read(result, bytes_read)? {
                Some(count) => return Ok(count),
                None => self.wait()?,
            }
        }
    }
}

impl Write for DeadlinePipe {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        loop {
            self.check_deadline()?;
            match self.file.write(buffer) {
                Ok(0) if !buffer.is_empty() => self.wait()?,
                result => return result,
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn installed_process(pid: u32, caller_sid: &str, server_sid: &str) -> Result<PathBuf, String> {
    let process = OwnedHandle(
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
            .map_err(|error| format!("open installed caller process: {error}"))?,
    );
    let image = process_image(process.0)
        .map_err(|error| format!("read installed caller image: {error}"))?
        .canonicalize()
        .map_err(|error| format!("resolve installed caller image: {error}"))?;
    let root = keytao_install_root()?;
    if !crate::authorization::installed_caller(caller_sid, server_sid, &image, &root) {
        return Err("caller image or user is outside the KeyTao installation".into());
    }
    Ok(image)
}

#[derive(Default)]
struct Runtime {
    engine: Option<InProcEngine>,
    generation: u64,
    ready: bool,
}

impl Runtime {
    fn deploy(&mut self) -> Result<(), String> {
        self.ready = false;
        self.generation = self.generation.wrapping_add(1);
        let result = if let Some(engine) = &self.engine {
            engine.deploy_server()
        } else {
            let engine = InProcEngine::default();
            let result = engine.deploy_server();
            if result.is_ok() {
                self.engine = Some(engine);
            }
            result
        };
        self.ready = result.is_ok();
        self.write_status(
            result.as_ref().err().map(String::as_str),
            result
                .as_ref()
                .ok()
                .and_then(|()| self.engine.as_ref().map(|engine| engine.state().schema_id)),
        );
        result
    }

    fn refresh(&mut self) -> Result<(), String> {
        let changed = self
            .engine
            .as_ref()
            .is_some_and(|engine| engine.reload_needed());
        if self.engine.is_some() && (changed || !self.ready) {
            // Invalidate every old connection before librime can tear its sessions down.
            if self.ready {
                self.generation = self.generation.wrapping_add(1);
                self.ready = false;
            }
            let engine = self.engine.as_ref().unwrap();
            let result = engine.reload_server();
            self.write_status(
                result.as_ref().err().map(String::as_str),
                result.as_ref().ok().map(|()| engine.state().schema_id),
            );
            result?;
            self.ready = true;
        }
        if self.engine.is_none() {
            let engine = InProcEngine::default();
            let result = engine.build_server();
            self.write_status(
                result.as_ref().err().map(String::as_str),
                result.as_ref().ok().map(|handle| handle.schema_id.clone()),
            );
            result?;
            self.engine = Some(engine);
            self.generation = self.generation.wrapping_add(1);
            self.ready = true;
        }
        Ok(())
    }

    fn write_status(&self, error: Option<&str>, schema: Option<String>) {
        if let Some(dir) = keytao_core::default_user_data_dir() {
            if let Err(error) = engine_status::write_status(&dir, error, schema.as_deref()) {
                keytao_core::rt_log!(
                    Level::Info,
                    "ipc",
                    "status_write_failed",
                    msg = error.to_string()
                );
            }
        }
    }

    fn session(&mut self) -> Result<(InProcEngine, u64), String> {
        self.refresh()?;
        Ok((
            self.engine.as_ref().unwrap().fork_session()?,
            self.generation,
        ))
    }
}

pub fn run() -> Result<(), String> {
    let identity = Arc::new(current_identity()?);
    let dir =
        keytao_core::default_user_data_dir().ok_or("cannot determine KeyTao user directory")?;
    runtime_log::init(&dir, "windows-ime-server");
    let name = crate::engine_config::pipe_name(&identity.sid, identity.session_id);
    let runtime = Arc::new(Mutex::new(Runtime::default()));
    let Some(mut listener) = acquire_listener(&name, &identity)? else {
        return Ok(());
    };
    loop {
        let watched_runtime = Arc::clone(&runtime);
        let started = std::thread::Builder::new()
            .name("ime-runtime-reload".into())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_millis(100));
                let result = {
                    let mut runtime = watched_runtime.lock().unwrap_or_else(|p| p.into_inner());
                    if runtime.engine.is_none() {
                        continue;
                    }
                    runtime.refresh()
                };
                if let Err(error) = result {
                    keytao_core::rt_log!(Level::Info, "ipc", "reload_failed", msg = error);
                    std::thread::sleep(Duration::from_secs(5));
                }
            });
        match started {
            Ok(_) => break,
            Err(error) => {
                keytao_core::rt_log!(
                    Level::Info,
                    "ipc",
                    "reload_thread_failed",
                    severity = "warn",
                    msg = error.to_string()
                );
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    loop {
        let connected = unsafe { ConnectNamedPipe(listener.0, None) };
        if let Err(error) = connected {
            if error.code() != ERROR_PIPE_CONNECTED.to_hresult() {
                keytao_core::rt_log!(
                    Level::Info,
                    "ipc",
                    "accept_failed",
                    severity = "warn",
                    msg = error.to_string()
                );
                let _ = unsafe { DisconnectNamedPipe(listener.0) };
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
        }
        // Keep this instance open on failure so another process cannot acquire the pipe name.
        let next = match create_pipe(&name, &identity.sid, false) {
            Ok(next) => next,
            Err(error) => {
                keytao_core::rt_log!(
                    Level::Info,
                    "ipc",
                    "create_next_pipe_failed",
                    severity = "warn",
                    msg = error.to_string()
                );
                let _ = unsafe { DisconnectNamedPipe(listener.0) };
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        let connection = Arc::new(Mutex::new(Some(std::mem::replace(&mut listener, next))));
        let client_connection = Arc::clone(&connection);
        let identity = identity.clone();
        let runtime = runtime.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("ime-pipe-client".into())
            .spawn(move || {
                let connection = client_connection
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .take()
                    .unwrap();
                if let Err(error) = serve(connection, &identity, &runtime) {
                    keytao_core::rt_log!(Level::Info, "ipc", "connection_closed", msg = error);
                }
            })
        {
            keytao_core::rt_log!(
                Level::Info,
                "ipc",
                "client_thread_failed",
                severity = "warn",
                msg = error.to_string()
            );
            if let Some(connection) = connection.lock().unwrap_or_else(|p| p.into_inner()).take() {
                reject_busy_connection(connection, &error.to_string());
            }
        }
    }
}

fn reject_busy_connection(pipe: OwnedHandle, reason: &str) {
    if unsafe { SetNamedPipeHandleState(pipe.0, Some(&PIPE_NOWAIT), None, None) }.is_err() {
        return;
    }
    let file = unsafe { File::from_raw_handle(pipe.0 .0) };
    std::mem::forget(pipe);
    let mut pipe = DeadlinePipe {
        file,
        deadline: Instant::now() + Duration::from_millis(250),
    };
    let Ok(hello) = read_frame::<_, RequestEnvelope>(&mut pipe) else {
        return;
    };
    if write_frame(
        &mut pipe,
        &hello.reply(Response::HelloErr(format!("server busy: {reason}"))),
    )
    .is_ok()
    {
        // Keep the reply alive until the client closes, without an unbounded FlushFileBuffers.
        let _ = io::copy(&mut pipe, &mut io::sink());
    }
}

fn serve(pipe: OwnedHandle, server: &Identity, runtime: &Mutex<Runtime>) -> Result<(), String> {
    let handle = pipe.0;
    let mut file = unsafe { File::from_raw_handle(handle.0) };
    std::mem::forget(pipe);
    let mut pid = 0;
    let authenticated = crate::handshake::accept(
        &mut file,
        || {
            unsafe { GetNamedPipeClientProcessId(handle, &mut pid) }
                .map_err(|error| format!("read pipe client pid: {error}"))?;
            let client = pipe_client_identity(handle, pid)?;
            if !crate::authorization::same_user_session(
                &client.sid,
                &server.sid,
                client.session_id,
                server.session_id,
            ) {
                return Err("pipe client user or session does not match".into());
            }
            Ok(client)
        },
        env!("CARGO_PKG_VERSION"),
    );
    let (hello, client) = match authenticated {
        Ok(Some(authenticated)) => authenticated,
        Ok(None) => return Ok(()),
        Err(error) => {
            // The shared logger has no Warn level and reserves pid for the server process.
            eprintln!("WARN KeyTao IME server rejected client {pid}: {error}");
            keytao_core::rt_log!(
                Level::Info,
                "ipc",
                "hello_rejected",
                severity = "warn",
                client_pid = pid,
                msg = error.clone()
            );
            // Do not discard a rejection reply when closing the server end immediately.
            let _ = unsafe { FlushFileBuffers(handle) };
            return Err(error);
        }
    };
    keytao_core::rt_log!(
        Level::Info,
        "ipc",
        "client_authenticated",
        client_pid = pid,
        is_appcontainer = client.is_appcontainer
    );
    let role = match hello.message {
        Request::Hello { role, .. } => role,
        _ => unreachable!(),
    };
    let mut session = ClientSession::default();
    let mut sequence = hello.seq;
    loop {
        let request: RequestEnvelope = read_frame(&mut file).map_err(|e| e.to_string())?;
        if request.seq <= sequence {
            return Err("non-increasing request sequence".into());
        }
        sequence = request.seq;
        let shutdown = matches!(request.message, Request::Shutdown);
        // A replacement must wait for deployment/userdb maintenance before exiting.
        let shutdown_guard = if shutdown
            && !client.is_appcontainer
            && installed_process(pid, &client.sid, &server.sid).is_ok()
        {
            Some(runtime.lock().unwrap_or_else(|p| p.into_inner()))
        } else {
            None
        };
        let response = if shutdown {
            if shutdown_guard.is_some() {
                Response::Ok
            } else {
                Response::Err(EngineError::Failed(
                    "Shutdown caller is not installed".into(),
                ))
            }
        } else if matches!(request.message, Request::Deploy) {
            // Identification tokens' AppContainer flag cannot independently authorize deployment.
            if crate::authorization::deployment_allowed(
                role,
                client.is_appcontainer,
                installed_process(pid, &client.sid, &server.sid).is_ok(),
            ) {
                Response::Deployed(runtime.lock().unwrap_or_else(|p| p.into_inner()).deploy())
            } else {
                Response::Deployed(Err(
                    "Deploy requires an installed authenticated App caller".into()
                ))
            }
        } else {
            dispatch(&request.message, &mut session, runtime)
        };
        let exit = shutdown && matches!(response, Response::Ok);
        write_frame(&mut file, &request.reply(response)).map_err(|e| e.to_string())?;
        if exit {
            // Flush waits until the replacement consumes the acknowledgment.
            if unsafe { FlushFileBuffers(HANDLE(file.as_raw_handle())) }.is_ok() {
                std::process::exit(0);
            }
        }
    }
}

#[derive(Default)]
struct ClientSession {
    engine: Option<InProcEngine>,
    generation: Option<u64>,
    initial_generation: Option<u64>,
}

fn dispatch(request: &Request, session: &mut ClientSession, runtime: &Mutex<Runtime>) -> Response {
    match request {
        Request::CreateSession => {
            // Keep the schema read inside the deploy barrier as it may rebuild a session.
            let mut runtime = runtime.lock().unwrap_or_else(|p| p.into_inner());
            let result = runtime.session();
            match result {
                Ok((engine, generation)) => {
                    let schema_id = engine.state().schema_id;
                    session.engine = Some(engine);
                    session.generation = Some(generation);
                    session.initial_generation = Some(generation);
                    Response::Handle { schema_id }
                }
                Err(error) => Response::Err(EngineError::Failed(error)),
            }
        }
        Request::DropSession => {
            *session = ClientSession::default();
            Response::Ok
        }
        Request::Generation => {
            // Bind the client's baseline to the session, even if a deploy races its first query.
            if let Some(generation) = session.initial_generation.take() {
                return Response::Generation(generation);
            }
            let Ok(runtime) = runtime.try_lock() else {
                return Response::Err(EngineError::Busy);
            };
            // The watcher reloads in the background; do not wait for it on a key deadline.
            let changed = runtime
                .engine
                .as_ref()
                .is_some_and(|engine| engine.reload_needed());
            Response::Generation(
                runtime
                    .generation
                    .wrapping_add(u64::from(changed && runtime.ready)),
            )
        }
        Request::Hello { .. } => Response::HelloErr("connection is already initialized".into()),
        // Privileged requests are authorized against the authenticated process in serve.
        Request::Deploy => Response::Deployed(Err("Deploy requires caller authorization".into())),
        Request::Shutdown => Response::Err(EngineError::Failed(
            "Shutdown requires caller authorization".into(),
        )),
        request => {
            // Brief stamp checks must not turn a composition key into raw input.
            let deadline = Instant::now() + Duration::from_millis(10);
            let runtime = loop {
                match runtime.try_lock() {
                    Ok(runtime) => break runtime,
                    Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(_) => return Response::Err(EngineError::Busy),
                }
            };
            if !runtime.ready || session.generation != Some(runtime.generation) {
                return Response::Err(EngineError::NotReady);
            }
            let Some(engine) = &session.engine else {
                return Response::Err(EngineError::NotReady);
            };
            match request {
                Request::ProcessKeys { keys } => Response::Key(engine.process_keys(keys)),
                Request::ProcessEnter => Response::Key(engine.process_enter()),
                Request::SelectCandidate { index } => {
                    Response::State(engine.select_candidate_on_page(*index))
                }
                Request::ChangePage { backward } => Response::State(engine.change_page(*backward)),
                Request::Reset => Response::State(engine.reset()),
                Request::GetState => Response::State(Some(engine.state())),
                Request::SetInputPolicy { policy } => {
                    Response::State(engine.set_input_policy(*policy))
                }
                Request::SetEnglishMode { enabled } => {
                    Response::English(engine.set_english_mode(*enabled))
                }
                _ => unreachable!(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_cannot_use_an_uncreated_session() {
        let runtime = Mutex::new(Runtime::default());
        let mut session = ClientSession::default();
        for request in [
            Request::ProcessEnter,
            Request::Reset,
            Request::GetState,
            Request::ProcessKeys {
                keys: vec![(97, 0)],
            },
        ] {
            assert!(matches!(
                dispatch(&request, &mut session, &runtime),
                Response::Err(EngineError::NotReady)
            ));
        }
    }

    #[test]
    fn deployment_dispatch_requires_authenticated_caller_authorization() {
        assert!(matches!(
            dispatch(
                &Request::Deploy,
                &mut ClientSession::default(),
                &Mutex::new(Runtime::default())
            ),
            Response::Deployed(Err(_))
        ));
    }

    #[test]
    fn generation_does_not_build_or_wait_for_a_runtime() {
        let runtime = Mutex::new(Runtime::default());
        let guard = runtime.lock().unwrap();
        assert!(matches!(
            dispatch(
                &Request::Generation,
                &mut ClientSession::default(),
                &runtime
            ),
            Response::Err(EngineError::Busy)
        ));
        drop(guard);
        assert!(matches!(
            dispatch(
                &Request::Generation,
                &mut ClientSession::default(),
                &runtime
            ),
            Response::Generation(0)
        ));
        assert!(runtime.lock().unwrap().engine.is_none());
    }

    #[test]
    fn initial_generation_does_not_swallow_a_racing_reload() {
        let runtime = Mutex::new(Runtime {
            engine: None,
            generation: 8,
            ready: false,
        });
        let mut session = ClientSession {
            engine: None,
            generation: Some(7),
            initial_generation: Some(7),
        };
        assert!(matches!(
            dispatch(&Request::Generation, &mut session, &runtime),
            Response::Generation(7)
        ));
        assert!(matches!(
            dispatch(&Request::Generation, &mut session, &runtime),
            Response::Generation(8)
        ));
    }

    #[test]
    fn reload_invalidates_sessions_even_when_the_runtime_is_not_ready() {
        let runtime = Mutex::new(Runtime {
            engine: Some(InProcEngine::default()),
            generation: 8,
            ready: true,
        });
        let mut session = ClientSession {
            engine: Some(InProcEngine::default()),
            generation: Some(7),
            initial_generation: None,
        };
        let request = Request::ProcessKeys { keys: vec![] };
        assert!(matches!(
            dispatch(&request, &mut session, &runtime),
            Response::Err(EngineError::NotReady)
        ));
        session.generation = Some(8);
        assert!(matches!(
            dispatch(&request, &mut session, &runtime),
            Response::Key(None)
        ));
        runtime.lock().unwrap().ready = false;
        assert!(matches!(
            dispatch(&request, &mut session, &runtime),
            Response::Err(EngineError::NotReady)
        ));
    }
}
