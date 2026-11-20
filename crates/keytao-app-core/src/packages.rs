#[cfg(test)]
pub(crate) use crate::legacy_flat::{cleanup_manifest, finalize_pending, stage_install};
#[cfg(test)]
use crate::legacy_flat::{ActiveManifest, sha256_file};
pub(crate) use crate::legacy_flat::PendingPackage;
pub use crate::legacy_flat::{record_package, sync_generated};
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
    if scheme_key == crate::rime_snow::SCHEMA_ID {
        crate::rime_snow::remove_legacy(core, host).await?;
    }
    let root = host.user_root().map_err(CoreError::Other)?;
    let installed = crate::package_activation::read_package(core, &root, scheme_key)?;
    let retained = if installed.is_some() || core.state().active_scheme.as_deref() == Some(scheme_key) {
        match crate::legacy_flat::remove_active_package(core, scheme_key, host).await {
            Ok(retained) => retained,
            Err(error) => {
                *core.lock_state() = crate::AppState::load(&core.env().data_dir)?;
                return Err(error);
            }
        }
    } else {
        Vec::new()
    };
    let store = PackageStore::new(core);
    store.checked_root()?;
    if scheme_key == PACKAGE_KEY
        && keytao_core::engine_choice::read(&root, Some(scheme_key)) == PackageEngine::Libjd
    {
        set_package_engine(core, &root, scheme_key, "rime")?;
    }
    store.remove(scheme_key)?;
    core.update_state(|state| {
        state.installed_packages.remove(scheme_key);
    })?;
    Ok(retained)
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

pub async fn activate_scheme(
    core: &Core,
    scheme_key: &str,
    root: &Path,
    #[cfg(target_os = "windows")] finish_windows: crate::install::WindowsInstallFinish,
) -> Result<crate::InstallResult, crate::CoreError> {
    if keytao_core::RuntimeDirs::isolation_enabled(root) {
        return Err(CoreError::Other("方案包隔离已启用".into()));
    }
    crate::legacy_flat::activate_scheme(
        core,
        scheme_key,
        root,
        #[cfg(target_os = "windows")]
        finish_windows,
    )
    .await
}

pub(crate) fn archive_paths(zip_bytes: &[u8], root: &Path) -> Result<BTreeSet<String>, String> {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).map_err(|e| e.to_string())?;
    checked_archive_paths(&mut archive, root)
}

pub(super) fn checked_archive_paths<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    root: &Path,
) -> Result<BTreeSet<String>, String> {
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

#[cfg(test)]
#[path = "package_removal_tests.rs"]
mod removal_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::legacy_flat::prepare_active_rime_lua;
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
    fn package_selection_rejects_invalid_user_yaml_without_changes() {
        let dir = tempfile::tempdir().unwrap();
        let core = open_core(dir.path());
        let root = dir.path().join("rime");
        fs::create_dir_all(&root).unwrap();
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file(
            "default.custom.yaml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(b"patch:\n  schema_list:\n    - schema: keytao\n")
            .unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        stage_install(
            &core,
            &bytes,
            BTreeMap::new(),
            root.to_string_lossy().into_owned(),
        )
        .unwrap();
        for user in ["var: [\n", "- keep\n", "var: keep\n", "var: {}\nvar: {}\n"] {
            fs::write(root.join("user.yaml"), user).unwrap();
            assert!(record_package(&core, "keytao", Some("v1"), true).is_err());
            assert_eq!(fs::read_to_string(root.join("user.yaml")).unwrap(), user);
            assert!(core.state().active_scheme.is_none());
            assert!(core.pending_package.lock().unwrap().is_some());
        }
    }

    #[test]
    fn streaming_hash_matches_whole_file_across_chunks() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let bytes: Vec<u8> = (0..(3 * 64 * 1024 + 173))
            .map(|i| (i % 251) as u8)
            .collect();
        fs::write(file.path(), &bytes).unwrap();
        assert_eq!(
            sha256_file(file.path()).unwrap(),
            format!("{:x}", Sha256::digest(fs::read(file.path()).unwrap()))
        );
    }

    #[test]
    fn deploy_preparation_preserves_lua_snapshots_and_checks_stored_lua_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let core = open_core(dir.path());
        let root = dir.path().join("rime");
        fs::create_dir_all(&root).unwrap();
        let files = vec![
            ("rime.lua".into(), b"helper = require('helper')\n".to_vec()),
            ("lua/helper.lua".into(), b"return 1\n".to_vec()),
            ("keytao.schema.yaml".into(), b"schema: {schema_id: keytao}\n".to_vec()),
            ("default.custom.yaml".into(), b"patch: {schema_list: [{schema: keytao}]}\n".to_vec()),
            ("large.dict.yaml".into(), vec![b'x'; 1024 * 1024]),
        ];
        let archive = archive_from_files(&files).unwrap();
        store_cloud_archive(&core, "keytao", Some("v1"), "official", None, &archive, &root).unwrap();
        record_package(&core, "keytao", Some("v1"), true).unwrap();
        fs::write(root.join("default.custom.yaml"), &files[3].1).unwrap();
        prepare_active_rime_lua(&core, &root).unwrap();
        let snapshot = root.join(".keytao-package-lua/keytao/lua/helper.lua");
        fs::remove_file(PackageStore::new(&core).package_path("keytao").join("files/large.dict.yaml")).unwrap();
        fs::remove_file(&snapshot).unwrap();
        prepare_active_rime_lua(&core, &root).unwrap();
        assert_eq!(fs::read(&snapshot).unwrap(), b"return 1\n");
        let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(100);
        fs::File::options().write(true).open(&snapshot).unwrap().set_modified(modified).unwrap();
        prepare_active_rime_lua(&core, &root).unwrap();
        assert_eq!(fs::metadata(&snapshot).unwrap().modified().unwrap(), modified);
        let manifest_path = root.join(keytao_core::ACTIVE_SCHEME_MANIFEST);
        let manifest = fs::read(&manifest_path).unwrap();
        for relative in ["rime.lua", "lua/helper.lua"] {
            let stored = PackageStore::new(&core).package_path("keytao").join("files").join(relative);
            let original = fs::read(&stored).unwrap();
            fs::write(&stored, vec![b'x'; original.len()]).unwrap();
            prepare_active_rime_lua(&core, &root).unwrap();
            assert_eq!(fs::read(&snapshot).unwrap(), b"return 1\n");
            assert_eq!(fs::read(&manifest_path).unwrap(), manifest);
            fs::write(stored, original).unwrap();
        }
    }

    #[test]
    fn deploy_preparation_migrates_legacy_lua_from_the_verified_package() {
        let dir = tempfile::tempdir().unwrap();
        let core = open_core(dir.path());
        let root = dir.path().join("rime");
        fs::create_dir_all(&root).unwrap();
        let package_lua = "number_translator = require('xnumber')\r\n";
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file("rime.lua", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(package_lua.as_bytes()).unwrap();
        zip.start_file("keytao.schema.yaml", zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(b"schema: {schema_id: keytao}\n").unwrap();
        zip.start_file("default.custom.yaml", zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(b"patch: {schema_list: [{schema: keytao}]}\n").unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        store_cloud_archive(&core, "keytao", Some("v1"), "official", None, &bytes, &root).unwrap();
        record_package(&core, "keytao", Some("v1"), true).unwrap();
        fs::write(root.join("default.custom.yaml"), b"patch: {schema_list: [{schema: keytao}]}\n").unwrap();
        fs::write(root.join("user.yaml"), b"var: {previously_selected_schema: keytao}\n").unwrap();
        fs::write(
            root.join(keytao_core::ACTIVE_SCHEME_MANIFEST),
            serde_json::json!({
                "root": root.canonicalize().unwrap(),
                "scheme_key": "keytao", "manifest": {},
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            root.join("rime.lua"),
            "number_translator = require('translators.xnumber')\n",
        )
        .unwrap();

        for _ in 0..2 {
            crate::deploy::record_result(&core, &root, false).unwrap();
            assert_eq!(
                fs::read_to_string(root.join("rime.lua")).unwrap(),
                package_lua
            );
        }
        let active: serde_json::Value = serde_json::from_slice(
            &fs::read(root.join(keytao_core::ACTIVE_SCHEME_MANIFEST)).unwrap(),
        )
        .unwrap();
        assert_eq!(active["rime_lua"], package_lua);

        let manifest_path = root.join(keytao_core::ACTIVE_SCHEME_MANIFEST);
        let manifest_before = fs::read(&manifest_path).unwrap();
        let archive_path = PackageStore::new(&core)
            .package_path("keytao")
            .join("files/rime.lua");
        for archive_exists in [true, false] {
            if archive_exists {
                fs::write(&archive_path, b"corrupt archive").unwrap();
            } else {
                fs::remove_file(&archive_path).unwrap();
            }
            crate::deploy::record_result(&core, &root, true).unwrap();
            crate::deploy::record_result(&core, &root, false).unwrap();
            assert!(!core.state().last_deploy.unwrap().success);
            assert_eq!(
                fs::read_to_string(root.join("rime.lua")).unwrap(),
                package_lua
            );
            assert_eq!(fs::read(&manifest_path).unwrap(), manifest_before);
        }
    }

    #[tokio::test]
    async fn active_package_selects_main_schema_only_after_standalone_addon() {
        for (previous, existing_user) in [
            ("easy_en", true),
            ("wanxiang", false),
            ("rime_ice", true),
            ("rime_ice", false),
            ("rime_mint", true),
            ("rime_mint", false),
            ("rime_frost", true),
            ("rime_frost", false),
            ("triple_pinyin_lssp", true),
            ("triple_pinyin_lssp", false),
            ("keytao", true),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let core = open_core(dir.path());
            let root = dir.path().join("rime");
            fs::create_dir_all(&root).unwrap();
            fs::write(
                root.join(keytao_core::ACTIVE_SCHEME_MANIFEST),
                serde_json::json!({"scheme_key": previous}).to_string(),
            )
            .unwrap();
            let addon = if previous == "keytao" {
                "easy_en"
            } else {
                previous
            };
            let existing_schemas = if previous == "keytao" {
                "    - schema: keytao\n    - schema: easy_en\n".into()
            } else {
                format!("    - schema: {addon}\n")
            };
            fs::write(
                root.join("default.custom.yaml"),
                format!("patch:\n  schema_list:\n{existing_schemas}"),
            )
            .unwrap();
            fs::write(root.join(format!("{addon}.schema.yaml")), "schema: {}\n").unwrap();
            let defaults = if previous == "keytao" {
                "config_version: user\n"
            } else {
                include_str!("addon-default.yaml")
            };
            fs::write(root.join("default.yaml"), defaults).unwrap();
            let user =
                "custom_key: keep\nvar:\n  previously_selected_schema: easy_en\n  option: true\n";
            if existing_user {
                fs::write(root.join("user.yaml"), user).unwrap();
            }
            let archive = dir.path().join("main.zip");
            let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
            for (name, content) in [
                (
                    "default.custom.yaml",
                    "patch:\n  schema_list:\n    - schema: txjx\n",
                ),
                ("txjx.schema.yaml", "schema:\n  schema_id: txjx\n"),
            ] {
                zip.start_file(name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(content.as_bytes()).unwrap();
            }
            zip.finish().unwrap();
            crate::install::smart_install(
                &core,
                archive.to_string_lossy().into_owned(),
                root.to_string_lossy().into_owned(),
                Some("txjx"),
                #[cfg(target_os = "windows")]
                |_, _, _, _| Ok(Vec::new()),
            )
            .await
            .unwrap();
            record_package(&core, "txjx", Some("v1"), true).unwrap();
            let content = fs::read_to_string(root.join("user.yaml")).unwrap();
            let value: serde_yaml::Value = serde_yaml::from_str(&content).unwrap();
            assert_eq!(
                value["var"]["previously_selected_schema"].as_str(),
                // Switching packages selects the new package's primary schema.
                Some("txjx")
            );
            if existing_user {
                assert_eq!(value["custom_key"].as_str(), Some("keep"));
                assert_eq!(value["var"]["option"].as_bool(), Some(true));
            }
            assert_eq!(
                crate::scheme_files::parse_schema_list(
                    &fs::read_to_string(root.join("default.custom.yaml")).unwrap()
                ),
                if previous == "keytao" { vec!["keytao", addon, "txjx"] } else { vec!["txjx", addon] }
            );
            assert!(root.join(format!("{addon}.schema.yaml")).is_file());
            assert_eq!(core.state().active_scheme.as_deref(), Some("txjx"));
            if previous == "keytao" {
                assert_eq!(
                    fs::read_to_string(root.join("default.yaml")).unwrap(),
                    defaults
                );
            } else {
                assert!(!root.join("default.yaml").exists());
            }
        }
    }

    #[test]
    fn standalone_transition_preserves_invalid_user_settings_and_pending_receipt() {
        for content in ["[invalid", "scalar", "var: scalar\n"] {
            let dir = tempfile::tempdir().unwrap();
            let core = open_core(dir.path());
            let root = dir.path().join("rime");
            fs::create_dir_all(&root).unwrap();
            let manifest = root.join(keytao_core::ACTIVE_SCHEME_MANIFEST);
            let previous = "{\"scheme_key\":\"easy_en\"}";
            fs::write(&manifest, previous).unwrap();
            fs::write(
                root.join("default.custom.yaml"),
                "patch:\n  schema_list:\n    - schema: txjx\n    - schema: easy_en\n",
            )
            .unwrap();
            fs::write(root.join("user.yaml"), content).unwrap();
            stage_install(
                &core,
                &user_words_package(),
                BTreeMap::new(),
                root.to_string_lossy().into_owned(),
            )
            .unwrap();
            assert!(record_package(&core, "txjx", Some("v1"), true).is_err());
            assert_eq!(fs::read_to_string(root.join("user.yaml")).unwrap(), content);
            assert_eq!(fs::read_to_string(manifest).unwrap(), previous);
            assert!(core.pending_package.lock().unwrap().is_some());
            assert!(core.state().active_scheme.is_none());
        }
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
        crate::scheme::record_installed_scheme(&core, "keytao", Some("v1")).unwrap();
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
    async fn download_scheme_package_preserves_pending_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let core = open_core(dir.path());
        let root = dir.path().join("rime");
        fs::create_dir_all(&root).unwrap();
        let pending_bytes = user_words_package();
        let manifest = BTreeMap::from([("txjx.schema.yaml".into(), "pending hash".into())]);
        stage_install(
            &core,
            &pending_bytes,
            manifest.clone(),
            root.to_string_lossy().into_owned(),
        )
        .unwrap();
        let archive = dir.path().join("download.zip");
        fs::write(&archive, &pending_bytes).unwrap();

        download_scheme_package_with(
            &core,
            "xmjd",
            &root,
            || std::future::ready(Ok(("v2".into(), "local".into()))),
            |_| std::future::ready(Ok(archive.to_string_lossy().into_owned())),
        )
        .await
        .unwrap();

        assert!(core.pending_package.lock().unwrap().is_some());
        record_package(&core, "txjx", Some("v1"), true).unwrap();
        let (stored, metadata) = stored_archive(&core, "txjx").unwrap();
        assert_eq!(archive_contents(&stored), archive_contents(&pending_bytes));
        assert_eq!(metadata.manifest, manifest);
        let active: ActiveManifest = serde_json::from_slice(
            &fs::read(root.join(keytao_core::ACTIVE_SCHEME_MANIFEST)).unwrap(),
        )
        .unwrap();
        assert_eq!(active.scheme_key, "txjx");
        assert_eq!(active.manifest, manifest);
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

    fn assert_reconciliation_report(core: &Core, root: &Path) {
        assert_eq!(
            fs::read_to_string(root.join("txjx.user.dict.yaml")).unwrap(),
            "---\nname: txjx.user\n...\n# Keep this comment\nCustom\tbb\nMine\tcc\n"
        );
        let report = crate::user_words::take_user_words_report(core, "txjx").unwrap();
        assert_eq!(report.removed.len(), 1);
        assert_eq!(report.removed[0].word, "Official");
        assert_eq!(report.conflicts.len(), 1);
        assert_eq!(report.conflicts[0].entry.word, "Custom");
    }

    #[test]
    fn active_package_record_reconciles_once_and_retains_report() {
        let dir = tempfile::tempdir().unwrap();
        let core = open_core(dir.path());
        let root = dir.path().join("rime");
        write_reconciliation_words(&root);
        stage_install(
            &core,
            &user_words_package(),
            BTreeMap::new(),
            root.to_string_lossy().into_owned(),
        )
        .unwrap();
        record_package(&core, "txjx", Some("v2"), true).unwrap();
        record_package(&core, "txjx", Some("v2"), true).unwrap();
        assert_reconciliation_report(&core, &root);
    }

    #[tokio::test]
    async fn inactive_package_download_reconciles_without_changing_active_scheme() {
        let dir = tempfile::tempdir().unwrap();
        let core = open_core(dir.path());
        record_package(&core, "keytao", Some("v1"), true).unwrap();
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
