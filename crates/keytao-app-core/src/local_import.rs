use super::{PackageMeta, excluded_from_manifest, safe_path, sha256_reader, validate_relative};
use crate::generic_rime::{DEFAULT_MAX_EXPANDED_BYTES, NOMEDIA_FILE, has_excluded_hidden_component};
use crate::package_store::{PackageStore, Staging};
use crate::{Core, CoreError, package_fs::context};
use keytao_core::runtime_log::AppOperation;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Take, Write},
    path::Path,
};

pub(crate) const MAX_LOCAL_IMPORT_FILES: usize = 10_000;
pub(crate) const MAX_LOCAL_IMPORT_MB: u64 = DEFAULT_MAX_EXPANDED_BYTES / (1024 * 1024);
const MAX_ZIP_RAW_ENTRIES: usize = MAX_LOCAL_IMPORT_FILES * 10;
const MAX_ZIP_UNWRAP_DEPTH: usize = 3;

pub(crate) fn oversized_local_error(relative: &str, bytes: u64) -> CoreError {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    CoreError::Other(format!(
        "方案体积超过 {MAX_LOCAL_IMPORT_MB} MB 限制（在 {relative} 处已达 {mb:.1} MB）。请导入纯 Rime 文件夹或 Rime 方案 .zip 压缩包（系统会自动过滤 build 目录）"
    ))
}

pub(crate) fn too_many_files_local_error() -> CoreError {
    CoreError::Other(format!(
        "方案文件过多（超过 {MAX_LOCAL_IMPORT_FILES} 个限制）。请导入纯 Rime 文件夹或 Rime 方案 .zip 压缩包（系统会自动过滤 build 目录）"
    ))
}

fn ships_with_scheme(name: &str) -> bool {
    name.ends_with(".user.dict.yaml") || name.ends_with("_user.txt") || name == "user_dict.txt"
}

fn is_excluded_top_entry(top: &str) -> bool {
    matches!(
        top,
        "build" | "sync" | "log" | "logs" | "installation.yaml" | "user.yaml" | "__MACOSX"
    )
}

fn excluded_common(relative: &str, exclude_default_custom: bool) -> bool {
    let top = relative.split('/').next().unwrap_or(relative);
    is_excluded_top_entry(top)
        || has_excluded_hidden_component(relative)
        || relative.split('/').any(|part| {
            part == "__MACOSX"
                || part.eq_ignore_ascii_case("System Volume Information")
                || part.eq_ignore_ascii_case("$RECYCLE.BIN")
                || part.eq_ignore_ascii_case("app-state.json")
                || part.ends_with(".userdb")
                || part.ends_with(".leveldb")
                || (exclude_default_custom
                    && matches!(part, "default.custom.yaml" | "default-custom.yaml"))
                // Schemes import their user dictionaries, so ship them; install keeps an
                // existing copy in the Rime root and only fills in a missing one.
                || (part != "user.yaml"
                    && !matches!(part, "default.custom.yaml" | "default-custom.yaml")
                    && !ships_with_scheme(part)
                    && crate::install::is_protected_user_file(part))
        })
}

fn excluded(relative: &str) -> bool {
    excluded_common(relative, true)
}

fn excluded_from_archive_extract(relative: &str) -> bool {
    excluded_common(relative, false)
}

#[cfg(any(windows, test))]
fn excluded_attributes(attributes: u32, allow_hidden_marker: bool) -> bool {
    // Windows hidden/system flags; readonly scheme files are still importable.
    let excluded = if allow_hidden_marker { 0x4 } else { 0x2 | 0x4 };
    attributes & excluded != 0
}

fn import_io<T>(
    result: std::io::Result<T>,
    operation: &str,
    path: &Path,
) -> Result<T, CoreError> {
    result.map_err(|error| context(operation, path, error).into())
}

#[derive(Clone)]
enum ImportFile {
    Disk(u64),
    Bytes(Vec<u8>),
}

impl ImportFile {
    fn byte_len(&self) -> u64 {
        match self {
            Self::Disk(size) => *size,
            Self::Bytes(bytes) => bytes.len() as u64,
        }
    }
}

/// Cap total scan size during directory collection so multi-schema folders
/// whose individual split packages each stay under `DEFAULT_MAX_EXPANDED_BYTES`
/// are not prematurely rejected before splitting.
const MAX_MULTI_SCHEMA_SCAN_BYTES: u64 = DEFAULT_MAX_EXPANDED_BYTES * 8;

fn changed_source(path: &Path) -> CoreError {
    CoreError::Other(format!("导入期间方案文件已更改：{}", path.display()))
}

fn open_source(path: &Path, size: u64) -> Result<BufReader<Take<fs::File>>, CoreError> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0);
    }
    let file = import_io(options.open(path), "读取文件", path)?;
    let metadata = import_io(file.metadata(), "读取文件属性", path)?;
    if !metadata.is_file() || metadata.len() != size {
        return Err(changed_source(path));
    }
    Ok(BufReader::with_capacity(64 * 1024, file.take(size + 1)))
}

fn read_source(path: &Path, size: u64) -> Result<Vec<u8>, CoreError> {
    let mut bytes = Vec::new();
    import_io(open_source(path, size)?.read_to_end(&mut bytes), "读取文件", path)?;
    if bytes.len() as u64 != size {
        return Err(changed_source(path));
    }
    Ok(bytes)
}

pub(super) fn has_root_schema(directory: &Path) -> Result<bool, CoreError> {
    let entries = fs::read_dir(directory).map_err(|error| context("读取目录", directory, error))?;
    for entry in entries {
        let entry = import_io(entry, "读取目录项", directory)?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.ends_with(".schema.yaml") || excluded(name) {
            continue;
        }
        let metadata = import_io(entry.metadata(), "读取文件属性", &entry.path())?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if excluded_attributes(metadata.file_attributes(), false) {
                continue;
            }
        }
        if metadata.is_file() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn collect(
    directory: &Path,
    prefix: &str,
    files: &mut BTreeMap<String, ImportFile>,
    size: &mut u64,
    skip_trime_assets: bool,
) -> Result<(), CoreError> {
    let entries = import_io(fs::read_dir(directory), "读取目录", directory)?;
    for entry in entries {
        let entry = import_io(entry, "读取目录项", directory)?;
        let path = entry.path();
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| CoreError::Other(format!("方案包文件名无效：{}", path.display())))?;
        let relative = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if excluded(&relative) {
            tracing::warn!(path = %path.display(), "Skipping hidden, protected or runtime import entry");
            continue;
        }
        validate_relative(&relative).map_err(CoreError::Other)?;
        if skip_trime_assets && crate::generic_rime::is_trime_asset(&relative) {
            continue;
        }
        let metadata = import_io(entry.metadata(), "读取文件属性", &path)?;
        let kind = metadata.file_type();
        let is_nomedia = relative.rsplit('/').next() == Some(NOMEDIA_FILE);
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if excluded_attributes(metadata.file_attributes(), is_nomedia && kind.is_file()) {
                tracing::warn!(path = %path.display(), "Skipping hidden or system import entry");
                continue;
            }
        }
        if kind.is_symlink() {
            return Err(CoreError::Other(format!(
                "方案包不能包含符号链接：{}",
                path.display()
            )));
        }
        if kind.is_dir() && !is_nomedia {
            collect(&path, &relative, files, size, skip_trime_assets)?;
        } else if kind.is_file() {
            if files.len() >= MAX_LOCAL_IMPORT_FILES {
                return Err(too_many_files_local_error());
            }
            let next_size = size.saturating_add(metadata.len());
            if next_size > MAX_MULTI_SCHEMA_SCAN_BYTES {
                return Err(oversized_local_error(&relative, next_size));
            }
            *size = next_size;
            let file = if relative.ends_with(".custom.yaml") {
                let bytes = read_source(&path, metadata.len())?;
                let content = std::str::from_utf8(&bytes)
                    .map_err(|error| CoreError::Other(error.to_string()))?;
                let bytes = crate::managed_custom::strip(content)
                    .map_err(CoreError::Other)?
                    .into_bytes();
                if bytes.is_empty() {
                    continue;
                }
                ImportFile::Bytes(bytes)
            } else if !relative.contains('/') && relative.ends_with(".schema.yaml") {
                ImportFile::Bytes(read_source(&path, metadata.len())?)
            } else if relative == crate::remove_filter::SCRIPT_PATH
                && metadata.len() == crate::remove_filter::SCRIPT.len() as u64
            {
                let bytes = read_source(&path, metadata.len())?;
                if crate::managed_custom::is_generated(&relative, &bytes) {
                    continue;
                }
                ImportFile::Bytes(bytes)
            } else {
                ImportFile::Disk(metadata.len())
            };
            files.insert(relative, file);
        }
    }
    Ok(())
}

fn sanitize_zip_entry(raw: &str) -> Result<Option<String>, CoreError> {
    let normalized = raw
        .trim_start_matches("./")
        .trim_start_matches('/')
        .trim_end_matches('/');
    if normalized.is_empty()
        || normalized == "__MACOSX"
        || normalized.starts_with("__MACOSX/")
    {
        return Ok(None);
    }
    validate_relative(normalized).map_err(CoreError::Other)?;
    Ok(Some(normalized.to_owned()))
}

fn detect_zip_scheme_prefix(entries: &[(usize, String, bool)]) -> Option<String> {
    let mut current = String::new();
    for _ in 0..=MAX_ZIP_UNWRAP_DEPTH {
        let mut child_dirs = BTreeSet::new();
        for (_, path, _) in entries {
            let Some(rest) = path.strip_prefix(&current) else {
                continue;
            };
            if !rest.contains('/') {
                if rest.ends_with(".schema.yaml") && !excluded(rest) {
                    return Some(current);
                }
                continue;
            }
            if let Some((child, _)) = rest.split_once('/')
                && !child.is_empty()
                && !child.starts_with('.')
                && !is_excluded_top_entry(child)
            {
                child_dirs.insert(child);
            }
        }
        let mut with_schema = Vec::new();
        for &child in &child_dirs {
            let child_prefix = format!("{current}{child}/");
            let has_schema = entries.iter().any(|(_, path, _)| {
                path.strip_prefix(&child_prefix).is_some_and(|rest| {
                    !rest.contains('/') && rest.ends_with(".schema.yaml") && !excluded(rest)
                })
            });
            if has_schema {
                with_schema.push(child);
            }
        }
        if with_schema.len() == 1 {
            return Some(format!("{current}{}/", with_schema[0]));
        }
        if child_dirs.len() == 1 {
            let only = child_dirs.into_iter().next().unwrap();
            current = format!("{current}{only}/");
        } else {
            break;
        }
    }
    None
}

pub(super) fn extract_filtered_zip(
    file: fs::File,
    dest: &Path,
    reject_symlinks: bool,
) -> Result<(), CoreError> {
    let mut archive = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|error| CoreError::Other(format!("无法读取方案压缩包：{error}")))?;
    if archive.len() > MAX_ZIP_RAW_ENTRIES {
        return Err(too_many_files_local_error());
    }
    let mut entries = Vec::new();
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| CoreError::Other(format!("读取压缩包条目失败：{error}")))?;
        if entry.is_dir() || entry.name().ends_with('/') {
            continue;
        }
        let Some(relative) = sanitize_zip_entry(entry.name())? else {
            continue;
        };
        let is_symlink = entry.is_symlink()
            || entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 == 0o120000);
        entries.push((index, relative, is_symlink));
    }

    let Some(prefix) = detect_zip_scheme_prefix(&entries) else {
        return Err(CoreError::Other(
            "压缩包中未找到 .schema.yaml 方案文件，请导入纯 Rime 文件夹或 Rime 方案 .zip 压缩包"
                .into(),
        ));
    };

    let mut seen_lower = BTreeSet::new();
    let mut count = 0usize;
    let mut total = 0u64;
    for (index, raw_path, is_symlink) in entries {
        let Some(relative) = raw_path.strip_prefix(&prefix) else {
            continue;
        };
        if relative.is_empty() || excluded_from_archive_extract(relative) {
            continue;
        }
        if is_symlink {
            if reject_symlinks {
                return Err(CoreError::Other(format!(
                    "方案包不能包含符号链接：{relative}"
                )));
            }
            continue;
        }
        if !seen_lower.insert(relative.to_lowercase()) {
            return Err(CoreError::Other(format!(
                "方案压缩包存在大小写冲突文件：{relative}"
            )));
        }
        count += 1;
        if count > MAX_LOCAL_IMPORT_FILES {
            return Err(too_many_files_local_error());
        }
        let mut entry = archive
            .by_index(index)
            .map_err(|error| CoreError::Other(format!("读取压缩包条目失败：{error}")))?;
        let expected = entry.size();
        total = total.saturating_add(expected);
        if total > DEFAULT_MAX_EXPANDED_BYTES {
            return Err(oversized_local_error(relative, total));
        }
        let target = safe_path(dest, relative).map_err(CoreError::Other)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = import_io(fs::File::create_new(&target), "写入解压文件", &target)?;
        let written = std::io::copy(&mut entry.by_ref().take(expected.saturating_add(1)), &mut out)?;
        if written != expected {
            return Err(CoreError::Other("压缩包文件大小不符".into()));
        }
    }
    Ok(())
}

fn strip_zip_extension(name: &str) -> &str {
    let trimmed = name.trim();
    match trimmed.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() && extension.eq_ignore_ascii_case("zip") => {
            stem
        }
        _ => trimmed,
    }
}

fn import_local_zip_scheme(
    core: &Core,
    archive_path: &Path,
    display_name: &str,
    scheme_key: Option<&str>,
) -> Result<String, CoreError> {
    let mut file = import_io(fs::File::open(archive_path), "读取方案压缩包", archive_path)?;
    let mut magic = [0u8; 4];
    let read = import_io(file.read(&mut magic), "读取方案压缩包", archive_path)?;
    if read < 4 || &magic[..2] != b"PK" || !matches!(magic[2..4], [3, 4] | [5, 6]) {
        return Err(CoreError::Other(
            "仅支持导入纯 Rime 文件夹或 Rime 方案 .zip 压缩包".into(),
        ));
    }
    import_io(file.seek(SeekFrom::Start(0)), "读取方案压缩包", archive_path)?;

    let cache_dir = &core.env().cache_dir;
    fs::create_dir_all(cache_dir)?;
    let stage = tempfile::Builder::new()
        .prefix(".local-zip-")
        .tempdir_in(cache_dir)?;
    extract_filtered_zip(file, stage.path(), true)?;

    let stem = archive_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(strip_zip_extension)
        .filter(|name| !name.is_empty());
    let cleaned_display_name = strip_zip_extension(display_name);
    let origin = archive_path.to_string_lossy().into_owned();
    import_local_scheme_with(
        core,
        stage.path(),
        cleaned_display_name,
        scheme_key,
        LocalImportOptions {
            slug_hint: stem,
            version_override: None,
            origin: Some(&origin),
            git: None,
        },
    )
}

fn build_default_custom(
    defaults: &[(String, Vec<u8>)],
    schemas: &[String],
) -> Result<Vec<u8>, CoreError> {
    let existing = defaults
        .iter()
        .find(|(name, _)| name == "default.custom.yaml")
        .or_else(|| defaults.iter().find(|(name, _)| name == "default-custom.yaml"));
    if let Some((_, raw)) = existing
        && let Ok(text) = std::str::from_utf8(raw)
        && let Ok(stripped) = crate::managed_custom::strip(text)
        && let Ok(serde_yaml::Value::Mapping(mut root)) = serde_yaml::from_str(&stripped)
        && root
            .get(serde_yaml::Value::String("patch".into()))
            .is_some_and(serde_yaml::Value::is_mapping)
    {
        if let Some(patch) = root
            .get_mut(serde_yaml::Value::String("patch".into()))
            .and_then(serde_yaml::Value::as_mapping_mut)
        {
            patch.retain(|key, _| {
                !key.as_str()
                    .is_some_and(|k| k == "schema_list" || k.starts_with("schema_list/"))
            });
            let list: Vec<_> = schemas
                .iter()
                .map(|id| BTreeMap::from([("schema", id.as_str())]))
                .collect();
            patch.insert(
                "schema_list".into(),
                serde_yaml::to_value(list).map_err(|e| CoreError::Other(e.to_string()))?,
            );
            return serde_yaml::to_string(&root)
                .map(String::into_bytes)
                .map_err(|e| CoreError::Other(e.to_string()));
        }
    }
    let mut config = String::from("patch:\n  schema_list:\n");
    for schema in schemas {
        config.push_str(&format!(
            "    - schema: {}\n",
            serde_json::to_string(schema)?
        ));
    }
    Ok(config.into_bytes())
}

fn slug_from_str(name: &str) -> String {
    let mut slug = String::new();
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
        if slug.len() >= 80 {
            break;
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "scheme".into()
    } else {
        slug.into()
    }
}

fn slug(directory: &Path) -> String {
    slug_from_str(
        directory
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(""),
    )
}

fn infer_schema_display_name(
    files: &BTreeMap<String, ImportFile>,
    schemas: &[String],
) -> Option<String> {
    for schema_id in schemas {
        let Some(ImportFile::Bytes(bytes)) = files.get(&format!("{schema_id}.schema.yaml")) else {
            continue;
        };
        let Ok(value) = serde_yaml::from_slice::<serde_yaml::Value>(bytes) else {
            continue;
        };
        if let Some(name) = value
            .get("schema")
            .and_then(|s| s.get("name"))
            .and_then(serde_yaml::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Some(name.to_owned());
        }
    }
    None
}

fn collect_yaml_schema_refs(
    value: &serde_yaml::Value,
    dep_schemas: &mut BTreeSet<String>,
    dicts: &mut BTreeSet<String>,
) {
    match value {
        serde_yaml::Value::Mapping(map) => {
            for (k, v) in map {
                let Some(key) = k.as_str() else {
                    collect_yaml_schema_refs(v, dep_schemas, dicts);
                    continue;
                };
                match key {
                    "dependencies" => {
                        if let Some(seq) = v.as_sequence() {
                            for item in seq {
                                if let Some(id) = item.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                                    dep_schemas.insert(id.to_owned());
                                }
                            }
                        }
                    }
                    "__include" => {
                        let push_ref = |raw: &str, deps: &mut BTreeSet<String>| {
                            let id = raw
                                .split(':')
                                .next()
                                .unwrap_or_default()
                                .trim()
                                .trim_end_matches(".schema.yaml")
                                .trim_end_matches(".schema");
                            if !id.is_empty() {
                                deps.insert(id.to_owned());
                            }
                        };
                        if let Some(s) = v.as_str() {
                            push_ref(s, dep_schemas);
                        } else if let Some(seq) = v.as_sequence() {
                            for item in seq {
                                if let Some(s) = item.as_str() {
                                    push_ref(s, dep_schemas);
                                }
                            }
                        }
                    }
                    "dictionary" | "user_dict" => {
                        if let Some(name) = v.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                            dicts.insert(name.to_owned());
                        }
                    }
                    _ => collect_yaml_schema_refs(v, dep_schemas, dicts),
                }
            }
        }
        serde_yaml::Value::Sequence(seq) => {
            for item in seq {
                collect_yaml_schema_refs(item, dep_schemas, dicts);
            }
        }
        _ => {}
    }
}

/// Reads only the YAML header (up to `...` or 64 KiB) of a `.dict.yaml` file
/// to extract its `name` and `import_tables` without loading large word lists.
fn parse_dict_header(
    directory: &Path,
    relative: &str,
    file: &ImportFile,
) -> (Option<String>, Vec<String>) {
    let header_text = match file {
        ImportFile::Bytes(bytes) => {
            let cap = bytes.len().min(64 * 1024);
            let raw = String::from_utf8_lossy(&bytes[..cap]);
            extract_yaml_header(&raw)
        }
        ImportFile::Disk(size) => {
            let Ok(path) = safe_path(directory, relative) else {
                return (None, Vec::new());
            };
            let Ok(reader) = open_source(&path, *size) else {
                return (None, Vec::new());
            };
            let mut out = String::new();
            let mut bytes_read = 0usize;
            for line in reader.lines().map_while(Result::ok) {
                bytes_read += line.len() + 1;
                let trimmed = line.trim();
                if trimmed == "..." {
                    break;
                }
                out.push_str(&line);
                out.push('\n');
                if bytes_read >= 64 * 1024 {
                    break;
                }
            }
            out
        }
    };
    let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(&header_text) else {
        return (None, Vec::new());
    };
    let name = value
        .get("name")
        .and_then(serde_yaml::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let imports = value
        .get("import_tables")
        .and_then(serde_yaml::Value::as_sequence)
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(str::trim))
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    (name, imports)
}

fn extract_yaml_header(raw: &str) -> String {
    let mut out = String::new();
    for line in raw.lines() {
        if line.trim() == "..." {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn dict_stem(relative: &str) -> Option<&str> {
    let file_name = relative.rsplit('/').next().unwrap_or(relative);
    file_name.strip_suffix(".dict.yaml")
}

struct SchemaPartition {
    owned_schemas: BTreeSet<String>,
    owned_dict_paths: BTreeSet<String>,
}

fn compute_schema_partitions(
    directory: &Path,
    files: &BTreeMap<String, ImportFile>,
    primary_schemas: &[String],
) -> BTreeMap<String, SchemaPartition> {
    let all_schema_ids: BTreeSet<String> = files
        .keys()
        .filter(|name| !name.contains('/'))
        .filter_map(|name| name.strip_suffix(".schema.yaml").map(str::to_owned))
        .collect();

    // Map dictionary logical names (and file stems) -> relative paths, plus import_tables edges.
    let mut dict_name_to_paths: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut dict_path_imports: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (relative, file) in files {
        let Some(stem) = dict_stem(relative) else {
            continue;
        };
        dict_name_to_paths
            .entry(stem.to_owned())
            .or_default()
            .push(relative.clone());
        let (declared_name, imports) = parse_dict_header(directory, relative, file);
        if let Some(name) = declared_name
            && name != stem
        {
            dict_name_to_paths
                .entry(name)
                .or_default()
                .push(relative.clone());
        }
        dict_path_imports.insert(relative.clone(), imports);
    }

    let mut partitions = BTreeMap::new();
    for primary in primary_schemas {
        let mut owned_schemas = BTreeSet::new();
        let mut schema_queue = vec![primary.clone()];
        let mut dict_queue: Vec<String> = Vec::new();
        let mut seen_dict_names = BTreeSet::new();

        while let Some(schema_id) = schema_queue.pop() {
            if !owned_schemas.insert(schema_id.clone()) {
                continue;
            }
            for candidate in [
                format!("{schema_id}.schema.yaml"),
                format!("{schema_id}.custom.yaml"),
            ] {
                if let Some(ImportFile::Bytes(bytes)) = files.get(&candidate)
                    && let Ok(value) = serde_yaml::from_slice::<serde_yaml::Value>(bytes)
                {
                    let mut deps = BTreeSet::new();
                    let mut dicts = BTreeSet::new();
                    collect_yaml_schema_refs(&value, &mut deps, &mut dicts);
                    for dep in deps {
                        if all_schema_ids.contains(&dep) && !owned_schemas.contains(&dep) {
                            schema_queue.push(dep);
                        }
                    }
                    for dict in dicts {
                        if seen_dict_names.insert(dict.clone()) {
                            dict_queue.push(dict);
                        }
                    }
                }
            }
            // Implicitly associate `<schema_id>.dict.yaml` if present.
            if seen_dict_names.insert(schema_id.clone()) {
                dict_queue.push(schema_id);
            }
        }

        let mut owned_dict_paths = BTreeSet::new();
        while let Some(dict_name) = dict_queue.pop() {
            let Some(paths) = dict_name_to_paths.get(&dict_name) else {
                continue;
            };
            for path in paths {
                if !owned_dict_paths.insert(path.clone()) {
                    continue;
                }
                if let Some(imports) = dict_path_imports.get(path) {
                    for imported in imports {
                        if seen_dict_names.insert(imported.clone()) {
                            dict_queue.push(imported.clone());
                        }
                    }
                }
            }
        }

        partitions.insert(
            primary.clone(),
            SchemaPartition {
                owned_schemas,
                owned_dict_paths,
            },
        );
    }
    partitions
}

fn filter_files_for_schema(
    all_files: &BTreeMap<String, ImportFile>,
    target_schema: &str,
    partitions: &BTreeMap<String, SchemaPartition>,
) -> BTreeMap<String, ImportFile> {
    let Some(target) = partitions.get(target_schema) else {
        return all_files.clone();
    };
    let all_claimed_dicts: BTreeSet<&str> = partitions
        .values()
        .flat_map(|p| p.owned_dict_paths.iter().map(String::as_str))
        .collect();
    let other_primary_schemas: Vec<&str> = partitions
        .keys()
        .filter(|id| id.as_str() != target_schema && !target.owned_schemas.contains(id.as_str()))
        .map(String::as_str)
        .collect();

    let mut selected = BTreeMap::new();
    for (relative, file) in all_files {
        // 1. Root `.schema.yaml` / `.custom.yaml`: keep only schemas owned by target.
        if !relative.contains('/') {
            if let Some(id) = relative.strip_suffix(".schema.yaml") {
                if target.owned_schemas.contains(id) {
                    selected.insert(relative.clone(), file.clone());
                }
                continue;
            }
            if let Some(id) = relative.strip_suffix(".custom.yaml")
                && !matches!(id, "default" | "squirrel" | "weasel")
            {
                if target.owned_schemas.contains(id) {
                    selected.insert(relative.clone(), file.clone());
                }
                continue;
            }
        }
        // 2. Dictionary files (`*.dict.yaml`): separate per schema; keep unowned shared dicts.
        if let Some(stem) = dict_stem(relative) {
            if target.owned_dict_paths.contains(relative) {
                selected.insert(relative.clone(), file.clone());
            } else if !all_claimed_dicts.contains(relative.as_str()) {
                let belongs_to_other = other_primary_schemas.iter().any(|other| {
                    stem == *other
                        || stem
                            .strip_prefix(other)
                            .is_some_and(|rest| rest.starts_with('.') || rest.starts_with('_') || rest.starts_with('-'))
                });
                if !belongs_to_other {
                    selected.insert(relative.clone(), file.clone());
                }
            }
            continue;
        }
        // 3. Shared/common files (lua/, rime.lua, opencc/, default.yaml, symbols, etc.):
        //    keep in every split package.
        selected.insert(relative.clone(), file.clone());
    }
    selected
}

fn build_archive(
    directory: &Path,
    files: BTreeMap<String, ImportFile>,
    output: &mut fs::File,
) -> Result<BTreeMap<String, String>, CoreError> {
    let mut manifest = BTreeMap::new();
    let mut archive = zip::ZipWriter::new(output);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .last_modified_time(zip::DateTime::default())
        .unix_permissions(0o644);
    for (name, file) in files {
        archive
            .start_file(&name, options)
            .map_err(|error| CoreError::Other(error.to_string()))?;
        let digest = match file {
            ImportFile::Bytes(bytes) => {
                archive.write_all(&bytes)?;
                format!("{:x}", Sha256::digest(&bytes))
            }
            ImportFile::Disk(size) => {
                let path = safe_path(directory, &name).map_err(CoreError::Other)?;
                let mut input = open_source(&path, size)?;
                let mut digest = Sha256::new();
                let mut written = 0;
                loop {
                    let bytes = import_io(input.fill_buf(), "读取文件", &path)?;
                    if bytes.is_empty() {
                        break;
                    }
                    let len = bytes.len();
                    written += len as u64;
                    if written > size {
                        return Err(changed_source(&path));
                    }
                    archive.write_all(bytes)?;
                    digest.update(bytes);
                    input.consume(len);
                }
                if written != size {
                    return Err(changed_source(&path));
                }
                format!("{:x}", digest.finalize())
            }
        };
        if !excluded_from_manifest(&name) {
            manifest.insert(name, digest);
        }
    }
    archive
        .finish()
        .map_err(|error| CoreError::Other(error.to_string()))?;
    Ok(manifest)
}

#[derive(Default)]
pub(super) struct LocalImportOptions<'a> {
    pub slug_hint: Option<&'a str>,
    pub version_override: Option<&'a str>,
    pub origin: Option<&'a str>,
    pub git: Option<super::GitSchemeConfig>,
}

pub fn import_local_scheme(
    core: &Core,
    path: &Path,
    display_name: &str,
    scheme_key: Option<&str>,
) -> Result<String, CoreError> {
    let canonical = path
        .canonicalize()
        .map_err(|error| context("读取方案路径", path, error))?;
    if canonical.is_dir() {
        import_local_scheme_with(
            core,
            &canonical,
            display_name,
            scheme_key,
            LocalImportOptions::default(),
        )
    } else if canonical.is_file() {
        import_local_zip_scheme(core, &canonical, display_name, scheme_key)
    } else {
        Err(CoreError::Other(
            "请选择纯 Rime 文件夹或 Rime 方案 .zip 压缩包".into(),
        ))
    }
}

pub(super) fn import_local_scheme_with(
    core: &Core,
    directory: &Path,
    display_name: &str,
    scheme_key: Option<&str>,
    options: LocalImportOptions<'_>,
) -> Result<String, CoreError> {
    let _operation = AppOperation::start("import_local_scheme");
    let directory = directory
        .canonicalize()
        .map_err(|error| context("读取方案目录", directory, error))?;
    if !directory.is_dir() {
        return Err(CoreError::Other(
            "请选择纯 Rime 文件夹或 Rime 方案 .zip 压缩包".into(),
        ));
    }
    if !has_root_schema(&directory)? {
        return Err(CoreError::Other(
            "目录根部未找到 .schema.yaml 方案文件，请导入纯 Rime 文件夹或 Rime 方案 .zip 压缩包"
                .into(),
        ));
    }
    let mut files = BTreeMap::new();
    let mut total_size = 0;
    let skip_trime_assets = core.env().platform == crate::Platform::Android && options.git.is_some();
    collect(&directory, "", &mut files, &mut total_size, skip_trime_assets)?;
    let mut schemas: Vec<_> = files
        .keys()
        .filter(|name| !name.contains('/'))
        .filter_map(|name| name.strip_suffix(".schema.yaml").map(str::to_owned))
        .collect();
    if schemas.is_empty() {
        return Err(CoreError::Other(
            "目录根部未找到 .schema.yaml 方案文件，请导入纯 Rime 文件夹或 Rime 方案 .zip 压缩包"
                .into(),
        ));
    }
    files.retain(|name, _| {
        name.strip_suffix(".custom.yaml").is_none_or(|schema| {
            !name.contains('/')
                && !matches!(schema, "squirrel" | "weasel")
                && schemas.iter().any(|id| id == schema)
        })
    });
    let defaults = ["default.yaml", "default-custom.yaml", "default.custom.yaml"].into_iter()
        .filter_map(|name| match fs::read(directory.join(name)) {
            Ok(bytes) => Some(Ok((name.to_owned(), bytes))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => Some(Err(context("读取方案配置", &directory.join(name), error))),
        }).collect::<Result<Vec<_>, _>>()?;
    let schema_order: Vec<_> = crate::package_schemas::default_schemas(&defaults)?
        .into_iter()
        .filter(|id| schemas.contains(id))
        .collect();
    if !schema_order.is_empty() {
        schemas.retain(|schema| schema_order.contains(schema));
    } else {
        let schema_files = schemas
            .iter()
            .filter_map(|id| {
                let path = format!("{id}.schema.yaml");
                match files.get(&path) {
                    Some(ImportFile::Bytes(bytes)) => Some((path, bytes.clone())),
                    _ => None,
                }
            })
            .collect::<Vec<_>>();
        let candidates = crate::package_schemas::enumerate("local-import", &schema_files, None)?;
        schemas.retain(|id| candidates.iter().any(|schema| schema.selected && &schema.id == id));
    }
    schemas.sort_by_key(|schema| {
        schema_order
            .iter()
            .position(|id| id == schema)
            .unwrap_or(usize::MAX)
    });

    // If re-syncing a previously split Git scheme, narrow to its specific schema partition.
    if let Some(target_schema) = options
        .git
        .as_ref()
        .and_then(|g| g.schema_id.as_deref())
        .filter(|id| schemas.iter().any(|s| s == id))
    {
        let partitions = compute_schema_partitions(&directory, &files, &schemas);
        let filtered = filter_files_for_schema(&files, target_schema, &partitions);
        return write_single_imported_package(
            core,
            &directory,
            filtered,
            &defaults,
            &[target_schema.to_owned()],
            display_name,
            scheme_key,
            &options,
            options.slug_hint,
        );
    }

    // Multi-schema split import: when adding a new local/Git package with multiple
    // primary schemas, split schemas & dictionaries per scheme while sharing lua/common files,
    // and validate size independently for each split scheme.
    if scheme_key.is_none() && schemas.len() > 1 {
        let partitions = compute_schema_partitions(&directory, &files, &schemas);
        let mut first_key: Option<String> = None;
        for schema_id in &schemas {
            let split_files = filter_files_for_schema(&files, schema_id, &partitions);
            let split_name = infer_schema_display_name(&split_files, std::slice::from_ref(schema_id))
                .unwrap_or_else(|| schema_id.clone());
            let mut split_options = LocalImportOptions {
                slug_hint: Some(schema_id.as_str()),
                version_override: options.version_override,
                origin: options.origin,
                git: options.git.clone().map(|mut git| {
                    git.schema_id = Some(schema_id.clone());
                    git
                }),
            };
            // Preserve custom display name prefix only if user explicitly named a single scheme;
            // for split multi-schema imports, each package takes its schema's own display name.
            let _ = &mut split_options;
            let imported_key = write_single_imported_package(
                core,
                &directory,
                split_files,
                &defaults,
                std::slice::from_ref(schema_id),
                &split_name,
                None,
                &split_options,
                Some(schema_id.as_str()),
            )?;
            if first_key.is_none() {
                first_key = Some(imported_key);
            }
        }
        return first_key.ok_or_else(|| {
            CoreError::Other("目录根部未找到可导入的 .schema.yaml 方案文件".into())
        });
    }

    write_single_imported_package(
        core,
        &directory,
        files,
        &defaults,
        &schemas,
        display_name,
        scheme_key,
        &options,
        options.slug_hint,
    )
}

#[allow(clippy::too_many_arguments)]
fn write_single_imported_package(
    core: &Core,
    directory: &Path,
    mut files: BTreeMap<String, ImportFile>,
    defaults: &[(String, Vec<u8>)],
    schemas: &[String],
    display_name: &str,
    scheme_key: Option<&str>,
    options: &LocalImportOptions<'_>,
    slug_hint: Option<&str>,
) -> Result<String, CoreError> {
    let inferred_name = if options.git.is_some() {
        infer_schema_display_name(&files, schemas)
    } else {
        None
    };
    let config = build_default_custom(defaults, schemas)?;
    if files.len() >= MAX_LOCAL_IMPORT_FILES {
        return Err(too_many_files_local_error());
    }
    let package_bytes = config.len() as u64
        + files.values().map(ImportFile::byte_len).sum::<u64>();
    if package_bytes > DEFAULT_MAX_EXPANDED_BYTES {
        return Err(oversized_local_error("default.custom.yaml", package_bytes));
    }
    files.insert("default.custom.yaml".into(), ImportFile::Bytes(config));
    let store = PackageStore::new(core);
    let existing_meta = scheme_key.and_then(|key| store.read_meta(key).ok().flatten());
    let _guard = core
        .pending_package
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut temporary = store.staging_file(Staging::ImportArchive)?;
    let archive = temporary.as_file_mut();
    let manifest = build_archive(directory, files, archive)?;
    archive.rewind()?;
    let digest = sha256_reader(&mut *archive)?;
    archive.rewind()?;
    let version = match options.version_override.map(str::trim).filter(|s| !s.is_empty()) {
        Some(tag) if matches!(tag, "latest" | "nightly") => format!("{tag}-{}", &digest[..8]),
        Some(tag) => tag.to_owned(),
        None => digest[..12].to_owned(),
    };
    let state = core.state();
    let scheme_key = if let Some(key) = scheme_key {
        if !crate::scheme::is_local_scheme_key(key) || !state.installed_packages.contains_key(key) {
            return Err(CoreError::Other("本地方案不存在".into()));
        }
        key.to_owned()
    } else {
        let base_slug = slug_hint
            .map(slug_from_str)
            .unwrap_or_else(|| slug(directory));
        let base = format!("local-{base_slug}");
        let mut key = base.clone();
        let mut suffix = 2u64;
        while state.installed_packages.contains_key(&key)
            || store.package_path(&key).exists()
            || store.metadata_path(&key).exists()
        {
            key = format!("{base}-{suffix}");
            suffix += 1;
        }
        key
    };
    let trimmed_name = display_name.trim();
    let resolved_name = if !trimmed_name.is_empty() {
        trimmed_name.to_owned()
    } else if let Some(existing_name) = existing_meta
        .as_ref()
        .and_then(|meta| meta.display_name.clone())
        .filter(|name| !name.trim().is_empty())
    {
        existing_name
    } else if let Some(inferred) = inferred_name {
        inferred
    } else if let Some(hint) = slug_hint.filter(|s| !s.trim().is_empty()) {
        hint.to_owned()
    } else {
        directory
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&scheme_key)
            .to_owned()
    };
    let metadata = PackageMeta {
        scheme_key: scheme_key.clone(),
        engine: Default::default(),
        version: Some(version.clone()),
        installed_at_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        source: "local".into(),
        display_name: Some(resolved_name),
        selection_base_version: None,
        git: options
            .git
            .clone()
            .or_else(|| existing_meta.and_then(|meta| meta.git)),
        manifest,
        zip_sha256: digest,
    };
    let default_origin = directory.to_string_lossy().into_owned();
    store.write_archive(
        archive,
        &metadata,
        Some(options.origin.unwrap_or(&default_origin)),
    )?;
    core.update_state(|state| {
        state.installed_packages.insert(
            scheme_key.clone(),
            crate::state::InstalledPackage {
                version: Some(version),
            },
        );
        state.scheme_cloud_ids.remove(&scheme_key);
        if state.active_scheme.as_deref() == Some(&scheme_key) {
            state.last_deploy = None;
        }
    })?;
    Ok(scheme_key)
}

#[cfg(test)]
#[path = "local_import_tests.rs"]
mod tests;
