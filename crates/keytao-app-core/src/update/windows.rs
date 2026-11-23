use std::{
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
};
use windows::{
    Win32::{
        Foundation::{ERROR_CANCELLED, RPC_E_CHANGED_MODE},
        System::{
            Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
            Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY, RegGetValueW},
            SystemInformation::GetSystemDirectoryW,
        },
        UI::{
            Shell::{SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW},
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    },
    core::{HRESULT, PCWSTR, w},
};

const PRODUCT_KEY: PCWSTR = w!("Software\\rea\\KeyTao");
const UNINSTALL_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\KeyTao");

fn registry_string(key: PCWSTR, name: PCWSTR) -> Option<String> {
    let mut value = [0u16; 1024];
    let mut size = std::mem::size_of_val(&value) as u32;
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
    .ok()?;
    let end = value.iter().position(|&unit| unit == 0)?;
    String::from_utf16(&value[..end]).ok()
}

fn running_registered_install() -> Option<()> {
    if registry_string(UNINSTALL_KEY, w!("MainBinaryName"))? != "keytao-app.exe" {
        return None;
    }
    let root = PathBuf::from(registry_string(PRODUCT_KEY, PCWSTR::null())?);
    let location = registry_string(UNINSTALL_KEY, w!("InstallLocation"))?;
    let location = Path::new(location.trim_matches('"'));
    if !root.is_absolute()
        || !location.is_absolute()
        || root.canonicalize().ok()? != location.canonicalize().ok()?
        || root.join("keytao-app.exe").canonicalize().ok()?
            != std::env::current_exe().ok()?.canonicalize().ok()?
        || !root.join("uninstall.exe").is_file()
        || !root.join("flutter_windows.dll").is_file()
        || !root.join("keytao_app_bridge.dll").is_file()
    {
        return None;
    }
    Some(())
}

pub(super) fn can_restart() -> bool {
    running_registered_install().is_some()
}

pub(super) fn open_installer(path: &Path, restart: bool) -> Result<(), String> {
    if restart && !can_restart() {
        return Err("当前运行的 KeyTao 安装已变化，请重新检查更新".into());
    }
    let file: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut system_directory = [0u16; 1024];
    let length = unsafe { GetSystemDirectoryW(Some(&mut system_directory)) } as usize;
    if length == 0 || length >= system_directory.len() {
        return Err("无法读取 Windows 系统目录".into());
    }
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: if restart {
            w!("/S /KEYTAO_RESTART")
        } else {
            PCWSTR::null()
        },
        lpDirectory: PCWSTR(system_directory.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if com.is_err() && com != RPC_E_CHANGED_MODE {
        return Err(format!("无法初始化 Windows 安装服务：{}", com.message()));
    }
    let result = unsafe { ShellExecuteExW(&mut info) };
    if com.is_ok() {
        unsafe { CoUninitialize() };
    }
    result.map_err(|error| {
        if error.code() == HRESULT::from_win32(ERROR_CANCELLED.0) {
            "已取消 Windows 安装授权".into()
        } else {
            format!("无法启动 KeyTao 安装程序：{error}")
        }
    })
}
