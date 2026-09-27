use keytao_app_core::{AppEnv, Core, CoreEvent, EventSink, Platform};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

pub(crate) struct TauriEventSink(pub AppHandle);

impl EventSink for TauriEventSink {
    fn emit(&self, event: CoreEvent) {
        let name = event.tauri_name();
        let result = match event {
            CoreEvent::InstallProgress(payload) => self.0.emit(name, payload),
            CoreEvent::DeployProgress(payload) => self.0.emit(name, payload),
            CoreEvent::WindowsImeStatus(payload) => self.0.emit(name, payload),
            CoreEvent::WindowsImeAction(payload) => self.0.emit(name, payload),
        };
        if let Err(error) = result {
            tracing::warn!("Failed to emit {name}: {error}");
        }
    }
}

pub(crate) fn manage_core(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "windows")]
    let platform = Platform::Windows;
    #[cfg(target_os = "macos")]
    let platform = Platform::MacOs;
    #[cfg(target_os = "linux")]
    let platform = Platform::Linux;
    #[cfg(target_os = "android")]
    let platform = Platform::Android;
    #[cfg(target_os = "ios")]
    let platform = Platform::Ios;

    let env = AppEnv {
        // Credentials belong in private local storage, never roaming or shared IME data.
        data_dir: app.path().app_local_data_dir()?,
        cache_dir: app.path().app_cache_dir()?,
        resource_dir: app.path().resource_dir()?,
        app_version: app.package_info().version.to_string(),
        platform,
    };
    let core = Core::new(env, Arc::new(TauriEventSink(app.clone())))?;
    #[cfg(not(target_os = "android"))]
    if let Err(error) = core.initialize_onboarding(super::default_keytao_user_root(app).ok()) {
        tracing::warn!(%error, "Failed to initialize onboarding");
    }
    if let Some(path) = core.recovered_state_file() {
        tracing::warn!(
            quarantine_path = %path.display(),
            "Recovered corrupt app state; using default state"
        );
    }
    app.manage(core);
    Ok(())
}
