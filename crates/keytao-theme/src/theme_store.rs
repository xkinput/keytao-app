//! Source-file storage and lossless migration for App and later bridge callers.
//! All paths are explicit: these APIs never select or access a real IME directory.

use crate::{
    share::{
        canonical_source_json, decode_theme_share, encode_theme_share, export_theme_source_json,
        resolve_import_conflict, ThemeImportDisposition, ThemeImportResolution,
        THEME_SHARE_FILE_THRESHOLD,
    },
    source::{
        compile_theme, parse_local_theme_source, parse_theme_source_yaml, resolve_theme_source,
        resolved_to_flat, ThemeSource,
    },
    EffectiveColorScheme, UiColorScheme,
};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};
use unicode_segmentation::UnicodeSegmentation;

/// The first v2 migration always uses this editable source identity.
pub const CURRENT_THEME_ID: &str = "my.current";
pub const DEFAULT_THEME_ID: &str = "keytao.default";

static STORE_LOCK: Mutex<()> = Mutex::new(());

fn lock_store() -> MutexGuard<'static, ()> {
    STORE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemePreviewColors {
    pub background: String,
    pub foreground: String,
    pub accent: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeSummary {
    pub id: String,
    pub name: String,
    pub built_in: bool,
    pub current: bool,
    pub light: ThemePreviewColors,
    pub dark: ThemePreviewColors,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeExport {
    pub id: String,
    pub file_name: String,
    pub json: String,
    pub share_string: String,
    pub use_file: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeImportResult {
    pub id: String,
    pub name: String,
    pub disposition: String,
    pub renamed_from: Option<String>,
    pub dropped: Vec<String>,
    pub clamped: Vec<String>,
    pub unknown: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeImportPreview {
    pub id: String,
    pub name: String,
    pub can_overwrite: bool,
    pub conflict: bool,
}

fn built_in_theme() -> ThemeSource {
    parse_source_value(
        json!({"version":3,"meta":{"id":DEFAULT_THEME_ID,"name":"默认","appearance":"both"}}),
    )
    .expect("built-in theme source is valid")
}

fn is_built_in(id: &str) -> bool {
    id.starts_with("keytao.")
}

fn check_store_path(theme_path: &Path, path: &Path) -> Result<(), String> {
    for ancestor in path
        .ancestors()
        .take_while(|ancestor| Some(*ancestor) != theme_path.parent())
    {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "Theme paths cannot contain symlinks: {}",
                    ancestor.display()
                ));
            }
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => {
                return Err(format!(
                    "Inspect theme path {}: {error}",
                    ancestor.display()
                ))
            }
        }
    }
    Ok(())
}

fn read_source(theme_path: &Path, id: &str) -> Result<ThemeSource, String> {
    crate::source::validate_theme_id(id)?;
    if id == DEFAULT_THEME_ID {
        return Ok(built_in_theme());
    }
    if is_built_in(id) {
        return Err("Unknown built-in theme".into());
    }
    let path = source_path(theme_path, id);
    check_store_path(theme_path, &path)?;
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("Read theme source {}: {error}", path.display()))?;
    if metadata.len() > crate::source::MAX_THEME_SOURCE_BYTES as u64 {
        return Err("Theme source exceeds 64 KiB".into());
    }
    let source =
        parse_theme_source_yaml(&fs::read_to_string(&path).map_err(|error| error.to_string())?)?;
    if source.id() != id {
        return Err(format!(
            "Theme id does not match filename: {}",
            path.display()
        ));
    }
    Ok(source)
}

fn installed_sources(theme_path: &Path) -> Result<Vec<ThemeSource>, String> {
    let directory = source_path(theme_path, DEFAULT_THEME_ID)
        .parent()
        .unwrap()
        .to_owned();
    check_store_path(theme_path, &directory)?;
    let mut sources = vec![built_in_theme()];
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(sources),
        Err(error) => return Err(format!("List themes {}: {error}", directory.display())),
    };
    let mut ids = Vec::new();
    for entry in entries {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("yaml") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if !is_built_in(id) {
            ids.push(id.to_owned());
        }
    }
    ids.sort();
    for id in ids {
        if let Ok(source) = read_source(theme_path, &id) {
            sources.push(source);
        }
    }
    Ok(sources)
}

fn preview_colors(source: &ThemeSource, scheme: EffectiveColorScheme) -> ThemePreviewColors {
    let resolved = resolve_theme_source(source, scheme);
    let hex = |color: crate::RgbaColor| {
        format!(
            "#{:02X}{:02X}{:02X}{:02X}",
            color.red, color.green, color.blue, color.alpha
        )
    };
    ThemePreviewColors {
        background: hex(resolved.panel.background),
        foreground: hex(resolved.candidate.foreground),
        accent: hex(resolved
            .ui
            .accent_color
            .unwrap_or(resolved.candidate.selected_border_color)),
    }
}

/// List the embedded default and validated installed source files.
pub fn list_themes(theme_path: &Path) -> Result<Vec<ThemeSummary>, String> {
    let _lock = lock_store();
    check_store_path(theme_path, theme_path)?;
    let base = read_base(theme_path)?;
    let current = base
        .get("themeId")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_THEME_ID);
    installed_sources(theme_path)?
        .into_iter()
        .map(|source| {
            Ok(ThemeSummary {
                id: source.id().into(),
                name: source.name().into(),
                built_in: is_built_in(source.id()),
                current: source.id() == current,
                light: preview_colors(&source, EffectiveColorScheme::Light),
                dark: preview_colors(&source, EffectiveColorScheme::Dark),
            })
        })
        .collect()
}

fn write_compiled(theme_path: &Path, source: &ThemeSource, base: &Value) -> Result<(), String> {
    check_store_path(theme_path, theme_path)?;
    let compiled = compile_theme(source, &value_yaml(base)?)?;
    let compiled = serde_yaml::from_str(&compiled).map_err(|error| error.to_string())?;
    crate::write_theme_yaml_atomic(theme_path, &compiled)
}

/// Apply an existing source, preserving local UI settings and manual edits.
pub fn apply_theme(theme_path: &Path, id: &str) -> Result<(), String> {
    let _lock = lock_store();
    check_store_path(theme_path, theme_path)?;
    let source = read_source(theme_path, id)?;
    resolve_manual_theme_edits_unlocked(theme_path, ManualThemeEditChoice::SaveAsNew)?;
    write_compiled(theme_path, &source, &read_base(theme_path)?)
}

fn save_library_source(theme_path: &Path, source: &ThemeSource) -> Result<(), String> {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let content = canonical_source_json(source)?;
    let path = source_path(theme_path, source.id());
    check_store_path(theme_path, &path)?;
    fs::create_dir_all(path.parent().unwrap()).map_err(|error| error.to_string())?;
    let temporary = path.with_file_name(format!(
        ".{}.{}.{}.tmp",
        source.id(),
        std::process::id(),
        SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| {
            format!(
                "Create theme temporary file {}: {error}",
                temporary.display()
            )
        })?;
    // JSON is valid YAML and keeps every accepted canonical source within 64 KiB.
    let result = (|| {
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, &path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|error| format!("Write theme source {}: {error}", path.display()))
}

/// Duplicate into the editable namespace without changing the active theme.
pub fn duplicate_theme(theme_path: &Path, id: &str) -> Result<String, String> {
    let _lock = lock_store();
    let mut source = read_source(theme_path, id)?;
    let base = format!(
        "my.{}",
        id.trim_start_matches("keytao.").trim_start_matches("my.")
    );
    let next = unused_copy_id(theme_path, &base)?;
    source.value["meta"]["id"] = json!(next);
    source.value["meta"]["name"] = json!(format!(
        "{}（副本）",
        source.name().graphemes(true).take(60).collect::<String>()
    ));
    let source =
        crate::source::parse_theme_source_json(&crate::share::canonical_json(&source.value)?)?;
    save_library_source(theme_path, &source)?;
    Ok(source.id().into())
}

/// Deleting the active installed theme safely selects the embedded default.
pub fn delete_theme(theme_path: &Path, id: &str) -> Result<(), String> {
    let _lock = lock_store();
    crate::source::validate_theme_id(id)?;
    if is_built_in(id) {
        return Err("Built-in themes cannot be deleted".into());
    }
    let path = source_path(theme_path, id);
    check_store_path(theme_path, &path)?;
    if !fs::metadata(&path)
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("Theme source is not a file".into());
    }
    check_store_path(theme_path, theme_path)?;
    let base = read_base(theme_path)?;
    if base.get("themeId").and_then(Value::as_str) == Some(id) {
        if read_source(theme_path, id).is_ok() {
            resolve_manual_theme_edits_unlocked(theme_path, ManualThemeEditChoice::SaveAsNew)?;
        } else {
            backup_manual_theme(theme_path)?;
        }
        write_compiled(theme_path, &built_in_theme(), &read_base(theme_path)?)?;
    }
    fs::remove_file(path).map_err(|error| error.to_string())
}

pub fn export_theme(theme_path: &Path, id: &str) -> Result<ThemeExport, String> {
    let _lock = lock_store();
    let source = read_source(theme_path, id)?;
    let json = export_theme_source_json(&source)?;
    let share_string = encode_theme_share(&source)?;
    Ok(ThemeExport {
        id: id.into(),
        file_name: format!("{id}.keytao-theme.json"),
        json,
        use_file: share_string.len() > THEME_SHARE_FILE_THRESHOLD,
        share_string,
    })
}

fn resolve_store_import_conflict(
    theme_path: &Path,
    incoming: &ThemeSource,
    overwrite: bool,
) -> Result<ThemeImportResolution, String> {
    let mut resolution =
        resolve_import_conflict(incoming, &installed_sources(theme_path)?, overwrite)?;
    if matches!(
        resolution.disposition,
        ThemeImportDisposition::Install | ThemeImportDisposition::SaveAsCopy
    ) {
        let source = resolution.source.as_mut().unwrap();
        let path = source_path(theme_path, source.id());
        check_store_path(theme_path, &path)?;
        // Invalid source files still reserve their filenames until explicitly overwritten.
        if path.try_exists().map_err(|error| error.to_string())? {
            if overwrite && resolution.disposition == ThemeImportDisposition::Install {
                resolution.disposition = ThemeImportDisposition::Overwrite;
            } else {
                let base = if is_built_in(incoming.id()) {
                    format!("my.{}", incoming.id().trim_start_matches("keytao."))
                } else {
                    incoming.id().to_owned()
                };
                source.value["meta"]["id"] = json!(unused_copy_id(theme_path, &base)?);
                if resolution.disposition == ThemeImportDisposition::Install {
                    source.value["meta"]["name"] = json!(format!(
                        "{}（副本）",
                        source.name().graphemes(true).take(60).collect::<String>()
                    ));
                }
                *source = crate::source::parse_theme_source_json(&crate::share::canonical_json(
                    &source.value,
                )?)?;
                resolution.disposition = ThemeImportDisposition::SaveAsCopy;
            }
        }
    }
    Ok(resolution)
}

pub fn preview_theme_import(theme_path: &Path, text: &str) -> Result<ThemeImportPreview, String> {
    let _lock = lock_store();
    let incoming = decode_theme_share(text)?;
    let resolution = resolve_store_import_conflict(theme_path, &incoming, false)?;
    let conflict = resolution.disposition == ThemeImportDisposition::SaveAsCopy;
    Ok(ThemeImportPreview {
        id: incoming.id().into(),
        name: incoming.name().into(),
        can_overwrite: conflict && !is_built_in(incoming.id()),
        conflict,
    })
}

/// Import portable text with the shared conflict rules and original diagnostics.
pub fn import_theme(
    theme_path: &Path,
    text: &str,
    overwrite: bool,
) -> Result<ThemeImportResult, String> {
    let _lock = lock_store();
    check_store_path(theme_path, theme_path)?;
    let incoming = decode_theme_share(text)?;
    let resolution = resolve_store_import_conflict(theme_path, &incoming, overwrite)?;
    let source = resolution.source.as_ref().unwrap_or(&incoming);
    let disposition = match resolution.disposition {
        ThemeImportDisposition::Install => "installed",
        ThemeImportDisposition::AlreadyInstalled => "alreadyInstalled",
        ThemeImportDisposition::SaveAsCopy => "copied",
        ThemeImportDisposition::Overwrite => "overwritten",
    };
    if resolution.source.is_some() {
        let base = read_base(theme_path)?;
        if base.get("themeId").and_then(Value::as_str) == Some(source.id()) {
            if read_source(theme_path, source.id()).is_ok() {
                resolve_manual_theme_edits_unlocked(theme_path, ManualThemeEditChoice::SaveAsNew)?;
            } else {
                backup_manual_theme(theme_path)?;
            }
            save_source_and_compile(theme_path, source, &read_base(theme_path)?)?;
        } else {
            save_library_source(theme_path, source)?;
        }
    }
    Ok(ThemeImportResult {
        id: source.id().into(),
        name: source.name().into(),
        disposition: disposition.into(),
        renamed_from: (source.id() != incoming.id()).then(|| incoming.id().into()),
        dropped: incoming.report.dropped,
        clamped: incoming.report.clamped,
        unknown: incoming.report.unknown,
        warnings: incoming.report.warnings,
    })
}

/// The App's conflict dialog defaults to preserving a manual edit as a new source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ManualThemeEditChoice {
    #[default]
    SaveAsNew,
    Discard,
}

/// Migrate a local v2 document and pin every value changed by the new resolver.
/// Local `ui` behavior settings remain in the compilation base, never in source.
pub fn migrate_v2_theme(yaml: &str, id: &str, name: &str) -> Result<ThemeSource, String> {
    let original = yaml_object(yaml)?;
    let yaml = value_yaml(&original)?;
    let mut value = json!({"version": 3, "meta": {"id": id, "name": name, "appearance": "both"}});
    map_legacy_layer(&original, &mut value);
    let compiled = original.get("themeHash").is_some();
    if compiled {
        value.as_object_mut().unwrap().remove("mobile");
        let light = original.pointer("/resolvedUi/light/effectiveColorScheme");
        let dark = original.pointer("/resolvedUi/dark/effectiveColorScheme");
        if light == dark && light.is_some_and(|v| matches!(v.as_str(), Some("light" | "dark"))) {
            value["meta"]["appearance"] = light.unwrap().clone();
        }
    }
    for (variant, alias) in [("light", "light"), ("dark", "night")] {
        if let Some(layer) = original.get(variant).or_else(|| original.get(alias)) {
            let mut mapped = json!({});
            map_legacy_layer(layer, &mut mapped);
            if compiled {
                mapped.as_object_mut().unwrap().remove("mobile");
            }
            value[variant] = mapped;
        }
        if compiled {
            if let Some(ui) = original.pointer(&format!("/resolvedUi/{variant}")) {
                let mut ui = ui.clone();
                crate::source::encode_colors(&mut ui);
                for key in ["accentColor", "cornerRadius", "backgroundColor"] {
                    if let Some(value_ui) = ui.get(key).filter(|v| !v.is_null()) {
                        value[variant]["common"][key] = value_ui.clone();
                    }
                }
            }
        }
    }
    let mut source = parse_source_value(value)?;
    for (variant, scheme) in [
        ("light", EffectiveColorScheme::Light),
        ("dark", EffectiveColorScheme::Dark),
    ] {
        let old = crate::resolve_compiled_theme_yaml(&yaml, scheme);
        let new = resolve_theme_source(&source, scheme);
        let old_flat = resolved_to_flat(&old);
        let new_flat = resolved_to_flat(&new);
        let mut mapped_old = json!({});
        let mut mapped_new = json!({});
        map_legacy_layer(&old_flat, &mut mapped_old);
        map_legacy_layer(&new_flat, &mut mapped_new);
        let differences = difference(&mapped_old, &mapped_new);
        let value = source
            .value
            .as_object_mut()
            .ok_or("Theme source must be an object")?;
        merge(
            value.entry(variant).or_insert_with(|| json!({})),
            &differences,
        );
        source = parse_source_value(source.value)?;
    }
    for (variant, scheme) in [
        ("light", EffectiveColorScheme::Light),
        ("dark", EffectiveColorScheme::Dark),
    ] {
        let old = crate::resolve_compiled_theme_yaml(&yaml, scheme);
        let new = resolve_theme_source(&source, scheme);
        let mut old_mobile = old.mobile;
        let mut new_mobile = new.mobile;
        crate::source::encode_colors(&mut old_mobile);
        crate::source::encode_colors(&mut new_mobile);
        let mobile = difference(&old_mobile, &new_mobile);
        merge(&mut source.value[variant]["mobile"], &mobile);
    }
    parse_source_value(source.value)
}

/// Return the current source's safe path, or `None` for an unmigrated v2 file.
pub fn current_theme_source_path(theme_path: &Path) -> Result<Option<PathBuf>, String> {
    check_store_path(theme_path, theme_path)?;
    let base = read_base(theme_path)?;
    source_path_from_base(theme_path, &base)
}

fn source_path_from_base(theme_path: &Path, base: &Value) -> Result<Option<PathBuf>, String> {
    let Some(id) = base.get("themeId").and_then(Value::as_str) else {
        return Ok(None);
    };
    // Validate before using a document-supplied id as a filename.
    let source =
        parse_source_value(json!({"version": 3, "meta": {"id": id, "name": "Current theme"}}))?;
    Ok(Some(source_path(theme_path, source.id())))
}

/// Read the active v3 source without performing a migration or any writes.
pub fn read_current_theme_source(theme_path: &Path) -> Result<Option<ThemeSource>, String> {
    let Some(path) = current_theme_source_path(theme_path)? else {
        return Ok(None);
    };
    read_source(
        theme_path,
        path.file_stem()
            .and_then(|id| id.to_str())
            .ok_or("Invalid theme filename")?,
    )
    .map(Some)
}

fn editor_snapshot_unlocked(
    theme_path: &Path,
    platform: crate::editor::EditorPlatform,
) -> Result<String, String> {
    check_store_path(theme_path, theme_path)?;
    let base = read_base(theme_path)?;
    let yaml = value_yaml(&base)?;
    let stored_source = read_current_theme_source(theme_path)?;
    let has_stored_source = stored_source.is_some();
    let mut source = match stored_source {
        Some(source) => source,
        None if base.as_object().is_some_and(Map::is_empty) => built_in_theme(),
        None => migrate_v2_theme(&yaml, CURRENT_THEME_ID, "Current theme")?,
    };
    if has_stored_source && compiled_theme_has_manual_edits(&source, &yaml)? {
        let id = unused_copy_id(theme_path, CURRENT_THEME_ID)?;
        source = preserve_manual_changes(source.clone(), &base, &id).unwrap_or(source);
    }
    let local = crate::resolve_compiled_theme_yaml(&yaml, EffectiveColorScheme::Light).ui;
    let resolved = |scheme| {
        let mut theme = resolve_theme_source(&source, scheme);
        theme.ui.color_scheme = local.color_scheme;
        theme.ui.embedded_composition = local.embedded_composition;
        theme.ui.hide_candidate_panel = local.hide_candidate_panel;
        theme.ui.show_tray_mode_indicator = local.show_tray_mode_indicator;
        theme
    };
    let source: Value = serde_json::from_str(&canonical_source_json(&source)?)
        .map_err(|error| error.to_string())?;
    serde_json::to_string(&json!({
        "source": source,
        "resolvedLight": resolved(EffectiveColorScheme::Light),
        "resolvedDark": resolved(EffectiveColorScheme::Dark),
        "colorScheme": local.color_scheme,
        "capabilities": crate::editor::editor_capabilities(platform),
    }))
    .map_err(|error| error.to_string())
}

/// Read the editor source and both resolved variants without mutating files.
pub fn get_theme_editor(
    theme_path: &Path,
    platform: crate::editor::EditorPlatform,
) -> Result<String, String> {
    let _lock = lock_store();
    editor_snapshot_unlocked(theme_path, platform)
}

/// Save one appearance-specific edit using the normal manual-edit and fork policy.
pub fn update_theme_editor(
    theme_path: &Path,
    platform: crate::editor::EditorPlatform,
    target: &str,
    path: &str,
    value: Option<Value>,
) -> Result<String, String> {
    // Validate before any migration, backup, or fork can write to the store.
    crate::editor::edit_theme_source_field(&built_in_theme(), target, path, value.clone())?;
    let _lock = lock_store();
    resolve_manual_theme_edits_unlocked(theme_path, ManualThemeEditChoice::SaveAsNew)?;
    let source = read_current_theme_source(theme_path)?.ok_or("Missing current theme source")?;
    let mut next = crate::editor::edit_theme_source_field(&source, target, path, value)?;
    let changed = canonical_source_json(&source)? != canonical_source_json(&next)?;
    if changed && is_built_in(source.id()) {
        let id = unused_copy_id(theme_path, &format!("my.{}", &source.id()[7..]))?;
        next.value["meta"]["id"] = json!(id);
        next = parse_source_value(next.value)?;
    }
    if changed {
        let base = read_base(theme_path)?;
        save_source_and_compile(theme_path, &next, &base)?;
    }
    editor_snapshot_unlocked(theme_path, platform)
}

/// Appearance selection is local; it never replays source styling values.
pub fn set_theme_editor_color_scheme(
    theme_path: &Path,
    platform: crate::editor::EditorPlatform,
    color_scheme: Option<UiColorScheme>,
) -> Result<String, String> {
    let _lock = lock_store();
    check_store_path(theme_path, theme_path)?;
    let (mut base, _) = read_yaml_for_update(theme_path)?;
    let root = base.as_mapping_mut().ok_or("Theme must be a mapping")?;
    let ui = root
        .entry(serde_yaml::Value::String("ui".into()))
        .or_insert_with(|| serde_yaml::Value::Mapping(serde_yaml::Mapping::new()))
        .as_mapping_mut()
        .ok_or("Theme UI must be a mapping")?;
    for alias in ["color_scheme", "nightMode", "night_mode"] {
        ui.remove(serde_yaml::Value::String(alias.into()));
    }
    if let Some(color_scheme) = color_scheme {
        ui.insert(
            serde_yaml::Value::String("colorScheme".into()),
            serde_yaml::to_value(color_scheme).map_err(|error| error.to_string())?,
        );
    } else {
        ui.remove(serde_yaml::Value::String("colorScheme".into()));
    }
    crate::write_theme_yaml_atomic(theme_path, &base)?;
    editor_snapshot_unlocked(theme_path, platform)
}

/// Perform or resume the one-time migration without replacing existing sources.
/// Rebuild from the compiled appearance when the active source was removed.
/// Also move a legacy keyboard block only when `keyboard.yaml` does not exist.
pub fn migrate_current_theme_once(theme_path: &Path) -> Result<bool, String> {
    let _lock = lock_store();
    check_store_path(theme_path, theme_path)?;
    let (base, _) = read_base_for_update(theme_path)?;
    migrate_theme_from_base(theme_path, &base)
}

fn migrate_theme_from_base(theme_path: &Path, base: &Value) -> Result<bool, String> {
    if base.get("themeId").and_then(Value::as_str) == Some(DEFAULT_THEME_ID) {
        return Ok(false);
    }
    if let Some(path) = source_path_from_base(theme_path, base)? {
        if path
            .try_exists()
            .map_err(|e| format!("Read theme source {}: {e}", path.display()))?
        {
            return Ok(false);
        }
    }
    let yaml = value_yaml(base)?;
    let path = source_path(theme_path, CURRENT_THEME_ID);
    let id = if path
        .try_exists()
        .map_err(|e| format!("Read theme source {}: {e}", path.display()))?
    {
        unused_copy_id(theme_path, CURRENT_THEME_ID)?
    } else {
        CURRENT_THEME_ID.into()
    };
    let source = migrate_v2_theme(&yaml, &id, "Current theme")?;
    save_source_and_compile(theme_path, &source, base)?;
    Ok(true)
}

/// Refresh an installed source at App startup without overwriting manual edits.
/// A conflicting compiled file is left intact for `resolve_manual_theme_edits`.
pub fn refresh_current_theme(theme_path: &Path) -> Result<bool, String> {
    let _lock = lock_store();
    check_store_path(theme_path, theme_path)?;
    if read_base_document(theme_path)?.1 {
        return Ok(false);
    }
    let (base, _) = read_base_for_update(theme_path)?;
    if migrate_theme_from_base(theme_path, &base)? {
        return Ok(true);
    }
    let source = read_current_theme_source(theme_path)?.ok_or("Missing current theme source")?;
    let base = read_base(theme_path)?;
    let yaml = value_yaml(&base)?;
    if compiled_theme_has_manual_edits(&source, &yaml)? {
        return Ok(false);
    }
    let compiled = compile_theme(&source, &yaml)?;
    let compiled: serde_yaml::Value = serde_yaml::from_str(&compiled).map_err(|e| e.to_string())?;
    if serde_yaml::to_value(&base).map_err(|e| e.to_string())? == compiled {
        return Ok(false);
    }
    crate::write_theme_yaml_atomic(theme_path, &compiled)?;
    Ok(true)
}

/// Detect manual changes to compiled appearance. An intact compiled hash lets
/// source edits and defaults upgrades recompile without a false conflict.
/// Local UI flags can be edited independently without causing a source conflict.
pub fn compiled_theme_has_manual_edits(
    source: &ThemeSource,
    compiled_yaml: &str,
) -> Result<bool, String> {
    let actual = yaml_object(compiled_yaml)?;
    if ["accentColor", "cornerRadius", "backgroundColor"]
        .iter()
        .any(|key| actual["ui"].get(*key).is_some())
    {
        return Ok(true);
    }
    if let Some(hash) = actual.get("compiledHash").and_then(Value::as_str) {
        return Ok(hash != crate::source::compiled_appearance_hash(&actual)?);
    }
    let expected = yaml_object(&compile_theme(source, compiled_yaml)?)?;
    if actual.get("themeHash") != expected.get("themeHash") {
        return Ok(true);
    }
    for key in [
        "font",
        "panel",
        "candidate",
        "navigation",
        "modeHint",
        "light",
        "dark",
        "mobile",
    ] {
        if actual.get(key) != expected.get(key) {
            return Ok(true);
        }
    }
    for variant in ["light", "dark"] {
        for key in [
            "accentColor",
            "cornerRadius",
            "backgroundColor",
            "effectiveColorScheme",
        ] {
            if actual["resolvedUi"][variant].get(key) != expected["resolvedUi"][variant].get(key) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Resolve a compiled-file conflict. `SaveAsNew` preserves the current v2 look;
/// `Discard` reapplies the existing source. Return the active source path.
/// Unrepresentable edits are backed up beside the compilation and logged before
/// reapplying the source. A failed backup leaves the original files untouched.
pub fn resolve_manual_theme_edits(
    theme_path: &Path,
    choice: ManualThemeEditChoice,
) -> Result<PathBuf, String> {
    let _lock = lock_store();
    resolve_manual_theme_edits_unlocked(theme_path, choice)
}

fn resolve_manual_theme_edits_unlocked(
    theme_path: &Path,
    choice: ManualThemeEditChoice,
) -> Result<PathBuf, String> {
    check_store_path(theme_path, theme_path)?;
    let (mut base, backup) = read_base_for_update(theme_path)?;
    if migrate_theme_from_base(theme_path, &base)? {
        base = read_base(theme_path)?;
    }
    let source = read_current_theme_source(theme_path)?.ok_or("Missing current theme source")?;
    let yaml = value_yaml(&base)?;
    if !compiled_theme_has_manual_edits(&source, &yaml)? {
        if backup.is_some() {
            save_source_and_compile(theme_path, &source, &base)?;
        }
        return Ok(source_path(theme_path, source.id()));
    }
    let next = match choice {
        ManualThemeEditChoice::Discard => source,
        ManualThemeEditChoice::SaveAsNew => {
            let id = unused_copy_id(theme_path, CURRENT_THEME_ID)?;
            match preserve_manual_changes(source.clone(), &base, &id) {
                Ok(copy) => copy,
                Err(error) => {
                    let backup = match &backup {
                        Some(path) => path.clone(),
                        None => backup_manual_theme(theme_path)?,
                    };
                    eprintln!(
                        "Warning: {error}; manual theme backed up to {}; continuing from the source",
                        backup.display()
                    );
                    source
                }
            }
        }
    };
    save_source_and_compile(theme_path, &next, &base)?;
    Ok(source_path(theme_path, next.id()))
}

fn backup_manual_theme(theme_path: &Path) -> Result<PathBuf, String> {
    let content = fs::read(theme_path).map_err(|e| e.to_string())?;
    let mut timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    loop {
        let mut name = theme_path
            .file_name()
            .ok_or("Missing theme filename")?
            .to_os_string();
        name.push(format!(".manual-{timestamp}.bak"));
        let backup = theme_path.with_file_name(name);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup)
        {
            Ok(mut file) => {
                file.write_all(&content)
                    .and_then(|()| file.sync_all())
                    .map_err(|e| format!("Back up manual theme {}: {e}", backup.display()))?;
                return Ok(backup);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                timestamp = timestamp
                    .checked_add(1)
                    .ok_or("Theme backup timestamp overflow")?;
            }
            Err(error) => {
                return Err(format!(
                    "Back up manual theme {}: {error}",
                    backup.display()
                ))
            }
        }
    }
}

fn preserve_manual_changes(
    mut source: ThemeSource,
    actual: &Value,
    id: &str,
) -> Result<ThemeSource, String> {
    if actual.get("ui").is_some_and(|v| !v.is_object()) {
        return Err("Local UI settings must be a mapping; original files were preserved".into());
    }
    let expected = yaml_object(&compile_theme(&source, &value_yaml(actual)?)?)?;
    for (variant, scheme) in [
        ("light", EffectiveColorScheme::Light),
        ("dark", EffectiveColorScheme::Dark),
    ] {
        let actual = compiled_variant_for_migration(actual, scheme)?;
        let expected = compiled_variant_for_migration(&expected, scheme)?;
        let changed = difference(&actual, &expected);
        let mut mapped = json!({});
        map_legacy_layer(&changed, &mut mapped);
        if let Some(selected) = mapped
            .pointer_mut("/mobile/keyboard/keyStates/selected")
            .and_then(Value::as_object_mut)
        {
            selected.remove("softBackground");
        }
        let mut partial = json!({"version":3,"meta":{"id":"my.validate","name":"Validate"}});
        partial[variant] = mapped;
        let partial = parse_source_value(partial)?;
        if let Some(patch) = partial.valid.get(variant) {
            merge(&mut source.value[variant], patch);
        }
    }
    source.value["meta"]["id"] = Value::String(id.into());
    source.value["meta"]["name"] = Value::String(format!("{} (copy)", source.name()));
    source = parse_source_value(source.value)?;
    // Compiled mobile colors can stay unchanged when a desktop knob is edited.
    for (variant, scheme) in [
        ("light", EffectiveColorScheme::Light),
        ("dark", EffectiveColorScheme::Dark),
    ] {
        let actual = compiled_variant_for_migration(actual, scheme)?;
        let mut expected = resolve_theme_source(&source, scheme).mobile;
        crate::source::encode_colors(&mut expected);
        let mobile = difference(&actual["mobile"], &expected);
        if mobile.as_object().is_some_and(|v| !v.is_empty()) {
            merge(&mut source.value[variant]["mobile"], &mobile);
        }
    }
    let source = parse_source_value(source.value)?;
    let mut candidate = yaml_object(&compile_theme(&source, &value_yaml(actual)?)?)?;
    let mut actual = actual.clone();
    for value in [&mut candidate, &mut actual] {
        value["ui"]["colorScheme"] = json!("auto");
    }
    let candidate = value_yaml(&candidate)?;
    let actual = value_yaml(&actual)?;
    for scheme in [EffectiveColorScheme::Light, EffectiveColorScheme::Dark] {
        let expected =
            crate::resolved_theme_json(&crate::resolve_compiled_theme_yaml(&actual, scheme))
                .map_err(|e| e.to_string())?;
        let compiled =
            crate::resolved_theme_json(&crate::resolve_compiled_theme_yaml(&candidate, scheme))
                .map_err(|e| e.to_string())?;
        if compiled != expected {
            return Err("Manual theme changes cannot be represented by a v3 source; original files were preserved".into());
        }
    }
    Ok(source)
}

fn compiled_variant_for_migration(
    root: &Value,
    scheme: EffectiveColorScheme,
) -> Result<Value, String> {
    let mut legacy = root.clone();
    legacy["ui"]["colorScheme"] = json!("auto");
    let theme = crate::resolve_compiled_theme_yaml(&value_yaml(&legacy)?, scheme);
    let mut flat = resolved_to_flat(&theme);
    flat["ui"] = serde_json::to_value(&theme.ui).map_err(|e| e.to_string())?;
    crate::source::encode_colors(&mut flat["ui"]);
    let mut mobile = theme.mobile;
    if let Some(base) = root.get("mobile") {
        merge(&mut mobile, base);
    }
    let variant = if scheme == EffectiveColorScheme::Dark {
        "dark"
    } else {
        "light"
    };
    if let Some(variant) = root.get(variant).or_else(|| {
        if variant == "dark" {
            root.get("night")
        } else {
            None
        }
    }) {
        if let Some(patch) = variant.get("mobile") {
            merge(&mut mobile, patch);
        }
    }
    crate::source::encode_colors(&mut mobile);
    flat["mobile"] = mobile;
    Ok(flat)
}

pub(crate) fn write_source_settings(
    theme_path: &Path,
    color_scheme: UiColorScheme,
    accent_color: Option<String>,
    corner_radius: Option<Option<f32>>,
    background_color: Option<String>,
    orientation: Option<crate::PanelOrientation>,
    font_size: Option<f32>,
) -> Result<(), String> {
    let _lock = lock_store();
    resolve_manual_theme_edits_unlocked(theme_path, ManualThemeEditChoice::SaveAsNew)?;
    let mut source =
        read_current_theme_source(theme_path)?.ok_or("Missing current theme source")?;
    let built_in = is_built_in(source.id()).then(|| source.clone());
    source = edit_source_settings(
        source,
        accent_color,
        corner_radius,
        background_color,
        orientation,
        font_size,
    )?;
    if let Some(original) = built_in {
        let changed = [EffectiveColorScheme::Light, EffectiveColorScheme::Dark]
            .into_iter()
            .any(|scheme| {
                let before = resolve_theme_source(&original, scheme);
                let after = resolve_theme_source(&source, scheme);
                resolved_to_flat(&before) != resolved_to_flat(&after)
                    || before.mobile != after.mobile
                    || before.ui.effective_color_scheme != after.ui.effective_color_scheme
            });
        if changed {
            let id = unused_copy_id(theme_path, &format!("my.{}", &source.id()[7..]))?;
            source.value["meta"]["id"] = json!(id);
            source = parse_source_value(source.value)?;
        } else {
            source = original;
        }
    }
    let mut base = read_base(theme_path)?;
    if !base["ui"].is_object() {
        base["ui"] = json!({});
    }
    base["ui"]["colorScheme"] = serde_json::to_value(color_scheme).map_err(|e| e.to_string())?;
    if is_built_in(source.id()) {
        write_compiled(theme_path, &source, &base)
    } else {
        save_source_and_compile(theme_path, &source, &base)
    }
}

/// Apply the existing App's global controls to a source without writing files.
/// Release matching derived pins while preserving distinct explicit overrides.
pub(crate) fn edit_source_settings(
    mut source: ThemeSource,
    accent_color: Option<String>,
    corner_radius: Option<Option<f32>>,
    background_color: Option<String>,
    orientation: Option<crate::PanelOrientation>,
    font_size: Option<f32>,
) -> Result<ThemeSource, String> {
    let accent = accent_color.map(|s| if s.is_empty() { Value::Null } else { s.into() });
    let radius = corner_radius.map(|r| r.map_or(Value::Null, Value::from));
    let background = background_color.map(|s| if s.is_empty() { Value::Null } else { s.into() });
    for (key, value) in [
        ("accentColor", accent),
        ("cornerRadius", radius),
        ("backgroundColor", background),
    ] {
        let Some(value) = value else { continue };
        let old = source.value["common"][key].clone();
        let mut normalized =
            json!({"version":3,"meta":{"id":"my.validate","name":"Validate"},"common":{}});
        if !value.is_null() {
            normalized["common"][key] = value.clone();
        }
        let normalized = parse_source_value(normalized)?;
        let new = &normalized.value["common"][key];
        if &old != new {
            release_matching_derivations(&mut source, key)?;
        }
        set_optional(&mut source.value, &["common", key], Some(value));
    }
    for (section, key, value) in [
        (
            "panel",
            "orientation",
            orientation.map(|v| serde_json::to_value(v).expect("orientation serializes")),
        ),
        ("font", "size", font_size.map(Value::from)),
    ] {
        if let Some(value) = value {
            for variant in ["light", "dark"] {
                remove_field(&mut source.value[variant], &["pc", section, key]);
            }
            source.value["pc"][section][key] = value;
        }
    }
    parse_source_value(source.value)
}

fn release_matching_derivations(source: &mut ThemeSource, knob: &str) -> Result<(), String> {
    let mut paths = Vec::new();
    if knob == "accentColor" {
        for field in [
            "selectedLabelColor",
            "selectedBorderColor",
            "selectedBackground",
            "pressedBackground",
            "hoverBackground",
        ] {
            paths.push(("candidate".to_string(), field.to_string()));
        }
        for field in ["borderColor", "foreground"] {
            paths.push(("modeHint".to_string(), field.to_string()));
        }
    } else if knob == "cornerRadius" {
        for section in ["panel", "candidate", "navigation", "modeHint"] {
            paths.push((section.to_string(), "cornerRadius".into()));
        }
    } else {
        let flat = resolved_to_flat(&resolve_theme_source(source, EffectiveColorScheme::Light));
        for section in ["panel", "candidate", "navigation", "modeHint"] {
            for (field, value) in flat[section].as_object().unwrap() {
                if value.as_str().is_some_and(|s| s.starts_with('#')) {
                    paths.push((section.into(), field.clone()));
                }
            }
        }
    }
    let mut derived = source.value.clone();
    for variant in [None, Some("light"), Some("dark")] {
        let layer = match variant {
            Some(v) => &mut derived[v],
            None => &mut derived,
        };
        for (section, field) in &paths {
            remove_field(layer, &["pc", section, field]);
        }
    }
    let derived = parse_source_value(derived)?;
    let light = resolved_to_flat(&resolve_theme_source(&derived, EffectiveColorScheme::Light));
    let dark = resolved_to_flat(&resolve_theme_source(&derived, EffectiveColorScheme::Dark));
    for variant in [None, Some("light"), Some("dark")] {
        let layer = match variant {
            Some(v) => &mut source.value[v],
            None => &mut source.value,
        };
        for (section, field) in &paths {
            let actual = &layer["pc"][section][field];
            let matches = |expected: &Value| {
                actual == expected
                    || (section == "panel"
                        && field == "background"
                        && actual
                            .as_str()
                            .zip(expected.as_str())
                            .is_some_and(|(a, b)| a.get(..7) == b.get(..7)))
            };
            let release = match variant {
                Some("dark") => matches(&dark[section][field]),
                Some(_) => matches(&light[section][field]),
                None => matches(&light[section][field]) || matches(&dark[section][field]),
            };
            if release {
                remove_field(layer, &["pc", section, field]);
            }
        }
    }
    Ok(())
}

fn remove_field(value: &mut Value, path: &[&str]) {
    let mut parent = value;
    for key in &path[..path.len() - 1] {
        let Some(child) = parent.get_mut(*key) else {
            return;
        };
        parent = child;
    }
    if let Some(parent) = parent.as_object_mut() {
        parent.remove(path[path.len() - 1]);
    }
}

fn source_path(theme_path: &Path, id: &str) -> PathBuf {
    theme_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("themes")
        .join(format!("{id}.yaml"))
}

fn unused_copy_id(theme_path: &Path, id: &str) -> Result<String, String> {
    for suffix in 2u64.. {
        let suffix = format!("-{suffix}");
        let root = &id[..id.len().min(64 - suffix.len())];
        let copy = format!("{}{suffix}", root.trim_end_matches(['.', '-', '_']));
        match fs::symlink_metadata(source_path(theme_path, &copy)) {
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(copy),
            Err(error) => return Err(format!("Inspect theme copy id: {error}")),
        }
    }
    unreachable!()
}

fn read_base(path: &Path) -> Result<Value, String> {
    read_base_document(path).map(|(base, _)| base)
}

fn read_base_document(path: &Path) -> Result<(Value, bool), String> {
    match fs::read_to_string(path) {
        Ok(yaml) => crate::source::parse_compilation_base(&yaml),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((json!({}), false)),
        Err(e) => Err(format!("Read theme {}: {e}", path.display())),
    }
}

fn yaml_object(yaml: &str) -> Result<Value, String> {
    crate::source::parse_compilation_base(yaml).map(|(base, _)| base)
}

/// Read a compilation base for an explicit setting write, backing up any
/// unreadable document or invalid mapping sections before they are replaced.
pub fn read_theme_for_update(theme_path: &Path) -> Result<serde_yaml::Value, String> {
    check_store_path(theme_path, theme_path)?;
    read_yaml_for_update(theme_path).map(|(base, _)| base)
}

fn read_base_for_update(theme_path: &Path) -> Result<(Value, Option<PathBuf>), String> {
    let (base, backup) = read_yaml_for_update(theme_path)?;
    let base = serde_json::to_value(base).map_err(|e| e.to_string())?;
    Ok((base, backup))
}

fn read_yaml_for_update(theme_path: &Path) -> Result<(serde_yaml::Value, Option<PathBuf>), String> {
    let yaml = match fs::read_to_string(theme_path) {
        Ok(yaml) => yaml,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((serde_yaml::Value::Mapping(serde_yaml::Mapping::new()), None));
        }
        Err(error) => return Err(format!("Read theme {}: {error}", theme_path.display())),
    };
    let (base, repaired) = match crate::source::parse_compilation_base_yaml(&yaml) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("Warning: {error}; replacing unreadable theme document");
            (serde_yaml::Value::Mapping(serde_yaml::Mapping::new()), true)
        }
    };
    let backup = if repaired {
        let backup = backup_manual_theme(theme_path)?;
        eprintln!("Warning: original theme backed up to {}", backup.display());
        Some(backup)
    } else {
        None
    };
    Ok((base, backup))
}

fn parse_source_value(value: Value) -> Result<ThemeSource, String> {
    parse_local_theme_source(value)
}

fn value_yaml(value: &Value) -> Result<String, String> {
    serde_yaml::to_string(value).map_err(|e| e.to_string())
}

fn save_source_and_compile(
    theme_path: &Path,
    source: &ThemeSource,
    base: &Value,
) -> Result<(), String> {
    let compiled = compile_theme(source, &value_yaml(base)?)?;
    check_store_path(theme_path, theme_path)?;
    save_library_source(theme_path, source)?;
    extract_legacy_keyboard(theme_path, base)?;
    let compiled_yaml = serde_yaml::from_str(&compiled).map_err(|e| e.to_string())?;
    crate::write_theme_yaml_atomic(theme_path, &compiled_yaml)
}

fn extract_legacy_keyboard(theme_path: &Path, base: &Value) -> Result<(), String> {
    let Some(keyboard) = base.get("keyboard") else {
        return Ok(());
    };
    let path = theme_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("keyboard.yaml");
    let content = value_yaml(keyboard)?;
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => file
            .write_all(content.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(format!("Write keyboard migration {}: {e}", path.display())),
    }
}

fn map_legacy_layer(old: &Value, source: &mut Value) {
    if old.get("ui").is_some() {
        let mut ui = serde_json::to_value(
            crate::resolve_legacy_theme_yaml(
                &value_yaml(old).expect("theme serializes"),
                EffectiveColorScheme::Light,
            )
            .ui,
        )
        .expect("resolved UI serializes");
        crate::source::encode_colors(&mut ui);
        for key in ["accentColor", "cornerRadius", "backgroundColor"] {
            if let Some(value) = ui.get(key).filter(|v| !v.is_null()) {
                source["common"][key] = value.clone();
            }
        }
    }
    if let Some(font) = old.get("font").and_then(Value::as_object) {
        for (key, value) in font {
            if matches!(key.as_str(), "family" | "weight") {
                source["common"]["font"][key] = value.clone();
            } else {
                source["pc"]["font"][key] = value.clone();
            }
        }
    }
    for key in ["panel", "candidate", "navigation", "modeHint"] {
        if let Some(value) = old.get(key).or_else(|| {
            if key == "modeHint" {
                old.get("mode_hint")
            } else {
                None
            }
        }) {
            source["pc"][key] = value.clone();
        }
    }
    if let Some(mobile) = old.get("mobile") {
        let mut mobile = mobile.clone();
        crate::source::encode_colors(&mut mobile);
        source["mobile"] = mobile;
    }
}

fn difference(old: &Value, new: &Value) -> Value {
    let mut result = Map::new();
    if let Some(old) = old.as_object() {
        for (key, value) in old {
            if value == &new[key] {
                continue;
            }
            let changed = if value.is_object() {
                difference(value, &new[key])
            } else {
                value.clone()
            };
            if changed.as_object().is_none_or(|v| !v.is_empty()) {
                result.insert(key.clone(), changed);
            }
        }
    }
    Value::Object(result)
}

fn merge(target: &mut Value, patch: &Value) {
    if let Some(patch) = patch.as_object() {
        if !target.is_object() {
            *target = json!({});
        }
        for (key, value) in patch {
            merge(&mut target[key], value);
        }
    } else {
        *target = patch.clone();
    }
}

fn set_optional(root: &mut Value, path: &[&str], value: Option<Value>) {
    let Some(value) = value else { return };
    let mut parent = root;
    for key in &path[..path.len() - 1] {
        if !parent.is_object() {
            *parent = json!({});
        }
        parent = &mut parent[*key];
    }
    if !parent.is_object() {
        *parent = json!({});
    }
    let key = path[path.len() - 1];
    if value.is_null() {
        parent.as_object_mut().unwrap().remove(key);
    } else {
        parent[key] = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "keytao-theme-store-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn theme(&self) -> PathBuf {
            self.0.join("theme.yaml")
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn editor_store_roundtrips_each_section_and_keeps_variants_separate() {
        let dir = TempDir::new();
        apply_theme(&dir.theme(), DEFAULT_THEME_ID).unwrap();
        let initial_count = installed_sources(&dir.theme()).unwrap().len();
        let platform = crate::editor::EditorPlatform::Macos;
        let fields = [
            ("common.accentColor", json!("#12345678")),
            ("pc.panel.paddingX", json!(13)),
            (
                "mobile.keyboard.keyTypes.enter.foreground",
                json!("#FEDCBA98"),
            ),
            ("mobile.candidateBar.showComment", json!(false)),
        ];
        for target in ["light", "dark"] {
            for (path, value) in &fields {
                let snapshot =
                    update_theme_editor(&dir.theme(), platform, target, path, Some(value.clone()))
                        .unwrap();
                let snapshot: Value = serde_json::from_str(&snapshot).unwrap();
                let id = snapshot["source"]["meta"]["id"].as_str().unwrap();
                assert_eq!(id, "my.default-2");
                let source = read_source(&dir.theme(), id).unwrap();
                let pointer = format!("/{target}/{}", path.replace('.', "/"));
                assert_eq!(
                    crate::share::canonical_json(source.value.pointer(&pointer).unwrap()).unwrap(),
                    crate::share::canonical_json(snapshot["source"].pointer(&pointer).unwrap()).unwrap()
                );
                let compiled = fs::read_to_string(dir.theme()).unwrap();
                assert!(!compiled_theme_has_manual_edits(&source, &compiled).unwrap());
                update_theme_editor(&dir.theme(), platform, target, path, None).unwrap();
                assert!(read_source(&dir.theme(), id)
                    .unwrap()
                    .value
                    .pointer(&pointer)
                    .is_none());
            }
        }
        let snapshot: Value =
            serde_json::from_str(&get_theme_editor(&dir.theme(), platform).unwrap()).unwrap();
        assert_eq!(snapshot["resolvedLight"]["mobile"]["isDark"], false);
        assert_eq!(snapshot["resolvedDark"]["mobile"]["isDark"], true);
        assert_eq!(installed_sources(&dir.theme()).unwrap().len(), initial_count + 1);
    }

    #[test]
    fn editor_local_appearance_preserves_source_and_hand_edited_styles() {
        let dir = TempDir::new();
        apply_theme(&dir.theme(), DEFAULT_THEME_ID).unwrap();
        let initial_count = installed_sources(&dir.theme()).unwrap().len();
        let platform = crate::editor::EditorPlatform::Macos;
        let before = fs::read_to_string(dir.theme()).unwrap();
        let mut manual = yaml_object(&before).unwrap();
        manual["panel"]["paddingX"] = json!(17);
        fs::write(dir.theme(), value_yaml(&manual).unwrap()).unwrap();
        let before_read = fs::read_to_string(dir.theme()).unwrap();
        let preview: Value =
            serde_json::from_str(&get_theme_editor(&dir.theme(), platform).unwrap()).unwrap();
        assert_eq!(preview["resolvedLight"]["panel"]["paddingX"], 17.0);
        assert_eq!(
            preview["resolvedDark"]["panel"]["paddingX"],
            crate::resolve_compiled_theme_yaml(&before_read, EffectiveColorScheme::Dark).panel.padding_x
        );
        assert_eq!(fs::read_to_string(dir.theme()).unwrap(), before_read);
        assert_eq!(installed_sources(&dir.theme()).unwrap().len(), initial_count);
        let snapshot =
            set_theme_editor_color_scheme(&dir.theme(), platform, Some(UiColorScheme::Dark)).unwrap();
        let snapshot: Value = serde_json::from_str(&snapshot).unwrap();
        assert_eq!(snapshot["colorScheme"], "dark");
        let snapshot = set_theme_editor_color_scheme(&dir.theme(), platform, None).unwrap();
        let snapshot: Value = serde_json::from_str(&snapshot).unwrap();
        assert_eq!(snapshot["colorScheme"], "auto");
        assert!(read_base(&dir.theme()).unwrap()["ui"].get("colorScheme").is_none());
        assert_eq!(snapshot["source"], preview["source"]);
        let after = read_base(&dir.theme()).unwrap();
        assert_eq!(after["themeId"], DEFAULT_THEME_ID);
        assert_eq!(after["panel"]["paddingX"], 17);
        assert_eq!(installed_sources(&dir.theme()).unwrap().len(), initial_count);
        assert!(
            compiled_theme_has_manual_edits(&built_in_theme(), &value_yaml(&after).unwrap())
                .unwrap()
        );
        let snapshot = update_theme_editor(
            &dir.theme(),
            platform,
            "dark",
            "pc.font.size",
            Some(json!(26)),
        )
        .unwrap();
        let snapshot: Value = serde_json::from_str(&snapshot).unwrap();
        assert_eq!(snapshot["source"]["meta"], preview["source"]["meta"]);
        assert_eq!(snapshot["source"]["light"], preview["source"]["light"]);
        assert_eq!(snapshot["resolvedLight"]["panel"]["paddingX"], 17.0);
        assert_eq!(snapshot["resolvedDark"]["font"]["size"], 26.0);
        assert_eq!(snapshot["colorScheme"], "auto");
    }

    #[test]
    fn editor_invalid_edits_have_no_filesystem_side_effects() {
        let dir = TempDir::new();
        let platform = crate::editor::EditorPlatform::Macos;
        for (target, path, value) in [
            ("base", "common.accentColor", Some(json!("#FFFFFF"))),
            ("light", "meta.id", Some(json!("../../escape"))),
            (
                "dark",
                "mobile.candidateBar.showComment",
                Some(json!("false")),
            ),
            (
                "light",
                "common.font",
                Some(json!({"family": "New", "weight": "unsupported"})),
            ),
            (
                "dark",
                "common.font",
                Some(json!({"family": 42, "weight": "regular"})),
            ),
        ] {
            assert!(update_theme_editor(&dir.theme(), platform, target, path, value).is_err());
            assert!(!dir.theme().exists());
            assert!(!dir.0.join("themes").exists());
        }
        let snapshot = get_theme_editor(&dir.theme(), platform).unwrap();
        assert!(snapshot.contains(DEFAULT_THEME_ID));
        assert!(!dir.theme().exists());
    }

    #[test]
    fn v2_migration_golden_corpus_is_byte_identical_in_both_schemes() {
        let corpus = [
            ("default", crate::DEFAULT_THEME_YAML),
            ("mobile-color-channel", "mobile:\n  keyboard:\n    key:\n      borderColor: '#FF222C1C'\n"),
            ("accent", "ui:\n  accentColor: '#FF3366'\n"),
            ("accent-explicit", "ui:\n  accentColor: '#30AA70'\ncandidate:\n  selectedBackground: '#F1E2D3'\n  pressedBackground: '#ABCDEF'\nmodeHint:\n  foreground: '#010203'\n"),
            ("corner-explicit", "ui:\n  cornerRadius: 25\ncandidate:\n  cornerRadius: 2\nnavigation:\n  cornerRadius: 1\n"),
            ("dark-background-on-light", "ui:\n  backgroundColor: '#151A20'\n  colorScheme: light\nlight:\n  candidate:\n    foreground: '#152535'\ndark:\n  candidate:\n    foreground: '#EEF0FF'\n"),
            ("font-light", "font:\n  weight: light\n  size: 27\n"),
            ("font-heavy", "font:\n  weight: heavy\n  family: 'PingFang SC, Microsoft YaHei UI, Noto Sans CJK SC'\n"),
            ("invalid-short-accent", "ui: {accentColor: '#abc'}\n"),
            ("invalid-alpha-background", "ui: {backgroundColor: '#20304080'}\n"),
            ("legacy-text", "font: {family: ' A ,B,  C D '}\ncandidate: {labelSuffix: '12345678'}\nmodeHint: {chineseText: '12345678', englishText: 'ABCDEFGH'}\n"),
            ("every-ui-flag", "ui:\n  color_scheme: auto\n  nightMode: false\n  embedded_composition: true\n  hide_candidate_panel: true\n  show_tray_mode_indicator: false\n  accentColor: '#FA8444'\n  cornerRadius: 9\n  backgroundColor: '#F0EEE8'\nfont:\n  family: 'A, B, C'\n  weight: ultralight\npanel:\n  maxWidth: 450\ncandidate:\n  maxWidth: 250\n"),
        ];
        let dir = TempDir::new();
        for (name, yaml) in corpus {
            let source = migrate_v2_theme(yaml, "my.golden", name).unwrap();
            let source = parse_theme_source_yaml(&source.to_yaml().unwrap()).unwrap();
            assert!(source.value.get("ui").is_none());
            fs::write(dir.theme(), compile_theme(&source, yaml).unwrap()).unwrap();
            for scheme in [EffectiveColorScheme::Light, EffectiveColorScheme::Dark] {
                let old = crate::resolve_legacy_theme_yaml(yaml, scheme);
                let new = crate::resolve_theme_from_paths_with_system_scheme(
                    None,
                    Some(&dir.theme()),
                    scheme,
                );
                assert_eq!(
                    crate::resolved_theme_json(&new).unwrap(),
                    crate::resolved_theme_json(&old).unwrap(),
                    "{name} {scheme:?}"
                );
            }
        }
    }

    #[test]
    fn migration_is_once_and_extracts_keyboard_without_overwriting_it() {
        for existing_keyboard in [false, true] {
            let dir = TempDir::new();
            fs::write(
                dir.theme(),
                "ui:\n  accentColor: '#123456'\nkeyboard:\n  height: 260\n",
            )
            .unwrap();
            let keyboard = dir.0.join("keyboard.yaml");
            if existing_keyboard {
                fs::write(&keyboard, "height: 310\n").unwrap();
            }
            assert!(migrate_current_theme_once(&dir.theme()).unwrap());
            let saved = fs::read_to_string(dir.0.join("themes/my.current.yaml")).unwrap();
            assert!(!migrate_current_theme_once(&dir.theme()).unwrap());
            assert_eq!(
                saved,
                fs::read_to_string(dir.0.join("themes/my.current.yaml")).unwrap()
            );
            let layout: Value =
                serde_yaml::from_str(&fs::read_to_string(keyboard).unwrap()).unwrap();
            assert_eq!(layout["height"], if existing_keyboard { 310 } else { 260 });
            let compiled = read_base(&dir.theme()).unwrap();
            assert_eq!(compiled["version"], 2);
            assert!(compiled.get("common").is_none());
            assert!(compiled.get("keyboard").is_none());
            assert!(compiled["ui"].get("accentColor").is_none());
        }
    }

    #[test]
    fn migration_recovers_orphaned_and_missing_sources() {
        for (missing_source, appearance) in [
            (false, "both"),
            (true, "both"),
            (true, "light"),
            (true, "dark"),
        ] {
            let dir = TempDir::new();
            fs::write(dir.theme(), "ui: {accentColor: '#FF0000'}\n").unwrap();
            if missing_source {
                migrate_current_theme_once(&dir.theme()).unwrap();
                let mut source = read_current_theme_source(&dir.theme()).unwrap().unwrap();
                source.value["meta"]["appearance"] = json!(appearance);
                let source = parse_source_value(source.value).unwrap();
                save_source_and_compile(&dir.theme(), &source, &read_base(&dir.theme()).unwrap())
                    .unwrap();
                fs::remove_dir_all(dir.0.join("themes")).unwrap();
            } else {
                fs::create_dir_all(dir.0.join("themes")).unwrap();
                fs::write(dir.0.join("themes/my.current.yaml"), "orphaned source").unwrap();
            }
            let before: Vec<_> = [EffectiveColorScheme::Light, EffectiveColorScheme::Dark]
                .into_iter()
                .map(|scheme| {
                    crate::resolved_theme_json(&crate::resolve_theme_from_paths_with_system_scheme(
                        None,
                        Some(&dir.theme()),
                        scheme,
                    ))
                    .unwrap()
                })
                .collect();
            assert!(refresh_current_theme(&dir.theme()).unwrap());
            assert!(!refresh_current_theme(&dir.theme()).unwrap());
            for (index, scheme) in [EffectiveColorScheme::Light, EffectiveColorScheme::Dark]
                .into_iter()
                .enumerate()
            {
                assert_eq!(
                    crate::resolved_theme_json(
                        &crate::resolve_theme_from_paths_with_system_scheme(
                            None,
                            Some(&dir.theme()),
                            scheme
                        )
                    )
                    .unwrap(),
                    before[index]
                );
            }
            crate::write_theme_ui_to_path(&dir.theme(), UiColorScheme::Dark, None).unwrap();
            assert_eq!(
                read_base(&dir.theme()).unwrap()["ui"]["colorScheme"],
                "dark"
            );
            assert!(read_current_theme_source(&dir.theme()).unwrap().is_some());
        }
    }

    #[test]
    fn writeback_keeps_migrated_text_verbatim() {
        let dir = TempDir::new();
        fs::write(
            dir.theme(),
            "font: {family: ' A ,B,  C D '}\ncandidate: {labelSuffix: '12345678'}\n",
        )
        .unwrap();
        crate::write_theme_ui_to_path(&dir.theme(), UiColorScheme::Auto, None).unwrap();
        crate::write_theme_ui_to_path(&dir.theme(), UiColorScheme::Dark, None).unwrap();
        let theme = crate::resolve_theme_from_paths_with_system_scheme(
            None,
            Some(&dir.theme()),
            EffectiveColorScheme::Light,
        );
        assert_eq!(theme.font.family.as_deref(), Some(" A ,B,  C D "));
        assert_eq!(theme.candidate.label_suffix, "12345678");
    }

    #[test]
    fn manual_edit_detection_preserves_by_default_and_can_discard() {
        for choice in [
            ManualThemeEditChoice::default(),
            ManualThemeEditChoice::Discard,
        ] {
            let dir = TempDir::new();
            fs::write(dir.theme(), "version: 2\n").unwrap();
            migrate_current_theme_once(&dir.theme()).unwrap();
            let original = fs::read_to_string(dir.0.join("themes/my.current.yaml")).unwrap();
            let source = read_current_theme_source(&dir.theme()).unwrap().unwrap();
            let mut base = read_base(&dir.theme()).unwrap();
            let mut bad_hash = base.clone();
            bad_hash["themeHash"] = Value::String("0000000000000000".into());
            assert!(
                !compiled_theme_has_manual_edits(&source, &value_yaml(&bad_hash).unwrap()).unwrap()
            );
            base["ui"]["hideCandidatePanel"] = Value::Bool(true);
            assert!(
                !compiled_theme_has_manual_edits(&source, &value_yaml(&base).unwrap()).unwrap()
            );
            base["candidate"]["labelSuffix"] = Value::String(")".into());
            fs::write(dir.theme(), value_yaml(&base).unwrap()).unwrap();
            assert!(compiled_theme_has_manual_edits(&source, &value_yaml(&base).unwrap()).unwrap());
            let path = resolve_manual_theme_edits(&dir.theme(), choice).unwrap();
            let new = read_current_theme_source(&dir.theme()).unwrap().unwrap();
            let compiled = read_base(&dir.theme()).unwrap();
            assert!(
                !compiled_theme_has_manual_edits(&new, &value_yaml(&compiled).unwrap()).unwrap()
            );
            assert_eq!(
                fs::read_to_string(dir.0.join("themes/my.current.yaml")).unwrap(),
                original
            );
            match choice {
                ManualThemeEditChoice::SaveAsNew => {
                    assert_eq!(path.file_name().unwrap(), "my.current-2.yaml");
                    assert_eq!(compiled["candidate"]["labelSuffix"], ")");
                }
                ManualThemeEditChoice::Discard => {
                    assert_eq!(compiled["candidate"]["labelSuffix"], ".")
                }
            }
        }
    }

    #[test]
    fn save_manual_copy_preserves_both_resolved_variants_and_source_partial() {
        let dir = TempDir::new();
        let source = parse_source_value(json!({"version":3,"meta":{"id":"my.original","name":"Original","author":"Author","appearance":"both"},"common":{"accentColor":"#123456","cornerRadius":21,"font":{"family":"A, B","weight":"heavy"}},"future":{"keep":true},"dark":{"pc":{"candidate":{"foreground":"#FFEECC"}}}})).unwrap();
        save_source_and_compile(
            &dir.theme(),
            &source,
            &json!({"ui":{"hideCandidatePanel":true}}),
        )
        .unwrap();
        let mut manual = read_base(&dir.theme()).unwrap();
        manual["candidate"]["labelSuffix"] = json!(")");
        manual["dark"]["candidate"]["labelSuffix"] = json!("]");
        fs::write(dir.theme(), value_yaml(&manual).unwrap()).unwrap();
        let before: Vec<_> = [EffectiveColorScheme::Light, EffectiveColorScheme::Dark]
            .into_iter()
            .map(|scheme| {
                crate::resolved_theme_json(&crate::resolve_theme_from_paths_with_system_scheme(
                    None,
                    Some(&dir.theme()),
                    scheme,
                ))
                .unwrap()
            })
            .collect();
        resolve_manual_theme_edits(&dir.theme(), ManualThemeEditChoice::default()).unwrap();
        let copy = read_current_theme_source(&dir.theme()).unwrap().unwrap();
        assert_eq!(copy.value["meta"]["author"], "Author");
        assert_eq!(copy.value["future"]["keep"], true);
        assert_eq!(copy.value["common"], source.value["common"]);
        assert_eq!(copy.value["light"]["pc"]["candidate"]["labelSuffix"], ")");
        assert_eq!(copy.value["dark"]["pc"]["candidate"]["labelSuffix"], "]");
        assert!(copy.value["light"].get("mobile").is_none());
        for (index, scheme) in [EffectiveColorScheme::Light, EffectiveColorScheme::Dark]
            .into_iter()
            .enumerate()
        {
            let after =
                crate::resolved_theme_json(&crate::resolve_theme_from_paths_with_system_scheme(
                    None,
                    Some(&dir.theme()),
                    scheme,
                ))
                .unwrap();
            assert_eq!(after, before[index], "{scheme:?}");
        }
        assert!(dir.0.join("themes/my.original.yaml").is_file());
    }

    #[test]
    fn unrepresentable_manual_changes_are_backed_up_without_blocking_writeback() {
        for appearance in ["light", "both"] {
            let dir = TempDir::new();
            let source = parse_source_value(json!({"version":3,"meta":{"id":"my.original","name":"Original","appearance":appearance},"common":{"accentColor":"#123456"}})).unwrap();
            save_source_and_compile(
                &dir.theme(),
                &source,
                &json!({"ui":{"hideCandidatePanel":true,"showTrayModeIndicator":false},"future":{"keep":true}}),
            ).unwrap();
            let source_path = dir.0.join("themes/my.original.yaml");
            let original_source = fs::read_to_string(&source_path).unwrap();
            let mut manual = read_base(&dir.theme()).unwrap();
            if appearance == "light" {
                manual["dark"]["candidate"]["labelSuffix"] = json!(")");
            } else {
                manual["mobile"]["keyboard"]["keyStates"]["selected"]["background"] =
                    serde_json::to_value(crate::parse_color("#ABCDEF").unwrap()).unwrap();
            }
            let original_compiled = value_yaml(&manual).unwrap();
            fs::write(dir.theme(), &original_compiled).unwrap();
            for _ in 0..3 {
                crate::write_theme_ui_to_path(&dir.theme(), UiColorScheme::Auto, None).unwrap();
                let compiled = read_base(&dir.theme()).unwrap();
                assert_eq!(compiled["ui"]["hideCandidatePanel"], true);
                assert_eq!(compiled["ui"]["showTrayModeIndicator"], false);
                assert_eq!(compiled["future"]["keep"], true);
            }
            assert_eq!(fs::read_to_string(&source_path).unwrap(), original_source);
            let backups: Vec<_> = fs::read_dir(&dir.0)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| {
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("theme.yaml.manual-")
                })
                .collect();
            assert_eq!(backups.len(), 1);
            assert_eq!(fs::read_to_string(&backups[0]).unwrap(), original_compiled);
            assert!(!dir.0.join("themes/my.current-2.yaml").exists());
        }
    }

    #[test]
    fn source_writeback_preserves_unknown_fields_and_keeps_v2_separate() {
        let dir = TempDir::new();
        fs::write(
            dir.theme(),
            "version: 2\nui:\n  showTrayModeIndicator: false\n",
        )
        .unwrap();
        migrate_current_theme_once(&dir.theme()).unwrap();
        let path = dir.0.join("themes/my.current.yaml");
        let mut source = read_current_theme_source(&dir.theme()).unwrap().unwrap();
        source.value["future"] = json!({"untouched": true});
        source = parse_source_value(source.value).unwrap();
        save_source_and_compile(&dir.theme(), &source, &read_base(&dir.theme()).unwrap()).unwrap();
        crate::write_ime_ui_settings_to_path(
            &dir.theme(),
            UiColorScheme::Dark,
            crate::PanelOrientation::Horizontal,
            Some("#123456".into()),
            27.0,
            Some(22.0),
            "#445566".into(),
        )
        .unwrap();
        let source = read_current_theme_source(&dir.theme()).unwrap().unwrap();
        assert_eq!(source.value["future"]["untouched"], true);
        assert_eq!(source.value["version"], 3);
        assert_eq!(source.value["pc"]["font"]["size"], 27.0);
        assert!(source.value.get("ui").is_none());
        assert!(path.is_file());
        let compiled = read_base(&dir.theme()).unwrap();
        assert_eq!(compiled["version"], 2);
        assert_eq!(compiled["ui"]["colorScheme"], "dark");
        assert_eq!(compiled["ui"]["showTrayModeIndicator"], false);
        assert!(compiled["ui"].get("accentColor").is_none());
        assert!(compiled.get("common").is_none());
    }

    #[test]
    fn editing_common_controls_releases_derived_pins_and_keeps_explicit_values() {
        let original =
            "ui:\n  accentColor: '#123456'\n  cornerRadius: 24\n  backgroundColor: '#151A20'\n";
        let source = migrate_v2_theme(original, "my.controls", "Controls").unwrap();
        let source = edit_source_settings(
            source,
            Some("#EE5500".into()),
            Some(Some(8.0)),
            Some("#F0EEE8".into()),
            None,
            None,
        )
        .unwrap();
        for scheme in [EffectiveColorScheme::Light, EffectiveColorScheme::Dark] {
            let theme = resolve_theme_source(&source, scheme);
            assert_eq!(theme.panel.corner_radius, 8.0);
            assert_eq!(theme.candidate.corner_radius, 5.5);
            assert_eq!(
                theme.candidate.selected_label_color,
                crate::parse_color("#EE5500").unwrap()
            );
            assert_eq!(
                theme.panel.background,
                crate::parse_color("#F0EEE8F2").unwrap()
            );
        }
        let source = parse_source_value(json!({"version":3,"meta":{"id":"my.explicit","name":"Explicit"},"common":{"accentColor":"#123456","cornerRadius":24},"pc":{"candidate":{"selectedBackground":"#22334455","cornerRadius":3}}})).unwrap();
        let source = edit_source_settings(
            source,
            Some("#EE5500".into()),
            Some(Some(8.0)),
            None,
            None,
            None,
        )
        .unwrap();
        let theme = resolve_theme_source(&source, EffectiveColorScheme::Light);
        assert_eq!(
            theme.candidate.selected_background,
            crate::parse_color("#22334455").unwrap()
        );
        assert_eq!(theme.candidate.corner_radius, 3.0);
    }

    #[test]
    fn invalid_local_ui_is_repaired_without_panicking_or_losing_source_fields() {
        for invalid in [json!([]), json!(false)] {
            let dir = TempDir::new();
            fs::write(
                dir.theme(),
                "font: {size: 27}\ncandidate: {labelSuffix: ')'}\nfuture: {keep: true}\n",
            )
            .unwrap();
            migrate_current_theme_once(&dir.theme()).unwrap();
            let mut base = read_base(&dir.theme()).unwrap();
            base["ui"] = invalid;
            let original = value_yaml(&base).unwrap();
            fs::write(dir.theme(), &original).unwrap();
            for _ in 0..2 {
                crate::write_theme_ui_to_path(&dir.theme(), UiColorScheme::Dark, None).unwrap();
                let base = read_base(&dir.theme()).unwrap();
                assert_eq!(base["ui"]["colorScheme"], "dark");
                assert_eq!(base["font"]["size"], 27.0);
                assert_eq!(base["candidate"]["labelSuffix"], ")");
                assert_eq!(base["future"]["keep"], true);
                assert_manual_backup(&dir, &original);
            }
        }
    }

    fn assert_manual_backup(dir: &TempDir, original: &str) {
        let backups: Vec<_> = fs::read_dir(&dir.0)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("theme.yaml.manual-")
            })
            .collect();
        assert_eq!(backups.len(), 1);
        let name = backups[0].file_name().unwrap().to_str().unwrap();
        name.strip_prefix("theme.yaml.manual-")
            .unwrap()
            .strip_suffix(".bak")
            .unwrap()
            .parse::<u64>()
            .unwrap();
        assert_eq!(fs::read_to_string(&backups[0]).unwrap(), original);
    }

    #[test]
    fn malformed_theme_is_backed_up_before_requested_settings_are_written() {
        for original in ["ui: [", "ui:\n  colorScheme: [dark", "- not-a-mapping"] {
            let dir = TempDir::new();
            fs::write(dir.theme(), original).unwrap();
            crate::write_ime_ui_settings_to_path(
                &dir.theme(),
                UiColorScheme::Dark,
                crate::PanelOrientation::Horizontal,
                Some("#123456".into()),
                27.0,
                Some(9.0),
                "#654321".into(),
            )
            .unwrap();
            let base = read_base(&dir.theme()).unwrap();
            assert_eq!(base["ui"]["colorScheme"], "dark");
            assert_eq!(base["panel"]["orientation"], "horizontal");
            assert_eq!(base["font"]["size"], 27.0);
            let source = read_current_theme_source(&dir.theme()).unwrap().unwrap();
            assert_eq!(source.value["common"]["accentColor"], "#123456FF");
            assert_eq!(source.value["common"]["cornerRadius"], 9.0);
            assert_eq!(source.value["common"]["backgroundColor"], "#654321");
            assert_manual_backup(&dir, original);
        }
    }

    #[test]
    fn malformed_theme_writeback_never_overwrites_existing_sources() {
        for active in [CURRENT_THEME_ID, "my.other"] {
            for original in ["ui: [", "ui:\n  colorScheme: [dark", "- not-a-mapping"] {
                let dir = TempDir::new();
                let mut originals = Vec::new();
                for id in ["my.current", "my.current-2", "my.other"] {
                    let source = parse_source_value(json!({
                        "version": 3, "meta": {"id": id, "name": "Custom"},
                        "common": {
                            "accentColor": "#123456FF", "cornerRadius": 9,
                            "backgroundColor": "#654321",
                        },
                    }))
                    .unwrap();
                    save_source_and_compile(&dir.theme(), &source, &json!({})).unwrap();
                    let path = source_path(&dir.theme(), id);
                    originals.push((path.clone(), fs::read(&path).unwrap()));
                }
                let source = parse_theme_source_yaml(
                    &fs::read_to_string(source_path(&dir.theme(), active)).unwrap(),
                )
                .unwrap();
                fs::write(dir.theme(), compile_theme(&source, "{}").unwrap()).unwrap();
                fs::write(dir.theme(), original).unwrap();
                for _ in 0..2 {
                    crate::write_theme_ui_to_path(&dir.theme(), UiColorScheme::Dark, None).unwrap();
                    for (path, content) in &originals {
                        assert_eq!(&fs::read(path).unwrap(), content, "{}", path.display());
                    }
                    assert_eq!(
                        read_base(&dir.theme()).unwrap()["ui"]["colorScheme"],
                        "dark"
                    );
                    assert!(read_current_theme_source(&dir.theme()).unwrap().is_some());
                    assert_manual_backup(&dir, original);
                }
            }
        }
    }

    #[test]
    fn migration_discards_only_invalid_mapping_sections_and_backs_up_original() {
        for section in ["ui", "panel", "light", "dark"] {
            for invalid in [json!([]), json!(false)] {
                let dir = TempDir::new();
                let mut base = json!({
                    "ui": {"embeddedComposition": true, "hideCandidatePanel": true, "showTrayModeIndicator": false},
                    "font": {"size": 27}, "candidate": {"labelSuffix": ")"},
                    "future": {"keep": [1, 2]},
                });
                base[section] = invalid;
                let original = value_yaml(&base).unwrap();
                fs::write(dir.theme(), &original).unwrap();
                crate::write_theme_ui_to_path(&dir.theme(), UiColorScheme::Dark, None).unwrap();
                let saved = read_base(&dir.theme()).unwrap();
                assert_eq!(saved["ui"]["colorScheme"], "dark");
                if section != "ui" {
                    assert_eq!(saved["ui"]["embeddedComposition"], true);
                    assert_eq!(saved["ui"]["hideCandidatePanel"], true);
                    assert_eq!(saved["ui"]["showTrayModeIndicator"], false);
                }
                assert_eq!(saved["font"]["size"], 27.0);
                assert_eq!(saved["candidate"]["labelSuffix"], ")");
                assert_eq!(saved["future"]["keep"], json!([1, 2]));
                assert_manual_backup(&dir, &original);
            }
        }
    }

    #[test]
    fn startup_refresh_updates_old_compilation_without_confusing_manual_edits() {
        let dir = TempDir::new();
        migrate_current_theme_once(&dir.theme()).unwrap();
        let source = read_current_theme_source(&dir.theme()).unwrap().unwrap();
        let mut older = read_base(&dir.theme()).unwrap();
        older["font"]["labelSize"] = json!(17);
        older["compiledHash"] = json!(crate::source::compiled_appearance_hash(&older).unwrap());
        fs::write(dir.theme(), value_yaml(&older).unwrap()).unwrap();
        assert!(!compiled_theme_has_manual_edits(&source, &value_yaml(&older).unwrap()).unwrap());
        assert!(refresh_current_theme(&dir.theme()).unwrap());
        assert!(!refresh_current_theme(&dir.theme()).unwrap());
        let mut manual = read_base(&dir.theme()).unwrap();
        manual["font"]["labelSize"] = json!(19);
        fs::write(dir.theme(), value_yaml(&manual).unwrap()).unwrap();
        assert!(!refresh_current_theme(&dir.theme()).unwrap());
        assert_eq!(read_base(&dir.theme()).unwrap()["font"]["labelSize"], 19);
    }

    #[test]
    fn local_source_edits_recompile_without_forking_or_losing_changes() {
        for refresh in [false, true] {
            let dir = TempDir::new();
            migrate_current_theme_once(&dir.theme()).unwrap();
            let mut source = read_current_theme_source(&dir.theme()).unwrap().unwrap();
            source.value["common"]["accentColor"] = json!("#00FF00FF");
            fs::write(
                dir.0.join("themes/my.current.yaml"),
                source.to_yaml().unwrap(),
            )
            .unwrap();
            if refresh {
                assert!(refresh_current_theme(&dir.theme()).unwrap());
            }
            crate::write_theme_ui_to_path(&dir.theme(), UiColorScheme::Auto, None).unwrap();
            let source = read_current_theme_source(&dir.theme()).unwrap().unwrap();
            assert_eq!(source.id(), CURRENT_THEME_ID);
            assert_eq!(source.value["common"]["accentColor"], "#00FF00FF");
            assert_eq!(
                crate::resolve_theme_from_paths_with_system_scheme(
                    None,
                    Some(&dir.theme()),
                    EffectiveColorScheme::Light
                )
                .ui
                .accent_color,
                crate::parse_color("#00FF00")
            );
        }
    }

    #[test]
    fn manual_legacy_ui_knobs_are_not_silently_recompiled_away() {
        for (key, value) in [
            ("accentColor", json!("#FF0000")),
            ("cornerRadius", json!(25)),
            ("backgroundColor", json!("#203040")),
        ] {
            let dir = TempDir::new();
            migrate_current_theme_once(&dir.theme()).unwrap();
            let source = read_current_theme_source(&dir.theme()).unwrap().unwrap();
            let mut base = read_base(&dir.theme()).unwrap();
            base["ui"][key] = value;
            let yaml = value_yaml(&base).unwrap();
            fs::write(dir.theme(), &yaml).unwrap();
            assert!(compiled_theme_has_manual_edits(&source, &yaml).unwrap());
            assert!(!refresh_current_theme(&dir.theme()).unwrap());
            assert_eq!(fs::read_to_string(dir.theme()).unwrap(), yaml);
        }
    }

    fn manual_appearance_writebacks(
        section: &str,
        key: &str,
        value: Value,
        preserve_manual_edit: bool,
    ) {
        let dir = TempDir::new();
        fs::write(dir.theme(), "ui: {embeddedComposition: true, hideCandidatePanel: true, showTrayModeIndicator: false}\ncandidate: {labelSuffix: ')'}\nfuture: {keep: true}\n").unwrap();
        migrate_current_theme_once(&dir.theme()).unwrap();
        let original_source = fs::read_to_string(dir.0.join("themes/my.current.yaml")).unwrap();
        let source = read_current_theme_source(&dir.theme()).unwrap().unwrap();
        let mut base = read_base(&dir.theme()).unwrap();
        base[section][key] = value;
        let manual_yaml = value_yaml(&base).unwrap();
        fs::write(dir.theme(), &manual_yaml).unwrap();
        let expected_yaml = if preserve_manual_edit {
            manual_yaml.clone()
        } else {
            compile_theme(&source, &manual_yaml).unwrap()
        };
        let expected: Vec<_> = [EffectiveColorScheme::Light, EffectiveColorScheme::Dark]
            .into_iter()
            .map(|scheme| {
                crate::resolved_theme_json(&crate::resolve_compiled_theme_yaml(
                    &expected_yaml,
                    scheme,
                ))
                .unwrap()
            })
            .collect();
        for _ in 0..3 {
            crate::write_theme_ui_to_path(&dir.theme(), UiColorScheme::Auto, None).unwrap();
            let backups: Vec<_> = fs::read_dir(&dir.0)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| {
                    let name = path.file_name().unwrap().to_string_lossy();
                    name.starts_with("theme.yaml.manual-") && name.ends_with(".bak")
                })
                .collect();
            if preserve_manual_edit {
                assert!(backups.is_empty());
            } else {
                assert_eq!(backups.len(), 1);
                assert_eq!(fs::read_to_string(&backups[0]).unwrap(), manual_yaml);
            }
            let base = read_base(&dir.theme()).unwrap();
            assert_eq!(base["ui"]["embeddedComposition"], true);
            assert_eq!(base["ui"]["hideCandidatePanel"], true);
            assert_eq!(base["ui"]["showTrayModeIndicator"], false);
            assert_eq!(base["future"]["keep"], true);
            for (index, scheme) in [EffectiveColorScheme::Light, EffectiveColorScheme::Dark]
                .into_iter()
                .enumerate()
            {
                let theme = crate::resolve_theme_from_paths_with_system_scheme(
                    None,
                    Some(&dir.theme()),
                    scheme,
                );
                assert_eq!(theme.candidate.label_suffix, ")");
                assert_eq!(crate::resolved_theme_json(&theme).unwrap(), expected[index]);
            }
        }
        let expected_id = if preserve_manual_edit {
            "my.current-2"
        } else {
            CURRENT_THEME_ID
        };
        assert_eq!(
            read_current_theme_source(&dir.theme())
                .unwrap()
                .unwrap()
                .id(),
            expected_id
        );
        assert_eq!(
            fs::read_to_string(dir.0.join("themes/my.current.yaml")).unwrap(),
            original_source
        );
    }

    #[test]
    fn manual_accent_survives_three_source_writebacks() {
        manual_appearance_writebacks("ui", "accentColor", json!("#FF0000"), true);
    }

    #[test]
    fn manual_radius_survives_three_writebacks() {
        manual_appearance_writebacks("ui", "cornerRadius", json!(25), true);
    }

    #[test]
    fn manual_background_is_backed_up_before_three_source_writebacks() {
        manual_appearance_writebacks("ui", "backgroundColor", json!("#203040"), false);
    }

    #[test]
    fn manual_panel_color_survives_three_source_writebacks() {
        manual_appearance_writebacks("panel", "background", json!("#11223344"), true);
    }

    #[test]
    fn built_in_edits_fork_and_document_ids_cannot_escape_themes_directory() {
        let dir = TempDir::new();
        let source = parse_source_value(
            json!({"version":3,"meta":{"id":"keytao.default","name":"Default"}}),
        )
        .unwrap();
        save_source_and_compile(&dir.theme(), &source, &json!({})).unwrap();
        let built_in_path = dir.0.join("themes/keytao.default.yaml");
        let original = fs::read_to_string(&built_in_path).unwrap();
        crate::write_theme_ui_to_path(&dir.theme(), UiColorScheme::Auto, Some("#123456".into()))
            .unwrap();
        assert_eq!(fs::read_to_string(built_in_path).unwrap(), original);
        assert_eq!(
            read_current_theme_source(&dir.theme())
                .unwrap()
                .unwrap()
                .id(),
            "my.default-2"
        );
        for id in ["../escape", "con.theme", "my.nul"] {
            fs::write(
                dir.theme(),
                value_yaml(&json!({"version":2,"themeId":id})).unwrap(),
            )
            .unwrap();
            assert!(current_theme_source_path(&dir.theme()).is_err());
        }
    }

    fn import_source(id: &str, extra: Value) -> String {
        let mut source = json!({"version":3,"meta":{"id":id,"name":"薄荷"}});
        merge(&mut source, &extra);
        source.to_string()
    }

    #[test]
    fn library_roundtrip_keeps_source_partial_and_local_ui() {
        let dir = TempDir::new();
        fs::write(dir.theme(), "ui: {colorScheme: dark, embeddedComposition: true, hideCandidatePanel: true, showTrayModeIndicator: false}\n").unwrap();
        let result = import_theme(
            &dir.theme(),
            &import_source(
                "rea.mint",
                json!({
                    "common":{"accentColor":"#2bb38a"},
                    "ui":{"colorScheme":"light"},"themeId":"local","keyboard":{"rows":[]},
                    "mobile":{"keyboard":{"height":900,"key":{"foreground":"#123"}}}
                }),
            ),
            false,
        )
        .unwrap();
        assert_eq!(result.disposition, "installed");
        apply_theme(&dir.theme(), &result.id).unwrap();
        let exported = export_theme(&dir.theme(), &result.id).unwrap();
        assert_eq!(exported.file_name, "rea.mint.keytao-theme.json");
        assert!(!exported.use_file);
        let value: Value = serde_json::from_str(&exported.json).unwrap();
        assert!(value.get("ui").is_none());
        assert!(value.get("themeId").is_none());
        assert!(value.get("keyboard").is_none());
        assert!(value["mobile"]["keyboard"].get("height").is_none());
        assert_eq!(value["common"]["accentColor"], "#2BB38AFF");
        assert!(value.get("pc").is_none());
        let other = TempDir::new();
        let imported = import_theme(&other.theme(), &exported.share_string, false).unwrap();
        assert_eq!(
            export_theme(&other.theme(), &imported.id).unwrap().json,
            exported.json
        );
        let current = list_themes(&dir.theme()).unwrap();
        assert_eq!(current.iter().filter(|theme| theme.current).count(), 1);
        let selected = current.iter().find(|theme| theme.id == result.id).unwrap();
        assert!(!selected.built_in);
        assert_eq!(selected.light.accent, "#2BB38AFF");
        for colors in [&selected.light, &selected.dark] {
            assert_eq!(colors.background.len(), 9);
            assert_eq!(colors.foreground.len(), 9);
        }
        let ui = &read_base(&dir.theme()).unwrap()["ui"];
        assert_eq!(ui["colorScheme"], "dark");
        assert_eq!(ui["embeddedComposition"], true);
        assert_eq!(ui["hideCandidatePanel"], true);
        assert_eq!(ui["showTrayModeIndicator"], false);
    }

    #[test]
    fn library_conflicts_preserve_reports_and_recompile_current_overwrites() {
        let dir = TempDir::new();
        let original = import_source("my.mint", json!({"common":{"accentColor":"#123"}}));
        assert!(
            !preview_theme_import(&dir.theme(), &original)
                .unwrap()
                .conflict
        );
        import_theme(&dir.theme(), &original, false).unwrap();
        let path = source_path(&dir.theme(), "my.mint");
        let original_yaml = fs::read_to_string(&path).unwrap();
        let duplicate = import_theme(&dir.theme(), &original, true).unwrap();
        assert_eq!(duplicate.disposition, "alreadyInstalled");
        assert_eq!(fs::read_to_string(&path).unwrap(), original_yaml);
        let changed = import_source(
            "my.mint",
            json!({
                "common":{"accentColor":"#abc","cornerRadius":100},
                "pc":{"candidate":{"foreground":"bad","labelColor":"#456"}},
                "future":{"keep":true}
            }),
        );
        let preview = preview_theme_import(&dir.theme(), &changed).unwrap();
        assert!(preview.conflict && preview.can_overwrite);
        let copy = import_theme(&dir.theme(), &changed, false).unwrap();
        assert_eq!(copy.id, "my.mint-2");
        assert_eq!(copy.name, "薄荷（副本）");
        assert_eq!(copy.renamed_from.as_deref(), Some("my.mint"));
        assert!(copy
            .dropped
            .iter()
            .any(|field| field.contains("foreground")));
        assert!(copy
            .clamped
            .iter()
            .any(|field| field.contains("cornerRadius")));
        assert!(copy.unknown.iter().any(|field| field.contains("future")));
        assert_eq!(
            read_source(&dir.theme(), &copy.id).unwrap().value["pc"]["candidate"]["labelColor"],
            "#445566FF"
        );
        apply_theme(&dir.theme(), "my.mint").unwrap();
        crate::write_theme_ui_to_path(&dir.theme(), UiColorScheme::Dark, None).unwrap();
        let overwritten = import_theme(&dir.theme(), &changed, true).unwrap();
        assert_eq!(overwritten.disposition, "overwritten");
        assert_eq!(read_base(&dir.theme()).unwrap()["themeId"], "my.mint");
        assert_eq!(
            read_base(&dir.theme()).unwrap()["ui"]["colorScheme"],
            "dark"
        );
        let compiled = crate::resolve_compiled_theme_yaml(
            &fs::read_to_string(dir.theme()).unwrap(),
            EffectiveColorScheme::Light,
        );
        assert_eq!(compiled.ui.accent_color, crate::parse_color("#AABBCC"));
    }

    #[test]
    fn library_builtin_protection_and_fork_names_are_stable() {
        let dir = TempDir::new();
        let default = list_themes(&dir.theme()).unwrap();
        assert_eq!(default.len(), 1);
        assert!(default[0].built_in && default[0].current);
        assert!(delete_theme(&dir.theme(), DEFAULT_THEME_ID).is_err());
        apply_theme(&dir.theme(), DEFAULT_THEME_ID).unwrap();
        assert!(!source_path(&dir.theme(), DEFAULT_THEME_ID).exists());
        crate::write_theme_ui_to_path(&dir.theme(), UiColorScheme::Dark, None).unwrap();
        assert_eq!(
            read_base(&dir.theme()).unwrap()["themeId"],
            DEFAULT_THEME_ID
        );
        assert!(!source_path(&dir.theme(), DEFAULT_THEME_ID).exists());
        let current = resolve_theme_source(&built_in_theme(), EffectiveColorScheme::Light);
        crate::write_ime_ui_settings_to_path(
            &dir.theme(),
            UiColorScheme::Light,
            current.panel.orientation,
            None,
            current.font.size,
            current.ui.corner_radius,
            String::new(),
        )
        .unwrap();
        assert_eq!(
            read_base(&dir.theme()).unwrap()["themeId"],
            DEFAULT_THEME_ID
        );
        assert!(!source_path(&dir.theme(), DEFAULT_THEME_ID).exists());
        assert!(!source_path(&dir.theme(), "my.default-2").exists());
        assert_eq!(
            duplicate_theme(&dir.theme(), DEFAULT_THEME_ID).unwrap(),
            "my.default-2"
        );
        assert_eq!(
            duplicate_theme(&dir.theme(), DEFAULT_THEME_ID).unwrap(),
            "my.default-3"
        );
        crate::write_theme_ui_to_path(&dir.theme(), UiColorScheme::Auto, Some("#123456".into()))
            .unwrap();
        assert_eq!(read_base(&dir.theme()).unwrap()["themeId"], "my.default-4");
        let builtin = import_source(DEFAULT_THEME_ID, json!({"common":{"accentColor":"#F00"}}));
        let preview = preview_theme_import(&dir.theme(), &builtin).unwrap();
        assert!(preview.conflict && !preview.can_overwrite);
        let result = import_theme(&dir.theme(), &builtin, true).unwrap();
        assert_eq!(result.disposition, "copied");
        assert_eq!(result.id, "my.default-5");
        assert!(delete_theme(&dir.theme(), "keytao.other").is_err());
        delete_theme(&dir.theme(), "my.default-4").unwrap();
        assert_eq!(
            read_base(&dir.theme()).unwrap()["themeId"],
            DEFAULT_THEME_ID
        );
        assert!(refresh_current_theme(&dir.theme()).is_ok());
        assert!(!source_path(&dir.theme(), "my.default-4").exists());
    }

    #[test]
    fn library_serializes_duplicate_naming_and_handles_long_ids() {
        let dir = TempDir::new();
        let id = format!("my.{}", "x".repeat(61));
        import_theme(&dir.theme(), &import_source(&id, json!({})), false).unwrap();
        let path = dir.theme();
        let mut threads = Vec::new();
        for _ in 0..4 {
            let path = path.clone();
            let id = id.clone();
            threads.push(std::thread::spawn(move || {
                duplicate_theme(&path, &id).unwrap()
            }));
        }
        let mut copies: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        copies.sort();
        copies.dedup();
        assert_eq!(copies.len(), 4);
        for id in copies {
            crate::source::validate_theme_id(&id).unwrap();
            assert_eq!(id.len(), 64);
            assert!(read_source(&path, &id).is_ok());
        }
    }

    #[test]
    fn library_limits_reject_without_writes_and_large_shares_use_files() {
        let dir = TempDir::new();
        for invalid in [
            "version: 3\nmeta: {id: my.yaml, name: YAML}".to_owned(),
            import_source("../escape", json!({})),
            import_source("my.con", json!({})),
            import_source("my.large", json!({"future":"x".repeat(65536)})),
            import_source("my.new", json!({"version":4})),
            format!("keytao-theme:3:{}", "A".repeat(96 * 1024)),
        ] {
            assert!(import_theme(&dir.theme(), &invalid, false).is_err());
            assert!(!dir.theme().exists());
            assert!(!dir.0.join("themes").exists());
        }
        let mut seed = 1u32;
        let noise: String = (0..12000)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                char::from(b'!' + ((seed >> 24) % 90) as u8)
            })
            .collect();
        import_theme(
            &dir.theme(),
            &import_source("my.file", json!({"future":noise})),
            false,
        )
        .unwrap();
        let exported = export_theme(&dir.theme(), "my.file").unwrap();
        assert!(exported.share_string.len() > 4000);
        assert!(exported.use_file);
    }

    #[test]
    fn library_stores_canonical_limit_without_yaml_expansion() {
        let dir = TempDir::new();
        let mut source = json!({"version":3,"meta":{"id":"my.boundary","name":"Boundary"},
            "future":vec![json!({"a":{"b":{"c":0}}}); 2500],"padding":""});
        let padding = crate::source::MAX_THEME_SOURCE_BYTES - source.to_string().len();
        source["padding"] = json!("x".repeat(padding));
        let text = source.to_string();
        assert_eq!(text.len(), crate::source::MAX_THEME_SOURCE_BYTES);
        assert!(
            serde_yaml::to_string(&source).unwrap().len() > crate::source::MAX_THEME_SOURCE_BYTES
        );
        import_theme(&dir.theme(), &text, false).unwrap();
        assert_eq!(
            fs::read(source_path(&dir.theme(), "my.boundary"))
                .unwrap()
                .len(),
            crate::source::MAX_THEME_SOURCE_BYTES
        );
        assert_eq!(list_themes(&dir.theme()).unwrap().len(), 2);
        assert_eq!(
            export_theme(&dir.theme(), "my.boundary")
                .unwrap()
                .json
                .len(),
            crate::source::MAX_THEME_SOURCE_BYTES
        );
        apply_theme(&dir.theme(), "my.boundary").unwrap();
        assert_eq!(
            read_current_theme_source(&dir.theme())
                .unwrap()
                .unwrap()
                .id(),
            "my.boundary"
        );
    }

    #[test]
    fn library_self_import_compares_normalized_legacy_exports() {
        let dir = TempDir::new();
        let source = parse_theme_source_yaml("version: 3\nmeta: {id: my.legacy, name: Legacy}\ncommon: {font: {family: ' A , B '}}\npc: {candidate: {labelSuffix: '12345678'}}\n").unwrap();
        save_source_and_compile(&dir.theme(), &source, &json!({})).unwrap();
        let original = fs::read(source_path(&dir.theme(), source.id())).unwrap();
        let exported = export_theme(&dir.theme(), source.id()).unwrap();
        assert!(
            !preview_theme_import(&dir.theme(), &exported.share_string)
                .unwrap()
                .conflict
        );
        let imported = import_theme(&dir.theme(), &exported.share_string, false).unwrap();
        assert_eq!(imported.disposition, "alreadyInstalled");
        assert_eq!(
            fs::read(source_path(&dir.theme(), source.id())).unwrap(),
            original
        );
    }

    #[cfg(unix)]
    #[test]
    fn library_regression_allows_symlinked_data_directory_ancestors() {
        use std::os::unix::fs::symlink;
        let dir = TempDir::new();
        let data = dir.0.join("data");
        let alias = dir.0.join("alias");
        fs::create_dir(&data).unwrap();
        symlink(&data, &alias).unwrap();
        let theme = alias.join("theme.yaml");
        crate::write_theme_appearance_to_path(
            &theme,
            UiColorScheme::Dark,
            Some("#123456".into()),
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            read_theme_for_update(&theme).unwrap()["ui"]["colorScheme"],
            "dark"
        );
        assert!(!migrate_current_theme_once(&theme).unwrap());
        refresh_current_theme(&theme).unwrap();
        let source = import_source("my.linked", json!({}));
        preview_theme_import(&theme, &source).unwrap();
        import_theme(&theme, &source, false).unwrap();
        apply_theme(&theme, "my.linked").unwrap();
        assert!(list_themes(&theme)
            .unwrap()
            .iter()
            .any(|item| item.id == "my.linked" && item.current));
        export_theme(&theme, "my.linked").unwrap();
        duplicate_theme(&theme, "my.linked").unwrap();
        delete_theme(&theme, "my.linked").unwrap();
        assert_eq!(read_base(&theme).unwrap()["themeId"], DEFAULT_THEME_ID);
    }

    #[test]
    fn library_regression_skips_invalid_sources_during_listing_and_import() {
        for (file, content) in [
            ("my.foo copy.yaml", import_source("my.foo", json!({}))),
            ("my.bar.yaml", import_source("my.foo", json!({}))),
            ("my.broken.yaml", "version: [".into()),
        ] {
            let dir = TempDir::new();
            let good = import_source("my.good", json!({}));
            import_theme(&dir.theme(), &good, false).unwrap();
            let bad_path = dir.0.join("themes").join(file);
            fs::write(&bad_path, &content).unwrap();
            let themes = list_themes(&dir.theme()).unwrap();
            assert_eq!(themes.len(), 2);
            assert!(themes.iter().any(|theme| theme.id == "my.good"));
            let incoming = import_source("my.new", json!({}));
            assert!(
                !preview_theme_import(&dir.theme(), &incoming)
                    .unwrap()
                    .conflict
            );
            assert_eq!(
                import_theme(&dir.theme(), &incoming, false).unwrap().id,
                "my.new"
            );
            assert_eq!(fs::read_to_string(&bad_path).unwrap(), content);
        }
    }

    #[test]
    fn library_regression_preserves_invalid_source_import_collisions() {
        let dir = TempDir::new();
        fs::create_dir(dir.0.join("themes")).unwrap();
        for id in ["my.bad", "my.bad-2"] {
            fs::write(source_path(&dir.theme(), id), "version: [").unwrap();
        }
        let incoming = import_source("my.bad", json!({}));
        let preview = preview_theme_import(&dir.theme(), &incoming).unwrap();
        assert!(preview.conflict && preview.can_overwrite);
        let imported = import_theme(&dir.theme(), &incoming, false).unwrap();
        assert_eq!(imported.id, "my.bad-3");
        assert_eq!(imported.disposition, "copied");
        for id in ["my.bad", "my.bad-2"] {
            assert_eq!(
                fs::read_to_string(source_path(&dir.theme(), id)).unwrap(),
                "version: ["
            );
        }
        assert_eq!(
            import_theme(&dir.theme(), &incoming, true)
                .unwrap()
                .disposition,
            "overwritten"
        );
        let changed = import_source("my.bad", json!({"common":{"accentColor":"#123"}}));
        assert_eq!(
            import_theme(&dir.theme(), &changed, false).unwrap().id,
            "my.bad-4"
        );
        assert_eq!(
            fs::read_to_string(source_path(&dir.theme(), "my.bad-2")).unwrap(),
            "version: ["
        );
        apply_theme(&dir.theme(), "my.bad").unwrap();
        let compiled = fs::read_to_string(dir.theme()).unwrap();
        fs::write(source_path(&dir.theme(), "my.bad"), "version: [").unwrap();
        assert_eq!(
            import_theme(&dir.theme(), &changed, true)
                .unwrap()
                .disposition,
            "overwritten"
        );
        assert_eq!(read_base(&dir.theme()).unwrap()["themeId"], "my.bad");
        assert!(!compiled_theme_has_manual_edits(
            &read_source(&dir.theme(), "my.bad").unwrap(),
            &fs::read_to_string(dir.theme()).unwrap(),
        )
        .unwrap());
        assert!(fs::read_dir(&dir.0).unwrap().any(|entry| {
            let path = entry.unwrap().path();
            path.extension().and_then(|extension| extension.to_str()) == Some("bak")
                && fs::read_to_string(path).ok().as_deref() == Some(&compiled)
        }));
    }

    #[test]
    fn library_regression_deletes_invalid_sources_and_preserves_active_compilation() {
        for active in [false, true] {
            for invalid in [
                "version: [".to_owned(),
                import_source("my.other", json!({})),
            ] {
                let dir = TempDir::new();
                import_theme(&dir.theme(), &import_source("my.broken", json!({})), false).unwrap();
                if active {
                    apply_theme(&dir.theme(), "my.broken").unwrap();
                    let mut base = read_base(&dir.theme()).unwrap();
                    base["panel"]["gap"] = json!(13);
                    fs::write(dir.theme(), value_yaml(&base).unwrap()).unwrap();
                }
                let compiled = fs::read_to_string(dir.theme()).ok();
                let path = source_path(&dir.theme(), "my.broken");
                fs::write(&path, invalid).unwrap();
                delete_theme(&dir.theme(), "my.broken").unwrap();
                assert!(!path.exists());
                if active {
                    assert_eq!(
                        read_base(&dir.theme()).unwrap()["themeId"],
                        DEFAULT_THEME_ID
                    );
                    assert!(fs::read_dir(&dir.0).unwrap().any(|entry| {
                        let path = entry.unwrap().path();
                        path.extension().and_then(|extension| extension.to_str()) == Some("bak")
                            && fs::read_to_string(path).ok() == compiled
                    }));
                }
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn library_rejects_symlink_escapes_and_mismatched_ids() {
        use std::os::unix::fs::symlink;
        let dir = TempDir::new();
        let outside = TempDir::new();
        symlink(&outside.0, dir.0.join("themes")).unwrap();
        let source = import_source("my.escape", json!({}));
        assert!(import_theme(&dir.theme(), &source, false).is_err());
        assert!(list_themes(&dir.theme()).is_err());
        assert!(!outside.0.join("my.escape.yaml").exists());
        fs::remove_file(dir.0.join("themes")).unwrap();
        import_theme(&dir.theme(), &source, false).unwrap();
        let path = source_path(&dir.theme(), "my.escape");
        fs::remove_file(&path).unwrap();
        fs::write(outside.0.join("source.yaml"), &source).unwrap();
        symlink(outside.0.join("source.yaml"), &path).unwrap();
        assert!(apply_theme(&dir.theme(), "my.escape").is_err());
        assert!(delete_theme(&dir.theme(), "my.escape").is_err());
        assert!(export_theme(&dir.theme(), "my.escape").is_err());
        assert_eq!(
            fs::read_to_string(outside.0.join("source.yaml")).unwrap(),
            source
        );
        fs::remove_file(&path).unwrap();
        fs::write(&path, import_source("my.other", json!({}))).unwrap();
        assert!(apply_theme(&dir.theme(), "my.escape").is_err());
        apply_theme(&dir.theme(), DEFAULT_THEME_ID).unwrap();
        let compiled = fs::read(dir.theme()).unwrap();
        fs::write(outside.theme(), &compiled).unwrap();
        fs::remove_file(dir.theme()).unwrap();
        symlink(outside.theme(), dir.theme()).unwrap();
        assert!(read_theme_for_update(&dir.theme()).is_err());
        assert!(crate::write_theme_ui_to_path(&dir.theme(), UiColorScheme::Dark, None).is_err());
        assert_eq!(fs::read(outside.theme()).unwrap(), compiled);
    }
}
