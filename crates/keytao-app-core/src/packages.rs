use crate::package_store::{PackageStore, Staging};
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
pub(crate) mod package_logos;

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
    archive: crate::package_store::StagingFile,
    manifest: BTreeMap<String, String>,
    root: String,
    zip_sha256: String,
    pub(crate) activation: Option<crate::package_activation::Activation>,
    pub(crate) transaction: Option<crate::package_activation::Transaction>,
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

fn sha256_file(path: &Path) -> std::io::Result<String> {
    sha256_reader(BufReader::new(fs::File::open(path)?))
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
            .start_file(path, zip::write::SimpleFileOptions::default())
            .map_err(|error| CoreError::Other(error.to_string()))?;
        archive.write_all(bytes)?;
    }
    Ok(archive
        .finish()
        .map_err(|error| CoreError::Other(error.to_string()))?
        .into_inner())
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
    let (files, meta) = match PackageStore::new(core).read_files(&scheme_key) {
        Ok(files) => files,
        Err(error) => {
            tracing::warn!(%scheme_key, %error, "Skipping active rime.lua snapshot refresh: no verified stored archive");
            return Ok(());
        }
    };
    let active = ActiveManifest {
        root: root.canonicalize()?.to_string_lossy().into_owned(),
        scheme_key,
        manifest: meta.manifest,
        rime_lua: files
            .into_iter()
            .find(|(path, _)| path == "rime.lua")
            .map(|(_, bytes)| {
                String::from_utf8(bytes).map_err(|error| CoreError::Other(error.to_string()))
            })
            .transpose()?,
    };
    let path = safe_path(root, keytao_core::ACTIVE_SCHEME_MANIFEST).map_err(CoreError::Other)?;
    crate::scheme_files::write_file_atomic(&path, &serde_json::to_vec_pretty(&active)?)?;
    keytao_core::regenerate_active_rime_lua(root).map_err(CoreError::Other)
}

pub(crate) fn refresh_generated(
    core: &Core,
    root: &Path,
    activation: &mut crate::package_activation::Activation,
) -> Result<(), CoreError> {
    let mut paths = crate::managed_custom::generated_paths(root).map_err(CoreError::Other)?;
    let addons = crate::package_activation::addon_ownership(core, root)?;
    paths.extend(["default.custom.yaml", "default-custom.yaml"].map(str::to_owned));
    activation.generated.clear();
    for path in paths {
        if addons.contains_key(&path) {
            continue;
        }
        if let Some(file) = crate::package_activation::fingerprint(root, &path)? {
            activation.generated.push(file);
        }
    }
    if let Some(content) = crate::install::prepare_rime_lua_install(
        activation.rime_lua.as_deref(),
        keytao_core::installed_addon_rime_lua_lines(root),
    ) {
        activation
            .generated
            .push(crate::package_activation::OwnedFile {
                path: "rime.lua".into(),
                sha256: sha256_reader(content.as_bytes())?,
                move_file: false,
            });
    }
    Ok(())
}

pub fn sync_generated(core: &Core, root: &Path) -> Result<(), CoreError> {
    let mut active = crate::package_activation::read(core, root)?.unwrap_or_else(|| {
        crate::package_activation::Activation {
            root: root.to_string_lossy().into_owned(),
            ..Default::default()
        }
    });
    active.root = root.canonicalize()?.to_string_lossy().into_owned();
    refresh_generated(core, root, &mut active)?;
    crate::package_activation::write(core, &active)?;
    Ok(())
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

pub(crate) fn stage_install(
    core: &Core,
    zip_bytes: &[u8],
    manifest: BTreeMap<String, String>,
    root: String,
) -> Result<(), CoreError> {
    use std::io::Write;
    let store = PackageStore::new(core);
    let dir = store.checked_root()?;
    fs::create_dir_all(&dir)?;
    let mut archive = store.staging_file(Staging::Pending)?;
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
        activation: None,
        transaction: None,
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
    if let Some(receipt) = pending
        .as_ref()
        .filter(|receipt| active && !receipt.root.is_empty())
    {
        // Reject invalid user settings before consuming the retryable receipt.
        select_main_schema_after_addon(Path::new(&receipt.root))?;
    }
    let Some(mut receipt) = pending.take() else {
        return Ok(());
    };
    let store = PackageStore::new(core);
    store.checked_root()?;
    let previous = store
        .read_metadata_bytes(scheme_key)
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
    // Publish the durable store version before either activation marker.
    if let Some(transaction) = receipt.transaction.as_mut() {
        let mut archive = zip::ZipArchive::new(BufReader::new(receipt.archive.reopen()?))
            .map_err(|e| CoreError::Other(e.to_string()))?;
        let paths = checked_archive_paths(&mut archive, Path::new(&receipt.root))
            .map_err(CoreError::Other)?;
        transaction.snapshot_package(scheme_key, &paths)?;
    }
    #[cfg(target_os = "windows")]
    let publish_public = active
        && receipt.transaction.is_some()
        && crate::windows_public_schemas::is_default_user_root(Path::new(&receipt.root));
    #[cfg(target_os = "windows")]
    if publish_public {
        if let Some(previous) = core.state().active_scheme {
            let files = store.package_path(&previous).join("files");
            if files.is_dir() {
                crate::windows_public_schemas::remove_package(core, &files, Path::new(&receipt.root))
                    .map_err(CoreError::Other)?;
            }
        }
    }
    store.write_archive(BufReader::new(receipt.archive.reopen()?), &meta, None)?;
    #[cfg(target_os = "windows")]
    if publish_public {
        crate::windows_public_schemas::publish_activation(
            core,
            &store.package_path(scheme_key).join("files"),
            None,
            Path::new(&receipt.root),
        ).map_err(CoreError::Other)?;
    }
    if active {
        let active_manifest = ActiveManifest {
            root: receipt.root.clone(),
            scheme_key: scheme_key.into(),
            manifest: receipt.manifest.clone(),
            // Native IME processes deploy without access to the app's package store.
            rime_lua: match receipt.activation.as_ref() {
                Some(activation) => activation.rime_lua.clone(),
                None => rime_lua_from_reader(BufReader::new(receipt.archive.reopen()?))
                    .map_err(CoreError::Other)?,
            },
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
        let mut temporary = store.staging_file(Staging::ActiveManifest)?;
        serde_json::to_writer_pretty(temporary.as_file_mut(), &active_manifest)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(store.active_manifest_path())
            .map_err(|error| CoreError::from(error.error))?;
        if let Some(mut activation) = receipt.activation.take() {
            activation.key = scheme_key.into();
            activation.scheme_key = scheme_key.into();
            activation.manifest = receipt.manifest.clone();
            activation.rime_lua = active_manifest.rime_lua;
            crate::scheme_files::write_file_atomic(
                &store.checked_activation_path()?,
                &serde_json::to_vec_pretty(&activation)?,
            )?;
        }
    }
    if !preserve_cloud_id && core.state().scheme_cloud_ids.contains_key(scheme_key) {
        core.update_state(|state| {
            state.scheme_cloud_ids.remove(scheme_key);
        })?;
    }
    if active {
        core.update_state(|state| {
            state.installed_packages.insert(
                scheme_key.into(),
                crate::state::InstalledPackage {
                    version: version.map(str::to_owned),
                },
            );
            state.active_scheme = Some(scheme_key.into());
            state.last_deploy = None;
        })?;
    }
    if let Some(transaction) = receipt.transaction.take() {
        transaction.commit()?;
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
        if let Some(mut receipt) = core
            .pending_package
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            if let Some(transaction) = receipt.transaction.take() {
                transaction.commit()?;
            }
        }
        return Ok(());
    }
    let state = core.state();
    let selection_base_version = if crate::scheme::is_local_scheme_key(scheme_key)
        && state.active_scheme.as_deref() == Some(scheme_key)
    {
        PackageStore::new(core)
            .read_meta(scheme_key)?
            .and_then(|meta| meta.selection_base_version)
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
    if let Err(error) = finalize_pending(core, scheme_key, version, active) {
        *core.lock_state() = crate::AppState::load(&core.env().data_dir)?;
        return Err(error);
    }
    if installed_root.is_none() {
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
    }
    if let Some(root) = installed_root {
        crate::user_words::remember_reconciliation(core, scheme_key, &root, true);
    }
    Ok(())
}

pub fn list_installed_packages(core: &Core) -> Result<Vec<InstalledPackageInfo>, CoreError> {
    PackageStore::new(core).list_installed_packages()
}

pub async fn remove_scheme_package(
    core: &Core,
    scheme_key: &str,
    host: &impl crate::scheme_host::SchemeHost,
) -> Result<Vec<String>, CoreError> {
    let scheme_key = crate::scheme::validate_scheme_key(scheme_key).map_err(CoreError::Other)?;
    let retained = if core.state().active_scheme.as_deref() == Some(scheme_key) {
        match remove_active_package(core, scheme_key, host).await {
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
    store.remove(scheme_key)?;
    core.update_state(|state| {
        state.installed_packages.remove(scheme_key);
    })?;
    Ok(retained)
}

async fn remove_active_package(
    core: &Core,
    scheme_key: &str,
    host: &impl crate::scheme_host::SchemeHost,
) -> Result<Vec<String>, CoreError> {
    let root = host.user_root().map_err(CoreError::Other)?.canonicalize()?;
    if let Some(activation) = crate::package_activation::read(core, &root)?
        .filter(|activation| !activation.key.is_empty())
    {
        return remove_owned_package(core, scheme_key, host, &root, activation).await;
    }
    let mut active = None;
    for path in [
        safe_path(&root, keytao_core::ACTIVE_SCHEME_MANIFEST).map_err(CoreError::Other)?,
        PackageStore::new(core).checked_active_manifest_path()?,
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
            let manifest = PackageStore::new(core)
                .read_meta(scheme_key)?
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
    let files = PackageStore::new(core).read_files(scheme_key)?.0;
    let paths = crate::package_activation::transaction_paths(&root, &files, None)?;
    let transaction = crate::package_activation::Transaction::begin(core, &root, paths)?;
    let addons = crate::package_activation::addon_ownership(core, &root)?;
    let protected: BTreeSet<_> = manifest
        .keys()
        .filter(|relative| {
            crate::package_activation::user_file(relative)
                || addons.contains_key(*relative)
        })
        .cloned()
        .collect();
    let schema_ids: Vec<_> = manifest
        .keys()
        .filter(|relative| !protected.contains(*relative))
        .filter_map(|relative| relative.strip_suffix(".schema.yaml").map(str::to_owned))
        .collect();
    let (relative, mut config) =
        crate::addon::addon_default_custom(&root).map_err(CoreError::Other)?;
    for schema in &schema_ids {
        config = crate::addon::update_addon_schema_list_content(&config, schema, false)
            .map_err(CoreError::Other)?;
    }
    let mut receipt = crate::wanxiang::InstallReceipt::new(&root).map_err(CoreError::Other)?;
    let logs = cleanup_manifest_with(&root, &manifest, &protected, |relative| {
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
    let result: Result<(), CoreError> = async {
        for marker in ["build/.keytao-deployed", "build/.keytao-deploying"] {
            match fs::remove_file(safe_path(&root, marker).map_err(CoreError::Other)?) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        let remaining = crate::scheme_files::parse_schema_list(&config)
            .into_iter()
            .collect();
        crate::package_artifacts::cleanup(&root, &files, &remaining)?;
        let default_build = safe_path(&root, "build/default.yaml").map_err(CoreError::Other)?;
        if default_build.try_exists()? {
            fs::remove_file(default_build)?;
        }
        #[cfg(target_os = "windows")]
        {
            if crate::windows_public_schemas::is_default_user_root(&root) {
                crate::windows_public_schemas::remove_package(
                    core,
                    &PackageStore::new(core).package_path(scheme_key).join("files"),
                    &root,
                ).map_err(CoreError::Other)?;
            }
        }
        if empty {
            host.write_reload_stamp(&root).map_err(CoreError::Other)?;
        } else {
            host.deploy().await.map_err(CoreError::Other)?;
        }
        Ok(())
    }
    .await;
    if let Err(error) = result {
        let rollback = receipt.rollback();
        let restore_state = core.update_state(|state| {
            state.active_scheme = previous.active_scheme;
            state.last_deploy = previous.last_deploy;
        });
        rollback.map_err(|restore| {
            CoreError::Other(format!("{error}; restoring sources failed: {restore}"))
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
    let store_manifest = PackageStore::new(core).checked_active_manifest_path()?;
    match fs::remove_file(store_manifest) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    core.update_state(|state| {
        state.active_scheme = None;
        if empty {
            state.last_deploy = None;
        } else if let Some(deploy) = state.last_deploy.as_mut() {
            deploy.scheme_key = None;
        }
    })?;
    if let Err(error) = transaction.commit() {
        *core.lock_state() = crate::AppState::load(&core.env().data_dir)?;
        return Err(error);
    }
    Ok(crate::package_activation::retained(&logs))
}

async fn remove_owned_package(
    core: &Core,
    scheme_key: &str,
    host: &impl crate::scheme_host::SchemeHost,
    root: &Path,
    mut activation: crate::package_activation::Activation,
) -> Result<Vec<String>, CoreError> {
    if activation.key != scheme_key {
        return Err(CoreError::Other(
            "Active scheme manifest does not match".into(),
        ));
    }
    let files = PackageStore::new(core).read_files(scheme_key)?.0;
    let paths = crate::package_activation::transaction_paths(root, &[], Some(&activation))?;
    let transaction = crate::package_activation::Transaction::begin(core, root, paths)?;
    let logs = crate::package_activation::deactivate(core, root, &mut activation)?;
    let mut remaining = BTreeSet::new();
    for name in ["default.custom.yaml", "default-custom.yaml"] {
        if let Ok(content) = fs::read_to_string(safe_path(root, name).map_err(CoreError::Other)?) {
            remaining.extend(crate::scheme_files::parse_schema_list(&content));
        }
    }
    crate::package_artifacts::cleanup(root, &files, &remaining)?;
    for name in [
        "build/default.yaml",
        "build/.keytao-deployed",
        "build/.keytao-deploying",
        keytao_core::ACTIVE_SCHEME_MANIFEST,
    ] {
        let path = safe_path(root, name).map_err(CoreError::Other)?;
        if path.try_exists()? {
            fs::remove_file(path)?;
        }
    }
    // Keep the native deployer's package source in sync with the remaining add-ons.
    let lua = fs::read_to_string(safe_path(root, "rime.lua").map_err(CoreError::Other)?).ok();
    let marker = ActiveManifest {
        root: root.to_string_lossy().into_owned(),
        scheme_key: String::new(),
        manifest: BTreeMap::new(),
        rime_lua: lua,
    };
    crate::scheme_files::write_file_atomic(
        &root.join(keytao_core::ACTIVE_SCHEME_MANIFEST),
        &serde_json::to_vec_pretty(&marker)?,
    )?;
    #[cfg(target_os = "windows")]
    if crate::windows_public_schemas::is_default_user_root(root) {
        crate::windows_public_schemas::remove_package(
            core,
            &PackageStore::new(core).package_path(scheme_key).join("files"),
            root,
        ).map_err(CoreError::Other)?;
    }
    let deployment = if remaining.is_empty() {
        host.write_reload_stamp(root)
            .map(|_| ())
            .map_err(CoreError::Other)
    } else {
        host.deploy().await.map_err(CoreError::Other)
    };
    if let Err(error) = deployment {
        drop(transaction);
        return Err(error);
    }
    let mut inactive = crate::package_activation::Activation {
        root: root.canonicalize()?.to_string_lossy().into_owned(),
        kept_backups: activation.kept_backups,
        ..Default::default()
    };
    refresh_generated(core, root, &mut inactive)?;
    crate::package_activation::write(core, &inactive)?;
    let legacy = PackageStore::new(core).checked_active_manifest_path()?;
    if legacy.try_exists()? {
        fs::remove_file(legacy)?;
    }
    core.update_state(|state| {
        state.active_scheme = None;
        state.last_deploy = None;
    })?;
    if let Err(error) = transaction.commit() {
        *core.lock_state() = crate::AppState::load(&core.env().data_dir)?;
        return Err(error);
    }
    Ok(crate::package_activation::retained(&logs))
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
    #[cfg(target_os = "android")] _android_extract: impl FnOnce(
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
    let store = PackageStore::new(core);
    store.checked_root()?;
    if !store.package_path(scheme_key).is_dir() {
        return Err(CoreError::Other(
            "Stored package directory is missing; re-download this scheme before activation".into(),
        ));
    }
    let (files, meta) = store.read_files(scheme_key)?;
    if meta.scheme_key != scheme_key || meta.version != package.version {
        return Err(CoreError::Other(
            "Stored package metadata is inconsistent; re-download this scheme before activation"
                .into(),
        ));
    }
    let result = crate::install::smart_install_files(
        core,
        files,
        root.to_string_lossy().into_owned(),
        meta.source == "local",
        #[cfg(target_os = "windows")]
        finish_windows,
    )
    .await?;
    if let Some(receipt) = core
        .pending_package
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_mut()
    {
        receipt.zip_sha256 = meta.zip_sha256;
    }
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
    if let Some(mut active) = crate::package_activation::read(core, root)
        .map_err(|e| e.to_string())?
        .filter(|active| !active.key.is_empty())
    {
        let logs = crate::package_activation::deactivate(core, root, &mut active)
            .map_err(|e| e.to_string())?;
        crate::package_activation::write(core, &active).map_err(|e| e.to_string())?;
        return Ok(logs);
    }
    let Some(active) = core.state().active_scheme else {
        return Ok(Vec::new());
    };
    crate::scheme::validate_scheme_key(&active)?;
    let path = PackageStore::new(core)
        .checked_active_manifest_path()
        .map_err(|e| e.to_string())?;
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
    let addons = crate::package_activation::addon_ownership(core, root)
        .map_err(|e| e.to_string())?;
    let protected = meta
        .manifest
        .keys()
        .filter(|path| addons.contains_key(*path))
        .cloned()
        .collect();
    let mut logs = cleanup_manifest(root, &meta.manifest, &protected)?;
    if meta.rime_lua.is_some() {
        let path = safe_path(root, "rime.lua")?;
        let expected = crate::install::prepare_rime_lua_install(
            meta.rime_lua.as_deref(),
            keytao_core::installed_addon_rime_lua_lines(root),
        );
        if fs::read_to_string(&path).ok() == expected {
            match crate::install::prepare_rime_lua_install(
                None,
                keytao_core::installed_addon_rime_lua_lines(root),
            ) {
                Some(content) => crate::scheme_files::write_file_atomic(&path, content.as_bytes())
                    .map_err(|e| e.to_string())?,
                None if path.exists() => fs::remove_file(path).map_err(|e| e.to_string())?,
                None => {}
            }
        } else if path.exists() {
            logs.push("[KEPT] User-modified file: rime.lua".into());
        }
    }
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
    cleanup_manifest_with(root, manifest, incoming, |relative| {
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
