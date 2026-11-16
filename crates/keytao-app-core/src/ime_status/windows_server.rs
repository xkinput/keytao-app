//! Installed IME server lifecycle and App-only deployment RPCs.

use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle, process::CommandExt},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use super::server_recovery::{HelloFailure, can_manage_server};
use keytao_ime_ipc::{
    ClientRole, EngineMode, PROTOCOL_VERSION, Request, RequestEnvelope, Response, read_response,
    write_frame,
};
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_NO_DATA, ERROR_PIPE_BUSY, HANDLE, HLOCAL,
            LocalFree, WAIT_OBJECT_0,
        },
        Security::{
            AdjustTokenPrivileges, Authorization::ConvertSidToStringSidW, DuplicateTokenEx,
            GetTokenInformation, LookupPrivilegeValueW, SE_IMPERSONATE_NAME, SE_PRIVILEGE_ENABLED,
            SecurityImpersonation, TOKEN_ADJUST_DEFAULT, TOKEN_ADJUST_PRIVILEGES,
            TOKEN_ADJUST_SESSIONID, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_ELEVATION,
            TOKEN_PRIVILEGES, TOKEN_QUERY, TOKEN_USER, TokenElevation, TokenPrimary, TokenUser,
        },
        System::{
            Com::{CLSCTX_LOCAL_SERVER, CoCreateInstance, IDispatch, IServiceProvider},
            Pipes::{GetNamedPipeServerProcessId, PIPE_NOWAIT, SetNamedPipeHandleState},
            Registry::{
                HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, KEY_SET_VALUE,
                REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_ANY, RRF_RT_REG_DWORD,
                RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY, RegCloseKey, RegCreateKeyExW, RegGetValueW,
                RegSetValueExW,
            },
            RemoteDesktop::ProcessIdToSessionId,
            Threading::{
                CREATE_NO_WINDOW, CREATE_PROCESS_LOGON_FLAGS, CreateProcessWithTokenW,
                DETACHED_PROCESS, GetCurrentProcess, OpenProcess, OpenProcessToken,
                PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
                QueryFullProcessImageNameW, STARTF_USESHOWWINDOW, STARTUPINFOW, TerminateProcess,
                WaitForSingleObject,
            },
        },
        UI::{
            Shell::{
                CSIDL_DESKTOP, IShellBrowser, IShellDispatch2, IShellFolderViewDual, IShellWindows,
                SID_STopLevelBrowser, SVGIO_BACKGROUND, SWC_DESKTOP, SWFO_NEEDDISPATCH,
                ShellWindows,
            },
            WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId, SW_HIDE},
        },
    },
    core::{BSTR, Interface, PCWSTR, PWSTR, VARIANT, w},
};

const SERVER_EXE: &str = "keytao-windows-ime-server.exe";
const START_TIMEOUT: Duration = Duration::from_secs(5);
const DEPLOY_TIMEOUT: Duration = Duration::from_secs(120);
// Win32 SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION; never grant impersonation rights.
const PIPE_SECURITY_QOS: u32 = 0x0010_0000 | 0x0001_0000;
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
    match open_pipe(&pipe) {
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
    start_server(&server, &mut last_start, &ServerLauncher::prepare()?)
}

struct ServerLauncher {
    shell_token: Option<ProcessHandle>,
}

impl ServerLauncher {
    fn prepare() -> Result<Self, String> {
        let elevated = app_is_elevated()?;
        let shell_token = elevated.then(shell_primary_token).transpose();
        if !can_manage_server(elevated, shell_token.is_ok()) {
            return Err(elevation_launch_error(shell_token.err().unwrap()));
        }
        Ok(Self {
            shell_token: shell_token?,
        })
    }
}

fn elevation_launch_error(error: String) -> String {
    format!(
        "无法使用交互式桌面权限启动输入法服务：{error}；当前 KeyTao 以管理员权限运行，请关闭后正常启动 KeyTao（不要以管理员身份运行），再重试部署。"
    )
}

fn shell_primary_token() -> Result<ProcessHandle, String> {
    unsafe {
        let shell = GetShellWindow();
        if shell.0.is_null() {
            return Err("interactive shell window is unavailable".into());
        }
        let mut pid = 0;
        if GetWindowThreadProcessId(shell, Some(&mut pid)) == 0 || pid == 0 {
            return Err("interactive shell process is unavailable".into());
        }
        let process = ProcessHandle(
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
                .map_err(|error| format!("open interactive shell process: {error}"))?,
        );
        if process_session(pid)? != process_session(std::process::id())?
            || process_sid(process.0)? != process_sid(GetCurrentProcess())?
        {
            return Err("interactive shell user or session does not match KeyTao".into());
        }
        let mut token = HANDLE::default();
        OpenProcessToken(process.0, TOKEN_DUPLICATE, &mut token)
            .map_err(|error| format!("open interactive shell token: {error}"))?;
        let token = ProcessHandle(token);
        let mut primary = HANDLE::default();
        // Preserve the shell's token even when UAC is disabled and the shell itself is elevated.
        DuplicateTokenEx(
            token.0,
            TOKEN_QUERY
                | TOKEN_ASSIGN_PRIMARY
                | TOKEN_DUPLICATE
                | TOKEN_ADJUST_DEFAULT
                | TOKEN_ADJUST_SESSIONID,
            None,
            SecurityImpersonation,
            TokenPrimary,
            &mut primary,
        )
        .map_err(|error| format!("duplicate interactive shell token: {error}"))?;
        Ok(ProcessHandle(primary))
    }
}

fn enable_impersonate_privilege() -> windows::core::Result<()> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES, &mut token)?;
        let token = ProcessHandle(token);
        let mut privileges = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            ..Default::default()
        };
        LookupPrivilegeValueW(
            PCWSTR::null(),
            SE_IMPERSONATE_NAME,
            &mut privileges.Privileges[0].Luid,
        )?;
        privileges.Privileges[0].Attributes = SE_PRIVILEGE_ENABLED;
        AdjustTokenPrivileges(token.0, false, Some(&privileges), 0, None, None)
    }
}

fn start_server_via_explorer(path: &str, directory: &str) -> Result<(), String> {
    let _com = super::windows_init_com_apartment()?;
    (|| unsafe {
        let shell_windows: IShellWindows =
            CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER)?;
        let mut hwnd = 0;
        let desktop = shell_windows.FindWindowSW(
            &VARIANT::from(CSIDL_DESKTOP as i32),
            &VARIANT::default(),
            SWC_DESKTOP,
            &mut hwnd,
            SWFO_NEEDDISPATCH,
        )?;
        let provider: IServiceProvider = desktop.cast()?;
        let browser: IShellBrowser = provider.QueryService(&SID_STopLevelBrowser)?;
        let view = browser.QueryActiveShellView()?;
        let background: IDispatch = view.GetItemObject(SVGIO_BACKGROUND)?;
        let folder: IShellFolderViewDual = background.cast()?;
        let shell: IShellDispatch2 = folder.Application()?.cast()?;
        shell.ShellExecute(
            &BSTR::from(path),
            &VARIANT::from(""),
            &VARIANT::from(directory),
            &VARIANT::from("open"),
            &VARIANT::from(SW_HIDE.0),
        )
    })()
    .map_err(|error: windows::core::Error| error.to_string())
}

fn start_server(
    server: &Path,
    last_start: &mut Option<(PathBuf, Instant)>,
    launcher: &ServerLauncher,
) -> Result<(), String> {
    if let Some(token) = &launcher.shell_token {
        let path = super::windows_normal_path(server);
        let directory = super::windows_normal_path(
            server
                .parent()
                .ok_or("installed IME server has no parent directory")?,
        );
        let application: Vec<_> = path.encode_utf16().chain(Some(0)).collect();
        let current_directory: Vec<_> = directory.encode_utf16().chain(Some(0)).collect();
        let mut command: Vec<_> = format!("\"{path}\"")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let startup = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            dwFlags: STARTF_USESHOWWINDOW,
            wShowWindow: SW_HIDE.0 as u16,
            ..Default::default()
        };
        let mut process = PROCESS_INFORMATION::default();
        let _ = enable_impersonate_privilege();
        let result = unsafe {
            CreateProcessWithTokenW(
                token.0,
                // The matching interactive shell already has the user's profile loaded.
                CREATE_PROCESS_LOGON_FLAGS(0),
                PCWSTR(application.as_ptr()),
                PWSTR(command.as_mut_ptr()),
                // The server uses the Windows subsystem and never creates a console.
                PROCESS_CREATION_FLAGS(0),
                None,
                PCWSTR(current_directory.as_ptr()),
                &startup,
                &mut process,
            )
        };
        match result {
            Ok(()) => {
                drop(ProcessHandle(process.hThread));
                drop(ProcessHandle(process.hProcess));
            }
            Err(error) => {
                start_server_via_explorer(&path, &directory).map_err(|shell_error| {
                    elevation_launch_error(format!(
                        "start installed IME server with shell token: {error}; start installed IME server via Explorer: {shell_error}"
                    ))
                })?;
            }
        }
    } else {
        Command::new(server)
            .creation_flags(CREATE_NO_WINDOW.0 | DETACHED_PROCESS.0)
            .spawn()
            .map_err(|error| format!("start installed IME server: {error}"))?;
    }
    *last_start = Some((server.to_path_buf(), Instant::now()));
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
    Ok(keytao_ime_ipc::pipe_name(
        &process_sid(unsafe { GetCurrentProcess() })?,
        process_session(std::process::id())?,
    ))
}

fn process_session(pid: u32) -> Result<u32, String> {
    let mut session = 0;
    unsafe { ProcessIdToSessionId(pid, &mut session) }.map_err(|error| error.to_string())?;
    Ok(session)
}

fn process_sid(process: HANDLE) -> Result<String, String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token).map_err(|error| error.to_string())?;
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
            sid.map_err(|error| error.to_string())
        })();
        let _ = CloseHandle(token);
        result
    }
}

fn server_image(file: &File) -> Result<PathBuf, String> {
    Ok(verified_server(file)?.image)
}

struct ProcessHandle(HANDLE);

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

struct VerifiedServer {
    process: ProcessHandle,
    pid: u32,
    image: PathBuf,
}

fn verified_server(file: &File) -> Result<VerifiedServer, String> {
    unsafe {
        let mut pid = 0;
        GetNamedPipeServerProcessId(HANDLE(file.as_raw_handle()), &mut pid)
            .map_err(|error| error.to_string())?;
        let process = ProcessHandle(
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                false,
                pid,
            )
            .map_err(|error| format!("open IME server process: {error}"))?,
        );
        if process_session(pid)? != process_session(std::process::id())? {
            return Err("IME server session does not match".into());
        }
        let mut image = vec![0u16; 32768];
        let mut size = image.len() as u32;
        let result = QueryFullProcessImageNameW(
            process.0,
            PROCESS_NAME_WIN32,
            PWSTR(image.as_mut_ptr()),
            &mut size,
        );
        result.map_err(|error| format!("read IME server image: {error}"))?;
        let image = PathBuf::from(
            String::from_utf16(&image[..size as usize]).map_err(|error| error.to_string())?,
        )
        .canonicalize()
        .map_err(|error| error.to_string())?;
        verify_installation_path(&image)?;
        if !image
            .file_name()
            .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(SERVER_EXE))
        {
            return Err("pipe peer is not the installed IME server executable".into());
        }
        Ok(VerifiedServer {
            process,
            pid,
            image,
        })
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

/// False permits in-process deployment only in InProc mode with no server pipe.
pub(super) fn deploy_if_available() -> Result<bool, String> {
    if !server_enabled() {
        super::server_recovery::require_absent_server(open_pipe(&pipe_name()?).map(|_| ()))?;
        return Ok(false);
    }
    start_watchdog();
    // Serialize recovery/deploy with this App's watchdog, including the full deployment RPC.
    let mut last_start = LAST_START.lock().unwrap_or_else(|error| error.into_inner());
    deploy_via_server(&mut last_start)
}

fn open_pipe(name: &str) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .security_qos_flags(PIPE_SECURITY_QOS)
        .open(name)
}

struct ServerConnection {
    pipe: DeadlinePipe,
    peer: VerifiedServer,
}

fn connect_for_deploy(
    name: &str,
    last_start: &mut Option<(PathBuf, Instant)>,
    launcher: &Result<ServerLauncher, String>,
) -> Result<Option<(ServerConnection, bool)>, String> {
    let deadline = Instant::now() + START_TIMEOUT;
    let mut started = false;
    let file = loop {
        match open_pipe(name) {
            Ok(file) => break file,
            Err(error) if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND.0 as i32) => {
                if !started {
                    let launcher = launcher.as_ref().map_err(Clone::clone)?;
                    start_server(&installed_server_exe()?, last_start, launcher)?;
                    started = true;
                }
                if Instant::now() >= deadline {
                    return Err("IME server did not become available for deployment; in-process deployment is disabled in Pipe mode".into());
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
    let peer = verified_server(&file)?;
    let current = same_path(&peer.image, &installed_server_exe()?);
    unsafe {
        SetNamedPipeHandleState(HANDLE(file.as_raw_handle()), Some(&PIPE_NOWAIT), None, None)
    }
    .map_err(|error| format!("set deployment pipe deadline mode: {error}"))?;
    Ok(Some((
        ServerConnection {
            pipe: DeadlinePipe {
                file,
                deadline: Instant::now() + START_TIMEOUT,
                bytes_read: 0,
            },
            peer,
        },
        current,
    )))
}

fn hello(pipe: &mut DeadlinePipe) -> Result<(), HelloFailure> {
    let hello = RequestEnvelope {
        seq: 1,
        message: Request::Hello {
            protocol: PROTOCOL_VERSION,
            client_build: env!("CARGO_PKG_VERSION").into(),
            role: ClientRole::App,
            is_appcontainer: false,
        },
    };
    write_frame(pipe, &hello)
        .map_err(|error| HelloFailure::other(format!("send IME server hello: {error}")))?;
    let bytes_read = pipe.bytes_read;
    match read_response(pipe, hello.seq)
        .map_err(|error| HelloFailure::read(error, pipe.bytes_read - bytes_read))?
    {
        Response::HelloOk { protocol, .. } if protocol == PROTOCOL_VERSION => Ok(()),
        Response::HelloErr(reason) => Err(HelloFailure::other(format!(
            "IME server deployment handshake rejected: {reason}"
        ))),
        response => Err(HelloFailure::other(format!(
            "IME server deployment handshake rejected: {response:?}"
        ))),
    }
}

fn replace_before_deploy(
    connection: ServerConnection,
    authenticated: bool,
    name: &str,
) -> Result<(), String> {
    let ServerConnection { mut pipe, peer } = connection;
    tracing::warn!(server_pid = peer.pid, image = %peer.image.display(), authenticated, "recovering IME server before Deploy");
    if authenticated {
        // Shutdown waits behind another caller's deploy; a timeout must not become a forced kill.
        pipe.deadline = Instant::now() + DEPLOY_TIMEOUT;
        write_frame(
            &mut pipe,
            &RequestEnvelope {
                seq: 2,
                message: Request::Shutdown,
            },
        )
        .map_err(|error| format!("send IME server shutdown: {error}"))?;
        match read_response(&mut pipe, 2)
            .map_err(|error| format!("read IME server shutdown: {error}"))?
        {
            Response::Ok => {}
            response => return Err(format!("IME server rejected shutdown: {response:?}")),
        }
    } else {
        // Recovery permits termination only for a stale image or EOF before any Hello reply.
        // Holding the original process handle prevents PID reuse before the terminate handle opens.
        if unsafe { WaitForSingleObject(peer.process.0, 0) } != WAIT_OBJECT_0 {
            let process = ProcessHandle(
                unsafe { OpenProcess(PROCESS_TERMINATE, false, peer.pid) }
                    .map_err(|error| format!("open verified IME server for recovery: {error}"))?,
            );
            unsafe { TerminateProcess(process.0, 1) }
                .map_err(|error| format!("stop verified IME server before deploy: {error}"))?;
        }
    }
    if unsafe { WaitForSingleObject(peer.process.0, START_TIMEOUT.as_millis() as u32) }
        != WAIT_OBJECT_0
    {
        return Err("old IME server has not exited; deployment was not sent".into());
    }
    drop(pipe);
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        match open_pipe(name) {
            Err(error) if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND.0 as i32) => {
                return Ok(());
            }
            Ok(file) => {
                let replacement = verified_server(&file)?;
                if replacement.pid != peer.pid {
                    // Another App may already have started the replacement after the old writer exited.
                    return Ok(());
                }
            }
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY.0 as i32) => {}
            Err(error) => return Err(format!("wait for old IME server pipe release: {error}")),
        }
        if Instant::now() >= deadline {
            return Err("old IME server pipe was not released; deployment was not sent".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn deploy_via_server(last_start: &mut Option<(PathBuf, Instant)>) -> Result<bool, String> {
    let name = pipe_name()?;
    // Prepare the launch token before allowing recovery to stop the existing writer.
    let launcher = ServerLauncher::prepare();
    super::server_recovery::deploy_once(
        launcher.as_ref().map(|_| ()).map_err(Clone::clone),
        || connect_for_deploy(&name, last_start, &launcher),
        |connection| {
            let result = hello(&mut connection.pipe);
            if let Err(error) = &result {
                tracing::warn!(server_pid = connection.peer.pid, error = %error.reason, "IME server handshake failed before Deploy");
            }
            result
        },
        |connection, authenticated| replace_before_deploy(connection, authenticated, &name),
        |mut connection| {
            let pipe = &mut connection.pipe;
            // No retry or in-process fallback from this point: a failed write may have sent Deploy.
            pipe.deadline = Instant::now() + DEPLOY_TIMEOUT;
            write_frame(
                pipe,
                &RequestEnvelope {
                    seq: 2,
                    message: Request::Deploy,
                },
            )
            .map_err(|error| format!("send IME server deploy: {error}"))?;
            match read_response(pipe, 2)
                .map_err(|error| format!("read IME server deploy: {error}"))?
            {
                Response::Deployed(result) => result,
                response => Err(format!(
                    "unexpected IME server deploy response: {response:?}"
                )),
            }
        },
    )
}

fn app_is_elevated() -> Result<bool, String> {
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
        .map_err(|error| error.to_string())?;
    let token = ProcessHandle(token);
    let mut elevation = TOKEN_ELEVATION::default();
    let mut size = 0;
    unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        )
    }
    .map_err(|error| error.to_string())?;
    Ok(elevation.TokenIsElevated != 0)
}

// PIPE_NOWAIT keeps each OS call bounded; framing retries only until the RPC deadline.
struct DeadlinePipe {
    file: File,
    deadline: Instant,
    bytes_read: usize,
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
                Ok(count) => {
                    self.bytes_read += count;
                    return Ok(count);
                }
                Err(error) => return Err(error),
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
