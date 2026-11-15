use keytao_core::runtime_log::{self, Level};
use keytao_ime_ipc::{
    negotiate_protocol, read_frame, write_frame, EngineError, Request, RequestEnvelope, Response,
};
use keytao_windows_ime::{
    engine_backend::{ImeEngineBackend, InProcEngine},
    engine_status,
    pipe_security::{current_identity, process_identity, Identity, OwnedHandle},
};
use std::{
    fs::File,
    os::windows::io::FromRawHandle,
    sync::{Arc, Mutex, TryLockError},
    time::{Duration, Instant},
};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{LocalFree, ERROR_PIPE_CONNECTED, HLOCAL},
        Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
            },
            PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
        },
        Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX},
        System::Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId,
            PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
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

fn create_pipe(name: &str, sid: &str, first: bool) -> Result<OwnedHandle, String> {
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
    }
    .map_err(|e| e.to_string())?;
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
        return Err(format!(
            "create exclusive IME pipe: {}",
            windows::core::Error::from_win32()
        ));
    }
    Ok(OwnedHandle(handle))
}

#[derive(Default)]
struct Runtime {
    engine: Option<InProcEngine>,
    generation: u64,
    ready: bool,
}

impl Runtime {
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
    let mut listener = create_pipe(&name, &identity.sid, true)?;
    let watched_runtime = Arc::clone(&runtime);
    std::thread::Builder::new()
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
        })
        .map_err(|e| e.to_string())?;
    loop {
        let connected = unsafe { ConnectNamedPipe(listener.0, None) };
        // Create the next instance before handing off the last one, retaining ownership of the name.
        let next = create_pipe(&name, &identity.sid, false)?;
        let connection = std::mem::replace(&mut listener, next);
        if let Err(error) = connected {
            if error.code() != ERROR_PIPE_CONNECTED.to_hresult() {
                keytao_core::rt_log!(Level::Info, "ipc", "accept_failed", msg = error.to_string());
                continue;
            }
        }
        let identity = identity.clone();
        let runtime = runtime.clone();
        std::thread::Builder::new()
            .name("ime-pipe-client".into())
            .spawn(move || {
                if let Err(error) = serve(connection, &identity, &runtime) {
                    keytao_core::rt_log!(Level::Info, "ipc", "connection_closed", msg = error);
                }
            })
            .map_err(|e| e.to_string())?;
    }
}

fn serve(pipe: OwnedHandle, server: &Identity, runtime: &Mutex<Runtime>) -> Result<(), String> {
    let mut pid = 0;
    unsafe { GetNamedPipeClientProcessId(pipe.0, &mut pid) }.map_err(|e| e.to_string())?;
    let client = process_identity(pid)?;
    if client.session_id != server.session_id || client.sid != server.sid {
        return Err("pipe client user or session does not match".into());
    }
    keytao_core::rt_log!(
        Level::Info,
        "ipc",
        "client_authenticated",
        pid = pid,
        is_appcontainer = client.is_appcontainer
    );
    let mut file = unsafe { File::from_raw_handle(pipe.0 .0) };
    std::mem::forget(pipe);
    let hello: RequestEnvelope = read_frame(&mut file).map_err(|e| e.to_string())?;
    let response = match &hello.message {
        Request::Hello { protocol, .. } => negotiate_protocol(*protocol, env!("CARGO_PKG_VERSION")),
        _ => Response::HelloErr("expected Hello".into()),
    };
    write_frame(&mut file, &hello.reply(response.clone())).map_err(|e| e.to_string())?;
    if !matches!(response, Response::HelloOk { .. }) {
        return Ok(());
    }
    let mut session = ClientSession::default();
    let mut sequence = hello.seq;
    loop {
        let request: RequestEnvelope = read_frame(&mut file).map_err(|e| e.to_string())?;
        if request.seq <= sequence {
            return Err("non-increasing request sequence".into());
        }
        sequence = request.seq;
        let response = dispatch(&request.message, &mut session, runtime);
        write_frame(&mut file, &request.reply(response)).map_err(|e| e.to_string())?;
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
            let result = runtime.lock().unwrap_or_else(|p| p.into_inner()).session();
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
        // Deployment remains in the App until Batch 3, regardless of the claimed client role.
        Request::Deploy => Response::Deployed(Err("Deploy is unavailable in Batch 2".into())),
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
    fn batch_two_never_accepts_deployment() {
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
