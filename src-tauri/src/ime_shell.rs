use super::*;
use keytao_app_core::ime_host::ImeHost;

pub(super) struct ShellImeHost<R: tauri::Runtime = tauri::Wry>(pub AppHandle<R>);

impl<R: tauri::Runtime> ImeHost for ShellImeHost<R> {
    fn user_root(&self) -> Result<PathBuf, String> {
        default_keytao_user_root(&self.0)
    }

    fn write_reload_stamp(&self, root: &Path) -> Result<Option<PathBuf>, String> {
        write_default_reload_stamp(&self.0, root)
    }

    #[cfg(target_os = "windows")]
    fn publish_english_settings(&self, enabled: bool) -> Result<(), String> {
        windows_public_schemas::publish_english_settings(enabled)
    }

    #[cfg(target_os = "android")]
    fn deploy_ime_data(&self) -> Result<serde_json::Value, String> {
        self.0
            .state::<ScopedStorageHandle<R>>()
            .0
            .run_mobile_plugin("deployImeData", ())
            .map_err(|e| e.to_string())
    }

    #[cfg(target_os = "linux")]
    fn managed_helper(&self) -> &ManagedImeHelper {
        self.0.state::<ManagedImeHelper>().inner()
    }

    #[cfg(target_os = "linux")]
    fn suspend_for_deploy(&self, user: String, shared: String) -> keytao_core::ImeRuntime {
        let state: tauri::State<rime::RimeEngine> = self.0.state();
        state.suspend_for_deploy(user, shared)
    }

    #[cfg(target_os = "linux")]
    fn resume_after_deploy(&self) -> Result<(), String> {
        let state: tauri::State<rime::RimeEngine> = self.0.state();
        state.resume()
    }
}

#[cfg(target_os = "linux")]
pub(super) fn launch_keytao_ime(app: &AppHandle, restart: bool) -> Result<LinuxImeStatus, String> {
    keytao_app_core::ime_status::linux::launch_keytao_ime(
        &app.state::<Arc<Core>>(),
        &ShellImeHost(app.clone()),
        restart,
    )
}

#[cfg(target_os = "linux")]
pub(super) fn stop_managed_ime_helper(app: &AppHandle) {
    keytao_app_core::ime_status::linux::stop_managed_ime_helper(&ShellImeHost(app.clone()))
}
