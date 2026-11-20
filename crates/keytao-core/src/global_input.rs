#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
use crate::english_mode::EnglishModeController;
use crate::{ImeBackend, SchemaSwitchRequest};
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
use std::path::PathBuf;
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InputState {
    pub schema_id: String,
    ascii_mode: bool,
    options: BTreeMap<String, bool>,
}

impl InputState {
    pub fn read(engine: &dyn ImeBackend) -> Self {
        let state = engine.state();
        Self {
            schema_id: state.schema_id,
            ascii_mode: state.ascii_mode,
            options: crate::english_mode::schema_options(engine),
        }
    }

    pub fn is_switcher(&self) -> bool {
        self.schema_id == ".default"
    }

    pub fn restore(&self, engine: &dyn ImeBackend, root: &Path) -> Result<(), String> {
        engine.select_schema(&self.schema_id)?;
        engine.apply_reset_switch_request(root)?;
        engine.apply_switch_request(
            &SchemaSwitchRequest {
                generation: 0,
                schema_id: None,
                options: self.options.clone(),
            },
            false,
        )?;
        engine.apply_ascii_mode(self.ascii_mode);
        Ok(())
    }
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
pub(crate) struct GlobalInput {
    pub version: u64,
    root: PathBuf,
    state: InputState,
    pub english_mode: EnglishModeController,
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
impl GlobalInput {
    pub fn get<'a>(slot: &'a mut Option<Self>, engine: &dyn ImeBackend, root: &Path) -> &'a mut Self {
        if slot.as_ref().is_some_and(|global| global.root != root) {
            *slot = None;
        }
        slot.get_or_insert_with(|| Self::new(engine, root))
    }

    pub fn new(engine: &dyn ImeBackend, root: &Path) -> Self {
        let mut english_mode = EnglishModeController::default();
        english_mode.configure(
            crate::english_mode::read_english_mode(root),
            engine.list_schemas(),
        );
        let state = InputState::read(engine);
        if !state.is_switcher() {
            crate::publish_current_schema(root, &state.schema_id);
        }
        Self {
            version: 1,
            root: root.to_path_buf(),
            state,
            english_mode,
        }
    }

    pub fn request_schema(&mut self, id: &str) {
        if self.state.schema_id != id {
            self.english_mode
                .remember_chinese(&self.state.schema_id, &self.state.options);
            self.state.schema_id = id.to_owned();
            self.state.options.clear();
            self.version += 1;
            crate::publish_current_schema(&self.root, id);
        }
    }

    pub fn apply(&mut self, engine: &dyn ImeBackend, root: &Path) -> Result<(), String> {
        let current = engine
            .current_schema()
            .ok_or("Rime current schema is unavailable")?;
        if current.id != self.state.schema_id {
            let error = if engine
                .list_schemas()
                .iter()
                .any(|s| s.id == self.state.schema_id)
            {
                engine.select_schema(&self.state.schema_id).err()
            } else {
                Some(format!(
                    "schema {} is no longer installed",
                    self.state.schema_id
                ))
            };
            if let Some(error) = error {
                engine.select_schema(&current.id)?;
                engine.apply_reset_switch_request(root)?;
                engine.apply_ascii_mode(
                    self.state.ascii_mode
                        || self.english_mode.is_english_schema(&self.state.schema_id),
                );
                self.state = InputState::read(engine);
                self.version += 1;
                crate::rt_log!(
                    crate::runtime_log::Level::Info,
                    "error",
                    "global_input_restore",
                    msg = error
                );
            } else {
                engine.apply_reset_switch_request(root)?;
            }
        }
        engine.apply_switch_request(
            &SchemaSwitchRequest {
                generation: 0,
                schema_id: None,
                options: self.state.options.clone(),
            },
            false,
        )?;
        engine.apply_ascii_mode(self.state.ascii_mode);
        let applied = InputState::read(engine);
        if self.state != applied {
            if self.state.schema_id != applied.schema_id {
                crate::publish_current_schema(root, &applied.schema_id);
            }
            self.state = applied;
            self.version += 1;
        }
        Ok(())
    }

    pub fn publish(&mut self, before: &InputState, after: &InputState) {
        if after.is_switcher() {
            return;
        }
        let previous = self.state.clone();
        if before.schema_id != after.schema_id {
            self.english_mode
                .remember_chinese(&before.schema_id, &before.options);
            self.state = after.clone();
        } else {
            if before.ascii_mode != after.ascii_mode {
                self.state.ascii_mode = after.ascii_mode;
            }
            // A composing session can still be on an older global schema.
            if self.state.schema_id == after.schema_id {
                for (name, enabled) in &after.options {
                    if before.options.get(name) != Some(enabled) {
                        self.state.options.insert(name.clone(), *enabled);
                    }
                }
            }
        }
        if self.state != previous {
            if self.state.schema_id != previous.schema_id {
                crate::publish_current_schema(&self.root, &self.state.schema_id);
            }
            self.version += 1;
        }
    }

    pub fn matches(&self, state: &InputState) -> bool {
        &self.state == state
    }
}

#[cfg(all(
    test,
    any(target_os = "linux", target_os = "windows", target_os = "macos")
))]
mod tests {
    use super::*;

    #[test]
    fn versions_track_changes_and_merge_only_the_fields_a_session_changed() {
        let root = std::env::temp_dir().join(format!(
            "keytao-global-input-version-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let first = InputState {
            schema_id: "chinese".into(),
            ascii_mode: false,
            options: BTreeMap::from([("full_shape".into(), false), ("single_char".into(), false)]),
        };
        let mut global = GlobalInput {
            version: 1,
            root: root.clone(),
            state: first.clone(),
            english_mode: EnglishModeController::default(),
        };
        global.publish(&first, &first);
        assert_eq!(global.version, 1);
        let mut changed = first.clone();
        changed.ascii_mode = true;
        changed.options.insert("full_shape".into(), true);
        global.publish(&first, &changed);
        assert_eq!(global.version, 2);
        let mut stale = first.clone();
        stale.options.insert("single_char".into(), true);
        global.publish(&first, &stale);
        assert_eq!(global.version, 3);
        assert!(global.state.ascii_mode);
        assert!(global.state.options["full_shape"]);
        assert!(global.state.options["single_char"]);
        let mut other_schema = changed.clone();
        other_schema.schema_id = "other".into();
        other_schema.options.clear();
        global.publish(&changed, &other_schema);
        assert_eq!(global.version, 4);
        global.publish(&first, &stale);
        assert_eq!(global.version, 4);
        assert!(global.state.options.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}
