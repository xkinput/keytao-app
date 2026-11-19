//! V3 source validation and compilation. Native readers continue consuming v2.
use crate::*;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use unicode_segmentation::UnicodeSegmentation;

pub const THEME_SOURCE_VERSION: u32 = 3;
pub const MAX_THEME_SOURCE_BYTES: usize = 64 * 1024;
pub const MAX_THEME_SOURCE_DEPTH: usize = 8;

/// Nonfatal diagnostics use source paths so editors can highlight each field.
#[derive(Clone, Debug, Default, Serialize)]
pub struct ImportReport {
    pub dropped: Vec<String>,
    pub clamped: Vec<String>,
    pub unknown: Vec<String>,
    pub warnings: Vec<String>,
}

/// Validated partial source. Unknown fields survive YAML storage and sharing;
/// invalid known fields never reach resolution. Construct through the parsers.
#[derive(Clone, Debug)]
pub struct ThemeSource {
    pub(crate) value: Value,
    pub(crate) valid: Value,
    pub report: ImportReport,
}

impl ThemeSource {
    pub fn id(&self) -> &str {
        self.valid["meta"]["id"].as_str().expect("validated id")
    }
    pub fn name(&self) -> &str {
        self.valid["meta"]["name"].as_str().expect("validated name")
    }
    /// Source partial, including unknown fields, with invalid fields removed.
    pub fn value(&self) -> &Value {
        &self.value
    }
    pub fn to_yaml(&self) -> Result<String, String> {
        serde_yaml::to_string(&self.value).map_err(|e| e.to_string())
    }
}

/// Local files only. Preserve legacy text verbatim across migration and edits;
/// pasted JSON/share imports normalize families and enforce text length limits.
pub fn parse_theme_source_yaml(text: &str) -> Result<ThemeSource, String> {
    if text.len() > MAX_THEME_SOURCE_BYTES {
        return Err("theme source exceeds 64 KiB".into());
    }
    let value: Value = serde_yaml::from_str(text).map_err(|e| e.to_string())?;
    parse_local_theme_source(value)
}

pub fn parse_theme_source_json(text: &str) -> Result<ThemeSource, String> {
    if text.len() > MAX_THEME_SOURCE_BYTES {
        return Err("theme source exceeds 64 KiB".into());
    }
    parse_value(
        serde_json::from_str(text).map_err(|e| e.to_string())?,
        false,
    )
}

pub(crate) fn parse_local_theme_source(value: Value) -> Result<ThemeSource, String> {
    parse_value(value, true)
}

/// Validate an id before using it as a filename on any supported platform.
pub fn validate_theme_id(id: &str) -> Result<(), String> {
    let valid = (3..=64).contains(&id.len())
        && id.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && id.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
        });
    let reserved = id.split('.').any(|p| {
        matches!(p, "con" | "prn" | "aux" | "nul")
            || ((p.starts_with("com") || p.starts_with("lpt"))
                && p.len() == 4
                && matches!(p.as_bytes()[3], b'1'..=b'9'))
    });
    if valid && !reserved {
        Ok(())
    } else {
        Err("invalid or reserved theme id".into())
    }
}

fn depth(value: &Value) -> usize {
    match value {
        Value::Object(v) => 1 + v.values().map(depth).max().unwrap_or(0),
        Value::Array(v) => 1 + v.iter().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

fn parse_value(mut value: Value, preserve_local_text: bool) -> Result<ThemeSource, String> {
    if !value.is_object() {
        return Err("theme source must be an object".into());
    }
    if depth(&value) > MAX_THEME_SOURCE_DEPTH {
        return Err("theme source nesting exceeds 8".into());
    }
    if crate::share::canonical_json(&value)?.len() > MAX_THEME_SOURCE_BYTES {
        return Err("theme source exceeds 64 KiB".into());
    }
    match value.get("version").and_then(Value::as_u64) {
        Some(3) => (),
        Some(v) if v > 3 => return Err("theme requires a newer App".into()),
        _ => return Err("theme source version must be 3".into()),
    }
    if !value.get("meta").is_some_and(Value::is_object) {
        return Err("theme meta is required".into());
    }
    validate_theme_id(value["meta"]["id"].as_str().ok_or("theme id is required")?)?;
    if !value["meta"]["name"].is_string() {
        return Err("theme name is required".into());
    }
    normalize_aliases(&mut value);
    let mut report = ImportReport::default();
    let schema = source_schema(preserve_local_text);
    let valid = validate_object(&mut value, &schema, "", &mut report);
    if valid["meta"]["name"].as_str().is_none_or(str::is_empty) {
        return Err("theme name is empty".into());
    }
    let mut source = ThemeSource {
        value,
        valid,
        report,
    };
    for scheme in [EffectiveColorScheme::Light, EffectiveColorScheme::Dark] {
        let t = resolve_theme_source(&source, scheme);
        if contrast_ratio(t.panel.background, t.candidate.foreground) < 3.0 {
            source.report.warnings.push(format!(
                "{scheme:?}.pc.candidate.foreground contrast below 3:1"
            ));
        }
        let key = &t.mobile["keyboard"]["key"];
        if let (Some(bg), Some(fg)) = (
            resolved_color(&key["background"]),
            resolved_color(&key["foreground"]),
        ) {
            if bg.alpha > 0 && contrast_ratio(bg, fg) < 3.0 {
                source
                    .report
                    .warnings
                    .push(format!("{scheme:?}.mobile.keyboard.key contrast below 3:1"));
            }
        }
    }
    Ok(source)
}

fn resolved_color(v: &Value) -> Option<RgbaColor> {
    Some(rgba(
        v["red"].as_u64()? as u8,
        v["green"].as_u64()? as u8,
        v["blue"].as_u64()? as u8,
        v["alpha"].as_u64()? as u8,
    ))
}

fn normalize_aliases(v: &mut Value) {
    if let Some(obj) = v.as_object_mut() {
        if let Some(old) = obj.remove("night") {
            obj.entry("dark").or_insert(old);
        }
        if let Some(pc) = obj.get_mut("pc").and_then(Value::as_object_mut) {
            if let Some(old) = pc.remove("mode_hint") {
                pc.entry("modeHint").or_insert(old);
            }
        }
        for key in ["light", "dark"] {
            if let Some(value) = obj.get_mut(key) {
                normalize_aliases(value);
            }
        }
    }
}

fn source_schema(preserve_local_text: bool) -> Value {
    let pc = resolved_to_flat(&ResolvedImeTheme::schema_base(UiTheme::default()));
    let common = json!({"accentColor":"color", "backgroundColor":"color", "cornerRadius":0, "font":{"family":if preserve_local_text { "localFamily" } else { "family" }, "weight":"weight"}});
    let mut pc_schema = pc;
    pc_schema["font"].as_object_mut().unwrap().remove("family");
    pc_schema["font"].as_object_mut().unwrap().remove("weight");
    // Convert serialized PC leaf values to validation descriptors.
    fn describe(v: &mut Value, key: &str, preserve_local_text: bool) {
        match v {
            Value::Object(obj) => {
                for (k, value) in obj {
                    describe(value, k, preserve_local_text);
                }
            }
            Value::String(_) => {
                *v = Value::String(
                    match key {
                        "orientation" => "orientation",
                        "labelSuffix" | "chineseText" | "englishText" => {
                            if preserve_local_text {
                                "localText"
                            } else {
                                "text4"
                            }
                        }
                        _ => "color",
                    }
                    .into(),
                )
            }
            _ => (),
        }
    }
    describe(&mut pc_schema, "", preserve_local_text);
    let style = json!({"background":"color","foreground":"color","hintForeground":"color","pressedBackground":"color","pressedForeground":"color","borderColor":"color","borderWidth":0,"cornerRadius":0,"shadowColor":"color","shadowOffsetY":0,"fontScale":1,"hintFontScale":1,"fontWeight":"weight"});
    let mut key = style.clone();
    key["swipeHintForeground"] = json!("color");
    key["swipeHintVisible"] = json!(true);
    let mut colors = style.clone();
    colors.as_object_mut().unwrap().retain(|_, v| v == "color");
    let mobile = json!({"keyboard": {"background":"color","pageBackground":"color","key":key,
        "keyTypes":{"character":style,"function":style,"space":style,"enter":style,"backspace":style,"shift":style},
        "keyClasses":{"accent":style,"accentPunctuation":style},"keyStates":{"selected":colors,"shiftOn":colors},
        "toolbar":{"background":"color","foreground":"color","selectedForeground":"color"},"floating":{"borderColor":"color","shadowColor":"color"}},
        "candidateBar":{"background":"color","foreground":"color","selectedForeground":"color","selectedBackground":"color","commentColor":"color","fontScale":1,"showComment":true,"preeditForeground":"color","preeditBackground":"color","expandedBackground":"color"}});
    let variant = json!({"common":common,"pc":pc_schema,"mobile":mobile});
    json!({"version":3,"meta":{"id":"id","name":"text64","author":"text64","appearance":"appearance"},"common":common,"pc":pc_schema,"mobile":mobile,"light":variant,"dark":variant})
}

fn validate_object(
    value: &mut Value,
    schema: &Value,
    path: &str,
    report: &mut ImportReport,
) -> Value {
    let mut valid = Map::new();
    let obj = value.as_object_mut().expect("object checked");
    let keys: Vec<_> = obj.keys().cloned().collect();
    for key in keys {
        let full = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        let Some(rule) = schema.get(&key) else {
            report.unknown.push(full);
            continue;
        };
        let input = obj.get_mut(&key).unwrap();
        let output = if rule.is_object() {
            input
                .is_object()
                .then(|| validate_object(input, rule, &full, report))
        } else {
            validate_leaf(input, rule, &full, report)
        };
        match output {
            Some(v) => {
                if !rule.is_object() {
                    *input = v.clone();
                }
                valid.insert(key, v);
            }
            None => {
                report.dropped.push(full);
                obj.remove(&key);
            }
        }
    }
    Value::Object(valid)
}

fn validate_leaf(
    value: &Value,
    rule: &Value,
    path: &str,
    report: &mut ImportReport,
) -> Option<Value> {
    if rule.is_boolean() {
        return value.as_bool().map(Value::Bool);
    }
    if rule.is_number() {
        if path == "version" {
            return Some(json!(THEME_SOURCE_VERSION));
        }
        let number = value.as_f64()?;
        if !number.is_finite() {
            return None;
        }
        let original_path = path;
        let path = path
            .strip_prefix("light.")
            .or_else(|| path.strip_prefix("dark."))
            .unwrap_or(path);
        let result = if path == "version" {
            3.0
        } else if path == "common.cornerRadius" {
            f64::from(normalize_corner_radius(number.clamp(0.0, 28.0) as f32).ok()?)
        } else if path.starts_with("mobile.") {
            crate::mobile_theme::sanitize_mobile_number(path.rsplit('.').next()?, number)?
        } else {
            sanitize_pc_number(path.strip_prefix("pc.")?, number)?
        };
        if result != number {
            report.clamped.push(original_path.into());
        }
        return Some(json!(result));
    }
    let text = value.as_str();
    match rule.as_str()? {
        "color" => {
            let text = text?;
            let expanded = if text.starts_with('#')
                && text.len() == 4
                && text[1..].bytes().all(|b| b.is_ascii_hexdigit())
            {
                format!(
                    "#{}",
                    text[1..].chars().flat_map(|c| [c, c]).collect::<String>()
                )
            } else {
                text.to_owned()
            };
            if !(expanded.eq_ignore_ascii_case("transparent")
                || (expanded.starts_with('#') && matches!(expanded.len(), 7 | 9)))
            {
                return None;
            }
            parse_color(&expanded).map(|c| {
                // Keep omitted background alpha distinct from an explicit FF.
                if path.ends_with("common.backgroundColor") && expanded.len() == 7 {
                    json!(expanded.to_ascii_uppercase())
                } else {
                    json!(color_hex(c))
                }
            })
        }
        "weight" => serde_json::from_value::<FontWeight>(value.clone())
            .ok()
            .map(|_| value.clone()),
        "orientation" => serde_json::from_value::<PanelOrientation>(value.clone())
            .ok()
            .map(|_| value.clone()),
        "appearance" => text
            .filter(|s| matches!(*s, "both" | "light" | "dark"))
            .map(|s| json!(s)),
        "localText" => text.map(|s| json!(s)),
        "family" | "localFamily" => {
            if rule == "localFamily" && text.is_some() {
                return text.map(|s| json!(s));
            }
            let joined = if let Some(text) = text {
                text.to_owned()
            } else {
                value
                    .as_array()?
                    .iter()
                    .map(|v| v.as_str())
                    .collect::<Option<Vec<_>>>()?
                    .join(", ")
            };
            let families: Vec<_> = joined
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();
            if families.len() > 8 || families.iter().any(|s| s.chars().count() > 64) {
                return None;
            }
            Some(json!(families.join(", ")))
        }
        "id" => text
            .filter(|s| validate_theme_id(s).is_ok())
            .map(|s| json!(s)),
        "text64" | "text4" => {
            let raw = text?;
            let clean: String = raw
                .chars()
                .filter(|c| {
                    !c.is_control() && !matches!(*c,'\u{202A}'..='\u{202E}'|'\u{2066}'..='\u{2069}')
                })
                .collect();
            let max = if rule == "text64" { 64 } else { 4 };
            let result = clean.graphemes(true).take(max).collect::<String>();
            if result != raw {
                report.clamped.push(path.into());
            }
            Some(json!(result))
        }
        _ => None,
    }
}

fn sanitize_pc_number(path: &str, number: f64) -> Option<f64> {
    let (section, field) = path.split_once('.')?;
    let number = number.clamp(-f64::from(f32::MAX), f64::from(f32::MAX));
    let partial: PartialTheme = serde_json::from_value(json!({section:{field:number}})).ok()?;
    let mut theme = ResolvedImeTheme::schema_base(UiTheme::default());
    theme.apply(partial);
    resolved_to_flat(&theme.sanitized())
        .get(section)?
        .get(field)?
        .as_f64()
}

pub(crate) fn color_hex(c: RgbaColor) -> String {
    format!("#{:02X}{:02X}{:02X}{:02X}", c.red, c.green, c.blue, c.alpha)
}

/// Flat v2 appearance fields, with colors encoded as YAML-compatible strings.
pub(crate) fn resolved_to_flat(theme: &ResolvedImeTheme) -> Value {
    let mut value = serde_json::to_value(theme).expect("resolved theme serializes");
    value
        .as_object_mut()
        .unwrap()
        .retain(|k, _| !matches!(k.as_str(), "version" | "ui" | "mobile"));
    encode_colors(&mut value);
    if value["font"]["family"].is_null() {
        value["font"]["family"] = json!("");
    }
    value
}

pub(crate) fn encode_colors(value: &mut Value) {
    if let Some(c) = resolved_color(value) {
        *value = json!(color_hex(c));
        return;
    }
    match value {
        Value::Object(obj) => {
            for v in obj.values_mut() {
                encode_colors(v);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                encode_colors(v);
            }
        }
        _ => (),
    }
}

pub(crate) fn merge(base: &mut Value, patch: &Value) {
    if let (Some(base), Some(patch)) = (base.as_object_mut(), patch.as_object()) {
        for (key, val) in patch {
            if let Some(old) = base.get_mut(key) {
                merge(old, val);
            } else {
                base.insert(key.clone(), val.clone());
            }
        }
    } else {
        *base = patch.clone();
    }
}

fn variant(source: &ThemeSource, scheme: EffectiveColorScheme) -> Value {
    let mut root = source.valid.clone();
    if let Some(patch) = source.valid.get(if scheme == EffectiveColorScheme::Dark {
        "dark"
    } else {
        "light"
    }) {
        merge(&mut root, patch);
    }
    root
}

/// Resolve a source for an effective system appearance. Local UI flags are
/// supplied separately to `compile_theme`, never imported from source files.
pub fn resolve_theme_source(
    source: &ThemeSource,
    scheme: EffectiveColorScheme,
) -> ResolvedImeTheme {
    let appearance = source
        .valid
        .pointer("/meta/appearance")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            fn has_color(v: &Value) -> bool {
                match v {
                    Value::String(s) => s.starts_with('#'),
                    Value::Object(o) => o.values().any(has_color),
                    _ => false,
                }
            }
            if source.valid.get("dark").is_some()
                || source.valid.get("light").is_some()
                || !["common", "pc", "mobile"]
                    .iter()
                    .any(|k| source.valid.get(*k).is_some_and(has_color))
            {
                "both".into()
            } else {
                let v = variant(source, EffectiveColorScheme::Light);
                let bg = v
                    .pointer("/pc/panel/background")
                    .or_else(|| v.pointer("/common/backgroundColor"))
                    .and_then(Value::as_str)
                    .and_then(parse_color)
                    .unwrap_or_else(|| {
                        resolve_legacy_theme_yaml("", EffectiveColorScheme::Light)
                            .panel
                            .background
                    });
                if relative_luminance(bg) < 0.18 {
                    "dark".into()
                } else {
                    "light".into()
                }
            }
        });
    let effective = match appearance.as_str() {
        "light" => EffectiveColorScheme::Light,
        "dark" => EffectiveColorScheme::Dark,
        _ => scheme,
    };
    let v = variant(source, effective);
    let mut theme = resolve_legacy_theme_yaml("", effective);
    theme.ui.effective_color_scheme = effective;
    let common = &v["common"];
    theme.ui.accent_color = common["accentColor"].as_str().and_then(parse_color);
    theme.ui.corner_radius = common["cornerRadius"].as_f64().map(|v| v as f32);
    theme.ui.background_color = common["backgroundColor"].as_str().and_then(parse_color);
    if let Some(mut bg) = theme.ui.background_color {
        if common["backgroundColor"]
            .as_str()
            .is_some_and(|s| s.len() == 7)
        {
            bg.alpha = theme.panel.background.alpha;
        }
        let bg = v
            .pointer("/pc/panel/background")
            .and_then(Value::as_str)
            .and_then(parse_color)
            .unwrap_or(bg);
        if v.pointer("/pc/candidate/foreground").is_none() {
            let alternate_scheme = if effective == EffectiveColorScheme::Light {
                EffectiveColorScheme::Dark
            } else {
                EffectiveColorScheme::Light
            };
            let alternate = resolve_legacy_theme_yaml("", alternate_scheme);
            if contrast_ratio(bg, alternate.candidate.foreground)
                > contrast_ratio(bg, theme.candidate.foreground)
            {
                let alt = resolved_to_flat(&alternate);
                let mut flat = resolved_to_flat(&theme);
                for section in ["panel", "candidate", "navigation", "modeHint"] {
                    for (key, val) in alt[section].as_object().unwrap() {
                        if val.as_str().is_some_and(|s| s.starts_with('#'))
                            && !(section == "panel" && key == "background")
                        {
                            flat[section][key] = val.clone();
                        }
                    }
                }
                theme.apply(serde_json::from_value(flat).expect("flat theme"));
            }
        }
        theme.panel.background = bg;
    }
    if let Some(radius) = theme.ui.corner_radius {
        theme.panel.corner_radius = radius;
        theme.candidate.corner_radius = radius * 11.0 / 16.0;
        theme.navigation.corner_radius = radius * 10.0 / 16.0;
        theme.mode_hint.corner_radius = radius * 14.0 / 16.0;
    }
    if let Some(bg) = v
        .pointer("/pc/panel/background")
        .and_then(Value::as_str)
        .and_then(parse_color)
    {
        theme.panel.background = bg;
    }
    if let Some(accent) = theme.ui.accent_color {
        theme.apply_accent_color(accent);
    }
    // Explicit fields always win over derived defaults.
    if let Some(pc) = v.get("pc") {
        theme.apply(serde_json::from_value(pc.clone()).expect("validated pc partial"));
    }
    if let Some(font) = common.get("font") {
        theme
            .font
            .apply(serde_json::from_value(font.clone()).expect("validated common font"));
    }
    let mut theme = theme.sanitized();
    theme.mobile = crate::mobile_theme::resolve_mobile_theme(
        &theme,
        &v["mobile"],
        effective == EffectiveColorScheme::Dark,
    );
    theme
}

/// Compile a validated partial to explicit v2 light/dark fields. Only local UI
/// settings and unrelated legacy extension keys are retained from the base.
/// `resolvedUi` is compatibility metadata; it never participates in derivation.
pub fn compile_theme(source: &ThemeSource, base_theme_yaml: &str) -> Result<String, String> {
    let base: Value = if base_theme_yaml.trim().is_empty() {
        json!({})
    } else {
        serde_yaml::from_str(base_theme_yaml).map_err(|e| e.to_string())?
    };
    if !base.is_object() {
        return Err("base theme must be a mapping".into());
    }
    let local = resolve_legacy_theme_yaml(base_theme_yaml, EffectiveColorScheme::Light).ui;
    let mut light = resolve_theme_source(source, EffectiveColorScheme::Light);
    let mut dark = resolve_theme_source(source, EffectiveColorScheme::Dark);
    for theme in [&mut light, &mut dark] {
        theme.ui.color_scheme = local.color_scheme;
        theme.ui.embedded_composition = local.embedded_composition;
        theme.ui.hide_candidate_panel = local.hide_candidate_panel;
        theme.ui.show_tray_mode_indicator = local.show_tray_mode_indicator;
    }
    let mut root = base;
    root.as_object_mut().unwrap().retain(|key, _| {
        !matches!(
            key.as_str(),
            "version"
                | "font"
                | "panel"
                | "candidate"
                | "navigation"
                | "modeHint"
                | "mode_hint"
                | "light"
                | "dark"
                | "night"
                | "ui"
                | "mobile"
                | "resolvedUi"
                | "themeId"
                | "themeHash"
                | "compiledHash"
                | "common"
                | "pc"
                | "meta"
                | "keyboard"
        )
    });
    merge(&mut root, &resolved_to_flat(&light));
    let mut dark_flat = resolved_to_flat(&dark);
    dark_flat["mobile"] = dark.mobile.clone();
    root["version"] = json!(THEME_SCHEMA_VERSION);
    root["dark"] = dark_flat;
    root["mobile"] = light.mobile.clone();
    root["themeId"] = json!(source.id());
    root["themeHash"] = json!(&crate::share::theme_source_hash(source)?[..16]);
    root["ui"] = json!({"colorScheme":local.color_scheme,"embeddedComposition":local.embedded_composition,"hideCandidatePanel":local.hide_candidate_panel,"showTrayModeIndicator":local.show_tray_mode_indicator});
    let source_ui = |ui: &UiTheme| json!({"effectiveColorScheme":ui.effective_color_scheme,"accentColor":ui.accent_color,"cornerRadius":ui.corner_radius,"backgroundColor":ui.background_color});
    let mut ui = json!({"light":source_ui(&light.ui),"dark":source_ui(&dark.ui)});
    encode_colors(&mut ui);
    root["resolvedUi"] = ui;
    // The source hash cannot detect edits to compiled fields or a defaults upgrade.
    root["compiledHash"] = json!(compiled_appearance_hash(&root)?);
    serde_yaml::to_string(&root).map_err(|e| e.to_string())
}

pub(crate) fn compiled_appearance_hash(root: &Value) -> Result<String, String> {
    let mut appearance = Map::new();
    for key in [
        "font",
        "panel",
        "candidate",
        "navigation",
        "modeHint",
        "light",
        "dark",
        "mobile",
        "resolvedUi",
    ] {
        if let Some(value) = root.get(key) {
            appearance.insert(key.into(), value.clone());
        }
    }
    Ok(format!(
        "{:x}",
        Sha256::digest(crate::share::canonical_json(&Value::Object(appearance))?.as_bytes())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(extra: Value) -> ThemeSource {
        let mut v =
            json!({"version":3,"meta":{"id":"my.sample","name":"Sample","appearance":"both"}});
        merge(&mut v, &extra);
        parse_theme_source_json(&v.to_string()).unwrap()
    }
    #[test]
    fn fields_validate_independently_and_unknowns_survive_storage() {
        let s = source(
            json!({"pc":{"panel":{"background":"bad","borderColor":"#abc","paddingX":99,"future":{"value":true}},"candidate":{"foreground":"#12345678"}},"future":42}),
        );
        assert!(s.valid.pointer("/pc/panel/background").is_none());
        assert_eq!(s.valid["pc"]["panel"]["borderColor"], "#AABBCCFF");
        assert_eq!(s.valid["pc"]["panel"]["paddingX"], 32.0);
        assert_eq!(s.value["pc"]["panel"]["future"]["value"], true);
        assert!(s.report.unknown.contains(&"pc.panel.future".into()));
        assert!(s.report.dropped.contains(&"pc.panel.background".into()));
        let again = parse_theme_source_yaml(&s.to_yaml().unwrap()).unwrap();
        assert_eq!(s.value, again.value);
    }
    #[test]
    fn imports_limit_text_while_local_migration_text_survives_reloads() {
        let value = json!({"version":3,"meta":{"id":"my.legacy","name":"Legacy"},"common":{"font":{"family":" A ,B,  C D "}},"pc":{"candidate":{"labelSuffix":"12345678"},"modeHint":{"chineseText":"12345678","englishText":"ABCDEFGH"}}});
        let imported = parse_theme_source_json(&value.to_string()).unwrap();
        assert_eq!(imported.value["common"]["font"]["family"], "A, B, C D");
        assert_eq!(imported.value["pc"]["candidate"]["labelSuffix"], "1234");
        assert_eq!(imported.value["pc"]["modeHint"]["chineseText"], "1234");
        assert_eq!(imported.value["pc"]["modeHint"]["englishText"], "ABCD");
        let local = parse_local_theme_source(value.clone()).unwrap();
        let reloaded = parse_theme_source_yaml(&local.to_yaml().unwrap()).unwrap();
        assert_eq!(reloaded.value, value);
    }
    #[test]
    fn versions_ids_text_and_caps() {
        for id in [
            "con.foo", "abc.nul", "my.com1", "my.lpt9", "../bad", "UPPER", "a", "a..b", "abc/def",
        ] {
            assert!(validate_theme_id(id).is_err(), "{id}");
        }
        for id in ["my.current", "abc", "a-b._foo", "com0.test"] {
            assert!(validate_theme_id(id).is_ok(), "{id}");
        }
        for version in [json!(4), json!(2), Value::Null] {
            assert!(parse_theme_source_json(
                &json!({"version":version,"meta":{"id":"my.sample","name":"Sample"}}).to_string()
            )
            .is_err());
        }
        let s = source(json!({"meta":{"name":"a\u{202E}b\n\u{2069}c","author":"x".repeat(90)}}));
        assert_eq!(s.name(), "abc");
        assert_eq!(s.value["meta"]["author"].as_str().unwrap().len(), 64);
        assert!(parse_theme_source_json(&format!(
            "{{\"version\":3,\"meta\":{{\"id\":\"abc\",\"name\":\"{}\"}}}}",
            "x".repeat(65536)
        ))
        .is_err());
        let mut deep = json!(true);
        for _ in 0..10 {
            deep = json!({"a":deep});
        }
        assert!(parse_theme_source_json(&deep.to_string()).is_err());
        let s = source(
            json!({"common":{"cornerRadius":1e40},"dark":{"pc":{"font":{"size":1e40},"panel":{"gap":-1e40}}}}),
        );
        assert_eq!(s.valid["common"]["cornerRadius"], 28.0);
        assert_eq!(s.valid["dark"]["pc"]["font"]["size"], 36.0);
        assert_eq!(s.valid["dark"]["pc"]["panel"]["gap"], 0.0);
        assert!(s.report.clamped.contains(&"dark.pc.font.size".into()));
    }
    #[test]
    fn explicit_values_win_and_alpha_is_preserved() {
        let s = source(
            json!({"common":{"accentColor":"#AA332244","cornerRadius":20,"backgroundColor":"#10101080","font":{"family":["PingFang SC","Noto Sans"],"weight":"heavy"}},"pc":{"candidate":{"selectedBackground":"#12345678","cornerRadius":3}},"mobile":{"keyboard":{"pageBackground":"#10203040","key":{"background":"#99887766","fontScale":9}}}}),
        );
        let t = resolve_theme_source(&s, EffectiveColorScheme::Light);
        assert_eq!(t.candidate.selected_background.alpha, 0x78);
        assert_eq!(t.panel.background.alpha, 0x80);
        assert_eq!(t.candidate.corner_radius, 3.0);
        assert_eq!(t.font.family.as_deref(), Some("PingFang SC, Noto Sans"));
        assert_eq!(t.mobile["keyboard"]["key"]["background"]["alpha"], 0x66);
        assert_eq!(t.mobile["keyboard"]["pageBackground"]["alpha"], 0x40);
        assert_eq!(t.mobile["keyboard"]["key"]["fontScale"], 1.3);
        assert!(relative_luminance(t.candidate.foreground) > 0.5);
        let s = source(
            json!({"common":{"backgroundColor":"#000"},"pc":{"panel":{"background":"#FFFFFF"}}}),
        );
        for scheme in [EffectiveColorScheme::Light, EffectiveColorScheme::Dark] {
            let t = resolve_theme_source(&s, scheme);
            assert!(contrast_ratio(t.panel.background, t.candidate.foreground) >= 3.0);
        }
    }
    #[test]
    fn common_background_defaults_alpha_unless_explicit() {
        for (patch, expected) in [
            (json!({"common":{"backgroundColor":"#654321"}}), "#654321F2"),
            (json!({"common":{"backgroundColor":"#632"}}), "#663322F2"),
            (
                json!({"common":{"backgroundColor":"#654321FF"}}),
                "#654321FF",
            ),
            (
                json!({"common":{"backgroundColor":"#65432180"}}),
                "#65432180",
            ),
            (
                json!({"common":{"backgroundColor":"transparent"}}),
                "#00000000",
            ),
            (
                json!({"light":{"common":{"backgroundColor":"#654321"}},"dark":{"common":{"backgroundColor":"#654321"}}}),
                "#654321F2",
            ),
            (
                json!({"common":{"backgroundColor":"#654321"},"pc":{"panel":{"background":"#112233"}}}),
                "#112233FF",
            ),
            (
                json!({"common":{"backgroundColor":"#654321"},"pc":{"panel":{"background":"#11223340"}}}),
                "#11223340",
            ),
        ] {
            let source = source(patch);
            let source = parse_theme_source_yaml(&source.to_yaml().unwrap()).unwrap();
            let compiled = compile_theme(&source, "").unwrap();
            for scheme in [EffectiveColorScheme::Light, EffectiveColorScheme::Dark] {
                for theme in [
                    resolve_theme_source(&source, scheme),
                    crate::resolve_compiled_theme_yaml(&compiled, scheme),
                ] {
                    assert_eq!(theme.panel.background, parse_color(expected).unwrap());
                }
            }
        }
    }
    #[test]
    fn variants_appearance_and_aliases() {
        let s = source(
            json!({"meta":{"appearance":"light"},"common":{"accentColor":"#abc"},"night":{"pc":{"panel":{"background":"#000"}}},"pc":{"mode_hint":{"duration":99}}}),
        );
        let t = resolve_theme_source(&s, EffectiveColorScheme::Dark);
        assert_eq!(t.ui.effective_color_scheme, EffectiveColorScheme::Light);
        assert_eq!(t.mobile["isDark"], false);
        assert_eq!(t.mode_hint.duration, 4.0);
        let s=parse_theme_source_json(r##"{"version":3,"meta":{"id":"my.infer","name":"Dark"},"pc":{"panel":{"background":"#000"}}}"##).unwrap();
        assert_eq!(
            resolve_theme_source(&s, EffectiveColorScheme::Light).mobile["isDark"],
            true
        );
    }
    #[test]
    fn compiled_v2_preserves_native_contract_and_local_flags() {
        let s = source(
            json!({"common":{"font":{"family":"A, B","weight":"light"}},"dark":{"pc":{"panel":{"maxWidth":999}}}}),
        );
        let compiled=compile_theme(&s,"ui:\n  colorScheme: dark\n  hideCandidatePanel: true\n  showTrayModeIndicator: false\n").unwrap();
        let v: Value = serde_yaml::from_str(&compiled).unwrap();
        assert_eq!(v["version"], 2);
        assert!(v["ui"].get("accentColor").is_none());
        assert_eq!(v["dark"]["panel"]["maxWidth"], 999.0);
        assert_eq!(v["themeHash"].as_str().unwrap().len(), 16);
        let partial: PartialTheme = serde_yaml::from_str(&compiled).unwrap();
        let t = resolve_legacy_partials(
            vec![serde_yaml::from_str(DEFAULT_THEME_YAML).unwrap(), partial],
            EffectiveColorScheme::Dark,
        );
        let v = serde_json::to_value(t).unwrap();
        assert_eq!(v["font"]["family"], "A, B");
        assert!(v["ui"]["effectiveEmbeddedComposition"].as_bool().unwrap());
        assert!(v["panel"]["maxWidth"].is_number());
        assert!(v["candidate"]["maxWidth"].is_number());
    }
}
