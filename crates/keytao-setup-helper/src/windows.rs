use super::{owned_executable_paths, owned_image, owned_name, same_process, OwnedProcess, Result};
use std::{mem, os::windows::ffi::OsStrExt, path::Path, ptr};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetLastError, ERROR_INVALID_PARAMETER, ERROR_NO_MORE_FILES, FILETIME, HANDLE,
        INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_OBJECT_0,
    },
    Storage::FileSystem::GetFullPathNameW,
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        },
        RemoteDesktop::ProcessIdToSessionId,
        Threading::{
            GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, TerminateProcess,
            WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
            PROCESS_TERMINATE,
        },
    },
};

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn error(context: &str) -> String {
    format!("{context}: {}", std::io::Error::last_os_error())
}

fn full_path(path: &[u16]) -> Result<String> {
    let mut buffer = vec![0; 32768];
    let length = unsafe {
        GetFullPathNameW(
            path.as_ptr(),
            buffer.len() as u32,
            buffer.as_mut_ptr(),
            ptr::null_mut(),
        )
    } as usize;
    if length == 0 || length >= buffer.len() {
        return Err(error("Unable to verify executable path"));
    }
    String::from_utf16(&buffer[..length]).map_err(|e| e.to_string())
}

pub(super) fn allowed_paths(directory: &Path) -> Result<[String; 2]> {
    if directory.as_os_str().is_empty() || !directory.has_root() {
        return Err("InstallDir must be an absolute directory path.".into());
    }
    let path: Vec<_> = directory.as_os_str().encode_wide().chain(Some(0)).collect();
    let root = full_path(&path)?;
    match std::fs::metadata(&root) {
        Ok(metadata) if !metadata.is_dir() => {
            return Err(format!("Expected an installation directory: {root}"))
        }
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.to_string()),
        _ => {}
    }
    Ok(owned_executable_paths(&root))
}

fn exited(process: &Handle) -> Result<bool> {
    match unsafe { WaitForSingleObject(process.0, 0) } {
        WAIT_FAILED => Err(error("Unable to inspect process state")),
        WAIT_OBJECT_0 => Ok(true),
        _ => Ok(false),
    }
}

fn image_path(process: &Handle) -> Result<String> {
    let mut buffer = vec![0; 32768];
    let mut length = buffer.len() as u32;
    if unsafe { QueryFullProcessImageNameW(process.0, 0, buffer.as_mut_ptr(), &mut length) } == 0 {
        return Err(error("Unable to verify process executable path"));
    }
    buffer[length as usize] = 0;
    full_path(&buffer)
}

fn creation_time(process: &Handle) -> Result<u64> {
    let mut created: FILETIME = unsafe { mem::zeroed() };
    let mut exited = created;
    let mut kernel = created;
    let mut user = created;
    if unsafe { GetProcessTimes(process.0, &mut created, &mut exited, &mut kernel, &mut user) } == 0
    {
        return Err(error("Unable to verify process creation time"));
    }
    Ok((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

fn open(pid: u32, stop: bool) -> Result<Option<Handle>> {
    let access = PROCESS_QUERY_LIMITED_INFORMATION
        | PROCESS_SYNCHRONIZE
        | if stop { PROCESS_TERMINATE } else { 0 };
    let handle = unsafe { OpenProcess(access, 0, pid) };
    if !handle.is_null() {
        return Ok(Some(Handle(handle)));
    }
    if unsafe { GetLastError() } == ERROR_INVALID_PARAMETER {
        return Ok(None);
    }
    Err(error(&format!(
        "Unable to {} process (PID {pid})",
        if stop { "stop" } else { "verify" }
    )))
}

fn inspect(entry: &PROCESSENTRY32W, allowed: &[String; 2]) -> Result<Option<OwnedProcess>> {
    let name = String::from_utf16_lossy(
        &entry.szExeFile[..entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len())],
    );
    if !owned_name(&name) {
        return Ok(None);
    }
    let Some(process) = open(entry.th32ProcessID, false)? else {
        return Ok(None);
    };
    let result = (|| {
        if exited(&process)? {
            return Ok(None);
        }
        let image_path = image_path(&process)?;
        if !owned_image(allowed, &image_path) {
            return Ok(None);
        }
        let mut session = 0;
        if unsafe { ProcessIdToSessionId(entry.th32ProcessID, &mut session) } == 0 {
            return Err(error("Unable to inspect process session"));
        }
        Ok(Some(OwnedProcess {
            pid: entry.th32ProcessID,
            name,
            image_path,
            session,
            created: creation_time(&process)?,
        }))
    })();
    match result {
        Err(_) if exited(&process)? => Ok(None),
        other => other,
    }
}

pub(super) fn find(allowed: &[String; 2]) -> Result<Vec<OwnedProcess>> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(error("Unable to enumerate running processes"));
    }
    let snapshot = Handle(snapshot);
    let mut entry: PROCESSENTRY32W = unsafe { mem::zeroed() };
    entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut found = Vec::new();
    let mut more = unsafe { Process32FirstW(snapshot.0, &mut entry) };
    while more != 0 {
        if let Some(process) = inspect(&entry, allowed)? {
            found.push(process);
        }
        more = unsafe { Process32NextW(snapshot.0, &mut entry) };
    }
    if unsafe { GetLastError() } != ERROR_NO_MORE_FILES {
        return Err(error("Unable to enumerate running processes"));
    }
    Ok(found)
}

pub(super) fn stop(target: &OwnedProcess, allowed: &[String; 2]) -> Result<()> {
    let Some(process) = open(target.pid, true)? else {
        return Ok(());
    };
    if exited(&process)? {
        return Ok(());
    }
    // Revalidate identity on the termination handle so PID reuse cannot target another process.
    if !same_process(target, &image_path(&process)?, creation_time(&process)?) {
        return Ok(());
    }
    if !owned_image(allowed, &target.image_path) {
        return Err("Refusing to stop a non-owned executable.".into());
    }
    if unsafe { TerminateProcess(process.0, 0) } == 0 {
        let failure = error(&format!("Unable to stop {}", target.name));
        if exited(&process)? {
            return Ok(());
        }
        return Err(failure);
    }
    if unsafe { WaitForSingleObject(process.0, 5000) } != WAIT_OBJECT_0 {
        return Err(format!("Timed out stopping {}.", target.name));
    }
    Ok(())
}
