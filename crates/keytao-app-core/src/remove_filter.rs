use std::{collections::BTreeSet, fs, path::Path};

use serde_yaml::Value;

const FILTER: &str = "lua_filter@*keytao_remove_filter";
pub(crate) const SCRIPT: &str = include_str!("keytao_remove_filter.lua");
pub(crate) const SCRIPT_PATH: &str = "lua/keytao_remove_filter.lua";

fn uses_lua(value: &Value) -> bool {
    match value {
        Value::String(value) => value.starts_with("lua_") && value != FILTER,
        Value::Sequence(values) => values.iter().any(uses_lua),
        Value::Mapping(values) => values.values().any(uses_lua),
        _ => false,
    }
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

pub(crate) fn strip_legacy(content: &str) -> Result<String, String> {
    Ok(crate::managed_custom::strip_legacy_patch(
        content,
        "engine/filters/@next",
        FILTER,
        Value::String(FILTER.into()),
    ))
}

pub(crate) fn ensure_remove_filter(root: &Path) -> Result<bool, String> {
    ensure_for_schemas(root, &crate::managed_custom::active_schema_ids(root)?)
}

pub(crate) fn ensure_for_schemas(root: &Path, schemas: &BTreeSet<String>) -> Result<bool, String> {
    let mut changed = false;
    let mut needed = false;
    let mut uncertain = false;
    for id in schemas.iter().filter(|id| id.starts_with("keytao")) {
        let result = (|| -> Result<(), String> {
            let schema_path = crate::packages::safe_path(root, &format!("{id}.schema.yaml"))?;
            let Some(schema) = read_yaml(&schema_path)? else {
                return Err("Active remove-filter schema is unavailable".into());
            };
            let relative = format!("{id}.custom.yaml");
            let path = crate::packages::safe_path(root, &relative)?;
            let content = match fs::read_to_string(&path) {
                Ok(content) => content,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(error) => return Err(error.to_string()),
            };
            let original = crate::managed_custom::set_patch(&content, "remove-filter", None)?;
            let custom: Value =
                serde_yaml::from_str(&original).map_err(|error| error.to_string())?;
            let inherited = schema
                .get("__include")
                .and_then(Value::as_str)
                .or_else(|| schema["engine"]["__include"].as_str())
                .is_some_and(|include| include.starts_with("keytao.schema:"));
            let inherited_lua = inherited
                && read_yaml(&crate::packages::safe_path(root, "keytao.schema.yaml")?)?
                    .is_some_and(|base| uses_lua(&base["engine"]));
            let has_lua =
                uses_lua(&schema["engine"]) || uses_lua(&custom["patch"]) || inherited_lua;
            if has_lua {
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
                    return Err("User-modified remove filter script was preserved".into());
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
            let patch = (has_lua && !original.contains(FILTER))
                .then_some("{engine/filters/@next: lua_filter@*keytao_remove_filter}");
            let updated = crate::managed_custom::set_patch(&original, "remove-filter", patch)?;
            changed |= crate::managed_custom::write_custom(root, &relative, &content, &updated)?;
            Ok(())
        })();
        if let Err(error) = result {
            uncertain = true;
            tracing::warn!(schema = id, %error, "Skipped user dictionary remove filter patch");
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
    fn registration_preserves_custom_bytes_and_adopts_legacy_entries() {
        for original in [
            "",
            "# User comments without final newline",
            "patch:\n  menu/page_size: 9\n  engine/filters/+: [simplifier]\n",
            "# CRLF\r\npatch: {menu/page_size: 9}\r\n",
            "__patch: existing:/patch\npatch: {}\n",
        ] {
            let dir = tempfile::tempdir().unwrap();
            fs::write(
                dir.path().join("keytao.schema.yaml"),
                "engine: {filters: [lua_filter@*existing]}\n",
            )
            .unwrap();
            let path = dir.path().join("keytao.custom.yaml");
            fs::write(&path, original).unwrap();
            assert!(ensure_remove_filter(dir.path()).unwrap());
            let generated = fs::read_to_string(&path).unwrap();
            assert!(generated.contains("# keytao-generated begin v1"));
            assert!(generated.contains(FILTER));
            assert_eq!(crate::managed_custom::strip(&generated).unwrap(), original);
            assert!(!ensure_remove_filter(dir.path()).unwrap());
        }
        assert_eq!(
            strip_legacy(&format!("patch:\n  engine/filters/@next: {FILTER}\n")).unwrap(),
            ""
        );
        let original = "patch:\n  menu/page_size: 9\n";
        assert_eq!(
            strip_legacy(&format!(
                "patch:\n  engine/filters/@next: {FILTER}\n  menu/page_size: 9\n"
            ))
            .unwrap(),
            original
        );
    }

    #[test]
    fn active_set_and_lua_dependency_determine_filter_lifetime() {
        let dir = tempfile::tempdir().unwrap();
        for id in ["keytao", "keytao-dz", "keytao-cx", "xmjd6"] {
            fs::write(
                dir.path().join(format!("{id}.schema.yaml")),
                "engine: {filters: [lua_filter@*existing]}\n",
            )
            .unwrap();
        }
        let ids = BTreeSet::from(["keytao-cx".to_owned()]);
        assert!(ensure_for_schemas(dir.path(), &ids).unwrap());
        assert!(!dir.path().join("keytao.custom.yaml").exists());
        assert!(dir.path().join("keytao-cx.custom.yaml").is_file());
        let script = fs::read_to_string(dir.path().join(SCRIPT_PATH)).unwrap();
        assert!(
            script
                .contains("function M.tags_match(_, env)\n  return next(env.removed) ~= nil\nend")
        );
        fs::write(
            dir.path().join("keytao-cx.schema.yaml"),
            "engine: {filters: [uniquifier]}\n",
        )
        .unwrap();
        assert!(ensure_for_schemas(dir.path(), &ids).unwrap());
        assert!(!dir.path().join("keytao-cx.custom.yaml").exists());
        assert!(!dir.path().join(SCRIPT_PATH).exists());
    }

    #[test]
    fn invalid_user_yaml_is_preserved_and_does_not_block_siblings() {
        let dir = tempfile::tempdir().unwrap();
        for id in ["keytao", "keytao-dz"] {
            fs::write(
                dir.path().join(format!("{id}.schema.yaml")),
                "engine: {filters: [lua_filter@*existing]}\n",
            )
            .unwrap();
        }
        let invalid = "patch: [\n";
        fs::write(dir.path().join("keytao.custom.yaml"), invalid).unwrap();
        assert!(ensure_remove_filter(dir.path()).unwrap());
        assert_eq!(
            fs::read_to_string(dir.path().join("keytao.custom.yaml")).unwrap(),
            invalid
        );
        assert!(dir.path().join("keytao-dz.custom.yaml").is_file());
        assert!(!ensure_remove_filter(dir.path()).unwrap());
    }

    #[test]
    fn inherited_lua_and_user_filter_registrations_are_preserved() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("keytao.schema.yaml"),
            "engine: {filters: [lua_filter@*existing]}\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("keytao-dz.schema.yaml"),
            "engine: {__include: 'keytao.schema:/engine'}\n",
        )
        .unwrap();
        let original =
            format!("# User registration\npatch:\n  engine/filters/+: [{FILTER}] # Keep comment\n");
        fs::write(dir.path().join("keytao.custom.yaml"), &original).unwrap();
        assert!(ensure_remove_filter(dir.path()).unwrap());
        assert_eq!(
            fs::read_to_string(dir.path().join("keytao.custom.yaml")).unwrap(),
            original
        );
        assert!(
            fs::read_to_string(dir.path().join("keytao-dz.custom.yaml"))
                .unwrap()
                .contains(FILTER)
        );
    }

    #[test]
    fn manually_modified_filter_script_is_never_overwritten_or_removed() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("keytao.schema.yaml"),
            "engine: {filters: [lua_filter@*existing]}\n",
        )
        .unwrap();
        fs::create_dir(dir.path().join("lua")).unwrap();
        fs::write(dir.path().join(SCRIPT_PATH), "-- User modifications\n").unwrap();
        assert!(!ensure_remove_filter(dir.path()).unwrap());
        assert_eq!(
            fs::read_to_string(dir.path().join(SCRIPT_PATH)).unwrap(),
            "-- User modifications\n"
        );
        assert!(!crate::managed_custom::strip_file(dir.path(), SCRIPT_PATH).unwrap());
    }

    #[test]
    fn uncertain_active_schemas_keep_the_existing_filter_script() {
        for (schema, custom) in [
            ("engine: [\n", ""),
            ("engine: {filters: [uniquifier]}\n", "patch: [\n"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            fs::create_dir(dir.path().join("lua")).unwrap();
            fs::write(dir.path().join(SCRIPT_PATH), SCRIPT).unwrap();
            fs::write(dir.path().join("keytao.schema.yaml"), schema).unwrap();
            fs::write(dir.path().join("keytao.custom.yaml"), custom).unwrap();
            assert!(
                !ensure_for_schemas(dir.path(), &BTreeSet::from(["keytao".to_owned()])).unwrap()
            );
            assert_eq!(
                fs::read(dir.path().join(SCRIPT_PATH)).unwrap(),
                SCRIPT.as_bytes()
            );
            assert!(ensure_for_schemas(dir.path(), &BTreeSet::new()).unwrap());
            assert!(!dir.path().join(SCRIPT_PATH).exists());
        }
    }
}
