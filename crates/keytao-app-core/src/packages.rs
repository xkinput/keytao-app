use crate::{Core, CoreError};
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
mod package_logos;

#[derive(Serialize, Deserialize)]
pub struct PackageMeta {
    pub scheme_key: String,
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

#[derive(Serialize, Deserialize)]
struct ActiveManifest {
    root: String,
    #[serde(default)]
    scheme_key: String,
    manifest: BTreeMap<String, String>,
    #[serde(default)]
    rime_lua: Option<String>,
}

#[derive(Serialize)]
pub struct InstalledPackageInfo {
    pub scheme_key: String,
    pub version: Option<String>,
    pub active: bool,
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

fn has_scheme_update(installed: &str, latest: &str) -> bool {
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
    if core.state().active_scheme.as_deref() == Some(scheme_key) {
        return Err(CoreError::Other(
            "Cannot download the active scheme package; update it through install and deploy"
                .into(),
        ));
    }
    let (version, url) = fetch().await?;
    let archive_path = download(url).await?;
    let result = (|| {
        store_cloud_archive_reader(
            core,
            scheme_key,
            Some(&version),
            "official",
            None,
            BufReader::new(fs::File::open(&archive_path)?),
            user_root,
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

pub(crate) fn stored_metadata(
    core: &Core,
    scheme_key: &str,
) -> Result<Option<PackageMeta>, CoreError> {
    let scheme_key = crate::scheme::validate_scheme_key(scheme_key).map_err(CoreError::Other)?;
    let _store_guard = core
        .pending_package
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let metadata = safe_path(&checked_store_dir(core)?, &format!("{scheme_key}.json"))
        .map_err(CoreError::Other)?;
    let bytes = match fs::read(metadata) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let meta: PackageMeta = serde_json::from_slice(&bytes)?;
    if meta.scheme_key != scheme_key
        || meta.zip_sha256.len() != 64
        || !meta.zip_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(CoreError::Other("本地方案包信息无效".into()));
    }
    Ok(Some(meta))
}

fn sha256_reader(mut reader: impl Read) -> std::io::Result<String> {
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

fn sha256_file(path: &Path) -> std::io::Result<String> {
    sha256_reader(BufReader::new(fs::File::open(path)?))
}

pub(crate) fn stored_archive_file(
    core: &Core,
    scheme_key: &str,
) -> Result<(BufReader<fs::File>, PackageMeta), CoreError> {
    let scheme_key = crate::scheme::validate_scheme_key(scheme_key).map_err(CoreError::Other)?;
    let _store_guard = core
        .pending_package
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let dir = checked_store_dir(core)?;
    let archive = safe_path(&dir, &format!("{scheme_key}.zip")).map_err(CoreError::Other)?;
    let metadata = safe_path(&dir, &format!("{scheme_key}.json")).map_err(CoreError::Other)?;
    // The upload size limit applies to the rebuilt dependency zip, not this distribution.
    let mut archive = BufReader::new(fs::File::open(archive)?);
    let meta: PackageMeta = serde_json::from_slice(&fs::read(metadata)?)?;
    if meta.scheme_key != scheme_key || meta.zip_sha256 != sha256_reader(&mut archive)? {
        return Err(CoreError::Other("本地方案包校验失败".into()));
    }
    archive.rewind()?;
    Ok((archive, meta))
}

#[cfg(test)]
pub(crate) fn stored_archive(
    core: &Core,
    scheme_key: &str,
) -> Result<(Vec<u8>, PackageMeta), CoreError> {
    let (mut archive, meta) = stored_archive_file(core, scheme_key)?;
    let mut bytes = Vec::new();
    archive.read_to_end(&mut bytes)?;
    Ok((bytes, meta))
}

pub(crate) fn rime_lua_from_archive(zip_bytes: &[u8]) -> Result<Option<String>, String> {
    rime_lua_from_reader(std::io::Cursor::new(zip_bytes))
}

fn rime_lua_from_reader(reader: impl Read + Seek) -> Result<Option<String>, String> {
    let mut archive = zip::ZipArchive::new(reader).map_err(|error| error.to_string())?;
    let mut file = match archive.by_name("rime.lua") {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let mut content = String::new();
    file.read_to_string(&mut content)
        .map_err(|error| error.to_string())?;
    Ok(Some(content))
}

pub(crate) fn prepare_active_rime_lua(core: &Core, root: &Path) -> Result<(), CoreError> {
    let addon_only = ["default.custom.yaml", "default-custom.yaml"]
        .into_iter()
        .find_map(|name| fs::read_to_string(root.join(name)).ok())
        .is_some_and(|content| {
            crate::scheme_files::parse_schema_list(&content)
                .iter()
                .all(|schema| keytao_core::is_addon_schema(schema))
        });
    if addon_only {
        return keytao_core::regenerate_active_rime_lua(root).map_err(CoreError::Other);
    }
    let Some(scheme_key) = core.state().active_scheme else {
        return Ok(());
    };
    let (archive, meta) = match stored_archive_file(core, &scheme_key) {
        Ok(archive) => archive,
        Err(error) => {
            tracing::warn!(%scheme_key, %error, "Skipping active rime.lua snapshot refresh: no verified stored archive");
            return Ok(());
        }
    };
    let active = ActiveManifest {
        root: root.canonicalize()?.to_string_lossy().into_owned(),
        scheme_key,
        manifest: meta.manifest,
        rime_lua: rime_lua_from_reader(archive).map_err(CoreError::Other)?,
    };
    let path = safe_path(root, keytao_core::ACTIVE_SCHEME_MANIFEST).map_err(CoreError::Other)?;
    crate::scheme_files::write_file_atomic(&path, &serde_json::to_vec_pretty(&active)?)?;
    keytao_core::regenerate_active_rime_lua(root).map_err(CoreError::Other)
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
) -> Result<String, CoreError> {
    let scheme_key = crate::scheme::validate_scheme_key(scheme_key).map_err(CoreError::Other)?;
    let dir = checked_store_dir(core)?;
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
            || dir.join(format!("{key}.zip")).exists()
            || dir.join(format!("{key}.json")).exists()
    };
    let mut key = scheme_key.to_owned();
    let existing = fs::read(dir.join(format!("{scheme_key}.json")))
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
    if core.state().active_scheme.as_deref() == Some(scheme_key) {
        return Err(CoreError::Other("请先切换到其他方案".into()));
    }
    fs::create_dir_all(&dir)?;
    let archive_path = safe_path(&dir, &format!("{scheme_key}.zip")).map_err(CoreError::Other)?;
    let meta_path = safe_path(&dir, &format!("{scheme_key}.json")).map_err(CoreError::Other)?;
    let mut stored = tempfile::Builder::new()
        .prefix(".cloud-")
        .tempfile_in(&dir)?;
    std::io::copy(&mut reader, stored.as_file_mut())?;
    stored.as_file().sync_all()?;
    let mut metadata = tempfile::Builder::new()
        .prefix(".meta-")
        .tempfile_in(&dir)?;
    serde_json::to_writer_pretty(metadata.as_file_mut(), &meta)?;
    metadata.as_file().sync_all()?;
    stored
        .persist(archive_path)
        .map_err(|error| CoreError::from(error.error))?;
    metadata
        .persist(meta_path)
        .map_err(|error| CoreError::from(error.error))?;
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

fn select_main_schema_after_addon(root: &Path) -> Result<(), CoreError> {
    let manifest =
        safe_path(root, keytao_core::ACTIVE_SCHEME_MANIFEST).map_err(CoreError::Other)?;
    let previous: serde_json::Value = match fs::read(manifest) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !previous["scheme_key"]
        .as_str()
        .is_some_and(keytao_core::is_addon_schema)
    {
        return Ok(());
    }
    let mut schemas = Vec::new();
    for name in ["default.custom.yaml", "default-custom.yaml"] {
        let path = safe_path(root, name).map_err(CoreError::Other)?;
        match fs::read_to_string(path) {
            Ok(content) => {
                schemas = crate::scheme_files::parse_schema_list(&content);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let schema = schemas
        .into_iter()
        .find(|schema| !keytao_core::is_addon_schema(schema))
        .ok_or_else(|| CoreError::Other("Active package has no primary schema".into()))?;
    let path = safe_path(root, "user.yaml").map_err(CoreError::Other)?;
    let mut value = match fs::read_to_string(&path) {
        Ok(content) => serde_yaml::from_str::<serde_yaml::Value>(&content)
            .map_err(|error| CoreError::Other(format!("Invalid user.yaml: {error}")))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            serde_yaml::Value::Mapping(serde_yaml::Mapping::new())
        }
        Err(error) => return Err(error.into()),
    };
    let mapping = value
        .as_mapping_mut()
        .ok_or_else(|| CoreError::Other("user.yaml must be a YAML mapping".into()))?;
    let vars = mapping
        .entry(serde_yaml::Value::String("var".into()))
        .or_insert_with(|| serde_yaml::Value::Mapping(serde_yaml::Mapping::new()))
        .as_mapping_mut()
        .ok_or_else(|| CoreError::Other("user.yaml var must be a YAML mapping".into()))?;
    vars.insert("previously_selected_schema".into(), schema.into());
    let content =
        serde_yaml::to_string(&value).map_err(|error| CoreError::Other(error.to_string()))?;
    crate::scheme_files::write_file_atomic(&path, content.as_bytes())?;
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
    let previous = fs::read(dir.join(format!("{scheme_key}.json")))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<PackageMeta>(&bytes).ok())
        .filter(|meta| meta.zip_sha256 == receipt.zip_sha256);
    let preserve_cloud_id = previous.is_some();
    let display_name = previous.as_ref().and_then(|meta| meta.display_name.clone());
    let meta = PackageMeta {
        scheme_key: scheme_key.into(),
        version: version.map(str::to_owned),
        installed_at_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        source: previous.map_or_else(|| "official".into(), |meta| meta.source),
        display_name,
        selection_base_version: None,
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
    if active {
        let active_manifest = ActiveManifest {
            root: receipt.root.clone(),
            scheme_key: scheme_key.into(),
            manifest: receipt.manifest.clone(),
            // Native IME processes deploy without access to the app's package store.
            rime_lua: rime_lua_from_reader(BufReader::new(receipt.archive.reopen()?))
                .map_err(CoreError::Other)?,
        };
        if !receipt.root.is_empty() {
            let root = Path::new(&receipt.root);
            select_main_schema_after_addon(root)?;
            if let Some(path) = crate::addon::owned_addon_default(root).map_err(CoreError::Other)? {
                fs::remove_file(path)?;
            }
            let mut marker = tempfile::Builder::new()
                .prefix(".active-")
                .tempfile_in(root)?;
            serde_json::to_writer_pretty(marker.as_file_mut(), &active_manifest)?;
            marker.as_file().sync_all()?;
            marker
                .persist(root.join(keytao_core::ACTIVE_SCHEME_MANIFEST))
                .map_err(|error| CoreError::from(error.error))?;
        }
        let mut temporary = tempfile::Builder::new()
            .prefix(".active-")
            .tempfile_in(&dir)?;
        serde_json::to_writer_pretty(temporary.as_file_mut(), &active_manifest)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(dir.join("active-manifest.json"))
            .map_err(|error| CoreError::from(error.error))?;
    }
    temporary
        .persist(dir.join(format!("{scheme_key}.json")))
        .map_err(|error| CoreError::from(error.error))?;
    *pending = None;
    if !preserve_cloud_id && core.state().scheme_cloud_ids.contains_key(scheme_key) {
        core.update_state(|state| {
            state.scheme_cloud_ids.remove(scheme_key);
        })?;
    }
    Ok(())
}

fn select_package_primary_schema(receipt: &PendingPackage) -> Result<(), CoreError> {
    let mut archive = zip::ZipArchive::new(BufReader::new(receipt.archive.reopen()?))
        .map_err(|error| CoreError::Other(error.to_string()))?;
    let Some(index) = archive.file_names().position(|name| {
        matches!(
            name.rsplit('/').next(),
            Some("default.custom.yaml" | "default-custom.yaml")
        )
    }) else {
        return Ok(());
    };
    let mut config = String::new();
    archive
        .by_index(index)
        .map_err(|error| CoreError::Other(error.to_string()))?
        .read_to_string(&mut config)?;
    let Some(primary) = crate::scheme_files::parse_schema_list(&config)
        .into_iter()
        .next()
    else {
        return Ok(());
    };
    let path = safe_path(Path::new(&receipt.root), "user.yaml").map_err(CoreError::Other)?;
    let mut user: serde_yaml::Mapping = match fs::read(&path) {
        Ok(bytes) => serde_yaml::from_slice(&bytes)
            .map_err(|error| CoreError::Other(format!("Cannot read user.yaml: {error}")))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_yaml::Mapping::new(),
        Err(error) => return Err(error.into()),
    };
    let vars = user
        .entry(serde_yaml::Value::String("var".into()))
        .or_insert_with(|| serde_yaml::Value::Mapping(serde_yaml::Mapping::new()))
        .as_mapping_mut()
        .ok_or_else(|| CoreError::Other("user.yaml var must be a mapping".into()))?;
    vars.insert(
        serde_yaml::Value::String("previously_selected_schema".into()),
        serde_yaml::Value::String(primary),
    );
    let content =
        serde_yaml::to_string(&user).map_err(|error| CoreError::Other(error.to_string()))?;
    crate::scheme_files::write_file_atomic(&path, content.as_bytes())?;
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
    let state = core.state();
    let selection_base_version = if crate::scheme::is_local_scheme_key(scheme_key)
        && state.active_scheme.as_deref() == Some(scheme_key)
    {
        stored_metadata(core, scheme_key)?.and_then(|meta| meta.selection_base_version)
    } else {
        None
    };
    let updating_active_package = state.active_scheme.as_deref() == Some(scheme_key)
        && state
            .installed_packages
            .get(scheme_key)
            .is_some_and(|package| {
                has_scheme_update(
                    selection_base_version
                        .as_deref()
                        .or(package.version.as_deref())
                        .unwrap_or_default(),
                    version.unwrap_or_default(),
                )
            });
    let installed_root = {
        let pending = core
            .pending_package
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(receipt) = pending.as_ref().filter(|receipt| !receipt.root.is_empty()) {
            // Only a same-package version update preserves a manual schema selection.
            if !updating_active_package {
                select_package_primary_schema(receipt)?;
            }
            Some(PathBuf::from(&receipt.root))
        } else {
            None
        }
    };
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
            state.last_deploy = None;
        }
    })?;
    if let Some(root) = installed_root {
        crate::user_words::remember_reconciliation(core, scheme_key, &root, true);
    }
    Ok(())
}

pub fn list_installed_packages(core: &Core) -> Result<Vec<InstalledPackageInfo>, CoreError> {
    let dir = checked_store_dir(core)?;
    let state = core.state();
    Ok(state
        .installed_packages
        .into_iter()
        .filter_map(|(scheme_key, package)| {
            let has_package_zip = crate::scheme::validate_scheme_key(&scheme_key).is_ok()
                && fs::symlink_metadata(dir.join(format!("{scheme_key}.zip")))
                    .is_ok_and(|meta| meta.is_file());
            let meta = if crate::scheme::validate_scheme_key(&scheme_key).is_ok() {
                match stored_metadata(core, &scheme_key) {
                    Ok(meta) => meta,
                    Err(error) => {
                        tracing::warn!(%scheme_key, %error, "Skipping unreadable package metadata");
                        return None;
                    }
                }
            } else {
                None
            };
            Some(InstalledPackageInfo {
                logo_path: if has_package_zip {
                    package_logos::package_logo_path(core, &scheme_key)
                        .map(|path| path.to_string_lossy().into_owned())
                } else {
                    None
                },
                active: state.active_scheme.as_deref() == Some(&scheme_key),
                supports_user_words: crate::user_words::supports_user_words(core, &scheme_key)
                    .unwrap_or_else(|error| {
                        tracing::warn!(%scheme_key, %error, "Cannot read user dictionary support");
                        false
                    }),
                scheme_key,
                version: package.version,
                has_package_zip,
                source: meta
                    .as_ref()
                    .map(|meta| meta.source.clone())
                    .unwrap_or_else(|| "official".into()),
                display_name: meta.and_then(|meta| meta.display_name),
            })
        })
        .collect())
}

pub async fn remove_scheme_package(
    core: &Core,
    scheme_key: &str,
    host: &impl crate::scheme_host::SchemeHost,
) -> Result<(), CoreError> {
    let scheme_key = crate::scheme::validate_scheme_key(scheme_key).map_err(CoreError::Other)?;
    if core.state().active_scheme.as_deref() == Some(scheme_key) {
        remove_active_package(core, scheme_key, host).await?;
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

async fn remove_active_package(
    core: &Core,
    scheme_key: &str,
    host: &impl crate::scheme_host::SchemeHost,
) -> Result<(), CoreError> {
    let root = host.user_root().map_err(CoreError::Other)?.canonicalize()?;
    let mut active = None;
    for path in [
        safe_path(&root, keytao_core::ACTIVE_SCHEME_MANIFEST).map_err(CoreError::Other)?,
        checked_store_dir(core)?.join("active-manifest.json"),
    ] {
        match fs::read(path) {
            Ok(bytes) => {
                let manifest: ActiveManifest = serde_json::from_slice(&bytes)?;
                if Path::new(&manifest.root) != root
                    || (!manifest.scheme_key.is_empty() && manifest.scheme_key != scheme_key)
                {
                    return Err(CoreError::Other(
                        "Active scheme manifest does not match".into(),
                    ));
                }
                active = Some(manifest);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let (manifest, package_rime_lua) = match active {
        Some(active) => (active.manifest, active.rime_lua),
        None => {
            let manifest = stored_metadata(core, scheme_key)?
                .ok_or_else(|| CoreError::Other("Active scheme file manifest is missing".into()))?
                .manifest;
            (manifest, None)
        }
    };
    let user_rime_lua =
        match fs::read_to_string(safe_path(&root, "rime.lua").map_err(CoreError::Other)?) {
            Ok(content) if Some(content.as_str()) != package_rime_lua.as_deref() => Some(content),
            Ok(_) => None,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
    let protected: BTreeSet<_> = manifest
        .keys()
        .filter(|relative| {
            relative.ends_with(".custom.yaml")
                || relative.split('/').any(|part| {
                    part == "sync" || part.contains(".userdb") || part.starts_with(".keytao-")
                })
                || crate::addon::is_package_file(relative)
                || crate::wanxiang::installable(relative)
                || relative.as_str() == "wanxiang-lts-zh-hans.gram"
                || crate::rime_ice::installable(relative)
        })
        .cloned()
        .collect();
    let schema_ids: Vec<_> = manifest
        .keys()
        .filter(|relative| !protected.contains(*relative))
        .filter_map(|relative| relative.strip_suffix(".schema.yaml").map(str::to_owned))
        .collect();
    let dictionary_ids: Vec<_> = manifest
        .keys()
        .filter(|relative| !protected.contains(*relative))
        .filter_map(|relative| relative.strip_suffix(".dict.yaml").map(str::to_owned))
        .collect();
    let (relative, mut config) =
        crate::addon::addon_default_custom(&root).map_err(CoreError::Other)?;
    for schema in &schema_ids {
        config = crate::addon::update_addon_schema_list_content(&config, schema, false)
            .map_err(CoreError::Other)?;
    }
    let mut receipt = crate::wanxiang::InstallReceipt::new(&root).map_err(CoreError::Other)?;
    cleanup_manifest_with(&root, &manifest, &protected, |relative| {
        receipt.move_original(relative)
    })
    .map_err(CoreError::Other)?;
    crate::addon::write_schema_list_content_transaction(
        &mut receipt,
        relative,
        &config,
        &crate::addon::shared_data_dir(core, &root),
        user_rime_lua.as_deref(),
    )
    .map_err(CoreError::Other)?;
    let previous = core.state();
    let empty = crate::scheme_files::parse_schema_list(&config).is_empty();
    #[cfg(target_os = "windows")]
    let mut public_update = None;
    let result: Result<(), CoreError> = async {
        for marker in ["build/.keytao-deployed", "build/.keytao-deploying"] {
            match fs::remove_file(safe_path(&root, marker).map_err(CoreError::Other)?) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        keytao_core::invalidate_rime_build_artifacts(&root, &schema_ids, &dictionary_ids)
            .map_err(CoreError::Other)?;
        #[cfg(target_os = "windows")]
        {
            let files = manifest
                .keys()
                .filter(|relative| {
                    !protected.contains(*relative) && !excluded_from_manifest(relative)
                })
                .map(PathBuf::from)
                .collect::<Vec<_>>();
            public_update = Some(
                crate::windows_public_schemas::update_public_schemas(|| {
                    for schema in &schema_ids {
                        host.remove_public_schema(schema, &files)?;
                    }
                    Ok(())
                })
                .map_err(CoreError::Other)?,
            );
        }
        if empty {
            host.write_reload_stamp(&root).map_err(CoreError::Other)?;
        } else {
            host.deploy().await.map_err(CoreError::Other)?;
        }
        core.update_state(|state| {
            state.active_scheme = None;
            if empty {
                state.last_deploy = None;
            } else if let Some(deploy) = state.last_deploy.as_mut() {
                deploy.scheme_key = None;
            }
        })
    }
    .await;
    if let Err(error) = result {
        let rollback = receipt.rollback();
        #[cfg(target_os = "windows")]
        let public_rollback = public_update.map_or(Ok(()), |update| update.rollback());
        let restore_state = core.update_state(|state| {
            state.active_scheme = previous.active_scheme;
            state.last_deploy = previous.last_deploy;
        });
        rollback.map_err(|restore| {
            CoreError::Other(format!("{error}; restoring sources failed: {restore}"))
        })?;
        #[cfg(target_os = "windows")]
        public_rollback.map_err(|restore| {
            CoreError::Other(format!("{error}; restoring public schemas failed: {restore}"))
        })?;
        restore_state?;
        let restore_deploy = host.deploy().await;
        if let Some(lua) = user_rime_lua {
            crate::scheme_files::write_file_atomic(
                &safe_path(&root, "rime.lua").map_err(CoreError::Other)?,
                lua.as_bytes(),
            )?;
        }
        restore_deploy.map_err(|restore| {
            CoreError::Other(format!(
                "{error}; restoring the previous deployment failed: {restore}"
            ))
        })?;
        return Err(error);
    }
    receipt.finish().map_err(CoreError::Other)?;
    let store_manifest = checked_store_dir(core)?.join("active-manifest.json");
    match fs::remove_file(store_manifest) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
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
        || meta.zip_sha256 != sha256_file(&archive)?
    {
        return Err(CoreError::Other(
            "Stored package metadata is inconsistent; re-download this scheme before activation"
                .into(),
        ));
    }
    #[cfg(not(target_os = "android"))]
    let result = crate::install::smart_install_preserving_settings(
        core,
        archive.to_string_lossy().into_owned(),
        root.to_string_lossy().into_owned(),
        meta.source == "local",
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
    checked_archive_paths(&mut archive, root)
}

fn checked_archive_paths<R: Read + Seek>(
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
    let mut logs = cleanup_manifest(root, &meta.manifest, &incoming)?;
    // Package-owned auxiliary schemas must not survive as user schema references.
    for name in ["default.custom.yaml", "default-custom.yaml"] {
        let path = safe_path(root, name)?;
        if !path.is_file() {
            continue;
        }
        let mut content = fs::read_to_string(&path).map_err(|error| error.to_string())?;
        let mut changed = false;
        for id in crate::scheme_files::parse_schema_list(&content) {
            let source = format!("{id}.schema.yaml");
            if meta.manifest.contains_key(&source)
                && !incoming.contains(&source)
                && !safe_path(root, &source)?.is_file()
            {
                content = crate::addon::update_addon_schema_list_content(&content, &id, false)?;
                logs.push(format!("[REMOVED SCHEMA] {id}"));
                changed = true;
            }
        }
        if changed {
            crate::scheme_files::write_file_atomic(&path, content.as_bytes())
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(logs)
}

pub(crate) fn cleanup_manifest(
    root: &Path,
    manifest: &BTreeMap<String, String>,
    incoming: &BTreeSet<String>,
) -> Result<Vec<String>, String> {
    let mut incoming = incoming.clone();
    incoming.extend(
        manifest
            .keys()
            .filter(|path| path.starts_with("lua/"))
            .cloned(),
    );
    cleanup_manifest_with(root, manifest, &incoming, |relative| {
        fs::remove_file(safe_path(root, relative)?).map_err(|error| error.to_string())
    })
}

fn cleanup_manifest_with(
    root: &Path,
    manifest: &BTreeMap<String, String>,
    incoming: &BTreeSet<String>,
    mut remove: impl FnMut(&str) -> Result<(), String>,
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
        match sha256_file(&path) {
            Ok(digest) if digest == *expected => {
                remove(relative)?;
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
        let bytes = zip.finish().unwrap().into_inner();
        store_cloud_archive(&core, "keytao", Some("v1"), "official", None, &bytes, &root).unwrap();
        record_package(&core, "keytao", Some("v1"), true).unwrap();
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
        let archive_path = store_dir(&core).join("keytao.zip");
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
                ["txjx", addon]
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
        let store = checked_store_dir(&core).unwrap();
        fs::create_dir_all(&store).unwrap();
        fs::write(store.join("local-corrupt.json"), "invalid json").unwrap();
        fs::create_dir(store.join("local-unreadable.json")).unwrap();
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
    async fn download_scheme_package_refuses_active_key() {
        let dir = tempfile::tempdir().unwrap();
        let core = open_core(dir.path());
        crate::scheme::record_installed_scheme(&core, "keytao", Some("v1")).unwrap();

        let error = download_scheme_package_with(
            &core,
            "keytao",
            &dir.path().join("rime"),
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
        let meta: PackageMeta =
            serde_json::from_slice(&fs::read(store_dir(&core).join("xmjd.json")).unwrap()).unwrap();
        assert_eq!(meta.version.as_deref(), Some("20261105"));
        assert_eq!(meta.manifest.len(), 1);
        assert!(!store_dir(&core).join("active-manifest.json").exists());
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
        assert_eq!(stored, pending_bytes);
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
            let metadata_path = store_dir(&core).join("txjx.json");
            let mut previous: PackageMeta =
                serde_json::from_slice(&fs::read(&metadata_path).unwrap()).unwrap();
            previous.display_name = Some("Existing name".into());
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
            assert_eq!(stored, bytes);
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
            assert!(!store_dir(&core).join("txjx.zip").exists());
        }
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
