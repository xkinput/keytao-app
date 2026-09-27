//! Shell-owned state and mobile plugin calls used by the IME backend.
use std::path::{Path, PathBuf};

pub trait ImeHost: Send + Sync + 'static {
    fn user_root(&self) -> Result<PathBuf, String>;
    fn write_reload_stamp(&self, root: &Path) -> Result<Option<PathBuf>, String>;
    #[cfg(target_os = "windows")]
    fn publish_english_settings(&self, enabled: bool) -> Result<(), String>;
    #[cfg(target_os = "android")]
    fn deploy_ime_data(&self) -> Result<serde_json::Value, String>;
    #[cfg(target_os = "linux")]
    fn managed_helper(&self) -> &crate::ime_status::linux::ManagedImeHelper;
    #[cfg(target_os = "linux")]
    fn suspend_for_deploy(&self, user: String, shared: String) -> keytao_core::ImeRuntime;
    #[cfg(target_os = "linux")]
    fn resume_after_deploy(&self) -> Result<(), String>;
}
