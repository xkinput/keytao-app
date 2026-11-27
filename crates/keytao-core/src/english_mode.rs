//! English input preference and schema switching shared by desktop frontends.
//! `ascii_mode` remains librime's pass-through flag; English dictionary input
//! deliberately keeps that flag off so the engine can produce candidates.

use crate::{
    ImeState, KeyProcessResult, RIME_MOD_ALT, RIME_MOD_CONTROL, RIME_MOD_SHIFT, RIME_RELEASE_MASK,
    SchemaInfo, SchemaShortcut, SchemaSwitch,
};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, collections::BTreeMap, path::Path};

pub const SETTINGS_FILE: &str = "keytao-ime.json";
pub const SWITCH_KEY_SETTING: &str = "englishSwitchKey";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifierSwitchKey {
    Shift,
    ShiftL,
    ShiftR,
    Control,
    ControlL,
    ControlR,
    Alt,
    AltL,
    AltR,
}

impl ModifierSwitchKey {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shift => "Shift",
            Self::ShiftL => "Shift_L",
            Self::ShiftR => "Shift_R",
            Self::Control => "Control",
            Self::ControlL => "Control_L",
            Self::ControlR => "Control_R",
            Self::Alt => "Alt",
            Self::AltL => "Alt_L",
            Self::AltR => "Alt_R",
        }
    }

    pub fn matches(self, keycode: u32) -> bool {
        match self {
            Self::Shift => matches!(keycode, 0xffe1 | 0xffe2),
            Self::ShiftL => keycode == 0xffe1,
            Self::ShiftR => keycode == 0xffe2,
            Self::Control => matches!(keycode, 0xffe3 | 0xffe4),
            Self::ControlL => keycode == 0xffe3,
            Self::ControlR => keycode == 0xffe4,
            Self::Alt => matches!(keycode, 0xffe9 | 0xffea),
            Self::AltL => keycode == 0xffe9,
            Self::AltR => keycode == 0xffea,
        }
    }

    /// Control/Alt keysyms that platform frontends must forward on solo taps.
    pub fn modifier_taps(self) -> Vec<u32> {
        match self {
            Self::Control => vec![0xffe3, 0xffe4],
            Self::ControlL => vec![0xffe3],
            Self::ControlR => vec![0xffe4],
            Self::Alt => vec![0xffe9, 0xffea],
            Self::AltL => vec![0xffe9],
            Self::AltR => vec![0xffea],
            Self::Shift | Self::ShiftL | Self::ShiftR => Vec::new(),
        }
    }
}

/// Desktop Chinese/English switch key preference.
///
/// `Schema` preserves the active Rime schema's own `ascii_composer` /
/// `key_binder` configuration (and libjd's built-in Shift/CapsLock defaults).
/// Any other variant overrides modifier taps and/or adds a global toggle shortcut
/// across both librime and libjd sessions without requiring redeployment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum EnglishSwitchKey {
    #[default]
    Schema,
    ModifierTap(ModifierSwitchKey),
    CapsLock,
    Shortcut {
        spec: String,
        shortcut: SchemaShortcut,
    },
}

impl EnglishSwitchKey {
    pub fn parse(value: &str) -> Result<Self, String> {
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("schema") {
            return Ok(Self::Schema);
        }
        let preset = match trimmed.to_ascii_lowercase().as_str() {
            "shift" => Some(Self::ModifierTap(ModifierSwitchKey::Shift)),
            "shift_l" => Some(Self::ModifierTap(ModifierSwitchKey::ShiftL)),
            "shift_r" => Some(Self::ModifierTap(ModifierSwitchKey::ShiftR)),
            "control" | "ctrl" => Some(Self::ModifierTap(ModifierSwitchKey::Control)),
            "control_l" | "ctrl_l" => Some(Self::ModifierTap(ModifierSwitchKey::ControlL)),
            "control_r" | "ctrl_r" => Some(Self::ModifierTap(ModifierSwitchKey::ControlR)),
            "alt" | "option" => Some(Self::ModifierTap(ModifierSwitchKey::Alt)),
            "alt_l" | "option_l" => Some(Self::ModifierTap(ModifierSwitchKey::AltL)),
            "alt_r" | "option_r" => Some(Self::ModifierTap(ModifierSwitchKey::AltR)),
            "caps_lock" | "capslock" => Some(Self::CapsLock),
            _ => None,
        };
        if let Some(key) = preset {
            return Ok(key);
        }
        let parts: Vec<&str> = trimmed.split('+').map(str::trim).collect();
        if parts.is_empty() || parts.iter().any(|part| part.is_empty()) {
            return Err(format!("无效的切换中英键设置：{value}"));
        }
        let key_token = *parts.last().unwrap();
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        for &modifier in &parts[..parts.len() - 1] {
            match modifier.to_ascii_lowercase().as_str() {
                "control" | "ctrl" => ctrl = true,
                "alt" | "mod1" | "option" => alt = true,
                "shift" => shift = true,
                _ => return Err(format!("不支持的修饰键：{modifier}")),
            }
        }
        let mut canonical = String::new();
        if ctrl {
            canonical.push_str("Control+");
        }
        if alt {
            canonical.push_str("Alt+");
        }
        if shift {
            canonical.push_str("Shift+");
        }
        canonical.push_str(key_token);
        let shortcut = SchemaShortcut::parse(&canonical, "always")
            .ok_or_else(|| format!("无效的切换中英键设置：{value}"))?;
        let is_function_key = (0xffbe..=0xffe0).contains(&shortcut.key);
        let has_ctrl_or_alt = shortcut.modifiers & (RIME_MOD_CONTROL | RIME_MOD_ALT) != 0;
        let is_shift_safe = shortcut.modifiers == RIME_MOD_SHIFT
            && (shortcut.key == 0x20 || is_function_key);
        if !has_ctrl_or_alt && !is_shift_safe && !is_function_key {
            return Err("自定义快捷键需包含 Ctrl / Alt、Shift+空格 或功能键 F1-F12".into());
        }
        Ok(Self::Shortcut {
            spec: canonical,
            shortcut,
        })
    }

    pub fn from_setting(value: &str) -> Self {
        Self::parse(value).unwrap_or_default()
    }

    pub fn as_setting(&self) -> Cow<'_, str> {
        match self {
            Self::Schema => Cow::Borrowed("schema"),
            Self::ModifierTap(key) => Cow::Borrowed(key.as_str()),
            Self::CapsLock => Cow::Borrowed("Caps_Lock"),
            Self::Shortcut { spec, .. } => Cow::Borrowed(spec.as_str()),
        }
    }

    pub fn is_schema(&self) -> bool {
        matches!(self, Self::Schema)
    }

    pub fn modifier_taps(&self) -> Vec<u32> {
        match self {
            Self::ModifierTap(key) => key.modifier_taps(),
            Self::Schema | Self::CapsLock | Self::Shortcut { .. } => Vec::new(),
        }
    }

    pub fn shortcut(&self) -> Option<SchemaShortcut> {
        match self {
            Self::Shortcut { shortcut, .. } => Some(*shortcut),
            _ => None,
        }
    }

    pub fn matches_modifier_tap(&self, keycode: u32) -> bool {
        matches!(self, Self::ModifierTap(key) if key.matches(keycode))
    }

    pub fn matches_caps_lock(&self) -> bool {
        matches!(self, Self::CapsLock)
    }

    pub fn matches_shortcut(&self, keycode: u32, mask: u32) -> bool {
        mask & RIME_RELEASE_MASK == 0
            && matches!(self, Self::Shortcut { shortcut, .. } if shortcut.matches_key(keycode, mask))
    }

    pub fn apply_to_state(&self, state: &mut ImeState) {
        if self.is_schema() {
            return;
        }
        state.modifier_taps = self.modifier_taps();
        if let Some(shortcut) = self.shortcut() {
            if !state.schema_shortcuts.contains(&shortcut) {
                state.schema_shortcuts.push(shortcut);
            }
        }
    }
}

pub fn read_english_switch_key(root: &Path) -> EnglishSwitchKey {
    std::fs::read(root.join(SETTINGS_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| {
            value
                .get(SWITCH_KEY_SETTING)
                .and_then(|v| v.as_str())
                .map(EnglishSwitchKey::from_setting)
        })
        .unwrap_or_default()
}

/// Preserve unrelated settings, and reject malformed files instead of erasing
/// them. Removes the field when set back to `Schema`.
pub fn write_english_switch_key(root: &Path, key: &EnglishSwitchKey) -> Result<String, String> {
    let path = root.join(SETTINGS_FILE);
    let mut config = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes)
            .map_err(|e| format!("read English switch key settings: {e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(e) => return Err(format!("read English switch key settings: {e}")),
    };
    let object = config
        .as_object_mut()
        .ok_or("English input settings must be an object")?;
    let setting = key.as_setting().into_owned();
    if key.is_schema() {
        object.remove(SWITCH_KEY_SETTING);
    } else {
        object.insert(SWITCH_KEY_SETTING.into(), serde_json::json!(setting));
    }
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&config).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("save English switch key settings: {e}"))?;
    Ok(setting)
}

fn is_active_ascii_switch_style(style: &str) -> bool {
    matches!(
        style,
        "inline_ascii"
            | "commit_code"
            | "commit_text"
            | "clear"
            | "set_ascii_mode"
            | "unset_ascii_mode"
    )
}

fn extract_switch_keys_from_mapping(
    switch_key: &serde_yaml::Mapping,
    include_caps_lock: bool,
) -> Vec<String> {
    let mut keys = Vec::new();
    let ordered: &[&str] = if include_caps_lock {
        &[
            "Shift_L",
            "Shift_R",
            "Control_L",
            "Control_R",
            "Alt_L",
            "Alt_R",
            "Caps_Lock",
        ]
    } else {
        &[
            "Shift_L",
            "Shift_R",
            "Control_L",
            "Control_R",
            "Alt_L",
            "Alt_R",
        ]
    };
    for &name in ordered {
        let enabled = switch_key
            .get(serde_yaml::Value::String(name.into()))
            .and_then(serde_yaml::Value::as_str)
            .is_some_and(is_active_ascii_switch_style);
        if enabled {
            keys.push(name.to_owned());
        }
    }
    keys
}

fn read_yaml_file(path: &Path) -> Option<serde_yaml::Value> {
    let bytes = std::fs::read(path).ok()?;
    serde_yaml::from_slice(&bytes).ok()
}

fn yaml_switch_key_map<'a>(doc: &'a serde_yaml::Value) -> Option<&'a serde_yaml::Mapping> {
    if let Some(map) = doc
        .get("ascii_composer")
        .and_then(|v| v.get("switch_key"))
        .and_then(serde_yaml::Value::as_mapping)
    {
        return Some(map);
    }
    if let Some(patch) = doc.get("patch").and_then(serde_yaml::Value::as_mapping) {
        if let Some(map) = patch
            .get(serde_yaml::Value::String("ascii_composer/switch_key".into()))
            .and_then(serde_yaml::Value::as_mapping)
        {
            return Some(map);
        }
        if let Some(map) = patch
            .get(serde_yaml::Value::String("ascii_composer".into()))
            .and_then(|v| v.get("switch_key"))
            .and_then(serde_yaml::Value::as_mapping)
        {
            return Some(map);
        }
    }
    None
}

fn yaml_key_binder_ascii_toggles(doc: &serde_yaml::Value) -> Vec<String> {
    let mut keys = Vec::new();
    let Some(bindings) = doc
        .get("key_binder")
        .and_then(|v| v.get("bindings"))
        .and_then(serde_yaml::Value::as_sequence)
    else {
        return keys;
    };
    for binding in bindings {
        if binding.get("toggle").and_then(serde_yaml::Value::as_str) == Some("ascii_mode") {
            if let Some(accept) = binding.get("accept").and_then(serde_yaml::Value::as_str) {
                if let Ok(parsed) = EnglishSwitchKey::parse(accept) {
                    let setting = parsed.as_setting().into_owned();
                    if !keys.contains(&setting) {
                        keys.push(setting);
                    }
                }
            }
        }
    }
    keys
}

/// Read the active schema's configured Chinese/English switch keys on desktop,
/// reflecting `libjd_engine` defaults or the deployed Rime schema/default YAML.
pub fn read_schema_english_switch_keys(root: &Path) -> Vec<String> {
    read_schema_english_switch_keys_with_caps(root, !cfg!(target_os = "macos"))
}

pub fn read_schema_english_switch_keys_with_caps(
    root: &Path,
    include_caps_lock: bool,
) -> Vec<String> {
    if !crate::engine_choice::preferred(root).is_rime() {
        let mut keys = vec!["Shift_L".into(), "Shift_R".into()];
        if include_caps_lock {
            keys.push("Caps_Lock".into());
        }
        return keys;
    }

    let dirs = match crate::RuntimeDirs::resolve(root, root) {
        Ok(dirs) => dirs,
        Err(_) => crate::RuntimeDirs::flat(root, root),
    };
    let schema_id = crate::current_schema_id_from_dir(root)
        .or_else(|| crate::current_schema_id_from_dir(&dirs.user_data_dir))
        .or_else(|| {
            std::fs::read_to_string(dirs.user_data_dir.join(".keytao-current-schema"))
                .ok()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
        });

    let schema_doc = schema_id.as_deref().and_then(|id| {
        read_yaml_file(&dirs.staging_dir.join(format!("{id}.schema.yaml")))
            .or_else(|| read_yaml_file(&dirs.user_data_dir.join(format!("{id}.schema.yaml"))))
    });
    let default_doc = read_yaml_file(&dirs.staging_dir.join("default.yaml"))
        .or_else(|| read_yaml_file(&dirs.user_data_dir.join("default.custom.yaml")))
        .or_else(|| read_yaml_file(&dirs.user_data_dir.join("default.yaml")));

    let uses_ascii_composer = schema_doc.as_ref().map_or(true, |doc| {
        doc.get("engine")
            .and_then(|v| v.get("processors"))
            .and_then(serde_yaml::Value::as_sequence)
            .is_none_or(|list| {
                list.iter().any(|item| {
                    item.as_str()
                        .and_then(|s| s.split('@').next())
                        == Some("ascii_composer")
                })
            })
    });

    if uses_ascii_composer {
        if let Some(map) = schema_doc.as_ref().and_then(yaml_switch_key_map) {
            let keys = extract_switch_keys_from_mapping(map, include_caps_lock);
            if !keys.is_empty() {
                return keys;
            }
        } else if let Some(map) = default_doc.as_ref().and_then(yaml_switch_key_map) {
            let keys = extract_switch_keys_from_mapping(map, include_caps_lock);
            if !keys.is_empty() {
                return keys;
            }
        }
    }

    for doc in [schema_doc.as_ref(), default_doc.as_ref()].into_iter().flatten() {
        let keys = yaml_key_binder_ascii_toggles(doc);
        if !keys.is_empty() {
            return keys;
        }
    }
    Vec::new()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnglishMode {
    #[default]
    Ascii,
    Schema,
}

impl EnglishMode {
    pub fn from_setting(value: &str) -> Self {
        if value.trim().eq_ignore_ascii_case("schema") {
            Self::Schema
        } else {
            Self::Ascii
        }
    }
}

pub fn read_english_mode(root: &Path) -> EnglishMode {
    std::fs::read(root.join(SETTINGS_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| {
            value
                .get("englishMode")
                .and_then(|v| v.as_str())
                .map(EnglishMode::from_setting)
        })
        .unwrap_or_default()
}

/// Preserve unrelated settings, and reject malformed files instead of erasing
/// them. The caller signals its normal runtime reload after saving this value.
pub fn write_english_mode(root: &Path, mode: EnglishMode) -> Result<(), String> {
    let path = root.join(SETTINGS_FILE);
    let mut config = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes)
            .map_err(|e| format!("read English input settings: {e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(e) => return Err(format!("read English input settings: {e}")),
    };
    let object = config
        .as_object_mut()
        .ok_or("English input settings must be an object")?;
    object.insert("englishMode".into(), serde_json::json!(mode));
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&config).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("save English input settings: {e}"))
}

/// Same preference order as the Android and iOS keyboards.
pub fn is_english_schema(id: &str, name: &str) -> bool {
    matches!(id, "easy_en" | "english")
        || name.eq_ignore_ascii_case("Easy English")
        || name.eq_ignore_ascii_case("English")
}

pub fn english_schema_id(schemas: &[SchemaInfo]) -> Option<&str> {
    ["easy_en", "english"]
        .into_iter()
        .find_map(|id| schemas.iter().find(|s| s.id == id).map(|s| s.id.as_str()))
        .or_else(|| {
            schemas
                .iter()
                .find(|s| {
                    s.name.eq_ignore_ascii_case("Easy English")
                        || s.name.eq_ignore_ascii_case("English")
                })
                .map(|s| s.id.as_str())
        })
}

pub(crate) trait EnglishBackend {
    fn current_schema(&self) -> Option<SchemaInfo>;
    fn schema_switches(&self) -> Vec<SchemaSwitch>;
    fn get_option(&self, name: &str) -> bool;
    fn set_option(&self, name: &str, enabled: bool) -> ImeState;
    fn select_schema(&self, id: &str) -> Result<ImeState, String>;
    fn discard_composition(&self);
    fn state(&self) -> ImeState;
    fn apply_ascii_mode(&self, enabled: bool);
    fn set_ascii_mode(&self, enabled: bool) -> ImeState;
}

pub(crate) fn schema_option_names(switches: Vec<SchemaSwitch>) -> impl Iterator<Item = String> {
    switches
        .into_iter()
        .flat_map(|s| s.name.into_iter().chain(s.options))
        .filter(|name| name != "ascii_mode")
}

pub(crate) fn schema_options(engine: &dyn EnglishBackend) -> BTreeMap<String, bool> {
    schema_option_names(engine.schema_switches())
        .map(|name| {
            let enabled = engine.get_option(&name);
            (name, enabled)
        })
        .collect()
}

#[derive(Default)]
pub(crate) struct EnglishModeController {
    pub mode: EnglishMode,
    pub switch_key: EnglishSwitchKey,
    schemas: Vec<SchemaInfo>,
    chinese_schema: Option<String>,
    chinese_options: BTreeMap<String, bool>,
}

impl EnglishModeController {
    #[cfg(test)]
    pub fn configure(&mut self, mode: EnglishMode, schemas: Vec<SchemaInfo>) {
        self.configure_with_switch_key(mode, EnglishSwitchKey::Schema, schemas);
    }

    pub fn configure_with_switch_key(
        &mut self,
        mode: EnglishMode,
        switch_key: EnglishSwitchKey,
        schemas: Vec<SchemaInfo>,
    ) {
        self.mode = mode;
        self.switch_key = switch_key;
        self.schemas = schemas;
    }

    pub fn is_english(&self, engine: &dyn EnglishBackend) -> bool {
        engine.state().is_english_mode()
    }

    pub fn is_english_schema(&self, id: &str) -> bool {
        self.schemas
            .iter()
            .any(|s| s.id == id && is_english_schema(&s.id, &s.name))
    }

    pub fn chinese_schema(&self) -> Option<&str> {
        self.chinese_schema.as_deref()
    }

    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos", test))]
    pub fn remember_chinese(&mut self, id: &str, options: &BTreeMap<String, bool>) {
        if !self.is_english_schema(id) {
            self.chinese_schema = Some(id.to_owned());
            self.chinese_options = options.clone();
        }
    }

    /// Reconcile an ascii_composer switch after a desktop key event. A handled
    /// language switch must also consume the event, even if librime leaves the
    /// Shift release unhandled, or a frontend fallback would switch twice.
    pub fn reconcile_ascii_toggle(
        &mut self,
        engine: &dyn EnglishBackend,
        before: bool,
        result: &mut KeyProcessResult,
    ) -> Result<(), String> {
        if self.mode != EnglishMode::Schema || before || !result.state.ascii_mode {
            return Ok(());
        }
        let in_english = engine
            .current_schema()
            .is_some_and(|s| self.is_english_schema(&s.id));
        let switched = self.switch(engine, !in_english);
        // On a failed return to Chinese, keep the English dictionary usable.
        if switched.is_err() && in_english {
            engine.apply_ascii_mode(false);
        }
        let (mut state, error) = match switched {
            Ok(state) => (state, None),
            Err(error) => (engine.state(), Some(error)),
        };
        if let Some(commit) = result.state.committed.take() {
            state.committed = Some(format!("{commit}{}", state.committed.unwrap_or_default()));
        }
        result.state = state;
        result.accepted = true;
        error.map_or(Ok(()), Err)
    }

    /// A rebuilt engine starts on its configured Chinese schema. Entering
    /// English again must not replace the option snapshot saved before reload.
    pub fn restore(&mut self, engine: &dyn EnglishBackend, english: bool) -> Result<(), String> {
        let previous_schema = self.chinese_schema.clone();
        let previous_options = self.chinese_options.clone();
        self.switch(engine, english)?;
        if english
            && previous_schema
                .as_ref()
                .is_some_and(|id| self.schemas.iter().any(|s| &s.id == id))
        {
            self.chinese_schema = previous_schema;
            self.chinese_options = previous_options;
        }
        Ok(())
    }

    pub fn switch(
        &mut self,
        engine: &dyn EnglishBackend,
        english: bool,
    ) -> Result<ImeState, String> {
        let current = engine
            .current_schema()
            .ok_or("Rime current schema is unavailable")?;
        let in_english =
            is_english_schema(&current.id, &current.name) || self.is_english_schema(&current.id);
        // A user can also choose an English schema directly from the schema
        // menu. Switching back to Chinese must leave it even in ASCII mode.
        if self.mode == EnglishMode::Ascii && (english || !in_english) {
            return Ok(engine.set_ascii_mode(english));
        }
        if english == in_english {
            return Ok(engine.set_ascii_mode(false));
        }
        let target = if english {
            let Some(id) = english_schema_id(&self.schemas) else {
                return Ok(engine.set_ascii_mode(true));
            };
            id.to_owned()
        } else {
            self.chinese_schema
                .as_ref()
                .filter(|id| {
                    self.schemas
                        .iter()
                        .any(|s| &s.id == *id && !is_english_schema(&s.id, &s.name))
                })
                .or_else(|| {
                    self.schemas
                        .iter()
                        .find(|s| !is_english_schema(&s.id, &s.name))
                        .map(|s| &s.id)
                })
                .cloned()
                .ok_or("No Chinese input schema is available")?
        };
        let snapshot = schema_options(engine);
        let previous_ascii = engine.state().ascii_mode;
        // Leave an already committed string pending until a successful result
        // can return it to the host; failed deployment must not swallow it.
        engine.discard_composition();
        if let Err(error) = engine.select_schema(&target) {
            // Some librime builds mutate the active schema before discovering
            // missing compiled data. Keep the previous input method usable.
            if engine.select_schema(&current.id).is_ok() {
                for (name, enabled) in &snapshot {
                    engine.set_option(name, *enabled);
                }
                engine.apply_ascii_mode(previous_ascii);
            }
            return Err(error);
        }
        if english {
            self.chinese_schema = Some(current.id);
            self.chinese_options = snapshot;
        } else if self.chinese_schema.as_deref() == Some(target.as_str()) {
            for name in schema_options(engine).keys() {
                if let Some(enabled) = self.chinese_options.get(name) {
                    engine.set_option(name, *enabled);
                }
            }
        }
        Ok(engine.set_ascii_mode(false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeEngine(RefCell<FakeState>);
    struct FakeState {
        schema: String,
        ascii: bool,
        options: BTreeMap<String, bool>,
        selected: Vec<String>,
        fail: Option<String>,
        pending_commit: Option<String>,
    }
    fn schema(id: &str, name: &str) -> SchemaInfo {
        SchemaInfo {
            id: id.into(),
            name: name.into(),
        }
    }
    fn setup(mode: EnglishMode, schemas: Vec<SchemaInfo>) -> (EnglishModeController, FakeEngine) {
        let mut controller = EnglishModeController::default();
        controller.configure(mode, schemas);
        let engine = FakeEngine(RefCell::new(FakeState {
            schema: "chinese_alt".into(),
            ascii: false,
            options: BTreeMap::from([
                ("simplification".into(), true),
                ("ascii_punct".into(), true),
            ]),
            selected: Vec::new(),
            fail: None,
            pending_commit: None,
        }));
        (controller, engine)
    }
    fn schemas() -> Vec<SchemaInfo> {
        vec![
            schema("chinese", "Chinese"),
            schema("chinese_alt", "Other"),
            schema("easy_en", "Easy English"),
        ]
    }

    impl EnglishBackend for FakeEngine {
        fn current_schema(&self) -> Option<SchemaInfo> {
            Some(schema(&self.0.borrow().schema, ""))
        }
        fn schema_switches(&self) -> Vec<SchemaSwitch> {
            vec![
                SchemaSwitch {
                    name: Some("ascii_mode".into()),
                    options: vec![],
                    states: vec![],
                    reset: None,
                },
                SchemaSwitch {
                    name: None,
                    options: vec!["simplification".into()],
                    states: vec![],
                    reset: None,
                },
                SchemaSwitch {
                    name: Some("ascii_punct".into()),
                    options: vec![],
                    states: vec![],
                    reset: None,
                },
            ]
        }
        fn get_option(&self, name: &str) -> bool {
            self.0.borrow().options.get(name).copied().unwrap_or(false)
        }
        fn set_option(&self, name: &str, enabled: bool) -> ImeState {
            self.0.borrow_mut().options.insert(name.into(), enabled);
            self.state()
        }
        fn select_schema(&self, id: &str) -> Result<ImeState, String> {
            {
                let mut st = self.0.borrow_mut();
                st.selected.push(id.into());
                st.schema = id.into();
                st.ascii = true;
                st.options.clear();
                if st.fail.as_deref() == Some(id) {
                    return Err("missing deployed dictionary".into());
                }
            }
            Ok(self.state())
        }
        fn discard_composition(&self) {}
        fn state(&self) -> ImeState {
            let mut state = ImeState::empty();
            state.ascii_mode = self.0.borrow().ascii;
            state.schema_id = self.0.borrow().schema.clone();
            state
        }
        fn apply_ascii_mode(&self, enabled: bool) {
            self.0.borrow_mut().ascii = enabled;
        }
        fn set_ascii_mode(&self, enabled: bool) -> ImeState {
            self.apply_ascii_mode(enabled);
            let mut state = self.state();
            state.committed = self.0.borrow_mut().pending_commit.take();
            state
        }
    }

    #[test]
    fn dictionary_english_roundtrip_restores_previous_chinese_and_options() {
        let (mut mode, engine) = setup(EnglishMode::Schema, schemas());
        engine.0.borrow_mut().pending_commit = Some("already committed".into());
        let entered = mode.switch(&engine, true).unwrap();
        assert_eq!(engine.0.borrow().schema, "easy_en");
        assert!(
            !entered.ascii_mode,
            "English dictionary must keep candidate processing enabled"
        );
        assert!(mode.is_english(&engine));
        assert_eq!(entered.committed.as_deref(), Some("already committed"));
        mode.switch(&engine, false).unwrap();
        assert_eq!(engine.0.borrow().schema, "chinese_alt");
        assert!(engine.get_option("simplification"));
        assert!(engine.get_option("ascii_punct"));
        assert!(!mode.is_english(&engine));
        assert!(!engine.0.borrow().options.contains_key("ascii_mode"));
    }

    #[test]
    fn ascii_preference_and_missing_english_schema_remain_passthrough() {
        for (preference, schemas) in [
            (EnglishMode::Ascii, schemas()),
            (EnglishMode::Schema, vec![schema("chinese_alt", "Chinese")]),
        ] {
            let (mut mode, engine) = setup(preference, schemas);
            assert!(mode.switch(&engine, true).unwrap().ascii_mode);
            assert!(mode.is_english(&engine));
            assert!(!mode.switch(&engine, false).unwrap().ascii_mode);
            assert!(engine.0.borrow().selected.is_empty());
        }
    }

    #[test]
    fn reload_while_english_retains_chinese_option_snapshot() {
        let (mut mode, engine) = setup(EnglishMode::Schema, schemas());
        mode.switch(&engine, true).unwrap();
        {
            let mut st = engine.0.borrow_mut();
            st.schema = "chinese".into();
            st.options.clear();
        }
        mode.configure(EnglishMode::Schema, schemas());
        mode.restore(&engine, true).unwrap();
        mode.switch(&engine, false).unwrap();
        assert_eq!(engine.0.borrow().schema, "chinese_alt");
        assert!(engine.get_option("simplification"));
        assert!(engine.get_option("ascii_punct"));
    }

    #[test]
    fn removed_chinese_schema_falls_back_to_an_installed_chinese_schema() {
        let (mut mode, engine) = setup(EnglishMode::Schema, schemas());
        mode.switch(&engine, true).unwrap();
        mode.configure(
            EnglishMode::Schema,
            vec![
                schema("chinese", "Chinese"),
                schema("easy_en", "Easy English"),
            ],
        );
        mode.switch(&engine, false).unwrap();
        assert_eq!(engine.0.borrow().schema, "chinese");
        assert!(!engine.get_option("simplification"));
        assert!(!engine.get_option("ascii_punct"));
    }

    #[test]
    fn english_schema_discovery_matches_mobile_priority() {
        let mut values = vec![
            schema("custom", "English"),
            schema("english", "Bundled"),
            schema("easy_en", "Easy English"),
        ];
        assert_eq!(english_schema_id(&values), Some("easy_en"));
        values.pop();
        assert_eq!(english_schema_id(&values), Some("english"));
        values.pop();
        assert_eq!(english_schema_id(&values), Some("custom"));
        assert_eq!(english_schema_id(&[schema("wanxiang", "万象")]), None);
    }

    #[test]
    fn failed_english_schema_selection_does_not_replace_return_schema() {
        let (mut mode, engine) = setup(EnglishMode::Schema, schemas());
        engine.0.borrow_mut().fail = Some("easy_en".into());
        engine.0.borrow_mut().pending_commit = Some("pending".into());
        assert!(mode.switch(&engine, true).is_err());
        assert_eq!(engine.0.borrow().schema, "chinese_alt");
        assert!(!engine.state().ascii_mode);
        assert!(engine.get_option("simplification"));
        assert!(engine.get_option("ascii_punct"));
        assert_eq!(engine.0.borrow().pending_commit.as_deref(), Some("pending"));
        assert!(mode.chinese_schema.is_none());
    }

    #[test]
    fn shift_toggle_enters_and_leaves_dictionary_without_frontend_double_toggle() {
        let (mut mode, engine) = setup(EnglishMode::Schema, schemas());
        for (expected_schema, expected_english) in [("easy_en", true), ("chinese_alt", false)] {
            // ascii_composer can change mode while returning false for the
            // Shift release. The host must see this as handled exactly once.
            engine.apply_ascii_mode(true);
            let mut result = KeyProcessResult {
                state: engine.state(),
                accepted: false,
            };
            result.state.committed = Some("committed before switch".into());
            mode.reconcile_ascii_toggle(&engine, false, &mut result)
                .unwrap();
            assert!(result.accepted);
            assert!(!result.state.ascii_mode);
            assert_eq!(result.state.schema_id, expected_schema);
            assert_eq!(result.state.is_english_mode(), expected_english);
            assert_eq!(
                result.state.committed.as_deref(),
                Some("committed before switch")
            );
        }
        assert!(engine.get_option("simplification"));
    }

    #[test]
    fn ordinary_keys_and_ascii_preference_do_not_switch_schemas() {
        for (preference, before, after) in [
            (EnglishMode::Schema, false, false),
            (EnglishMode::Schema, true, true),
            (EnglishMode::Schema, true, false),
            (EnglishMode::Ascii, false, true),
        ] {
            let (mut mode, engine) = setup(preference, schemas());
            engine.apply_ascii_mode(after);
            let mut result = KeyProcessResult {
                state: engine.state(),
                accepted: false,
            };
            mode.reconcile_ascii_toggle(&engine, before, &mut result)
                .unwrap();
            assert!(engine.0.borrow().selected.is_empty());
            assert_eq!(result.state.ascii_mode, after);
            assert!(!result.accepted);
        }
    }

    #[test]
    fn failed_shift_return_keeps_english_dictionary_and_reports_actual_state() {
        let (mut mode, engine) = setup(EnglishMode::Schema, schemas());
        mode.switch(&engine, true).unwrap();
        engine.0.borrow_mut().fail = Some("chinese_alt".into());
        engine.apply_ascii_mode(true);
        let mut result = KeyProcessResult {
            state: engine.state(),
            accepted: false,
        };
        assert!(mode
            .reconcile_ascii_toggle(&engine, false, &mut result)
            .is_err());
        assert_eq!(result.state.schema_id, "easy_en");
        assert!(!result.state.ascii_mode);
        assert!(result.state.is_english_mode());
        assert!(result.accepted);
    }

    #[test]
    fn logical_language_does_not_require_the_ascii_passthrough_flag() {
        for (id, name, ascii, expected) in [
            ("easy_en", "", false, true),
            ("english", "", false, true),
            ("custom", "eAsY eNgLiSh", false, true),
            ("custom", "ENGLISH", false, true),
            ("wanxiang", "万象", true, true),
            ("wanxiang", "万象", false, false),
        ] {
            let mut state = ImeState::empty();
            state.schema_id = id.into();
            state.schema_name = name.into();
            state.ascii_mode = ascii;
            assert_eq!(state.is_english_mode(), expected);
            assert_eq!(state.ascii_mode, ascii);
        }
    }

    #[test]
    fn manually_selected_english_schema_can_return_to_chinese_with_ascii_preference() {
        let (mut mode, engine) = setup(EnglishMode::Ascii, schemas());
        engine.0.borrow_mut().schema = "easy_en".into();
        assert!(mode.is_english(&engine));
        let returned = mode.switch(&engine, false).unwrap();
        assert_eq!(returned.schema_id, "chinese");
        assert!(!returned.is_english_mode());
    }

    #[test]
    fn settings_preserve_unrelated_values_and_reject_invalid_json() {
        let root = std::env::temp_dir().join(format!(
            "keytao-english-settings-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        assert_eq!(read_english_mode(&root), EnglishMode::Ascii);
        std::fs::write(root.join(SETTINGS_FILE), br#"{"other":7}"#).unwrap();
        write_english_mode(&root, EnglishMode::Schema).unwrap();
        assert_eq!(read_english_mode(&root), EnglishMode::Schema);
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join(SETTINGS_FILE)).unwrap()).unwrap();
        assert_eq!(value["other"], 7);
        std::fs::write(root.join(SETTINGS_FILE), b"invalid").unwrap();
        assert!(write_english_mode(&root, EnglishMode::Ascii).is_err());
        assert_eq!(std::fs::read(root.join(SETTINGS_FILE)).unwrap(), b"invalid");
        std::fs::remove_file(root.join(SETTINGS_FILE)).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn english_switch_key_parses_canonicalizes_and_rejects_unsafe_bindings() {
        assert_eq!(EnglishSwitchKey::parse("").unwrap(), EnglishSwitchKey::Schema);
        assert_eq!(
            EnglishSwitchKey::parse("schema").unwrap(),
            EnglishSwitchKey::Schema
        );
        for (input, expected) in [
            ("shift", "Shift"),
            ("Shift_L", "Shift_L"),
            ("shift_r", "Shift_R"),
            ("ctrl", "Control"),
            ("control_l", "Control_L"),
            ("Control_R", "Control_R"),
            ("option", "Alt"),
            ("alt_l", "Alt_L"),
            ("Option_R", "Alt_R"),
            ("Caps_Lock", "Caps_Lock"),
            ("ctrl+space", "Control+space"),
            ("Shift+Ctrl+space", "Control+Shift+space"),
            ("Option+grave", "Alt+grave"),
            ("Shift+space", "Shift+space"),
            ("F2", "F2"),
        ] {
            let parsed = EnglishSwitchKey::parse(input).unwrap_or_else(|e| panic!("{input}: {e}"));
            assert_eq!(parsed.as_setting(), expected, "{input}");
        }
        for rejected in [
            "a",
            "space",
            "Shift+a",
            "Shift+1",
            "Return",
            "BackSpace",
            "Super+space",
            "Control+",
            "+space",
        ] {
            assert!(
                EnglishSwitchKey::parse(rejected).is_err(),
                "expected {rejected:?} to be rejected"
            );
        }
    }

    #[test]
    fn english_switch_key_roundtrips_and_applies_state_overrides() {
        let root = std::env::temp_dir().join(format!(
            "keytao-switch-key-settings-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        assert_eq!(read_english_switch_key(&root), EnglishSwitchKey::Schema);
        std::fs::write(root.join(SETTINGS_FILE), br#"{"englishMode":"schema","other":7}"#).unwrap();

        let ctrl_l = EnglishSwitchKey::parse("Control_L").unwrap();
        assert_eq!(write_english_switch_key(&root, &ctrl_l).unwrap(), "Control_L");
        assert_eq!(read_english_switch_key(&root), ctrl_l);
        let mut state = ImeState::empty();
        state.modifier_taps = vec![0xffe4];
        ctrl_l.apply_to_state(&mut state);
        assert_eq!(state.modifier_taps, vec![0xffe3]);

        let combo = EnglishSwitchKey::parse("Control+space").unwrap();
        assert_eq!(
            write_english_switch_key(&root, &combo).unwrap(),
            "Control+space"
        );
        assert_eq!(read_english_switch_key(&root), combo);
        let mut state = ImeState::empty();
        state.modifier_taps = vec![0xffe3];
        combo.apply_to_state(&mut state);
        assert!(state.modifier_taps.is_empty());
        assert_eq!(state.schema_shortcuts.len(), 1);
        assert!(combo.matches_shortcut(0x20, RIME_MOD_CONTROL));
        assert!(!combo.matches_shortcut(0x20, RIME_MOD_CONTROL | RIME_RELEASE_MASK));

        assert_eq!(
            write_english_switch_key(&root, &EnglishSwitchKey::Schema).unwrap(),
            "schema"
        );
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join(SETTINGS_FILE)).unwrap()).unwrap();
        assert_eq!(saved["englishMode"], "schema");
        assert_eq!(saved["other"], 7);
        assert!(saved.get(SWITCH_KEY_SETTING).is_none());

        std::fs::remove_file(root.join(SETTINGS_FILE)).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn read_schema_english_switch_keys_reads_compiled_default_schema_override_and_libjd() {
        let root = std::env::temp_dir().join(format!(
            "keytao-schema-switch-keys-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let build = root.join("build");
        std::fs::create_dir_all(&build).unwrap();

        // keydo-like default.yaml: Shift_L disabled, Shift_R + Control_L/R + Caps_Lock enabled.
        std::fs::write(root.join(".keytao-current-schema"), "keydo").unwrap();
        std::fs::write(
            build.join("keydo.schema.yaml"),
            "engine:\n  processors:\n    - ascii_composer\n    - speller\n",
        )
        .unwrap();
        std::fs::write(
            build.join("default.yaml"),
            "ascii_composer:\n  switch_key:\n    Shift_L: noop\n    Shift_R: commit_text\n    Control_L: commit_code\n    Control_R: commit_code\n    Caps_Lock: inline_ascii\n",
        )
        .unwrap();
        assert_eq!(
            read_schema_english_switch_keys_with_caps(&root, false),
            vec!["Shift_R", "Control_L", "Control_R"]
        );
        assert_eq!(
            read_schema_english_switch_keys_with_caps(&root, true),
            vec!["Shift_R", "Control_L", "Control_R", "Caps_Lock"]
        );

        // Schema-level ascii_composer/switch_key overrides default.yaml.
        std::fs::write(
            build.join("keydo.schema.yaml"),
            "engine:\n  processors:\n    - ascii_composer\nascii_composer:\n  switch_key:\n    Shift_L: commit_text\n",
        )
        .unwrap();
        assert_eq!(
            read_schema_english_switch_keys_with_caps(&root, true),
            vec!["Shift_L"]
        );

        // libjd_engine reports its built-in Shift (+ Caps_Lock on Windows/Linux).
        std::fs::write(
            root.join(SETTINGS_FILE),
            br#"{"engine":"libjd_engine"}"#,
        )
        .unwrap();
        assert_eq!(
            read_schema_english_switch_keys_with_caps(&root, false),
            vec!["Shift_L", "Shift_R"]
        );
        assert_eq!(
            read_schema_english_switch_keys_with_caps(&root, true),
            vec!["Shift_L", "Shift_R", "Caps_Lock"]
        );

        let _ = std::fs::remove_dir_all(root);
    }
}
