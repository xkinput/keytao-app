use crate::package_store::PackageStore;
use crate::{Core, CoreError};
use keytao_core::engine_choice::{PACKAGE_KEY, PackageEngine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufReader, Read, Seek},
    path::{Path, PathBuf},
};

#[path = "local_import.rs"]
mod local_import;
pub use local_import::import_local_scheme;
#[path = "package_logos.rs"]
pub(crate) mod package_logos;

#[derive(Serialize, Deserialize)]
pub struct PackageMeta {
    pub scheme_key: String,
    #[serde(default, skip_serializing_if = "PackageEngine::is_rime")]
    pub engine: PackageEngine,
    pub version: Option<String>,
    pub installed_at_unix: u64,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_base_version: Option<String>,
    pub manifest: BTreeMap<String, String>,
    pub zip_sha256: String,
}

#[derive(Serialize)]
pub struct InstalledPackageInfo {
    pub scheme_key: String,
    pub engine: String,
    pub version: Option<String>,
    pub active: bool,
    pub in_use: bool,
    pub has_package_zip: bool,
    pub supports_user_words: bool,
    pub source: String,
    pub display_name: Option<String>,
    pub logo_path: Option<String>,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct SchemeUpdateInfo {
    pub scheme_key: String,
    pub installed_version: String,
    pub latest_version: String,
    pub has_update: bool,
}

pub(super) fn has_scheme_update(installed: &str, latest: &str) -> bool {
    let installed = installed.strip_prefix('v').unwrap_or(installed);
    let latest = latest.strip_prefix('v').unwrap_or(latest);
    !installed.is_empty() && !latest.is_empty() && installed != latest
}

async fn download_scheme_package_with<Fetch, FetchFuture, Download, DownloadFuture>(
    core: &Core,
    scheme_key: &str,
    user_root: &Path,
    fetch: Fetch,
    download: Download,
) -> Result<(), CoreError>
where
    Fetch: FnOnce() -> FetchFuture,
    FetchFuture: std::future::Future<Output = Result<(String, String), CoreError>>,
    Download: FnOnce(String) -> DownloadFuture,
    DownloadFuture: std::future::Future<Output = Result<String, CoreError>>,
{
    let scheme_key = crate::scheme::validate_scheme_key(scheme_key).map_err(CoreError::Other)?;
    let (version, url) = fetch().await?;
    let archive_path = download(url.clone()).await?;
    let result = (|| {
        store_cloud_archive_reader(
            core,
            scheme_key,
            Some(&version),
            "official",
            None,
            BufReader::new(fs::File::open(&archive_path)?),
            user_root,
            Some(&url),
        )?;
        core.emit(crate::CoreEvent::InstallProgress(
            crate::events::InstallProgress {
                stage: "done".into(),
                percent: 100,
                message: "下载完成！".into(),
            },
        ));
        Ok(())
    })();
    let _ = crate::package_fs::remove_file(Path::new(&archive_path));
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
    user_root: &Path,
) -> Result<(), CoreError> {
    crate::package_migration::ensure_before_write(core, user_root).await?;
    download_scheme_package_with(
        core,
        scheme_key,
        user_root,
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
        if crate::scheme::validate_scheme_key(&scheme_key).is_err()
            || crate::scheme::is_local_scheme_key(&scheme_key)
        {
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

pub(crate) fn sha256_reader(mut reader: impl Read) -> std::io::Result<String> {
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(test)]
pub(crate) fn stored_archive(
    core: &Core,
    scheme_key: &str,
) -> Result<(Vec<u8>, PackageMeta), CoreError> {
    let (files, meta) = PackageStore::new(core).read_files(scheme_key)?;
    Ok((archive_from_files(&files)?, meta))
}

pub(crate) fn archive_from_files(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, CoreError> {
    use std::io::Write;
    let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (path, bytes) in files {
        archive
            .start_file(path, zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored))
            .map_err(|error| CoreError::Other(error.to_string()))?;
        archive.write_all(bytes)?;
    }
    Ok(archive
        .finish()
        .map_err(|error| CoreError::Other(error.to_string()))?
        .into_inner())
}

#[cfg(test)]
pub(crate) fn store_cloud_archive(
    core: &Core,
    scheme_key: &str,
    version: Option<&str>,
    source: &str,
    display_name: Option<&str>,
    zip_bytes: &[u8],
    user_root: &Path,
) -> Result<String, CoreError> {
    store_cloud_archive_reader(
        core,
        scheme_key,
        version,
        source,
        display_name,
        std::io::Cursor::new(zip_bytes),
        user_root,
        None,
    )
}

pub(crate) fn store_cloud_archive_reader(
    core: &Core,
    scheme_key: &str,
    version: Option<&str>,
    source: &str,
    display_name: Option<&str>,
    reader: impl Read + Seek,
    user_root: &Path,
    origin: Option<&str>,
) -> Result<String, CoreError> {
    let scheme_key = crate::scheme::validate_scheme_key(scheme_key).map_err(CoreError::Other)?;
    let store = PackageStore::new(core);
    let dir = store.checked_root()?;
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|error| CoreError::Other(error.to_string()))?;
    let mut manifest = BTreeMap::new();
    let mut expanded = 0u64;
    if archive.len() > 10_000 {
        return Err(CoreError::Other("方案包文件过多".into()));
    }
    let paths = checked_archive_paths(
        &mut archive,
        if source == "official" {
            &dir
        } else {
            user_root
        },
    )
    .map_err(CoreError::Other)?;
    let mut read_bytes = 0u64;
    for relative in paths {
        let mut file = archive
            .by_name(&relative)
            .map_err(|error| CoreError::Other(error.to_string()))?;
        expanded = expanded.saturating_add(file.size());
        if expanded > 512 * 1024 * 1024 {
            return Err(CoreError::Other("方案包解压后过大".into()));
        }
        if excluded_from_manifest(&relative) {
            continue;
        }
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            read_bytes = read_bytes.saturating_add(read as u64);
            if read_bytes > 512 * 1024 * 1024 {
                return Err(CoreError::Other("方案包解压后过大".into()));
            }
            digest.update(&buffer[..read]);
        }
        manifest.insert(relative, format!("{:x}", digest.finalize()));
    }
    let mut reader = archive.into_inner();
    reader.rewind()?;
    let digest = sha256_reader(&mut reader)?;
    reader.rewind()?;
    // Allocate under the store lock so concurrent downloads cannot claim the same key.
    let store_guard = core
        .pending_package
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let state = core.state();
    let occupied = |key: &str| {
        state.installed_packages.contains_key(key)
            || store.package_path(key).exists()
            || store.metadata_path(key).exists()
    };
    let mut key = scheme_key.to_owned();
    let existing = store
        .read_metadata_bytes(scheme_key)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<PackageMeta>(&bytes).ok())
        .filter(|meta| meta.zip_sha256.eq_ignore_ascii_case(&digest));
    let preserve_cloud_id = existing.is_some();
    if crate::scheme::is_local_scheme_key(scheme_key) && occupied(scheme_key) {
        if existing.is_none() {
            let mut number = 2u64;
            while occupied(&key) {
                let suffix = format!("-{number}");
                let base = &scheme_key[..scheme_key.len().min(106 - suffix.len())];
                key = format!("{base}{suffix}");
                number += 1;
            }
        }
    }
    let scheme_key = key.as_str();
    let meta = PackageMeta {
        scheme_key: scheme_key.into(),
        engine: PackageEngine::Rime,
        version: version.map(str::to_owned),
        installed_at_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        source: if source == "official" {
            existing
                .as_ref()
                .map_or(source, |meta| meta.source.as_str())
        } else if crate::scheme::is_local_scheme_key(scheme_key) {
            "local"
        } else {
            source
        }
        .into(),
        display_name: if source == "official" {
            existing.as_ref().and_then(|meta| meta.display_name.clone())
        } else if crate::scheme::is_local_scheme_key(scheme_key) {
            existing
                .as_ref()
                .and_then(|meta| meta.display_name.clone())
                .or_else(|| display_name.map(str::to_owned))
        } else {
            // Built-in names and variants resolve through the UI catalogue.
            None
        },
        selection_base_version: None,
        manifest,
        zip_sha256: digest,
    };
    fs::create_dir_all(&dir)?;
    store.write_archive(reader, &meta, origin)?;
    core.update_state(|state| {
        state.installed_packages.insert(
            scheme_key.into(),
            crate::state::InstalledPackage {
                version: version.map(str::to_owned),
            },
        );
        if source == "official" && !preserve_cloud_id {
            state.scheme_cloud_ids.remove(scheme_key);
        }
    })?;
    drop(store_guard);
    crate::user_words::remember_reconciliation(core, scheme_key, user_root, false);
    Ok(key)
}

pub(crate) fn excluded_from_manifest(relative: &str) -> bool {
    matches!(
        relative.rsplit('/').next().unwrap_or(relative),
        "default.custom.yaml" | "default-custom.yaml"
    ) || relative == "rime.lua"
        || crate::package_activation::user_file(relative)
}

pub fn list_installed_packages(core: &Core) -> Result<Vec<InstalledPackageInfo>, CoreError> {
    crate::addon_store::migrate_snow_package(core)?;
    PackageStore::new(core).list_installed_packages()
}

fn notify_package_engine_change(root: &Path) -> Result<(), CoreError> {
    keytao_core::ReloadStamp::write(root).map_err(CoreError::Other)?;
    Ok(())
}

pub fn set_package_engine(core: &Core, root: &Path, key: &str, engine: &str) -> Result<(), CoreError> {
    let engine = match engine {
        "rime" => PackageEngine::Rime,
        "libjd" if key == PACKAGE_KEY => PackageEngine::Libjd,
        "libjd" => return Err(CoreError::Other("只有键道6支持 ime-jd".into())),
        _ => return Err(CoreError::Other("未知输入引擎".into())),
    };
    if engine == PackageEngine::Libjd {
        let status = crate::libjd::libjd_status(core);
        if !status.available || status.installed_version.is_none() {
            return Err(CoreError::Other("请先安装 ime-jd".into()));
        }
    }
    keytao_core::isolated_deploy::with_isolated_package_lock(root, || {
        PackageStore::new(core).set_engine(root, key, engine).map_err(|error| error.to_string())
    })
    .map_err(CoreError::Other)?;
    if key == PACKAGE_KEY && keytao_core::engine_choice::active_package(root).as_deref() == Some(key) {
        notify_package_engine_change(root)?;
    }
    Ok(())
}

pub async fn remove_scheme_package(
    core: &Core,
    scheme_key: &str,
    host: &impl crate::scheme_host::SchemeHost,
) -> Result<Vec<String>, CoreError> {
    let scheme_key = crate::scheme::validate_scheme_key(scheme_key).map_err(CoreError::Other)?;
    let root = host.user_root().map_err(CoreError::Other)?;
    crate::package_migration::ensure_before_write(core, &root).await?;
    let store = PackageStore::new(core);
    store.checked_root()?;
    if keytao_core::RuntimeDirs::active_package_key(&root).as_deref() == Some(scheme_key) {
        let mut installed: BTreeSet<_> = core.state().installed_packages.into_keys().collect();
        installed.extend(
            crate::runtime_materialize::installed_addons(&store)?
                .into_iter()
                .filter(|key| *key != crate::addon::EASY_EN_ADDON_ID)
                .map(str::to_owned),
        );
        installed.remove(scheme_key);
        if let Some(replacement) = installed.first() {
            host.switch_package(replacement).await.map_err(CoreError::Other)?;
        } else {
            clear_active_package(&root, scheme_key)?;
        }
    }
    keytao_core::isolated_deploy::with_isolated_package_lock(&root, || {
        if keytao_core::RuntimeDirs::active_package_key(&root).as_deref() == Some(scheme_key) {
            return Err("请先切换方案包".into());
        }
        let namespace = if scheme_key.starts_with("local-") { "local" } else { "cloud" };
        let projected = safe_path(&root, &format!("scheme-packages/{namespace}/{scheme_key}"))?;
        let owner = safe_path(&projected, ".keytao-source-projection")?;
        let source_backup = match fs::read(&owner) {
            Ok(bytes) if bytes == b"1\n" => {
                let backup = tempfile::tempdir_in(root.join("runtime"))
                    .map_err(|error| error.to_string())?;
                crate::package_fs::rename(&projected, &backup.path().join("source"))
                    .map_err(|error| error.to_string())?;
                Some(backup)
            }
            Ok(_) => None,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.to_string()),
        };
        let removed = (|| {
            if scheme_key == crate::rime_snow::SCHEMA_ID {
                crate::addon_store::AddonStore::new(core, &root, scheme_key)?.remove()?;
            }
            store.remove(scheme_key).map_err(|error| error.to_string())
        })();
        if let Err(error) = removed {
            if let Some(backup) = source_backup
                && let Err(restore) = crate::package_fs::rename(&backup.path().join("source"), &projected)
            {
                return Err(format!(
                    "{error}; restore projected sources: {restore}; backup: {}",
                    backup.keep().display()
                ));
            }
            return Err(error);
        }
        core.update_state(|state| {
            state.installed_packages.remove(scheme_key);
            if state.active_scheme.as_deref() == Some(scheme_key) {
                state.active_scheme = None;
            }
        })
        .map_err(|error| error.to_string())
    })
    .map_err(CoreError::Other)?;
    #[cfg(any(target_os = "android", target_os = "ios"))]
    crate::runtime_materialize::isolated_packages(core, &root)?;
    let retained = keytao_core::RuntimeDirs::isolated_user_data_dir(&root, scheme_key)
        .filter(|path| path.is_dir())
        .and_then(|path| path.strip_prefix(&root).ok().map(Path::to_path_buf))
        .map(|path| vec![path.to_string_lossy().into_owned()])
        .unwrap_or_default();
    Ok(retained)
}

pub(crate) fn clear_active_package(root: &Path, key: &str) -> Result<(), CoreError> {
    use keytao_core::RuntimeDirs;
    keytao_core::isolated_deploy::with_isolated_package_lock(root, || {
        if RuntimeDirs::active_package_key(root).as_deref() != Some(key) {
            return Ok(());
        }
        let path = safe_path(root, "scheme-packages/active-manifest.json")?;
        let previous = match fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.to_string()),
        };
        let mut manifest: serde_json::Map<String, serde_json::Value> = previous
            .as_deref()
            .map(serde_json::from_slice)
            .transpose()
            .map_err(|error| error.to_string())?
            .unwrap_or_default();
        manifest.insert("active_package".into(), serde_json::Value::Null);
        if RuntimeDirs::migration_complete(root) {
            manifest.insert("runtime_migration_version".into(), 1.into());
        }
        fs::create_dir_all(path.parent().unwrap()).map_err(|error| error.to_string())?;
        let bytes = serde_json::to_vec(&manifest).map_err(|error| error.to_string())?;
        crate::scheme_files::write_file_atomic(&path, &bytes).map_err(|error| error.to_string())?;
        if let Err(error) = keytao_core::ReloadStamp::write(root) {
            let rollback = match previous {
                Some(bytes) => crate::scheme_files::write_file_atomic(&path, &bytes),
                None => fs::remove_file(&path),
            };
            return Err(match rollback {
                Ok(()) => error,
                Err(rollback) => format!("{error}; restore active package manifest: {rollback}"),
            });
        }
        Ok(())
    })
    .map_err(CoreError::Other)
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
            Err(error) => {
                return Err(crate::package_fs::context("读取文件属性", &path, error).to_string());
            }
        }
    }
    Ok(path)
}

pub(crate) fn staging_relative(root: &Path, dirs: &keytao_core::RuntimeDirs) -> String {
    dirs.staging_dir
        .strip_prefix(root)
        .expect("staging directory is inside the control root")
        .to_string_lossy()
        .replace('\\', "/")
}

pub(super) fn checked_archive_paths<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    root: &Path,
) -> Result<BTreeSet<String>, String> {
    let mut paths = BTreeSet::new();
    let mut names = BTreeSet::new();
    for index in 0..archive.len() {
        let file = archive.by_index(index).map_err(|e| e.to_string())?;
        let relative = file.name().trim_end_matches('/');
        safe_path(root, relative)?;
        if !names.insert(relative.to_lowercase()) {
            return Err(format!(
                "Package contains a case-insensitive duplicate path: {relative}"
            ));
        }
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

#[cfg(test)]
#[path = "package_removal_tests.rs"]
mod removal_tests;

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

    fn legacy_active_state(core: &Core, key: &str, version: &str) {
        core.update_state(|state| {
            state.active_scheme = Some(key.into());
            state.installed_packages.insert(key.into(), crate::state::InstalledPackage {
                version: Some(version.into()),
            });
        }).unwrap();
    }

    #[tokio::test]
    async fn live_install_stores_fresh_package_without_flat_activation() {
        let dir = tempfile::tempdir().unwrap();
        let core = open_core(dir.path());
        let root = dir.path().join("rime");
        fs::create_dir(&root).unwrap();
        let files = vec![(
            "alpha.schema.yaml".into(),
            b"schema: {schema_id: alpha}\n".to_vec(),
        )];
        let archive = dir.path().join("download.zip");
        fs::write(&archive, archive_from_files(&files).unwrap()).unwrap();

        crate::install::install_live_scheme(
            &core,
            archive.to_string_lossy().into_owned(),
            &root,
            "xmjd",
            Some("v1"),
        )
        .await
        .unwrap();

        assert!(!root.join("alpha.schema.yaml").exists());
        assert!(!root.join(keytao_core::ACTIVE_SCHEME_MANIFEST).exists());
        assert!(!root.join("build").exists());
        assert!(keytao_core::RuntimeDirs::active_package_key(&root).is_none());
        let (stored, meta) = PackageStore::new(&core).read_files("xmjd").unwrap();
        assert_eq!(stored, files);
        assert_eq!(meta.version.as_deref(), Some("v1"));
    }

    #[test]
    fn streaming_hash_matches_whole_file_across_chunks() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let bytes: Vec<u8> = (0..(3 * 64 * 1024 + 173))
            .map(|i| (i % 251) as u8)
            .collect();
        fs::write(file.path(), &bytes).unwrap();
        assert_eq!(
            sha256_reader(fs::File::open(file.path()).unwrap()).unwrap(),
            format!("{:x}", Sha256::digest(fs::read(file.path()).unwrap()))
        );
    }

    #[test]
    fn list_packages_skips_corrupt_and_unreadable_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let core = open_core(dir.path());
        let store = PackageStore::new(&core);
        fs::create_dir_all(store.checked_root().unwrap()).unwrap();
        fs::create_dir_all(store.package_path("local-corrupt")).unwrap();
        fs::create_dir_all(store.package_path("local-unreadable")).unwrap();
        fs::write(store.metadata_path("local-corrupt"), "invalid json").unwrap();
        fs::create_dir(store.metadata_path("local-unreadable")).unwrap();
        core.update_state(|state| {
            for key in ["local-corrupt", "local-unreadable", "keytao"] {
                state
                    .installed_packages
                    .insert(key.into(), crate::state::InstalledPackage { version: None });
            }
        })
        .unwrap();
        let packages = list_installed_packages(&core).unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].scheme_key, "keytao");
        assert_eq!(core.state().installed_packages.len(), 3);
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
    async fn download_scheme_package_allows_active_key_without_mutating_user_files() {
        let dir = tempfile::tempdir().unwrap();
        let core = open_core(dir.path());
        legacy_active_state(&core, "keytao", "v1");
        let root = dir.path().join("rime");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("user.yaml"), "var: {previously_selected_schema: keytao}").unwrap();
        let archive = dir.path().join("download.zip");
        fs::write(&archive, user_words_package()).unwrap();
        download_scheme_package_with(
            &core,
            "keytao",
            &root,
            || std::future::ready(Ok(("v2".into(), "local-source".into()))),
            |_| std::future::ready(Ok(archive.to_string_lossy().into_owned())),
        )
        .await
        .unwrap();
        assert_eq!(fs::read_to_string(root.join("user.yaml")).unwrap(), "var: {previously_selected_schema: keytao}");
        assert_eq!(core.state().active_scheme.as_deref(), Some("keytao"));
        assert_eq!(
            core.state().installed_packages["keytao"].version.as_deref(),
            Some("v2")
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
        legacy_active_state(&core, "keytao", "v1");
        legacy_active_state(&core, "xmjd", "20261104");
        legacy_active_state(&core, "keytao", "v1");
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
            &rime,
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
        let meta: PackageMeta = serde_json::from_slice(
            &fs::read(PackageStore::new(&core).metadata_path("xmjd")).unwrap(),
        )
        .unwrap();
        assert_eq!(meta.version.as_deref(), Some("20261105"));
        assert_eq!(meta.manifest.len(), 1);
        assert!(!PackageStore::new(&core).active_manifest_path().exists());
    }

    #[tokio::test]
    async fn download_scheme_package_preserves_cloud_reference_only_for_identical_archive() {
        for identical in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let core = open_core(dir.path());
            let root = dir.path().join("rime");
            let original = user_words_package();
            store_cloud_archive(&core, "txjx", Some("v1"), "cloud", None, &original, &root)
                .unwrap();
            let metadata_path = PackageStore::new(&core).metadata_path("txjx");
            let mut previous: serde_json::Value =
                serde_json::from_slice(&fs::read(&metadata_path).unwrap()).unwrap();
            previous["display_name"] = serde_json::json!("Existing name");
            fs::write(&metadata_path, serde_json::to_vec(&previous).unwrap()).unwrap();
            core.update_state(|state| {
                state.scheme_cloud_ids.insert("txjx".into(), "21".into());
            })
            .unwrap();
            let bytes = if identical {
                original.clone()
            } else {
                let mut zip = zip::ZipWriter::new_append(std::io::Cursor::new(original)).unwrap();
                zip.set_comment("Changed archive");
                zip.finish().unwrap().into_inner()
            };
            let archive = dir.path().join("download.zip");
            fs::write(&archive, &bytes).unwrap();

            download_scheme_package_with(
                &core,
                "txjx",
                &root,
                || std::future::ready(Ok(("v2".into(), "local".into()))),
                |_| std::future::ready(Ok(archive.to_string_lossy().into_owned())),
            )
            .await
            .unwrap();

            let (stored, metadata) = stored_archive(&core, "txjx").unwrap();
            assert_eq!(archive_contents(&stored), archive_contents(&bytes));
            assert_eq!(metadata.zip_sha256, format!("{:x}", Sha256::digest(&bytes)));
            assert_eq!(metadata.version.as_deref(), Some("v2"));
            assert_eq!(
                metadata.source,
                if identical { "cloud" } else { "official" }
            );
            assert_eq!(
                metadata.display_name.as_deref(),
                identical.then_some("Existing name")
            );
            assert_eq!(
                core.state()
                    .scheme_cloud_ids
                    .get("txjx")
                    .map(String::as_str),
                identical.then_some("21")
            );
        }
    }

    #[tokio::test]
    async fn download_scheme_package_enforces_archive_expansion_limits() {
        let mut oversized = user_words_package();
        let entry = oversized
            .windows(4)
            .position(|window| window == b"PK\x01\x02")
            .unwrap();
        // Declare an oversized entry without allocating its advertised content.
        oversized[entry + 24..entry + 28]
            .copy_from_slice(&(512u32 * 1024 * 1024 + 1).to_le_bytes());
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for index in 0..10_001 {
            zip.start_file(index.to_string(), zip::write::SimpleFileOptions::default())
                .unwrap();
        }
        let many_files = zip.finish().unwrap().into_inner();
        for (bytes, expected) in [
            (oversized, "方案包解压后过大"),
            (many_files, "方案包文件过多"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let core = open_core(dir.path());
            let archive = dir.path().join("download.zip");
            fs::write(&archive, bytes).unwrap();

            let error = download_scheme_package_with(
                &core,
                "txjx",
                &dir.path().join("rime"),
                || std::future::ready(Ok(("v2".into(), "local".into()))),
                |_| std::future::ready(Ok(archive.to_string_lossy().into_owned())),
            )
            .await
            .unwrap_err();

            assert_eq!(error.to_string(), expected);
            assert!(core.state().installed_packages.is_empty());
            assert!(!PackageStore::new(&core).package_path("txjx").exists());
        }
    }

    fn archive_contents(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        (0..archive.len())
            .filter_map(|index| {
                let mut file = archive.by_index(index).unwrap();
                if file.is_dir() {
                    return None;
                }
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes).unwrap();
                Some((file.name().into(), bytes))
            })
            .collect()
    }

    fn user_words_package() -> Vec<u8> {
        let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (path, content) in [
            (
                "txjx.schema.yaml",
                "translator:\n  dictionary: txjx.extended\n",
            ),
            (
                "txjx.extended.dict.yaml",
                "---\nname: txjx.extended\nimport_tables: [txjx.phrases, txjx.user]\n...\n",
            ),
            (
                "txjx.phrases.dict.yaml",
                "---\nname: txjx.phrases\n...\nOfficial\taa\nOther\tbb\n",
            ),
            ("txjx.user.dict.yaml", "---\nname: txjx.user\n...\n"),
        ] {
            archive
                .start_file(path, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(content.as_bytes()).unwrap();
        }
        archive.finish().unwrap().into_inner()
    }

    fn write_reconciliation_words(root: &Path) {
        fs::create_dir_all(root).unwrap();
        fs::write(
            root.join("txjx.user.dict.yaml"),
            "---\nname: txjx.user\n...\n# Keep this comment\nOfficial\taa\nCustom\tbb\nMine\tcc\n",
        )
        .unwrap();
    }

    #[tokio::test]
    async fn inactive_package_download_reconciles_without_changing_active_scheme() {
        let dir = tempfile::tempdir().unwrap();
        let core = open_core(dir.path());
        legacy_active_state(&core, "keytao", "v1");
        let root = dir.path().join("rime");
        write_reconciliation_words(&root);
        let original = fs::read(root.join("txjx.user.dict.yaml")).unwrap();
        let archive = dir.path().join("new-package.zip");
        fs::write(&archive, user_words_package()).unwrap();
        download_scheme_package_with(
            &core,
            "txjx",
            &root,
            || std::future::ready(Ok(("v2".into(), "local".into()))),
            |_| std::future::ready(Ok(archive.to_string_lossy().into_owned())),
        )
        .await
        .unwrap();
        assert_eq!(core.state().active_scheme.as_deref(), Some("keytao"));
        assert_eq!(
            fs::read(root.join("txjx.user.dict.yaml")).unwrap(),
            original
        );
        let report = crate::user_words::take_user_words_report(&core, "txjx").unwrap();
        assert!(report.removed.is_empty());
        assert_eq!(report.conflicts.len(), 1);
        assert_eq!(report.conflicts[0].entry.word, "Custom");
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
            legacy_active_state(&core, scheme_key, version);
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
