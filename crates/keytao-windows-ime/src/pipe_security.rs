//! Process/token identity checks for the per-user, per-session pipe.

use std::path::PathBuf;
use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL},
        Security::{
            Authorization::ConvertSidToStringSidW, GetTokenInformation, RevertToSelf,
            TokenIsAppContainer, TokenUser, TOKEN_QUERY, TOKEN_USER,
        },
        System::{
            Pipes::ImpersonateNamedPipeClient,
            Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY},
            RemoteDesktop::ProcessIdToSessionId,
            Threading::{
                GetCurrentProcess, GetCurrentThread, OpenProcess, OpenProcessToken,
                OpenThreadToken, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
    },
};

pub struct OwnedHandle(pub HANDLE);

// SAFETY: The owned kernel handle can be transferred between threads.
unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub struct Identity {
    pub sid: String,
    pub session_id: u32,
    pub is_appcontainer: bool,
}

pub fn current_identity() -> Result<Identity, String> {
    identity(unsafe { GetCurrentProcess() }, std::process::id())
}

pub fn process_identity(pid: u32) -> Result<Identity, String> {
    let process = OwnedHandle(
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
            .map_err(|e| format!("open pipe peer: {e}"))?,
    );
    identity(process.0, pid)
}

/// Call only after reading a client frame, which establishes the pipe's security context.
pub fn pipe_client_identity(pipe: HANDLE, pid: u32) -> Result<Identity, String> {
    struct RevertGuard;
    impl Drop for RevertGuard {
        fn drop(&mut self) {
            // Continuing under a client's token would cross the server's trust boundary.
            if unsafe { RevertToSelf() }.is_err() {
                std::process::abort();
            }
        }
    }

    let session_id = process_session_id(pid)?;
    unsafe { ImpersonateNamedPipeClient(pipe) }
        .map_err(|error| format!("identify pipe client: {error}"))?;
    let _revert = RevertGuard;
    let mut token = HANDLE::default();
    // Query as the server even when the pipe supplied an identification-level token.
    unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &mut token) }
        .map_err(|error| format!("open pipe client token: {error}"))?;
    let token = OwnedHandle(token);
    token_identity(token.0, session_id)
}

pub fn process_image(process: HANDLE) -> windows::core::Result<PathBuf> {
    let mut image = vec![0u16; 32768];
    let mut length = image.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(image.as_mut_ptr()),
            &mut length,
        )
    }?;
    Ok(PathBuf::from(String::from_utf16_lossy(
        &image[..length as usize],
    )))
}

pub fn keytao_runtime_root() -> Result<PathBuf, String> {
    let runtime = native_program_files_dir()?
        .join("KeyTao")
        .join("keytao-windows-ime-runtime");
    crate::engine_config::resolve_install_root([runtime])
        .ok_or_else(|| "cannot resolve KeyTao runtime root".into())
}

pub fn keytao_app_root() -> Result<PathBuf, String> {
    let installed = [
        (windows::core::w!("Software\\rea\\KeyTao"), PCWSTR::null()),
        (
            windows::core::w!("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\KeyTao"),
            windows::core::w!("InstallLocation"),
        ),
    ]
    .into_iter()
    .filter_map(|(key, name)| machine_registry_string(key, name).map(PathBuf::from));
    crate::engine_config::resolve_install_root(installed)
        .or_else(|| {
            let program_files = native_program_files_dir().ok()?;
            crate::engine_config::resolve_install_root([program_files.join("KeyTao")])
        })
        .ok_or_else(|| "cannot resolve KeyTao app root".into())
}

fn native_program_files_dir() -> Result<PathBuf, String> {
    machine_registry_string(
        windows::core::w!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion"),
        windows::core::w!("ProgramFilesDir"),
    )
    .and_then(|path| crate::engine_config::resolve_install_root([PathBuf::from(path)]))
    .ok_or_else(|| "cannot resolve native Program Files directory for KeyTao roots".into())
}

fn machine_registry_string(key: PCWSTR, name: PCWSTR) -> Option<String> {
    let mut value = vec![0u16; 32768];
    let mut size = (value.len() * std::mem::size_of::<u16>()) as u32;
    // keytao.nsi's Context macro uses SetRegView 64, independent of the TSF host's bitness.
    let result = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key,
            name,
            RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
            None,
            Some(value.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if result.is_err() || size < 4 || size as usize > value.len() * 2 || size % 2 != 0 {
        return None;
    }
    let value = &value[..size as usize / 2];
    String::from_utf16(value.strip_suffix(&[0])?).ok()
}

fn identity(process: HANDLE, pid: u32) -> Result<Identity, String> {
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.map_err(|e| e.to_string())?;
    let token = OwnedHandle(token);
    token_identity(token.0, process_session_id(pid)?)
}

fn process_session_id(pid: u32) -> Result<u32, String> {
    let mut session_id = 0;
    unsafe { ProcessIdToSessionId(pid, &mut session_id) }.map_err(|error| error.to_string())?;
    Ok(session_id)
}

fn token_identity(token: HANDLE, session_id: u32) -> Result<Identity, String> {
    unsafe {
        let mut size = 0;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut size);
        if size == 0 {
            return Err("empty process token identity".into());
        }
        // TOKEN_USER requires pointer alignment, unlike Vec<u8>.
        let mut buffer = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
        GetTokenInformation(
            token,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            size,
            &mut size,
        )
        .map_err(|e| e.to_string())?;
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let mut sid_text = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut sid_text).map_err(|e| e.to_string())?;
        let sid = sid_text.to_string();
        LocalFree(HLOCAL(sid_text.0.cast()));
        let mut appcontainer = 0u32;
        GetTokenInformation(
            token,
            TokenIsAppContainer,
            Some((&mut appcontainer as *mut u32).cast()),
            4,
            &mut size,
        )
        .map_err(|e| e.to_string())?;
        Ok(Identity {
            sid: sid.map_err(|e| e.to_string())?,
            session_id,
            is_appcontainer: appcontainer != 0,
        })
    }
}
