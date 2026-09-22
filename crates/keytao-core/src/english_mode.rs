//! English input preference and schema switching shared by desktop frontends.
//! `ascii_mode` remains librime's pass-through flag; English dictionary input
//! deliberately keeps that flag off so the engine can produce candidates.

use crate::{ImeState, KeyProcessResult, SchemaInfo, SchemaSwitch};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

pub const SETTINGS_FILE: &str = "keytao-ime.json";

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

#[derive(Default)]
pub(crate) struct EnglishModeController {
    pub mode: EnglishMode,
    schemas: Vec<SchemaInfo>,
    chinese_schema: Option<String>,
    chinese_options: BTreeMap<String, bool>,
}

impl EnglishModeController {
    pub fn configure(&mut self, mode: EnglishMode, schemas: Vec<SchemaInfo>) {
        self.mode = mode;
        self.schemas = schemas;
    }

    pub fn is_english(&self, engine: &impl EnglishBackend) -> bool {
        engine.state().is_english_mode()
    }

    pub fn is_english_schema(&self, id: &str) -> bool {
        self.schemas
            .iter()
            .any(|s| s.id == id && is_english_schema(&s.id, &s.name))
    }

    /// Reconcile an ascii_composer switch after a desktop key event. A handled
    /// language switch must also consume the event, even if librime leaves the
    /// Shift release unhandled, or a frontend fallback would switch twice.
    pub fn reconcile_ascii_toggle(
        &mut self,
        engine: &impl EnglishBackend,
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
    pub fn restore(&mut self, engine: &impl EnglishBackend, english: bool) -> Result<(), String> {
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
        engine: &impl EnglishBackend,
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
        let names = engine
            .schema_switches()
            .into_iter()
            .flat_map(|s| s.name.into_iter().chain(s.options))
            .chain(std::iter::once("ascii_punct".into()));
        let snapshot: BTreeMap<_, _> = names
            .filter(|name| name != "ascii_mode")
            .map(|name| {
                let enabled = engine.get_option(&name);
                (name, enabled)
            })
            .collect();
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
            for (name, enabled) in &self.chinese_options {
                engine.set_option(name, *enabled);
            }
        }
        Ok(engine.set_ascii_mode(false))
    }
}

impl EnglishBackend for crate::Engine {
    fn current_schema(&self) -> Option<SchemaInfo> {
        self.current_schema()
    }
    fn schema_switches(&self) -> Vec<SchemaSwitch> {
        self.schema_switches()
    }
    fn get_option(&self, name: &str) -> bool {
        self.get_option(name)
    }
    fn set_option(&self, name: &str, enabled: bool) -> ImeState {
        self.set_option(name, enabled)
    }
    fn select_schema(&self, id: &str) -> Result<ImeState, String> {
        self.select_schema(id)
    }
    fn discard_composition(&self) {
        self.discard_composition()
    }
    fn state(&self) -> ImeState {
        self.state()
    }
    fn apply_ascii_mode(&self, enabled: bool) {
        self.apply_ascii_mode(enabled)
    }
    fn set_ascii_mode(&self, enabled: bool) -> ImeState {
        self.set_ascii_mode(enabled)
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
}
