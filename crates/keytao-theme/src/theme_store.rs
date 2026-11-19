//! Source-file storage and lossless migration for App and later bridge callers.
//! All paths are explicit: these APIs never select or access a real IME directory.

use crate::{
    source::{
        compile_theme, parse_local_theme_source, parse_theme_source_yaml, resolve_theme_source,
        resolved_to_flat, ThemeSource,
    },
    EffectiveColorScheme, UiColorScheme,
};
use serde_json::{json, Map, Value};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

/// The first v2 migration always uses this editable source identity.
pub const CURRENT_THEME_ID: &str = "my.current";

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
        let old = crate::resolve_compiled_theme_yaml(yaml, scheme);
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
    if compiled {
        for (variant, scheme) in [
            ("light", EffectiveColorScheme::Light),
            ("dark", EffectiveColorScheme::Dark),
        ] {
            let old = crate::resolve_compiled_theme_yaml(yaml, scheme);
            let new = resolve_theme_source(&source, scheme);
            let mut mobile = difference(&old.mobile, &new.mobile);
            crate::source::encode_colors(&mut mobile);
            source.value[variant]["mobile"] = mobile;
        }
        source = parse_source_value(source.value)?;
    }
    Ok(source)
}

/// Return the current source's safe path, or `None` for an unmigrated v2 file.
pub fn current_theme_source_path(theme_path: &Path) -> Result<Option<PathBuf>, String> {
    let base = read_base(theme_path)?;
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
    let yaml = fs::read_to_string(&path)
        .map_err(|e| format!("Read theme source {}: {e}", path.display()))?;
    parse_theme_source_yaml(&yaml).map(Some)
}

/// Perform or resume the one-time migration, replacing an unreferenced orphan.
/// Rebuild from the compiled appearance when the active source was removed.
/// Also move a legacy keyboard block only when `keyboard.yaml` does not exist.
pub fn migrate_current_theme_once(theme_path: &Path) -> Result<bool, String> {
    let base = read_base(theme_path)?;
    if let Some(path) = current_theme_source_path(theme_path)? {
        if path
            .try_exists()
            .map_err(|e| format!("Read theme source {}: {e}", path.display()))?
        {
            return Ok(false);
        }
    }
    let yaml = value_yaml(&base)?;
    let source = migrate_v2_theme(&yaml, CURRENT_THEME_ID, "Current theme")?;
    save_source_and_compile(theme_path, &source, &base)?;
    Ok(true)
}

/// Refresh an installed source at App startup without overwriting manual edits.
/// A conflicting compiled file is left intact for `resolve_manual_theme_edits`.
pub fn refresh_current_theme(theme_path: &Path) -> Result<bool, String> {
    if migrate_current_theme_once(theme_path)? {
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
    migrate_current_theme_once(theme_path)?;
    let source = read_current_theme_source(theme_path)?.ok_or("Missing current theme source")?;
    let base = read_base(theme_path)?;
    let yaml = value_yaml(&base)?;
    if !compiled_theme_has_manual_edits(&source, &yaml)? {
        return Ok(source_path(theme_path, source.id()));
    }
    let next = match choice {
        ManualThemeEditChoice::Discard => source,
        ManualThemeEditChoice::SaveAsNew => {
            let id = unused_copy_id(theme_path, CURRENT_THEME_ID);
            match preserve_manual_changes(source.clone(), &base, &id) {
                Ok(copy) => copy,
                Err(error) => {
                    let backup = backup_manual_theme(theme_path)?;
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
        let mut name = theme_path.file_name().ok_or("Missing theme filename")?.to_os_string();
        name.push(format!(".manual-{timestamp}.bak"));
        let backup = theme_path.with_file_name(name);
        match fs::OpenOptions::new().write(true).create_new(true).open(&backup) {
            Ok(mut file) => {
                file.write_all(&content).and_then(|()| file.sync_all())
                    .map_err(|e| format!("Back up manual theme {}: {e}", backup.display()))?;
                return Ok(backup);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                timestamp = timestamp.checked_add(1).ok_or("Theme backup timestamp overflow")?;
            }
            Err(error) => return Err(format!("Back up manual theme {}: {error}", backup.display())),
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
    for (variant, scheme) in [("light", EffectiveColorScheme::Light), ("dark", EffectiveColorScheme::Dark)] {
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
    resolve_manual_theme_edits(theme_path, ManualThemeEditChoice::SaveAsNew)?;
    let mut source =
        read_current_theme_source(theme_path)?.ok_or("Missing current theme source")?;
    if source.id().starts_with("keytao.") {
        let id = unused_copy_id(theme_path, &format!("my.{}", &source.id()[7..]));
        source.value["meta"]["id"] = Value::String(id);
    }
    source = edit_source_settings(
        source,
        accent_color,
        corner_radius,
        background_color,
        orientation,
        font_size,
    )?;
    let mut base = read_base(theme_path)?;
    if !base["ui"].is_object() {
        base["ui"] = json!({});
    }
    base["ui"]["colorScheme"] = serde_json::to_value(color_scheme).map_err(|e| e.to_string())?;
    save_source_and_compile(theme_path, &source, &base)
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

fn unused_copy_id(theme_path: &Path, id: &str) -> String {
    for suffix in 2u64.. {
        let suffix = format!("-{suffix}");
        let copy = format!("{}{}", &id[..id.len().min(64 - suffix.len())], suffix);
        if !source_path(theme_path, &copy).exists() {
            return copy;
        }
    }
    unreachable!()
}

fn read_base(path: &Path) -> Result<Value, String> {
    match fs::read_to_string(path) {
        Ok(yaml) => yaml_object(&yaml),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(format!("Read theme {}: {e}", path.display())),
    }
}

fn yaml_object(yaml: &str) -> Result<Value, String> {
    let value: Value =
        serde_yaml::from_str(yaml).map_err(|e| format!("Invalid theme YAML: {e}"))?;
    if value.is_null() {
        return Ok(json!({}));
    }
    if !value.is_object() {
        return Err("Theme document must be an object".into());
    }
    Ok(value)
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
    let source_path = source_path(theme_path, source.id());
    if let Some(parent) = source_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let source_yaml = serde_yaml::to_value(&source.value).map_err(|e| e.to_string())?;
    crate::write_theme_yaml_atomic(&source_path, &source_yaml)?;
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
    fn v2_migration_golden_corpus_is_byte_identical_in_both_schemes() {
        let corpus = [
            ("default", crate::DEFAULT_THEME_YAML),
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
            let backups: Vec<_> = fs::read_dir(&dir.0).unwrap().map(|entry| entry.unwrap().path())
                .filter(|path| path.file_name().unwrap().to_string_lossy().starts_with("theme.yaml.manual-"))
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
        let dir = TempDir::new();
        migrate_current_theme_once(&dir.theme()).unwrap();
        let mut base = read_base(&dir.theme()).unwrap();
        base["ui"] = json!([]);
        fs::write(dir.theme(), value_yaml(&base).unwrap()).unwrap();
        crate::write_theme_ui_to_path(&dir.theme(), UiColorScheme::Dark, None).unwrap();
        assert_eq!(
            read_base(&dir.theme()).unwrap()["ui"]["colorScheme"],
            "dark"
        );
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
            .into_iter().map(|scheme| crate::resolved_theme_json(
                &crate::resolve_compiled_theme_yaml(&expected_yaml, scheme)
            ).unwrap()).collect();
        for _ in 0..3 {
            crate::write_theme_ui_to_path(&dir.theme(), UiColorScheme::Auto, None).unwrap();
            let backups: Vec<_> = fs::read_dir(&dir.0).unwrap()
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
            for (index, scheme) in [EffectiveColorScheme::Light, EffectiveColorScheme::Dark].into_iter().enumerate() {
                let theme = crate::resolve_theme_from_paths_with_system_scheme(None, Some(&dir.theme()), scheme);
                assert_eq!(theme.candidate.label_suffix, ")");
                assert_eq!(crate::resolved_theme_json(&theme).unwrap(), expected[index]);
            }
        }
        let expected_id = if preserve_manual_edit { "my.current-2" } else { CURRENT_THEME_ID };
        assert_eq!(read_current_theme_source(&dir.theme()).unwrap().unwrap().id(), expected_id);
        assert_eq!(fs::read_to_string(dir.0.join("themes/my.current.yaml")).unwrap(), original_source);
    }

    #[test]
    fn manual_accent_is_backed_up_before_three_source_writebacks() {
        manual_appearance_writebacks("ui", "accentColor", json!("#FF0000"), false);
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
    fn manual_panel_color_is_backed_up_before_three_source_writebacks() {
        manual_appearance_writebacks("panel", "background", json!("#11223344"), false);
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
}
