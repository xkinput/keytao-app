#[cfg(target_os = "linux")]
pub mod linux;
pub mod macos;
#[cfg(any(target_os = "windows", test))]
pub mod windows;

pub fn app_shared_data_dir(core: &crate::Core) -> String {
    #[cfg(target_os = "macos")]
    let shared = macos::macos_app_shared_data_dir(core);
    #[cfg(target_os = "linux")]
    let shared = linux::linux_app_shared_data_dir(core);
    #[cfg(target_os = "windows")]
    let shared = windows::windows_app_shared_data_dir(core);
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let shared: Option<String> = {
        let _ = core;
        None
    };
    shared.unwrap_or_else(keytao_core::default_shared_data_dir)
}
