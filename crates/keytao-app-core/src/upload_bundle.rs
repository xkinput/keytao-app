use crate::{CoreError, packages::staging_relative};
use keytao_core::RuntimeDirs;
use serde_yaml::Value;
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufReader, Cursor, Read, Seek, Write},
    path::Path,
};

#[derive(Debug)]
pub struct UploadBundle {
    pub bytes: Vec<u8>,
    pub files: Vec<String>,
    pub total_size: u64,
    pub sha256: String,
    schemas: BTreeMap<String, SchemaFiles>,
    runtime_source: Option<tempfile::TempDir>,
}

#[derive(Debug, Default)]
struct SchemaFiles {
    sources: BTreeSet<String>,
    dictionaries: BTreeMap<String, bool>,
    prisms: BTreeSet<(String, String)>,
    dictionary_sources: BTreeMap<String, BTreeSet<String>>,
    reverse_lookups: BTreeSet<String>,
}

struct Collector<'a> {
    root: &'a Path,
    shared: Option<&'a Path>,
    package: Option<&'a Path>,
    lua: Option<&'a LuaSnapshot>,
    files: BTreeMap<String, BundleFile>,
    schemas: BTreeMap<String, SchemaFiles>,
    pending: BTreeSet<String>,
    bindings: BTreeMap<String, String>,
    targets: BTreeSet<(String, String, bool)>,
}

struct LuaSnapshot {
    directory: tempfile::TempDir,
    managed: BTreeSet<String>,
}

impl LuaSnapshot {
    fn for_schemas(root: &Path, ids: &[String]) -> Result<Option<Self>, CoreError> {
        let path = crate::packages::safe_path(root, keytao_core::ACTIVE_SCHEME_MANIFEST)
            .map_err(failure)?;
        let mut manifest: serde_json::Value = match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let Some(packages) = manifest["packages"].as_object() else { return Ok(None) };
        let Some((key, package)) = packages.iter().find(|(_, package)| {
            package["schemas"].as_array().is_some_and(|schemas| {
                schemas.iter().any(|schema| schema.as_str().is_some_and(|id| ids.iter().any(|selected| selected == id)))
            })
        }) else { return Ok(None) };
        let selected = package["schemas"].as_array().unwrap().iter()
            .filter_map(serde_json::Value::as_str).find(|id| ids.iter().any(|selected| selected == id)).unwrap().to_owned();
        if manifest["root"].as_str().map(Path::new) != Some(root.canonicalize()?.as_path()) {
            return Err(failure("active scheme root does not match the upload source"));
        }
        if manifest["lua_owner"]["key"].as_str() == Some(key) { return Ok(None); }
        let directory = tempfile::tempdir()?;
        let mut managed = BTreeSet::new();
        for (key, package) in packages {
            crate::scheme::validate_scheme_key(key).map_err(failure)?;
            if let Some(files) = package["lua_files"].as_object() {
                for (relative, source) in files {
                    if !relative.starts_with("lua/") { return Err(failure("invalid Lua module path")); }
                    crate::packages::safe_path(root, relative).map_err(failure)?;
                    let source = source.as_str().ok_or_else(|| failure("invalid Lua snapshot path"))?;
                    if !source.starts_with(&format!(".keytao-package-lua/{key}/lua/")) {
                        return Err(failure("invalid Lua snapshot path"));
                    }
                    let input = crate::packages::safe_path(root, source).map_err(failure)?;
                    let output = crate::packages::safe_path(directory.path(), source).map_err(failure)?;
                    fs::create_dir_all(output.parent().unwrap())?;
                    fs::copy(input, output)?;
                    managed.insert(relative.clone());
                }
            }
        }
        if let Some(files) = manifest["lua_owner"]["files"].as_object() {
            managed.extend(files.keys().cloned());
        }
        let dirs = keytao_core::RuntimeDirs::flat(directory.path(), directory.path());
        let build = staging_relative(directory.path(), &dirs);
        fs::create_dir_all(&dirs.staging_dir)?;
        let defaults = serde_yaml::to_string(&serde_json::json!({"schema_list": [{"schema": selected}]}))
            .map_err(|error| failure(error.to_string()))?;
        fs::write(directory.path().join("default.yaml"), defaults)?;
        let compiled = crate::packages::safe_path(
            directory.path(),
            &format!("{build}/{selected}.schema.yaml"),
        )
        .map_err(failure)?;
        fs::write(compiled, "")?;
        manifest["root"] = directory.path().canonicalize()?.to_string_lossy().as_ref().into();
        fs::write(directory.path().join(keytao_core::ACTIVE_SCHEME_MANIFEST), serde_json::to_vec(&manifest)?)?;
        keytao_core::regenerate_active_rime_lua(directory.path()).map_err(failure)?;
        Ok(Some(Self { directory, managed }))
    }
}

enum BundleFile {
    Contents(Vec<u8>),
    Copy(fs::File, u64),
}

impl BundleFile {
    fn contents(&self) -> Result<Cow<'_, [u8]>, CoreError> {
        match self {
            Self::Contents(bytes) => Ok(Cow::Borrowed(bytes)),
            Self::Copy(file, _) => {
                let mut file = file;
                file.rewind()?;
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes)?;
                Ok(Cow::Owned(bytes))
            }
        }
    }

    fn len(&self) -> u64 {
        match self {
            Self::Contents(bytes) => bytes.len() as u64,
            Self::Copy(_, size) => *size,
        }
    }
}

fn failure(message: impl Into<String>) -> CoreError {
    CoreError::Other(message.into())
}

fn allowed(name: &str) -> Result<(), CoreError> {
    crate::packages::validate_relative(name).map_err(failure)?;
    if name.split('/').any(|part| {
        matches!(
            part,
            "build" | "sync" | "logs" | "installation.yaml" | "user.yaml"
        ) || part.ends_with(".userdb")
            || part.ends_with(".log")
            || part.starts_with(".keytao-sync-base-")
            || part.contains(".broken-")
    }) {
        return Err(failure(format!("禁止上传文件：{name}")));
    }
    Ok(())
}

pub(crate) fn yaml(bytes: &[u8], name: &str) -> Result<Value, CoreError> {
    let text = std::str::from_utf8(bytes).map_err(|e| failure(format!("{name}: {e}")))?;
    // Dictionary rows after the YAML header are tab-separated text, not YAML.
    // Some symbol presets contain literal C0 characters in their symbol strings.
    // Escape those only for dependency inspection; archive bytes stay unchanged.
    let header = text
        .lines()
        .take_while(|line| line.trim() != "...")
        .map(|line| {
            let line: String = line
                .trim_end()
                .chars()
                .map(|c| {
                    if c.is_control() && c != '\t' {
                        format!("\\x{:02x}", u32::from(c))
                    } else {
                        c.to_string()
                    }
                })
                .collect();
            format!("{line}\n")
        })
        .collect::<String>();
    // Allow duplicate keys in symbol presets during dependency inspection.
    struct Loose(Value);
    impl<'de> serde::Deserialize<'de> for Loose {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct Visitor;
            impl<'de> serde::de::Visitor<'de> for Visitor {
                type Value = Loose;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("YAML value")
                }
                fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Loose, E> {
                    Ok(Loose(v.into()))
                }
                fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Loose, E> {
                    Ok(Loose(v.into()))
                }
                fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Loose, E> {
                    Ok(Loose(v.into()))
                }
                fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Loose, E> {
                    Ok(Loose(Value::Number(v.into())))
                }
                fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Loose, E> {
                    Ok(Loose(v.into()))
                }
                fn visit_unit<E: serde::de::Error>(self) -> Result<Loose, E> {
                    Ok(Loose(Value::Null))
                }
                fn visit_seq<A: serde::de::SeqAccess<'de>>(
                    self,
                    mut seq: A,
                ) -> Result<Loose, A::Error> {
                    let mut values = Vec::new();
                    while let Some(Loose(v)) = seq.next_element()? {
                        values.push(v);
                    }
                    Ok(Loose(Value::Sequence(values)))
                }
                fn visit_map<A: serde::de::MapAccess<'de>>(
                    self,
                    mut map: A,
                ) -> Result<Loose, A::Error> {
                    let mut values = serde_yaml::Mapping::new();
                    while let Some((Loose(k), Loose(v))) = map.next_entry()? {
                        values.insert(k, v);
                    }
                    Ok(Loose(Value::Mapping(values)))
                }
            }
            d.deserialize_any(Visitor)
        }
    }
    serde_yaml::from_str::<Loose>(&header)
        .map(|v| v.0)
        .map_err(|e| failure(format!("{name}: {e}")))
}

fn source_path(root: &Path, package: Option<&Path>, name: &str) -> Result<std::path::PathBuf, CoreError> {
    let path = crate::packages::safe_path(root, name).map_err(failure)?;
    if let Some(package) = package {
        if !path.try_exists()? {
            return crate::packages::safe_path(package, name).map_err(failure);
        }
    }
    Ok(path)
}

impl Collector<'_> {
    fn source(&self, name: &str) -> Result<std::path::PathBuf, CoreError> {
        let root = self.lua.filter(|lua| name == "rime.lua" || lua.managed.contains(name))
            .map_or(self.root, |lua| lua.directory.path());
        source_path(root, self.package, name)
    }
    fn shared_file(&self, name: &str) -> bool {
        self.shared.is_some_and(|root| root.join(name).is_file())
    }

    fn add(&mut self, schema: &str, name: &str) -> Result<bool, CoreError> {
        self.add_file(schema, name, false)
    }

    fn add_file(&mut self, schema: &str, name: &str, copy_only: bool) -> Result<bool, CoreError> {
        allowed(name)?;
        let path = self.source(name)?;
        if !path.try_exists()? && self.shared_file(name) {
            return Ok(false);
        }
        let sources = &mut self.schemas.get_mut(schema).unwrap().sources;
        if !sources.insert(name.into()) {
            return Ok(false);
        }
        if !self.files.contains_key(name) {
            let read_error = |e| failure(format!("方案 {schema}：无法读取引用文件 {name}: {e}"));
            let file = if copy_only {
                let source = fs::File::open(&path).map_err(read_error)?;
                let size = source.metadata().map_err(read_error)?.len();
                BundleFile::Copy(source, size)
            } else {
                BundleFile::Contents(fs::read(path).map_err(read_error)?)
            };
            self.files.insert(name.into(), file);
        }
        Ok(true)
    }

    fn config(&mut self, schema: &str, name: &str) -> Result<(), CoreError> {
        if self.add(schema, name)? {
            let value = yaml(&self.files[name].contents()?, name)?;
            self.scan(schema, &value, true)?;
        }
        Ok(())
    }

    fn dictionary(&mut self, schema: &str, id: &str, table: Option<bool>) -> Result<(), CoreError> {
        if id.is_empty() {
            return Ok(());
        }
        if let Some(table) = table {
            *self
                .schemas
                .get_mut(schema)
                .unwrap()
                .dictionaries
                .entry(id.into())
                .or_default() |= table;
        }
        self.config(schema, &format!("{id}.dict.yaml"))
    }

    fn target(
        &mut self,
        schema: &str,
        target: &Value,
        preset: bool,
        discover: bool,
    ) -> Result<(), CoreError> {
        match target {
            Value::String(target) => {
                let id = if preset {
                    target.as_str()
                } else {
                    // A reference without ':' addresses the current document.
                    let Some((id, _)) = target.split_once(':') else {
                        return Ok(());
                    };
                    if id.is_empty() {
                        return Ok(());
                    }
                    id
                };
                let mut name = if id.ends_with(".yaml") {
                    id.to_owned()
                } else {
                    format!("{id}.yaml")
                };
                if preset && !self.source(&name)?.is_file() {
                    let symbols = format!("{id}.symbols.yaml");
                    if self.source(&symbols)?.is_file()
                        || (!self.shared_file(&name) && self.shared_file(&symbols))
                    {
                        name = symbols;
                    }
                }
                self.add(schema, &name)?;
                let Some(bytes) = self.files.get(&name) else {
                    return Ok(());
                };
                if !self
                    .targets
                    .insert((schema.into(), target.clone(), discover))
                {
                    return Ok(());
                }
                let value = yaml(&bytes.contents()?, &name)?;
                let mut selected = &value;
                if !preset {
                    let section = target.split_once(':').unwrap().1.trim_start_matches('/');
                    for part in section.split('/').filter(|part| !part.is_empty()) {
                        selected = if let Some(index) =
                            part.strip_prefix('@').and_then(|s| s.parse::<usize>().ok())
                        {
                            selected.as_sequence().and_then(|v| v.get(index))
                        } else {
                            selected.get(part)
                        }
                        .ok_or_else(|| {
                            failure(format!("方案 {schema}：{name} 缺少引用目标 {target}"))
                        })?;
                    }
                }
                self.scan(schema, selected, discover)?;
            }
            Value::Sequence(items) => {
                for item in items {
                    self.target(schema, item, preset, discover)?;
                }
            }
            Value::Mapping(_) => self.scan(schema, target, discover)?,
            _ => {}
        }
        Ok(())
    }

    fn scan(&mut self, schema: &str, value: &Value, discover: bool) -> Result<(), CoreError> {
        if discover {
            for components in ["filters", "translators"] {
                if let Some(items) = value["engine"][components].as_sequence() {
                    for component in items.iter().filter_map(Value::as_str) {
                        let (kind, namespace) = component
                            .split_once('@')
                            .unwrap_or((component, "reverse_lookup"));
                        if matches!(kind, "reverse_lookup_filter" | "reverse_lookup_translator") {
                            self.schemas
                                .get_mut(schema)
                                .unwrap()
                                .reverse_lookups
                                .insert(namespace.into());
                        }
                    }
                }
            }
        }
        self.scan_value(schema, value, discover, false)
    }

    fn scan_value(
        &mut self,
        schema: &str,
        value: &Value,
        discover: bool,
        reverse_only: bool,
    ) -> Result<(), CoreError> {
        match value {
            Value::Mapping(map) => {
                if discover && matches!(value["db_class"].as_str(), Some("stabledb" | "tabledb")) {
                    if let Some(id) = value["user_dict"].as_str().filter(|id| !id.is_empty()) {
                        let name = if id.ends_with(".txt") {
                            id.to_owned()
                        } else {
                            format!("{id}.txt")
                        };
                        allowed(&name)?;
                        let path = crate::packages::safe_path(self.root, &name).map_err(failure)?;
                        if path.try_exists()? {
                            self.add(schema, &name)?;
                        }
                    }
                }
                for (key, item) in map {
                    let key = key.as_str().unwrap_or("").rsplit('/').next().unwrap_or("");
                    match key {
                        "import_preset" => self.target(schema, item, true, discover)?,
                        "__include" | "__patch" => self.target(schema, item, false, discover)?,
                        _ if !discover => {}
                        "dictionary" => {
                            if let Some(id) = item.as_str() {
                                self.dictionary(schema, id, Some(!reverse_only))?;
                                if !reverse_only && !id.is_empty() {
                                    let prism = value["prism"]
                                        .as_str()
                                        .filter(|name| !name.is_empty())
                                        .unwrap_or(id);
                                    self.schemas
                                        .get_mut(schema)
                                        .unwrap()
                                        .prisms
                                        .insert((prism.into(), id.into()));
                                }
                            }
                        }
                        "import_tables" => {
                            if let Some(ids) = item.as_sequence() {
                                for id in ids {
                                    if let Some(id) = id.as_str() {
                                        self.dictionary(schema, id, None)?;
                                    }
                                }
                            }
                        }
                        "dependencies" => {
                            if let Some(ids) = item.as_sequence() {
                                for id in ids {
                                    if let Some(id) = id.as_str() {
                                        self.pending.insert(id.into());
                                    }
                                }
                            }
                        }
                        "opencc_config" => {
                            if let Some(id) = item.as_str().filter(|id| !id.is_empty()) {
                                self.opencc(schema, id)?;
                            }
                        }
                        _ => {}
                    }
                    let reverse =
                        reverse_only || self.schemas[schema].reverse_lookups.contains(key);
                    self.scan_value(schema, item, discover, reverse)?;
                }
            }
            Value::Sequence(items) => {
                for item in items {
                    self.scan_value(schema, item, discover, reverse_only)?;
                }
            }
            Value::String(component) if discover && component.starts_with("lua_") => {
                if let Some((_, name)) = component.split_once('@') {
                    let name = name.split('@').next().unwrap_or("");
                    if let Some(module) = name.strip_prefix('*') {
                        self.lua(schema, module, false)?;
                    } else {
                        let global = name.split('.').next().unwrap_or(name);
                        let path = self.source("rime.lua")?;
                        if !path.try_exists()? && self.shared_file("rime.lua") {
                            return Ok(());
                        }
                        let text = fs::read_to_string(path).map_err(|e| {
                            failure(format!("方案 {schema}：rime.lua ({name}): {e}"))
                        })?;
                        let (module, binding) = lua_binding(&text, global).ok_or_else(|| {
                            failure(format!(
                                "方案 {schema}：rime.lua 缺少 {name} 的 require 绑定"
                            ))
                        })?;
                        self.bindings.insert(global.into(), binding);
                        self.lua(schema, &module, false)?;
                        // Track the original for freshness, but export only selected bindings.
                        self.schemas
                            .get_mut(schema)
                            .unwrap()
                            .sources
                            .insert("rime.lua".into());
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn lua(&mut self, schema: &str, module: &str, optional: bool) -> Result<(), CoreError> {
        let module = module.replace('.', "/");
        let direct = format!("lua/{module}.lua");
        let init = format!("lua/{module}/init.lua");
        let name = if self.source(&direct)?.is_file() {
            direct
        } else if self.source(&init)?.is_file() {
            init
        } else if self.shared_file(&direct) {
            direct
        } else if self.shared_file(&init) {
            init
        } else {
            if optional {
                return Ok(());
            }
            return Err(failure(format!(
                "方案 {schema}：缺少引用文件 {direct} 或 {init}"
            )));
        };
        if self.add(schema, &name)? {
            let contents = self.files[&name].contents()?;
            let text =
                std::str::from_utf8(&contents).map_err(|e| failure(format!("{name}: {e}")))?;
            let modules = lua_requires(text);
            for (module, optional) in modules {
                self.lua(schema, &module, optional)?;
            }
        }
        Ok(())
    }

    fn opencc(&mut self, schema: &str, name: &str) -> Result<(), CoreError> {
        let name = format!("opencc/{name}");
        if self.add(schema, &name)? {
            let config: serde_json::Value = serde_json::from_slice(&self.files[&name].contents()?)
                .map_err(|e| failure(format!("{name}: {e}")))?;
            fn files(value: &serde_json::Value, out: &mut Vec<String>) {
                match value {
                    serde_json::Value::Object(map) => {
                        for (key, value) in map {
                            if key == "file" {
                                if let Some(name) = value.as_str() {
                                    out.push(name.into());
                                }
                            }
                            files(value, out);
                        }
                    }
                    serde_json::Value::Array(items) => {
                        for item in items {
                            files(item, out);
                        }
                    }
                    _ => {}
                }
            }
            let mut names = Vec::new();
            files(&config, &mut names);
            for dependency in names {
                let dependency = dependency.trim_start_matches("./");
                let relative = Path::new(&name)
                    .parent()
                    .unwrap()
                    .join(dependency)
                    .to_string_lossy()
                    .into_owned();
                let root_relative = format!("opencc/{dependency}");
                let file = if self.source(&relative)?.is_file() {
                    &relative
                } else if self.source(&root_relative)?.is_file()
                    || (!self.shared_file(&relative) && self.shared_file(&root_relative))
                {
                    &root_relative
                } else {
                    &relative
                };
                self.add_file(schema, file, true)?;
            }
        }
        Ok(())
    }
}

// Tokenize only enough Lua to find literal requires; never evaluate package code.
#[derive(Debug, PartialEq)]
enum Token {
    Name(String),
    Literal(String),
    Symbol(char),
}

fn lua_tokens(text: &str) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"--") {
            i += 2;
            if bytes.get(i) == Some(&b'[') {
                let start = i;
                i += 1;
                while bytes.get(i) == Some(&b'=') {
                    i += 1;
                }
                if bytes.get(i) == Some(&b'[') {
                    let end = format!("]{}]", "=".repeat(i - start - 1));
                    i += 1;
                    i = text[i..]
                        .find(&end)
                        .map_or(bytes.len(), |n| i + n + end.len());
                    continue;
                }
            }
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if bytes[i] == b'[' {
            let start = i;
            let mut delimiter = i + 1;
            while bytes.get(delimiter) == Some(&b'=') {
                delimiter += 1;
            }
            if bytes.get(delimiter) == Some(&b'[') {
                let close = format!("]{}]", "=".repeat(delimiter - start - 1));
                let content = delimiter + 1;
                let end = text[content..]
                    .find(&close)
                    .map_or(bytes.len(), |n| content + n);
                out.push(Token::Literal(text[content..end].into()));
                i = (end + close.len()).min(bytes.len());
            } else {
                out.push(Token::Symbol('['));
                i += 1;
            }
        } else if matches!(bytes[i], b'\'' | b'"') {
            let quote = bytes[i];
            i += 1;
            let start = i;
            while i < bytes.len() && bytes[i] != quote {
                if bytes[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            out.push(Token::Literal(text[start..i.min(bytes.len())].into()));
            i += usize::from(i < bytes.len());
        } else if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            out.push(Token::Name(text[start..i].into()));
        } else {
            if !bytes[i].is_ascii_whitespace() {
                out.push(Token::Symbol(bytes[i] as char));
            }
            i += 1;
        }
    }
    out
}

fn lua_requires(text: &str) -> Vec<(String, bool)> {
    let tokens = lua_tokens(text);
    let mut modules = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        if token == &Token::Name("require".into()) {
            if i > 0 && matches!(tokens[i - 1], Token::Symbol('.' | ':')) {
                continue;
            }
            let mut next = i + 1 + usize::from(tokens.get(i + 1) == Some(&Token::Symbol('(')));
            let optional = i >= 2
                && tokens[i - 2] == Token::Name("pcall".into())
                && tokens[i - 1] == Token::Symbol('(')
                && tokens.get(next) == Some(&Token::Symbol(','));
            if optional {
                next += 1;
            }
            if let Some(Token::Literal(module)) = tokens.get(next) {
                modules.push((module.clone(), optional));
            }
        }
    }
    modules
}

fn lua_binding(text: &str, name: &str) -> Option<(String, String)> {
    let tokens = lua_tokens(text);
    for (i, token) in tokens.iter().enumerate() {
        if token != &Token::Name(name.into())
            || tokens.get(i + 1) != Some(&Token::Symbol('='))
            || tokens.get(i + 2) != Some(&Token::Name("require".into()))
        {
            continue;
        }
        if i > 0
            && (tokens[i - 1] == Token::Name("local".into())
                || matches!(tokens[i - 1], Token::Symbol('.' | ':')))
        {
            continue;
        }
        let next = i + 3 + usize::from(tokens.get(i + 3) == Some(&Token::Symbol('(')));
        if let Some(Token::Literal(module)) = tokens.get(next) {
            let mut binding = format!("{name} = require({})", serde_json::to_string(module).ok()?);
            let mut end = next + 1 + usize::from(tokens.get(next + 1) == Some(&Token::Symbol(')')));
            while tokens.get(end) == Some(&Token::Symbol('.')) {
                let Some(Token::Name(field)) = tokens.get(end + 1) else {
                    return None;
                };
                binding.push('.');
                binding.push_str(field);
                end += 2;
            }
            binding.push('\n');
            return Some((module.clone(), binding));
        }
    }
    None
}

pub fn build(
    root: &Path,
    shared: Option<&Path>,
    schema_ids: &[String],
) -> Result<UploadBundle, CoreError> {
    let dirs = RuntimeDirs::resolve(root, shared.unwrap_or(root)).map_err(failure)?;
    build_from(root, shared, &dirs, schema_ids)
}

fn build_from(
    root: &Path,
    shared: Option<&Path>,
    dirs: &RuntimeDirs,
    schema_ids: &[String],
) -> Result<UploadBundle, CoreError> {
    if schema_ids.is_empty() {
        return Err(failure("方案包缺少 schema_list/schema manifest"));
    }
    let user = &dirs.user_data_dir;
    let build = staging_relative(user, dirs);
    let package = dirs.active_package_key.as_ref().map(|_| dirs.shared_data_dir.as_path());
    let lua = if package.is_some() { None } else { LuaSnapshot::for_schemas(root, schema_ids)? };
    let mut collector = Collector {
        root: user,
        shared: if package.is_some() { None } else { shared },
        package,
        lua: lua.as_ref(),
        files: BTreeMap::new(),
        schemas: BTreeMap::new(),
        pending: schema_ids.iter().cloned().collect(),
        bindings: BTreeMap::new(),
        targets: BTreeSet::new(),
    };
    while let Some(id) = collector.pending.pop_first() {
        if collector.schemas.contains_key(&id) {
            continue;
        }
        collector.schemas.insert(id.clone(), SchemaFiles::default());
        let source = format!("{id}.schema.yaml");
        if !collector.add(&id, &source)? {
            collector.schemas.remove(&id);
            continue;
        }
        let source = yaml(&collector.files[&source].contents()?, &source)?;
        let compiled = format!("{build}/{id}.schema.yaml");
        let compiled_path = crate::packages::safe_path(user, &compiled).map_err(failure)?;
        let deployed = compiled_path.is_file();
        // The deployed schema contains Rime's resolved includes and custom patches.
        // Source references still supply the files needed to rebuild that schema.
        collector.scan(&id, &source, !deployed)?;
        let custom = format!("{id}.custom.yaml");
        if collector.source(&custom)?.try_exists()? {
            collector.add(&id, &custom)?;
            let value = yaml(&collector.files[&custom].contents()?, &custom)?;
            collector.scan(&id, &value, !deployed)?;
        }
        if deployed {
            let value = yaml(&fs::read(compiled_path)?, &compiled)?;
            collector.scan(&id, &value, true)?;
        }
    }
    let defaults = serde_yaml::to_string(&serde_json::json!({"patch":{"schema_list":schema_ids.iter().map(|id| serde_json::json!({"schema":id})).collect::<Vec<_>>()}})).map_err(|e| failure(e.to_string()))?;
    collector.files.insert(
        "default.custom.yaml".into(),
        BundleFile::Contents(defaults.into_bytes()),
    );
    if !collector.bindings.is_empty() {
        collector.files.insert(
            "rime.lua".into(),
            BundleFile::Contents(
                collector
                    .bindings
                    .into_values()
                    .collect::<String>()
                    .into_bytes(),
            ),
        );
    }
    fn dictionary_inputs(
        id: &str,
        files: &BTreeMap<String, BundleFile>,
        inputs: &mut BTreeSet<String>,
    ) -> Result<(), CoreError> {
        let name = format!("{id}.dict.yaml");
        let Some(bytes) = files.get(&name) else {
            // Shared dictionaries are satisfied by the installation, not uploaded inputs.
            return Ok(());
        };
        if !inputs.insert(name.clone()) {
            return Ok(());
        }
        let value = yaml(&bytes.contents()?, &name)?;
        if let Some(imports) = value["import_tables"].as_sequence() {
            for id in imports.iter().filter_map(Value::as_str) {
                dictionary_inputs(id, files, inputs)?;
            }
        }
        Ok(())
    }
    for schema in collector.schemas.values_mut() {
        for id in schema.dictionaries.keys() {
            let mut inputs = BTreeSet::new();
            dictionary_inputs(id, &collector.files, &mut inputs)?;
            schema.dictionary_sources.insert(id.clone(), inputs);
        }
    }
    let total_size = collector.files.values().map(BundleFile::len).sum();
    let files = collector.files.keys().cloned().collect();
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(zip::DateTime::default())
        .unix_permissions(0o644);
    for (name, file) in collector.files {
        zip.start_file(&name, options)
            .map_err(|e| failure(e.to_string()))?;
        match file {
            BundleFile::Contents(bytes) => zip.write_all(&bytes)?,
            BundleFile::Copy(mut source, _) => {
                source.rewind()?;
                let mut reader = BufReader::with_capacity(65_536, source);
                std::io::copy(&mut reader, &mut zip)
                    .map_err(|e| failure(format!("无法读取引用文件 {name}: {e}")))?;
            }
        }
    }
    let bytes = zip
        .finish()
        .map_err(|e| failure(e.to_string()))?
        .into_inner();
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    Ok(UploadBundle {
        bytes,
        files,
        total_size,
        sha256,
        schemas: collector.schemas,
        runtime_source: lua.map(|lua| lua.directory),
    })
}

pub fn verified(
    root: &Path,
    shared: Option<&Path>,
    schema_ids: &[String],
    mut smoke: impl FnMut(&Path, &str, u32) -> Result<(), String>,
) -> Result<UploadBundle, CoreError> {
    checked(root, shared, None, schema_ids, Some(&mut smoke))
}

pub fn verified_artifacts(
    root: &Path,
    shared: Option<&Path>,
    schema_ids: &[String],
) -> Result<UploadBundle, CoreError> {
    checked(root, shared, None, schema_ids, None)
}

pub(crate) fn verified_package(
    root: &Path,
    shared: Option<&Path>,
    package: &str,
    schema_ids: &[String],
    smoke: Option<&mut dyn FnMut(&Path, &str, u32) -> Result<(), String>>,
) -> Result<UploadBundle, CoreError> {
    checked(root, shared, Some(package), schema_ids, smoke)
}

pub(crate) fn verify_deploy_artifacts(
    root: &Path,
    shared: Option<&Path>,
    schema_ids: &[String],
) -> Result<(), CoreError> {
    let dirs = keytao_core::RuntimeDirs::resolve(root, shared.unwrap_or(root)).map_err(failure)?;
    let build = staging_relative(root, &dirs);
    if schema_ids.is_empty() {
        return Err(failure("方案包缺少 schema_list/schema manifest"));
    }
    let mut pending: BTreeSet<_> = schema_ids.iter().cloned().collect();
    let mut visited = BTreeSet::new();
    let sources = BTreeSet::new();
    while let Some(id) = pending.pop_first() {
        if !visited.insert(id.clone()) {
            continue;
        }
        if dirs.active_package_key.is_none() {
            let source = format!("{id}.schema.yaml");
            let source_path = crate::packages::safe_path(root, &source).map_err(failure)?;
            if !source_path.is_file() && shared.is_some_and(|dir| dir.join(&source).is_file()) {
                continue;
            }
        }
        let compiled = format!("{build}/{id}.schema.yaml");
        let compiled_path = verify_artifact(root, &id, &compiled, &sources, None, false)?;
        let value = yaml(&fs::read(compiled_path)?, &compiled)?;
        if value["schema"]["schema_id"].as_str() != Some(&id) {
            return Err(failure(format!(
                "方案 {id}：{compiled} 的 schema/schema_id 不匹配"
            )));
        }
        if let Some(dependencies) = value["schema"]["dependencies"].as_sequence() {
            pending.extend(
                dependencies
                    .iter()
                    .filter_map(Value::as_str)
                    .map(String::from),
            );
        }
        // Deployment needs compiled artifacts, not the upload's source/runtime closure.
        let mut schema = SchemaFiles::default();
        for components in ["filters", "translators"] {
            if let Some(items) = value["engine"][components].as_sequence() {
                for component in items.iter().filter_map(Value::as_str) {
                    let (kind, namespace) = component
                        .split_once('@')
                        .unwrap_or((component, "reverse_lookup"));
                    if matches!(kind, "reverse_lookup_filter" | "reverse_lookup_translator") {
                        schema.reverse_lookups.insert(namespace.into());
                    }
                }
            }
        }
        collect_deploy_dictionaries(&value, false, &mut schema);
        if dirs.active_package_key.is_none() {
            for dictionary in schema.dictionaries.keys().cloned().collect::<Vec<_>>() {
                let name = format!("{dictionary}.dict.yaml");
                let local = crate::packages::safe_path(root, &name).map_err(failure)?;
                if !local.is_file() && shared.is_some_and(|dir| dir.join(&name).is_file()) {
                    schema.dictionaries.remove(&dictionary);
                    schema.prisms.retain(|(_, id)| id != &dictionary);
                }
            }
        }
        for (dictionary, table) in schema.dictionaries {
            if table {
                verify_artifact(
                    root,
                    &id,
                    &format!("{build}/{dictionary}.table.bin"),
                    &sources,
                    None,
                    false,
                )?;
            }
            verify_artifact(
                root,
                &id,
                &format!("{build}/{dictionary}.reverse.bin"),
                &sources,
                None,
                false,
            )?;
        }
        for (prism, _) in schema.prisms {
            verify_artifact(
                root,
                &id,
                &format!("{build}/{prism}.prism.bin"),
                &sources,
                None,
                false,
            )?;
        }
    }
    Ok(())
}

fn collect_deploy_dictionaries(value: &Value, reverse_only: bool, schema: &mut SchemaFiles) {
    match value {
        Value::Mapping(map) => {
            if let Some(dictionary) = value["dictionary"].as_str().filter(|id| !id.is_empty()) {
                *schema.dictionaries.entry(dictionary.into()).or_default() |= !reverse_only;
                if !reverse_only {
                    let prism = value["prism"]
                        .as_str()
                        .filter(|id| !id.is_empty())
                        .unwrap_or(dictionary);
                    schema.prisms.insert((prism.into(), dictionary.into()));
                }
            }
            for (key, item) in map {
                let reverse = reverse_only
                    || key
                        .as_str()
                        .is_some_and(|key| schema.reverse_lookups.contains(key));
                collect_deploy_dictionaries(item, reverse, schema);
            }
        }
        Value::Sequence(items) => {
            for item in items {
                collect_deploy_dictionaries(item, reverse_only, schema);
            }
        }
        _ => {}
    }
}

fn checked(
    root: &Path,
    shared: Option<&Path>,
    target: Option<&str>,
    schema_ids: &[String],
    mut smoke: Option<&mut dyn FnMut(&Path, &str, u32) -> Result<(), String>>,
) -> Result<UploadBundle, CoreError> {
    let dirs = if let Some(key) = target.filter(|_| !RuntimeDirs::needs_migration(root)) {
        RuntimeDirs::for_package(root, shared.unwrap_or(root), key)
            .ok_or_else(|| failure(format!("无法读取方案包：{key}")))?
    } else {
        RuntimeDirs::resolve(root, shared.unwrap_or(root)).map_err(failure)?
    };
    let user = &dirs.user_data_dir;
    let staging = staging_relative(user, &dirs);
    let package = dirs.active_package_key.as_ref().map(|_| dirs.shared_data_dir.as_path());
    let mut bundle = build_from(root, shared, &dirs, schema_ids)?;
    if smoke.is_some() && package.is_some() {
        bundle.runtime_source = Some(tempfile::tempdir()?);
    }
    let smoke_root = if smoke.is_some() {
        bundle.prepare_smoke_source(&dirs, schema_ids)?
    } else { root };
    for (id, schema) in &bundle.schemas {
        let configs: BTreeSet<_> = schema
            .sources
            .iter()
            .filter(|name| name.ends_with(".yaml") && !name.ends_with(".dict.yaml"))
            .cloned()
            .collect();
        let compiled = format!("{staging}/{id}.schema.yaml");
        let compiled_path = verify_artifact(user, id, &compiled, &configs, package, true)?;
        for (dictionary, table) in &schema.dictionaries {
            let inputs = &schema.dictionary_sources[dictionary];
            if inputs.is_empty() {
                // Shared dictionaries are supplied by the runtime installation.
                continue;
            }
            if *table {
                verify_artifact(
                    user,
                    id,
                    &format!("{staging}/{dictionary}.table.bin"),
                    inputs,
                    package,
                    true,
                )?;
            }
            verify_artifact(
                user,
                id,
                &format!("{staging}/{dictionary}.reverse.bin"),
                inputs,
                package,
                true,
            )?;
        }
        for (prism, dictionary) in &schema.prisms {
            if schema.dictionary_sources[dictionary].is_empty() {
                continue;
            }
            let mut sources = configs.clone();
            sources.extend(schema.dictionary_sources[dictionary].iter().cloned());
            verify_artifact(
                user,
                id,
                &format!("{staging}/{prism}.prism.bin"),
                &sources,
                package,
                true,
            )?;
        }
        let value = yaml(&fs::read(compiled_path)?, &compiled)?;
        if value["schema"]["schema_id"].as_str() != Some(id) {
            return Err(failure(format!(
                "方案 {id}：{compiled} 的 schema/schema_id 不匹配"
            )));
        }
        if let Some(smoke) = smoke.as_mut() {
            let alphabet = value["speller"]["alphabet"]
                .as_str()
                .and_then(|s| s.chars().next())
                .ok_or_else(|| failure(format!("方案 {id}：{compiled} 缺少 speller/alphabet")))?;
            smoke(smoke_root, id, u32::from(alphabet))
                .map_err(|e| failure(format!("方案 {id}：输入冒烟检查失败: {e}")))?;
        }
    }
    Ok(bundle)
}

impl UploadBundle {
    fn prepare_smoke_source<'a>(&'a self, dirs: &'a RuntimeDirs, ids: &[String]) -> Result<&'a Path, CoreError> {
        let root = &dirs.user_data_dir;
        let Some(directory) = self.runtime_source.as_ref() else { return Ok(root) };
        let output = directory.path();
        let build = staging_relative(root, dirs);
        let output_dirs = keytao_core::RuntimeDirs::flat(output, output);
        let mut archive = zip::ZipArchive::new(Cursor::new(&self.bytes))
            .map_err(|error| failure(error.to_string()))?;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).map_err(|error| failure(error.to_string()))?;
            let destination = crate::packages::safe_path(output, entry.name()).map_err(failure)?;
            fs::create_dir_all(destination.parent().unwrap())?;
            let mut file = fs::File::create(destination)?;
            std::io::copy(&mut entry, &mut file)?;
        }
        for entry in fs::read_dir(crate::packages::safe_path(root, &build).map_err(failure)?)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_str().ok_or_else(|| failure("invalid build artifact path"))?;
            if name.starts_with(".keytao-") || !entry.file_type()?.is_file() { continue; }
            let source = crate::packages::safe_path(root, &format!("{build}/{name}"))
                .map_err(failure)?;
            fs::copy(source, output_dirs.staging_dir.join(name))?;
        }
        fs::write(output_dirs.staging_dir.join("default.yaml"), serde_yaml::to_string(&serde_json::json!({
            "schema_list": ids.iter().map(|id| serde_json::json!({"schema": id})).collect::<Vec<_>>()
        })).map_err(|error| failure(error.to_string()))?)?;
        keytao_core::complete_deployment(output).map_err(failure)?;
        #[cfg(target_os = "windows")]
        keytao_core::mark_windows_rime_build_repair_complete(output).map_err(failure)?;
        Ok(output)
    }
}

fn verify_artifact(
    root: &Path,
    id: &str,
    artifact: &str,
    sources: &BTreeSet<String>,
    package: Option<&Path>,
    require_freshness: bool,
) -> Result<std::path::PathBuf, CoreError> {
    let path = crate::packages::safe_path(root, artifact).map_err(failure)?;
    let metadata = fs::metadata(&path)
        .map_err(|e| failure(format!("方案 {id}：缺少构建产物 {artifact}: {e}")))?;
    if !metadata.is_file() {
        return Err(failure(format!("方案 {id}：构建产物不是文件 {artifact}")));
    }
    if !require_freshness {
        // librime reuses checksum-identical artifacts even when source mtimes change.
        return Ok(path);
    }
    let built = metadata
        .modified()
        .map_err(|e| failure(format!("方案 {id}：{artifact}: {e}")))?;
    // Runtime-only Lua/OpenCC/text resources do not produce dictionary binaries.
    for source in sources {
        let source_path = source_path(root, package, source)?;
        let modified = fs::metadata(source_path)
            .and_then(|m| m.modified())
            .map_err(|e| failure(format!("方案 {id}：{source}: {e}")))?;
        if built <= modified {
            return Err(failure(format!(
                "方案 {id}：构建产物 {artifact} 不新于源文件 {source}"
            )));
        }
    }
    Ok(path)
}

#[cfg(test)]
mod streaming_tests {
    use super::*;

    #[test]
    fn isolated_upload_uses_the_requested_package_sources_and_build() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let store = root.join("scheme-packages");
        fs::create_dir_all(&store).unwrap();
        fs::write(store.join("active-manifest.json"), r#"{"active_package":"one","runtime_migration_version":1}"#).unwrap();
        fs::write(root.join(keytao_core::ACTIVE_SCHEME_MANIFEST), "not the isolated manifest").unwrap();
        let id = "main".to_owned();
        for key in ["one", "two"] {
            let files = store.join("cloud").join(key).join("files");
            fs::create_dir_all(&files).unwrap();
            let source = files.join("main.schema.yaml");
            fs::write(&source, format!("schema: {{ schema_id: main, name: {key} }}\n")).unwrap();
            let past = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
            fs::File::options().write(true).open(source).unwrap().set_times(fs::FileTimes::new().set_modified(past)).unwrap();
        }
        let build = root.join("runtime/two/build");
        fs::create_dir_all(&build).unwrap();
        fs::write(root.join(RuntimeDirs::MIGRATION_MARKER), r#"{"version":1,"runtime_package":"one"}"#).unwrap();
        fs::write(build.join("main.schema.yaml"), "schema: { schema_id: main, name: two }\n").unwrap();
        let bundle = verified_package(root, None, "two", &[id], None).unwrap();
        let mut archive = zip::ZipArchive::new(Cursor::new(bundle.bytes)).unwrap();
        let mut schema = String::new();
        archive.by_name("main.schema.yaml").unwrap().read_to_string(&mut schema).unwrap();
        assert!(schema.contains("name: two"));
    }

    #[test]
    fn inactive_lua_owner_upload_uses_its_snapshot_without_changing_runtime_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("lua")).unwrap();
        fs::create_dir_all(root.join(".keytao-package-lua/keytao/lua")).unwrap();
        fs::create_dir_all(root.join(".keytao-package-lua/xmjd/lua/shared")).unwrap();
        fs::write(root.join("rime.lua"), "filter = require('a_shared')\n").unwrap();
        fs::write(root.join("lua/a_shared.lua"), "return 'A'\n").unwrap();
        fs::write(root.join("lua/shared.lua"), "return 'A'\n").unwrap();
        fs::write(root.join(".keytao-package-lua/keytao/lua/shared.lua"), "return 'A'\n").unwrap();
        fs::write(root.join(".keytao-package-lua/xmjd/lua/shared/init.lua"), "return 'B'\n").unwrap();
        fs::write(root.join("b.schema.yaml"), "schema: {schema_id: b}\nengine: {filters: [lua_filter@filter]}\n").unwrap();
        fs::write(root.join(keytao_core::ACTIVE_SCHEME_MANIFEST), serde_json::json!({
            "root": root.canonicalize().unwrap(),
            "lua_owner": {"key": "keytao"},
            "packages": {
                "keytao": {"schemas": ["a"], "rime_lua": "filter = require('a_shared')\n", "lua_files": {"lua/shared.lua": ".keytao-package-lua/keytao/lua/shared.lua"}},
                "xmjd": {"schemas": ["b"], "rime_lua": "filter = require('shared')\n", "lua_files": {"lua/shared/init.lua": ".keytao-package-lua/xmjd/lua/shared/init.lua"}}
            }
        }).to_string()).unwrap();
        let marker = fs::read(root.join(keytao_core::ACTIVE_SCHEME_MANIFEST)).unwrap();
        let bundle = build(root, None, &["b".into()]).unwrap();
        let mut archive = zip::ZipArchive::new(Cursor::new(bundle.bytes)).unwrap();
        let mut entry = String::new();
        archive.by_name("rime.lua").unwrap().read_to_string(&mut entry).unwrap();
        assert_eq!(entry, "filter = require(\"shared\")\n");
        let mut module = String::new();
        archive.by_name("lua/shared/init.lua").unwrap().read_to_string(&mut module).unwrap();
        assert_eq!(module, "return 'B'\n");
        assert!(archive.by_name("lua/shared.lua").is_err());
        fs::create_dir_all(root.join("build")).unwrap();
        let compiled = "schema: {schema_id: b}\nspeller: {alphabet: a}\nengine: {filters: [lua_filter@filter]}\n";
        fs::write(root.join("build/b.schema.yaml"), compiled).unwrap();
        for (name, seconds) in [("b.schema.yaml", 1), ("build/b.schema.yaml", 2)] {
            fs::File::open(root.join(name)).unwrap().set_times(fs::FileTimes::new().set_modified(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds)
            )).unwrap();
        }
        let mut smoked = false;
        verified(root, None, &["b".into()], |source, id, key| {
            assert_ne!(source, root);
            assert_eq!((id, key), ("b", u32::from('a')));
            assert_eq!(fs::read_to_string(source.join("rime.lua")).unwrap(), "filter = require(\"shared\")\n");
            assert_eq!(fs::read_to_string(source.join("lua/shared/init.lua")).unwrap(), "return 'B'\n");
            assert!(!source.join("lua/shared.lua").exists());
            assert!(keytao_core::schema_install_state(source).deployed);
            smoked = true;
            Ok(())
        }).unwrap();
        assert!(smoked);
        assert_eq!(fs::read_to_string(root.join("rime.lua")).unwrap(), "filter = require('a_shared')\n");
        assert_eq!(fs::read_to_string(root.join("lua/shared.lua")).unwrap(), "return 'A'\n");
        assert_eq!(fs::read(root.join(keytao_core::ACTIVE_SCHEME_MANIFEST)).unwrap(), marker);
    }

    #[test]
    fn streamed_opencc_resource_preserves_archive_bytes_and_hash() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("opencc")).unwrap();
        let schema = b"schema:\n  schema_id: sample\nemoji:\n  opencc_config: emoji.json\n";
        let config = br#"{"conversion_chain":[{"dict":{"type":"ocd2","file":"emoji.ocd2"}}]}"#;
        let resource: Vec<u8> = (0..3 * 65_536 + 31).map(|i| (i % 251) as u8).collect();
        let defaults = serde_yaml::to_string(&serde_json::json!({
            "patch": {"schema_list": [{"schema": "sample"}]}
        }))
        .unwrap();
        let inputs = BTreeMap::from([
            ("default.custom.yaml", defaults.into_bytes()),
            ("opencc/emoji.json", config.to_vec()),
            ("opencc/emoji.ocd2", resource),
            ("sample.schema.yaml", schema.to_vec()),
        ]);
        for (name, bytes) in &inputs {
            fs::write(dir.path().join(name), bytes).unwrap();
        }

        let bundle = build(dir.path(), None, &["sample".into()]).unwrap();
        let mut previous = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .last_modified_time(zip::DateTime::default())
            .unix_permissions(0o644);
        for (name, bytes) in &inputs {
            previous.start_file(name, options).unwrap();
            previous.write_all(bytes).unwrap();
        }
        let previous = previous.finish().unwrap().into_inner();
        assert_eq!(bundle.bytes, previous);
        assert_eq!(bundle.sha256, format!("{:x}", Sha256::digest(&previous)));
        assert_eq!(
            bundle.total_size,
            inputs.values().map(|bytes| bytes.len() as u64).sum::<u64>()
        );
    }
}

#[cfg(test)]
#[path = "upload_bundle_tests.rs"]
mod tests;
