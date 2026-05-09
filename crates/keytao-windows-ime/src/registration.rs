//! DllRegisterServer / DllUnregisterServer — TSF TIP registration.
//!
//! Registration steps (mirrors what regsvr32 does for TSF TIPs):
//!   1. Write CLSID → InprocServer32 in HKCR
//!   2. ITfInputProcessorProfiles::Register  → tells TSF about us
//!   3. ITfCategoryMgr::RegisterCategory     → marks us as a keyboard TIP
//!
//! Run: regsvr32 keytao_windows_ime.dll
//! Undo: regsvr32 /u keytao_windows_ime.dll

use windows::{
    core::{Result, PCWSTR},
    Win32::{
        System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
        System::Registry::{
            RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW, HKEY,
            HKEY_CLASSES_ROOT, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
        },
        UI::TextServices::{
            ITfCategoryMgr, ITfInputProcessorProfiles, CLSID_TF_CategoryMgr,
            CLSID_TF_InputProcessorProfiles, GUID_TFCAT_TIP_KEYBOARD,
        },
    },
};

use crate::{CLSID_TEXT_SERVICE, GUID_PROFILE, LANGID_CHINESE_SIMPLIFIED};

fn to_wide(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

fn guid_to_string(guid: &windows::core::GUID) -> String {
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        guid.data1, guid.data2, guid.data3,
        guid.data4[0], guid.data4[1], guid.data4[2], guid.data4[3],
        guid.data4[4], guid.data4[5], guid.data4[6], guid.data4[7],
    )
}

/// Write a REG_SZ value under a registry key.
unsafe fn reg_set_sz(hkey: HKEY, name: &str, value: &str) -> Result<()> {
    let name_w = to_wide(name);
    let val_w = to_wide(value);
    let bytes = std::slice::from_raw_parts(
        val_w.as_ptr() as *const u8,
        val_w.len() * 2,
    );
    RegSetValueExW(
        hkey,
        PCWSTR(name_w.as_ptr()),
        0,
        REG_SZ,
        Some(bytes),
    )
    .ok()
}

/// Create (or open) a registry key under HKCR and return its HKEY.
unsafe fn reg_create(parent: HKEY, subkey: &str) -> Result<HKEY> {
    let subkey_w = to_wide(subkey);
    let mut hkey = HKEY::default();
    let mut disposition = 0u32;
    RegCreateKeyExW(
        parent,
        PCWSTR(subkey_w.as_ptr()),
        0,
        PCWSTR::null(),
        REG_OPTION_NON_VOLATILE,
        KEY_WRITE,
        None,
        &mut hkey,
        Some(&mut disposition),
    )
    .ok()?;
    Ok(hkey)
}

/// Get the full path of this DLL from the OS.
fn dll_path() -> Result<String> {
    use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
    let hmod = crate::globals::DLL_INSTANCE
        .get()
        .copied()
        .unwrap_or_default();
    let mut buf = vec![0u16; 260];
    let len = unsafe { GetModuleFileNameW(hmod, &mut buf) } as usize;
    Ok(String::from_utf16_lossy(&buf[..len]))
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn register() -> Result<()> {
    let dll = dll_path()?;
    let clsid_str = guid_to_string(&CLSID_TEXT_SERVICE);

    unsafe {
        // 1. HKCR\CLSID\{...}
        let key_clsid =
            reg_create(HKEY_CLASSES_ROOT, &format!("CLSID\\{}", clsid_str))?;
        reg_set_sz(key_clsid, "", "KeyTao Input Method")?;
        RegCloseKey(key_clsid).ok();

        // 2. HKCR\CLSID\{...}\InprocServer32
        let key_inproc = reg_create(
            HKEY_CLASSES_ROOT,
            &format!("CLSID\\{}\\InprocServer32", clsid_str),
        )?;
        reg_set_sz(key_inproc, "", &dll)?;
        reg_set_sz(key_inproc, "ThreadingModel", "Apartment")?;
        RegCloseKey(key_inproc).ok();

        // 3. Register with TSF via ITfInputProcessorProfiles
        let profiles: ITfInputProcessorProfiles =
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;

        profiles.Register(&CLSID_TEXT_SERVICE)?;
        profiles.AddLanguageProfile(
            &CLSID_TEXT_SERVICE,
            LANGID_CHINESE_SIMPLIFIED,
            &GUID_PROFILE,
            &to_wide("键道输入法")[..to_wide("键道输入法").len() - 1], // strip null
            // Icon: use the DLL itself at resource index 0 (can be updated later)
            &to_wide(&dll)[..to_wide(&dll).len() - 1],
            0,
        )?;

        // 4. Register category: keyboard TIP
        let cat_mgr: ITfCategoryMgr =
            CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?;
        cat_mgr.RegisterCategory(
            &CLSID_TEXT_SERVICE,
            &GUID_TFCAT_TIP_KEYBOARD,
            &CLSID_TEXT_SERVICE,
        )?;
    }

    tracing::info!("KeyTao TSF registered (CLSID={})", clsid_str);
    Ok(())
}

pub fn unregister() -> Result<()> {
    let clsid_str = guid_to_string(&CLSID_TEXT_SERVICE);

    unsafe {
        // Remove TSF registrations first
        if let Ok(profiles) = CoCreateInstance::<ITfInputProcessorProfiles>(
            &CLSID_TF_InputProcessorProfiles,
            None,
            CLSCTX_INPROC_SERVER,
        ) {
            let _ = profiles.RemoveLanguageProfile(
                &CLSID_TEXT_SERVICE,
                LANGID_CHINESE_SIMPLIFIED,
                &GUID_PROFILE,
            );
            let _ = profiles.Unregister(&CLSID_TEXT_SERVICE);
        }

        if let Ok(cat_mgr) = CoCreateInstance::<ITfCategoryMgr>(
            &CLSID_TF_CategoryMgr,
            None,
            CLSCTX_INPROC_SERVER,
        ) {
            let _ = cat_mgr.UnregisterCategory(
                &CLSID_TEXT_SERVICE,
                &GUID_TFCAT_TIP_KEYBOARD,
                &CLSID_TEXT_SERVICE,
            );
        }

        // Remove HKCR\CLSID\{...} tree
        let clsid_w = to_wide(&format!("CLSID\\{}", clsid_str));
        let _ = RegDeleteTreeW(HKEY_CLASSES_ROOT, PCWSTR(clsid_w.as_ptr()));
    }

    tracing::info!("KeyTao TSF unregistered");
    Ok(())
}
