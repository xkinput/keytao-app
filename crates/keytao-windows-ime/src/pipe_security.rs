//! Process/token identity checks for the per-user, per-session pipe.

use std::path::PathBuf;
use windows::{
    core::PWSTR,
    Win32::{
        Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL},
        Security::{
            Authorization::ConvertSidToStringSidW, GetTokenInformation, TokenIsAppContainer,
            TokenUser, TOKEN_QUERY, TOKEN_USER,
        },
        System::{
            Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY},
            RemoteDesktop::ProcessIdToSessionId,
            Threading::{
                GetCurrentProcess, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
                PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
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

pub fn keytao_install_root() -> Result<PathBuf, String> {
    let mut value = vec![0u16; 32768];
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
    path.join("KeyTao")
        .canonicalize()
        .map_err(|error| format!("resolve KeyTao installation directory: {error}"))
}

fn identity(process: HANDLE, pid: u32) -> Result<Identity, String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token).map_err(|e| e.to_string())?;
        let token = OwnedHandle(token);
        let mut size = 0;
        let _ = GetTokenInformation(token.0, TokenUser, None, 0, &mut size);
        if size == 0 {
            return Err("empty process token identity".into());
        }
        // TOKEN_USER requires pointer alignment, unlike Vec<u8>.
        let mut buffer = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
        GetTokenInformation(
            token.0,
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
            token.0,
            TokenIsAppContainer,
            Some((&mut appcontainer as *mut u32).cast()),
            4,
            &mut size,
        )
        .map_err(|e| e.to_string())?;
        let mut session_id = 0;
        ProcessIdToSessionId(pid, &mut session_id).map_err(|e| e.to_string())?;
        Ok(Identity {
            sid: sid.map_err(|e| e.to_string())?,
            session_id,
            is_appcontainer: appcontainer != 0,
        })
    }
}
