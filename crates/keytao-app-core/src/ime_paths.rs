use std::path::{Path, PathBuf};
pub fn path_string(path: PathBuf) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn non_empty_string(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn default_user_data_dir_string() -> Option<String> {
    keytao_core::default_user_data_dir().map(path_string)
}
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn reload_stamp_path() -> Option<PathBuf> {
    keytao_core::ReloadStamp::default_path()
}
#[cfg(target_os = "ios")]
pub fn ios_reload_stamp_path(root: &Path) -> PathBuf {
    keytao_core::ReloadStamp::path(root)
}

#[cfg(target_os = "ios")]
pub fn write_ios_reload_stamp(root: &Path) -> Result<PathBuf, String> {
    keytao_core::ReloadStamp::write(root).map_err(|e| format!("写入 iOS 输入法重载标记失败: {e}"))
}

/// Same signature the input methods compare, so what the app reports and what a
/// frontend acts on can never drift apart.
pub fn file_signature(path: &Path) -> String {
    keytao_core::ReloadStamp::signature_at(path).unwrap_or_else(|| "missing".into())
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn reload_stamp_status() -> (Option<String>, Option<String>) {
    let Some(path) = reload_stamp_path() else {
        return (None, None);
    };
    let signature = file_signature(&path);
    (Some(path_string(path)), Some(signature))
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn shared_data_status(packaged: Option<String>) -> (Option<String>, String) {
    if let Some(path) = packaged {
        (Some(path), "packaged".into())
    } else {
        (
            non_empty_string(keytao_core::default_shared_data_dir()),
            "default_fallback".into(),
        )
    }
}
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub fn write_keytao_ime_reload_stamp() -> Result<(), String> {
    keytao_core::ReloadStamp::write_default()
        .map(|_| ())
        .map_err(|e| format!("写入 keytao-ime 重载标记失败: {e}"))
}
#[cfg(target_os = "android")]
pub fn android_reload_stamp_path(root: &Path) -> PathBuf {
    keytao_core::ReloadStamp::path(root)
}

#[cfg(target_os = "android")]
pub fn write_android_reload_stamp(root: &Path) -> Result<PathBuf, String> {
    keytao_core::ReloadStamp::write(root)
        .map_err(|e| format!("写入 Android 输入法重载标记失败: {e}"))
}
