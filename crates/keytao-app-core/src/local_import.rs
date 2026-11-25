use super::{PackageMeta, excluded_from_manifest, safe_path, sha256_reader, validate_relative};
use crate::generic_rime::{DEFAULT_MAX_EXPANDED_BYTES, NOMEDIA_FILE, has_excluded_hidden_component};
use crate::package_store::{PackageStore, Staging};
use crate::{Core, CoreError, package_fs::context};
use keytao_core::runtime_log::AppOperation;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Read, Seek, Take, Write},
    path::Path,
};

fn ships_with_scheme(name: &str) -> bool {
    name.ends_with(".user.dict.yaml") || name.ends_with("_user.txt") || name == "user_dict.txt"
}

fn excluded(relative: &str) -> bool {
    let top = relative.split('/').next().unwrap_or(relative);
    matches!(
        top,
        "build" | "sync" | "log" | "installation.yaml" | "user.yaml" | "__MACOSX"
    )
        || has_excluded_hidden_component(relative)
        || relative.split('/').any(|part| {
            part == "__MACOSX"
                || part.eq_ignore_ascii_case("System Volume Information")
                || part.eq_ignore_ascii_case("$RECYCLE.BIN")
                || part.eq_ignore_ascii_case("app-state.json")
                || part.ends_with(".userdb")
                || part == "default.custom.yaml"
                || part == "default-custom.yaml"
                // Schemes import their user dictionaries, so ship them; install keeps an
                // existing copy in the Rime root and only fills in a missing one.
                || (part != "user.yaml"
                    && !ships_with_scheme(part)
                    && crate::install::is_protected_user_file(part))
        })
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

enum ImportFile {
    Disk(u64),
    Bytes(Vec<u8>),
}

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
            if files.len() >= 10_000 {
                return Err(CoreError::Other("方案包文件过多".into()));
            }
            let next_size = size.saturating_add(metadata.len());
            if next_size > DEFAULT_MAX_EXPANDED_BYTES {
                return Err(CoreError::Other("方案包解压后过大".into()));
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

fn collect_orphan_prebuilt_bins(
    directory: &Path,
    files: &mut BTreeMap<String, ImportFile>,
    size: &mut u64,
) -> Result<(), CoreError> {
    let build_dir = directory.join("build");
    let metadata = match fs::symlink_metadata(&build_dir) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(context("读取文件属性", &build_dir, error).into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if excluded_attributes(metadata.file_attributes(), false) {
            return Ok(());
        }
    }
    let existing: std::collections::BTreeSet<String> = files.keys().cloned().collect();
    for entry in import_io(fs::read_dir(&build_dir), "读取目录", &build_dir)? {
        let entry = import_io(entry, "读取目录项", &build_dir)?;
        let path = entry.path();
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name.starts_with('.') || !name.ends_with(".bin") {
            continue;
        }
        let relative = format!("build/{name}");
        if validate_relative(&relative).is_err()
            || !crate::generic_rime::is_orphan_prebuilt_bin(
                &relative,
                |candidate| existing.contains(candidate),
                None::<&str>,
            )
        {
            continue;
        }
        let entry_meta = import_io(fs::symlink_metadata(&path), "读取文件属性", &path)?;
        if entry_meta.file_type().is_symlink() || !entry_meta.is_file() {
            continue;
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if excluded_attributes(entry_meta.file_attributes(), false) {
                continue;
            }
        }
        if files.len() >= 10_000 {
            return Err(CoreError::Other("方案包文件过多".into()));
        }
        let next_size = size.saturating_add(entry_meta.len());
        if next_size > DEFAULT_MAX_EXPANDED_BYTES {
            return Err(CoreError::Other("方案包解压后过大".into()));
        }
        *size = next_size;
        files.insert(relative, ImportFile::Disk(entry_meta.len()));
    }
    Ok(())
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
    directory: &Path,
    display_name: &str,
    scheme_key: Option<&str>,
) -> Result<String, CoreError> {
    import_local_scheme_with(
        core,
        directory,
        display_name,
        scheme_key,
        LocalImportOptions::default(),
    )
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
        return Err(CoreError::Other("请选择方案目录".into()));
    }
    if !has_root_schema(&directory)? {
        return Err(CoreError::Other(
            "目录根部未找到 .schema.yaml 方案文件".into(),
        ));
    }
    let mut files = BTreeMap::new();
    let mut total_size = 0;
    let skip_trime_assets = core.env().platform == crate::Platform::Android && options.git.is_some();
    collect(&directory, "", &mut files, &mut total_size, skip_trime_assets)?;
    collect_orphan_prebuilt_bins(&directory, &mut files, &mut total_size)?;
    let mut schemas: Vec<_> = files
        .keys()
        .filter(|name| !name.contains('/'))
        .filter_map(|name| name.strip_suffix(".schema.yaml").map(str::to_owned))
        .collect();
    if schemas.is_empty() {
        return Err(CoreError::Other(
            "目录根部未找到 .schema.yaml 方案文件".into(),
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
    let inferred_name = if options.git.is_some() {
        infer_schema_display_name(&files, &schemas)
    } else {
        None
    };
    let config = build_default_custom(&defaults, &schemas)?;
    files.insert("default.custom.yaml".into(), ImportFile::Bytes(config));
    let store = PackageStore::new(core);
    let existing_meta = scheme_key.and_then(|key| store.read_meta(key).ok().flatten());
    let _guard = core
        .pending_package
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut temporary = store.staging_file(Staging::ImportArchive)?;
    let archive = temporary.as_file_mut();
    let manifest = build_archive(&directory, files, archive)?;
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
        let base_slug = options
            .slug_hint
            .map(slug_from_str)
            .unwrap_or_else(|| slug(&directory));
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
    } else if let Some(hint) = options.slug_hint.filter(|s| !s.trim().is_empty()) {
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
        git: options.git.or_else(|| existing_meta.and_then(|meta| meta.git)),
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
