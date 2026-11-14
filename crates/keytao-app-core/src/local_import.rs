use super::{PackageMeta, checked_store_dir, excluded_from_manifest, safe_path, validate_relative};
use crate::{Core, CoreError};
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
    matches!(top, "build" | "sync" | "installation.yaml" | "user.yaml")
        || relative.split('/').any(|part| {
            matches!(part, ".git" | ".DS_Store")
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

fn collect(
    directory: &Path,
    prefix: &str,
    files: &mut BTreeMap<String, Vec<u8>>,
    size: &mut u64,
) -> Result<(), CoreError> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| CoreError::Other("方案包文件名无效".into()))?;
        let relative = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if excluded(&relative) {
            continue;
        }
        validate_relative(&relative).map_err(CoreError::Other)?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(CoreError::Other(format!(
                "方案包不能包含符号链接：{relative}"
            )));
        }
        if kind.is_dir() {
            collect(&entry.path(), &relative, files, size)?;
        } else if kind.is_file() {
            if files.len() >= 10_000 {
                return Err(CoreError::Other("方案包文件过多".into()));
            }
            *size = size.saturating_add(entry.metadata()?.len());
            if *size > 512 * 1024 * 1024 {
                return Err(CoreError::Other("方案包解压后过大".into()));
            }
            files.insert(relative, fs::read(entry.path())?);
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
        .map_err(|error| CoreError::Other(format!("无法读取方案目录：{error}")))?;
    if !directory.is_dir() {
        return Err(CoreError::Other("请选择方案目录".into()));
    }
    let mut files = BTreeMap::new();
    collect(&directory, "", &mut files, &mut 0)?;
    let schemas: Vec<_> = files
        .keys()
        .filter(|name| !name.contains('/'))
        .filter_map(|name| name.strip_suffix(".schema.yaml"))
        .collect();
    if schemas.is_empty() {
        return Err(CoreError::Other(
            "目录根部未找到 .schema.yaml 方案文件".into(),
        ));
    }
    let mut config = String::from("patch:\n  schema_list:\n");
    for schema in schemas {
        config.push_str(&format!(
            "    - schema: {}\n",
            serde_json::to_string(schema)?
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
    let store = checked_store_dir(core)?;
    let _guard = core
        .pending_package
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    fs::create_dir_all(&store)?;
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
            || store.join(format!("{key}.zip")).exists()
            || store.join(format!("{key}.json")).exists()
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
    let metadata = PackageMeta {
        scheme_key: scheme_key.clone(),
        version: Some(version.clone()),
        installed_at_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        source: "local".into(),
        display_name: Some(name.into()),
        manifest,
        zip_sha256: digest,
    };
    let mut archive = tempfile::Builder::new()
        .prefix(".local-")
        .tempfile_in(&store)?;
    archive.write_all(&bytes)?;
    archive.as_file().sync_all()?;
    let mut meta = tempfile::Builder::new()
        .prefix(".meta-")
        .tempfile_in(&store)?;
    serde_json::to_writer_pretty(meta.as_file_mut(), &metadata)?;
    meta.as_file().sync_all()?;
    archive
        .persist(safe_path(&store, &format!("{scheme_key}.zip")).map_err(CoreError::Other)?)
        .map_err(|error| CoreError::from(error.error))?;
    meta.persist(safe_path(&store, &format!("{scheme_key}.json")).map_err(CoreError::Other)?)
        .map_err(|error| CoreError::from(error.error))?;
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
