use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use serde_yaml::Value;

const FILTER: &str = "lua_filter@*keytao_grammar_hint";
pub(crate) const SCRIPT: &str = include_str!("keytao_grammar_hint.lua");
pub(crate) const SCRIPT_PATH: &str = "lua/keytao_grammar_hint.lua";
const WANXIANG_GRAMMAR_PATCH: &str = "{grammar/language: wanxiang-lts-zh-hans, grammar/collocation_max_length: 6, grammar/collocation_min_length: 2, grammar/collocation_penalty: -14, grammar/non_collocation_penalty: -6, grammar/weak_collocation_penalty: -100, grammar/rear_penalty: -18, translator/contextual_suggestions: true, translator/max_homophones: 8, translator/max_homographs: 7}";
const MAX_INCLUDE_DEPTH: usize = 4;

fn valid_language(language: &str) -> bool {
    !language.is_empty()
        && !matches!(language, "." | "..")
        && !language.contains(['/', '\\', '\0'])
}

fn read_yaml(path: &Path) -> Result<Option<Value>, String> {
    match fs::read_to_string(path) {
        Ok(content) => serde_yaml::from_str(&content)
            .map(Some)
            .map_err(|error| format!("Invalid {}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn parse_language_field(value: &Value) -> Option<Option<String>> {
    match value {
        Value::String(language) => Some(
            valid_language(language)
                .then(|| language.clone()),
        ),
        Value::Null => Some(None),
        _ => None,
    }
}

fn resolve_yaml_pointer<'a>(doc: &'a Value, pointer: &str) -> Option<&'a Value> {
    let trimmed = pointer.trim_start_matches('/');
    if trimmed.is_empty() {
        return Some(doc);
    }
    let mut current = doc;
    for part in trimmed.split('/') {
        if part.is_empty() {
            continue;
        }
        current = current.as_mapping()?.get(Value::String(part.to_owned()))?;
    }
    Some(current)
}

fn load_reference(root: &Path, reference: &str) -> Option<Value> {
    let (file_spec, pointer) = match reference.split_once(':') {
        Some((file, pointer)) => (file.trim(), pointer.trim()),
        None => (reference.trim(), ""),
    };
    if file_spec.is_empty() {
        return None;
    }
    let file_name = if file_spec.ends_with(".yaml") || file_spec.ends_with(".yml") {
        file_spec.to_owned()
    } else {
        format!("{file_spec}.yaml")
    };
    let path = crate::packages::safe_path(root, &file_name).ok()?;
    let doc = read_yaml(&path).ok()??;
    resolve_yaml_pointer(&doc, pointer).cloned()
}

fn collect_references(value: &Value) -> Vec<&str> {
    match value {
        Value::String(reference) => vec![reference.as_str()],
        Value::Sequence(items) => items.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

/// Returns `Some(Some(lang))` when a valid `grammar.language` is set,
/// `Some(None)` when explicitly cleared to `""` or `null`, or `None` when not specified.
fn extract_grammar_language(root: &Path, value: &Value, depth: usize) -> Option<Option<String>> {
    let mapping = value.as_mapping()?;
    let mut found: Option<Option<String>> = None;

    if depth < MAX_INCLUDE_DEPTH {
        for directive in ["__include", "__patch"] {
            if let Some(target) = mapping.get(Value::String(directive.into())) {
                for reference in collect_references(target) {
                    if let Some(referenced) = load_reference(root, reference) {
                        if let Some(lang) = extract_grammar_language(root, &referenced, depth + 1) {
                            found = Some(lang);
                        }
                    }
                }
                match target {
                    Value::Mapping(_) => {
                        if let Some(lang) = extract_grammar_language(root, target, depth + 1) {
                            found = Some(lang);
                        }
                    }
                    Value::Sequence(items) => {
                        for item in items {
                            if item.is_mapping() {
                                if let Some(lang) = extract_grammar_language(root, item, depth + 1) {
                                    found = Some(lang);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    if let Some(grammar) = mapping.get(Value::String("grammar".into())) {
        if let Some(grammar_map) = grammar.as_mapping() {
            if depth < MAX_INCLUDE_DEPTH {
                for directive in ["__include", "__patch"] {
                    if let Some(target) = grammar_map.get(Value::String(directive.into())) {
                        for reference in collect_references(target) {
                            if let Some(referenced) = load_reference(root, reference) {
                                if let Some(lang) = referenced
                                    .get("language")
                                    .and_then(parse_language_field)
                                {
                                    found = Some(lang);
                                }
                            }
                        }
                    }
                }
            }
            if let Some(lang) = grammar_map
                .get(Value::String("language".into()))
                .and_then(parse_language_field)
            {
                found = Some(lang);
            }
        } else if grammar.is_null() {
            found = Some(None);
        }
    }

    if let Some(lang) = mapping
        .get(Value::String("grammar/language".into()))
        .and_then(parse_language_field)
    {
        found = Some(lang);
    }

    for patch_key in ["patch", "patch/+"] {
        if let Some(patch) = mapping.get(Value::String(patch_key.into())) {
            if let Some(lang) = extract_grammar_language(root, patch, depth + 1) {
                found = Some(lang);
            }
        }
    }

    found
}

fn resolve_schema_grammar_language(root: &Path, schema: &Value, custom: &Value) -> Option<String> {
    let mut language = extract_grammar_language(root, schema, 0).flatten();
    if let Some(override_lang) = extract_grammar_language(root, custom, 0) {
        language = override_lang;
    }
    language
}

pub(crate) fn configured_grammar_languages(
    root: &Path,
    schemas: &BTreeSet<String>,
) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    for id in schemas {
        let Ok(schema_path) = crate::packages::safe_path(root, &format!("{id}.schema.yaml")) else {
            continue;
        };
        let Ok(Some(schema)) = read_yaml(&schema_path) else {
            continue;
        };
        let custom = crate::packages::safe_path(root, &format!("{id}.custom.yaml"))
            .ok()
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|content| crate::managed_custom::set_patch(&content, "grammar-hint", None).ok())
            .and_then(|original| serde_yaml::from_str::<Value>(&original).ok())
            .unwrap_or(Value::Null);
        if let Some(language) = resolve_schema_grammar_language(root, &schema, &custom) {
            result.insert(id.clone(), language);
        }
    }
    result
}

pub(crate) fn ensure_wanxiang_grammar_for_schemas(
    root: &Path,
    schemas: &BTreeSet<String>,
    enabled: bool,
) -> Result<bool, String> {
    let mut changed = false;
    for id in schemas {
        if id == crate::wanxiang::SCHEMA_ID || id == crate::addon::EASY_EN_ADDON_ID {
            continue;
        }
        let result = (|| -> Result<(), String> {
            let schema_path = crate::packages::safe_path(root, &format!("{id}.schema.yaml"))?;
            let Some(schema) = read_yaml(&schema_path)? else {
                return Err("Active schema is unavailable".into());
            };
            let relative = format!("{id}.custom.yaml");
            let path = crate::packages::safe_path(root, &relative)?;
            let content = match fs::read_to_string(&path) {
                Ok(content) => content,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(error) => return Err(error.to_string()),
            };
            let without_wanxiang =
                crate::managed_custom::set_patch(&content, "wanxiang-grammar", None)?;
            let without_managed =
                crate::managed_custom::set_patch(&without_wanxiang, "grammar-hint", None)?;
            let custom: Value =
                serde_yaml::from_str(&without_managed).map_err(|error| error.to_string())?;
            let user_defined = resolve_schema_grammar_language(root, &schema, &custom).is_some();
            let patch = (enabled && !cfg!(target_os = "ios") && !user_defined)
                .then_some(WANXIANG_GRAMMAR_PATCH);
            let updated =
                crate::managed_custom::set_patch(&without_wanxiang, "wanxiang-grammar", patch)?;
            changed |= crate::managed_custom::write_custom(root, &relative, &content, &updated)?;
            Ok(())
        })();
        if let Err(error) = result {
            tracing::warn!(schema = id, %error, "Skipped Wanxiang grammar patch");
        }
    }
    Ok(changed)
}

pub(crate) fn ensure_for_schemas(root: &Path, schemas: &BTreeSet<String>) -> Result<bool, String> {
    let mut changed = false;
    let mut needed = false;
    let mut uncertain = false;
    for id in schemas {
        let result = (|| -> Result<(), String> {
            let schema_path = crate::packages::safe_path(root, &format!("{id}.schema.yaml"))?;
            let Some(schema) = read_yaml(&schema_path)? else {
                return Err("Active schema is unavailable".into());
            };
            let relative = format!("{id}.custom.yaml");
            let path = crate::packages::safe_path(root, &relative)?;
            let content = match fs::read_to_string(&path) {
                Ok(content) => content,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(error) => return Err(error.to_string()),
            };
            let original = crate::managed_custom::set_patch(&content, "grammar-hint", None)?;
            let custom: Value =
                serde_yaml::from_str(&original).map_err(|error| error.to_string())?;
            let has_grammar =
                !cfg!(target_os = "ios") && resolve_schema_grammar_language(root, &schema, &custom).is_some();
            if has_grammar {
                let script = crate::packages::safe_path(root, SCRIPT_PATH)?;
                let existing = match fs::read(&script) {
                    Ok(bytes) => Some(bytes),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(error) => return Err(error.to_string()),
                };
                if existing
                    .as_deref()
                    .is_some_and(|bytes| bytes != SCRIPT.as_bytes())
                {
                    return Err("User-modified grammar hint script was preserved".into());
                }
                if existing.is_none() {
                    fs::create_dir_all(script.parent().unwrap())
                        .map_err(|error| error.to_string())?;
                    crate::scheme_files::write_file_atomic(&script, SCRIPT.as_bytes())
                        .map_err(|error| error.to_string())?;
                    changed = true;
                }
                needed = true;
            }
            let patch = (has_grammar && !original.contains(FILTER))
                .then_some("{engine/filters/@next: lua_filter@*keytao_grammar_hint}");
            let updated = crate::managed_custom::set_patch(&original, "grammar-hint", patch)?;
            changed |= crate::managed_custom::write_custom(root, &relative, &content, &updated)?;
            Ok(())
        })();
        if let Err(error) = result {
            uncertain = true;
            tracing::warn!(schema = id, %error, "Skipped grammar hint filter patch");
        }
    }
    if !needed && !uncertain {
        changed |= crate::managed_custom::strip_file(root, SCRIPT_PATH)?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_for_schema_custom_and_included_grammar_configs() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("wanxiang.schema.yaml"),
            "schema:\n  schema_id: wanxiang\ngrammar:\n  language: wanxiang-lts-zh-hans\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("rime_ice.schema.yaml"),
            "schema:\n  schema_id: rime_ice\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("rime_ice.custom.yaml"),
            "patch:\n  grammar/language: wanxiang-lts-zh-hans\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("grammar.yaml"),
            "patch:\n  grammar:\n    language: custom-model\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("frost.schema.yaml"),
            "schema:\n  schema_id: frost\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("frost.custom.yaml"),
            "patch:\n  __include: grammar:/patch\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("plain.schema.yaml"),
            "schema:\n  schema_id: plain\n",
        )
        .unwrap();

        let schemas = BTreeSet::from([
            "wanxiang".to_owned(),
            "rime_ice".to_owned(),
            "frost".to_owned(),
            "plain".to_owned(),
        ]);

        let configured = configured_grammar_languages(dir.path(), &schemas);
        assert_eq!(
            configured,
            BTreeMap::from([
                ("frost".to_owned(), "custom-model".to_owned()),
                ("rime_ice".to_owned(), "wanxiang-lts-zh-hans".to_owned()),
                ("wanxiang".to_owned(), "wanxiang-lts-zh-hans".to_owned()),
            ])
        );

        assert!(ensure_for_schemas(dir.path(), &schemas).unwrap());
        assert!(dir.path().join(SCRIPT_PATH).is_file());
        for id in ["wanxiang", "rime_ice", "frost"] {
            let content = fs::read_to_string(dir.path().join(format!("{id}.custom.yaml"))).unwrap();
            assert!(content.contains(FILTER), "{id} missing filter");
        }
        assert!(!dir.path().join("plain.custom.yaml").exists());
        assert!(!ensure_for_schemas(dir.path(), &schemas).unwrap());
    }

    #[test]
    fn custom_empty_language_disables_and_cleans_up_script() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("wanxiang.schema.yaml"),
            "schema:\n  schema_id: wanxiang\ngrammar:\n  language: wanxiang-lts-zh-hans\n",
        )
        .unwrap();
        let schemas = BTreeSet::from(["wanxiang".to_owned()]);
        assert!(ensure_for_schemas(dir.path(), &schemas).unwrap());
        assert!(dir.path().join(SCRIPT_PATH).is_file());

        // User explicitly disables grammar in custom yaml.
        fs::write(
            dir.path().join("wanxiang.custom.yaml"),
            "patch:\n  grammar/language: ''\n",
        )
        .unwrap();
        assert!(ensure_for_schemas(dir.path(), &schemas).unwrap());
        assert!(!dir.path().join(SCRIPT_PATH).exists());
        assert_eq!(
            fs::read_to_string(dir.path().join("wanxiang.custom.yaml")).unwrap(),
            "patch:\n  grammar/language: ''\n"
        );
    }

    #[test]
    fn user_modified_script_is_preserved() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("wanxiang.schema.yaml"),
            "grammar:\n  language: wanxiang-lts-zh-hans\n",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("lua")).unwrap();
        fs::write(dir.path().join(SCRIPT_PATH), "-- custom\n").unwrap();
        let schemas = BTreeSet::from(["wanxiang".to_owned()]);
        assert!(!ensure_for_schemas(dir.path(), &schemas).unwrap());
        assert_eq!(
            fs::read_to_string(dir.path().join(SCRIPT_PATH)).unwrap(),
            "-- custom\n"
        );
        assert!(!crate::managed_custom::strip_file(dir.path(), SCRIPT_PATH).unwrap());
    }
}
