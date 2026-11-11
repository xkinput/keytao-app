use crate::{Core, CoreError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
pub struct PackageMeta {
    pub scheme_key: String,
    pub version: Option<String>,
    pub installed_at_unix: u64,
    pub source: String,
    pub manifest: BTreeMap<String, String>,
    pub zip_sha256: String,
}

#[derive(Serialize, Deserialize)]
struct ActiveManifest {
    root: String,
    manifest: BTreeMap<String, String>,
}

#[derive(Serialize)]
pub struct InstalledPackageInfo {
    pub scheme_key: String,
    pub version: Option<String>,
    pub active: bool,
    pub has_package_zip: bool,
}

pub(crate) struct PendingPackage {
    archive: tempfile::NamedTempFile,
    manifest: BTreeMap<String, String>,
    root: String,
    zip_sha256: String,
}

fn store_dir(core: &Core) -> PathBuf {
    core.env().data_dir.join("scheme-packages")
}

fn checked_store_dir(core: &Core) -> Result<PathBuf, CoreError> {
    let dir = store_dir(core);
    if fs::symlink_metadata(&dir).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err(CoreError::Other(
            "Package store must not be a symlink".into(),
        ));
    }
    Ok(dir)
}

pub(crate) fn excluded_from_manifest(relative: &str) -> bool {
    matches!(
        relative.rsplit('/').next().unwrap_or(relative),
        "default.custom.yaml" | "default-custom.yaml"
    ) || relative == "rime.lua"
}

pub(crate) fn stage_install(
    core: &Core,
    zip_bytes: &[u8],
    manifest: BTreeMap<String, String>,
    root: String,
) -> Result<(), CoreError> {
    use std::io::Write;
    let dir = checked_store_dir(core)?;
    fs::create_dir_all(&dir)?;
    let mut archive = tempfile::Builder::new()
        .prefix(".pending-")
        .tempfile_in(dir)?;
    archive.write_all(zip_bytes)?;
    archive.as_file().sync_all()?;
    *core
        .pending_package
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(PendingPackage {
        archive,
        manifest,
        root,
        zip_sha256: format!("{:x}", Sha256::digest(zip_bytes)),
    });
    Ok(())
}

pub(crate) fn finalize_pending(
    core: &Core,
    scheme_key: &str,
    version: Option<&str>,
    active: bool,
) -> Result<(), CoreError> {
    let mut pending = core
        .pending_package
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some(receipt) = pending.as_ref() else {
        return Ok(());
    };
    let dir = checked_store_dir(core)?;
    let meta = PackageMeta {
        scheme_key: scheme_key.into(),
        version: version.map(str::to_owned),
        installed_at_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        source: "official".into(),
        manifest: receipt.manifest.clone(),
        zip_sha256: receipt.zip_sha256.clone(),
    };
    let mut temporary = tempfile::Builder::new()
        .prefix(".meta-")
        .tempfile_in(&dir)?;
    serde_json::to_writer_pretty(temporary.as_file_mut(), &meta)?;
    temporary.as_file().sync_all()?;
    // Retain the staged receipt on failure so callers can retry finalization.
    let archive_path = dir.join(format!("{scheme_key}.zip"));
    let mut archive = tempfile::Builder::new()
        .prefix(".archive-")
        .tempfile_in(&dir)?;
    std::io::copy(&mut receipt.archive.reopen()?, archive.as_file_mut())?;
    archive.as_file().sync_all()?;
    archive
        .persist(archive_path)
        .map_err(|error| CoreError::from(error.error))?;
    temporary
        .persist(dir.join(format!("{scheme_key}.json")))
        .map_err(|error| CoreError::from(error.error))?;
    if active {
        let mut temporary = tempfile::Builder::new()
            .prefix(".active-")
            .tempfile_in(&dir)?;
        serde_json::to_writer_pretty(
            temporary.as_file_mut(),
            &ActiveManifest {
                root: receipt.root.clone(),
                manifest: receipt.manifest.clone(),
            },
        )?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(dir.join("active-manifest.json"))
            .map_err(|error| CoreError::from(error.error))?;
    }
    *pending = None;
    Ok(())
}

pub fn record_package(
    core: &Core,
    scheme_key: &str,
    version: Option<&str>,
    active: bool,
) -> Result<(), CoreError> {
    let scheme_key =
        crate::scheme::validate_scheme_key(scheme_key.trim()).map_err(CoreError::Other)?;
    if !active {
        *core
            .pending_package
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        return Ok(());
    }
    finalize_pending(core, scheme_key, version, active)?;
    core.update_state(|state| {
        state.installed_packages.insert(
            scheme_key.into(),
            crate::state::InstalledPackage {
                version: version.map(str::to_owned),
            },
        );
        if active {
            state.active_scheme = Some(scheme_key.into());
        }
    })
}

pub fn list_installed_packages(core: &Core) -> Result<Vec<InstalledPackageInfo>, CoreError> {
    let dir = checked_store_dir(core)?;
    let state = core.state();
    Ok(state
        .installed_packages
        .into_iter()
        .map(|(scheme_key, package)| {
            let has_package_zip = crate::scheme::validate_scheme_key(&scheme_key).is_ok()
                && fs::symlink_metadata(dir.join(format!("{scheme_key}.zip")))
                    .is_ok_and(|meta| meta.is_file());
            InstalledPackageInfo {
                active: state.active_scheme.as_deref() == Some(&scheme_key),
                scheme_key,
                version: package.version,
                has_package_zip,
            }
        })
        .collect())
}

pub fn remove_scheme_package(core: &Core, scheme_key: &str) -> Result<(), CoreError> {
    let scheme_key = crate::scheme::validate_scheme_key(scheme_key).map_err(CoreError::Other)?;
    if core.state().active_scheme.as_deref() == Some(scheme_key) {
        return Err(CoreError::Other(
            "Cannot remove the active scheme package; activate another scheme first".into(),
        ));
    }
    let dir = checked_store_dir(core)?;
    for extension in ["zip", "json"] {
        match fs::remove_file(dir.join(format!("{scheme_key}.{extension}"))) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    core.update_state(|state| {
        state.installed_packages.remove(scheme_key);
    })
}

pub(crate) fn validate_relative(relative: &str) -> Result<(), String> {
    if relative.is_empty()
        || relative.contains(['\\', ':', '\0'])
        || relative.split('/').any(|part| {
            part.is_empty()
                || part == "."
                || part == ".."
                || part.ends_with(['.', ' '])
                || part.chars().any(char::is_control)
        })
        || Path::new(relative).is_absolute()
    {
        return Err(format!("Unsafe package path: {relative}"));
    }
    Ok(())
}

pub(crate) fn safe_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    validate_relative(relative)?;
    let mut path = root.to_owned();
    for part in relative.split('/') {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!("Package path contains a symlink: {relative}"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(path)
}

pub async fn activate_scheme(
    core: &Core,
    scheme_key: &str,
    root: &Path,
    #[cfg(target_os = "android")] android_extract: impl FnOnce(
        String,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>,
    >,
    #[cfg(target_os = "windows")] finish_windows: crate::install::WindowsInstallFinish,
) -> Result<crate::InstallResult, crate::CoreError> {
    let scheme_key = crate::scheme::validate_scheme_key(scheme_key).map_err(CoreError::Other)?;
    let state = core.state();
    let package = state.installed_packages.get(scheme_key).ok_or_else(|| {
        CoreError::Other("Scheme package is not installed; re-download it first".into())
    })?;
    let archive = checked_store_dir(core)?.join(format!("{scheme_key}.zip"));
    if !fs::symlink_metadata(&archive).is_ok_and(|meta| meta.is_file()) {
        return Err(CoreError::Other(
            "Stored package zip is missing; re-download this scheme before activation".into(),
        ));
    }
    let meta: PackageMeta = serde_json::from_slice(&fs::read(archive.with_extension("json"))?)?;
    if meta.scheme_key != scheme_key
        || meta.version != package.version
        || meta.zip_sha256 != format!("{:x}", Sha256::digest(fs::read(&archive)?))
    {
        return Err(CoreError::Other(
            "Stored package metadata is inconsistent; re-download this scheme before activation"
                .into(),
        ));
    }
    #[cfg(not(target_os = "android"))]
    let result = crate::install::smart_install(
        core,
        archive.to_string_lossy().into_owned(),
        root.to_string_lossy().into_owned(),
        #[cfg(target_os = "windows")]
        finish_windows,
    )
    .await?;
    #[cfg(target_os = "android")]
    let result = {
        let bytes = fs::read(&archive)?;
        let logs = cleanup_previous(core, root, &bytes).map_err(CoreError::Other)?;
        let value = android_extract(archive.to_string_lossy().into_owned())
            .await
            .map_err(CoreError::Other)?;
        let mut result = crate::install::finish_android_install(core, &value)?;
        result.logs.splice(0..0, logs);
        result
    };
    crate::scheme::record_installed_scheme(core, scheme_key, package.version.as_deref())?;
    Ok(result)
}

pub(crate) fn archive_paths(zip_bytes: &[u8], root: &Path) -> Result<BTreeSet<String>, String> {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).map_err(|e| e.to_string())?;
    let mut paths = BTreeSet::new();
    for index in 0..archive.len() {
        let file = archive.by_index(index).map_err(|e| e.to_string())?;
        let relative = file.name().trim_end_matches('/');
        safe_path(root, relative)?;
        if file
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(format!("Package contains a symlink: {relative}"));
        }
        if !file.is_dir() {
            paths.insert(relative.into());
        }
    }
    Ok(paths)
}

pub(crate) fn cleanup_previous(
    core: &Core,
    root: &Path,
    zip_bytes: &[u8],
) -> Result<Vec<String>, String> {
    let incoming = archive_paths(zip_bytes, root)?;
    let Some(active) = core.state().active_scheme else {
        return Ok(Vec::new());
    };
    crate::scheme::validate_scheme_key(&active)?;
    let path = checked_store_dir(core)
        .map_err(|e| e.to_string())?
        .join("active-manifest.json");
    let meta: ActiveManifest = match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string())?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    for relative in meta.manifest.keys() {
        validate_relative(relative)?;
    }
    let actual_root = match root.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    if Path::new(&meta.root) != actual_root {
        return Ok(Vec::new());
    }
    cleanup_manifest(root, &meta.manifest, &incoming)
}

pub(crate) fn cleanup_manifest(
    root: &Path,
    manifest: &BTreeMap<String, String>,
    incoming: &BTreeSet<String>,
) -> Result<Vec<String>, String> {
    let mut logs = Vec::new();
    for relative in manifest.keys() {
        safe_path(root, relative)?;
    }
    for (relative, expected) in manifest {
        if incoming.contains(relative) || excluded_from_manifest(relative) {
            continue;
        }
        let path = safe_path(root, relative)?;
        match fs::read(&path) {
            Ok(bytes) if format!("{:x}", Sha256::digest(&bytes)) == *expected => {
                fs::remove_file(&path).map_err(|error| error.to_string())?;
                logs.push(format!("[REMOVED] {relative}"));
            }
            Ok(_) => logs.push(format!("[KEPT] User-modified file: {relative}")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(logs)
}
