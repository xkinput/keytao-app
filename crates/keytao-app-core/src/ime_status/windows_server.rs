//! Installed IME server lifecycle and App-only deployment RPCs.

use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    os::windows::{io::AsRawHandle, process::CommandExt},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use keytao_ime_ipc::{
    ClientRole, EngineMode, PROTOCOL_VERSION, Request, RequestEnvelope, Response, read_response,
    write_frame,
};
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_NO_DATA, ERROR_PIPE_BUSY, HANDLE, HLOCAL,
            LocalFree,
        },
        Security::{
            Authorization::ConvertSidToStringSidW, GetTokenInformation, TOKEN_QUERY, TOKEN_USER,
            TokenUser,
        },
        System::{
            Pipes::{GetNamedPipeServerProcessId, PIPE_NOWAIT, SetNamedPipeHandleState},
            Registry::{
                HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, KEY_SET_VALUE,
                REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_ANY, RRF_RT_REG_DWORD,
                RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY, RegCloseKey, RegCreateKeyExW, RegGetValueW,
                RegSetValueExW,
            },
            RemoteDesktop::ProcessIdToSessionId,
            Threading::{
                CREATE_NO_WINDOW, DETACHED_PROCESS, GetCurrentProcess, OpenProcess,
                OpenProcessToken, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
                QueryFullProcessImageNameW,
            },
        },
    },
    core::{PCWSTR, PWSTR, w},
};

const SERVER_EXE: &str = "keytao-windows-ime-server.exe";
const START_TIMEOUT: Duration = Duration::from_secs(5);
const DEPLOY_TIMEOUT: Duration = Duration::from_secs(120);
static WATCHDOG_STARTED: AtomicBool = AtomicBool::new(false);
static LAST_START: Mutex<Option<(PathBuf, Instant)>> = Mutex::new(None);
static LAST_RUN_VALUE: Mutex<Option<PathBuf>> = Mutex::new(None);

pub(super) fn start_watchdog() {
    if !server_enabled() || WATCHDOG_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    if let Err(error) = std::thread::Builder::new()
        .name("keytao-ime-server-watchdog".into())
        .spawn(|| {
            loop {
                if let Err(error) = ensure_started() {
                    tracing::warn!(%error, "IME server watchdog failed");
                }
                std::thread::sleep(Duration::from_secs(5));
            }
        })
    {
        WATCHDOG_STARTED.store(false, Ordering::Release);
        tracing::warn!(%error, "could not start IME server watchdog");
    }
}

fn ensure_started() -> Result<(), String> {
    if !server_enabled() {
        return Ok(());
    }
    let mut last_start = LAST_START.lock().unwrap_or_else(|error| error.into_inner());
    let server = installed_server_exe()?;
    {
        let mut last_run_value = LAST_RUN_VALUE
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !last_run_value
            .as_ref()
            .is_some_and(|path| same_path(path, &server))
        {
            match write_run_value(&server) {
                Ok(()) => *last_run_value = Some(server.clone()),
                Err(error) => tracing::warn!(%error, "could not register IME server startup"),
            }
        }
    }
    let pipe = pipe_name()?;
    match OpenOptions::new().read(true).write(true).open(&pipe) {
        Ok(file) => {
            let current = server_image(&file)?;
            if same_path(&current, &server) {
                return Ok(());
            }
            // A newly installed server asks the old version to shut down.
        }
        Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY.0 as i32) => return Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("inspect IME server pipe: {error}")),
    }
    if last_start.as_ref().is_some_and(|(path, started)| {
        same_path(path, &server) && started.elapsed() < START_TIMEOUT
    }) {
        return Ok(());
    }
    Command::new(&server)
        .creation_flags(CREATE_NO_WINDOW.0 | DETACHED_PROCESS.0)
        .spawn()
        .map_err(|error| format!("start installed IME server: {error}"))?;
    *last_start = Some((server, Instant::now()));
    Ok(())
}

fn installed_server_exe() -> Result<PathBuf, String> {
    let root = registry_path(w!("SOFTWARE\\KeyTao"), w!("WindowsImeRuntimeDir"))?;
    let architectures = if super::windows_native_machine() == super::WINDOWS_PE_MACHINE_ARM64 {
        ["arm64x", "current", "x64"]
    } else {
        ["current", "x64", "arm64x"]
    };
    for path in architectures
        .into_iter()
        .map(|arch| root.join(arch).join(SERVER_EXE))
        .chain(std::iter::once(root.join(SERVER_EXE)))
    {
        if path.is_file() {
            let path = path.canonicalize().map_err(|error| error.to_string())?;
            verify_installation_path(&path)?;
            return Ok(path);
        }
    }
    Err("installed IME server is missing from WindowsImeRuntimeDir".into())
}

pub(super) fn installed_runtime_dir() -> Option<PathBuf> {
    installed_server_exe().ok()?.parent().map(Path::to_path_buf)
}

pub(super) fn program_files_dir() -> Result<PathBuf, String> {
    registry_path(
        w!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion"),
        w!("ProgramFilesDir"),
    )
}

fn registry_path(key: PCWSTR, name: PCWSTR) -> Result<PathBuf, String> {
    let mut value = vec![0u16; 32768];
    let mut size = (value.len() * 2) as u32;
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key,
            name,
            RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
            None,
            Some(value.as_mut_ptr().cast()),
            Some(&mut size),
        )
        .ok()
    }
    .map_err(|error| format!("read installed IME runtime registry: {error}"))?;
    if size < 4 || size as usize > value.len() * 2 || size % 2 != 0 {
        return Err("invalid installed IME runtime registry path".into());
    }
    let value = &value[..size as usize / 2];
    let end = value
        .iter()
        .position(|unit| *unit == 0)
        .ok_or("unterminated installed IME runtime registry path")?;
    let path = PathBuf::from(String::from_utf16(&value[..end]).map_err(|error| error.to_string())?);
    if !path.is_absolute() {
        return Err("installed IME runtime registry path is not absolute".into());
    }
    Ok(path)
}

fn server_enabled() -> bool {
    let mut value = 0u32;
    let mut size = std::mem::size_of_val(&value) as u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\KeyTao\\Ime"),
            w!("EngineMode"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
    };
    EngineMode::from_registry(
        (result.is_ok() && size == 4).then_some((REG_DWORD.0, value.to_le_bytes().as_slice())),
    ) == EngineMode::Pipe
}

fn initialize_engine_mode() -> Result<(), String> {
    let mut key = HKEY::default();
    unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            w!("Software\\KeyTao\\Ime"),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_QUERY_VALUE | KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
        .ok()
    }
    .map_err(|error| format!("open IME engine mode key: {error}"))?;
    let result = unsafe {
        let existing = RegGetValueW(
            key,
            PCWSTR::null(),
            w!("EngineMode"),
            RRF_RT_ANY,
            None,
            None,
            None,
        );
        if existing == ERROR_FILE_NOT_FOUND {
            RegSetValueExW(
                key,
                w!("EngineMode"),
                0,
                REG_DWORD,
                Some(&1u32.to_le_bytes()),
            )
            .ok()
        } else {
            existing.ok()
        }
    };
    unsafe {
        let _ = RegCloseKey(key);
    }
    result.map_err(|error| format!("initialize IME engine mode: {error}"))
}

fn write_run_value(server: &Path) -> Result<(), String> {
    initialize_engine_mode()?;
    let mut key = HKEY::default();
    unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
        .ok()
    }
    .map_err(|error| format!("open IME server Run key: {error}"))?;
    let command = format!("\"{}\"", super::windows_normal_path(server));
    let bytes: Vec<_> = command
        .encode_utf16()
        .chain(Some(0))
        .flat_map(u16::to_le_bytes)
        .collect();
    let result =
        unsafe { RegSetValueExW(key, w!("KeyTaoImeServer"), 0, REG_SZ, Some(&bytes)).ok() };
    unsafe {
        let _ = RegCloseKey(key);
    }
    result.map_err(|error| format!("write IME server Run value: {error}"))
}

fn pipe_name() -> Result<String, String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
            .map_err(|error| error.to_string())?;
        let result = (|| {
            let mut size = 0;
            let _ = GetTokenInformation(token, TokenUser, None, 0, &mut size);
            if size == 0 {
                return Err("empty current user token".into());
            }
            let mut buffer = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
            GetTokenInformation(
                token,
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                size,
                &mut size,
            )
            .map_err(|error| error.to_string())?;
            let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
            let mut text = PWSTR::null();
            ConvertSidToStringSidW(user.User.Sid, &mut text).map_err(|error| error.to_string())?;
            let sid = text.to_string();
            LocalFree(HLOCAL(text.0.cast()));
            let sid = sid.map_err(|error| error.to_string())?;
            let mut session = 0;
            ProcessIdToSessionId(std::process::id(), &mut session)
                .map_err(|error| error.to_string())?;
            Ok(keytao_ime_ipc::pipe_name(&sid, session))
        })();
        let _ = CloseHandle(token);
        result
    }
}

fn server_image(file: &File) -> Result<PathBuf, String> {
    unsafe {
        let mut pid = 0;
        GetNamedPipeServerProcessId(HANDLE(file.as_raw_handle()), &mut pid)
            .map_err(|error| error.to_string())?;
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
            .map_err(|error| format!("open IME server process: {error}"))?;
        let mut image = vec![0u16; 32768];
        let mut size = image.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(image.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(process);
        result.map_err(|error| format!("read IME server image: {error}"))?;
        let image = PathBuf::from(
            String::from_utf16(&image[..size as usize]).map_err(|error| error.to_string())?,
        )
        .canonicalize()
        .map_err(|error| error.to_string())?;
        verify_installation_path(&image)?;
        Ok(image)
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    left.components().count() == right.components().count() && starts_with_ci(left, right)
}

fn starts_with_ci(path: &Path, prefix: &Path) -> bool {
    let mut components = path.components();
    prefix.components().all(|prefix| {
        components.next().is_some_and(|path| {
            path.as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&prefix.as_os_str().to_string_lossy())
        })
    })
}

fn verify_installation_path(path: &Path) -> Result<(), String> {
    let root = program_files_dir()?
        .join("KeyTao")
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if path.components().count() > root.components().count() && starts_with_ci(path, &root) {
        Ok(())
    } else {
        Err("IME server image is outside Program Files\\KeyTao".into())
    }
}

/// False means no server connection could be opened; never retry an uncertain deploy in-process.
pub(super) fn deploy_if_available() -> Result<bool, String> {
    if !server_enabled() {
        return Ok(false);
    }
    start_watchdog();
    if let Err(error) = ensure_started() {
        tracing::warn!(%error, "IME server startup before deploy failed");
    }
    let pipe = pipe_name()?;
    let deadline = Instant::now() + START_TIMEOUT;
    let mut replacing_server = false;
    let file = loop {
        match OpenOptions::new().read(true).write(true).open(&pipe) {
            Ok(file) => {
                let image = server_image(&file)?;
                if same_path(&image, &installed_server_exe()?) {
                    break file;
                }
                replacing_server = true;
                drop(file);
                if Instant::now() >= deadline {
                    return Err("current IME server did not become available for deployment".into());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND.0 as i32) => {
                if Instant::now() >= deadline {
                    return if replacing_server {
                        Err("replacement IME server did not become available for deployment".into())
                    } else {
                        Ok(false)
                    };
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY.0 as i32) => {
                if Instant::now() >= deadline {
                    return Err("IME server deployment pipe remains busy".into());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(format!("connect to IME server for deploy: {error}")),
        }
    };
    unsafe {
        SetNamedPipeHandleState(HANDLE(file.as_raw_handle()), Some(&PIPE_NOWAIT), None, None)
    }
    .map_err(|error| format!("set deployment pipe deadline mode: {error}"))?;
    let mut pipe = DeadlinePipe {
        file,
        deadline: Instant::now() + START_TIMEOUT,
    };
    let hello = RequestEnvelope {
        seq: 1,
        message: Request::Hello {
            protocol: PROTOCOL_VERSION,
            client_build: env!("CARGO_PKG_VERSION").into(),
            role: ClientRole::App,
            is_appcontainer: false,
        },
    };
    write_frame(&mut pipe, &hello).map_err(|error| format!("send IME server hello: {error}"))?;
    match read_response(&mut pipe, hello.seq)
        .map_err(|error| format!("read IME server hello: {error}"))?
    {
        Response::HelloOk { protocol, .. } if protocol == PROTOCOL_VERSION => {}
        response => {
            return Err(format!(
                "IME server deployment handshake rejected: {response:?}"
            ));
        }
    }
    pipe.deadline = Instant::now() + DEPLOY_TIMEOUT;
    write_frame(
        &mut pipe,
        &RequestEnvelope {
            seq: 2,
            message: Request::Deploy,
        },
    )
    .map_err(|error| format!("send IME server deploy: {error}"))?;
    match read_response(&mut pipe, 2).map_err(|error| format!("read IME server deploy: {error}"))? {
        Response::Deployed(result) => result.map(|()| true),
        response => Err(format!(
            "unexpected IME server deploy response: {response:?}"
        )),
    }
}

// PIPE_NOWAIT keeps each OS call bounded; framing retries only until the RPC deadline.
struct DeadlinePipe {
    file: File,
    deadline: Instant,
}

impl DeadlinePipe {
    fn wait(&self) -> io::Result<()> {
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "IME server RPC timed out",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
        Ok(())
    }
}

impl Read for DeadlinePipe {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            if Instant::now() >= self.deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "IME server RPC timed out",
                ));
            }
            match self.file.read(buffer) {
                Err(error) if error.raw_os_error() == Some(ERROR_NO_DATA.0 as i32) => {
                    self.wait()?
                }
                result => return result,
            }
        }
    }
}

impl Write for DeadlinePipe {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        loop {
            if Instant::now() >= self.deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "IME server RPC timed out",
                ));
            }
            match self.file.write(buffer) {
                Ok(0) if !buffer.is_empty() => self.wait()?,
                Err(error) if error.raw_os_error() == Some(ERROR_NO_DATA.0 as i32) => {
                    self.wait()?
                }
                result => return result,
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
