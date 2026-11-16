use std::{fs, path::Path};

use serde_yaml::Value;

const FILTER: &str = "lua_filter@*keytao_remove_filter";
const SCRIPT: &str = include_str!("keytao_remove_filter.lua");

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

fn register_filter(content: &str, mut custom: Value) -> Result<String, String> {
    if custom.is_null() {
        custom = Value::Mapping(Default::default());
    }
    let mapping = custom.as_mapping_mut().ok_or("Invalid custom mapping")?;
    let has_patch = mapping.contains_key("patch");
    let patch = mapping
        .entry(Value::String("patch".into()))
        .or_insert_with(|| Value::Mapping(Default::default()))
        .as_mapping_mut()
        .ok_or("Invalid custom patch")?;
    if patch
        .get("engine/filters/+")
        .is_some_and(|filters| !filters.is_sequence())
    {
        return Err("Invalid engine/filters/+".into());
    }
    if patch
        .insert(
            Value::String("engine/filters/@next".into()),
            Value::String(FILTER.into()),
        )
        .is_some()
    {
        return Err("Existing engine/filters/@next customization".into());
    }

    let newline = if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut updated = content.to_owned();
    if has_patch {
        let mut offset = 0;
        let header = content.split_inclusive('\n').find(|line| {
            offset += line.len();
            line.strip_prefix("patch:")
                .is_some_and(|tail| tail.trim().is_empty() || tail.trim_start().starts_with('#'))
        });
        header.ok_or("No safe block-style patch header")?;
        let indent = content[offset..]
            .lines()
            .find(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
            .map(|line| line.len() - line.trim_start_matches(' ').len())
            .ok_or("No patch entries")?;
        updated.insert_str(
            offset,
            &format!(
                "{}engine/filters/@next: {FILTER}{newline}",
                " ".repeat(indent)
            ),
        );
    } else {
        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push_str(newline);
        }
        updated.push_str(&format!(
            "patch:{newline}  engine/filters/@next: {FILTER}{newline}"
        ));
    }
    // Reject ambiguous indentation or anchors that would change other values.
    if serde_yaml::from_str::<Value>(&updated).ok().as_ref() != Some(&custom) {
        return Err("Cannot safely insert remove filter registration".into());
    }
    Ok(updated)
}

pub(crate) fn ensure_remove_filter(root: &Path) -> Result<bool, String> {
    let mut changed = false;
    for id in ["keytao", "keytao-dz", "keytao-bj"] {
        let result = (|| -> Result<(), String> {
            let schema_path = crate::packages::safe_path(root, &format!("{id}.schema.yaml"))?;
            let Some(schema) = read_yaml(&schema_path)? else {
                return Ok(());
            };
            let custom_path = crate::packages::safe_path(root, &format!("{id}.custom.yaml"))?;
            let content = match fs::read_to_string(&custom_path) {
                Ok(content) => content,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(error) => return Err(error.to_string()),
            };
            let custom: Value = serde_yaml::from_str(&content)
                .map_err(|error| format!("Invalid {}: {error}", custom_path.display()))?;
            let inherited = schema
                .get("__include")
                .and_then(Value::as_str)
                .or_else(|| schema["engine"]["__include"].as_str())
                .is_some_and(|include| include.starts_with("keytao.schema:"));
            let inherited_lua = if inherited {
                read_yaml(&crate::packages::safe_path(root, "keytao.schema.yaml")?)?
                    .is_some_and(|base| uses_lua(&base["engine"]))
            } else {
                false
            };
            let has_lua =
                uses_lua(&schema["engine"]) || uses_lua(&custom["patch"]) || inherited_lua;
            if !has_lua {
                return Ok(());
            }
            let registration = if content.contains(FILTER) {
                None
            } else {
                Some(register_filter(&content, custom)?)
            };
            let path = crate::packages::safe_path(root, "lua/keytao_remove_filter.lua")?;
            if fs::read(&path).ok().as_deref() != Some(SCRIPT.as_bytes()) {
                fs::create_dir_all(path.parent().unwrap()).map_err(|error| error.to_string())?;
                crate::scheme_files::write_file_atomic(&path, SCRIPT.as_bytes())
                    .map_err(|error| error.to_string())?;
                changed = true;
            }
            if let Some(content) = registration {
                crate::scheme_files::write_file_atomic(&custom_path, content.as_bytes())
                    .map_err(|error| error.to_string())?;
                changed = true;
            }
            Ok(())
        })();
        if let Err(error) = result {
            tracing::warn!(schema = id, %error, "Skipped user dictionary remove filter patch");
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regression_remove_filter_custom_yaml_is_best_effort() {
        for custom in [
            "patch:\n  # menu/page_size: 9\n",
            "patch:\n  engine/filters/+: simplifier\n",
            "patch:\n  menu/page_size: 9\n  menu/page_size: 8\n",
            "patch: [\n",
            "- scalar customization\n",
            "patch: {menu/page_size: 9}\n",
            "patch:\n  engine/filters/@next: simplifier\n",
            "patch: &settings\n  menu/page_size: 9\ncopy: *settings\n",
        ] {
            let dir = tempfile::tempdir().unwrap();
            for id in ["keytao", "keytao-dz"] {
                fs::write(
                    dir.path().join(format!("{id}.schema.yaml")),
                    "engine: {filters: [lua_filter@*existing]}\n",
                )
                .unwrap();
            }
            let custom_path = dir.path().join("keytao.custom.yaml");
            fs::write(&custom_path, custom).unwrap();
            assert!(ensure_remove_filter(dir.path()).unwrap(), "{custom}");
            assert_eq!(fs::read_to_string(&custom_path).unwrap(), custom);
            let sibling = read_yaml(&dir.path().join("keytao-dz.custom.yaml"))
                .unwrap()
                .unwrap();
            assert_eq!(
                sibling["patch"]["engine/filters/@next"].as_str(),
                Some(FILTER)
            );
            assert_eq!(
                fs::read_to_string(dir.path().join("lua/keytao_remove_filter.lua")).unwrap(),
                SCRIPT
            );
            assert!(!ensure_remove_filter(dir.path()).unwrap(), "{custom}");
            assert_eq!(fs::read_to_string(&custom_path).unwrap(), custom);
        }

        for custom in [
            "patch:\n  menu/page_size: 9\n".to_owned(),
            format!("patch:\n  engine/filters/+: [{FILTER}]\n"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            fs::write(
                dir.path().join("keytao.schema.yaml"),
                "engine: {filters: [lua_filter@*existing]}\n",
            )
            .unwrap();
            let custom_path = dir.path().join("keytao.custom.yaml");
            fs::write(&custom_path, &custom).unwrap();
            fs::write(dir.path().join("lua"), "Existing file\n").unwrap();
            assert!(!ensure_remove_filter(dir.path()).unwrap());
            assert_eq!(fs::read_to_string(&custom_path).unwrap(), custom);
            assert_eq!(
                fs::read_to_string(dir.path().join("lua")).unwrap(),
                "Existing file\n"
            );
        }
    }

    #[test]
    fn regression_remove_filter_preserves_original_lines_and_is_idempotent() {
        for original in [
            "# User comments\ncustomization:\n  name: &name 'My setup'\npatch: # User settings\n  # Keep filters and ordering\n  menu/page_size: 9 # Inline comment\n  engine/filters/+: [simplifier]\nmetadata:\n  name: *name\n",
            "# Windows line endings\r\npatch:\r\n    menu/page_size: 9 # Keep this\r\n",
            "patch:\n  menu/page_size: 9 # No final newline",
            "# Empty customization without a final newline",
            "customization: {name: 'Keep this flow mapping'}\n",
        ] {
            let dir = tempfile::tempdir().unwrap();
            fs::write(
                dir.path().join("keytao.schema.yaml"),
                "engine: {filters: [lua_filter@*existing]}\n",
            )
            .unwrap();
            let custom_path = dir.path().join("keytao.custom.yaml");
            fs::write(&custom_path, original).unwrap();
            assert!(ensure_remove_filter(dir.path()).unwrap(), "{original}");
            let patched = fs::read_to_string(&custom_path).unwrap();
            let preserved = patched
                .split_inclusive('\n')
                .filter(|line| !line.contains(FILTER))
                .collect::<String>();
            if original.contains("patch:") {
                assert_eq!(preserved.as_bytes(), original.as_bytes());
            } else {
                let separator = if original.ends_with('\n') { "" } else { "\n" };
                assert_eq!(preserved, format!("{original}{separator}patch:\n"));
                assert!(patched.starts_with(original));
            }
            for line in original.split_inclusive('\n') {
                assert!(patched.contains(line), "Missing original bytes: {line:?}");
            }
            assert!(!ensure_remove_filter(dir.path()).unwrap());
            assert_eq!(fs::read(&custom_path).unwrap(), patched.as_bytes());
            assert_eq!(patched.matches(FILTER).count(), 1);
        }
    }

    #[test]
    fn existing_remove_filter_registration_is_byte_identical() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("keytao.schema.yaml"),
            "engine: {filters: [lua_filter@*existing]}\n",
        )
        .unwrap();
        let original = format!(
            "# Keep legacy registration\r\npatch:\r\n  engine/filters/+: [{FILTER}] # Keep comment\r\n"
        );
        let custom_path = dir.path().join("keytao.custom.yaml");
        fs::write(&custom_path, &original).unwrap();
        assert!(ensure_remove_filter(dir.path()).unwrap());
        assert_eq!(fs::read(&custom_path).unwrap(), original.as_bytes());
        assert!(!ensure_remove_filter(dir.path()).unwrap());
        assert_eq!(fs::read(&custom_path).unwrap(), original.as_bytes());
    }

    #[test]
    fn empty_custom_files_and_inherited_engines_are_supported() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("keytao.schema.yaml"),
            "engine: {filters: [lua_filter@*existing]}\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("keytao.custom.yaml"),
            "# Empty customization\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("keytao-dz.schema.yaml"),
            "engine: {__include: 'keytao.schema:/engine'}\n",
        )
        .unwrap();
        assert!(ensure_remove_filter(dir.path()).unwrap());
        for id in ["keytao", "keytao-dz"] {
            let custom = read_yaml(&dir.path().join(format!("{id}.custom.yaml")))
                .unwrap()
                .unwrap();
            assert_eq!(
                custom["patch"]["engine/filters/@next"].as_str(),
                Some(FILTER)
            );
        }
        assert!(!ensure_remove_filter(dir.path()).unwrap());
    }

    #[test]
    fn registers_only_keytao_lua_schemas_and_preserves_customizations() {
        let dir = tempfile::tempdir().unwrap();
        for id in ["keytao", "keytao-dz", "keytao-bj", "xmjd6"] {
            fs::write(
                dir.path().join(format!("{id}.schema.yaml")),
                "engine:\n  translators: [lua_translator@*existing]\n  filters: [uniquifier]\n",
            )
            .unwrap();
        }
        fs::write(
            dir.path().join("keytao.custom.yaml"),
            "patch:\n  menu/page_size: 9\n  engine/filters/+: [simplifier]\n",
        )
        .unwrap();
        assert!(ensure_remove_filter(dir.path()).unwrap());
        assert!(!ensure_remove_filter(dir.path()).unwrap());
        for id in ["keytao", "keytao-dz", "keytao-bj"] {
            let custom = read_yaml(&dir.path().join(format!("{id}.custom.yaml")))
                .unwrap()
                .unwrap();
            assert_eq!(
                custom["patch"]["engine/filters/@next"].as_str(),
                Some(FILTER)
            );
        }
        let custom = read_yaml(&dir.path().join("keytao.custom.yaml"))
            .unwrap()
            .unwrap();
        assert_eq!(custom["patch"]["menu/page_size"].as_u64(), Some(9));
        assert_eq!(
            custom["patch"]["engine/filters/+"][0].as_str(),
            Some("simplifier")
        );
        assert!(!dir.path().join("xmjd6.custom.yaml").exists());
        assert_eq!(
            fs::read_to_string(dir.path().join("lua/keytao_remove_filter.lua")).unwrap(),
            SCRIPT
        );
    }

    #[test]
    fn lua_free_scheme_does_not_gain_a_lua_dependency() {
        let dir = tempfile::tempdir().unwrap();
        let schema = dir.path().join("keytao.schema.yaml");
        fs::write(&schema, "engine: {filters: [uniquifier]}\n").unwrap();
        assert!(!ensure_remove_filter(dir.path()).unwrap());
        assert!(!dir.path().join("keytao.custom.yaml").exists());
        assert!(!dir.path().join("lua").exists());
        fs::write(&schema, "engine: {filters: [lua_filter@*existing]}\n").unwrap();
        assert!(ensure_remove_filter(dir.path()).unwrap());
        let custom_path = dir.path().join("keytao.custom.yaml");
        let original = fs::read(&custom_path).unwrap();
        fs::write(&schema, "engine: {filters: [uniquifier]}\n").unwrap();
        assert!(!ensure_remove_filter(dir.path()).unwrap());
        assert_eq!(fs::read(&custom_path).unwrap(), original);
    }
}
