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

#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct SchemeUpdateInfo {
    pub scheme_key: String,
    pub installed_version: String,
    pub latest_version: String,
    pub has_update: bool,
}

fn has_scheme_update(installed: &str, latest: &str) -> bool {
    let installed = installed.strip_prefix('v').unwrap_or(installed);
    let latest = latest.strip_prefix('v').unwrap_or(latest);
    !installed.is_empty() && !latest.is_empty() && installed != latest
}

async fn download_scheme_package_with<Fetch, FetchFuture, Download, DownloadFuture>(
    core: &Core,
    scheme_key: &str,
    fetch: Fetch,
    download: Download,
) -> Result<(), CoreError>
where
    Fetch: FnOnce() -> FetchFuture,
    FetchFuture: std::future::Future<Output = Result<(String, String), CoreError>>,
    Download: FnOnce(String) -> DownloadFuture,
    DownloadFuture: std::future::Future<Output = Result<String, CoreError>>,
{
    use std::io::Read;
    let scheme_key = crate::scheme::validate_scheme_key(scheme_key).map_err(CoreError::Other)?;
    if core.state().active_scheme.as_deref() == Some(scheme_key) {
        return Err(CoreError::Other(
            "Cannot download the active scheme package; update it through install and deploy"
                .into(),
        ));
    }
    let (version, url) = fetch().await?;
    let archive_path = download(url).await?;
    let result = (|| {
        let bytes = fs::read(&archive_path)?;
        let paths = archive_paths(&bytes, &checked_store_dir(core)?).map_err(CoreError::Other)?;
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes))
            .map_err(|error| CoreError::Other(error.to_string()))?;
        let mut manifest = BTreeMap::new();
        for relative in paths {
            if excluded_from_manifest(&relative) {
                continue;
            }
            let mut content = Vec::new();
            archive
                .by_name(&relative)
                .map_err(|error| CoreError::Other(error.to_string()))?
                .read_to_end(&mut content)?;
            manifest.insert(relative, format!("{:x}", Sha256::digest(&content)));
        }
        stage_install(core, &bytes, manifest, String::new())?;
        finalize_pending(core, scheme_key, Some(&version), false)?;
        core.update_state(|state| {
            state.installed_packages.insert(
                scheme_key.into(),
                crate::state::InstalledPackage {
                    version: Some(version),
                },
            );
        })?;
        core.emit(crate::CoreEvent::InstallProgress(
            crate::events::InstallProgress {
                stage: "done".into(),
                percent: 100,
                message: "下载完成！".into(),
            },
        ));
        Ok(())
    })();
    let _ = fs::remove_file(archive_path);
    result
}

async fn latest_package_release(
    core: &Core,
    scheme_key: &str,
    source: &str,
) -> Result<(String, String), CoreError> {
    if scheme_key != "keytao" {
        let release = crate::scheme::fetch_scheme_release(core, scheme_key.into()).await?;
        return Ok((release.version, release.download_url));
    }
    let release = crate::scheme::fetch_latest_release(core).await?;
    let version = release
        .version
        .strip_prefix('v')
        .unwrap_or(&release.version);
    let sources = if source == "github" {
        [release.github.as_ref(), release.gitee.as_ref()]
    } else {
        [release.gitee.as_ref(), release.github.as_ref()]
    };
    let url = sources
        .into_iter()
        .flatten()
        .filter(|source| source.version.strip_prefix('v').unwrap_or(&source.version) == version)
        .find_map(|source| {
            let url = match core.env().platform {
                crate::Platform::Windows => source.download_urls.windows.clone(),
                crate::Platform::MacOs => source.download_urls.macos.clone(),
                crate::Platform::Linux => source.download_urls.linux.clone(),
                crate::Platform::Android => source.download_urls.android.clone(),
                crate::Platform::Ios => source.download_urls.ios.clone(),
            };
            url.filter(|url| !url.is_empty())
        })
        .ok_or_else(|| {
            CoreError::Other("The latest scheme has no package for this platform".into())
        })?;
    Ok((release.version, url))
}

/// Downloads into the package store without installing or changing the active scheme.
pub async fn download_scheme_package(
    core: &Core,
    scheme_key: &str,
    source: &str,
) -> Result<(), CoreError> {
    download_scheme_package_with(
        core,
        scheme_key,
        || latest_package_release(core, scheme_key, source),
        |url| crate::download::download_to_temp(core, url),
    )
    .await
}

async fn check_scheme_updates_with<Fetch, FetchFuture>(
    core: &Core,
    mut fetch: Fetch,
) -> Vec<SchemeUpdateInfo>
where
    Fetch: FnMut(String) -> FetchFuture,
    FetchFuture: std::future::Future<Output = Result<String, CoreError>>,
{
    let mut updates = Vec::new();
    for (scheme_key, package) in core.state().installed_packages {
        if crate::scheme::validate_scheme_key(&scheme_key).is_err() {
            continue;
        }
        match fetch(scheme_key.clone()).await {
            Ok(latest_version) => {
                let installed_version = package.version.unwrap_or_default();
                updates.push(SchemeUpdateInfo {
                    has_update: has_scheme_update(&installed_version, &latest_version),
                    scheme_key,
                    installed_version,
                    latest_version,
                });
            }
            Err(error) => tracing::warn!(%scheme_key, %error, "Scheme update check failed"),
        }
    }
    updates
}

pub async fn check_scheme_updates(core: &Core) -> Vec<SchemeUpdateInfo> {
    check_scheme_updates_with(core, |scheme_key| async move {
        if scheme_key == "keytao" {
            Ok(crate::scheme::fetch_latest_release(core).await?.version)
        } else {
            Ok(crate::scheme::fetch_scheme_release(core, scheme_key)
                .await?
                .version)
        }
    })
    .await
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
        || crate::install::is_protected_user_file(relative)
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
        if incoming.contains(relative)
            || excluded_from_manifest(relative)
            || relative.starts_with("lua/")
        {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppEnv, CoreEvent, EventSink, Platform};
    use std::{io::Write, sync::Arc};

    struct NullSink;

    impl EventSink for NullSink {
        fn emit(&self, _: CoreEvent) {}
    }

    fn open_core(path: &Path) -> Arc<Core> {
        Core::new(
            AppEnv {
                data_dir: path.into(),
                cache_dir: path.join("cache"),
                resource_dir: path.join("resources"),
                app_version: "test".into(),
                platform: Platform::Android,
            },
            Arc::new(NullSink),
        )
        .unwrap()
    }

    #[test]
    fn scheme_update_comparison_accepts_tags_dates_equal_and_empty_versions() {
        for (installed, latest, expected) in [
            ("v1.0.0", "1.0.0", false),
            ("1.0.0", "v1.0.0", false),
            ("v1.0.0", "v1.0.1", true),
            ("20261104", "20261105", true),
            ("20261104", "20261104", false),
            ("", "v1", false),
            ("v1", "", false),
            ("v", "v1", false),
            ("", "", false),
        ] {
            assert_eq!(
                has_scheme_update(installed, latest),
                expected,
                "{installed:?} -> {latest:?}"
            );
        }
    }

    #[tokio::test]
    async fn latest_package_release_prefers_selected_source_and_falls_back() {
        for (source, gitee_url, expected) in [
            ("gitee", Some("gitee-android"), "gitee-android"),
            ("gitee", None, "github-android"),
            ("gitee", Some(""), "github-android"),
            ("github", Some("gitee-android"), "github-android"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let core = open_core(dir.path());
            fs::create_dir_all(&core.env().cache_dir).unwrap();
            let cache = serde_json::json!({
                "etag": "",
                "cached_at": std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
                "release": {
                    "version": "v2", "name": "release", "published_at": "", "body": "",
                    "github": { "version": "v2", "download_urls": {
                        "android": "github-android"
                    } },
                    "gitee": { "version": "2", "download_urls": {
                        "android": gitee_url, "macos": "gitee-macos"
                    } }
                }
            });
            fs::write(
                core.env().cache_dir.join("release_cache.json"),
                serde_json::to_vec(&cache).unwrap(),
            )
            .unwrap();

            assert_eq!(
                latest_package_release(&core, "keytao", source)
                    .await
                    .unwrap(),
                ("v2".into(), expected.into()),
                "selected source: {source}, gitee Android asset: {gitee_url:?}"
            );
        }
    }

    #[tokio::test]
    async fn download_scheme_package_refuses_active_key() {
        let dir = tempfile::tempdir().unwrap();
        let core = open_core(dir.path());
        crate::scheme::record_installed_scheme(&core, "keytao", Some("v1")).unwrap();

        let error = download_scheme_package_with(
            &core,
            "keytao",
            || async { panic!("Active package download must not fetch a release") },
            |_| async { panic!("Active package download must not download an archive") },
        )
        .await
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "Cannot download the active scheme package; update it through install and deploy"
        );
        assert_eq!(core.state().active_scheme.as_deref(), Some("keytao"));
        assert_eq!(
            core.state().installed_packages["keytao"].version.as_deref(),
            Some("v1")
        );
    }

    #[tokio::test]
    async fn download_scheme_package_preserves_rime_and_active_scheme() {
        let dir = tempfile::tempdir().unwrap();
        let core = open_core(dir.path());
        let rime = dir.path().join("rime");
        fs::create_dir_all(&rime).unwrap();
        fs::write(rime.join("keytao.schema.yaml"), "active schema").unwrap();
        fs::write(rime.join("version.txt"), "v1").unwrap();
        crate::scheme::record_installed_scheme(&core, "keytao", Some("v1")).unwrap();
        crate::scheme::record_installed_scheme(&core, "xmjd", Some("20261104")).unwrap();
        crate::scheme::record_installed_scheme(&core, "keytao", Some("v1")).unwrap();
        assert!(!list_installed_packages(&core).unwrap()[1].has_package_zip);

        let archive = dir.path().join("local-source.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        zip.start_file(
            "xmjd6.schema.yaml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(b"schema: xmjd6\n").unwrap();
        zip.finish().unwrap();
        download_scheme_package_with(
            &core,
            "xmjd",
            || std::future::ready(Ok(("20261105".into(), "local-source".into()))),
            |_| std::future::ready(Ok(archive.to_string_lossy().into_owned())),
        )
        .await
        .unwrap();

        assert_eq!(
            fs::read_to_string(rime.join("keytao.schema.yaml")).unwrap(),
            "active schema"
        );
        assert_eq!(fs::read_to_string(rime.join("version.txt")).unwrap(), "v1");
        assert_eq!(fs::read_dir(&rime).unwrap().count(), 2);
        assert_eq!(core.state().active_scheme.as_deref(), Some("keytao"));
        let packages = list_installed_packages(&core).unwrap();
        let updated = packages
            .iter()
            .find(|package| package.scheme_key == "xmjd")
            .unwrap();
        assert_eq!(updated.version.as_deref(), Some("20261105"));
        assert!(updated.has_package_zip);
        assert!(!updated.active);
        let reopened = open_core(dir.path());
        assert_eq!(reopened.state().active_scheme.as_deref(), Some("keytao"));
        assert_eq!(
            reopened.state().installed_packages["xmjd"]
                .version
                .as_deref(),
            Some("20261105")
        );
        let meta: PackageMeta =
            serde_json::from_slice(&fs::read(store_dir(&core).join("xmjd.json")).unwrap()).unwrap();
        assert_eq!(meta.version.as_deref(), Some("20261105"));
        assert_eq!(meta.manifest.len(), 1);
        assert!(!store_dir(&core).join("active-manifest.json").exists());
    }

    #[tokio::test]
    async fn scheme_updates_omit_failed_fetch_and_check_only_installed_packages() {
        let dir = tempfile::tempdir().unwrap();
        let core = open_core(dir.path());
        for (scheme_key, version) in [
            ("keytao", "v1.0"),
            ("txjx", "20261104"),
            ("xmjd", "20261104"),
        ] {
            crate::scheme::record_installed_scheme(&core, scheme_key, Some(version)).unwrap();
        }
        let mut fetched = Vec::new();
        let updates = check_scheme_updates_with(&core, |scheme_key| {
            fetched.push(scheme_key.clone());
            std::future::ready(match scheme_key.as_str() {
                "keytao" => Ok("1.0".into()),
                "txjx" => Err(CoreError::Other("Local source failed".into())),
                "xmjd" => Ok("20261105".into()),
                other => panic!("Unexpected source: {other}"),
            })
        })
        .await;
        assert_eq!(
            updates,
            vec![
                SchemeUpdateInfo {
                    scheme_key: "keytao".into(),
                    installed_version: "v1.0".into(),
                    latest_version: "1.0".into(),
                    has_update: false
                },
                SchemeUpdateInfo {
                    scheme_key: "xmjd".into(),
                    installed_version: "20261104".into(),
                    latest_version: "20261105".into(),
                    has_update: true
                },
            ]
        );
        assert_eq!(fetched, ["keytao", "txjx", "xmjd"]);
    }
}
