use crate::{valid_rime_build_id, write_file_atomic};
use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path},
};

const RECEIPT_FILE: &str = ".keytao-desktop-inputs.json";
const LEGACY_RECEIPT_FILE: &str = ".keytao-schema-inputs.json";

pub(crate) type SourceSnapshot = BTreeMap<String, InputFile>;

#[derive(Clone, Deserialize, Serialize, PartialEq)]
pub(crate) struct InputFile {
    hash: [u8; 32],
    stat: Option<StatKey>,
    dictionary_header: Option<String>,
}

#[derive(Clone, Deserialize, Serialize, PartialEq)]
struct StatKey {
    size: u64,
    mtime: i64,
    mtime_ns: i64,
    ctime: i64,
    ctime_ns: i64,
    inode: u64,
    device: u64,
}

fn stat_key(metadata: &fs::Metadata) -> Option<StatKey> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(StatKey {
            size: metadata.len(),
            mtime: metadata.mtime(),
            mtime_ns: metadata.mtime_nsec(),
            ctime: metadata.ctime(),
            ctime_ns: metadata.ctime_nsec(),
            inode: metadata.ino(),
            device: metadata.dev(),
        })
    }
    #[cfg(not(unix))]
    {
        // shortcut: without Unix ctime, hash every input until an equally safe key is available.
        let _ = metadata;
        None
    }
}

pub(crate) fn snapshot(root: &Path, shared: &Path) -> Option<SourceSnapshot> {
    fn collect(
        root: &Path,
        directory: &Path,
        files: &mut SourceSnapshot,
        cache: &SourceSnapshot,
    ) -> Option<()> {
        for entry in fs::read_dir(directory).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();
            let kind = entry.file_type().ok()?;
            let name = entry.file_name();
            let name = name.to_str()?;
            if kind.is_dir() {
                if !matches!(name, "build" | "sync")
                    && !name.starts_with('.')
                    && !name.ends_with(".userdb")
                {
                    collect(root, &path, files, cache)?;
                }
            } else if kind.is_file()
                && (name.ends_with(".dict.yaml")
                    || directory == root && (name.ends_with(".yaml") || name.ends_with(".txt")))
            {
                let name = path.strip_prefix(root).ok()?.to_str()?.replace('\\', "/");
                if files.contains_key(&name) {
                    continue;
                }
                let input = input_file(root, &name, cache.get(&name)).ok()??;
                files.insert(name, input);
            }
        }
        Some(())
    }
    let cache = read_staging_file(root, RECEIPT_FILE)
        .ok()
        .flatten()
        .and_then(|bytes| serde_json::from_slice::<Receipt>(&bytes).ok())
        .filter(|saved| shared.to_str() == Some(&saved.shared))
        .map(|saved| saved.files)
        .unwrap_or_default();
    let mut files = BTreeMap::new();
    collect(root, root, &mut files, &cache)?;
    if root != shared {
        collect(shared, shared, &mut files, &cache)?;
    }
    Some(files)
}

#[derive(Deserialize, Serialize)]
struct Receipt {
    version: u8,
    signature: String,
    shared: String,
    selected: BTreeSet<String>,
    schemas: BTreeMap<String, SchemaInputs>,
    default_inputs: [u8; 32],
    #[serde(default)]
    files: SourceSnapshot,
}

#[derive(Deserialize, Serialize, PartialEq)]
struct SchemaInputs {
    hash: [u8; 32],
    dependency_inputs: [u8; 32],
    dependencies: BTreeSet<String>,
    outputs: BTreeSet<String>,
}

pub(crate) fn recorded_schema_ids(root: &Path) -> Option<BTreeSet<String>> {
    let saved: Receipt =
        serde_json::from_slice(&read_staging_file(root, RECEIPT_FILE).ok()??).ok()?;
    Some(saved.schemas.into_keys().collect())
}

pub(crate) fn default_config_is_current(root: &Path, shared: &Path) -> bool {
    let current = || -> Option<bool> {
        read_staging_file(root, "default.yaml").ok()??;
        let saved: Receipt =
            serde_json::from_slice(&read_staging_file(root, RECEIPT_FILE).ok()??).ok()?;
        Some(
            saved.shared == shared.to_str()?
                && saved.default_inputs == default_inputs(root, shared, None)?,
        )
    };
    current() == Some(true)
}

fn default_inputs(
    root: &Path,
    shared: &Path,
    snapshot: Option<&SourceSnapshot>,
) -> Option<[u8; 32]> {
    let mut hash = Sha256::new();
    for name in ["default.yaml", "default.custom.yaml", "default-custom.yaml"] {
        hash.update(name.as_bytes());
        match resolve(root, shared, name, snapshot).ok()? {
            Some(bytes) => {
                hash.update([1]);
                hash.update((bytes.len() as u64).to_le_bytes());
                hash.update(bytes);
            }
            None => hash.update([0]),
        }
    }
    Some(hash.finalize().into())
}

pub(crate) fn changed_schemas(
    root: &Path,
    shared: &Path,
    selected: &[String],
    signature: &str,
) -> Option<Vec<String>> {
    let legacy: BTreeMap<String, [u8; 32]> =
        serde_json::from_slice(&read_staging_file(root, LEGACY_RECEIPT_FILE).ok()??).ok()?;
    // A newly imported legacy database still needs full maintenance's upgrade task.
    for entry in fs::read_dir(root).ok()? {
        if entry.ok()?.file_name().to_str()?.ends_with(".userdb.kct") {
            return None;
        }
    }
    let saved: Receipt =
        serde_json::from_slice(&read_staging_file(root, RECEIPT_FILE).ok()??).ok()?;
    let selected: BTreeSet<_> = selected.iter().cloned().collect();
    if saved.version != 1
        || saved.selected.is_empty()
        || saved.signature != signature
        || saved.shared != shared.to_str()?
        || !saved.selected.is_subset(&selected)
        || (!saved.selected.is_empty() && legacy.is_empty())
        || !selected.iter().all(|id| valid_rime_build_id(id))
        || !saved
            .selected
            .iter()
            .all(|id| saved.schemas.contains_key(id))
    {
        return None;
    }
    let mut changed: BTreeSet<_> = selected.difference(&saved.selected).cloned().collect();
    for (id, previous) in &saved.schemas {
        // Removed dependency sources require the old full-maintenance semantics.
        resolve(root, shared, &format!("{id}.schema.yaml"), None).ok()??;
        if read_staging_file(root, &format!("{id}.schema.yaml"))
            .ok()?
            .is_none()
        {
            changed.insert(id.clone());
            continue;
        }
        let current = schema_inputs(root, shared, id, None, &saved.files)?;
        if current.dependencies != previous.dependencies
            || current.dependency_inputs != previous.dependency_inputs
            || !previous
                .dependencies
                .iter()
                .all(|id| saved.schemas.contains_key(id))
        {
            return None;
        }
        if current != *previous || !outputs_present(root, &current.outputs)? {
            changed.insert(id.clone());
        }
    }
    include_dependency_consumers(&mut changed, &saved.schemas);
    Some(changed.into_iter().collect())
}

fn include_dependency_consumers(
    changed: &mut BTreeSet<String>,
    schemas: &BTreeMap<String, SchemaInputs>,
) {
    // A dependency edit must rebuild its consumers too, including transitive consumers.
    loop {
        let before = changed.len();
        for (id, schema) in schemas {
            if !schema.dependencies.is_disjoint(changed) {
                changed.insert(id.clone());
            }
        }
        if changed.len() == before {
            break;
        }
    }
}

pub(crate) fn changed_schema_inputs(root: &Path, shared: &Path) -> Option<BTreeSet<String>> {
    let saved: Receipt =
        serde_json::from_slice(&read_staging_file(root, RECEIPT_FILE).ok()??).ok()?;
    if saved.shared != shared.to_str()? {
        return None;
    }
    let mut changed = saved
        .schemas
        .iter()
        .filter(|(id, previous)| {
            schema_inputs(root, shared, id, None, &saved.files).as_ref() != Some(previous)
        })
        .map(|(id, _)| id.clone())
        .collect();
    include_dependency_consumers(&mut changed, &saved.schemas);
    Some(changed)
}

pub(crate) fn record(
    root: &Path,
    shared: &Path,
    selected: &[String],
    signature: &str,
    snapshot: Option<&SourceSnapshot>,
) -> Result<(), String> {
    let collect = || -> Option<Receipt> {
        let snapshot = snapshot?;
        let mut pending: BTreeSet<_> = selected.iter().cloned().collect();
        let mut schemas = BTreeMap::new();
        while let Some(id) = pending.pop_first() {
            if schemas.contains_key(&id) {
                continue;
            }
            let inputs = schema_inputs(root, shared, &id, Some(snapshot), snapshot)?;
            if !outputs_present(root, &inputs.outputs)? {
                return None;
            }
            pending.extend(inputs.dependencies.iter().cloned());
            schemas.insert(id, inputs);
        }
        Some(Receipt {
            version: 1,
            signature: signature.into(),
            shared: shared.to_str()?.into(),
            selected: selected.iter().cloned().collect(),
            schemas,
            default_inputs: default_inputs(root, shared, Some(snapshot))?,
            files: snapshot.clone(),
        })
    };
    let path = crate::RuntimeDirs::resolve(root, root)
        .staging_dir
        .join(RECEIPT_FILE);
    let Some(receipt) = collect() else {
        discard_unverified_legacy_inputs(root, shared, snapshot)?;
        return match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        };
    };
    let bytes = serde_json::to_vec(&receipt).map_err(|error| error.to_string())?;
    if read_staging_file(root, RECEIPT_FILE)?.as_deref() != Some(&bytes) {
        write_file_atomic(&path, &bytes).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn discard_unverified_legacy_inputs(
    root: &Path,
    shared: &Path,
    snapshot: Option<&SourceSnapshot>,
) -> Result<(), String> {
    let Some(bytes) = read_staging_file(root, LEGACY_RECEIPT_FILE)? else {
        return Ok(());
    };
    let mut trusted: BTreeMap<String, [u8; 32]> =
        serde_json::from_slice(&bytes).unwrap_or_default();
    let mut untrusted = BTreeSet::new();
    let mut schemas = BTreeMap::new();
    for id in trusted.keys().filter(|id| valid_rime_build_id(id)) {
        let inputs =
            snapshot.and_then(|snapshot| schema_inputs(root, shared, id, Some(snapshot), snapshot));
        if let Some(inputs) = inputs {
            schemas.insert(id.clone(), inputs);
        } else {
            untrusted.insert(id.clone());
        }
    }
    include_dependency_consumers(&mut untrusted, &schemas);
    if !untrusted.is_empty() {
        // complete_deployment may have paired post-compile edits with older build outputs.
        trusted.retain(|id, _| !untrusted.contains(id));
        let bytes = serde_json::to_vec(&trusted).map_err(|error| error.to_string())?;
        let path = crate::RuntimeDirs::resolve(root, root)
            .staging_dir
            .join(LEGACY_RECEIPT_FILE);
        write_file_atomic(&path, &bytes).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn schema_inputs(
    root: &Path,
    shared: &Path,
    id: &str,
    snapshot: Option<&SourceSnapshot>,
    cache: &SourceSnapshot,
) -> Option<SchemaInputs> {
    if !valid_rime_build_id(id) {
        return None;
    }
    let bytes = read_staging_file(root, &format!("{id}.schema.yaml")).ok()??;
    let compiled: Value = serde_yaml::from_slice(&bytes).ok()?;
    if compiled["schema"]["schema_id"].as_str()? != id {
        return None;
    }
    let timestamps = compiled["__build_info"]["timestamps"].as_mapping()?;
    if !timestamps.contains_key(Value::String(format!("{id}.schema"))) {
        return None;
    }
    let mut inputs = Inputs {
        root,
        shared,
        files: BTreeMap::new(),
        dependency_inputs: BTreeMap::new(),
        snapshot,
        cache,
    };
    for (name, timestamp) in timestamps {
        let name = name.as_str()?;
        if !valid_rime_build_id(name) || timestamp.as_u64().is_none() {
            return None;
        }
        inputs.config(&format!("{name}.yaml"), timestamp.as_u64()? != 0)?;
    }
    inputs.config(&format!("{id}.schema.yaml"), true)?;
    inputs.config(&format!("{id}.custom.yaml"), false)?;
    for default in ["default.yaml", "default.custom.yaml", "default-custom.yaml"] {
        inputs.config(default, false)?;
    }
    let mut outputs = BTreeSet::from([format!("{id}.schema.yaml")]);
    inputs.dictionaries(&compiled, &mut outputs)?;
    let dependencies = if compiled["schema"]["dependencies"].is_null() {
        BTreeSet::new()
    } else {
        compiled["schema"]["dependencies"]
            .as_sequence()?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .filter(|id| valid_rime_build_id(id))
                    .map(str::to_owned)
            })
            .collect::<Option<BTreeSet<_>>>()?
    };
    let mut hash = Sha256::new();
    hash.update(b"keytao-desktop-inputs/v1");
    hash.update(bytes);
    hash.update(serde_json::to_vec(&inputs.files).ok()?);
    Some(SchemaInputs {
        hash: hash.finalize().into(),
        dependency_inputs: Sha256::digest(serde_json::to_vec(&inputs.dependency_inputs).ok()?)
            .into(),
        dependencies,
        outputs,
    })
}

struct Inputs<'a> {
    root: &'a Path,
    shared: &'a Path,
    files: BTreeMap<String, Option<[u8; 32]>>,
    dependency_inputs: BTreeMap<String, Vec<String>>,
    snapshot: Option<&'a SourceSnapshot>,
    cache: &'a SourceSnapshot,
}

impl Inputs<'_> {
    fn config(&mut self, name: &str, required: bool) -> Option<()> {
        if self.files.contains_key(name) {
            return Some(());
        }
        let bytes = resolve(self.root, self.shared, name, self.snapshot).ok()?;
        if required && bytes.is_none() {
            return None;
        }
        self.files.insert(name.into(), None);
        if let Some(bytes) = bytes {
            let mut value: Value = serde_yaml::from_slice(&bytes).ok()?;
            let mut dependencies = Vec::new();
            dependency_directives(&value, &mut dependencies)?;
            if !dependencies.is_empty() {
                self.dependency_inputs.insert(name.into(), dependencies);
            }
            self.references(&value)?;
            let hash = if matches!(
                name,
                "default.yaml" | "default.custom.yaml" | "default-custom.yaml"
            ) {
                remove_schema_list(&mut value);
                if value.as_mapping().is_some_and(|map| map.is_empty()) {
                    return Some(());
                }
                Sha256::digest(serde_yaml::to_string(&value).ok()?.as_bytes()).into()
            } else {
                Sha256::digest(bytes).into()
            };
            self.files.insert(name.into(), Some(hash));
        }
        Some(())
    }

    fn references(&mut self, value: &Value) -> Option<()> {
        match value {
            Value::Mapping(map) => {
                for (key, value) in map {
                    match key.as_str()? {
                        "__include" => self.reference(value.as_str()?)?,
                        "__patch" => self.patch(value)?,
                        key if key.starts_with("__") && !matches!(key, "__build_info") => {
                            return None
                        }
                        _ => self.references(value)?,
                    }
                }
            }
            Value::Sequence(values) => {
                for value in values {
                    self.references(value)?;
                }
            }
            Value::Tagged(_) => return None,
            _ => {}
        }
        Some(())
    }

    fn patch(&mut self, value: &Value) -> Option<()> {
        match value {
            Value::String(reference) => self.reference(reference),
            Value::Mapping(_) => self.references(value),
            Value::Sequence(values) => {
                for value in values {
                    self.patch(value)?;
                }
                Some(())
            }
            _ => None,
        }
    }

    fn reference(&mut self, reference: &str) -> Option<()> {
        let Some((file, _)) = reference.split_once(':') else {
            return Some(());
        };
        if file.is_empty() {
            return Some(());
        }
        let id = file.strip_suffix(".yaml").unwrap_or(file);
        if !valid_rime_build_id(id) {
            return None;
        }
        self.config(&format!("{id}.yaml"), !reference.ends_with('?'))
    }

    fn dictionaries(&mut self, value: &Value, outputs: &mut BTreeSet<String>) -> Option<()> {
        match value {
            Value::Mapping(map) => {
                if let Some(dictionary) = value.get("dictionary") {
                    let id = dictionary.as_str()?;
                    if !id.is_empty() {
                        if !valid_rime_build_id(id) {
                            return None;
                        }
                        self.dictionary(id, &mut BTreeSet::new(), true)?;
                        outputs.extend([format!("{id}.table.bin"), format!("{id}.reverse.bin")]);
                        let prism = value.get("prism").map_or(Some(id), Value::as_str)?;
                        if !valid_rime_build_id(prism) {
                            return None;
                        }
                        outputs.insert(format!("{prism}.prism.bin"));
                    }
                }
                if let Some(packs) = value.get("packs") {
                    for pack in packs.as_sequence()? {
                        let id = pack.as_str().filter(|id| valid_rime_build_id(id))?;
                        self.dictionary(id, &mut BTreeSet::new(), true)?;
                        outputs.insert(format!("{id}.table.bin"));
                    }
                }
                for (key, value) in map {
                    if matches!(key.as_str(), Some("__include" | "__patch")) {
                        return None;
                    }
                    self.dictionaries(value, outputs)?;
                }
            }
            Value::Sequence(values) => {
                for value in values {
                    self.dictionaries(value, outputs)?;
                }
            }
            Value::Tagged(_) => return None,
            _ => {}
        }
        Some(())
    }

    fn dictionary(
        &mut self,
        id: &str,
        visiting: &mut BTreeSet<String>,
        primary: bool,
    ) -> Option<()> {
        let name = format!("{id}.dict.yaml");
        if !safe_relative(&name) || !visiting.insert(id.into()) {
            return None;
        }
        if !primary && self.files.contains_key(&name) {
            visiting.remove(id);
            return Some(());
        }
        let input =
            resolve_input(self.root, self.shared, &name, self.cache, self.snapshot).ok()??;
        self.files.insert(name, Some(input.hash));
        let header: Value = serde_yaml::from_str(input.dictionary_header.as_deref()?).ok()?;
        // GetTables uses the header name as its first data source, not the requested filename.
        if primary && header["name"].as_str()? != id {
            return None;
        }
        self.references(&header)?;
        if let Some(imports) = header.get("import_tables") {
            for import in imports.as_sequence()? {
                self.dictionary(import.as_str()?, visiting, false)?;
            }
        }
        let vocabulary = match header.get("vocabulary") {
            Some(value) => Some(value.as_str()?),
            None => None,
        };
        let use_vocabulary = match &header["use_preset_vocabulary"] {
            Value::Bool(value) => *value,
            Value::String(value) if value.eq_ignore_ascii_case("true") => true,
            Value::String(value) if value.eq_ignore_ascii_case("false") => false,
            Value::Null => false,
            _ => return None,
        };
        if use_vocabulary || vocabulary.is_some() {
            let id = vocabulary.filter(|id| !id.is_empty()).unwrap_or("essay");
            if !valid_rime_build_id(id) {
                return None;
            }
            let name = format!("{id}.txt");
            let input =
                resolve_input(self.root, self.shared, &name, self.cache, self.snapshot).ok()??;
            self.files.insert(name, Some(input.hash));
        }
        visiting.remove(id);
        Some(())
    }
}

fn dependency_directives(value: &Value, found: &mut Vec<String>) -> Option<()> {
    match value {
        Value::Mapping(map) => {
            for (key, value) in map {
                let key = key.as_str()?;
                if key == "dependencies" || key.starts_with("schema/dependencies") {
                    found.push(serde_yaml::to_string(value).ok()?);
                }
                dependency_directives(value, found)?;
            }
        }
        Value::Sequence(values) => {
            for value in values {
                dependency_directives(value, found)?;
            }
        }
        Value::Tagged(_) => return None,
        _ => {}
    }
    Some(())
}

fn remove_schema_list(value: &mut Value) {
    let Some(map) = value.as_mapping_mut() else {
        return;
    };
    map.remove(Value::String("schema_list".into()));
    if let Some(patch) = map
        .get_mut(Value::String("patch".into()))
        .and_then(Value::as_mapping_mut)
    {
        patch.retain(|key, _| {
            !key.as_str()
                .is_some_and(|key| key == "schema_list" || key.starts_with("schema_list/"))
        });
        if patch.is_empty() {
            map.remove(Value::String("patch".into()));
        }
    }
}

fn safe_relative(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('\\')
        && !name.contains(':')
        && Path::new(name)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn read_staging_file(root: &Path, name: &str) -> Result<Option<Vec<u8>>, String> {
    let staging = crate::RuntimeDirs::resolve(root, root).staging_dir;
    match fs::symlink_metadata(&staging) {
        Ok(metadata) if metadata.is_dir() => read_regular(&staging, name),
        Ok(_) => Err("non-regular input".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn regular_metadata(root: &Path, name: &str) -> Result<Option<fs::Metadata>, String> {
    if !safe_relative(name) {
        return Err("unsupported input path".into());
    }
    let mut path = root.to_path_buf();
    let components: Vec<_> = Path::new(name).components().collect();
    for (index, component) in components.iter().enumerate() {
        path.push(component.as_os_str());
        match fs::symlink_metadata(&path) {
            Ok(metadata)
                if if index + 1 == components.len() {
                    metadata.is_file()
                } else {
                    metadata.is_dir()
                } =>
            {
                if index + 1 == components.len() {
                    return Ok(Some(metadata));
                }
            }
            Ok(_) => return Err("non-regular input".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("empty input path".into())
}

fn read_regular(root: &Path, name: &str) -> Result<Option<Vec<u8>>, String> {
    if regular_metadata(root, name)?.is_none() {
        return Ok(None);
    }
    #[cfg(test)]
    INPUT_READS.with(|reads| *reads.borrow_mut().entry(root.join(name)).or_default() += 1);
    fs::read(root.join(name))
        .map(Some)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
thread_local! {
    static INPUT_READS: std::cell::RefCell<BTreeMap<std::path::PathBuf, usize>> =
        const { std::cell::RefCell::new(BTreeMap::new()) };
}

fn input_file(
    root: &Path,
    name: &str,
    cached: Option<&InputFile>,
) -> Result<Option<InputFile>, String> {
    let cache_cutoff = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|now| i64::try_from(now.as_secs()).ok())
        .and_then(|now| now.checked_sub(2));
    let Some(metadata) = regular_metadata(root, name)? else {
        return Ok(None);
    };
    let stat = stat_key(&metadata);
    // Coarse timestamps can hide same-tick rewrites, so only cache hashes of older inputs.
    let cache_stat = stat.clone().filter(|stat| {
        cache_cutoff.is_some_and(|cutoff| stat.mtime < cutoff && stat.ctime < cutoff)
    });
    if let Some(cached) = cached.filter(|cached| cache_stat.is_some() && cached.stat == cache_stat)
    {
        return Ok(Some(cached.clone()));
    }
    let bytes = read_regular(root, name)?.ok_or("input disappeared while hashing")?;
    let dictionary_header = if name.ends_with(".dict.yaml") {
        std::str::from_utf8(&bytes).ok().and_then(|text| {
            let end = text.lines().position(|line| line.trim_end() == "...")?;
            Some(text.lines().take(end + 1).collect::<Vec<_>>().join("\n"))
        })
    } else {
        None
    };
    if stat != regular_metadata(root, name)?.as_ref().and_then(stat_key) {
        return Err("input changed while hashing".into());
    }
    Ok(Some(InputFile {
        hash: Sha256::digest(bytes).into(),
        stat: cache_stat,
        dictionary_header,
    }))
}

fn resolve_input(
    root: &Path,
    shared: &Path,
    name: &str,
    cache: &SourceSnapshot,
    snapshot: Option<&SourceSnapshot>,
) -> Result<Option<InputFile>, String> {
    let input = match input_file(root, name, cache.get(name))? {
        Some(input) => Some(input),
        None => input_file(shared, name, cache.get(name))?,
    };
    let content = |file: &InputFile| (file.hash, file.dictionary_header.clone());
    if snapshot.is_some_and(|saved| input.as_ref().map(content) != saved.get(name).map(content)) {
        return Err("deployment inputs changed during compilation".into());
    }
    Ok(input)
}

fn resolve(
    root: &Path,
    shared: &Path,
    name: &str,
    snapshot: Option<&SourceSnapshot>,
) -> Result<Option<Vec<u8>>, String> {
    let bytes = match read_regular(root, name)? {
        Some(bytes) => Some(bytes),
        None => read_regular(shared, name)?,
    };
    // Never pair inputs changed during compilation with an older build.
    if snapshot.is_some_and(|saved| {
        bytes
            .as_ref()
            .map(|bytes| <[u8; 32]>::from(Sha256::digest(bytes)))
            .as_ref()
            != saved.get(name).map(|input| &input.hash)
    }) {
        return Err("deployment inputs changed during compilation".into());
    }
    Ok(bytes)
}

fn outputs_present(root: &Path, outputs: &BTreeSet<String>) -> Option<bool> {
    let dirs = crate::RuntimeDirs::resolve(root, root);
    let mut present = true;
    for output in outputs {
        let path = dirs.staging_dir.join(output);
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_file() => present &= metadata.len() != 0,
            Ok(_) => return None,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => present = false,
            Err(_) => return None,
        }
    }
    Some(present)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    const RECEIPT: &str = "build/.keytao-desktop-inputs.json";
    const LEGACY_RECEIPT: &str = "build/.keytao-schema-inputs.json";

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let fixture = Self(std::env::temp_dir().join(format!(
                "keytao-desktop-inputs-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )));
            fs::create_dir_all(fixture.0.join("build")).unwrap();
            fs::create_dir_all(fixture.0.join("shared")).unwrap();
            fixture.write(
                LEGACY_RECEIPT,
                &serde_json::to_string(&BTreeMap::from([("one", [0u8; 32])])).unwrap(),
            );
            fixture.write("build/default.yaml", "{}\n");
            fixture.write(
                "default.yaml",
                "config_version: '1'\nschema_list: [{schema: one}, {schema: two}]\n",
            );
            for id in ["one", "two"] {
                fixture.schema(id);
            }
            fixture.record();
            fixture
        }

        fn schema(&self, id: &str) {
            self.write(
                &format!("{id}.schema.yaml"),
                &format!("schema: {{schema_id: {id}}}\ntranslator: {{dictionary: {id}}}\n"),
            );
            self.write(&format!("build/{id}.schema.yaml"), &format!("__build_info: {{timestamps: {{{id}.schema: 1, default: 1, default.custom: 0}}}}\nschema: {{schema_id: {id}}}\ntranslator: {{dictionary: {id}}}\n"));
            self.write(&format!("{id}.dict.yaml"), &format!("---\nname: {id}\nversion: '1'\nuse_preset_vocabulary: false\n...\nword\tw\t1\n"));
            for extension in ["table.bin", "reverse.bin", "prism.bin"] {
                self.write(&format!("build/{id}.{extension}"), "binary");
            }
        }

        fn write(&self, name: &str, content: &str) {
            let path = self.0.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }

        fn record(&self) {
            record(
                &self.0,
                &self.0.join("shared"),
                &["one".into(), "two".into()],
                "librime-1",
                snapshot(&self.0, &self.0.join("shared")).as_ref(),
            )
            .unwrap();
        }

        fn changed(&self) -> Option<Vec<String>> {
            changed_schemas(
                &self.0,
                &self.0.join("shared"),
                &["one".into(), "two".into()],
                "librime-1",
            )
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn unchanged_inputs_skip_all_schemas_and_custom_patch_changes_only_one() {
        let fixture = Fixture::new();
        assert_eq!(fixture.changed(), Some(vec![]));
        fixture.write("one.custom.yaml", "patch: {menu/page_size: 8}\n");
        assert_eq!(fixture.changed(), Some(vec!["one".into()]));
        fixture.write("two.user.dict.yaml", "keep words");
        fixture.write("two.userdb/data", "keep database");
        fixture.write("sync/installation/data", "keep sync");
        fixture.record();
        assert_eq!(fixture.changed(), Some(vec![]));
        assert_eq!(
            fs::read(fixture.0.join("two.user.dict.yaml")).unwrap(),
            b"keep words"
        );
        assert!(fixture.0.join("two.userdb/data").is_file());
        assert!(fixture.0.join("sync/installation/data").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn recent_inputs_reject_matching_stats_and_do_not_certify_edits() {
        let fixture = Fixture::new();
        let shared = fixture.0.join("shared");
        let name = "one.dict.yaml";
        let path = fixture.0.join(name);
        let mut saved = snapshot(&fixture.0, &shared).unwrap();
        let bytes = fs::read_to_string(&path).unwrap().replace("word", "edit");
        fixture.write(name, &bytes);
        // Simulate a same-tick stat collision on a filesystem with coarse timestamps.
        saved.get_mut(name).unwrap().stat = stat_key(&fs::metadata(&path).unwrap());
        let current = input_file(&fixture.0, name, saved.get(name))
            .unwrap()
            .unwrap();
        assert_eq!(current.hash, <[u8; 32]>::from(Sha256::digest(&bytes)));
        assert!(current.stat.is_none());
        record(
            &fixture.0,
            &shared,
            &["one".into(), "two".into()],
            "librime-1",
            Some(&saved),
        )
        .unwrap();
        assert!(!fixture.0.join(RECEIPT).exists());
        assert!(fixture.changed().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn unchanged_stats_reuse_dictionary_hashes_without_reading_the_files() {
        let fixture = Fixture::new();
        fixture.write("shared/one.dict.yaml", "shadowed dictionary\n");
        std::thread::sleep(std::time::Duration::from_secs(3));
        fixture.record();
        let path = fixture.0.join("one.dict.yaml");
        let reads = || INPUT_READS.with(|reads| reads.borrow().get(&path).copied().unwrap_or(0));
        assert!(reads() > 0);
        let before = reads();
        let shadowed = fixture.0.join("shared/one.dict.yaml");
        let shadowed_before = INPUT_READS.with(|reads| reads.borrow()[&shadowed]);
        assert_eq!(fixture.changed(), Some(vec![]));
        fixture.record();
        assert_eq!(reads(), before);
        assert_eq!(
            INPUT_READS.with(|reads| reads.borrow()[&shadowed]),
            shadowed_before
        );

        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        let bytes = fs::read_to_string(&path).unwrap().replace("word", "edit");
        fixture.write("one.dict.yaml", &bytes);
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        assert_eq!(fixture.changed(), Some(vec!["one".into()]));
        assert!(reads() > before);
    }

    #[test]
    fn receipts_without_a_file_cache_still_hash_all_inputs() {
        let fixture = Fixture::new();
        let path = fixture.0.join(RECEIPT);
        let mut saved: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        saved.as_object_mut().unwrap().remove("files");
        fs::write(path, serde_json::to_vec(&saved).unwrap()).unwrap();
        let dictionary = fixture.0.join("one.dict.yaml");
        let before = INPUT_READS.with(|reads| reads.borrow()[&dictionary]);
        assert_eq!(fixture.changed(), Some(vec![]));
        fixture.record();
        assert!(INPUT_READS.with(|reads| reads.borrow()[&dictionary]) > before);
    }

    #[test]
    fn inputs_edited_during_deployment_are_not_certified() {
        for (name, before, after) in [
            (
                "one.custom.yaml",
                "patch: {menu/page_size: 3}\n",
                "patch: {menu/page_size: 9}\n",
            ),
            (
                "one.schema.yaml",
                "schema: {schema_id: one}\ntranslator: {dictionary: one}\n",
                "schema: {schema_id: one}\ntranslator: {dictionary: one}\nmenu: {page_size: 9}\n",
            ),
            (
                "one.dict.yaml",
                "---\nname: one\nversion: '1'\n...\nold\tw\t1\n",
                "---\nname: one\nversion: '1'\n...\nnew\tw\t1\n",
            ),
        ] {
            let fixture = Fixture::new();
            fixture.write(name, before);
            fixture.record();
            let compiled = fs::read(fixture.0.join("build/one.schema.yaml")).unwrap();
            let snapshot = snapshot(&fixture.0, &fixture.0.join("shared"));
            let modified = fs::metadata(fixture.0.join(name))
                .unwrap()
                .modified()
                .unwrap();
            fixture.write(name, after);
            fs::File::options()
                .write(true)
                .open(fixture.0.join(name))
                .unwrap()
                .set_modified(modified)
                .unwrap();
            crate::record_schema_input_fingerprints(&fixture.0, &["one".into(), "two".into()])
                .unwrap();
            record(
                &fixture.0,
                &fixture.0.join("shared"),
                &["one".into(), "two".into()],
                "librime-1",
                snapshot.as_ref(),
            )
            .unwrap();
            assert_ne!(fixture.changed(), Some(vec![]), "{name}");
            let fingerprints =
                crate::schema_input_fingerprints(&fixture.0, &["one".into(), "two".into()])
                    .unwrap();
            assert!(fingerprints["one"].is_none(), "{name}");
            assert!(fingerprints["two"].is_some(), "{name}");
            assert_eq!(
                fs::read(fixture.0.join("build/one.schema.yaml")).unwrap(),
                compiled
            );
            assert_eq!(fs::read_to_string(fixture.0.join(name)).unwrap(), after);
            crate::schema_inputs::invalidate_changed_deploy_schemas(
                &fixture.0,
                &["one".into(), "two".into()],
            )
            .unwrap();
            assert!(!fixture.0.join("build/one.schema.yaml").exists());
            assert!(fixture.0.join("build/two.schema.yaml").is_file());
        }
    }

    #[test]
    fn adding_a_package_ignores_schema_list_only_changes() {
        let fixture = Fixture::new();
        fixture.write("three.schema.yaml", "schema: {schema_id: three}\n");
        fixture.write(
            "default.custom.yaml",
            "patch: {schema_list: [{schema: one}, {schema: two}, {schema: three}]}\n",
        );
        assert_eq!(
            changed_schemas(
                &fixture.0,
                &fixture.0.join("shared"),
                &["one".into(), "two".into(), "three".into()],
                "librime-1"
            ),
            Some(vec!["three".into()])
        );
        fixture.write(
            "default.yaml",
            "config_version: '1'\nschema_list: [{schema: one}, {schema: two}, {schema: three}]\n",
        );
        assert_eq!(fixture.changed(), Some(vec![]));
        fixture.write("default.custom.yaml", "patch: {menu/page_size: 9, schema_list: [{schema: one}, {schema: two}, {schema: three}]}\n");
        assert_eq!(fixture.changed(), Some(vec!["one".into(), "two".into()]));
    }

    #[test]
    fn schema_list_only_edits_rebuild_default_config_without_rebuilding_schemas() {
        let fixture = Fixture::new();
        let shared = fixture.0.join("shared");
        assert!(default_config_is_current(&fixture.0, &shared));
        fixture.write(
            "default.custom.yaml",
            "patch: {schema_list: [{schema: two}, {schema: one}]}\n",
        );
        assert_eq!(fixture.changed(), Some(vec![]));
        assert!(!default_config_is_current(&fixture.0, &shared));
        fixture.record();
        assert!(default_config_is_current(&fixture.0, &shared));
        fs::remove_file(fixture.0.join("build/default.yaml")).unwrap();
        assert!(!default_config_is_current(&fixture.0, &shared));
    }

    #[test]
    fn nested_defaults_named_schema_list_remain_inputs() {
        for (before, after) in [
            (
                "patch: {nested: {schema_list: old}}\n",
                "patch: {nested: {schema_list: new}}\n",
            ),
            (
                "patch: {patch: {schema_list: old}}\n",
                "patch: {patch: {schema_list: new}}\n",
            ),
            (
                "__patch: {schema_list: old}\n",
                "__patch: {schema_list: new}\n",
            ),
        ] {
            let fixture = Fixture::new();
            fixture.write("default.custom.yaml", before);
            fixture.record();
            assert_eq!(fixture.changed(), Some(vec![]));
            fixture.write("default.custom.yaml", after);
            assert_eq!(fixture.changed(), Some(vec!["one".into(), "two".into()]));
        }
    }

    #[test]
    fn removing_a_schema_missing_receipt_and_signature_changes_require_full_maintenance() {
        let fixture = Fixture::new();
        let shared = fixture.0.join("shared");
        assert!(changed_schemas(&fixture.0, &shared, &["one".into()], "librime-1").is_none());
        assert!(changed_schemas(
            &fixture.0,
            &shared,
            &["one".into(), "two".into()],
            "librime-2"
        )
        .is_none());
        assert!(changed_schemas(
            &fixture.0,
            &fixture.0,
            &["one".into(), "two".into()],
            "librime-1"
        )
        .is_none());
        fs::remove_file(fixture.0.join(LEGACY_RECEIPT)).unwrap();
        assert!(fixture.changed().is_none());
        fixture.write(
            LEGACY_RECEIPT,
            &serde_json::to_string(&BTreeMap::from([("one", [0u8; 32])])).unwrap(),
        );
        fs::remove_file(fixture.0.join(RECEIPT)).unwrap();
        assert!(fixture.changed().is_none());
        fixture.record();
        fixture.write("old.userdb.kct", "legacy database");
        assert!(fixture.changed().is_none());
    }

    #[test]
    fn missing_compiled_outputs_rebuild_only_their_schema() {
        for output in [
            "one.table.bin",
            "one.reverse.bin",
            "one.prism.bin",
            "one.schema.yaml",
        ] {
            let fixture = Fixture::new();
            fs::remove_file(fixture.0.join("build").join(output)).unwrap();
            assert_eq!(fixture.changed(), Some(vec!["one".into()]), "{output}");
        }
    }

    #[test]
    fn dictionary_imports_packs_and_preset_vocabulary_are_content_inputs() {
        for changed_file in ["cn_dicts/import.dict.yaml", "pack.dict.yaml", "essay.txt"] {
            let fixture = Fixture::new();
            fixture.write("one.dict.yaml", "---\nname: one\nversion: '1'\nuse_preset_vocabulary: true\nimport_tables: [cn_dicts/import]\n...\nword\tw\t1\n");
            fixture.write(
                "cn_dicts/import.dict.yaml",
                "---\nname: imported\nversion: '1'\n...\nword\tw\t1\n",
            );
            fixture.write(
                "pack.dict.yaml",
                "---\nname: pack\nversion: '1'\n...\nword\tw\t1\n",
            );
            fixture.write("essay.txt", "word\t1\n");
            fixture.write("build/pack.table.bin", "binary");
            fixture.write("build/one.schema.yaml", "__build_info: {timestamps: {one.schema: 1, default: 1}}\nschema: {schema_id: one}\ntranslator: {dictionary: one, packs: [pack]}\n");
            fixture.record();
            assert_eq!(fixture.changed(), Some(vec![]));
            let path = fixture.0.join(changed_file);
            let mut bytes = fs::read(&path).unwrap();
            bytes.extend_from_slice(b"# changed\n");
            fs::write(path, bytes).unwrap();
            assert_eq!(
                fixture.changed(),
                Some(vec!["one".into()]),
                "{changed_file}"
            );
        }
    }

    #[test]
    fn primary_and_pack_header_name_mismatches_require_full_maintenance() {
        let fixture = Fixture::new();
        fixture.write("one.dict.yaml", "---\nname: source\nversion: '1'\n...\n");
        fixture.write(
            "source.dict.yaml",
            "---\nname: source\nversion: '1'\n...\nword\tw\t1\n",
        );
        assert!(fixture.changed().is_none());
        fixture.record();
        assert!(!fixture.0.join(RECEIPT).exists());
        fixture.write(
            "source.dict.yaml",
            "---\nname: source\nversion: '1'\n...\nchanged\tw\t1\n",
        );
        assert!(fixture.changed().is_none());

        let fixture = Fixture::new();
        fixture.write(
            "one.dict.yaml",
            "---\nname: one\nversion: '1'\nimport_tables: [pack]\n...\n",
        );
        fixture.write(
            "pack.dict.yaml",
            "---\nname: other_source\nversion: '1'\n...\n",
        );
        fixture.record();
        // Imported table headers do not call GetTables and may have a different name.
        assert_eq!(fixture.changed(), Some(vec![]));
        fixture.write("build/one.schema.yaml", "__build_info: {timestamps: {one.schema: 1}}\nschema: {schema_id: one}\ntranslator: {dictionary: one, packs: [pack]}\n");
        fixture.write("build/pack.table.bin", "binary");
        assert!(fixture.changed().is_none());
        fixture.record();
        assert!(!fixture.0.join(RECEIPT).exists());
    }

    #[test]
    fn vocabulary_flags_match_librime_and_local_references_stay_in_the_current_file() {
        let fixture = Fixture::new();
        fixture.write(
            "one.custom.yaml",
            "patch: {__include: local_patch}\nlocal_patch: {menu/page_size: 8}\n",
        );
        fixture.write(
            "one.dict.yaml",
            "---\nname: one\nversion: '1'\n...\nword\tw\t1\n",
        );
        fixture.record();
        assert_eq!(fixture.changed(), Some(vec![]));
        fixture.write("essay.txt", "word\t1\n");
        assert_eq!(fixture.changed(), Some(vec![]));
        fixture.write(
            "one.dict.yaml",
            "---\nname: one\nversion: '1'\nuse_preset_vocabulary: 'TRUE'\n...\nword\tw\t1\n",
        );
        fixture.record();
        fixture.write("essay.txt", "word\t2\n");
        assert_eq!(fixture.changed(), Some(vec!["one".into()]));
        fixture.write("one.dict.yaml", "---\nname: one\nversion: '1'\nuse_preset_vocabulary: false\nvocabulary: alternate\n...\nword\tw\t1\n");
        fixture.write("shared/alternate.txt", "word\t1\n");
        fixture.record();
        fixture.write("shared/alternate.txt", "word\t2\n");
        assert_eq!(fixture.changed(), Some(vec!["one".into()]));
        fixture.write("alternate.txt", "word\t3\n");
        fixture.record();
        fixture.write("shared/alternate.txt", "word\t4\n");
        assert_eq!(fixture.changed(), Some(vec![]));
    }

    #[test]
    fn include_patch_targets_shared_precedence_and_dependency_changes_are_conservative() {
        let fixture = Fixture::new();
        fixture.write("one.custom.yaml", "patch: {__include: 'preset:/patch'}\n");
        fixture.write("shared/preset.yaml", "patch: {menu/page_size: 6}\n");
        fixture.record();
        assert_eq!(fixture.changed(), Some(vec![]));
        fixture.write("shared/preset.yaml", "patch: {menu/page_size: 7}\n");
        assert_eq!(fixture.changed(), Some(vec!["one".into()]));
        fixture.write("preset.yaml", "patch: {menu/page_size: 8}\n");
        fixture.record();
        fixture.write("shared/preset.yaml", "patch: {menu/page_size: 9}\n");
        assert_eq!(fixture.changed(), Some(vec![]));
        fixture.write("one.custom.yaml", "patch: {schema/dependencies: [two]}\n");
        assert!(fixture.changed().is_none());

        let fixture = Fixture::new();
        fixture.write("build/one.schema.yaml", "__build_info: {timestamps: {one.schema: 1}}\nschema: {schema_id: one, dependencies: [two]}\ntranslator: {dictionary: one}\n");
        fixture.record();
        fixture.write(
            "two.dict.yaml",
            "---\nname: two\nversion: '1'\n...\nedit\tw\t1\n",
        );
        assert_eq!(
            changed_schema_inputs(&fixture.0, &fixture.0.join("shared")),
            Some(BTreeSet::from(["one".into(), "two".into()]))
        );
        fixture.write("two.custom.yaml", "patch: {menu/page_size: 8}\n");
        assert_eq!(fixture.changed(), Some(vec!["one".into(), "two".into()]));
        fs::remove_file(fixture.0.join("two.schema.yaml")).unwrap();
        assert!(fixture.changed().is_none());
    }

    #[test]
    fn unknown_inputs_invalidate_the_receipt_instead_of_certifying_stale_outputs() {
        let fixture = Fixture::new();
        fixture.write(
            "one.custom.yaml",
            "patch: {__include: '../outside:/patch'}\n",
        );
        assert!(fixture.changed().is_none());
        fixture.record();
        assert!(!fixture.0.join(RECEIPT).exists());
        let fixture = Fixture::new();
        fixture.write("one.custom.yaml", "patch: {__include: 'absent:/patch'}\n");
        assert!(fixture.changed().is_none());
        fixture.record();
        assert!(!fixture.0.join(RECEIPT).exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_inputs_and_parent_directories_require_full_maintenance() {
        let fixture = Fixture::new();
        std::os::unix::fs::symlink(
            fixture.0.join("two.dict.yaml"),
            fixture.0.join("one.custom.yaml"),
        )
        .unwrap();
        assert!(fixture.changed().is_none());
        let fixture = Fixture::new();
        std::os::unix::fs::symlink(fixture.0.join("shared"), fixture.0.join("imports")).unwrap();
        fixture.write(
            "one.dict.yaml",
            "---\nname: one\nversion: '1'\nimport_tables: [imports/other]\n...\n",
        );
        assert!(fixture.changed().is_none());
    }
}
