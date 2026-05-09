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

mod candidate_win;
mod globals;
mod key_event_sink;
mod key_map;
mod panel;
mod registration;
mod state;
mod text_service;

use windows::{
    core::{IUnknown, Result, GUID, HRESULT},
    Win32::{
        Foundation::{BOOL, HMODULE, S_FALSE, S_OK},
        System::Com::IClassFactory,
        System::LibraryLoader::DisableThreadLibraryCalls,
        UI::TextServices::*,
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

// ── DLL entry ─────────────────────────────────────────────────────────────────

#[no_mangle]
extern "system" fn DllMain(hinstance: HMODULE, reason: u32, _: *mut ()) -> BOOL {
    const DLL_PROCESS_ATTACH: u32 = 1;
    if reason == DLL_PROCESS_ATTACH {
        let _ = DLL_INSTANCE.set(hinstance);
        unsafe {
            let _ = DisableThreadLibraryCalls(hinstance);
        }
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
            .init();
    }
    BOOL::from(true)
}

// ── COM DLL exports ───────────────────────────────────────────────────────────

#[no_mangle]
extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut std::ffi::c_void,
) -> HRESULT {
    unsafe {
        let clsid = &*rclsid;
        if *clsid != CLSID_TEXT_SERVICE {
            return windows::Win32::Foundation::CLASS_E_CLASSNOTAVAILABLE;
        }
        let factory: IClassFactory = ClassFactory.into();
        factory.QueryInterface(riid, ppv as *mut _)
    }
}

#[no_mangle]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    if can_unload() {
        S_OK
    } else {
        S_FALSE
    }
}

#[no_mangle]
extern "system" fn DllRegisterServer() -> HRESULT {
    registration::register()
        .map(|_| S_OK)
        .unwrap_or_else(|e| e.into())
}

#[no_mangle]
extern "system" fn DllUnregisterServer() -> HRESULT {
    registration::unregister()
        .map(|_| S_OK)
        .unwrap_or_else(|e| e.into())
}
