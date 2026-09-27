//! Shell callbacks for scheme operations; deployment delegates to the core IME backend.
//! Core controls their ordering, including Wanxiang deployment rollback.
use std::{
    future::Future,
    path::{Path, PathBuf},
};

pub trait SchemeHost: Sync {
    fn user_root(&self) -> Result<PathBuf, String>;
    fn deploy(&self) -> impl Future<Output = Result<(), String>> + Send;
    fn write_reload_stamp(&self, root: &Path) -> Result<Option<PathBuf>, String>;
    #[cfg(target_os = "android")]
    fn copy_addon_assets(&self, id: &str) -> Result<(), String>;
    #[cfg(target_os = "windows")]
    fn publish_english_addon(&self) -> Result<(), String>;
    #[cfg(target_os = "windows")]
    fn remove_public_schema(&self, id: &str, files: &[PathBuf]) -> Result<(), String>;
    #[cfg(target_os = "windows")]
    fn publish_english_settings(&self, enabled: bool) -> Result<(), String>;
    #[cfg(target_os = "windows")]
    fn publish_wanxiang_sources(&self, files: &[(PathBuf, PathBuf)]) -> Result<(), String>;
}
