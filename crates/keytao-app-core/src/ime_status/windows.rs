use crate::{Core, CoreError, CoreEvent, events::WindowsImeStatus, ime_paths::*};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use time::OffsetDateTime;

#[cfg(target_os = "windows")]
const WINDOWS_TEXT_SERVICE_CLSID: &str = "{4A5C6D7E-8F90-1A2B-3C4D-5E6F7A8B9C0D}";

#[cfg(target_os = "windows")]
const WINDOWS_PROFILE_ID: &str = "{1B2C3D4E-5F60-7A8B-9C0D-1E2F3A4B5C6D}";

#[cfg(target_os = "windows")]
const WINDOWS_TEXT_SERVICE_GUID: windows::core::GUID = windows::core::GUID {
    data1: 0x4A5C6D7E,
    data2: 0x8F90,
    data3: 0x1A2B,
    data4: [0x3C, 0x4D, 0x5E, 0x6F, 0x7A, 0x8B, 0x9C, 0x0D],
};

#[cfg(target_os = "windows")]
const WINDOWS_PROFILE_GUID: windows::core::GUID = windows::core::GUID {
    data1: 0x1B2C3D4E,
    data2: 0x5F60,
    data3: 0x7A8B,
    data4: [0x9C, 0x0D, 0x1E, 0x2F, 0x3A, 0x4B, 0x5C, 0x6D],
};

#[cfg(target_os = "windows")]
const WINDOWS_LANGID_CHINESE_SIMPLIFIED: u16 = 0x0804;

#[cfg(target_os = "windows")]
const WINDOWS_PE_MACHINE_X86: u16 = 0x014C;

#[cfg(target_os = "windows")]
const WINDOWS_PE_MACHINE_X64: u16 = 0x8664;

#[cfg(target_os = "windows")]
const WINDOWS_PE_MACHINE_ARM64: u16 = 0xAA64;

#[cfg(target_os = "windows")]
static WINDOWS_IME_REGISTRATION_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "windows")]
static WINDOWS_IME_DATA_REPAIR_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "windows")]
const WINDOWS_IME_ENGINE_INIT_MUTEX_TIMEOUT_MS: u32 = 30_000;

#[cfg(target_os = "windows")]
pub struct WindowsImeEngineInitGuard(windows::Win32::Foundation::HANDLE);

#[cfg(target_os = "windows")]
impl WindowsImeEngineInitGuard {
    pub fn acquire() -> Result<Self, String> {
        use windows::{
            Win32::{
                Foundation::{CloseHandle, WAIT_ABANDONED, WAIT_OBJECT_0},
                System::Threading::{CreateMutexW, WaitForSingleObject},
            },
            core::PCWSTR,
        };

        let mut name: Vec<u16> = keytao_core::WINDOWS_IME_ENGINE_INIT_MUTEX_NAME
            .encode_utf16()
            .collect();
        name.push(0);
        let handle = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) }
            .map_err(|error| format!("create Windows IME engine mutex: {error}"))?;
        let wait = unsafe { WaitForSingleObject(handle, WINDOWS_IME_ENGINE_INIT_MUTEX_TIMEOUT_MS) };
        if wait != WAIT_OBJECT_0 && wait != WAIT_ABANDONED {
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err(format!(
                "wait for Windows IME engine mutex: 0x{:08x}",
                wait.0
            ));
        }
        Ok(Self(handle))
    }
}

#[cfg(target_os = "windows")]
impl Drop for WindowsImeEngineInitGuard {
    fn drop(&mut self) {
        use windows::Win32::{Foundation::CloseHandle, System::Threading::ReleaseMutex};

        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

#[cfg(target_os = "windows")]
struct WindowsComApartment(bool);

#[cfg(target_os = "windows")]
impl Drop for WindowsComApartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                windows::Win32::System::Com::CoUninitialize();
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn windows_init_com_apartment() -> Result<WindowsComApartment, String> {
    use windows::Win32::{
        Foundation::RPC_E_CHANGED_MODE,
        System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx},
    };

    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if hr.is_ok() {
        Ok(WindowsComApartment(true))
    } else if hr == RPC_E_CHANGED_MODE {
        Ok(WindowsComApartment(false))
    } else {
        Err(format!("initialize COM apartment: {}", hr.message()))
    }
}

#[cfg(target_os = "windows")]
struct WindowsTsfProfileManager {
    profile_manager: windows::Win32::UI::TextServices::ITfInputProcessorProfileMgr,
    _com: WindowsComApartment,
}

#[cfg(target_os = "windows")]
impl WindowsTsfProfileManager {
    fn open(context: &str) -> Result<Self, String> {
        use windows::{
            Win32::{
                System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
                UI::TextServices::{
                    CLSID_TF_InputProcessorProfiles, ITfInputProcessorProfileMgr,
                    ITfInputProcessorProfiles,
                },
            },
            core::{IUnknown, Interface},
        };

        let com = windows_init_com_apartment()?;
        let profiles: ITfInputProcessorProfiles = unsafe {
            CoCreateInstance(
                &CLSID_TF_InputProcessorProfiles,
                None::<&IUnknown>,
                CLSCTX_INPROC_SERVER,
            )
        }
        .map_err(|error| format!("open TSF input processor profiles {context}: {error}"))?;
        let profile_manager: ITfInputProcessorProfileMgr = profiles
            .cast()
            .map_err(|error| format!("open TSF profile manager {context}: {error}"))?;
        Ok(Self {
            profile_manager,
            _com: com,
        })
    }
}

#[cfg(target_os = "windows")]
pub struct WindowsImeProfileDeploymentGuard {
    reactivate: bool,
}

#[cfg(target_os = "windows")]
impl WindowsImeProfileDeploymentGuard {
    pub fn suspend() -> Result<Self, String> {
        use windows::Win32::UI::{
            Input::KeyboardAndMouse::HKL,
            TextServices::{
                GUID_TFCAT_TIP_KEYBOARD, TF_INPUTPROCESSORPROFILE, TF_IPPMF_FORSESSION,
                TF_PROFILETYPE_INPUTPROCESSOR,
            },
        };

        let context = WindowsTsfProfileManager::open("for deployment")?;
        let profile_manager = &context.profile_manager;

        let mut active = TF_INPUTPROCESSORPROFILE::default();
        let reactivate = unsafe {
            profile_manager
                .GetActiveProfile(&GUID_TFCAT_TIP_KEYBOARD, &mut active)
                .is_ok()
        } && active.dwProfileType == TF_PROFILETYPE_INPUTPROCESSOR
            && active.langid == WINDOWS_LANGID_CHINESE_SIMPLIFIED
            && active.clsid == WINDOWS_TEXT_SERVICE_GUID
            && active.guidProfile == WINDOWS_PROFILE_GUID;

        if reactivate {
            unsafe {
                profile_manager.DeactivateProfile(
                    TF_PROFILETYPE_INPUTPROCESSOR,
                    WINDOWS_LANGID_CHINESE_SIMPLIFIED,
                    &WINDOWS_TEXT_SERVICE_GUID,
                    &WINDOWS_PROFILE_GUID,
                    HKL::default(),
                    TF_IPPMF_FORSESSION,
                )
            }
            .map_err(|error| format!("deactivate KeyTao TSF profile for deployment: {error}"))?;
        }

        if let Err(error) =
            unsafe { profile_manager.ReleaseInputProcessor(&WINDOWS_TEXT_SERVICE_GUID, 0) }
        {
            tracing::warn!("release KeyTao TSF instances before deployment: {error}");
        }
        std::thread::sleep(std::time::Duration::from_millis(150));

        drop(context);
        Ok(Self { reactivate })
    }

    fn reactivate_profile(&self) -> Result<(), String> {
        use windows::Win32::UI::{
            Input::KeyboardAndMouse::HKL,
            TextServices::{
                GUID_TFCAT_TIP_KEYBOARD, TF_INPUTPROCESSORPROFILE,
                TF_IPPMF_DONTCARECURRENTINPUTLANGUAGE, TF_IPPMF_FORSESSION,
                TF_PROFILETYPE_INPUTPROCESSOR,
            },
        };

        if !self.reactivate {
            return Ok(());
        }
        let context = WindowsTsfProfileManager::open("after deployment")?;
        unsafe {
            context.profile_manager.ActivateProfile(
                TF_PROFILETYPE_INPUTPROCESSOR,
                WINDOWS_LANGID_CHINESE_SIMPLIFIED,
                &WINDOWS_TEXT_SERVICE_GUID,
                &WINDOWS_PROFILE_GUID,
                HKL::default(),
                TF_IPPMF_FORSESSION | TF_IPPMF_DONTCARECURRENTINPUTLANGUAGE,
            )
        }
        .map_err(|error| format!("reactivate KeyTao TSF profile after deployment: {error}"))?;

        let mut active = TF_INPUTPROCESSORPROFILE::default();
        unsafe {
            context
                .profile_manager
                .GetActiveProfile(&GUID_TFCAT_TIP_KEYBOARD, &mut active)
        }
        .map_err(|error| format!("verify KeyTao TSF profile after deployment: {error}"))?;
        if active.dwProfileType != TF_PROFILETYPE_INPUTPROCESSOR
            || active.langid != WINDOWS_LANGID_CHINESE_SIMPLIFIED
            || active.clsid != WINDOWS_TEXT_SERVICE_GUID
            || active.guidProfile != WINDOWS_PROFILE_GUID
        {
            return Err("KeyTao TSF profile did not become active after deployment".into());
        }
        Ok(())
    }

    pub fn resume(mut self) -> Result<(), String> {
        self.reactivate_profile()?;
        self.reactivate = false;
        Ok(())
    }
}

#[cfg(target_os = "windows")]
impl Drop for WindowsImeProfileDeploymentGuard {
    fn drop(&mut self) {
        if let Err(error) = self.reactivate_profile() {
            tracing::error!("{error}");
        }
    }
}

#[cfg(target_os = "windows")]
fn windows_tsf_profile_enabled() -> Result<bool, String> {
    use windows::{
        Win32::{
            System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
            UI::{
                Input::KeyboardAndMouse::HKL,
                TextServices::{
                    CLSID_TF_InputProcessorProfiles, ITfInputProcessorProfileMgr,
                    ITfInputProcessorProfiles, TF_INPUTPROCESSORPROFILE, TF_IPP_FLAG_ENABLED,
                    TF_PROFILETYPE_INPUTPROCESSOR,
                },
            },
        },
        core::{IUnknown, Interface},
    };

    let _com = windows_init_com_apartment()?;
    unsafe {
        let profiles: ITfInputProcessorProfiles = CoCreateInstance(
            &CLSID_TF_InputProcessorProfiles,
            None::<&IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
        .map_err(|e| format!("open TSF input processor profiles: {e}"))?;
        let profile_mgr: ITfInputProcessorProfileMgr = profiles
            .cast()
            .map_err(|e| format!("open modern TSF profile manager: {e}"))?;
        let mut profile = TF_INPUTPROCESSORPROFILE::default();
        profile_mgr
            .GetProfile(
                TF_PROFILETYPE_INPUTPROCESSOR,
                WINDOWS_LANGID_CHINESE_SIMPLIFIED,
                &WINDOWS_TEXT_SERVICE_GUID,
                &WINDOWS_PROFILE_GUID,
                HKL::default(),
                &mut profile,
            )
            .map_err(|e| format!("query KeyTao TSF profile: {e}"))?;
        Ok(profile.dwFlags & TF_IPP_FLAG_ENABLED != 0)
    }
}

#[cfg(target_os = "windows")]
fn windows_enable_ime_for_current_user() -> Result<(), String> {
    use windows::{
        Win32::{
            Foundation::{BOOL, FreeLibrary},
            System::{
                Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
                LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW},
            },
            UI::TextServices::{CLSID_TF_InputProcessorProfiles, ITfInputProcessorProfiles},
        },
        core::{Error, IUnknown, PCSTR, PCWSTR},
    };

    type InstallLayoutOrTipFn = unsafe extern "system" fn(PCWSTR, u32) -> BOOL;

    let _com = windows_init_com_apartment()?;
    unsafe {
        let profiles: ITfInputProcessorProfiles = CoCreateInstance(
            &CLSID_TF_InputProcessorProfiles,
            None::<&IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
        .map_err(|e| format!("open TSF input processor profiles: {e}"))?;
        let mut input_dll = "input.dll".encode_utf16().collect::<Vec<_>>();
        input_dll.push(0);
        let module = LoadLibraryExW(
            PCWSTR(input_dll.as_ptr()),
            None,
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
        .map_err(|e| format!("load system input.dll: {e}"))?;
        let Some(proc) = GetProcAddress(module, PCSTR(c"InstallLayoutOrTip".as_ptr().cast()))
        else {
            let error = Error::from_win32();
            let _ = FreeLibrary(module);
            return Err(format!("resolve InstallLayoutOrTip: {error}"));
        };
        let install_layout_or_tip: InstallLayoutOrTipFn = std::mem::transmute(proc);
        let mut tip = format!(
            "0x{WINDOWS_LANGID_CHINESE_SIMPLIFIED:04X}:{WINDOWS_TEXT_SERVICE_CLSID}{WINDOWS_PROFILE_ID}"
        )
        .encode_utf16()
        .collect::<Vec<_>>();
        tip.push(0);
        let installed = install_layout_or_tip(PCWSTR(tip.as_ptr()), 0);
        let install_error = (!installed.as_bool()).then(Error::from_win32);
        let _ = FreeLibrary(module);
        if let Some(error) = install_error {
            return Err(format!(
                "add KeyTao to the current user's enabled input methods: {error}"
            ));
        }

        // InstallLayoutOrTip updates the user's input list and can reset the
        // profile flag. Enable and verify only after that operation completes.
        profiles
            .EnableLanguageProfile(
                &WINDOWS_TEXT_SERVICE_GUID,
                WINDOWS_LANGID_CHINESE_SIMPLIFIED,
                &WINDOWS_PROFILE_GUID,
                BOOL::from(true),
            )
            .map_err(|e| format!("enable KeyTao TSF profile for current user: {e}"))?;
        let enabled = profiles
            .IsEnabledLanguageProfile(
                &WINDOWS_TEXT_SERVICE_GUID,
                WINDOWS_LANGID_CHINESE_SIMPLIFIED,
                &WINDOWS_PROFILE_GUID,
            )
            .map_err(|e| format!("verify KeyTao TSF profile for current user: {e}"))?;
        if !enabled.as_bool() {
            return Err("KeyTao TSF profile remains disabled for the current user".into());
        }
    }

    windows_tsf_profile_enabled()?.then_some(()).ok_or_else(|| {
        "KeyTao TSF profile is not exposed as enabled by the Windows profile manager".into()
    })
}

#[cfg(target_os = "windows")]
fn windows_ime_runtime_dir(core: &Core) -> Option<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(registered) = windows_registered_ime_path(false) {
        if let Some(parent) = PathBuf::from(registered).parent() {
            candidates.push(parent.to_path_buf());
        }
    }

    let native_arm64 = windows_native_machine() == WINDOWS_PE_MACHINE_ARM64;
    let runtime_names = if native_arm64 {
        ["arm64x", "current", "x64", "x86"]
    } else {
        ["current", "x64", "x86", "arm64x"]
    };

    let mut add_runtime_root = |root: PathBuf| {
        for name in runtime_names {
            candidates.push(root.join(name));
        }
    };

    {
        let resource_dir = core.env().resource_dir.clone();
        add_runtime_root(resource_dir.clone());
        add_runtime_root(resource_dir.join("keytao-windows-ime-runtime"));
        add_runtime_root(
            resource_dir
                .join("target")
                .join("keytao-windows-ime-runtime"),
        );
        add_runtime_root(
            resource_dir
                .join("resources")
                .join("keytao-windows-ime-runtime"),
        );
        add_runtime_root(
            resource_dir
                .join("_up_")
                .join("target")
                .join("keytao-windows-ime-runtime"),
        );
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            add_runtime_root(dir.to_path_buf());
            add_runtime_root(dir.join("keytao-windows-ime-runtime"));
            add_runtime_root(dir.join("resources"));
            add_runtime_root(dir.join("resources").join("keytao-windows-ime-runtime"));
            add_runtime_root(
                dir.join("_up_")
                    .join("target")
                    .join("keytao-windows-ime-runtime"),
            );
            add_runtime_root(
                dir.join("resources")
                    .join("_up_")
                    .join("target")
                    .join("keytao-windows-ime-runtime"),
            );
        }
    }

    candidates
        .into_iter()
        .find(|dir| windows_native_ime_runtime_complete(dir, native_arm64))
}

#[cfg(target_os = "windows")]
fn windows_native_machine() -> u16 {
    use windows::Win32::System::{
        SystemInformation::IMAGE_FILE_MACHINE,
        Threading::{GetCurrentProcess, IsWow64Process2},
    };

    let mut process_machine = IMAGE_FILE_MACHINE(0);
    let mut native_machine = IMAGE_FILE_MACHINE(0);
    let result = unsafe {
        IsWow64Process2(
            GetCurrentProcess(),
            &mut process_machine,
            Some(&mut native_machine),
        )
    };
    if result.is_ok() && native_machine.0 != 0 {
        native_machine.0
    } else if cfg!(target_arch = "aarch64") {
        WINDOWS_PE_MACHINE_ARM64
    } else if cfg!(target_pointer_width = "64") {
        WINDOWS_PE_MACHINE_X64
    } else {
        WINDOWS_PE_MACHINE_X86
    }
}

#[cfg(target_os = "windows")]
fn windows_native_ime_runtime_complete(dir: &Path, native_arm64: bool) -> bool {
    let data_complete = dir.join("rime-data").join("default.yaml").is_file()
        && dir.join("default-theme.yaml").is_file();
    if !data_complete {
        return false;
    }

    if native_arm64 {
        return dir.join("keytao_windows_ime.dll").is_file()
            && windows_pe_machine(&dir.join("keytao_windows_ime_arm64.dll"))
                == Some(WINDOWS_PE_MACHINE_ARM64)
            && windows_pe_machine(&dir.join("keytao_windows_ime_x64.dll"))
                == Some(WINDOWS_PE_MACHINE_X64)
            && windows_pe_machine(&dir.join("rime-arm64.dll")) == Some(WINDOWS_PE_MACHINE_ARM64)
            && windows_pe_machine(&dir.join("rime.dll")) == Some(WINDOWS_PE_MACHINE_X64);
    }

    let expected_machine = if cfg!(target_pointer_width = "64") {
        WINDOWS_PE_MACHINE_X64
    } else {
        WINDOWS_PE_MACHINE_X86
    };
    windows_pe_machine(&dir.join("keytao_windows_ime.dll")) == Some(expected_machine)
        && windows_pe_machine(&dir.join("rime.dll")) == Some(expected_machine)
}

#[cfg(target_os = "windows")]
fn windows_ime_x86_standard_runtime_dir() -> Option<PathBuf> {
    std::env::var_os("ProgramFiles(x86)").map(|program_files_x86| {
        PathBuf::from(program_files_x86)
            .join("KeyTao")
            .join("keytao-windows-ime-runtime")
            .join("x86")
    })
}

#[cfg(target_os = "windows")]
fn windows_x86_runtime_complete(dir: &Path) -> bool {
    let dll = dir.join("keytao_windows_ime.dll");
    dll.is_file()
        && windows_pe_machine(&dll) == Some(WINDOWS_PE_MACHINE_X86)
        && dir.join("rime.dll").is_file()
        && dir.join("rime-data").join("default.yaml").is_file()
        && dir.join("default-theme.yaml").is_file()
}

#[cfg(target_os = "windows")]
fn windows_ime_x86_packaged_runtime_dir(core: &Core) -> Option<PathBuf> {
    if !cfg!(target_pointer_width = "64") {
        return None;
    }

    let mut candidates = Vec::new();
    if let Some(native_dir) = windows_ime_runtime_dir(core) {
        if let Some(runtime_root) = native_dir.parent() {
            candidates.push(runtime_root.join("x86"));
        }
    }
    {
        let resource_dir = core.env().resource_dir.clone();
        candidates.extend([
            resource_dir.join("x86"),
            resource_dir.join("keytao-windows-ime-runtime").join("x86"),
            resource_dir
                .join("target")
                .join("keytao-windows-ime-runtime")
                .join("x86"),
            resource_dir
                .join("_up_")
                .join("target")
                .join("keytao-windows-ime-runtime")
                .join("x86"),
        ]);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.extend([
                dir.join("x86"),
                dir.join("keytao-windows-ime-runtime").join("x86"),
                dir.join("resources")
                    .join("keytao-windows-ime-runtime")
                    .join("x86"),
                dir.join("_up_")
                    .join("target")
                    .join("keytao-windows-ime-runtime")
                    .join("x86"),
            ]);
        }
    }

    candidates
        .into_iter()
        .find(|dir| windows_x86_runtime_complete(dir))
}

#[cfg(target_os = "windows")]
fn windows_ime_x86_runtime_dir(core: &Core) -> Option<PathBuf> {
    windows_registered_ime_path(true)
        .and_then(|path| PathBuf::from(path).parent().map(Path::to_path_buf))
        .filter(|dir| windows_x86_runtime_complete(dir))
        .or_else(|| windows_ime_x86_packaged_runtime_dir(core))
        .or_else(|| {
            windows_ime_x86_standard_runtime_dir().filter(|dir| windows_x86_runtime_complete(dir))
        })
}

#[cfg(target_os = "windows")]
fn windows_pe_machine(path: &Path) -> Option<u16> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path).ok()?;
    let mut dos_magic = [0_u8; 2];
    file.read_exact(&mut dos_magic).ok()?;
    if dos_magic != *b"MZ" {
        return None;
    }
    file.seek(SeekFrom::Start(0x3C)).ok()?;
    let mut offset = [0_u8; 4];
    file.read_exact(&mut offset).ok()?;
    file.seek(SeekFrom::Start(u32::from_le_bytes(offset) as u64))
        .ok()?;
    let mut header = [0_u8; 6];
    file.read_exact(&mut header).ok()?;
    if header[..4] != *b"PE\0\0" {
        return None;
    }
    Some(u16::from_le_bytes([header[4], header[5]]))
}

#[cfg(target_os = "windows")]
fn windows_normal_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    value.strip_prefix(r"\\?\").unwrap_or(&value).to_string()
}

#[cfg(target_os = "windows")]
pub fn windows_app_shared_data_dir(core: &Core) -> Option<String> {
    windows_ime_runtime_dir(core)
        .map(|dir| dir.join("rime-data"))
        .filter(|dir| dir.join("default.yaml").is_file())
        .map(|dir| windows_normal_path(&dir))
}

#[cfg(target_os = "windows")]
fn windows_registered_ime_path(use_x86_view: bool) -> Option<String> {
    use windows::{
        Win32::System::Registry::{
            HKEY, HKEY_CLASSES_ROOT, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY, REG_EXPAND_SZ,
            REG_SZ, RegCloseKey, RegOpenKeyExW, RegQueryValueExW,
        },
        core::PCWSTR,
    };

    let key = format!(r"HKCR\CLSID\{}\InprocServer32", WINDOWS_TEXT_SERVICE_CLSID);
    let subkey = key.strip_prefix(r"HKCR\")?;
    let mut subkey_wide = subkey.encode_utf16().collect::<Vec<_>>();
    subkey_wide.push(0);
    let view = if use_x86_view {
        KEY_WOW64_32KEY
    } else {
        KEY_WOW64_64KEY
    };
    let mut handle = HKEY::default();
    if unsafe {
        RegOpenKeyExW(
            HKEY_CLASSES_ROOT,
            PCWSTR(subkey_wide.as_ptr()),
            0,
            KEY_READ | view,
            &mut handle,
        )
        .ok()
    }
    .is_err()
    {
        return None;
    }

    let result = (|| {
        let mut value_type = REG_SZ;
        let mut byte_len = 0_u32;
        unsafe {
            RegQueryValueExW(
                handle,
                PCWSTR::null(),
                None,
                Some(&mut value_type),
                None,
                Some(&mut byte_len),
            )
            .ok()
            .ok()?;
        }
        if (value_type != REG_SZ && value_type != REG_EXPAND_SZ) || byte_len < 2 {
            return None;
        }

        let mut value = vec![0_u16; (byte_len as usize).div_ceil(2)];
        unsafe {
            RegQueryValueExW(
                handle,
                PCWSTR::null(),
                None,
                Some(&mut value_type),
                Some(value.as_mut_ptr().cast()),
                Some(&mut byte_len),
            )
            .ok()
            .ok()?;
        }
        let value_len = value
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(value.len());
        let value = String::from_utf16(&value[..value_len]).ok()?;
        (!value.is_empty()).then_some(value)
    })();

    unsafe {
        let _ = RegCloseKey(handle);
    }
    result
}

#[cfg(target_os = "windows")]
fn windows_ime_status_with_message(core: &Core, message: String) -> WindowsImeStatus {
    let runtime_dir = windows_ime_runtime_dir(core);
    let dll_path = runtime_dir
        .as_ref()
        .map(|dir| dir.join("keytao_windows_ime.dll"));
    let registered_path = windows_registered_ime_path(!cfg!(target_pointer_width = "64"));
    let x86_runtime_dir = windows_ime_x86_runtime_dir(core);
    let x86_dll_path = x86_runtime_dir
        .as_ref()
        .map(|dir| dir.join("keytao_windows_ime.dll"));
    let x86_registered_path = if cfg!(target_pointer_width = "64") {
        windows_registered_ime_path(true)
    } else {
        None
    };
    let x86_packaged = !cfg!(target_pointer_width = "64")
        || (x86_dll_path.as_ref().is_some_and(|path| path.is_file())
            && x86_runtime_dir
                .as_ref()
                .is_some_and(|dir| dir.join("rime.dll").is_file())
            && x86_runtime_dir
                .as_ref()
                .is_some_and(|dir| dir.join("rime-data").join("default.yaml").is_file())
            && x86_runtime_dir
                .as_ref()
                .is_some_and(|dir| dir.join("default-theme.yaml").is_file()));
    let packaged = dll_path.as_ref().is_some_and(|path| path.is_file())
        && runtime_dir
            .as_ref()
            .is_some_and(|dir| dir.join("rime.dll").is_file())
        && runtime_dir
            .as_ref()
            .is_some_and(|dir| dir.join("rime-data").join("default.yaml").is_file())
        && runtime_dir
            .as_ref()
            .is_some_and(|dir| dir.join("default-theme.yaml").is_file())
        && x86_packaged;
    let native_registered_dll = match (&dll_path, &registered_path) {
        (Some(dll), Some(path)) => {
            let lhs = std::fs::canonicalize(dll).unwrap_or_else(|_| dll.clone());
            let rhs = std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
            lhs.to_string_lossy()
                .eq_ignore_ascii_case(&rhs.to_string_lossy())
        }
        _ => false,
    };
    let x86_registered_dll = if cfg!(target_pointer_width = "64") {
        match (&x86_dll_path, &x86_registered_path) {
            (Some(dll), Some(path)) => {
                let lhs = std::fs::canonicalize(dll).unwrap_or_else(|_| dll.clone());
                let rhs = std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
                lhs.to_string_lossy()
                    .eq_ignore_ascii_case(&rhs.to_string_lossy())
            }
            _ => false,
        }
    } else {
        true
    };
    let registered_dll = native_registered_dll && x86_registered_dll;
    let (profile_enabled, mut profile_status) = match windows_tsf_profile_enabled() {
        Ok(true) => (true, "TSF profile 已启用".to_string()),
        Ok(false) => (false, "TSF profile 已存在但未启用".to_string()),
        Err(e) => (false, format!("TSF profile 不可用：{e}")),
    };
    if cfg!(target_pointer_width = "64") {
        let native_arm64 = windows_native_machine() == WINDOWS_PE_MACHINE_ARM64;
        profile_status.push_str(if x86_registered_dll && native_arm64 {
            "；ARM64X（ARM64/x64）/x86 COM 均已注册"
        } else if x86_registered_dll {
            "；x64/x86 COM 均已注册"
        } else {
            "；x86 COM 尚未注册"
        });
    }
    let registered = registered_dll && profile_enabled;
    let registration_state = if !packaged {
        "missing_runtime"
    } else if registered {
        "registered"
    } else if registered_dll || profile_enabled {
        "partial"
    } else {
        "not_registered"
    }
    .to_string();
    let (shared_data_dir, shared_data_source) =
        shared_data_status(windows_app_shared_data_dir(core));
    let (reload_stamp_path, reload_stamp_signature) = reload_stamp_status();

    WindowsImeStatus {
        supported: true,
        packaged,
        registered,
        registered_dll,
        profile_enabled,
        registration_busy: false,
        registration_state,
        registration_error: None,
        runtime_dir: runtime_dir.map(|p| windows_normal_path(&p)),
        dll_path: dll_path.map(|p| windows_normal_path(&p)),
        registered_path,
        profile_status,
        user_data_dir: default_user_data_dir_string(),
        shared_data_dir,
        shared_data_source,
        reload_stamp_path,
        reload_stamp_signature,
        message,
    }
}

#[cfg(target_os = "windows")]
fn windows_ime_registering_status(core: &Core, message: String) -> WindowsImeStatus {
    let mut status = windows_ime_status_with_message(core, message);
    status.registration_busy = true;
    status.registration_state = "registering".into();
    status
}

#[cfg(target_os = "windows")]
fn emit_windows_ime_status(core: &Core, status: WindowsImeStatus) {
    core.emit(CoreEvent::WindowsImeStatus(status));
}

#[cfg(target_os = "windows")]
fn windows_ime_data_repair_required() -> bool {
    keytao_core::default_user_data_dir()
        .as_deref()
        .is_some_and(keytao_core::windows_rime_build_repair_required)
}

#[cfg(target_os = "windows")]
fn windows_ime_repairing_status(core: &Core) -> WindowsImeStatus {
    let mut status = windows_ime_status_with_message(
        core,
        "正在后台重建 Windows 输入法词典，完成前不会加载旧构建产物".into(),
    );
    status.registration_busy = true;
    status.registration_state = "repairing".into();
    status
}

#[cfg(target_os = "windows")]
fn windows_ime_repair_failed_status(core: &Core, error: String) -> WindowsImeStatus {
    let mut status =
        windows_ime_status_with_message(core, format!("Windows 输入法词典修复失败：{error}"));
    status.registration_state = "repair_failed".into();
    status.registration_error = Some(error);
    status
}

#[cfg(target_os = "windows")]
fn windows_repair_ime_data_blocking(core: &Core) -> Result<WindowsImeStatus, String> {
    let user_dir = keytao_core::default_user_data_dir().ok_or("无法确定 KeyTao 输入法数据目录")?;
    if !keytao_core::windows_rime_build_repair_required(&user_dir) {
        return Ok(windows_ime_status_with_message(
            core,
            "KeyTao Windows IME 已就绪".into(),
        ));
    }

    let _engine_guard = WindowsImeEngineInitGuard::acquire()?;
    if !keytao_core::windows_rime_build_repair_required(&user_dir) {
        return Ok(windows_ime_status_with_message(
            core,
            "KeyTao Windows IME 已就绪".into(),
        ));
    }
    write_keytao_ime_reload_stamp()
        .map_err(|error| format!("通知现有输入法会话暂停加载失败：{error}"))?;
    let profile_guard = WindowsImeProfileDeploymentGuard::suspend()?;
    let invalidated = keytao_core::invalidate_active_windows_rime_build(&user_dir)?;
    tracing::info!(
        invalidated = invalidated.len(),
        "invalidated Windows RIME build artifacts before repair"
    );
    let shared =
        windows_app_shared_data_dir(core).unwrap_or_else(keytao_core::default_shared_data_dir);
    keytao_core::deploy(user_dir.to_string_lossy().into_owned(), shared)?;
    write_keytao_ime_reload_stamp()
        .map_err(|error| format!("通知现有输入法会话加载新词典失败：{error}"))?;
    keytao_core::mark_windows_rime_build_repair_complete(&user_dir)?;
    profile_guard.resume()?;

    let message = format!(
        "Windows 输入法词典修复完成，已重建 {} 个旧构建产物",
        invalidated.len()
    );
    Ok(windows_ime_status_with_message(core, message))
}

#[cfg(target_os = "windows")]
fn start_windows_ime_data_repair(core: &Arc<Core>) -> WindowsImeStatus {
    let repairing = windows_ime_repairing_status(core);
    if WINDOWS_IME_DATA_REPAIR_IN_PROGRESS.swap(true, Ordering::SeqCst) {
        return repairing;
    }

    let task_app = core.clone();
    tokio::task::spawn_blocking(move || {
        emit_windows_ime_status(&task_app, windows_ime_repairing_status(&task_app));
        let final_status = windows_repair_ime_data_blocking(&task_app)
            .unwrap_or_else(|error| windows_ime_repair_failed_status(&task_app, error));
        WINDOWS_IME_DATA_REPAIR_IN_PROGRESS.store(false, Ordering::SeqCst);
        emit_windows_ime_status(&task_app, final_status);
    });
    repairing
}

#[cfg(target_os = "windows")]
fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(target_os = "windows")]
struct WindowsImeRegistrationTarget {
    label: &'static str,
    source_dll_path: PathBuf,
    dll_path: PathBuf,
    x86: bool,
}

#[cfg(target_os = "windows")]
fn windows_ime_versioned_runtime_root() -> PathBuf {
    let program_data = std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
    let stamp = OffsetDateTime::now_utc().unix_timestamp_nanos();
    program_data
        .join("KeyTao")
        .join("keytao-windows-ime-runtime")
        .join(format!(
            "{}-{}-{stamp}",
            env!("CARGO_PKG_VERSION"),
            std::process::id()
        ))
}

#[cfg(target_os = "windows")]
fn run_regsvr32_elevated(
    targets: &[WindowsImeRegistrationTarget],
    unregister: bool,
) -> Result<(), String> {
    if targets.is_empty() {
        return Err("no Windows IME registration targets".into());
    }
    let temp_dir = std::env::temp_dir();
    let stamp = format!(
        "keytao-ime-register-{}-{}",
        std::process::id(),
        OffsetDateTime::now_utc().unix_timestamp_nanos()
    );
    let script_path = temp_dir.join(format!("{stamp}.ps1"));
    let result_path = temp_dir.join(format!("{stamp}.txt"));
    let system_root = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let registrations = targets
        .iter()
        .map(|target| {
            let source_dll_path = std::fs::canonicalize(&target.source_dll_path)
                .unwrap_or_else(|_| target.source_dll_path.clone());
            let source_dir = source_dll_path
                .parent()
                .ok_or("invalid source keytao_windows_ime.dll path")?;
            let dll_path =
                std::fs::canonicalize(&target.dll_path).unwrap_or_else(|_| target.dll_path.clone());
            let dll_dir = dll_path
                .parent()
                .ok_or("invalid keytao_windows_ime.dll path")?;
            let regsvr32 = system_root.join(if target.x86 && cfg!(target_pointer_width = "64") {
                r"SysWOW64\regsvr32.exe"
            } else {
                r"System32\regsvr32.exe"
            });
            Ok(format!(
                "@{{ Label = {}; Executable = {}; SourceDirectory = {}; Dll = {}; Directory = {} }}",
                powershell_quote(target.label),
                powershell_quote(&windows_normal_path(&regsvr32)),
                powershell_quote(&windows_normal_path(source_dir)),
                powershell_quote(&windows_normal_path(&dll_path)),
                powershell_quote(&windows_normal_path(dll_dir)),
            ))
        })
        .collect::<Result<Vec<_>, String>>()?
        .join(",\n  ");
    let unregister_arg = if unregister { "$arguments += '/u'" } else { "" };
    let prepare_runtime = if unregister { "$false" } else { "$true" };
    let runtime_registry_script = if unregister {
        String::new()
    } else {
        let native = targets
            .iter()
            .find(|target| !target.x86)
            .ok_or("missing native Windows IME registration target")?;
        let native_dir = native
            .dll_path
            .parent()
            .ok_or("invalid native Windows IME registration target")?;
        let runtime_root = native_dir
            .parent()
            .ok_or("invalid versioned Windows IME runtime root")?;
        let x86_dll = targets
            .iter()
            .find(|target| target.x86)
            .map(|target| windows_normal_path(&target.dll_path))
            .unwrap_or_default();
        format!(
            r#"
  $registryPath = 'HKLM:\SOFTWARE\KeyTao'
  New-Item -Path $registryPath -Force | Out-Null
  New-ItemProperty -Path $registryPath -Name 'WindowsImeRuntimeDir' -PropertyType String -Value {runtime_root} -Force | Out-Null
  New-ItemProperty -Path $registryPath -Name 'WindowsImeNativeDll' -PropertyType String -Value {native_dll} -Force | Out-Null
  New-ItemProperty -Path $registryPath -Name 'WindowsImeX86Dll' -PropertyType String -Value {x86_dll} -Force | Out-Null
"#,
            runtime_root = powershell_quote(&windows_normal_path(runtime_root)),
            native_dll = powershell_quote(&windows_normal_path(&native.dll_path)),
            x86_dll = powershell_quote(&x86_dll),
        )
    };
    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
$resultFile = {result}
$prepareRuntime = {prepare_runtime}
$registrations = @(
  {registrations}
)
try {{
  $failures = @()
  foreach ($registration in $registrations) {{
    $oldPath = $env:PATH
    try {{
      if ($prepareRuntime -and $registration.SourceDirectory -ne $registration.Directory) {{
        New-Item -ItemType Directory -Force -Path $registration.Directory | Out-Null
        Get-ChildItem -LiteralPath $registration.SourceDirectory -Force |
          Copy-Item -Destination $registration.Directory -Recurse -Force
      }}
      if (-not (Test-Path -LiteralPath $registration.Dll -PathType Leaf)) {{
        throw "Missing text service DLL after runtime preparation: $($registration.Dll)"
      }}
      Set-Location -LiteralPath $registration.Directory
      $env:PATH = $registration.Directory + ';' + $oldPath
      $arguments = @('/s')
      {unregister_arg}
      $arguments += $registration.Dll
      & $registration.Executable @arguments
      if ($LASTEXITCODE -ne 0) {{
        throw ("regsvr32 failed with exit code {{0}}" -f $LASTEXITCODE)
      }}
    }} catch {{
      $failures += ("{{0}}: {{1}}" -f $registration.Label, $_.Exception.Message)
    }} finally {{
      $env:PATH = $oldPath
    }}
  }}
  if ($failures.Count -gt 0) {{
    ($failures -join [Environment]::NewLine) | Set-Content -Encoding UTF8 -Path $resultFile
    exit 5
  }}
  {runtime_registry_script}
  "Windows IME registration completed" | Set-Content -Encoding UTF8 -Path $resultFile
  exit 0
}} catch {{
  ("PowerShell registration failed: " + $_.Exception.Message) | Set-Content -Encoding UTF8 -Path $resultFile
  exit 9
}}
"#,
        result = powershell_quote(&windows_normal_path(&result_path)),
        prepare_runtime = prepare_runtime,
        registrations = registrations,
        unregister_arg = unregister_arg,
        runtime_registry_script = runtime_registry_script,
    );
    std::fs::write(&script_path, script).map_err(|e| format!("write registration script: {e}"))?;

    let script_argument = format!("\"{}\"", windows_normal_path(&script_path));
    let elevated_script = format!(
        "$p = Start-Process -FilePath powershell.exe -ArgumentList @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', {}) -Verb RunAs -WindowStyle Hidden -Wait -PassThru; exit $p.ExitCode",
        powershell_quote(&script_argument)
    );
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &elevated_script,
        ])
        .output()
        .map_err(|e| format!("start regsvr32: {e}"))?;
    let detail = std::fs::read_to_string(&result_path)
        .unwrap_or_default()
        .trim()
        .to_string();
    let _ = std::fs::remove_file(&script_path);
    let _ = std::fs::remove_file(&result_path);
    if output.status.success() {
        Ok(())
    } else {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let process_detail = [detail.as_str(), stdout.trim(), stderr.trim()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        Err(format!(
            "TSF registration failed with exit code {}{}{}",
            output.status.code().unwrap_or(-1),
            if process_detail.is_empty() { "" } else { ": " },
            process_detail
        ))
    }
}

#[cfg(target_os = "windows")]
pub async fn windows_ime_status(core: Arc<Core>) -> Result<WindowsImeStatus, CoreError> {
    let result: Result<_, String> = async {
        tokio::task::spawn_blocking(move || {
            windows_ime_status_with_message(&core, "已刷新 KeyTao Windows IME 状态".into())
        })
        .await
        .map_err(|e| format!("读取 Windows IME 状态失败：{e}"))
    }
    .await;
    result.map_err(CoreError::Other)
}

#[cfg(target_os = "windows")]
fn windows_register_ime_blocking(core: &Core) -> Result<WindowsImeStatus, String> {
    let source_runtime_dir =
        windows_ime_runtime_dir(core).ok_or("安装包中未找到 keytao_windows_ime.dll")?;
    let runtime_root = windows_ime_versioned_runtime_root();
    let native_runtime_name = if windows_native_machine() == WINDOWS_PE_MACHINE_ARM64 {
        "arm64x"
    } else {
        "current"
    };
    let native_runtime_dir = runtime_root.join(native_runtime_name);
    let mut targets = Vec::new();
    if cfg!(target_pointer_width = "64") {
        let x86_source_runtime_dir = windows_ime_x86_packaged_runtime_dir(core)
            .or_else(|| windows_ime_x86_runtime_dir(core))
            .ok_or("安装包中未找到 x86 keytao_windows_ime.dll")?;
        let x86_runtime_dir = runtime_root.join("x86");
        targets.push(WindowsImeRegistrationTarget {
            label: "x86",
            source_dll_path: x86_source_runtime_dir.join("keytao_windows_ime.dll"),
            dll_path: x86_runtime_dir.join("keytao_windows_ime.dll"),
            x86: true,
        });
    }
    // Register the native profile last so its embedded icon path remains the
    // canonical profile path after both COM registry views are populated.
    targets.push(WindowsImeRegistrationTarget {
        label: "native",
        source_dll_path: source_runtime_dir.join("keytao_windows_ime.dll"),
        dll_path: native_runtime_dir.join("keytao_windows_ime.dll"),
        x86: false,
    });
    run_regsvr32_elevated(&targets, false)?;
    windows_enable_ime_for_current_user()?;
    let status = windows_ime_status_with_message(core, "已注册 KeyTao Windows IME".into());
    if status.registered {
        Ok(status)
    } else {
        Err("regsvr32 已结束，但 x64/x86 COM 注册或 TSF profile 仍不完整；请确认 UAC 已同意，并查看是否有安全软件拦截注册表写入".into())
    }
}
#[cfg(target_os = "windows")]
pub async fn windows_ime_ensure_registered(core: Arc<Core>) -> Result<WindowsImeStatus, CoreError> {
    let result: Result<_, String> = async {
        let status_app = core.clone();
        let status = tokio::task::spawn_blocking(move || {
            windows_ime_status_with_message(&status_app, "正在检测 KeyTao Windows IME 状态".into())
        })
        .await
        .map_err(|e| format!("读取 Windows IME 状态失败：{e}"))?;

        if !status.packaged {
            return Ok(status);
        }
        if status.registered {
            if windows_ime_data_repair_required() {
                return Ok(start_windows_ime_data_repair(&core));
            }
            let mut ready = status;
            ready.message = "KeyTao Windows IME 已就绪".into();
            return Ok(ready);
        }

        let can_repair_current_user = status.registered_dll;
        let initial_message = if can_repair_current_user {
            "正在为当前用户启用 KeyTao Windows IME"
        } else {
            "正在注册 KeyTao Windows IME，请确认 Windows UAC 提示"
        };
        if WINDOWS_IME_REGISTRATION_IN_PROGRESS.swap(true, Ordering::SeqCst) {
            return Ok(windows_ime_registering_status(&core, initial_message.into()));
        }

        let task_app = core.clone();
        tokio::task::spawn_blocking(move || {
            emit_windows_ime_status(
                &task_app,
                windows_ime_registering_status(&task_app, initial_message.into()),
            );

            let registration_result = if can_repair_current_user {
                match windows_enable_ime_for_current_user() {
                    Ok(()) => Ok(windows_ime_status_with_message(
                        &task_app,
                        "已为当前用户启用 KeyTao Windows IME".into(),
                    )),
                    Err(profile_error) => {
                        emit_windows_ime_status(
                            &task_app,
                            windows_ime_registering_status(
                                &task_app,
                                "当前用户 profile 修复失败，正在请求 UAC 完成注册".into(),
                            ),
                        );
                        windows_register_ime_blocking(&task_app).map_err(|registration_error| {
                            format!(
                                "当前用户 profile 修复失败：{profile_error}；完整注册失败：{registration_error}"
                            )
                        })
                    }
                }
            } else {
                windows_register_ime_blocking(&task_app)
            };

            let final_status = match registration_result {
                Ok(_status) if windows_ime_data_repair_required() => {
                    WINDOWS_IME_DATA_REPAIR_IN_PROGRESS.store(true, Ordering::SeqCst);
                    emit_windows_ime_status(&task_app, windows_ime_repairing_status(&task_app));
                    let repaired = windows_repair_ime_data_blocking(&task_app)
                        .unwrap_or_else(|error| windows_ime_repair_failed_status(&task_app, error));
                    WINDOWS_IME_DATA_REPAIR_IN_PROGRESS.store(false, Ordering::SeqCst);
                    repaired
                }
                Ok(status) => status,
                Err(error) => {
                    let mut status = windows_ime_status_with_message(
                        &task_app,
                        format!("KeyTao Windows IME 注册失败：{error}"),
                    );
                    status.registration_state = "failed".into();
                    status.registration_error = Some(error);
                    status
                }
            };

            WINDOWS_IME_REGISTRATION_IN_PROGRESS.store(false, Ordering::SeqCst);
            emit_windows_ime_status(&task_app, final_status);
        });

        Ok(windows_ime_registering_status(&core, initial_message.into()))
    }.await;
    result.map_err(CoreError::Other)
}
#[cfg(target_os = "windows")]
pub fn windows_deploy_rime_blocking(
    user_dir: PathBuf,
    shared_data_dir: String,
) -> Result<(), String> {
    let _engine_guard = WindowsImeEngineInitGuard::acquire()?;
    keytao_core::clear_windows_rime_build_repair_marker(&user_dir)?;
    write_keytao_ime_reload_stamp()
        .map_err(|error| format!("通知现有输入法会话暂停加载失败：{error}"))?;
    let profile_guard = WindowsImeProfileDeploymentGuard::suspend()?;
    keytao_core::invalidate_active_windows_rime_build(&user_dir)?;
    keytao_core::deploy(user_dir.to_string_lossy().into_owned(), shared_data_dir)?;
    write_keytao_ime_reload_stamp()
        .map_err(|error| format!("通知现有输入法会话加载新词典失败：{error}"))?;
    keytao_core::mark_windows_rime_build_repair_complete(&user_dir)?;
    profile_guard.resume()
}
