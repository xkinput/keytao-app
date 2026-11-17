use crate::{CoreError, packages::safe_path};
use serde_yaml::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

#[derive(Default)]
struct References {
    schemas: BTreeSet<String>,
    binaries: BTreeSet<String>,
    incomplete: bool,
}

pub(crate) fn changed_dictionary_ids(
    root: &Path,
    files: &[(String, Vec<u8>)],
) -> Result<BTreeSet<String>, CoreError> {
    let dictionaries: BTreeMap<_, _> = files
        .iter()
        .filter_map(|(name, bytes)| {
            name.strip_suffix(".dict.yaml")
                .map(|id| (id.to_owned(), bytes.as_slice()))
        })
        .collect();
    let mut changed = BTreeSet::new();
    let mut dependents: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (id, bytes) in &dictionaries {
        let name = format!("{id}.dict.yaml");
        if read_optional(root, &name)?.as_deref() != Some(*bytes) {
            changed.insert(id.clone());
        }
        // Dictionary entries after the YAML header are tab-separated text.
        let header = std::str::from_utf8(bytes).ok().map(|text| {
            text.lines()
                .take_while(|line| line.trim() != "...")
                .collect::<Vec<_>>()
                .join("\n")
        });
        let value = header.and_then(|text| serde_yaml::from_str::<Value>(&text).ok());
        let Some(value) = value.filter(Value::is_mapping) else {
            changed.insert(id.clone());
            continue;
        };
        if value["import_tables"].is_null() {
            continue;
        }
        let Some(imports) = value["import_tables"].as_sequence() else {
            changed.insert(id.clone());
            continue;
        };
        for import in imports {
            let Some(import) = import.as_str().filter(|id| !id.is_empty()) else {
                changed.insert(id.clone());
                continue;
            };
            let path = safe_path(root, &format!("{import}.dict.yaml")).map_err(CoreError::Other)?;
            if dictionaries.contains_key(import) {
                dependents
                    .entry(import.into())
                    .or_default()
                    .insert(id.clone());
            } else {
                match fs::metadata(path) {
                    Ok(metadata) if metadata.is_file() => {}
                    Ok(_) => {
                        changed.insert(id.clone());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        changed.insert(id.clone());
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }
    }
    let mut pending = changed.clone();
    while let Some(id) = pending.pop_first() {
        for parent in dependents.get(&id).into_iter().flatten() {
            if changed.insert(parent.clone()) {
                pending.insert(parent.clone());
            }
        }
    }
    // Existing platform invalidation also identifies nested sources by basename.
    let aliases: Vec<_> = changed
        .iter()
        .map(|id| id.rsplit('/').next().unwrap().to_owned())
        .collect();
    changed.extend(aliases);
    Ok(changed)
}

/// Removes only compiled outputs no longer referenced by the active schema closure.
pub(crate) fn cleanup(
    root: &Path,
    removed_files: &[(String, Vec<u8>)],
    remaining_schema_ids: &BTreeSet<String>,
) -> Result<Vec<String>, CoreError> {
    let removed_sources: BTreeMap<_, _> = removed_files
        .iter()
        .filter_map(|(name, bytes)| {
            name.strip_suffix(".schema.yaml")
                .map(|id| (id.to_owned(), bytes.as_slice()))
        })
        .collect();
    let removed = references(
        root,
        removed_sources.keys().cloned().collect(),
        &removed_sources,
    )?;
    let remaining = references(root, remaining_schema_ids.clone(), &BTreeMap::new())?;
    let mut candidates: BTreeSet<_> = removed
        .schemas
        .difference(&remaining.schemas)
        .map(|id| format!("build/{id}.schema.yaml"))
        .collect();
    // Missing or invalid active sources cannot prove that a dictionary is unused.
    if !remaining.incomplete {
        candidates.extend(removed.binaries.difference(&remaining.binaries).cloned());
    }
    let paths = candidates
        .into_iter()
        .map(|name| {
            safe_path(root, &name)
                .map(|path| (name, path))
                .map_err(CoreError::Other)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut deleted = Vec::new();
    for (name, _) in paths {
        let path = safe_path(root, &name).map_err(CoreError::Other)?;
        match fs::remove_file(path) {
            Ok(()) => deleted.push(name),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(deleted)
}

fn references(
    root: &Path,
    mut pending: BTreeSet<String>,
    sources: &BTreeMap<String, &[u8]>,
) -> Result<References, CoreError> {
    let mut result = References::default();
    let mut visited = BTreeSet::new();
    while let Some(id) = pending.pop_first() {
        if !visited.insert(id.clone()) {
            continue;
        }
        safe_path(root, &format!("build/{id}.schema.yaml")).map_err(CoreError::Other)?;
        result.schemas.insert(id.clone());
        let source = match sources.get(&id) {
            Some(bytes) => Some(bytes.to_vec()),
            None => read_optional(root, &format!("{id}.schema.yaml"))?,
        };
        let compiled = read_optional(root, &format!("build/{id}.schema.yaml"))?;
        let mut parsed = false;
        for (bytes, compiled) in [(source, false), (compiled, true)] {
            let Some(bytes) = bytes else {
                continue;
            };
            let Ok(value) = serde_yaml::from_slice::<Value>(&bytes) else {
                continue;
            };
            if !value.is_mapping()
                || (compiled && value["schema"]["schema_id"].as_str() != Some(&id))
            {
                continue;
            }
            parsed = true;
            if let Some(schema_id) = value["schema"]["schema_id"].as_str() {
                safe_path(root, &format!("build/{schema_id}.schema.yaml"))
                    .map_err(CoreError::Other)?;
                result.schemas.insert(schema_id.into());
            }
            collect_value(root, &value, &mut pending, &mut result.binaries)?;
        }
        result.incomplete |= !parsed;
    }
    Ok(result)
}

fn read_optional(root: &Path, name: &str) -> Result<Option<Vec<u8>>, CoreError> {
    let path = safe_path(root, name).map_err(CoreError::Other)?;
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn collect_value(
    root: &Path,
    value: &Value,
    pending: &mut BTreeSet<String>,
    binaries: &mut BTreeSet<String>,
) -> Result<(), CoreError> {
    match value {
        Value::Mapping(items) => {
            for (key, value) in items {
                let key = key.as_str().unwrap_or("").rsplit('/').next().unwrap_or("");
                match key {
                    "dictionary" | "prism" => {
                        if let Some(id) = value.as_str().filter(|id| !id.is_empty()) {
                            let extensions: &[&str] = if key == "dictionary" {
                                &["table", "prism", "reverse"]
                            } else {
                                &["prism"]
                            };
                            for extension in extensions {
                                let name = format!("build/{id}.{extension}.bin");
                                safe_path(root, &name).map_err(CoreError::Other)?;
                                binaries.insert(name);
                            }
                        }
                    }
                    "dependencies" => {
                        if let Some(ids) = value.as_sequence() {
                            for id in ids.iter().filter_map(Value::as_str) {
                                safe_path(root, &format!("{id}.schema.yaml"))
                                    .map_err(CoreError::Other)?;
                                pending.insert(id.into());
                            }
                        }
                    }
                    _ => {}
                }
                collect_value(root, value, pending, binaries)?;
            }
        }
        Value::Sequence(items) => {
            for value in items {
                collect_value(root, value, pending, binaries)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, name: &str, bytes: &[u8]) {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn dictionary_changes_preserve_shared_inputs_when_private_inputs_are_new() {
        let dir = tempfile::tempdir().unwrap();
        let shared = b"---\nname: shared\n...\nword\tabc\t100\n";
        write(dir.path(), "shared.dict.yaml", shared);
        let files = vec![
            ("shared.dict.yaml".into(), shared.to_vec()),
            ("private.dict.yaml".into(), b"name: private\n".to_vec()),
        ];
        assert_eq!(
            changed_dictionary_ids(dir.path(), &files).unwrap(),
            BTreeSet::from(["private".into()])
        );
    }

    #[test]
    fn dictionary_changes_propagate_through_transitive_imports() {
        let dir = tempfile::tempdir().unwrap();
        let files: Vec<(String, Vec<u8>)> = vec![
            (
                "parent.dict.yaml".into(),
                b"name: parent\nimport_tables: [middle]\n".to_vec(),
            ),
            (
                "middle.dict.yaml".into(),
                b"name: middle\nimport_tables: [leaf]\n".to_vec(),
            ),
            (
                "leaf.dict.yaml".into(),
                b"name: leaf\n...\nnew\ta\n".to_vec(),
            ),
        ];
        for (name, bytes) in &files {
            write(dir.path(), name, bytes);
        }
        write(dir.path(), "leaf.dict.yaml", b"name: leaf\n...\nold\ta\n");
        assert_eq!(
            changed_dictionary_ids(dir.path(), &files).unwrap(),
            BTreeSet::from(["parent".into(), "middle".into(), "leaf".into()])
        );
    }

    #[test]
    fn dictionary_import_cycles_terminate_and_propagate_changes() {
        let dir = tempfile::tempdir().unwrap();
        let files: Vec<(String, Vec<u8>)> = vec![
            (
                "a.dict.yaml".into(),
                b"name: a\nimport_tables: [b]\n".to_vec(),
            ),
            (
                "b.dict.yaml".into(),
                b"name: b\nimport_tables: [a]\n".to_vec(),
            ),
        ];
        for (name, bytes) in &files {
            write(dir.path(), name, bytes);
        }
        assert!(
            changed_dictionary_ids(dir.path(), &files)
                .unwrap()
                .is_empty()
        );
        write(dir.path(), "a.dict.yaml", b"name: old\n");
        assert_eq!(
            changed_dictionary_ids(dir.path(), &files).unwrap(),
            BTreeSet::from(["a".into(), "b".into()])
        );
    }

    #[test]
    fn missing_imports_and_invalid_headers_conservatively_invalidate() {
        let dir = tempfile::tempdir().unwrap();
        let files: Vec<(String, Vec<u8>)> = vec![
            (
                "missing.dict.yaml".into(),
                b"name: missing\nimport_tables: [absent]\n".to_vec(),
            ),
            ("invalid.dict.yaml".into(), b"name: [\n".to_vec()),
            (
                "external.dict.yaml".into(),
                b"name: external\nimport_tables: [installed]\n".to_vec(),
            ),
        ];
        for (name, bytes) in &files {
            write(dir.path(), name, bytes);
        }
        write(dir.path(), "installed.dict.yaml", b"name: installed\n");
        assert_eq!(
            changed_dictionary_ids(dir.path(), &files).unwrap(),
            BTreeSet::from(["missing".into(), "invalid".into()])
        );
    }

    #[test]
    fn nested_dictionary_changes_include_the_existing_basename_alias() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![("cn_dicts/a.dict.yaml".into(), b"name: a\n".to_vec())];
        assert_eq!(
            changed_dictionary_ids(dir.path(), &files).unwrap(),
            BTreeSet::from(["cn_dicts/a".into(), "a".into()])
        );
        let unsafe_files = vec![(
            "a.dict.yaml".into(),
            b"name: a\nimport_tables: [../outside]\n".to_vec(),
        )];
        assert!(changed_dictionary_ids(dir.path(), &unsafe_files).is_err());
    }

    #[test]
    fn removes_unused_artifacts_and_preserves_shared_dependencies_and_user_data() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let removed = vec![
            (
                "removed.schema.yaml".into(),
                b"schema: {schema_id: removed, dependencies: [helper]}\ntranslator: {dictionary: shared}\nextra: {dictionary: solo, prism: solo_prism}\n".to_vec(),
            ),
            (
                "helper.schema.yaml".into(),
                b"schema: {schema_id: helper}\ntranslator: {dictionary: helper_dict}\n".to_vec(),
            ),
        ];
        write(root, "active.schema.yaml", b"schema: {schema_id: active, dependencies: [helper]}\ntranslator: {dictionary: shared}\n");
        write(root, "helper.schema.yaml", &removed[1].1);
        for name in ["removed", "helper", "active", "unrelated"] {
            write(root, &format!("build/{name}.schema.yaml"), b"{}");
        }
        for name in ["solo", "shared", "helper_dict", "unrelated"] {
            for extension in ["table", "prism", "reverse"] {
                write(root, &format!("build/{name}.{extension}.bin"), b"compiled");
            }
        }
        write(root, "build/solo_prism.prism.bin", b"compiled");
        write(root, "solo.userdb/user-data", b"user dictionary");
        write(root, "sync/user/solo.userdb.txt", b"sync data");
        let deleted = cleanup(root, &removed, &BTreeSet::from(["active".into()])).unwrap();
        assert_eq!(deleted.len(), 5);
        assert!(deleted.contains(&"build/removed.schema.yaml".into()));
        assert!(deleted.contains(&"build/solo_prism.prism.bin".into()));
        for name in ["helper", "active", "unrelated"] {
            assert!(root.join(format!("build/{name}.schema.yaml")).exists());
        }
        for name in ["shared", "helper_dict", "unrelated"] {
            for extension in ["table", "prism", "reverse"] {
                assert!(root.join(format!("build/{name}.{extension}.bin")).exists());
            }
        }
        assert!(root.join("solo.userdb/user-data").exists());
        assert!(root.join("sync/user/solo.userdb.txt").exists());
    }

    #[test]
    fn missing_or_invalid_active_schema_preserves_dictionary_binaries() {
        for active in [None, Some(b"invalid: [".as_slice())] {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            if let Some(bytes) = active {
                write(root, "active.schema.yaml", bytes);
            }
            write(root, "build/removed.schema.yaml", b"{}");
            write(root, "build/shared.table.bin", b"compiled");
            let removed = vec![(
                "removed.schema.yaml".into(),
                b"translator: {dictionary: shared}".to_vec(),
            )];
            assert_eq!(
                cleanup(root, &removed, &BTreeSet::from(["active".into()])).unwrap(),
                vec!["build/removed.schema.yaml"]
            );
            assert!(root.join("build/shared.table.bin").exists());
        }
    }

    #[test]
    fn removes_artifacts_of_unused_transitive_dependencies() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let removed = vec![(
            "removed.schema.yaml".into(),
            b"schema: {schema_id: removed, dependencies: [helper]}".to_vec(),
        )];
        write(root, "helper.schema.yaml", b"schema: {schema_id: helper, dependencies: [leaf]}\ntranslator: {dictionary: helper_dict}");
        write(
            root,
            "leaf.schema.yaml",
            b"schema: {schema_id: leaf}\ntranslator: {dictionary: leaf_dict}",
        );
        for name in ["removed", "helper", "leaf"] {
            write(root, &format!("build/{name}.schema.yaml"), b"{}");
        }
        for name in ["helper_dict", "leaf_dict"] {
            for extension in ["table", "prism", "reverse"] {
                write(root, &format!("build/{name}.{extension}.bin"), b"compiled");
            }
        }
        assert_eq!(cleanup(root, &removed, &BTreeSet::new()).unwrap().len(), 9);
        assert!(root.join("helper.schema.yaml").exists());
        assert!(root.join("leaf.schema.yaml").exists());
    }

    #[test]
    fn invalid_removed_schema_still_removes_its_named_schema_artifact() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "build/removed.schema.yaml", b"not yaml");
        let removed = vec![("removed.schema.yaml".into(), b"also not yaml".to_vec())];
        assert_eq!(
            cleanup(dir.path(), &removed, &BTreeSet::new()).unwrap(),
            vec!["build/removed.schema.yaml"]
        );
    }

    #[test]
    fn unsafe_dictionary_reference_fails_before_deleting_any_artifact() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "build/removed.schema.yaml", b"{}");
        let removed = vec![(
            "removed.schema.yaml".into(),
            b"translator: {dictionary: ../outside}".to_vec(),
        )];
        assert!(cleanup(dir.path(), &removed, &BTreeSet::new()).is_err());
        assert!(dir.path().join("build/removed.schema.yaml").exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_build_directory_is_never_followed() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "removed.schema.yaml", b"{}");
        std::os::unix::fs::symlink(outside.path(), dir.path().join("build")).unwrap();
        let removed = vec![(
            "removed.schema.yaml".into(),
            b"schema: {schema_id: removed}".to_vec(),
        )];
        assert!(cleanup(dir.path(), &removed, &BTreeSet::new()).is_err());
        assert!(outside.path().join("removed.schema.yaml").exists());
    }
}
