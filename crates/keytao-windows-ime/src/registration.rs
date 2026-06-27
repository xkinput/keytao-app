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
    core::{IUnknown, Interface, Result, PCWSTR},
    Win32::{
        Foundation::BOOL,
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
            COINIT_APARTMENTTHREADED,
        },
        System::Registry::{
            RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW, HKEY, HKEY_CLASSES_ROOT,
            KEY_WRITE, REG_CREATE_KEY_DISPOSITION, REG_OPTION_NON_VOLATILE, REG_SZ,
        },
        UI::Input::KeyboardAndMouse::HKL,
        UI::TextServices::{
            CLSID_TF_CategoryMgr, CLSID_TF_InputProcessorProfiles, ITfCategoryMgr,
            ITfInputProcessorProfileMgr, ITfInputProcessorProfiles,
            GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT, GUID_TFCAT_TIPCAP_UIELEMENTENABLED,
            GUID_TFCAT_TIP_KEYBOARD,
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

        // 3. Register with TSF via ITfInputProcessorProfiles
        let profiles: ITfInputProcessorProfiles = CoCreateInstance(
            &CLSID_TF_InputProcessorProfiles,
            None::<&IUnknown>,
            CLSCTX_INPROC_SERVER,
        )?;

        profiles.Register(&CLSID_TEXT_SERVICE)?;
        let profile_desc = to_wide("KeyTao");
        let icon_path = to_wide(&dll);
        profiles.AddLanguageProfile(
            &CLSID_TEXT_SERVICE,
            LANGID_CHINESE_SIMPLIFIED,
            &GUID_PROFILE,
            &profile_desc[..profile_desc.len() - 1],
            // Icon: use the DLL itself at resource index 0 (can be updated later)
            &icon_path[..icon_path.len() - 1],
            0,
        )?;
        if let Ok(profile_mgr) = profiles.cast::<ITfInputProcessorProfileMgr>() {
            if let Err(e) = profile_mgr.RegisterProfile(
                &CLSID_TEXT_SERVICE,
                LANGID_CHINESE_SIMPLIFIED,
                &GUID_PROFILE,
                &profile_desc[..profile_desc.len() - 1],
                &icon_path[..icon_path.len() - 1],
                0,
                HKL::default(),
                0,
                BOOL::from(true),
                0,
            ) {
                tracing::warn!("modern TSF RegisterProfile fallback failed: {e}");
            }
        }

        // 4. Register category: keyboard TIP
        let cat_mgr: ITfCategoryMgr = CoCreateInstance(
            &CLSID_TF_CategoryMgr,
            None::<&IUnknown>,
            CLSCTX_INPROC_SERVER,
        )?;
        for category in [
            GUID_TFCAT_TIP_KEYBOARD,
            GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT,
            GUID_TFCAT_TIPCAP_UIELEMENTENABLED,
        ] {
            cat_mgr.RegisterCategory(&CLSID_TEXT_SERVICE, &category, &CLSID_TEXT_SERVICE)?;
        }

        // 5. AddLanguageProfile records the profile, but the input switcher only
        // offers profiles that are enabled for the current user.
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
    }

    tracing::info!("KeyTao TSF registered (CLSID={})", clsid_str);
    Ok(())
}

pub fn unregister() -> Result<()> {
    let _com = init_com_apartment();
    let clsid_str = guid_to_string(&CLSID_TEXT_SERVICE);

    unsafe {
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
            for category in [
                GUID_TFCAT_TIP_KEYBOARD,
                GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT,
                GUID_TFCAT_TIPCAP_UIELEMENTENABLED,
            ] {
                let _ =
                    cat_mgr.UnregisterCategory(&CLSID_TEXT_SERVICE, &category, &CLSID_TEXT_SERVICE);
            }
        }

        // Remove HKCR\CLSID\{...} tree
        let clsid_w = to_wide(&format!("CLSID\\{}", clsid_str));
        let _ = RegDeleteTreeW(HKEY_CLASSES_ROOT, PCWSTR(clsid_w.as_ptr()));
    }

    tracing::info!("KeyTao TSF unregistered");
    Ok(())
}
