use crate::runtime_state::{lock_for_key, reload_candidate, KEY_WAIT_BUDGET};
use keytao_core::deploy_child;
use keytao_core::runtime_log::{self, Level};
use keytao_core::DeployProgress;
use keytao_ime_ipc::{
    read_frame, read_response, write_frame, ClientRole, EngineError, Request, RequestEnvelope,
    Response, PROTOCOL_VERSION,
};
use keytao_windows_ime::{
    engine_backend::{ImeEngineBackend, InProcEngine},
    engine_status,
    pipe_security::{
        current_identity, keytao_app_root, keytao_runtime_root, pipe_client_identity,
        process_identity, process_image, Identity, OwnedHandle,
    },
};
use parking_lot::Mutex;
use std::{
    fs::File,
    io::{self, Read, Write},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle},
    },
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex, OnceLock},
    time::{Duration, Instant},
};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{
            LocalFree, ERROR_ACCESS_DENIED, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GENERIC_READ,
            GENERIC_WRITE, HANDLE, HLOCAL, WAIT_OBJECT_0, WAIT_TIMEOUT,
        },
        Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
            },
            PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
        },
        Storage::FileSystem::{
            CreateFileW, FindCloseChangeNotification, FindFirstChangeNotificationW,
            FindNextChangeNotification, FlushFileBuffers, ReadFile, FILE_FLAG_FIRST_PIPE_INSTANCE,
            FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
            FILE_SHARE_MODE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION,
            SECURITY_SQOS_PRESENT,
        },
        System::{
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
                SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            },
            Pipes::{
                ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe,
                GetNamedPipeClientProcessId, GetNamedPipeServerProcessId, SetNamedPipeHandleState,
                PIPE_NOWAIT, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
            },
            Threading::{
                GetCurrentProcess, OpenProcess, WaitForSingleObject,
                PROCESS_QUERY_LIMITED_INFORMATION,
            },
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
    let image = installed_process(pid, &peer.sid, &identity.sid, &[keytao_runtime_root()])
        .map_err(|error| format!("old IME server failed runtime root verification: {error}"))?;
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

fn installed_process(
    pid: u32,
    caller_sid: &str,
    server_sid: &str,
    roots: &[Result<PathBuf, String>],
) -> Result<PathBuf, String> {
    let process = OwnedHandle(
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
            .map_err(|error| format!("open installed caller process: {error}"))?,
    );
    let image = process_image(process.0)
        .map_err(|error| format!("read installed caller image: {error}"))?
        .canonicalize()
        .map_err(|error| format!("resolve installed caller image: {error}"))?;
    if !crate::authorization::installed_caller(
        caller_sid,
        server_sid,
        &image,
        roots.iter().filter_map(|root| root.as_deref().ok()),
    ) {
        return Err(format!(
            "caller image or user is outside the trusted KeyTao roots: {roots:?}"
        ));
    }
    Ok(image)
}

type Runtime = crate::runtime_state::Runtime<InProcEngine>;

static MAINTENANCE_LOCK: Mutex<()> = Mutex::new(());

fn ensure_deploy_job() -> Result<(), String> {
    // Keep the sole, non-inherited handle until server exit; children inherit job membership.
    static JOB: OnceLock<Result<OwnedHandle, String>> = OnceLock::new();
    JOB.get_or_init(|| {
        let job = OwnedHandle(
            unsafe { CreateJobObjectW(None, PCWSTR::null()) }
                .map_err(|error| format!("create deploy job: {error}"))?,
        );
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of_val(&limits) as u32,
            )
        }
        .map_err(|error| format!("configure deploy job: {error}"))?;
        // Joining before spawn avoids an orphan window between spawn and assignment.
        unsafe { AssignProcessToJobObject(job.0, GetCurrentProcess()) }
            .map_err(|error| format!("join deploy job: {error}"))?;
        Ok(job)
    })
    .as_ref()
    .map(|_| ())
    .map_err(Clone::clone)
}

fn deploy(
    runtime: &Mutex<Runtime>,
    package_plan: Option<&str>,
    progress: impl FnMut(&DeployProgress),
) -> Result<(), String> {
    let _deploy = MAINTENANCE_LOCK.lock();
    let root = keytao_core::default_user_data_dir().ok_or("cannot determine Rime directory")?;
    let migrating = keytao_core::RuntimeDirs::needs_migration(&root);
    ensure_deploy_job()?;
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut command = std::process::Command::new(executable);
    if let Some(plan) = package_plan {
        command.env(deploy_child::PACKAGE_TARGETS_ENV, plan);
    }
    // Published generations keep the old build available while the child compiles.
    runtime.lock().deploying = true;
    let result = deploy_child::run_deploy_child_command_with_progress(
        &mut command,
        Duration::from_secs(600),
        progress,
    );
    runtime.lock().deploying = false;
    result?;
    if package_plan.is_none() {
        keytao_core::ReloadStamp::write(&root)?;
        refresh(runtime)?;
    }
    drop(_deploy);
    if migrating && keytao_core::RuntimeDirs::migration_complete(&root) {
        if let Err(error) = keytao_app_core::windows_public_schemas::publish_active_if_needed(&root) {
            keytao_app_core::windows_public_schemas::public_warning("migration", &error);
        }
    }
    Ok(())
}

impl Runtime {
    fn write_status(error: Option<&str>, schema: Option<String>) {
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

    fn session(&self) -> Result<(InProcEngine, u64, String), String> {
        let engine = self.engine.as_ref().unwrap().fork_session()?;
        let schema_id = engine.state().schema_id;
        Ok((engine, self.generation, schema_id))
    }
}

// Callers hold MAINTENANCE_LOCK so deployment cannot race the unlocked checks/reload.
fn refresh(runtime: &Mutex<Runtime>) -> Result<bool, String> {
    if let Some(engine) = reload_candidate(runtime, InProcEngine::reload_needed) {
        let was_ready = {
            let mut runtime = runtime.lock();
            let was_ready = runtime.ready;
            runtime.ready = false;
            runtime.refreshing = true;
            was_ready
        };
        let result = engine.reload_server();
        if result
            .as_ref()
            .is_err_and(|error| error == keytao_core::PACKAGE_SWITCH_PENDING)
        {
            let mut runtime = runtime.lock();
            runtime.ready = was_ready;
            runtime.refreshing = false;
            return Ok(false);
        }
        let schema = result.as_ref().ok().map(|_| engine.state().schema_id);
        {
            let mut runtime = runtime.lock();
            // Schema-only requests preserve sessions, including their composition.
            if was_ready && !matches!(&result, Ok(false)) {
                runtime.generation = runtime.generation.wrapping_add(1);
            }
            runtime.ready = result.is_ok();
            runtime.refreshing = false;
        }
        Runtime::write_status(result.as_ref().err().map(String::as_str), schema);
        result?;
    } else if runtime.lock().engine.is_none() {
        let engine = Arc::new(InProcEngine::default());
        let result = engine.build_server();
        Runtime::write_status(
            result.as_ref().err().map(String::as_str),
            result.as_ref().ok().map(|handle| handle.schema_id.clone()),
        );
        result?;
        let mut runtime = runtime.lock();
        runtime.engine = Some(engine);
        runtime.generation = runtime.generation.wrapping_add(1);
        runtime.ready = true;
    }
    Ok(true)
}

struct ReloadNotification(HANDLE);

impl ReloadNotification {
    fn new(dir: &Path) -> Option<Self> {
        let path: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe {
            FindFirstChangeNotificationW(
                PCWSTR(path.as_ptr()),
                false,
                FILE_NOTIFY_CHANGE_LAST_WRITE
                    | FILE_NOTIFY_CHANGE_FILE_NAME
                    | FILE_NOTIFY_CHANGE_SIZE,
            )
        }
        .ok()
        .map(Self)
    }

    fn wait(&self) -> bool {
        match unsafe { WaitForSingleObject(self.0, 5_000) } {
            // Rearm before refreshing so a stamp written during reload is still observed.
            WAIT_OBJECT_0 => unsafe { FindNextChangeNotification(self.0) }.is_ok(),
            WAIT_TIMEOUT => true,
            _ => false,
        }
    }
}

impl Drop for ReloadNotification {
    fn drop(&mut self) {
        let _ = unsafe { FindCloseChangeNotification(self.0) };
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
        let watched_dir = dir.clone();
        let mut stamp = keytao_core::ReloadStampGate::new(&watched_dir);
        let started = std::thread::Builder::new()
            .name("ime-runtime-reload".into())
            .spawn(move || {
                let mut notification = ReloadNotification::new(&watched_dir);
                loop {
                    if !notification.as_ref().is_some_and(ReloadNotification::wait) {
                        drop(notification.take());
                        std::thread::sleep(Duration::from_secs(1));
                        notification = ReloadNotification::new(&watched_dir);
                    }
                    // Status/current-schema writes and timeouts are not reload requests.
                    let Some(signature) = stamp.pending() else {
                        continue;
                    };
                    let Some(_maintenance) = MAINTENANCE_LOCK.try_lock() else {
                        continue;
                    };
                    if watched_runtime.lock().engine.is_none() {
                        continue;
                    }
                    let result = refresh(&watched_runtime);
                    if matches!(&result, Ok(true)) {
                        stamp.commit(signature);
                    }
                    drop(_maintenance);
                    if let Err(error) = result {
                        keytao_core::rt_log!(Level::Info, "ipc", "reload_failed", msg = error);
                        std::thread::sleep(Duration::from_secs(5));
                    }
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
        let connection = Arc::new(StdMutex::new(Some(std::mem::replace(&mut listener, next))));
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
            && installed_process(
                pid,
                &client.sid,
                &server.sid,
                &[keytao_app_root(), keytao_runtime_root()],
            )
            .is_ok()
        {
            Some((MAINTENANCE_LOCK.lock(), runtime.lock()))
        } else {
            None
        };
        let response = if shutdown {
            if shutdown_guard.is_some() {
                Response::Ok
            } else {
                Response::Err(EngineError::Failed(
                    "Shutdown requires an authenticated caller under the KeyTao app or runtime root".into(),
                ))
            }
        } else if matches!(
            request.message,
            Request::Deploy | Request::DeployPackages { .. }
        ) {
            // Identification tokens' AppContainer flag cannot independently authorize deployment.
            if crate::authorization::deployment_allowed(
                role,
                client.is_appcontainer,
                installed_process(pid, &client.sid, &server.sid, &[keytao_app_root()]).is_ok(),
            ) {
                let package_plan = match &request.message {
                    Request::DeployPackages { plan } => Some(plan.as_str()),
                    _ => None,
                };
                Response::Deployed(deploy(runtime, package_plan, |progress| {
                    if package_plan.is_some() {
                        // A disconnected App must not interrupt the deployment or engine reload.
                        let _ = write_frame(
                            &mut file,
                            &request.reply(Response::DeployProgress(progress.clone())),
                        );
                    }
                }))
            } else {
                Response::Deployed(Err(
                    "Deploy requires an authenticated App caller under the KeyTao app root".into(),
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
            let result = loop {
                let current = runtime.lock();
                if current.deploying && current.ready {
                    // Staging keeps the live engine available to new sessions too.
                    break current.session();
                }
                drop(current);
                let Some(_maintenance) = MAINTENANCE_LOCK.try_lock_for(KEY_WAIT_BUDGET) else {
                    continue;
                };
                break refresh(runtime).and_then(|_| runtime.lock().session());
            };
            match result {
                Ok((engine, generation, schema_id)) => {
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
            let Some(runtime) = runtime.try_lock() else {
                return Response::Err(EngineError::Busy);
            };
            Response::Generation(runtime.generation)
        }
        Request::Hello { .. } => Response::HelloErr("connection is already initialized".into()),
        // Privileged requests are authorized against the authenticated process in serve.
        Request::Deploy | Request::DeployPackages { .. } => {
            Response::Deployed(Err("Deploy requires caller authorization".into()))
        }
        Request::Shutdown => Response::Err(EngineError::Failed(
            "Shutdown requires caller authorization".into(),
        )),
        request => {
            let Some(runtime) = lock_for_key(runtime) else {
                return Response::Err(EngineError::Busy);
            };
            if let Some(error) = runtime.key_error(session.generation) {
                return Response::Err(error);
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
                Request::SetContextAsciiMode { enabled } => {
                    Response::State(engine.set_context_ascii_mode(*enabled))
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
    fn reload_notification_rearms_after_stamp_changes() {
        let dir = std::env::temp_dir().join(format!(
            "keytao-reload-notification-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let notification = ReloadNotification::new(&dir).unwrap();
        for contents in ["first", "second"] {
            std::fs::write(keytao_core::ReloadStamp::path(&dir), contents).unwrap();
            assert_eq!(
                unsafe { WaitForSingleObject(notification.0, 5_000) },
                WAIT_OBJECT_0
            );
            assert!(notification.wait());
        }
        drop(notification);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(ReloadNotification::new(&dir).is_none());
    }

    #[test]
    fn requests_cannot_use_an_uncreated_session() {
        let runtime = Mutex::new(Runtime::default());
        let mut session = ClientSession::default();
        for request in [
            Request::ProcessEnter,
            Request::Reset,
            Request::GetState,
            Request::SetContextAsciiMode {
                enabled: Some(true),
            },
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
        for request in [
            Request::Deploy,
            Request::DeployPackages { plan: "[]".into() },
        ] {
            assert!(matches!(
                dispatch(
                    &request,
                    &mut ClientSession::default(),
                    &Mutex::new(Runtime::default())
                ),
                Response::Deployed(Err(_))
            ));
        }
    }

    #[test]
    fn generation_does_not_build_or_wait_for_a_runtime() {
        let runtime = Mutex::new(Runtime::default());
        let guard = runtime.lock();
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
        assert!(runtime.lock().engine.is_none());
    }

    #[test]
    fn initial_generation_does_not_swallow_a_racing_reload() {
        let runtime = Mutex::new(Runtime {
            engine: None,
            generation: 8,
            ready: false,
            deploying: false,
            refreshing: false,
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
            engine: Some(Arc::new(InProcEngine::default())),
            generation: 8,
            ready: true,
            deploying: false,
            refreshing: false,
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
        runtime.lock().ready = false;
        assert!(matches!(
            dispatch(&request, &mut session, &runtime),
            Response::Err(EngineError::NotReady)
        ));
    }
}
