use crate::{
    Core, CoreError,
    package_store::{PackageStore, StoredPackage},
};
use serde::Serialize;
use serde_yaml::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

fn yaml_error(error: serde_yaml::Error) -> CoreError {
    CoreError::Other(error.to_string())
}

#[derive(Debug, Serialize)]
pub struct PackageSchema {
    pub id: String,
    pub name: String,
    pub selected: bool,
}

pub(crate) fn default_schemas(files: &[(String, Vec<u8>)]) -> Result<Vec<String>, CoreError> {
    let mut ids = Vec::new();
    for name in ["default.yaml", "default-custom.yaml", "default.custom.yaml"] {
        if let Some((_, bytes)) = files.iter().find(|(path, _)| path == name) {
            let value: Value = serde_yaml::from_slice(bytes).map_err(yaml_error)?;
            let list = if name == "default.yaml" {
                value.get("schema_list")
            } else {
                value
                    .get("patch")
                    .and_then(|patch| patch.get("schema_list"))
            };
            if let Some(list) = list.filter(|value| !value.is_null()) {
                let parsed: Vec<_> = list
                    .as_sequence()
                    .ok_or_else(|| CoreError::Other("schema_list must be a list".into()))?
                    .iter()
                    .filter_map(|entry| entry.get("schema").and_then(Value::as_str))
                    .map(str::to_owned)
                    .collect();
                if !parsed.is_empty() {
                    ids = parsed;
                }
            }
        }
    }
    Ok(ids)
}

fn included_schemas(value: &serde_json::Value, dependencies: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::Object(mapping) => {
            for (key, value) in mapping {
                if key == "__include" {
                    let values = value
                        .as_array()
                        .map(Vec::as_slice)
                        .unwrap_or_else(|| std::slice::from_ref(value));
                    for value in values {
                        if let Some(reference) = value.as_str() {
                            let id = reference
                                .split(':')
                                .next()
                                .unwrap_or_default()
                            .trim_end_matches(".schema.yaml")
                            .trim_end_matches(".schema");
                            if !id.is_empty() {
                                dependencies.insert(id.into());
                            }
                        }
                    }
                }
                included_schemas(value, dependencies);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                included_schemas(value, dependencies);
            }
        }
        _ => {}
    }
}

pub(crate) fn enumerate(
    key: &str,
    files: &[(String, Vec<u8>)],
    selection: Option<&[String]>,
) -> Result<Vec<PackageSchema>, CoreError> {
    let mut defaults = default_schemas(files)?;
    let mut sources = BTreeMap::new();
    let mut dependencies = BTreeSet::new();
    for (path, bytes) in files {
        if path.contains('/') {
            continue;
        }
        if let Some(id) = path.strip_suffix(".schema.yaml") {
            let content =
                std::str::from_utf8(bytes).map_err(|error| CoreError::Other(error.to_string()))?;
            sources.insert(
                id.to_owned(),
                keytao_core::parse_schema_name(content).unwrap_or_else(|| id.into()),
            );
            dependencies.extend(keytao_core::parse_schema_dependencies(content));
            included_schemas(
                &serde_yaml::from_str::<serde_json::Value>(content).unwrap_or_default(),
                &mut dependencies,
            );
        }
    }
    if defaults.is_empty() {
        defaults = if crate::addon_store::IDS.contains(&key) && key != "snow_jiandao" {
            vec![key.into()]
        } else {
            let primary: Vec<_> = sources
                .keys()
                .filter(|id| !dependencies.contains(*id))
                .cloned()
                .collect();
            if primary.is_empty() {
                sources.keys().cloned().collect()
            } else {
                primary
            }
        };
    }
    let mut ids: Vec<_> = defaults
        .iter()
        .filter(|id| sources.contains_key(*id))
        .cloned()
        .collect();
    for id in sources.keys() {
        if !ids.contains(id) {
            ids.push(id.clone());
        }
    }
    let selected: Vec<_> = selection
        .unwrap_or(&defaults)
        .iter()
        .filter(|id| ids.contains(id))
        .cloned()
        .collect();
    let selected = if selected.is_empty() {
        &defaults
    } else {
        &selected
    };
    let mut seen = BTreeSet::new();
    ids.into_iter()
        .filter(|id| seen.insert(id.clone()))
        .map(|id| {
            crate::packages::validate_relative(&id).map_err(CoreError::Other)?;
            Ok(PackageSchema {
                name: sources.get(&id).cloned().unwrap_or_else(|| id.clone()),
                selected: selected.contains(&id),
                id,
            })
        })
        .collect()
}

pub(crate) fn reconcile_selection(
    key: &str,
    files: &[(String, Vec<u8>)],
    selected: &[String],
    previous_paths: &[String],
) -> Result<Vec<String>, CoreError> {
    let schemas = enumerate(key, files, None)?;
    let mut retained: Vec<_> = selected
        .iter()
        .filter(|id| schemas.iter().any(|schema| &schema.id == *id))
        .cloned()
        .collect();
    if retained.is_empty() {
        return Ok(schemas
            .into_iter()
            .filter(|schema| schema.selected)
            .map(|schema| schema.id)
            .collect());
    }
    retained.extend(
        schemas
            .into_iter()
            .filter(|schema| {
                schema.selected
                    && !previous_paths.contains(&format!("{}.schema.yaml", schema.id))
                    && !selected.contains(&schema.id)
            })
            .map(|schema| schema.id),
    );
    Ok(retained)
}

pub(crate) fn filter_files(
    key: &str,
    files: &mut Vec<(String, Vec<u8>)>,
    selection: Option<&[String]>,
) -> Result<Vec<String>, CoreError> {
    let selected: Vec<_> = enumerate(key, files, selection)?
        .into_iter()
        .filter(|schema| schema.selected)
        .map(|schema| schema.id)
        .collect();
    if selected.is_empty() {
        return Err(CoreError::Other("至少选择一个方案".into()));
    }
    let list = Value::Sequence(
        selected
            .iter()
            .map(|id| {
                let mut entry = serde_yaml::Mapping::new();
                entry.insert("schema".into(), id.clone().into());
                Value::Mapping(entry)
            })
            .collect(),
    );
    let mut found = false;
    for (path, bytes) in files
        .iter_mut()
        .filter(|(path, _)| matches!(path.as_str(), "default.custom.yaml" | "default-custom.yaml"))
    {
        let mut value: Value = serde_yaml::from_slice(bytes).map_err(yaml_error)?;
        let patch = value
            .as_mapping_mut()
            .ok_or_else(|| CoreError::Other(format!("{path} must be a mapping")))?
            .entry("patch".into())
            .or_insert_with(|| Value::Mapping(Default::default()))
            .as_mapping_mut()
            .ok_or_else(|| CoreError::Other("patch must be a mapping".into()))?;
        patch.insert("schema_list".into(), list.clone());
        *bytes = serde_yaml::to_string(&value)
            .map_err(yaml_error)?
            .into_bytes();
        found = true;
    }
    if !found {
        files.push(("default.custom.yaml".into(), b"patch: {}".to_vec()));
        return filter_files(key, files, Some(&selected));
    }
    Ok(selected)
}

pub(crate) fn metadata_path(core: &Core, key: &str) -> Result<std::path::PathBuf, CoreError> {
    let store = PackageStore::new(core);
    if crate::addon_store::IDS.contains(&key) && key != "snow_jiandao" {
        Ok(
            crate::packages::safe_path(&store.checked_addon_path(key)?, "package.json")
                .map_err(CoreError::Other)?,
        )
    } else {
        store.checked_metadata_path(key)
    }
}

pub(crate) fn read_selection(path: &Path) -> Result<Option<Vec<String>>, CoreError> {
    let stored: StoredPackage = serde_json::from_slice(&fs::read(path)?)?;
    Ok(stored.selected_schemas)
}

pub(crate) fn directory_schemas(root: &Path) -> Result<Vec<(String, Vec<u8>)>, CoreError> {
    let mut files = Vec::new();
    if !root.try_exists()? {
        return Ok(files);
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".schema.yaml")
            || matches!(
                name.as_str(),
                "default.yaml" | "default.custom.yaml" | "default-custom.yaml"
            )
        {
            files.push((
                name.clone(),
                fs::read(crate::packages::safe_path(root, &name).map_err(CoreError::Other)?)?,
            ));
        }
    }
    Ok(files)
}

fn files(core: &Core, key: &str) -> Result<Vec<(String, Vec<u8>)>, CoreError> {
    if crate::addon_store::IDS.contains(&key) && key != "snow_jiandao" {
        crate::addon_store::source_files(core, key)
            .map_err(CoreError::Other)?
            .into_iter()
            .map(|(source, relative)| {
                Ok((
                    relative.to_string_lossy().replace('\\', "/"),
                    fs::read(source)?,
                ))
            })
            .collect()
    } else {
        Ok(PackageStore::new(core).read_files(key)?.0)
    }
}

pub fn package_schemas(core: &Core, key: &str) -> Result<Vec<PackageSchema>, CoreError> {
    let files = files(core, key)?;
    enumerate(
        key,
        &files,
        read_selection(&metadata_path(core, key)?)?.as_deref(),
    )
}

pub fn is_addon_package_key(key: &str) -> bool {
    crate::addon_store::IDS.contains(&key) && key != "snow_jiandao"
}

pub fn set_package_schemas(core: &Core, key: &str, ids: &[String]) -> Result<(), CoreError> {
    let schemas = package_schemas(core, key)?;
    if ids.is_empty()
        || ids
            .iter()
            .any(|id| !schemas.iter().any(|schema| &schema.id == id))
    {
        return Err(CoreError::Other("请选择至少一个包内方案".into()));
    }
    let path = metadata_path(core, key)?;
    let mut stored: StoredPackage = serde_json::from_slice(&fs::read(&path)?)?;
    stored.selected_schemas = Some(
        schemas
            .into_iter()
            .filter(|schema| ids.contains(&schema.id))
            .map(|schema| schema.id)
            .collect(),
    );
    crate::scheme_files::write_file_atomic(&path, &serde_json::to_vec_pretty(&stored)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<(String, Vec<u8>)> {
        [
            ("alpha.schema.yaml", "schema: {schema_id: alpha, name: Alpha, dependencies: [reverse]}\nengine: {__include: helper.schema:/engine}"),
            ("beta.schema.yaml", "schema: {schema_id: beta, name: Beta}"),
            ("reverse.schema.yaml", "schema: {schema_id: reverse, name: Reverse}"),
            ("helper.schema.yaml", "schema: {schema_id: helper, name: Helper}"),
            ("default.yaml", "schema_list: [{schema: beta}]"),
        ].into_iter().map(|(path, content)| (path.into(), content.as_bytes().to_vec())).collect()
    }

    #[test]
    fn enumeration_lists_all_schema_yaml_files_and_marks_defaults_selected() {
        let mut files = fixture();
        let schemas = enumerate("local-demo", &files, None).unwrap();
        assert_eq!(
            schemas
                .iter()
                .map(|schema| (schema.id.as_str(), schema.name.as_str(), schema.selected))
                .collect::<Vec<_>>(),
            [
                ("beta", "Beta", true),
                ("alpha", "Alpha", false),
                ("helper", "Helper", false),
                ("reverse", "Reverse", false),
            ]
        );
        files.push((
            "default.custom.yaml".into(),
            b"patch: {schema_list: [{schema: alpha}, {schema: reverse}]}".to_vec(),
        ));
        let schemas = enumerate("local-demo", &files, None).unwrap();
        assert_eq!(
            schemas
                .iter()
                .map(|schema| (schema.id.as_str(), schema.selected))
                .collect::<Vec<_>>(),
            [
                ("alpha", true),
                ("reverse", true),
                ("beta", false),
                ("helper", false),
            ]
        );
        files.retain(|(path, _)| !path.starts_with("default"));
        let without_default = enumerate("local-demo", &files, None).unwrap();
        assert_eq!(
            without_default
                .iter()
                .map(|schema| (schema.id.as_str(), schema.selected))
                .collect::<Vec<_>>(),
            [
                ("alpha", true),
                ("beta", true),
                ("helper", false),
                ("reverse", false),
            ]
        );
    }

    #[test]
    fn repeated_patch_keys_preserve_schema_names_and_keep_all_schemas_selectable() {
        let files = vec![
            ("alpha.schema.yaml".into(), b"schema: {schema_id: alpha, name: Alpha, dependencies: [reverse]}\n__patch: first\n__patch: second".to_vec()),
            ("reverse.schema.yaml".into(), b"schema: {schema_id: reverse, name: Reverse}".to_vec()),
        ];
        let schemas = enumerate("local-demo", &files, None).unwrap();
        assert_eq!(
            schemas
                .iter()
                .map(|s| (s.id.as_str(), s.name.as_str(), s.selected))
                .collect::<Vec<_>>(),
            [("alpha", "Alpha", true), ("reverse", "Reverse", false)]
        );
    }

    #[test]
    fn filtering_preserves_sources_and_other_settings() {
        let mut files = fixture();
        files.push((
            "default.custom.yaml".into(),
            b"patch: {menu/page_size: 7}".to_vec(),
        ));
        filter_files("local-demo", &mut files, Some(&["alpha".into()])).unwrap();
        let value: Value = serde_yaml::from_slice(
            &files
                .iter()
                .find(|(path, _)| path == "default.custom.yaml")
                .unwrap()
                .1,
        )
        .unwrap();
        assert_eq!(value["patch"]["menu/page_size"].as_u64(), Some(7));
        assert_eq!(
            value["patch"]["schema_list"][0]["schema"].as_str(),
            Some("alpha")
        );
        assert!(files.iter().any(|(path, _)| path == "reverse.schema.yaml"));
    }

    #[test]
    fn update_drops_removed_ids_adds_new_defaults_and_recovers_empty_selection() {
        let files = fixture();
        let old = vec!["alpha.schema.yaml".into()];
        assert_eq!(
            reconcile_selection(
                "local-demo",
                &files,
                &["alpha".into(), "removed".into()],
                &old
            )
            .unwrap(),
            ["alpha", "beta"]
        );
        assert_eq!(
            reconcile_selection("local-demo", &files, &["removed".into()], &old).unwrap(),
            ["beta"]
        );
        let schemas = enumerate("local-demo", &files, Some(&["removed".into()])).unwrap();
        assert_eq!(
            schemas
                .iter()
                .filter(|schema| schema.selected)
                .map(|schema| schema.id.as_str())
                .collect::<Vec<_>>(),
            ["beta"]
        );
    }

}
