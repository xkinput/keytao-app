use crate::{
    Core, CoreError,
    package_store::{PackageFile, PackageStore, StoredPackage},
    packages::sha256_reader,
};
use keytao_core::isolated_deploy::{InputStat, stable_input_stat};
use serde::Serialize;
use serde_yaml::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufRead, BufReader, Read},
    path::{Component, Path, PathBuf},
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

pub(crate) fn is_schema_config(name: &str) -> bool {
    !name.contains('/')
        && (name.ends_with(".schema.yaml")
            || matches!(
                name,
                "default.yaml" | "default.custom.yaml" | "default-custom.yaml"
            ))
}

pub(crate) fn directory_schemas(root: &Path) -> Result<Vec<(String, Vec<u8>)>, CoreError> {
    let mut files = Vec::new();
    if !root.try_exists()? {
        return Ok(files);
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_schema_config(&name) {
            files.push((
                name.clone(),
                fs::read(crate::packages::safe_path(root, &name).map_err(CoreError::Other)?)?,
            ));
        }
    }
    Ok(files)
}

pub fn package_schemas(core: &Core, key: &str) -> Result<Vec<PackageSchema>, CoreError> {
    let store = PackageStore::new(core);
    let _guard;
    let (stored, paths) = if is_addon_package_key(key) {
        _guard = crate::addon_store::operation();
        store.recover_addon(key)?;
        crate::addon_store::file_inventory(&store.checked_addon_path(key)?, key)?
    } else {
        _guard = core
            .pending_package
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        PackageStore::file_inventory_at(&store.checked_package_path(key)?, key, false)?
    };
    let files = stored
        .files
        .into_iter()
        .zip(paths)
        .filter(|(file, _)| is_schema_config(&file.path))
        .map(|(file, path)| {
            let bytes = PackageStore::read_verified_file(&path, &file)?;
            Ok((file.path, bytes))
        })
        .collect::<Result<Vec<_>, CoreError>>()?;
    enumerate(key, &files, stored.selected_schemas.as_deref())
}

pub(crate) struct PackageWordSupport {
    manifest: InputStat,
    sources: Vec<(PathBuf, Option<InputStat>)>,
    supported: bool,
}

impl PackageWordSupport {
    fn matches(&self, path: &Path) -> bool {
        stable_input_stat(path).is_ok_and(|stat| stat == self.manifest)
            && self.sources.iter().all(|(path, stat)| {
                stat.as_ref()
                    .is_some_and(|stat| stable_input_stat(path).is_ok_and(|current| current == *stat))
            })
    }
}

pub(crate) fn supports_user_words(core: &Core, key: &str) -> Result<bool, CoreError> {
    let key = match key {
        "keytao-dz" | "keytao-bj" => "keytao",
        _ => key,
    };
    let store = PackageStore::new(core);
    let guard = core
        .pending_package
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let path = store.checked_package_path(key)?;
    if !path.try_exists()? {
        drop(guard);
        return crate::user_words::supports_user_words(core, key);
    }
    let metadata = store.checked_metadata_path(key)?;
    let manifest = stable_input_stat(&metadata).ok();
    let (stored, paths) = PackageStore::file_inventory_at(&path, key, false)?;
    let mut cache = core
        .package_word_support
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(cached) = cache.get(key)
        && manifest.as_ref() == Some(&cached.manifest)
        && cached.matches(&metadata)
    {
        return Ok(cached.supported);
    }
    cache.remove(key);

    let base = match key {
        "xmjd" | "xmjd-feiding" => "xmjd6",
        _ => key,
    };
    let mut sources = Vec::new();
    let mut names = Vec::new();
    if let Some((file, path)) = dictionary_source(&stored, &paths, base, ".schema.yaml")? {
        sources.push((path.to_owned(), stable_input_stat(path).ok()));
        let bytes = PackageStore::read_verified_file(path, file)?;
        std::str::from_utf8(&bytes).map_err(|error| CoreError::Other(error.to_string()))?;
        if let Some(name) = parse_header(&header_bytes(bytes.as_slice())?)?
            .get("translator")
            .and_then(|value| value.get("dictionary"))
            .and_then(Value::as_str)
        {
            names.push(name.to_owned());
        }
    }
    names.extend([format!("{base}.extended"), base.into()]);
    let mut supported = false;
    for name in names {
        let Some((file, path)) = dictionary_source(&stored, &paths, &name, ".dict.yaml")? else {
            continue;
        };
        sources.push((path.to_owned(), stable_input_stat(path).ok()));
        let input = fs::File::open(path)?;
        let mut input = BufReader::new(input.take(file.size.saturating_add(1)));
        let header = header_bytes(&mut input)?;
        if sha256_reader(header.as_slice().chain(&mut input))? != file.sha256
            || input.get_ref().limit() != 1
        {
            return Err(CoreError::Other("Package file checksum mismatch".into()));
        }
        let config = parse_header(&header)?;
        if let Some(name) = config
            .get("import_tables")
            .and_then(Value::as_sequence)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .find(|name| name.ends_with(".user"))
        {
            if name.contains('/') {
                return Err(CoreError::Unsupported(
                    "Nested user dictionary paths are not supported".into(),
                ));
            }
            if name.contains('\\')
                || name.chars().any(char::is_whitespace)
                || !Path::new(name)
                    .components()
                    .all(|part| matches!(part, Component::Normal(_)))
            {
                return Err(CoreError::Other(
                    "Invalid user dictionary name in package".into(),
                ));
            }
            dictionary_source(&stored, &paths, name, ".dict.yaml")?;
            supported = true;
        }
        break;
    }
    if let Some(manifest) = manifest {
        let cached = PackageWordSupport {
            manifest,
            sources,
            supported,
        };
        if cached.matches(&metadata) {
            cache.insert(key.to_owned(), cached);
        }
    }
    Ok(supported)
}

fn dictionary_source<'a>(
    stored: &'a StoredPackage,
    paths: &'a [PathBuf],
    name: &str,
    extension: &str,
) -> Result<Option<(&'a PackageFile, &'a Path)>, CoreError> {
    let filename = format!("{name}{extension}");
    let files = stored
        .files
        .iter()
        .zip(paths)
        .map(|(file, path)| (file, path.as_path()));
    if let Some(file) = files.clone().find(|(file, _)| file.path == filename) {
        return Ok(Some(file));
    }
    let suffix = format!("/{filename}");
    let mut matches = files.filter(|(file, _)| file.path.ends_with(&suffix));
    let found = matches.next();
    if matches.next().is_some() {
        return Err(CoreError::Other(format!(
            "Package dictionary path is ambiguous: {name}"
        )));
    }
    Ok(found)
}

fn header_bytes(mut input: impl BufRead) -> Result<Vec<u8>, CoreError> {
    let mut bytes = Vec::new();
    loop {
        let offset = bytes.len();
        if input.read_until(b'\n', &mut bytes)? == 0
            || std::str::from_utf8(&bytes[offset..]).is_ok_and(|line| line.trim() == "...")
        {
            return Ok(bytes);
        }
    }
}

fn parse_header(bytes: &[u8]) -> Result<Value, CoreError> {
    let text = std::str::from_utf8(bytes).map_err(|error| CoreError::Other(error.to_string()))?;
    serde_yaml::from_str(text.trim_start_matches('\u{feff}')).map_err(yaml_error)
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

    #[test]
    fn metadata_checks_consumed_files_without_reading_unrelated_sources() {
        struct Sink;
        impl crate::EventSink for Sink {
            fn emit(&self, _: crate::CoreEvent) {}
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let core = Core::new(
            crate::AppEnv {
                data_dir: root.join("data"),
                cache_dir: root.join("cache"),
                resource_dir: root.join("resources"),
                app_version: "test".into(),
                platform: crate::Platform::Android,
            },
            std::sync::Arc::new(Sink),
        )
        .unwrap();
        let schema = b"schema: {schema_id: xmjd6, name: XMJD}\ntranslator: {dictionary: nested/stock}\n";
        let dictionary = b"---\nname: stock\nimport_tables: [xmjd6.user]\n...\nword\tab\n";
        let files = vec![
            ("xmjd6.schema.yaml".into(), schema.to_vec()),
            ("nested/stock.dict.yaml".into(), dictionary.to_vec()),
            ("unused.dict.yaml".into(), b"original".to_vec()),
        ];
        let archive = crate::packages::archive_from_files(&files).unwrap();
        crate::packages::store_cloud_archive(
            &core,
            "xmjd",
            Some("v1"),
            "official",
            None,
            &archive,
            root,
        )
        .unwrap();
        let files = PackageStore::new(&core).package_path("xmjd").join("files");
        let dictionary_path = files.join("nested/stock.dict.yaml");
        fs::write(files.join("unused.dict.yaml"), b"modified").unwrap();
        assert!(supports_user_words(&core, "xmjd").unwrap());
        assert_eq!(package_schemas(&core, "xmjd").unwrap()[0].id, "xmjd6");

        let mut changed = dictionary.to_vec();
        changed[dictionary.len() - 2] = b'c';
        fs::write(dictionary_path, changed).unwrap();
        assert!(supports_user_words(&core, "xmjd").is_err());
        assert!(package_schemas(&core, "xmjd").is_ok());

        let mut changed = schema.to_vec();
        changed[0] = b'S';
        fs::write(files.join("xmjd6.schema.yaml"), changed).unwrap();
        assert!(package_schemas(&core, "xmjd").is_err());
    }

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
