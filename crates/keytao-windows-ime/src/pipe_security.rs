//! Process/token identity checks for the per-user, per-session pipe.

use windows::{
    core::PWSTR,
    Win32::{
        Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL},
        Security::{
            Authorization::ConvertSidToStringSidW, GetTokenInformation, TokenIsAppContainer,
            TokenUser, TOKEN_QUERY, TOKEN_USER,
        },
        System::{
            RemoteDesktop::ProcessIdToSessionId,
            Threading::{
                GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
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
