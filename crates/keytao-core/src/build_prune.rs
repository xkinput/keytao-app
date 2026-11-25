//! Reachability of compiled Rime outputs; uncertainty always retains the build.

use crate::RuntimeDirs;
use serde_yaml::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufRead, BufReader, Read},
    path::{Component, Path, PathBuf},
};

const SUFFIXES: [&str; 5] = [
    ".schema.yaml",
    ".prism.bin",
    ".table.bin",
    ".reverse.bin",
    ".correction.bin",
];

fn output_id(name: &str) -> Option<&str> {
    let id = SUFFIXES
        .iter()
        .find_map(|suffix| name.strip_suffix(suffix))?;
    (crate::valid_rime_build_id(id) && !id.contains("userdb") && !id.contains(".user.dict"))
        .then_some(id)
}

pub fn is_rime_compiled_output(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let id = output_id(name);
    if id.is_some_and(|id| {
        let id = id.to_ascii_lowercase();
        id.contains("userdb") || id.contains(".user.dict")
    }) || (name != "default.yaml" && id.is_none()) {
        return false;
    }
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    if name.ends_with(".yaml") {
        let mut bytes = Vec::new();
        if file.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes).is_err()
            || bytes.len() > 8 * 1024 * 1024
        {
            return false;
        }
        return serde_yaml::from_slice::<Value>(&bytes).is_ok_and(|document| {
            let source = id.map_or_else(|| "default".to_owned(), |id| format!("{id}.schema"));
            document["__build_info"]["timestamps"][source.as_str()].as_i64().is_some()
                && id.is_none_or(|id| document["schema"]["schema_id"].as_str() == Some(id))
        });
    }
    let mut header = [0u8; 32];
    if (&file).read_exact(&mut header).is_err() {
        return false;
    }
    [
        (".table.bin", b"Rime::Table/".as_slice()),
        (".prism.bin", b"Rime::Prism/".as_slice()),
        (".reverse.bin", b"Rime::Reverse/".as_slice()),
    ].into_iter().any(|(suffix, magic)| {
        name.ends_with(suffix) && header.strip_prefix(magic).is_some_and(|version| {
            let end = version.iter().position(|&byte| byte == 0).unwrap_or(0);
            end > 0 && version[..end].iter().all(|byte| byte.is_ascii_digit() || *byte == b'.')
        })
    })
}

/// Remove only proven unreachable compiled files after a verified deployment.
/// Unresolved inputs retain every file; failed deletions never block deployment.
pub fn prune_rime_build_artifacts(
    user_dir: &Path,
    shared_dir: Option<&Path>,
) -> Result<usize, String> {
    let dirs = RuntimeDirs::resolve(user_dir, shared_dir.unwrap_or(user_dir))?;
    let planned = (|| {
        let shared_build = shared_build_dir(&dirs, shared_dir.is_some());
        let bytes = fs::read(checked_compiled_file(&dirs.staging_dir, "default.yaml")?).ok()?;
        let default: Value = serde_yaml::from_slice(&bytes).ok()?;
        let schemas = default.get("schema_list")?.as_sequence()?;
        if schemas.is_empty() {
            return None;
        }
        let schemas: Vec<String> = schemas
            .iter()
            .map(|entry| {
                entry
                    .get("schema")?
                    .as_str()
                    .filter(|id| crate::valid_rime_build_id(id))
                    .map(str::to_owned)
            })
            .collect::<Option<_>>()?;
        let outputs = reachable_rime_build_outputs(user_dir, shared_dir, &schemas, false, true)?;
        let mut keep: BTreeSet<String> = outputs
            .iter()
            .filter_map(|name| output_id(name).map(str::to_owned))
            .collect();
        // A dictionary ID also protects a compiled schema with that ID.
        let mut checked: BTreeSet<_> = schemas.into_iter().collect();
        loop {
            let kept_schemas: Vec<_> = keep
                .iter()
                .filter(|id| {
                    !checked.contains(*id)
                        && std::iter::once(dirs.staging_dir.as_path())
                            .chain(shared_build)
                            .any(|build| {
                                fs::symlink_metadata(build.join(format!("{id}.schema.yaml")))
                                    .is_ok()
                            })
                })
                .cloned()
                .collect();
            if kept_schemas.is_empty() {
                break;
            }
            keep.extend(
                reachable_rime_build_outputs(user_dir, shared_dir, &kept_schemas, false, true)?
                    .iter()
                    .filter_map(|name| output_id(name).map(str::to_owned)),
            );
            checked.extend(kept_schemas);
        }
        if !fs::symlink_metadata(&dirs.staging_dir).ok()?.is_dir() {
            return None;
        }
        fs::read_dir(&dirs.staging_dir)
            .ok()?
            .map(|entry| {
                let entry = entry.ok()?;
                let kind = entry.file_type().ok()?;
                let obsolete = kind.is_file()
                    && entry
                        .file_name()
                        .to_str()
                        .and_then(output_id)
                        .is_some_and(|id| !keep.contains(id));
                Some(obsolete.then(|| entry.path()))
            })
            .collect::<Option<Vec<_>>>()
    })();
    let Some(planned) = planned else {
        return Ok(0);
    };
    let mut removed = 0;
    for path in planned.into_iter().flatten() {
        match crate::remove_rime_build_artifact(&path) {
            Ok(deleted) => removed += usize::from(deleted),
            Err(error) => eprintln!("KeyTao: warning: build prune skipped file: {error}"),
        }
    }
    Ok(removed)
}

/// The Windows pre-deploy optimization uses the same graph without requiring
/// dictionary inputs, which may already have been removed with their package.
pub(crate) fn reachable_rime_build_outputs(
    user_dir: &Path,
    shared_dir: Option<&Path>,
    schemas: &[String],
    require_current_inputs: bool,
    resolve_references: bool,
) -> Option<BTreeSet<String>> {
    let dirs = RuntimeDirs::resolve(user_dir, shared_dir.unwrap_or(user_dir)).ok()?;
    let fingerprints = require_current_inputs
        .then(|| crate::schema_input_fingerprints(user_dir, schemas).ok())
        .flatten();
    if require_current_inputs && fingerprints.is_none() {
        return None;
    }
    let mut graph = Graph {
        user: &dirs.user_data_dir,
        shared: (shared_dir.is_some() || dirs.active_package_key.is_some())
            .then_some(dirs.shared_data_dir.as_path()),
        build: &dirs.staging_dir,
        shared_build: shared_build_dir(&dirs, shared_dir.is_some()),
        strict: resolve_references,
        protect_references: require_current_inputs || resolve_references,
        outputs: BTreeSet::new(),
        configs: BTreeSet::new(),
        dictionaries: BTreeSet::new(),
        lua: BTreeSet::new(),
        lua_modules: BTreeSet::new(),
        lua_overrides: BTreeMap::new(),
        uses_lua: false,
    };
    for id in schemas {
        if !crate::valid_rime_build_id(id) {
            return None;
        }
        graph.config(&format!("{id}.schema.yaml"), true)?;
    }
    if let Some(fingerprints) = fingerprints {
        for name in &graph.configs {
            if let Some(id) = name.strip_suffix(".schema.yaml") {
                if !fingerprints.get(id).is_some_and(Option::is_some) {
                    return None;
                }
            }
        }
    }
    if graph.strict {
        graph.lua_sources()?;
    }
    Some(graph.outputs)
}

fn shared_build_dir(dirs: &RuntimeDirs, has_shared: bool) -> Option<&Path> {
    (has_shared || dirs.active_package_key.is_some()).then_some(
        if dirs.user_data_dir == dirs.shared_data_dir {
            dirs.staging_dir.as_path()
        } else {
            dirs.prebuilt_data_dir.as_path()
        },
    )
}

// Reject links at every component instead of following package-controlled paths.
fn read_inside(root: &Path, relative: &str) -> Option<Vec<u8>> {
    fs::read(checked_file(root, relative)?).ok()
}

fn checked_file(root: &Path, relative: &str) -> Option<PathBuf> {
    let mut path = root.to_path_buf();
    for component in Path::new(relative).components() {
        let Component::Normal(component) = component else {
            return None;
        };
        path.push(component);
        if fs::symlink_metadata(&path).ok()?.file_type().is_symlink() {
            return None;
        }
    }
    fs::metadata(&path).ok()?.is_file().then_some(path)
}

fn checked_compiled_file(build: &Path, name: &str) -> Option<PathBuf> {
    fs::symlink_metadata(build).ok()?.is_dir().then_some(())?;
    checked_file(build, name)
}

fn dictionary_header(path: &Path) -> Option<Value> {
    // Bound malformed headers without reading dictionary rows or compiled binaries.
    let mut reader = BufReader::new(fs::File::open(path).ok()?).take(1024 * 1024);
    let mut header = String::new();
    loop {
        let mut line = String::new();
        let count = reader.read_line(&mut line).ok()?;
        if line.trim() == "..." {
            break;
        }
        if reader.limit() == 0 {
            return None;
        }
        if count == 0 {
            break;
        }
        header.push_str(&line);
    }
    serde_yaml::from_str(&header).ok()
}

fn patch_keys_resolved(value: &Value) -> Option<()> {
    match value {
        Value::Mapping(map) => {
            for (key, value) in map {
                // Patch paths require Rime's merge semantics; retain the build instead.
                let key = key.as_str()?;
                if key.contains('/') || matches!(key, "__include" | "__patch") {
                    return None;
                }
                patch_keys_resolved(value)?;
            }
        }
        Value::Sequence(values) => {
            for value in values {
                patch_keys_resolved(value)?;
            }
        }
        Value::Tagged(_) => return None,
        _ => {}
    }
    Some(())
}

fn present(root: &Path, relative: &str) -> bool {
    !matches!(fs::symlink_metadata(root.join(relative)), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
}

struct Graph<'a> {
    user: &'a Path,
    shared: Option<&'a Path>,
    build: &'a Path,
    shared_build: Option<&'a Path>,
    strict: bool,
    protect_references: bool,
    outputs: BTreeSet<String>,
    configs: BTreeSet<String>,
    dictionaries: BTreeSet<String>,
    lua: BTreeSet<String>,
    lua_modules: BTreeSet<String>,
    lua_overrides: BTreeMap<String, Vec<u8>>,
    uses_lua: bool,
}

impl Graph<'_> {
    fn present(&self, name: &str) -> bool {
        self.lua_overrides.contains_key(name)
            || present(self.user, name)
            || self.shared.is_some_and(|root| present(root, name))
    }

    fn resolve(&self, name: &str) -> Option<Vec<u8>> {
        if let Some(bytes) = self.lua_overrides.get(name) {
            return Some(bytes.clone());
        }
        fs::read(self.resolve_path(name)?).ok()
    }

    fn resolve_path(&self, name: &str) -> Option<PathBuf> {
        // A present but unreadable user override must not fall back to shared data.
        if present(self.user, name) {
            return checked_file(self.user, name);
        }
        self.shared.and_then(|root| checked_file(root, name))
    }

    fn compiled_present(&self, name: &str) -> bool {
        present(self.build, name) || self.shared_build.is_some_and(|build| present(build, name))
    }

    fn compiled_path(&self, name: &str) -> Option<PathBuf> {
        if present(self.build, name) {
            return checked_compiled_file(self.build, name);
        }
        self.shared_build
            .and_then(|build| checked_compiled_file(build, name))
    }

    fn config(&mut self, name: &str, schema: bool) -> Option<()> {
        if !self.configs.insert(name.into()) {
            return Some(());
        }
        let bytes = if self.compiled_present(name) {
            fs::read(self.compiled_path(name)?).ok()?
        } else if !schema {
            self.resolve(name)?
        } else {
            return None;
        };
        let value: Value = serde_yaml::from_slice(&bytes).ok()?;
        value.as_mapping()?;
        if schema {
            if self.strict {
                self.keep_config_strings(&value);
            }
            let id = name.strip_suffix(".schema.yaml")?;
            if !crate::valid_rime_build_id(id) || value["schema"]["schema_id"].as_str()? != id {
                return None;
            }
            self.outputs.insert(name.into());
            if let Some(dependencies) = value["schema"].get("dependencies") {
                for dependency in dependencies.as_sequence()? {
                    let id = dependency
                        .as_str()
                        .filter(|id| crate::valid_rime_build_id(id))?;
                    self.config(&format!("{id}.schema.yaml"), true)?;
                }
            }
        }
        self.references(&value, &value)
    }

    fn keep_string(&mut self, value: &str) {
        let normalized = value.strip_prefix("build/").unwrap_or(value);
        let derived = SUFFIXES
            .iter()
            .find_map(|suffix| normalized.strip_suffix(suffix))
            .unwrap_or(normalized);
        for id in [value, derived] {
            if crate::valid_rime_build_id(id) {
                self.outputs
                    .extend(SUFFIXES.iter().map(|suffix| format!("{id}{suffix}")));
            }
        }
    }

    fn keep_config_strings(&mut self, value: &Value) {
        match value {
            Value::String(value) => self.keep_string(value),
            Value::Tagged(value) => self.keep_config_strings(&value.value),
            Value::Sequence(values) => {
                for value in values {
                    self.keep_config_strings(value);
                }
            }
            Value::Mapping(values) => {
                for value in values.values() {
                    self.keep_config_strings(value);
                }
            }
            _ => {}
        }
    }

    fn dictionary(&mut self, id: &str, primary: bool) -> Option<()> {
        let path = Path::new(id);
        if id.is_empty()
            || !path
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
        {
            return None;
        }
        if primary && !crate::valid_rime_build_id(id) {
            return None;
        }
        if !self.dictionaries.insert(id.into()) {
            return Some(());
        }
        if crate::valid_rime_build_id(id) {
            self.outputs
                .extend([format!("{id}.table.bin"), format!("{id}.reverse.bin")]);
            if self.strict {
                self.outputs.insert(format!("{id}.correction.bin"));
            }
        }
        if !self.strict {
            return Some(());
        }
        let name = format!("{id}.dict.yaml");
        if !self.present(&name) {
            return [".table.bin", ".reverse.bin", ".correction.bin"]
                .iter()
                .any(|suffix| self.compiled_path(&format!("{id}{suffix}")).is_some())
                .then_some(());
        }
        let value = dictionary_header(&self.resolve_path(&name)?)?;
        value.as_mapping()?;
        let name = value.get("name")?.as_str()?;
        if crate::valid_rime_build_id(name) {
            self.outputs.extend([
                format!("{name}.table.bin"),
                format!("{name}.reverse.bin"),
                format!("{name}.correction.bin"),
            ]);
        } else if name.is_empty()
            || !Path::new(name)
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
        {
            return None;
        }
        self.references(&value, &value)
    }

    fn references(&mut self, value: &Value, document: &Value) -> Option<()> {
        match value {
            Value::Mapping(map) => {
                if self.strict {
                    if let Some(dependencies) = value.get("dependencies") {
                        for dependency in dependencies.as_sequence()? {
                            let id = dependency
                                .as_str()
                                .filter(|id| crate::valid_rime_build_id(id))?;
                            self.config(&format!("{id}.schema.yaml"), true)?;
                        }
                    }
                }
                if let Some(dictionary) = value.get("dictionary") {
                    let id = dictionary.as_str()?;
                    if !id.is_empty() {
                        self.dictionary(id, true)?;
                        let prism = value.get("prism").map_or(Some(id), Value::as_str)?;
                        if !crate::valid_rime_build_id(prism) {
                            return None;
                        }
                        self.outputs.insert(format!("{prism}.prism.bin"));
                    }
                }
                if let Some(prism) = value.get("prism") {
                    let id = prism.as_str().filter(|id| crate::valid_rime_build_id(id))?;
                    self.outputs.insert(format!("{id}.prism.bin"));
                    if self.strict && self.compiled_path(&format!("{id}.prism.bin")).is_none() {
                        return None;
                    }
                }
                if let Some(corrector) = value.get("corrector") {
                    if let Some(id) = corrector.as_str().filter(|id| !id.is_empty()) {
                        if !crate::valid_rime_build_id(id) {
                            return None;
                        }
                        self.outputs.insert(format!("{id}.correction.bin"));
                        if self.strict {
                            self.dictionary(id, true)?;
                        }
                    } else if !matches!(corrector, Value::Bool(_) | Value::Null | Value::Mapping(_))
                    {
                        return None;
                    }
                }
                for key in ["packs", "import_tables"] {
                    if let Some(values) = value.get(key) {
                        for item in values.as_sequence()? {
                            let id = item.as_str()?;
                            if self.strict || key == "import_tables" {
                                self.dictionary(id, key == "packs")?;
                            } else {
                                if !crate::valid_rime_build_id(id) {
                                    return None;
                                }
                                self.outputs.insert(format!("{id}.table.bin"));
                                if self.protect_references {
                                    self.outputs.insert(format!("{id}.reverse.bin"));
                                }
                            }
                        }
                    }
                }
                for (key, value) in map {
                    if self.strict
                        && key
                            .as_str()
                            .is_some_and(|key| key == "translator" || key.ends_with("_translator"))
                        && value.get("prism").is_none()
                    {
                        if let Some(id) = value
                            .get("dictionary")
                            .and_then(Value::as_str)
                            .filter(|id| !id.is_empty())
                        {
                            self.compiled_path(&format!("{id}.prism.bin"))?;
                        }
                    }
                    match key.as_str().unwrap_or_default() {
                        "__include" => self.directive(value, document, false)?,
                        "__patch" => match value {
                            Value::Mapping(_) => {
                                patch_keys_resolved(value)?;
                                self.references(value, document)?;
                            }
                            Value::Sequence(values) => {
                                for value in values {
                                    self.directive(value, document, true)?;
                                }
                            }
                            _ => self.directive(value, document, true)?,
                        },
                        "__build_info" => {}
                        _ => self.references(value, document)?,
                    }
                }
            }
            Value::Sequence(values) => {
                for value in values {
                    self.references(value, document)?;
                }
            }
            Value::String(value) if self.strict && value.starts_with("lua_") => {
                self.uses_lua = true;
                if let Some(module) = value
                    .split_once("@*")
                    .map(|(_, name)| name.split('*').next().unwrap_or_default())
                {
                    self.lua_modules.insert(module.into());
                }
            }
            Value::Tagged(_) => return None,
            _ => {}
        }
        Some(())
    }

    fn directive(&mut self, value: &Value, document: &Value, patch: bool) -> Option<()> {
        if !self.strict {
            return None;
        }
        let reference = value.as_str()?;
        let (file, section) = reference.split_once(':').unwrap_or(("", reference));
        let external;
        let target = if file.is_empty() {
            document
        } else {
            let file = file.strip_suffix(".yaml").unwrap_or(file);
            if !crate::valid_rime_build_id(file) {
                return None;
            }
            let name = format!("{file}.yaml");
            self.config(&name, file.ends_with(".schema"))?;
            let bytes = if self.compiled_present(&name) {
                fs::read(self.compiled_path(&name)?).ok()?
            } else {
                self.resolve(&name)?
            };
            external = serde_yaml::from_slice::<Value>(&bytes).ok()?;
            &external
        };
        let mut target = target;
        for key in section
            .trim_start_matches('/')
            .trim_end_matches('?')
            .split('/')
            .filter(|part| !part.is_empty())
        {
            target = target.get(key)?;
        }
        if patch {
            patch_keys_resolved(target)?;
        }
        Some(())
    }

    fn lua_module(&mut self, name: &str) -> Option<()> {
        if name.is_empty()
            || !name.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/')
            })
        {
            return None;
        }
        let module = name.replace('.', "/");
        let path = format!("lua/{module}.lua");
        if self.present(&path) {
            self.lua_file(&path)
        } else {
            self.lua_file(&format!("lua/{module}/init.lua"))
        }
    }

    fn lua_sources(&mut self) -> Option<()> {
        let modules = self.lua_modules.clone();
        let mut resolved = BTreeSet::new();
        if self.present("rime.lua") {
            self.lua_file("rime.lua")?;
        }
        for module in &modules {
            let path = module.replace('.', "/");
            if self.present(&format!("lua/{path}.lua"))
                || self.present(&format!("lua/{path}/init.lua"))
            {
                self.lua_module(module)?;
                resolved.insert(module.clone());
            }
        }
        let mut saw_lua = !self.lua.is_empty();
        if present(self.user, crate::ACTIVE_SCHEME_MANIFEST) {
            let bytes = read_inside(self.user, crate::ACTIVE_SCHEME_MANIFEST)?;
            let manifest: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
            if let Some(packages) = manifest.get("packages") {
                for (key, package) in packages.as_object()? {
                    let selected = package.get("schemas")?.as_array()?.iter().any(|id| {
                        id.as_str()
                            .is_some_and(|id| self.configs.contains(&format!("{id}.schema.yaml")))
                    });
                    if !selected {
                        continue;
                    }
                    self.lua.clear();
                    self.lua_overrides.clear();
                    if let Some(files) = package.get("lua_files") {
                        for (name, source) in files.as_object()? {
                            let source = source.as_str()?;
                            if !name.starts_with("lua/")
                                || !source.starts_with(&format!(".keytao-package-lua/{key}/lua/"))
                            {
                                return None;
                            }
                            self.lua_overrides
                                .insert(name.clone(), read_inside(self.user, source)?);
                        }
                    }
                    match package.get("rime_lua") {
                        Some(serde_json::Value::String(source)) => {
                            self.lua.insert("rime.lua".into());
                            self.lua_source(source)?;
                        }
                        None | Some(serde_json::Value::Null) => {}
                        _ => return None,
                    }
                    for module in &modules {
                        let path = module.replace('.', "/");
                        if self.lua_overrides.contains_key(&format!("lua/{path}.lua"))
                            || self
                                .lua_overrides
                                .contains_key(&format!("lua/{path}/init.lua"))
                        {
                            self.lua_module(module)?;
                            resolved.insert(module.clone());
                        }
                    }
                    saw_lua |= !self.lua.is_empty();
                }
            }
        }
        (resolved.len() == modules.len() && (!self.uses_lua || saw_lua)).then_some(())
    }

    fn lua_file(&mut self, name: &str) -> Option<()> {
        if !self.lua.insert(name.into()) {
            return Some(());
        }
        let bytes = self.resolve(name)?;
        self.lua_source(std::str::from_utf8(&bytes).ok()?)
    }

    fn lua_source(&mut self, source: &str) -> Option<()> {
        let tokens = lua_tokens(source)?;
        for (index, token) in tokens.iter().enumerate() {
            if let Token::String(value) = token {
                self.keep_string(value);
            }
            if matches!(token, Token::Name(name) if matches!(name.as_str(), "loadstring" | "getfenv" | "setfenv"))
            {
                return None;
            }
            if matches!(token, Token::String(name) if matches!(name.as_str(), "ReverseDb" | "ReverseLookup" | "require" | "dofile" | "loadfile"))
                || matches!(token, Token::Name(name) if matches!(name.as_str(), "_G" | "_ENV") && tokens.get(index + 1) == Some(&Token::Symbol('[')))
            {
                return None;
            }
            let Token::Name(api) = token else {
                continue;
            };
            if !matches!(
                api.as_str(),
                "ReverseDb" | "ReverseLookup" | "require" | "dofile" | "loadfile"
            ) {
                continue;
            }
            let mut argument = index + 1;
            let parenthesized = tokens.get(argument) == Some(&Token::Symbol('('));
            argument += usize::from(parenthesized);
            let reverse = matches!(api.as_str(), "ReverseDb" | "ReverseLookup");
            let Some(Token::String(value)) = tokens.get(argument) else {
                if reverse {
                    continue;
                }
                return None;
            };
            if parenthesized && tokens.get(argument + 1) != Some(&Token::Symbol(')')) {
                if reverse {
                    continue;
                }
                return None;
            }
            match api.as_str() {
                "require" => self.lua_module(value)?,
                "dofile" | "loadfile" => self.lua_file(value)?,
                "ReverseLookup" => self.dictionary(value, true)?,
                _ => {
                    let id = value.strip_prefix("build/")?.strip_suffix(".reverse.bin")?;
                    self.dictionary(id, true)?;
                }
            }
        }
        Some(())
    }
}

#[derive(Debug, PartialEq)]
enum Token {
    Name(String),
    String(String),
    Symbol(char),
}

fn lua_tokens(source: &str) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    let bytes = source.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        if byte.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let comment = bytes.get(i..i + 2) == Some(b"--");
        if comment {
            i += 2;
        }
        if bytes.get(i) == Some(&b'[') {
            let start = i;
            i += 1;
            while bytes.get(i) == Some(&b'=') {
                i += 1;
            }
            if bytes.get(i) == Some(&b'[') {
                let close = format!("]{}]", "=".repeat(i - start - 1));
                let content = i + 1;
                let end = source.get(content..)?.find(&close)? + content;
                if !comment {
                    // Lua ignores one newline immediately after a long string opens.
                    let value = &source[content..end];
                    let value = value
                        .strip_prefix("\r\n")
                        .or_else(|| value.strip_prefix("\n\r"))
                        .or_else(|| value.strip_prefix('\n'))
                        .or_else(|| value.strip_prefix('\r'))
                        .unwrap_or(value);
                    tokens.push(Token::String(value.into()));
                }
                i = end + close.len();
                continue;
            }
            i = start;
        }
        if comment {
            while i < bytes.len() && !matches!(bytes[i], b'\n' | b'\r') {
                i += 1;
            }
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            let quote = byte;
            i += 1;
            let mut value = Vec::new();
            while i < bytes.len() && bytes[i] != quote {
                if bytes[i] == b'\\' {
                    i += 1;
                    let escaped = *bytes.get(i)?;
                    match escaped {
                        b'n' => value.push(b'\n'),
                        b'r' => value.push(b'\r'),
                        b't' => value.push(b'\t'),
                        b'a' => value.push(7),
                        b'b' => value.push(8),
                        b'f' => value.push(12),
                        b'v' => value.push(11),
                        b'\\' | b'\'' | b'"' => value.push(escaped),
                        b'\n' => value.push(b'\n'),
                        b'z' => {
                            i += 1;
                            while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
                                i += 1;
                            }
                            continue;
                        }
                        b'x' => {
                            value.push(u8::from_str_radix(source.get(i + 1..i + 3)?, 16).ok()?);
                            i += 2;
                        }
                        b'0'..=b'9' => {
                            let start = i;
                            while i < start + 3 && bytes.get(i).is_some_and(u8::is_ascii_digit) {
                                i += 1;
                            }
                            value.push(source.get(start..i)?.parse::<u8>().ok()?);
                            continue;
                        }
                        _ => return None,
                    }
                } else {
                    value.push(bytes[i]);
                }
                i += 1;
            }
            if i == bytes.len() {
                return None;
            }
            tokens.push(Token::String(String::from_utf8(value).ok()?));
            i += 1;
        } else if byte.is_ascii_alphabetic() || byte == b'_' {
            let start = i;
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            tokens.push(Token::Name(source[start..i].into()));
        } else {
            tokens.push(Token::Symbol(byte as char));
            i += 1;
        }
    }
    Some(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "build-prune-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(root.join("build")).unwrap();
            let fixture = Self(root);
            fixture.write("build/default.yaml", "schema_list: [{schema: active}]\n");
            fixture.schema("active", "translator: {dictionary: main}\n");
            fixture.dictionary("main", "");
            fixture
        }
        fn write(&self, name: &str, content: &str) {
            let path = self.0.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
        fn schema(&self, id: &str, extra: &str) {
            self.write(
                &format!("build/{id}.schema.yaml"),
                &format!("schema: {{schema_id: {id}}}\n{extra}"),
            );
        }
        fn dictionary(&self, id: &str, extra: &str) {
            self.write(
                &format!("{id}.dict.yaml"),
                &format!("---\nname: {id}\n{extra}...\nword\tw\t1\n"),
            );
            for suffix in &SUFFIXES[1..] {
                self.write(&format!("build/{id}{suffix}"), "compiled");
            }
        }
        fn prune(&self) -> usize {
            prune_rime_build_artifacts(&self.0, None).unwrap()
        }
        fn exists(&self, name: &str) -> bool {
            self.0.join(name).exists()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    use std::path::PathBuf;

    #[test]
    fn deselected_and_uninstalled_outputs_are_removed_without_user_data() {
        let f = Fixture::new();
        f.schema("deselected", "translator: {dictionary: deselected}\n");
        f.dictionary("deselected", "");
        f.dictionary("uninstalled", "");
        fs::remove_file(f.0.join("uninstalled.dict.yaml")).unwrap();
        let preserved = [
            "default.yaml",
            "installation.yaml",
            "user.yaml",
            "active.custom.yaml",
            "deselected.user.dict.yaml",
            "deselected.userdb/data",
            "sync/device/words",
            "build/default.yaml",
            "build/symbols.yaml",
            "build/old.userdb.table.bin",
            "build/old.user.dict.table.bin",
            "build/deselected.userdb/data",
            "build/unknown.bin",
        ];
        for name in preserved {
            if name != "build/default.yaml" {
                f.write(name, "personal");
            }
        }
        assert_eq!(f.prune(), 9);
        for name in preserved {
            assert!(f.exists(name), "{name}");
        }
        assert!(f.exists("build/active.schema.yaml"));
        assert!(f.exists("build/main.table.bin"));
        assert!(!f.exists("build/deselected.schema.yaml"));
        assert!(!f.exists("build/uninstalled.reverse.bin"));
    }

    #[test]
    fn shared_dictionary_and_dependency_survive_until_last_consumer_leaves() {
        let f = Fixture::new();
        f.schema("active", "translator: {dictionary: shared}\n");
        f.dictionary("shared", "");
        f.schema("dependency", "translator: {dictionary: dependent}\n");
        f.dictionary("dependent", "");
        f.write("build/active.schema.yaml", "schema: {schema_id: active, dependencies: [dependency]}\ntranslator: {dictionary: shared}\n");
        f.schema("removed", "translator: {dictionary: shared}\n");
        assert_eq!(f.prune(), 5);
        assert!(f.exists("build/dependency.schema.yaml"));
        assert!(f.exists("build/dependent.table.bin"));
        assert!(f.exists("build/shared.table.bin"));
        f.schema("active", "");
        assert!(f.prune() > 0);
        assert!(!f.exists("build/shared.table.bin"));
        assert!(!f.exists("build/dependency.schema.yaml"));
    }

    #[test]
    fn dictionaries_imports_packs_prisms_correctors_and_compiled_directives_are_kept() {
        let f = Fixture::new();
        f.dictionary("obsolete", "");
        f.schema("active", "translator: {dictionary: main, packs: [pack], prism: custom_prism, corrector: correction}\nextra_translator: {dictionary: extra}\nreverse_lookup: {dictionary: reverse}\nother_lookup: {dictionary: lookup}\nmenu: {__include: 'dependency.schema:/menu'}\nspeller: {__patch: ['patches:/speller']}\npunctuator: {full_shape: {'/': ['/', '、'], 1: ['①']}}\n");
        f.dictionary("main", "import_tables: [imports/words]\n");
        f.dictionary("imports/words", "import_tables: [nested]\n");
        for id in [
            "pack",
            "extra",
            "reverse",
            "lookup",
            "correction",
            "nested",
            "patched",
        ] {
            f.dictionary(id, "");
        }
        f.write("build/custom_prism.prism.bin", "compiled");
        f.schema("dependency", "menu: {page_size: 9}\n");
        f.write("build/patches.yaml", "speller: {dictionary: patched}\n");
        assert_eq!(f.prune(), 4);
        for id in [
            "main",
            "pack",
            "extra",
            "reverse",
            "lookup",
            "correction",
            "nested",
            "patched",
        ] {
            assert!(f.exists(&format!("build/{id}.table.bin")), "{id}");
        }
        assert!(f.exists("build/dependency.schema.yaml"));
    }

    #[test]
    fn compiled_include_schema_dependencies_are_kept_transitively() {
        let f = Fixture::new();
        f.dictionary("obsolete", "");
        f.write("build/active.schema.yaml", "schema: {schema_id: active, __include: 'common:/schema'}\ntranslator: {dictionary: main}\n");
        f.write("build/common.yaml", "schema: {dependencies: [legacy]}\n");
        f.write(
            "build/legacy.schema.yaml",
            "schema: {schema_id: legacy, dependencies: [transitive]}\n",
        );
        f.dictionary("legacy", "");
        f.schema("transitive", "");
        assert_eq!(f.prune(), 4);
        assert!(f.exists("build/legacy.schema.yaml"));
        assert!(f.exists("build/legacy.reverse.bin"));
        assert!(f.exists("build/transitive.schema.yaml"));
        assert!(!f.exists("build/obsolete.table.bin"));
    }

    #[test]
    fn keytao_dynamic_reverse_db_and_arithmetic_load_allow_pruning() {
        let f = Fixture::new();
        f.schema("removed", "translator: {dictionary: removed}\n");
        f.dictionary("removed", "");
        f.schema(
            "active",
            "engine: {filters: [lua_filter@keytao_filter]}\ntranslator: {dictionary: main}\n",
        );
        f.write(
            "rime.lua",
            "keytao_filter = require('keytao_filter')\nrequire('xnumber')\n",
        );
        f.write("lua/keytao_filter.lua", "local dict = env.engine.schema.config:get_string('translator/dictionary')\nenv.reverse_path = 'build/' .. dict .. '.reverse.bin'\nlocal db = ReverseDb(env.reverse_path)\nlocal lookup = ReverseLookup(dict)\n");
        f.write(
            "lua/xnumber.lua",
            "return function(expression) return load('return ' .. expression)() end\n",
        );
        assert_eq!(f.prune(), 5);
        assert!(!f.exists("build/removed.schema.yaml"));
        for suffix in &SUFFIXES[1..] {
            assert!(!f.exists(&format!("build/removed{suffix}")));
            assert!(f.exists(&format!("build/main{suffix}")));
        }
        assert!(f.exists("build/active.schema.yaml"));
    }

    #[test]
    fn arbitrary_lua_string_literals_protect_bare_and_derived_ids() {
        for literal in [
            "literal",
            "build/literal.reverse.bin",
            "literal.table.bin",
            "literal.prism.bin",
            "literal.schema.yaml",
        ] {
            let f = Fixture::new();
            f.dictionary("obsolete", "");
            f.dictionary("literal", "");
            if crate::valid_rime_build_id(literal) {
                f.dictionary(literal, "");
            }
            f.write("rime.lua", &format!("local dictionary = '{literal}'\n"));
            assert_eq!(f.prune(), 4, "{literal}");
            assert!(f.exists("build/literal.table.bin"), "{literal}");
            if crate::valid_rime_build_id(literal) {
                assert!(
                    f.exists(&format!("build/{literal}.reverse.bin")),
                    "{literal}"
                );
            }
        }
    }

    #[test]
    fn long_lua_literal_initial_newline_is_not_part_of_dictionary_id() {
        for newline in ["\n", "\r", "\r\n", "\n\r"] {
            let f = Fixture::new();
            f.dictionary("obsolete", "");
            f.dictionary("literal", "");
            f.write(
                "rime.lua",
                &format!("local dict = [=[{newline}literal]=]\nReverseLookup(dict)\n"),
            );
            assert_eq!(f.prune(), 4);
            assert!(f.exists("build/literal.reverse.bin"));
        }
    }

    #[test]
    fn nested_config_string_values_protect_dictionary_outputs() {
        let f = Fixture::new();
        f.dictionary("obsolete", "");
        for id in ["configured", "sequence", "metadata", "tagged"] {
            f.dictionary(id, "");
        }
        f.schema("active", "translator: {dictionary: main}\ncustom: {nested: {reverse: configured}, names: [sequence]}\n__build_info: {custom: metadata, tagged: !dictionary tagged}\n");
        assert_eq!(f.prune(), 4);
        for id in ["configured", "sequence", "metadata", "tagged"] {
            for suffix in &SUFFIXES[1..] {
                assert!(f.exists(&format!("build/{id}{suffix}")), "{id}{suffix}");
            }
        }
    }

    #[test]
    fn literal_lua_dictionaries_are_kept_and_unused_lua_is_not_scanned() {
        let f = Fixture::new();
        f.dictionary("obsolete", "");
        f.schema("active", "engine: {filters: [lua_filter@filter, 'lua_filter@*module*filter']}\ntranslator: {dictionary: main}\n");
        for id in ["reverse", "legacy", "nested"] {
            f.dictionary(id, "");
        }
        f.write(
            "rime.lua",
            "local module = require('module')\nlocal db = ReverseLookup('legacy')\n",
        );
        f.write("lua/module.lua", "-- ReverseLookup(dynamic)\nlocal ignored = 'ReverseDb(dynamic)'\nlocal text = '\\t'\nlocal db = ReverseDb('build/reverse.reverse.bin')\nrequire 'nested'\n");
        f.write(
            "lua/nested.lua",
            "-- comment\rreturn ReverseLookup [[nested]]\r",
        );
        f.write("lua/unused.lua", "ReverseLookup(dynamic)\n");
        assert_eq!(f.prune(), 4);
        for id in ["reverse", "legacy", "nested"] {
            assert!(f.exists(&format!("build/{id}.reverse.bin")));
        }
    }

    #[test]
    fn inactive_package_lua_snapshots_and_root_entrypoint_protect_dictionaries() {
        let f = Fixture::new();
        f.dictionary("obsolete", "");
        f.write(
            "build/default.yaml",
            "schema_list: [{schema: active}, {schema: inactive}]\n",
        );
        f.schema(
            "inactive",
            "engine: {filters: ['lua_filter@*inactive*filter']}\n",
        );
        for id in ["entrypoint", "inline", "snapshot"] {
            f.dictionary(id, "");
        }
        f.write("rime.lua", "local db = ReverseLookup('entrypoint')\n");
        f.write(
            ".keytao-package-lua/other/lua/inactive.lua",
            "return ReverseDb('build/snapshot.reverse.bin')\n",
        );
        f.write(
            ".keytao-package-lua/other/lua/unused.lua",
            "ReverseLookup(dynamic)\n",
        );
        f.write(crate::ACTIVE_SCHEME_MANIFEST, &serde_json::json!({"packages": {"other": {"schemas": ["inactive"], "rime_lua": "local db = ReverseLookup('inline')\nrequire('inactive')\n", "lua_files": {"lua/inactive.lua": ".keytao-package-lua/other/lua/inactive.lua", "lua/unused.lua": ".keytao-package-lua/other/lua/unused.lua"}}}}).to_string());
        assert_eq!(f.prune(), 4);
        for id in ["entrypoint", "inline", "snapshot"] {
            assert!(f.exists(&format!("build/{id}.reverse.bin")));
        }
    }

    #[test]
    fn uncertain_references_or_compiled_schemas_retain_every_output() {
        for mode in [
            "invalid_schema",
            "dependency",
            "dictionary",
            "include",
            "missing_lua",
            "invalid_lua",
            "invalid_utf8_lua",
            "dictionary_schema",
            "invalid_default",
            "empty_default",
            "indexed_lua",
            "indexed_require",
            "patch_path",
        ] {
            let f = Fixture::new();
            f.dictionary("obsolete", "");
            match mode {
                "invalid_schema" => f.write("build/active.schema.yaml", "schema: [broken\n"),
                "dependency" => f.write(
                    "build/active.schema.yaml",
                    "schema: {schema_id: active, dependencies: [missing]}\n",
                ),
                "dictionary" => f.schema("active", "translator: {dictionary: absent}\n"),
                "include" => f.schema("active", "menu: {__include: 'absent:/menu'}\n"),
                "dictionary_schema" => f.write("build/main.schema.yaml", "invalid\n"),
                "invalid_default" => f.write("build/default.yaml", "schema_list: [wrong]\n"),
                "empty_default" => f.write("build/default.yaml", "schema_list: []\n"),
                "patch_path" => {
                    f.schema("active", "__patch: {'translator/dictionary': obsolete}\n")
                }
                _ => {
                    f.schema("active", "engine: {filters: [lua_filter@filter]}\n");
                    if mode == "invalid_utf8_lua" {
                        fs::write(f.0.join("rime.lua"), [0xff]).unwrap();
                    } else if mode != "missing_lua" {
                        f.write(
                            "rime.lua",
                            match mode {
                                "indexed_lua" => "_G['ReverseLookup']('obsolete')\n",
                                "indexed_require" => "_G['require']('hidden')\n",
                                _ => "local dictionary = 'unterminated\n",
                            },
                        );
                    }
                }
            }
            assert_eq!(f.prune(), 0, "{mode}");
            assert!(f.exists("build/obsolete.table.bin"), "{mode}");
        }
    }

    #[test]
    fn shared_sources_and_builds_are_resolved_without_mutating_shared_data() {
        let f = Fixture::new();
        let shared = Fixture::new();
        fs::remove_file(f.0.join("main.dict.yaml")).unwrap();
        for suffix in &SUFFIXES[1..] {
            fs::remove_file(f.0.join(format!("build/main{suffix}"))).unwrap();
        }
        f.write("build/active.schema.yaml", "schema: {schema_id: active, dependencies: [shared_schema]}\ntranslator: {dictionary: main}\n");
        shared.schema("shared_schema", "translator: {dictionary: secondary}\n");
        shared.dictionary("secondary", "");
        f.dictionary("obsolete", "");
        assert_eq!(
            prune_rime_build_artifacts(&f.0, Some(&shared.0)).unwrap(),
            4
        );
        assert!(shared.exists("build/main.table.bin"));
        assert!(shared.exists("build/shared_schema.schema.yaml"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_inputs_and_build_directories_fail_closed() {
        use std::os::unix::fs::symlink;
        let f = Fixture::new();
        f.dictionary("obsolete", "");
        fs::rename(f.0.join("main.dict.yaml"), f.0.join("source.dict.yaml")).unwrap();
        symlink("source.dict.yaml", f.0.join("main.dict.yaml")).unwrap();
        assert_eq!(f.prune(), 0);
        assert!(f.exists("build/obsolete.table.bin"));
        fs::remove_file(f.0.join("main.dict.yaml")).unwrap();
        fs::rename(f.0.join("source.dict.yaml"), f.0.join("main.dict.yaml")).unwrap();
        fs::rename(f.0.join("build"), f.0.join("actual-build")).unwrap();
        symlink("actual-build", f.0.join("build")).unwrap();
        assert_eq!(f.prune(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_compiled_shadow_never_falls_back_to_source() {
        use std::os::unix::fs::symlink;
        let f = Fixture::new();
        f.dictionary("obsolete", "");
        f.schema("active", "menu: {__include: 'patches:/menu'}\n");
        f.write("patches.yaml", "menu: {page_size: 8}\n");
        symlink("missing.yaml", f.0.join("build/patches.yaml")).unwrap();
        assert_eq!(f.prune(), 0);
        assert!(f.exists("build/obsolete.table.bin"));
        fs::remove_file(f.0.join("build/patches.yaml")).unwrap();
        assert_eq!(f.prune(), 8);
    }
}
