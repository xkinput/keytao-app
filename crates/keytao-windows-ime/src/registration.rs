//! DllRegisterServer / DllUnregisterServer - TSF TIP registration.
//!
//! Registration steps (mirrors what regsvr32 does for TSF TIPs):
//!   1. Write CLSID to InprocServer32 in HKCR
//!   2. ITfInputProcessorProfiles::Register tells TSF about us
//!   3. ITfCategoryMgr::RegisterCategory marks us as a keyboard TIP
//!
//! Run: regsvr32 keytao_windows_ime.dll
//! Undo: regsvr32 /u keytao_windows_ime.dll

use windows::{
    core::{Error, IUnknown, Interface, Result, HRESULT, PCSTR, PCWSTR},
    Win32::{
        Foundation::{FreeLibrary, BOOL, RPC_E_CHANGED_MODE},
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
            COINIT_APARTMENTTHREADED,
        },
        System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32},
        System::Registry::{
            RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW, HKEY, HKEY_CLASSES_ROOT,
            KEY_WRITE, REG_CREATE_KEY_DISPOSITION, REG_OPTION_NON_VOLATILE, REG_SZ,
        },
        UI::Input::KeyboardAndMouse::HKL,
        UI::TextServices::{
            CLSID_TF_CategoryMgr, CLSID_TF_InputProcessorProfiles, ITfCategoryMgr,
            ITfInputProcessorProfileMgr, ITfInputProcessorProfiles,
            GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER, GUID_TFCAT_TIPCAP_INPUTMODECOMPARTMENT,
            GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT, GUID_TFCAT_TIPCAP_UIELEMENTENABLED,
            GUID_TFCAT_TIP_KEYBOARD,
        },
    },
};

use std::path::PathBuf;

use crate::{CLSID_TEXT_SERVICE, GUID_PROFILE, LANGID_CHINESE_SIMPLIFIED, PROFILE_ICON_INDEX};

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

fn init_com_apartment() -> Result<ComApartment> {
    let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if result.is_ok() {
        Ok(ComApartment(true))
    } else if result == RPC_E_CHANGED_MODE {
        Ok(ComApartment(false))
    } else {
        Err(result.into())
    }
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
            category: GUID_TFCAT_TIP_KEYBOARD,
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
        CategoryRegistration {
            category: GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER,
            item: CLSID_TEXT_SERVICE,
        },
        CategoryRegistration {
            category: GUID_TFCAT_TIPCAP_INPUTMODECOMPARTMENT,
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
    if let Some(profile_mgr) = profile_mgr {
        profile_mgr.RegisterProfile(
            &CLSID_TEXT_SERVICE,
            LANGID_CHINESE_SIMPLIFIED,
            &GUID_PROFILE,
            profile_desc,
            icon_path,
            PROFILE_ICON_INDEX,
            HKL::default(),
            0,
            BOOL::from(true),
            0,
        )?;
    } else {
        profiles.Register(&CLSID_TEXT_SERVICE)?;
        profiles.AddLanguageProfile(
            &CLSID_TEXT_SERVICE,
            LANGID_CHINESE_SIMPLIFIED,
            &GUID_PROFILE,
            profile_desc,
            icon_path,
            PROFILE_ICON_INDEX,
        )?;
    }

    Ok(())
}

fn tip_install_string() -> String {
    format!(
        "0x{:04X}:{}{}",
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
    let module = LoadLibraryExW(
        PCWSTR(input_dll.as_ptr()),
        None,
        LOAD_LIBRARY_SEARCH_SYSTEM32,
    )?;
    let Some(proc) = GetProcAddress(module, PCSTR(c"InstallLayoutOrTip".as_ptr().cast())) else {
        let error = Error::from_win32();
        let _ = FreeLibrary(module);
        return Err(error);
    };
    let install_layout_or_tip: InstallLayoutOrTipFn = std::mem::transmute(proc);
    let result = install_layout_or_tip(tip, flags);
    let _ = FreeLibrary(module);
    Ok(result)
}

unsafe fn enable_profile_for_current_user(profiles: &ITfInputProcessorProfiles) -> Result<()> {
    profiles.EnableLanguageProfile(
        &CLSID_TEXT_SERVICE,
        LANGID_CHINESE_SIMPLIFIED,
        &GUID_PROFILE,
        BOOL::from(true),
    )?;

    let enabled = profiles.IsEnabledLanguageProfile(
        &CLSID_TEXT_SERVICE,
        LANGID_CHINESE_SIMPLIFIED,
        &GUID_PROFILE,
    )?;
    if !enabled.as_bool() {
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

fn com_server_path(dll: &str) -> String {
    let path = PathBuf::from(dll);
    let is_arm64x_target = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.eq_ignore_ascii_case("keytao_windows_ime_arm64.dll")
                || name.eq_ignore_ascii_case("keytao_windows_ime_x64.dll")
        });
    if !is_arm64x_target {
        return dll.to_owned();
    }

    let wrapper = path.with_file_name("keytao_windows_ime.dll");
    if wrapper.is_file() {
        wrapper.to_string_lossy().into_owned()
    } else {
        dll.to_owned()
    }
}

// Public API

pub fn register() -> Result<()> {
    let _com = init_com_apartment()?;
    let dll = dll_path()?;
    let inproc_server = com_server_path(&dll);
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
        reg_set_sz(key_inproc, "", &inproc_server)?;
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
        let icon_path = to_wide(&dll);

        let profile_mgr = profiles.cast::<ITfInputProcessorProfileMgr>().ok();
        register_profile(
            &profiles,
            profile_mgr.as_ref(),
            &profile_desc[..profile_desc.len() - 1],
            &icon_path[..icon_path.len() - 1],
        )?;

        // 5. Add the TIP to the current user's enabled input methods. RegisterProfile
        // creates the profile, but Windows does not necessarily make it selectable
        // for the user until InstallLayoutOrTip is applied.
        install_tip_for_current_user()?;

        // 6. InstallLayoutOrTip owns the current-user input list. Re-apply the
        // enabled state afterward and verify the final state exposed by TSF.
        enable_profile_for_current_user(&profiles)?;
    }

    tracing::info!("KeyTao TSF registered (CLSID={})", clsid_str);
    Ok(())
}

pub fn unregister() -> Result<()> {
    let _com = init_com_apartment()?;
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
