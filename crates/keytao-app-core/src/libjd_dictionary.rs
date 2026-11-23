use crate::{
    Core, CoreError,
    package_store::{PackageFile, PackageStore, StoredPackage},
    packages::{safe_path, sha256_reader},
};
use keytao_core::{RuntimeDirs, libjd::MAX_DICTIONARY_TEXT_BYTES};
use serde_yaml::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_SCHEMA_SET_BYTES: usize = 16 * 1024 * 1024;
const MAX_DICTIONARY_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ROW_BYTES: u64 = 64 * 1024;

pub(crate) struct ConvertedDictionary {
    pub(crate) schema_id: String,
    pub(crate) schema_name: String,
    pub(crate) text: Vec<u8>,
    pub(crate) skipped: usize,
}

#[derive(Clone)]
struct Source<'a> {
    path: PathBuf,
    manifest: Option<&'a PackageFile>,
}

impl Source<'_> {
    fn open(&self, limit: u64) -> Result<File, CoreError> {
        let metadata = fs::symlink_metadata(&self.path)?;
        if !metadata.is_file() || metadata.len() > limit {
            return Err(CoreError::Other(format!(
                "Rime 文件无效或过大：{}",
                self.path.display()
            )));
        }
        Ok(File::open(&self.path)?)
    }

    fn verify(&self, input: &mut File) -> Result<(), CoreError> {
        if let Some(manifest) = self.manifest {
            if input.metadata()?.len() != manifest.size
                || sha256_reader((&mut *input).take(manifest.size + 1))? != manifest.sha256
            {
                return Err(CoreError::Other(format!(
                    "Rime 文件校验失败：{}",
                    manifest.path
                )));
            }
            input.rewind()?;
        }
        Ok(())
    }

    fn config(&self) -> Result<Vec<u8>, CoreError> {
        let mut input = self.open(MAX_CONFIG_BYTES)?;
        self.verify(&mut input)?;
        let mut bytes = Vec::new();
        input.take(MAX_CONFIG_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_CONFIG_BYTES {
            return Err(CoreError::Other("Rime 配置文件过大".into()));
        }
        Ok(bytes)
    }
}

fn runtime_source(root: &Path, relative: &str) -> Result<Option<Source<'static>>, CoreError> {
    let path = safe_path(root, relative).map_err(CoreError::Other)?;
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => Ok(Some(Source {
            path,
            manifest: None,
        })),
        Ok(_) => Err(CoreError::Other(format!("Rime 文件无效：{relative}"))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn runtime_yaml(root: &Path, relative: &str) -> Result<Option<Value>, CoreError> {
    runtime_source(root, relative)?
        .map(|source| yaml(&source.config()?, relative))
        .transpose()
}

fn yaml(bytes: &[u8], name: &str) -> Result<Value, CoreError> {
    serde_yaml::from_slice(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes))
        .map_err(|error| CoreError::Other(format!("无法读取 Rime 配置 {name}：{error}")))
}

pub(crate) fn convert(
    core: &Core,
    root: &Path,
    package: &str,
    mut progress: impl FnMut(usize, usize),
) -> Result<ConvertedDictionary, CoreError> {
    if package == crate::addon::EASY_EN_ADDON_ID {
        return Err(CoreError::Unsupported(
            "Easy English 不是独立输入方案".into(),
        ));
    }
    let _store_guard = core
        .pending_package
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let store = PackageStore::new(core);
    let (meta, paths) = if crate::package_schemas::is_addon_package_key(package) {
        addon_inventory(&store, package)?
    } else {
        let main = store.checked_package_path(package)?;
        if main.try_exists()? {
            PackageStore::file_inventory_at(&main, package, false)?
        } else if package == "snow_jiandao" {
            addon_inventory(&store, package)?
        } else {
            return Err(CoreError::Other("请先下载或导入此 Rime 方案".into()));
        }
    };
    let files: BTreeMap<_, _> = meta
        .files
        .iter()
        .zip(paths)
        .map(|(file, path)| {
            (
                file.path.clone(),
                Source {
                    path,
                    manifest: Some(file),
                },
            )
        })
        .collect();
    let runtime = RuntimeDirs::isolated_user_data_dir(root, package)
        .ok_or_else(|| CoreError::Other("Rime 方案目录无效".into()))?;
    let mut configs = Vec::new();
    let mut config_bytes = 0;
    for (name, source) in files.iter().filter(|(path, _)| {
        !path.contains('/')
            && (path.ends_with(".schema.yaml")
                || matches!(
                    path.as_str(),
                    "default.yaml" | "default.custom.yaml" | "default-custom.yaml"
                ))
    }) {
        let config = source.config()?;
        config_bytes += config.len();
        if config_bytes > MAX_SCHEMA_SET_BYTES {
            return Err(CoreError::Other("Rime 方案配置总量过大".into()));
        }
        configs.push((name.clone(), config));
    }
    let schemas =
        crate::package_schemas::enumerate(package, &configs, meta.selected_schemas.as_deref())?;
    let current = runtime_source(&runtime, ".keytao-current-schema")?
        .map(|source| source.config())
        .transpose()?
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|id| id.trim().to_owned());
    let previous = runtime_yaml(&runtime, "user.yaml")?.and_then(|value| {
        value["var"]["previously_selected_schema"]
            .as_str()
            .map(str::to_owned)
    });
    let schema = current
        .iter()
        .chain(previous.iter())
        .chain(meta.selected_schemas.iter().flatten())
        .find_map(|id| {
            schemas
                .iter()
                .find(|schema| schema.selected && &schema.id == id)
        })
        .or_else(|| schemas.iter().find(|schema| schema.selected))
        .or_else(|| schemas.first())
        .ok_or_else(|| CoreError::Other("Rime 方案没有可导入的配置".into()))?;
    let id = &schema.id;
    let filename = format!("{id}.schema.yaml");
    let (_, bytes) = configs
        .iter()
        .find(|(name, _)| name == &filename)
        .ok_or_else(|| CoreError::Other(format!("缺少 Rime 方案配置：{filename}")))?;
    let config = yaml(bytes, &filename)?;
    let name = config["schema"]["name"].as_str().unwrap_or(id);
    let shared = crate::deploy::deploy_shared_data_dir(core, root);
    let compiled = RuntimeDirs::for_package_in_store(root, &shared, &core.env().data_dir, package)
        .and_then(|dirs| {
            dirs.staging_dir
                .strip_prefix(&runtime)
                .ok()
                .map(Path::to_owned)
        })
        .map(|path| format!("{}/{filename}", path.to_string_lossy().replace('\\', "/")))
        .map(|path| runtime_yaml(&runtime, &path))
        .transpose()?
        .flatten();
    let dictionary = compiled
        .as_ref()
        .and_then(translator_dictionary)
        .or_else(|| translator_dictionary(&config))
        .ok_or_else(|| CoreError::Unsupported(format!("{name} 没有可导入的主词库")))?;

    let dictionaries = collect_tables(dictionary, &files, &runtime)?;
    let total = dictionaries.len();
    let mut converted = ConvertedDictionary {
        schema_id: id.clone(),
        schema_name: name.into(),
        text: Vec::new(),
        skipped: 0,
    };
    for (index, table) in dictionaries.into_iter().enumerate() {
        progress(index + 1, total);
        append_rows(&table, &mut converted.text, &mut converted.skipped)?;
    }
    if converted.text.is_empty() {
        return Err(CoreError::Unsupported(format!(
            "{name} 没有可导入的词条，ime-jd 支持 1 至 6 位小写字母编码及前置分号",
        )));
    }
    Ok(converted)
}

fn addon_inventory(
    store: &PackageStore<'_>,
    package: &str,
) -> Result<(StoredPackage, Vec<PathBuf>), CoreError> {
    let directory = store.checked_addon_path(package)?;
    if !directory.try_exists()? {
        return Err(CoreError::Other("请先下载或导入此 Rime 方案".into()));
    }
    crate::addon_store::validate_directory(&directory, package)?;
    let manifest = runtime_source(&directory, "package.json")?
        .ok_or_else(|| CoreError::Other("附加方案缺少词库清单".into()))?;
    let meta: StoredPackage = serde_json::from_slice(&manifest.config()?)?;
    let files = safe_path(&directory, "files").map_err(CoreError::Other)?;
    let paths = meta
        .files
        .iter()
        .map(|file| safe_path(&files, &file.path))
        .collect::<Result<Vec<_>, _>>()
        .map_err(CoreError::Other)?;
    Ok((meta, paths))
}

fn translator_dictionary(schema: &Value) -> Option<&str> {
    if let Some(dictionary) = schema["translator"]["dictionary"]
        .as_str()
        .filter(|name| !name.is_empty())
    {
        return Some(dictionary);
    }
    if let Some(translators) = schema["engine"]["translators"].as_sequence() {
        for component in translators.iter().filter_map(Value::as_str) {
            let (name, namespace) = component
                .split_once('@')
                .unwrap_or((component, "translator"));
            if matches!(name, "table_translator" | "script_translator")
                && let Some(dictionary) = schema[namespace.trim_start_matches('~')]["dictionary"]
                    .as_str()
                    .filter(|name| !name.is_empty())
            {
                return Some(dictionary);
            }
        }
    }
    None
}

struct Table<'a> {
    source: Source<'a>,
    text_column: usize,
    code_column: Option<usize>,
    data_offset: u64,
}

fn collect_tables<'a>(
    dictionary: &str,
    files: &BTreeMap<String, Source<'a>>,
    runtime: &Path,
) -> Result<Vec<Table<'a>>, CoreError> {
    let mut all = Vec::new();
    let mut completed = BTreeSet::new();
    let mut visiting = BTreeSet::new();
    let mut pending = vec![(dictionary.to_owned(), false)];
    let mut total_bytes = 0u64;
    while let Some((name, leave)) = pending.pop() {
        if leave {
            visiting.remove(&name);
            completed.insert(name);
            continue;
        }
        if completed.contains(&name) {
            continue;
        }
        if !visiting.insert(name.clone()) {
            return Err(CoreError::Other(format!("Rime 词库引用形成循环：{name}")));
        }
        let relative = format!("{name}.dict.yaml");
        crate::packages::validate_relative(&relative).map_err(CoreError::Other)?;
        let stored = files.get(&relative);
        let live = if name.ends_with(".user") || stored.is_none() {
            runtime_source(runtime, &relative)?
        } else {
            None
        };
        let source = live
            .or_else(|| stored.cloned())
            .ok_or_else(|| CoreError::Other(format!("缺少 Rime 词库：{name}")))?;
        let input = source.open(MAX_DICTIONARY_BYTES)?;
        total_bytes = total_bytes.saturating_add(input.metadata()?.len());
        if total_bytes > MAX_DICTIONARY_BYTES || all.len() >= 10_000 {
            return Err(CoreError::Other("待转换的 Rime 词库过大".into()));
        }
        let (config, data_offset) = read_header(BufReader::new(input), &name)?;
        let (text_column, code_column) = dictionary_columns(&config, &name)?;
        all.push(Table {
            source,
            text_column,
            code_column,
            data_offset,
        });
        pending.push((name, true));
        if let Some(imports) = config.get("import_tables").filter(|value| !value.is_null()) {
            let imports = imports
                .as_sequence()
                .ok_or_else(|| CoreError::Other("Rime 词库 import_tables 必须为列表".into()))?;
            if imports.len() > 10_000 || pending.len() + imports.len() > 20_000 {
                return Err(CoreError::Other("Rime 词库引用过多".into()));
            }
            for child in imports.iter().rev() {
                let child = child
                    .as_str()
                    .filter(|child| !child.is_empty())
                    .ok_or_else(|| CoreError::Other("Rime 词库引用无效".into()))?;
                pending.push((child.to_owned(), false));
            }
        }
    }
    Ok(all)
}

fn read_header(mut input: impl BufRead, name: &str) -> Result<(Value, u64), CoreError> {
    let mut header = Vec::new();
    loop {
        let offset = header.len();
        let read = (&mut input)
            .take(MAX_CONFIG_BYTES + 1 - offset as u64)
            .read_until(b'\n', &mut header)?;
        if header.len() as u64 > MAX_CONFIG_BYTES {
            return Err(CoreError::Other(format!("Rime 词库表头过大：{name}")));
        }
        if read == 0 || header[offset..].trim_ascii() == b"..." {
            break;
        }
    }
    Ok((yaml(&header, name)?, header.len() as u64))
}

fn dictionary_columns(config: &Value, name: &str) -> Result<(usize, Option<usize>), CoreError> {
    let Some(columns) = config.get("columns") else {
        return Ok((0, Some(1)));
    };
    let columns = columns
        .as_sequence()
        .ok_or_else(|| CoreError::Other(format!("Rime 词库列格式无效：{name}")))?;
    let position = |key| {
        columns
            .iter()
            .position(|column| column.as_str() == Some(key))
    };
    let text =
        position("text").ok_or_else(|| CoreError::Other(format!("Rime 词库缺少文本列：{name}")))?;
    Ok((text, position("code")))
}

fn append_rows(
    table: &Table<'_>,
    output: &mut Vec<u8>,
    skipped: &mut usize,
) -> Result<(), CoreError> {
    let mut input = table.source.open(MAX_DICTIONARY_BYTES)?;
    table.source.verify(&mut input)?;
    input.seek(SeekFrom::Start(table.data_offset))?;
    let mut reader = BufReader::new(input.take(MAX_DICTIONARY_BYTES));
    let mut line = Vec::new();
    let mut comments = true;
    loop {
        line.clear();
        if (&mut reader)
            .take(MAX_ROW_BYTES + 1)
            .read_until(b'\n', &mut line)?
            == 0
        {
            break;
        }
        if line.len() as u64 > MAX_ROW_BYTES {
            return Err(CoreError::Other("Rime 词库中有过长的词条".into()));
        }
        let line = line.strip_suffix(b"\n").unwrap_or(&line);
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        if comments && line.starts_with(b"#") {
            comments = line != b"# no comment";
            continue;
        }
        let (Some(word), Some(code)) = (
            line.split(|&byte| byte == b'\t').nth(table.text_column),
            table
                .code_column
                .and_then(|column| line.split(|&byte| byte == b'\t').nth(column)),
        ) else {
            *skipped += 1;
            continue;
        };
        let word = trim_spaces(word);
        let code = trim_spaces(code);
        if !compatible(word, code) {
            *skipped += 1;
            continue;
        }
        if output.len() + word.len() + code.len() + 2 > MAX_DICTIONARY_TEXT_BYTES {
            return Err(CoreError::Other("转换后的 ime-jd 词库超过 64 MiB".into()));
        }
        output.extend_from_slice(word);
        output.push(b'\t');
        output.extend_from_slice(code);
        output.push(b'\n');
    }
    Ok(())
}

fn trim_spaces(mut bytes: &[u8]) -> &[u8] {
    while let Some(rest) = bytes.strip_prefix(b" ") {
        bytes = rest;
    }
    while let Some(rest) = bytes.strip_suffix(b" ") {
        bytes = rest;
    }
    bytes
}

fn compatible(word: &[u8], code: &[u8]) -> bool {
    let mut keys = code.iter().copied().skip_while(|&key| key == b';');
    !word.is_empty()
        && word.len() <= 1023
        && !word.starts_with(b"#")
        && !word.iter().any(u8::is_ascii_control)
        && std::str::from_utf8(word).is_ok()
        && (1..=6).contains(&code.len())
        && keys.all(|key| key.is_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppEnv, CoreEvent, EventSink, Platform, packages};
    use std::sync::Arc;

    struct Sink;
    impl EventSink for Sink {
        fn emit(&self, _: CoreEvent) {}
    }

    fn core(root: &Path) -> Arc<Core> {
        Core::new(
            AppEnv {
                data_dir: root.join("app"),
                cache_dir: root.join("cache"),
                resource_dir: root.join("resources"),
                app_version: "test".into(),
                platform: Platform::Android,
            },
            Arc::new(Sink),
        )
        .unwrap()
    }

    #[test]
    fn converts_selected_schema_in_import_order_with_runtime_user_words() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let core = core(root);
        let files: Vec<_> = [
            ("default.yaml", "schema_list: [{schema: other}, {schema: main}]\n"),
            ("other.schema.yaml", "schema: {schema_id: other}\n"),
            ("main.schema.yaml", "schema: {schema_id: main, name: 测试}\ntranslator: {dictionary: main}\n"),
            ("main.dict.yaml", "---\nname: main\ncolumns: [code, text, weight]\nimport_tables: [dicts/first, dicts/last, main.user]\n...\r\nab\t主\t50\r\nabcdefgh\t长\t0\r\n"),
            ("dicts/first.dict.yaml", "name: first\nimport_tables: [dicts/shared]\n...\n一\tab\n"),
            ("dicts/shared.dict.yaml", "name: shared\n...\n共享\tab\n拼音\tni hao\n"),
            ("dicts/last.dict.yaml", "name: last\nimport_tables: [dicts/shared]\n...\n末\t;;a\n"),
            ("main.user.dict.yaml", "name: main.user\n...\n旧\tab\n"),
        ].into_iter().map(|(name, content)| (name.into(), content.as_bytes().to_vec())).collect();
        let archive = packages::archive_from_files(&files).unwrap();
        packages::store_cloud_archive(
            &core,
            "keytao",
            Some("test"),
            "official",
            None,
            &archive,
            root,
        )
        .unwrap();
        let runtime = root.join("runtime/keytao");
        fs::create_dir_all(&runtime).unwrap();
        fs::write(
            runtime.join("user.yaml"),
            "var: {previously_selected_schema: main}\n",
        )
        .unwrap();
        fs::write(
            runtime.join("main.user.dict.yaml"),
            "name: main.user\n...\n新\tab\n",
        )
        .unwrap();

        let mut progress = Vec::new();
        let result = convert(&core, root, "keytao", |i, n| progress.push((i, n))).unwrap();
        assert_eq!(result.schema_id, "main");
        assert_eq!(result.schema_name, "测试");
        assert_eq!(
            result.text,
            "主\tab\n一\tab\n共享\tab\n末\t;;a\n新\tab\n".as_bytes()
        );
        assert_eq!(result.skipped, 2);
        assert_eq!(progress, [(1, 5), (2, 5), (3, 5), (4, 5), (5, 5)]);
    }

    #[test]
    fn converts_an_unmaterialized_addon_from_the_addon_store() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let core = core(root);
        let source = tempfile::tempdir().unwrap();
        let files = [
            (
                "rime_ice.schema.yaml",
                "schema: {schema_id: rime_ice}\ntranslator: {dictionary: rime_ice}\n",
            ),
            ("rime_ice.dict.yaml", "name: rime_ice\n...\n冰\tbing\n"),
        ]
        .into_iter()
        .map(|(path, text)| {
            fs::write(source.path().join(path), text).unwrap();
            serde_json::json!({
                "path": path,
                "sha256": sha256_reader(text.as_bytes()).unwrap(),
                "size": text.len(),
            })
        })
        .collect::<Vec<_>>();
        let manifest = serde_json::json!({
            "format": 1,
            "version": "test",
            "source": "official",
            "archive_sha256": "a".repeat(64),
            "files": files,
        });
        crate::addon_store::AddonStore::new(&core, root, "rime_ice")
            .unwrap()
            .commit_isolated(source.path(), &serde_json::to_vec(&manifest).unwrap())
            .unwrap();

        let mut progress = Vec::new();
        let result = convert(&core, root, "rime_ice", |i, n| progress.push((i, n))).unwrap();
        assert_eq!(result.schema_id, "rime_ice");
        assert_eq!(result.text, "冰\tbing\n".as_bytes());
        assert_eq!(progress, [(1, 1)]);
    }

    #[test]
    fn refuses_cycles_and_import_paths_outside_the_runtime() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for import in ["main", "../external"] {
            fs::write(
                root.join("main.dict.yaml"),
                format!("name: main\nimport_tables: ['{import}']\n...\n"),
            )
            .unwrap();
            assert!(collect_tables("main", &BTreeMap::new(), root).is_err());
        }
    }
}
