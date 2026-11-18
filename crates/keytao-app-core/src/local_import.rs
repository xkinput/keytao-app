use super::{PackageMeta, excluded_from_manifest, validate_relative};
use crate::package_store::PackageStore;
use crate::{Core, CoreError, package_fs::context};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Cursor, Write},
    path::Path,
};

fn ships_with_scheme(name: &str) -> bool {
    name.ends_with(".user.dict.yaml") || name.ends_with("_user.txt") || name == "user_dict.txt"
}

fn excluded(relative: &str) -> bool {
    let top = relative.split('/').next().unwrap_or(relative);
    matches!(
        top,
        "build" | "sync" | "log" | "installation.yaml" | "user.yaml"
    )
        || relative.split('/').any(|part| {
            part.starts_with('.')
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
fn excluded_attributes(attributes: u32) -> bool {
    // Windows hidden/system flags; readonly scheme files are still importable.
    attributes & (0x2 | 0x4) != 0
}

fn import_io<T>(
    result: std::io::Result<T>,
    operation: &str,
    path: &Path,
) -> Result<T, CoreError> {
    result.map_err(|error| context(operation, path, error).into())
}

fn has_root_schema(directory: &Path) -> Result<bool, CoreError> {
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
            if excluded_attributes(metadata.file_attributes()) {
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
    files: &mut BTreeMap<String, Vec<u8>>,
    size: &mut u64,
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
        let metadata = import_io(entry.metadata(), "读取文件属性", &path)?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if excluded_attributes(metadata.file_attributes()) {
                tracing::warn!(path = %path.display(), "Skipping hidden or system import entry");
                continue;
            }
        }
        let kind = metadata.file_type();
        if kind.is_symlink() {
            return Err(CoreError::Other(format!(
                "方案包不能包含符号链接：{}",
                path.display()
            )));
        }
        if kind.is_dir() {
            collect(&path, &relative, files, size)?;
        } else if kind.is_file() {
            if files.len() >= 10_000 {
                return Err(CoreError::Other("方案包文件过多".into()));
            }
            let next_size = size.saturating_add(metadata.len());
            if next_size > 512 * 1024 * 1024 {
                return Err(CoreError::Other("方案包解压后过大".into()));
            }
            let mut bytes = import_io(fs::read(&path), "读取文件", &path)?;
            *size = next_size;
            if relative.ends_with(".custom.yaml") {
                let content = std::str::from_utf8(&bytes)
                    .map_err(|error| CoreError::Other(error.to_string()))?;
                bytes = crate::managed_custom::strip(content)
                    .map_err(CoreError::Other)?
                    .into_bytes();
                if bytes.is_empty() {
                    continue;
                }
            } else if crate::managed_custom::is_generated(&relative, &bytes) {
                continue;
            }
            files.insert(relative, bytes);
        }
    }
    Ok(())
}

fn slug(directory: &Path) -> String {
    let name = directory
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
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

pub fn import_local_scheme(
    core: &Core,
    directory: &Path,
    display_name: &str,
    scheme_key: Option<&str>,
) -> Result<String, CoreError> {
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
    collect(&directory, "", &mut files, &mut 0)?;
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
    let schema_order = ["default.custom.yaml", "default-custom.yaml"]
        .into_iter()
        .find_map(|name| fs::read_to_string(directory.join(name)).ok())
        .map(|content| crate::scheme_files::parse_schema_list(&content))
        .unwrap_or_default();
    schemas.sort_by_key(|schema| {
        schema_order
            .iter()
            .position(|id| id == schema)
            .unwrap_or(usize::MAX)
    });
    let mut config = String::from("patch:\n  schema_list:\n");
    for schema in schemas {
        config.push_str(&format!(
            "    - schema: {}\n",
            serde_json::to_string(&schema)?
        ));
    }
    files.insert("default.custom.yaml".into(), config.into_bytes());
    let manifest = files
        .iter()
        .filter(|(name, _)| !excluded_from_manifest(name))
        .map(|(name, bytes)| (name.clone(), format!("{:x}", Sha256::digest(bytes))))
        .collect();
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in files {
        archive
            .start_file(
                name,
                zip::write::SimpleFileOptions::default()
                    .last_modified_time(zip::DateTime::default())
                    .unix_permissions(0o644),
            )
            .map_err(|error| CoreError::Other(error.to_string()))?;
        archive.write_all(&bytes)?;
    }
    let bytes = archive
        .finish()
        .map_err(|error| CoreError::Other(error.to_string()))?
        .into_inner();
    let digest = format!("{:x}", Sha256::digest(&bytes));
    let version = digest[..12].to_owned();
    let store = PackageStore::new(core);
    let store_root = store.checked_root()?;
    let _guard = core
        .pending_package
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    fs::create_dir_all(&store_root).map_err(|error| context("创建方案存储目录", &store_root, error))?;
    let state = core.state();
    let scheme_key = if let Some(key) = scheme_key {
        if !crate::scheme::is_local_scheme_key(key) || !state.installed_packages.contains_key(key) {
            return Err(CoreError::Other("本地方案不存在".into()));
        }
        key.to_owned()
    } else {
        let base = format!("local-{}", slug(&directory));
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
    let name = display_name.trim();
    let name = if name.is_empty() {
        directory
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&scheme_key)
    } else {
        name
    };
    // Keep the active version until activation, including repeated imports and retries.
    let selection_base_version = if state.active_scheme.as_deref() == Some(&scheme_key) {
        fs::read(store.checked_metadata_path(&scheme_key)?)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<PackageMeta>(&bytes).ok())
            .and_then(|meta| meta.selection_base_version)
            .or_else(|| {
                state
                    .installed_packages
                    .get(&scheme_key)
                    .and_then(|package| package.version.clone())
            })
    } else {
        None
    };
    let metadata = PackageMeta {
        scheme_key: scheme_key.clone(),
        version: Some(version.clone()),
        installed_at_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        source: "local".into(),
        display_name: Some(name.into()),
        selection_base_version,
        manifest,
        zip_sha256: digest,
    };
    store.write_archive(
        Cursor::new(bytes),
        &metadata,
        Some(directory.to_string_lossy().as_ref()),
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
