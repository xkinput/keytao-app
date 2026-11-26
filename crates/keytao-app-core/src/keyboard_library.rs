//! Mobile keyboard profile library operations backed by `keytao-theme::keyboard_store`.

use crate::ime_host::ImeHost;
use keytao_theme::keyboard_store::{
    self, KeyboardExport, KeyboardImportPreview, KeyboardImportResult, KeyboardSummary,
};
use std::path::{Path, PathBuf};

fn keyboard_path(host: &impl ImeHost) -> Result<PathBuf, String> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    return Ok(host.user_root()?.join("keyboard.yaml"));
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = host;
        let theme_path = keytao_theme::default_user_theme_path()
            .ok_or("Cannot determine keytao data directory")?;
        Ok(theme_path
            .parent()
            .ok_or("Cannot determine keytao data directory")?
            .join("keyboard.yaml"))
    }
}

fn notify_mobile(host: &impl ImeHost, path: &Path) -> Result<(), String> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    host.write_reload_stamp(path.parent().ok_or("Missing keyboard directory")?)?;
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let _ = (host, path);
    Ok(())
}

pub fn get_keyboard_library(host: impl ImeHost) -> Result<Vec<KeyboardSummary>, String> {
    keyboard_store::list_keyboards(&keyboard_path(&host)?)
}

pub fn apply_keyboard(host: impl ImeHost, id: &str) -> Result<KeyboardSummary, String> {
    let path = keyboard_path(&host)?;
    let summary = keyboard_store::apply_keyboard(&path, id)?;
    notify_mobile(&host, &path)?;
    Ok(summary)
}

pub fn delete_keyboard(host: impl ImeHost, id: &str) -> Result<(), String> {
    let path = keyboard_path(&host)?;
    keyboard_store::delete_keyboard(&path, id)?;
    notify_mobile(&host, &path)
}

pub fn export_keyboard(host: impl ImeHost, id: &str) -> Result<KeyboardExport, String> {
    keyboard_store::export_keyboard(&keyboard_path(&host)?, id)
}

pub fn preview_keyboard_import(
    host: impl ImeHost,
    text: &str,
) -> Result<KeyboardImportPreview, String> {
    keyboard_store::preview_keyboard_import(&keyboard_path(&host)?, text)
}

pub fn import_keyboard(
    host: impl ImeHost,
    text: &str,
    overwrite: bool,
) -> Result<KeyboardImportResult, String> {
    let path = keyboard_path(&host)?;
    let result = keyboard_store::import_keyboard(&path, text, overwrite)?;
    if result.disposition == "overwritten" {
        notify_mobile(&host, &path)?;
    }
    Ok(result)
}

pub fn recommended_keyboard_for_schema(
    host: impl ImeHost,
    schema_id: &str,
) -> Result<Option<KeyboardSummary>, String> {
    keyboard_store::recommended_keyboard_for_schema(&keyboard_path(&host)?, schema_id)
}
