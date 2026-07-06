//! DllRegisterServer / DllUnregisterServer - TSF TIP registration.
//!
//! Registration steps (mirrors what regsvr32 does for TSF TIPs):
//!   1. Write CLSID to InprocServer32 in HKCR
//!   2. ITfInputProcessorProfiles::Register tells TSF about us
//!   3. ITfCategoryMgr::RegisterCategory marks us as a keyboard TIP
//!
//! Run: regsvr32 keytao_windows_ime.dll
//! Undo: regsvr32 /u keytao_windows_ime.dll

use std::path::{Path, PathBuf};

use windows::{
    core::{Error, IUnknown, Interface, Result, HRESULT, PCSTR, PCWSTR},
    Win32::{
        Foundation::{FreeLibrary, BOOL},
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
            COINIT_APARTMENTTHREADED,
        },
        System::LibraryLoader::{GetProcAddress, LoadLibraryW},
        System::Registry::{
            RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW, HKEY, HKEY_CLASSES_ROOT,
            KEY_WRITE, REG_CREATE_KEY_DISPOSITION, REG_OPTION_NON_VOLATILE, REG_SZ,
        },
        UI::Input::KeyboardAndMouse::HKL,
        UI::TextServices::{
            CLSID_TF_CategoryMgr, CLSID_TF_InputProcessorProfiles, ITfCategoryMgr,
            ITfInputProcessorProfileMgr, ITfInputProcessorProfiles, GUID_TFCAT_CATEGORY_OF_TIP,
            GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT, GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT,
            GUID_TFCAT_TIPCAP_UIELEMENTENABLED, GUID_TFCAT_TIP_KEYBOARD, TF_INPUTPROCESSORPROFILE,
            TF_IPP_FLAG_ENABLED, TF_PROFILETYPE_INPUTPROCESSOR,
        },
    },
};

use crate::{CLSID_TEXT_SERVICE, GUID_PROFILE, LANGID_CHINESE_SIMPLIFIED};

struct ComApartment(bool);

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                CoUninitialize();
            }
        }
    }
}

fn init_com_apartment() -> ComApartment {
    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok() };
    ComApartment(initialized)
}

fn to_wide(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

fn guid_to_string(guid: &windows::core::GUID) -> String {
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        guid.data1,
        guid.data2,
        guid.data3,
        guid.data4[0],
        guid.data4[1],
        guid.data4[2],
        guid.data4[3],
        guid.data4[4],
        guid.data4[5],
        guid.data4[6],
        guid.data4[7],
    )
}

fn error_message(message: impl Into<String>) -> Error {
    Error::new(HRESULT(0x80004005u32 as i32), message.into())
}

const ILOT_UNINSTALL: u32 = 0x0000_0001;

type InstallLayoutOrTipFn = unsafe extern "system" fn(PCWSTR, u32) -> BOOL;

struct CategoryRegistration {
    category: windows::core::GUID,
    item: windows::core::GUID,
}

fn category_registrations() -> [CategoryRegistration; 5] {
    [
        CategoryRegistration {
            category: GUID_TFCAT_CATEGORY_OF_TIP,
            item: GUID_TFCAT_TIP_KEYBOARD,
        },
        CategoryRegistration {
            category: GUID_TFCAT_TIP_KEYBOARD,
            item: CLSID_TEXT_SERVICE,
        },
        CategoryRegistration {
            category: GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT,
            item: CLSID_TEXT_SERVICE,
        },
        CategoryRegistration {
            category: GUID_TFCAT_TIPCAP_UIELEMENTENABLED,
            item: CLSID_TEXT_SERVICE,
        },
        CategoryRegistration {
            category: GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT,
            item: CLSID_TEXT_SERVICE,
        },
    ]
}

unsafe fn register_categories(cat_mgr: &ITfCategoryMgr) -> Result<()> {
    for entry in category_registrations() {
        cat_mgr.RegisterCategory(&CLSID_TEXT_SERVICE, &entry.category, &entry.item)?;
    }
    Ok(())
}

unsafe fn unregister_categories(cat_mgr: &ITfCategoryMgr) {
    for entry in category_registrations() {
        let _ = cat_mgr.UnregisterCategory(&CLSID_TEXT_SERVICE, &entry.category, &entry.item);
    }
}

unsafe fn register_profile(
    profiles: &ITfInputProcessorProfiles,
    profile_mgr: Option<&ITfInputProcessorProfileMgr>,
    profile_desc: &[u16],
    icon_path: &[u16],
) -> Result<()> {
    profiles.Register(&CLSID_TEXT_SERVICE)?;

    if let Some(profile_mgr) = profile_mgr {
        let _ = profile_mgr.UnregisterProfile(
            &CLSID_TEXT_SERVICE,
            LANGID_CHINESE_SIMPLIFIED,
            &GUID_PROFILE,
            0,
        );
        profile_mgr.RegisterProfile(
            &CLSID_TEXT_SERVICE,
            LANGID_CHINESE_SIMPLIFIED,
            &GUID_PROFILE,
            profile_desc,
            icon_path,
            0,
            HKL::default(),
            0,
            BOOL::from(true),
            0,
        )?;
    } else {
        let _ = profiles.RemoveLanguageProfile(
            &CLSID_TEXT_SERVICE,
            LANGID_CHINESE_SIMPLIFIED,
            &GUID_PROFILE,
        );
        profiles.AddLanguageProfile(
            &CLSID_TEXT_SERVICE,
            LANGID_CHINESE_SIMPLIFIED,
            &GUID_PROFILE,
            profile_desc,
            icon_path,
            0,
        )?;
    }

    profiles.EnableLanguageProfile(
        &CLSID_TEXT_SERVICE,
        LANGID_CHINESE_SIMPLIFIED,
        &GUID_PROFILE,
        BOOL::from(true),
    )?;
    if let Err(e) = profiles.EnableLanguageProfileByDefault(
        &CLSID_TEXT_SERVICE,
        LANGID_CHINESE_SIMPLIFIED,
        &GUID_PROFILE,
        BOOL::from(true),
    ) {
        tracing::warn!("failed to enable KeyTao profile by default: {e}");
    }

    Ok(())
}

fn tip_install_string() -> String {
    format!(
        "0x{:04X}:{}{};",
        LANGID_CHINESE_SIMPLIFIED,
        guid_to_string(&CLSID_TEXT_SERVICE),
        guid_to_string(&GUID_PROFILE)
    )
}

unsafe fn install_tip_for_current_user() -> Result<()> {
    let tip = to_wide(&tip_install_string());
    if call_install_layout_or_tip(PCWSTR(tip.as_ptr()), 0)?.as_bool() {
        Ok(())
    } else {
        Err(Error::from_win32())
    }
}

unsafe fn uninstall_tip_for_current_user() {
    let tip = to_wide(&tip_install_string());
    let _ = call_install_layout_or_tip(PCWSTR(tip.as_ptr()), ILOT_UNINSTALL);
}

unsafe fn call_install_layout_or_tip(tip: PCWSTR, flags: u32) -> Result<BOOL> {
    let input_dll = to_wide("input.dll");
    let module = LoadLibraryW(PCWSTR(input_dll.as_ptr()))?;
    let Some(proc) = GetProcAddress(module, PCSTR(b"InstallLayoutOrTip\0".as_ptr())) else {
        let error = Error::from_win32();
        let _ = FreeLibrary(module);
        return Err(error);
    };
    let install_layout_or_tip: InstallLayoutOrTipFn = std::mem::transmute(proc);
    let result = install_layout_or_tip(tip, flags);
    let _ = FreeLibrary(module);
    Ok(result)
}

unsafe fn ensure_profile_enabled(profile_mgr: &ITfInputProcessorProfileMgr) -> Result<()> {
    let mut profile = TF_INPUTPROCESSORPROFILE::default();
    profile_mgr.GetProfile(
        TF_PROFILETYPE_INPUTPROCESSOR,
        LANGID_CHINESE_SIMPLIFIED,
        &CLSID_TEXT_SERVICE,
        &GUID_PROFILE,
        HKL::default(),
        &mut profile,
    )?;
    if profile.dwFlags & TF_IPP_FLAG_ENABLED == 0 {
        return Err(error_message(
            "KeyTao TSF profile was registered but is not enabled for the current user",
        ));
    }
    Ok(())
}

/// Write a REG_SZ value under a registry key.
unsafe fn reg_set_sz(hkey: HKEY, name: &str, value: &str) -> Result<()> {
    let name_w = to_wide(name);
    let val_w = to_wide(value);
    let bytes = std::slice::from_raw_parts(val_w.as_ptr() as *const u8, val_w.len() * 2);
    RegSetValueExW(hkey, PCWSTR(name_w.as_ptr()), 0, REG_SZ, Some(bytes)).ok()
}

/// Create (or open) a registry key under HKCR and return its HKEY.
unsafe fn reg_create(parent: HKEY, subkey: &str) -> Result<HKEY> {
    let subkey_w = to_wide(subkey);
    let mut hkey = HKEY::default();
    let mut disposition = REG_CREATE_KEY_DISPOSITION(0);
    RegCreateKeyExW(
        parent,
        PCWSTR(subkey_w.as_ptr()),
        0,
        PCWSTR::null(),
        REG_OPTION_NON_VOLATILE,
        KEY_WRITE,
        None,
        &mut hkey,
        Some(&mut disposition as *mut _),
    )
    .ok()?;
    Ok(hkey)
}

/// Get the full path of this DLL from the OS.
fn dll_path() -> Result<String> {
    use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
    let hmod = crate::globals::DLL_INSTANCE
        .get()
        .map(|raw| windows::Win32::Foundation::HMODULE(*raw as _))
        .unwrap_or_default();
    let mut buf = vec![0u16; 32768];
    let len = unsafe { GetModuleFileNameW(hmod, &mut buf) } as usize;
    if len == 0 || len >= buf.len() {
        return Err(windows::core::Error::from_win32());
    }
    Ok(String::from_utf16_lossy(&buf[..len]))
}

fn profile_icon_path(dll: &str) -> Result<PathBuf> {
    let dll_path = Path::new(dll);
    let Some(dir) = dll_path.parent() else {
        return Err(error_message("KeyTao TSF DLL path has no parent directory"));
    };
    let icon = dir.join("keytao.ico");
    if icon.is_file() {
        Ok(icon)
    } else {
        Err(error_message(format!(
            "KeyTao TSF profile icon is missing: {}",
            icon.display()
        )))
    }
}

// Public API

pub fn register() -> Result<()> {
    let _com = init_com_apartment();
    let dll = dll_path()?;
    let clsid_str = guid_to_string(&CLSID_TEXT_SERVICE);

    unsafe {
        // 1. HKCR\CLSID\{...}
        let key_clsid = reg_create(HKEY_CLASSES_ROOT, &format!("CLSID\\{}", clsid_str))?;
        reg_set_sz(key_clsid, "", "KeyTao Input Method")?;
        let _ = RegCloseKey(key_clsid).ok();

        // 2. HKCR\CLSID\{...}\InprocServer32
        let key_inproc = reg_create(
            HKEY_CLASSES_ROOT,
            &format!("CLSID\\{}\\InprocServer32", clsid_str),
        )?;
        reg_set_sz(key_inproc, "", &dll)?;
        reg_set_sz(key_inproc, "ThreadingModel", "Apartment")?;
        let _ = RegCloseKey(key_inproc).ok();

        // 3. Register TSF categories before exposing/enabling the language profile.
        let cat_mgr: ITfCategoryMgr = CoCreateInstance(
            &CLSID_TF_CategoryMgr,
            None::<&IUnknown>,
            CLSCTX_INPROC_SERVER,
        )?;
        register_categories(&cat_mgr)?;

        // 4. Register with TSF via ITfInputProcessorProfiles.
        let profiles: ITfInputProcessorProfiles = CoCreateInstance(
            &CLSID_TF_InputProcessorProfiles,
            None::<&IUnknown>,
            CLSCTX_INPROC_SERVER,
        )?;
        let profile_desc = to_wide("KeyTao");
        let icon_path = profile_icon_path(&dll)?;
        let icon_path = to_wide(&icon_path.to_string_lossy());

        let profile_mgr = profiles.cast::<ITfInputProcessorProfileMgr>().ok();
        register_profile(
            &profiles,
            profile_mgr.as_ref(),
            &profile_desc[..profile_desc.len() - 1],
            &icon_path[..icon_path.len() - 1],
        )?;

        // 5. The modern profile API exposes the enabled flag that the input
        // switcher cares about. Treat a disabled profile as registration failure.
        if let Some(profile_mgr) = profile_mgr.as_ref() {
            ensure_profile_enabled(profile_mgr)?;
        }

        // Add the TIP to the current user's enabled input methods. RegisterProfile
        // creates the profile, but Windows does not necessarily make it selectable
        // for the user until InstallLayoutOrTip is applied.
        install_tip_for_current_user()?;
    }

    tracing::info!("KeyTao TSF registered (CLSID={})", clsid_str);
    Ok(())
}

pub fn unregister() -> Result<()> {
    let _com = init_com_apartment();
    let clsid_str = guid_to_string(&CLSID_TEXT_SERVICE);

    unsafe {
        uninstall_tip_for_current_user();

        // Remove TSF registrations first
        let profiles: windows::core::Result<ITfInputProcessorProfiles> = CoCreateInstance(
            &CLSID_TF_InputProcessorProfiles,
            None::<&IUnknown>,
            CLSCTX_INPROC_SERVER,
        );
        if let Ok(profiles) = profiles {
            if let Ok(profile_mgr) = profiles.cast::<ITfInputProcessorProfileMgr>() {
                let _ = profile_mgr.UnregisterProfile(
                    &CLSID_TEXT_SERVICE,
                    LANGID_CHINESE_SIMPLIFIED,
                    &GUID_PROFILE,
                    0,
                );
            }
            let _ = profiles.RemoveLanguageProfile(
                &CLSID_TEXT_SERVICE,
                LANGID_CHINESE_SIMPLIFIED,
                &GUID_PROFILE,
            );
            let _ = profiles.Unregister(&CLSID_TEXT_SERVICE);
        }

        let cat_mgr: windows::core::Result<ITfCategoryMgr> = CoCreateInstance(
            &CLSID_TF_CategoryMgr,
            None::<&IUnknown>,
            CLSCTX_INPROC_SERVER,
        );
        if let Ok(cat_mgr) = cat_mgr {
            unregister_categories(&cat_mgr);
        }

        // Remove HKCR\CLSID\{...} tree
        let clsid_w = to_wide(&format!("CLSID\\{}", clsid_str));
        let _ = RegDeleteTreeW(HKEY_CLASSES_ROOT, PCWSTR(clsid_w.as_ptr()));
    }

    tracing::info!("KeyTao TSF unregistered");
    Ok(())
}
