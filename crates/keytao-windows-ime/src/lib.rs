//! DLL entry point and exported COM/IME functions.
//!
//! Windows TSF TIP architecture (mirrors keytao-linux-ime structure):
//!
//!   DllGetClassObject            — returns ClassFactory
//!   ClassFactory::CreateInstance — creates TextService COM object
//!   ITfTextInputProcessor::Activate  — registers KeyEventSink + ThreadMgrSink
//!   ITfKeyEventSink::OnKeyDown       — engine.process_key → update panel
//!
//! Build:   cargo build --target x86_64-pc-windows-msvc --release
//! Install: regsvr32 keytao_windows_ime.dll

#![cfg(target_os = "windows")]
#![allow(non_snake_case)]

mod candidate_ui;
mod candidate_win;
mod display_attribute;
mod globals;
mod key_event_sink;
mod key_map;
mod panel;
mod registration;
mod state;
mod text_service;

use windows::{
    core::{Interface, GUID, HRESULT},
    Win32::{
        Foundation::{BOOL, E_POINTER, HMODULE, S_FALSE, S_OK},
        System::Com::IClassFactory,
        System::LibraryLoader::DisableThreadLibraryCalls,
    },
};

use globals::{can_unload, DLL_INSTANCE};
use text_service::ClassFactory;

// ── Well-known GUIDs ──────────────────────────────────────────────────────────

/// TextService CLSID — {4A5C6D7E-8F90-1A2B-3C4D-5E6F7A8B9C0D}
pub const CLSID_TEXT_SERVICE: GUID = GUID {
    data1: 0x4A5C6D7E,
    data2: 0x8F90,
    data3: 0x1A2B,
    data4: [0x3C, 0x4D, 0x5E, 0x6F, 0x7A, 0x8B, 0x9C, 0x0D],
};

/// Language profile GUID — {1B2C3D4E-5F60-7A8B-9C0D-1E2F3A4B5C6D}
pub const GUID_PROFILE: GUID = GUID {
    data1: 0x1B2C3D4E,
    data2: 0x5F60,
    data3: 0x7A8B,
    data4: [0x9C, 0x0D, 0x1E, 0x2F, 0x3A, 0x4B, 0x5C, 0x6D],
};

/// Simplified Chinese (zh-CN)
pub const LANGID_CHINESE_SIMPLIFIED: u16 = 0x0804;

/// Branding icon resource embedded in keytao_windows_ime.dll.
pub const PROFILE_ICON_RESOURCE_ID: u32 = 1;
pub const PROFILE_ICON_INDEX: u32 = (-1i32) as u32;

/// Display attribute used for active KeyTao composition text.
pub const GUID_DISPLAY_ATTRIBUTE_INPUT: GUID = GUID {
    data1: 0x9d1c2d2e,
    data2: 0x241c,
    data3: 0x4df3,
    data4: [0xb0, 0x63, 0x29, 0x2d, 0x4c, 0xe3, 0x7b, 0x91],
};

// ── DLL entry ─────────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "system" fn DllMain(hinstance: HMODULE, reason: u32, _: *mut ()) -> BOOL {
    const DLL_PROCESS_ATTACH: u32 = 1;
    if reason == DLL_PROCESS_ATTACH {
        let _ = DLL_INSTANCE.set(hinstance.0 as isize);
        unsafe {
            let _ = DisableThreadLibraryCalls(hinstance);
        }
    }
    BOOL::from(true)
}

// ── COM DLL exports ───────────────────────────────────────────────────────────

#[no_mangle]
/// # Safety
///
/// COM must pass valid GUID pointers and a writable output pointer.
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut std::ffi::c_void,
) -> HRESULT {
    unsafe {
        if rclsid.is_null() || riid.is_null() || ppv.is_null() {
            return E_POINTER;
        }
        *ppv = std::ptr::null_mut();
        let clsid = &*rclsid;
        if *clsid != CLSID_TEXT_SERVICE {
            return windows::Win32::Foundation::CLASS_E_CLASSNOTAVAILABLE;
        }
        let factory: IClassFactory = ClassFactory::new().into();
        factory.query(riid, ppv as *mut _)
    }
}

#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if can_unload() {
        S_OK
    } else {
        S_FALSE
    }
}

#[no_mangle]
pub extern "system" fn DllRegisterServer() -> HRESULT {
    match registration::register() {
        Ok(()) => S_OK,
        Err(error) => {
            let _ = registration::unregister();
            error.into()
        }
    }
}

#[no_mangle]
pub extern "system" fn DllUnregisterServer() -> HRESULT {
    registration::unregister()
        .map(|_| S_OK)
        .unwrap_or_else(|e| e.into())
}
