#![cfg(not(target_os = "windows"))]

#[path = "../../keytao-windows-ime/src/engine_status.rs"]
mod engine_status;
#[path = "../src/ime_status/windows.rs"]
mod windows;
