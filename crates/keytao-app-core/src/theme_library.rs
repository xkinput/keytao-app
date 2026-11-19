//! Theme library operations use the same platform paths as appearance settings.
use crate::ime_host::ImeHost;
use keytao_theme::theme_store::{
    self, ThemeExport, ThemeImportPreview, ThemeImportResult, ThemeSummary,
};
use std::path::{Path, PathBuf};

fn editor_platform() -> keytao_theme::editor::EditorPlatform {
    use keytao_theme::editor::EditorPlatform;
    match std::env::consts::OS {
        "macos" => EditorPlatform::Macos,
        "windows" => EditorPlatform::Windows,
        "android" => EditorPlatform::Android,
        "ios" => EditorPlatform::Ios,
        #[cfg(target_os = "linux")]
        "linux" => linux_editor_platform(
            &crate::ime_status::linux::active_ime_backends(),
            std::env::var_os("KEYTAO_IME_FORCE_OVERLAY").is_some(),
            std::env::var_os("DISPLAY").is_some(),
        ),
        #[cfg(not(target_os = "linux"))]
        "linux" => EditorPlatform::Linux,
        _ => EditorPlatform::Unsupported,
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_editor_platform(
    backends: &[String],
    force_overlay: bool,
    has_display: bool,
) -> keytao_theme::editor::EditorPlatform {
    use keytao_theme::editor::EditorPlatform;
    if !backends.is_empty()
        && backends
            .iter()
            .flat_map(|record| record.split(','))
            .all(|backend| {
                backend == "ibus"
                    // shortcut: ready records omit the IBus panel-owner probe; treat automatic overlays as system panels until renderer state is published.
                    || (backend == "ibus-engine" && !(force_overlay && has_display))
            })
    {
        EditorPlatform::LinuxKimpanel
    } else {
        EditorPlatform::Linux
    }
}

fn theme_path(host: &impl ImeHost) -> Result<PathBuf, String> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    return Ok(host.user_root()?.join("theme.yaml"));
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = host;
        keytao_theme::default_user_theme_path()
            .ok_or("Cannot determine keytao data directory".into())
    }
}

fn notify_mobile(host: &impl ImeHost, path: &Path) -> Result<(), String> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    host.write_reload_stamp(path.parent().ok_or("Missing theme directory")?)?;
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let _ = (host, path);
    Ok(())
}

pub fn get_theme_library(host: impl ImeHost) -> Result<Vec<ThemeSummary>, String> {
    theme_store::list_themes(&theme_path(&host)?)
}

pub fn get_theme_editor(host: impl ImeHost) -> Result<String, String> {
    theme_store::get_theme_editor(&theme_path(&host)?, editor_platform())
}

pub fn update_theme_editor(
    host: impl ImeHost,
    target: &str,
    path: &str,
    value_json: Option<&str>,
) -> Result<String, String> {
    let value = value_json
        .map(serde_json::from_str)
        .transpose()
        .map_err(|error| format!("Invalid theme edit JSON: {error}"))?;
    let theme_path = theme_path(&host)?;
    let snapshot =
        theme_store::update_theme_editor(&theme_path, editor_platform(), target, path, value)?;
    notify_mobile(&host, &theme_path)?;
    Ok(snapshot)
}

pub fn set_theme_editor_color_scheme(
    host: impl ImeHost,
    color_scheme: Option<keytao_theme::UiColorScheme>,
) -> Result<String, String> {
    let theme_path = theme_path(&host)?;
    let snapshot =
        theme_store::set_theme_editor_color_scheme(&theme_path, editor_platform(), color_scheme)?;
    notify_mobile(&host, &theme_path)?;
    Ok(snapshot)
}

pub fn apply_theme(host: impl ImeHost, id: &str) -> Result<(), String> {
    let path = theme_path(&host)?;
    theme_store::apply_theme(&path, id)?;
    notify_mobile(&host, &path)
}

pub fn duplicate_theme(host: impl ImeHost, id: &str) -> Result<String, String> {
    theme_store::duplicate_theme(&theme_path(&host)?, id)
}

pub fn delete_theme(host: impl ImeHost, id: &str) -> Result<(), String> {
    let path = theme_path(&host)?;
    theme_store::delete_theme(&path, id)?;
    notify_mobile(&host, &path)
}

pub fn export_theme(host: impl ImeHost, id: &str) -> Result<ThemeExport, String> {
    theme_store::export_theme(&theme_path(&host)?, id)
}

pub fn preview_theme_import(host: impl ImeHost, text: &str) -> Result<ThemeImportPreview, String> {
    theme_store::preview_theme_import(&theme_path(&host)?, text)
}

pub fn import_theme(
    host: impl ImeHost,
    text: &str,
    overwrite: bool,
) -> Result<ThemeImportResult, String> {
    let path = theme_path(&host)?;
    let result = theme_store::import_theme(&path, text, overwrite)?;
    if result.disposition == "overwritten" {
        notify_mobile(&host, &path)?;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use keytao_theme::editor::EditorPlatform;

    #[test]
    fn linux_capabilities_disable_only_verified_system_panel_backends() {
        for records in [
            vec!["ibus"],
            vec!["ibus", "ibus"],
            vec!["ibus-engine"],
            vec!["ibus", "ibus-engine"],
        ] {
            let records: Vec<String> = records.into_iter().map(str::to_owned).collect();
            assert_eq!(
                linux_editor_platform(&records, false, true),
                EditorPlatform::LinuxKimpanel
            );
        }
        for records in [
            vec![],
            vec!["xim,ibus"],
            vec!["wayland", "ibus"],
            vec!["unknown"],
            vec![""],
            vec!["ibus,"],
        ] {
            let records: Vec<String> = records.into_iter().map(str::to_owned).collect();
            assert_eq!(
                linux_editor_platform(&records, false, true),
                EditorPlatform::Linux
            );
        }
        let engine = vec!["ibus-engine".to_owned()];
        assert_eq!(
            linux_editor_platform(&engine, true, true),
            EditorPlatform::Linux
        );
        assert_eq!(
            linux_editor_platform(&engine, true, false),
            EditorPlatform::LinuxKimpanel
        );
    }
}
