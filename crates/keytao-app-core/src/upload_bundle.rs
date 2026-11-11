use crate::CoreError;
use serde_yaml::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Cursor, Write},
    path::Path,
};

#[derive(Debug)]
pub struct UploadBundle {
    pub bytes: Vec<u8>,
    pub files: Vec<String>,
    pub total_size: u64,
    pub sha256: String,
    schemas: BTreeMap<String, SchemaFiles>,
}

#[derive(Debug, Default)]
struct SchemaFiles {
    sources: BTreeSet<String>,
    dictionaries: BTreeMap<String, bool>,
    prisms: BTreeSet<(String, String)>,
    dictionary_sources: BTreeMap<String, BTreeSet<String>>,
    reverse_filters: BTreeSet<String>,
}

struct Collector<'a> {
    root: &'a Path,
    shared: Option<&'a Path>,
    files: BTreeMap<String, Vec<u8>>,
    schemas: BTreeMap<String, SchemaFiles>,
    pending: BTreeSet<String>,
    bindings: BTreeMap<String, String>,
    targets: BTreeSet<(String, String, bool)>,
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

fn yaml(bytes: &[u8], name: &str) -> Result<Value, CoreError> {
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

impl Collector<'_> {
    fn shared_file(&self, name: &str) -> bool {
        self.shared.is_some_and(|root| root.join(name).is_file())
    }

    fn add(&mut self, schema: &str, name: &str) -> Result<bool, CoreError> {
        allowed(name)?;
        let path = crate::packages::safe_path(self.root, name).map_err(failure)?;
        if !path.try_exists()? && self.shared_file(name) {
            return Ok(false);
        }
        let sources = &mut self.schemas.get_mut(schema).unwrap().sources;
        if !sources.insert(name.into()) {
            return Ok(false);
        }
        if !self.files.contains_key(name) {
            let bytes = fs::read(path)
                .map_err(|e| failure(format!("方案 {schema}：无法读取引用文件 {name}: {e}")))?;
            self.files.insert(name.into(), bytes);
        }
        Ok(true)
    }

    fn config(&mut self, schema: &str, name: &str) -> Result<(), CoreError> {
        if self.add(schema, name)? {
            let value = yaml(&self.files[name], name)?;
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
                if preset
                    && !self.root.join(&name).is_file()
                    && (self.root.join(format!("{id}.symbols.yaml")).is_file()
                        || (!self.shared_file(&name)
                            && self.shared_file(&format!("{id}.symbols.yaml"))))
                {
                    name = format!("{id}.symbols.yaml");
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
                let value = yaml(bytes, &name)?;
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
            if let Some(filters) = value["engine"]["filters"].as_sequence() {
                for filter in filters.iter().filter_map(Value::as_str) {
                    let (kind, namespace) =
                        filter.split_once('@').unwrap_or((filter, "reverse_lookup"));
                    if kind == "reverse_lookup_filter" {
                        self.schemas
                            .get_mut(schema)
                            .unwrap()
                            .reverse_filters
                            .insert(namespace.into());
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
                        reverse_only || self.schemas[schema].reverse_filters.contains(key);
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
                        let path =
                            crate::packages::safe_path(self.root, "rime.lua").map_err(failure)?;
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
        let name = if self.root.join(&direct).is_file() {
            direct
        } else if self.root.join(&init).is_file() {
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
            let text = std::str::from_utf8(&self.files[&name])
                .map_err(|e| failure(format!("{name}: {e}")))?;
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
            let config: serde_json::Value = serde_json::from_slice(&self.files[&name])
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
                let file = [&relative, &root_relative]
                    .into_iter()
                    .find(|name| self.root.join(name).is_file())
                    .or_else(|| {
                        [&relative, &root_relative]
                            .into_iter()
                            .find(|name| self.shared_file(name))
                    })
                    .unwrap_or(&relative);
                self.add(schema, file)?;
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
    if schema_ids.is_empty() {
        return Err(failure("方案包缺少 schema_list/schema manifest"));
    }
    let mut collector = Collector {
        root,
        shared,
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
        let source = yaml(&collector.files[&source], &source)?;
        let compiled = format!("build/{id}.schema.yaml");
        let compiled_path = crate::packages::safe_path(root, &compiled).map_err(failure)?;
        let deployed = compiled_path.is_file();
        // The deployed schema contains Rime's resolved includes and custom patches.
        // Source references still supply the files needed to rebuild that schema.
        collector.scan(&id, &source, !deployed)?;
        let custom = format!("{id}.custom.yaml");
        if root.join(&custom).try_exists()? {
            collector.add(&id, &custom)?;
            let value = yaml(&collector.files[&custom], &custom)?;
            collector.scan(&id, &value, !deployed)?;
        }
        if deployed {
            let value = yaml(&fs::read(compiled_path)?, &compiled)?;
            collector.scan(&id, &value, true)?;
        }
    }
    let defaults = serde_yaml::to_string(&serde_json::json!({"patch":{"schema_list":schema_ids.iter().map(|id| serde_json::json!({"schema":id})).collect::<Vec<_>>()}})).map_err(|e| failure(e.to_string()))?;
    collector
        .files
        .insert("default.custom.yaml".into(), defaults.into_bytes());
    if !collector.bindings.is_empty() {
        collector.files.insert(
            "rime.lua".into(),
            collector
                .bindings
                .into_values()
                .collect::<String>()
                .into_bytes(),
        );
    }
    fn dictionary_inputs(
        id: &str,
        files: &BTreeMap<String, Vec<u8>>,
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
        let value = yaml(bytes, &name)?;
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
    let total_size = collector
        .files
        .values()
        .map(|bytes| bytes.len() as u64)
        .sum();
    let files = collector.files.keys().cloned().collect();
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(zip::DateTime::default())
        .unix_permissions(0o644);
    for (name, bytes) in collector.files {
        zip.start_file(name, options)
            .map_err(|e| failure(e.to_string()))?;
        zip.write_all(&bytes)?;
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
    })
}

pub fn verified(
    root: &Path,
    shared: Option<&Path>,
    schema_ids: &[String],
    mut smoke: impl FnMut(&str, u32) -> Result<(), String>,
) -> Result<UploadBundle, CoreError> {
    checked(root, shared, schema_ids, Some(&mut smoke))
}

pub fn verified_artifacts(
    root: &Path,
    shared: Option<&Path>,
    schema_ids: &[String],
) -> Result<UploadBundle, CoreError> {
    checked(root, shared, schema_ids, None)
}

fn checked(
    root: &Path,
    shared: Option<&Path>,
    schema_ids: &[String],
    mut smoke: Option<&mut dyn FnMut(&str, u32) -> Result<(), String>>,
) -> Result<UploadBundle, CoreError> {
    let bundle = build(root, shared, schema_ids)?;
    for (id, schema) in &bundle.schemas {
        let configs: BTreeSet<_> = schema
            .sources
            .iter()
            .filter(|name| name.ends_with(".yaml") && !name.ends_with(".dict.yaml"))
            .cloned()
            .collect();
        let compiled = format!("build/{id}.schema.yaml");
        verify_artifact(root, id, &compiled, &configs)?;
        for (dictionary, table) in &schema.dictionaries {
            let inputs = &schema.dictionary_sources[dictionary];
            if *table {
                verify_artifact(root, id, &format!("build/{dictionary}.table.bin"), inputs)?;
            }
            verify_artifact(root, id, &format!("build/{dictionary}.reverse.bin"), inputs)?;
        }
        for (prism, dictionary) in &schema.prisms {
            let mut sources = configs.clone();
            sources.extend(schema.dictionary_sources[dictionary].iter().cloned());
            verify_artifact(root, id, &format!("build/{prism}.prism.bin"), &sources)?;
        }
        let value = yaml(&fs::read(root.join(&compiled))?, &compiled)?;
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
            smoke(id, u32::from(alphabet))
                .map_err(|e| failure(format!("方案 {id}：输入冒烟检查失败: {e}")))?;
        }
    }
    Ok(bundle)
}

fn verify_artifact(
    root: &Path,
    id: &str,
    artifact: &str,
    sources: &BTreeSet<String>,
) -> Result<(), CoreError> {
    let path = crate::packages::safe_path(root, artifact).map_err(failure)?;
    let metadata = fs::metadata(path)
        .map_err(|e| failure(format!("方案 {id}：缺少构建产物 {artifact}: {e}")))?;
    if !metadata.is_file() {
        return Err(failure(format!("方案 {id}：构建产物不是文件 {artifact}")));
    }
    let built = metadata
        .modified()
        .map_err(|e| failure(format!("方案 {id}：{artifact}: {e}")))?;
    // Runtime-only Lua/OpenCC/text resources do not produce dictionary binaries.
    for source in sources {
        let path = crate::packages::safe_path(root, source).map_err(failure)?;
        let modified = fs::metadata(path)
            .and_then(|m| m.modified())
            .map_err(|e| failure(format!("方案 {id}：{source}: {e}")))?;
        if built <= modified {
            return Err(failure(format!(
                "方案 {id}：构建产物 {artifact} 不新于源文件 {source}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "upload_bundle_tests.rs"]
mod tests;
