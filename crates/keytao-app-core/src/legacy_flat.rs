//! Shared-root package activation and rollback for the flat layout.

pub(super) mod install;

use crate::{
    Core, CoreError,
    package_store::{PackageStore, Staging},
    packages::{
        PackageMeta, checked_archive_paths, excluded_from_manifest, has_scheme_update, safe_path,
        sha256_reader,
    },
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufReader, Read, Seek},
    path::{Path, PathBuf},
};

#[derive(Default, Serialize, Deserialize)]
pub(super) struct ActiveManifest {
    #[serde(default)]
    root: String,
    #[serde(default)]
    pub(super) scheme_key: String,
    #[serde(default)]
    pub(super) manifest: BTreeMap<String, String>,
    #[serde(default)]
    pub(super) rime_lua: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    packages: BTreeMap<String, ActivePackage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lua_owner: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize)]
struct ActivePackage {
    schemas: Vec<String>,
    rime_lua: Option<String>,
    #[serde(default)]
    lua_files: BTreeMap<String, String>,
}

pub(crate) struct PendingPackage {
    archive: crate::package_store::StagingFile,
    manifest: BTreeMap<String, String>,
    root: String,
    zip_sha256: String,
    from_store: bool,
    pub(crate) activation: Option<crate::package_activation::Activation>,
    pub(crate) transaction: Option<crate::package_activation::Transaction>,
}

pub(super) fn sha256_file(path: &Path) -> std::io::Result<String> {
    sha256_reader(BufReader::new(fs::File::open(path)?))
}

fn rime_lua_from_archive(zip_bytes: &[u8]) -> Result<Option<String>, String> {
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
    let mut records = crate::package_activation::read_all(core, root)?;
    if !records.is_empty() {
        let mut verified = BTreeMap::new();
        for (key, record) in &mut records {
            let (mut files, meta) = match PackageStore::new(core).read_lua_files(key) {
                Ok(files) => files,
                Err(error) => {
                    tracing::warn!(%key, %error, "Skipping active Lua refresh: no verified stored package");
                    return Ok(());
                }
            };
            record.rime_lua = files.iter().find(|(path, _)| path == "rime.lua")
                .map(|(_, bytes)| String::from_utf8(bytes.clone()).map_err(|error| CoreError::Other(error.to_string())))
                .transpose()?;
            if record.manifest.is_empty() { record.manifest = meta.manifest; }
            files.retain(|(path, _)| path.starts_with("lua/"));
            verified.insert(key.clone(), files);
        }
        for record in records.values() { crate::package_activation::write(core, record)?; }
        publish_active_manifest(core, root, &records, Some(&verified))?;
        return keytao_core::regenerate_active_rime_lua(root).map_err(CoreError::Other);
    }
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
    let (mut files, meta) = match PackageStore::new(core).read_lua_files(&scheme_key) {
        Ok(files) => files,
        Err(error) => {
            tracing::warn!(%scheme_key, %error, "Skipping active rime.lua snapshot refresh: no verified stored archive");
            return Ok(());
        }
    };
    let active = crate::package_activation::Activation {
        root: root.canonicalize()?.to_string_lossy().into_owned(),
        key: scheme_key.clone(),
        scheme_key,
        owned: meta.manifest.iter().map(|(path, sha256)| crate::package_activation::OwnedFile {
            path: path.clone(), sha256: sha256.clone(), move_file: false,
        }).collect(),
        manifest: meta.manifest,
        rime_lua: files
            .iter()
            .find(|(path, _)| path == "rime.lua")
            .map(|(_, bytes)| {
                String::from_utf8(bytes.clone()).map_err(|error| CoreError::Other(error.to_string()))
            })
            .transpose()?,
        ..Default::default()
    };
    crate::package_activation::write(core, &active)?;
    files.retain(|(path, _)| path.starts_with("lua/"));
    let verified = BTreeMap::from([(active.key.clone(), files)]);
    publish_active_manifest(core, root, &crate::package_activation::read_all(core, root)?, Some(&verified))?;
    keytao_core::regenerate_active_rime_lua(root).map_err(CoreError::Other)
}

fn publish_active_manifest(
    core: &Core,
    root: &Path,
    records: &BTreeMap<String, crate::package_activation::Activation>,
    verified: Option<&BTreeMap<String, Vec<(String, Vec<u8>)>>>,
) -> Result<(), CoreError> {
    let path = safe_path(root, keytao_core::ACTIVE_SCHEME_MANIFEST).map_err(CoreError::Other)?;
    let previous = match fs::read(&path) {
        Ok(bytes) => Some(serde_json::from_slice::<ActiveManifest>(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let mut marker = ActiveManifest {
        root: root.canonicalize()?.to_string_lossy().into_owned(),
        lua_owner: previous.as_ref().and_then(|marker| marker.lua_owner.clone()),
        ..Default::default()
    };
    for (key, record) in records {
        crate::scheme::validate_scheme_key(key).map_err(CoreError::Other)?;
        let stored;
        let files = match verified.and_then(|packages| packages.get(key)) {
            Some(files) => files,
            None => {
                let store = PackageStore::new(core);
                stored = PackageStore::read_lua_files_at(&store.checked_package_path(key)?, key)?.0;
                &stored
            }
        };
        let mut lua_files = BTreeMap::new();
        for (relative, bytes) in files.iter().filter(|(path, _)| path.starts_with("lua/")) {
            let snapshot = format!(".keytao-package-lua/{key}/{relative}");
            let destination = safe_path(root, &snapshot).map_err(CoreError::Other)?;
            fs::create_dir_all(destination.parent().unwrap())?;
            crate::scheme_files::write_file_atomic(&destination, bytes)?;
            lua_files.insert(relative.clone(), snapshot);
        }
        let schemas = record.manifest.keys()
            .filter_map(|path| path.strip_suffix(".schema.yaml").map(str::to_owned))
            .collect();
        marker.packages.insert(key.clone(), ActivePackage {
            schemas,
            rime_lua: record.rime_lua.clone(),
            lua_files,
        });
        marker.manifest.extend(record.manifest.clone());
    }
    let selected = fs::read_to_string(root.join("user.yaml")).ok()
        .and_then(|content| serde_yaml::from_str::<serde_yaml::Value>(&content).ok())
        .and_then(|user| user["var"]["previously_selected_schema"].as_str().map(str::to_owned));
    let key = selected.and_then(|schema| records.iter().find(|(_, record)| record.manifest.contains_key(&format!("{schema}.schema.yaml"))).map(|(key, _)| key.clone()))
        .or_else(|| core.state().active_scheme.filter(|key| records.contains_key(key)))
        .or_else(|| records.keys().next().cloned());
    if let Some(key) = key {
        marker.scheme_key = key.clone();
        marker.rime_lua = records[&key].rime_lua.clone();
    }
    let bytes = serde_json::to_vec_pretty(&marker)?;
    crate::scheme_files::write_file_atomic(&path, &bytes)?;
    crate::scheme_files::write_file_atomic(&PackageStore::new(core).checked_active_manifest_path()?, &bytes)?;
    Ok(())
}

fn refresh_generated(
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
        from_store: false,
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
        engine: Default::default(),
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
    if !receipt.from_store {
        store.write_archive(BufReader::new(receipt.archive.reopen()?), &meta, None)?;
    } else {
        let path = store.checked_metadata_path(scheme_key)?;
        let mut stored: crate::package_store::StoredPackage = serde_json::from_slice(&fs::read(&path)?)?;
        if stored.meta.selection_base_version.take().is_some() {
            crate::scheme_files::write_file_atomic(&path, &serde_json::to_vec_pretty(&stored)?)?;
        }
    }
    let has_owned_activation = receipt.activation.is_some();
    let preserve_selection = active && !receipt.root.is_empty()
        && crate::package_activation::read_package(core, Path::new(&receipt.root), scheme_key)?.is_some();
    if active {
        if !receipt.root.is_empty() {
            let root = Path::new(&receipt.root);
            select_main_schema_after_addon(root)?;
            if let Some(path) = crate::addon::owned_addon_default(root).map_err(CoreError::Other)? {
                crate::package_fs::remove_file(&path)?;
            }
        }
        if let Some(mut activation) = receipt.activation.take() {
            activation.key = scheme_key.into();
            activation.scheme_key = scheme_key.into();
            activation.manifest = receipt.manifest.clone();
            let root = Path::new(&receipt.root);
            for record in crate::package_activation::read_all(core, root)?.values() {
                for backup in &mut activation.backups {
                    if let Some(original) = record.backups.iter().find(|original| crate::package_activation::same_file_path(root, &original.path, &backup.path)) {
                        *backup = original.clone();
                    }
                }
                for backup in &record.backups {
                    if (activation.owned.iter().any(|file| crate::package_activation::same_file_path(root, &file.path, &backup.path))
                        || matches!(backup.path.as_str(), "default.custom.yaml" | "default-custom.yaml" | "rime.lua"))
                        && !activation.backups.iter().any(|current| crate::package_activation::same_file_path(root, &current.path, &backup.path))
                    {
                        activation.backups.push(backup.clone());
                    }
                }
            }
            if let Some(previous) = crate::package_activation::read_package(core, root, scheme_key)? {
                let others: BTreeSet<_> = crate::package_activation::read_all(core, root)?.into_iter()
                    .filter(|(key, _)| key != scheme_key)
                    .flat_map(|(_, record)| record.owned.into_iter().map(|file| file.path))
                    .collect();
                for file in previous.owned.iter().filter(|file| !activation.manifest.keys().any(|path| crate::package_activation::same_file_path(root, path, &file.path))) {
                    if !crate::package_activation::user_file(&file.path)
                        && !others.iter().any(|path| crate::package_activation::same_file_path(root, path, &file.path))
                        && crate::package_activation::fingerprint(root, &file.path)?.is_some_and(|current| current.sha256 == file.sha256)
                    {
                        crate::package_fs::remove_file(&safe_path(root, &file.path).map_err(CoreError::Other)?)?;
                    }
                }
                activation.backups.retain(|backup| !previous.owned.iter().any(|file| crate::package_activation::same_file_path(root, &file.path, &backup.path))
                    && !previous.backups.iter().any(|old| crate::package_activation::same_file_path(root, &old.path, &backup.path)));
                activation.backups.extend(previous.backups);
                activation.kept_backups.extend(previous.kept_backups);
            }
            crate::package_activation::write(core, &activation)?;
        } else if !receipt.root.is_empty() {
            let marker = ActiveManifest {
                root: receipt.root.clone(), scheme_key: scheme_key.into(), manifest: receipt.manifest.clone(),
                rime_lua: rime_lua_from_reader(BufReader::new(receipt.archive.reopen()?)).map_err(CoreError::Other)?,
                ..Default::default()
            };
            let bytes = serde_json::to_vec_pretty(&marker)?;
            crate::scheme_files::write_file_atomic(&safe_path(Path::new(&receipt.root), keytao_core::ACTIVE_SCHEME_MANIFEST).map_err(CoreError::Other)?, &bytes)?;
            crate::scheme_files::write_file_atomic(&store.checked_active_manifest_path()?, &bytes)?;
        }
        if !receipt.root.is_empty() && has_owned_activation {
            let root = Path::new(&receipt.root);
            if let Some(transaction) = receipt.transaction.as_mut() {
                for relative in PackageStore::read_files_at(&store.checked_package_path(scheme_key)?, scheme_key)?.0.into_iter().map(|(path, _)| path).filter(|path| path.starts_with("lua/")) {
                    transaction.snapshot_user_file(&format!(".keytao-package-lua/{scheme_key}/{relative}"))?;
                }
            }
            let records = crate::package_activation::read_all(core, root)?;
            publish_active_manifest(core, root, &records, None)?;
            keytao_core::regenerate_active_rime_lua(root).map_err(CoreError::Other)?;
        }
    }
    #[cfg(target_os = "windows")]
    if publish_public {
        crate::windows_public_schemas::publish_activation(
            core,
            &store.package_path(scheme_key).join("files"),
            None,
            Path::new(&receipt.root),
        ).map_err(CoreError::Other)?;
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
            if !preserve_selection || state.active_scheme.is_none() {
                state.active_scheme = Some(scheme_key.into());
            }
            state.last_deploy = None;
        })?;
    }
    if let Some(transaction) = receipt.transaction.take() {
        transaction.commit()?;
    }
    if scheme_key == crate::rime_snow::SCHEMA_ID && !receipt.root.is_empty() {
        crate::addon_store::finish_snow_takeover(core, Path::new(&receipt.root))?;
    }
    Ok(())
}

fn select_package_primary_schema(core: &Core, key: &str, receipt: &PendingPackage, preserve: bool) -> Result<(), CoreError> {
    let mut archive = zip::ZipArchive::new(BufReader::new(receipt.archive.reopen()?))
        .map_err(|error| CoreError::Other(error.to_string()))?;
    let mut files = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| CoreError::Other(error.to_string()))?;
        if entry.is_dir() { continue; }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        files.push((entry.name().to_owned(), bytes));
    }
    if crate::package_schemas::enumerate(key, &files, None)?.is_empty() { return Ok(()); }
    let previous = match PackageStore::new(core).read_metadata_bytes(key) {
        Ok(bytes) => Some(serde_json::from_slice::<crate::package_store::StoredPackage>(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let selection = previous.as_ref().and_then(|stored| stored.selected_schemas.as_deref())
        .map(|selected| crate::package_schemas::reconcile_selection(key, &files, selected,
            &previous.as_ref().unwrap().files.iter().map(|file| file.path.clone()).collect::<Vec<_>>()))
        .transpose()?;
    let preserve = preserve || (receipt.from_store
        && previous.as_ref().is_some_and(|stored| stored.selected_schemas.is_some())
        && core.state().active_scheme.as_deref() == Some(key));
    let selected = crate::package_schemas::filter_files(key, &mut files, selection.as_deref())?;
    let root = Path::new(&receipt.root);
    let preserve = preserve || crate::package_activation::read_package(core, root, key)?.is_some();
    let addons = crate::package_activation::addon_ownership(core, root)?;
    let owned: Vec<_> = files.iter().filter(|(path, _)| !addons.contains_key(path))
        .filter_map(|(path, _)| path.strip_suffix(".schema.yaml").map(str::to_owned)).collect();
    let (relative, current) = crate::addon::addon_default_custom(root).map_err(CoreError::Other)?;
    let content = crate::addon::update_addon_schemas_content(&current, &owned, &selected).map_err(CoreError::Other)?;
    crate::scheme_files::write_file_atomic(&root.join(relative), content.as_bytes())?;
    let enabled = if preserve { crate::scheme_files::parse_schema_list(&content) } else { selected };
    crate::package_schemas::ensure_active_schema(root, &enabled, preserve)
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
            select_package_primary_schema(core, scheme_key, receipt, updating_active_package)?;
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
            state.active_scheme = Some(scheme_key.into());
            state.last_deploy = None;
        })?;
    }
    if let Some(root) = installed_root {
        crate::user_words::remember_reconciliation(core, scheme_key, &root, true);
    }
    Ok(())
}

pub(super) async fn remove_active_package(
    core: &Core,
    scheme_key: &str,
    host: &impl crate::scheme_host::SchemeHost,
) -> Result<Vec<String>, CoreError> {
    let root = host.user_root().map_err(CoreError::Other)?.canonicalize()?;
    let owned = crate::package_activation::read(core, &root)?
        .is_some_and(|record| !record.key.is_empty() || record.activations.is_some());
    if let Some(activation) = crate::package_activation::read_package(core, &root, scheme_key)?
        .filter(|activation| !activation.key.is_empty())
        .filter(|_| owned)
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
            match crate::package_fs::remove_file(
                &safe_path(&root, marker).map_err(CoreError::Other)?,
            ) {
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
            crate::package_fs::remove_file(&default_build)?;
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
    match crate::package_fs::remove_file(&store_manifest) {
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
    let previous_lua = match fs::read(safe_path(root, "rime.lua").map_err(CoreError::Other)?) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let snapshots: Vec<_> = files.iter().filter(|(path, _)| path.starts_with("lua/"))
        .map(|(path, _)| format!(".keytao-package-lua/{scheme_key}/{path}")).collect();
    let mut paths = crate::package_activation::transaction_paths(root, &[], Some(&activation))?;
    paths.extend(snapshots.iter().cloned());
    let transaction = crate::package_activation::Transaction::begin(core, root, paths)?;
    let logs = crate::package_activation::deactivate(core, root, &mut activation)?;
    for record in crate::package_activation::read_all(core, root)?.values().filter(|record| record.key != scheme_key) {
        let (remaining_files, _) = PackageStore::new(core).read_files(&record.key)?;
        for removed in &activation.owned {
            if record.owned.iter().any(|file| crate::package_activation::same_file_path(root, &file.path, &removed.path))
                && !crate::package_activation::user_file(&removed.path)
                && crate::package_activation::fingerprint(root, &removed.path)?.is_some_and(|current| current.sha256 == removed.sha256)
            {
                if let Some((_, bytes)) = remaining_files.iter().find(|(path, _)| crate::package_activation::same_file_path(root, path, &removed.path)) {
                    crate::scheme_files::write_file_atomic(&safe_path(root, &removed.path).map_err(CoreError::Other)?, bytes)?;
                }
            }
        }
    }
    for relative in snapshots {
        let path = safe_path(root, &relative).map_err(CoreError::Other)?;
        if path.try_exists()? { crate::package_fs::remove_file(&path)?; }
    }
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
    ] {
        let path = safe_path(root, name).map_err(CoreError::Other)?;
        if path.try_exists()? {
            crate::package_fs::remove_file(&path)?;
        }
    }
    let mut inactive = crate::package_activation::Activation {
        root: root.canonicalize()?.to_string_lossy().into_owned(),
        kept_backups: activation.kept_backups,
        ..Default::default()
    };
    refresh_generated(core, root, &mut inactive)?;
    crate::package_activation::remove(core, root, scheme_key, &inactive)?;
    let records = crate::package_activation::read_all(core, root)?;
    publish_active_manifest(core, root, &records, None)?;
    if records.is_empty() {
        let path = root.join(keytao_core::ACTIVE_SCHEME_MANIFEST);
        let mut marker: ActiveManifest = serde_json::from_slice(&fs::read(&path)?)?;
        marker.rime_lua = fs::read_to_string(safe_path(root, "rime.lua").map_err(CoreError::Other)?).ok();
        crate::scheme_files::write_file_atomic(&path, &serde_json::to_vec_pretty(&marker)?)?;
    } else {
        keytao_core::regenerate_active_rime_lua(root).map_err(CoreError::Other)?;
    }
    #[cfg(target_os = "windows")]
    if crate::windows_public_schemas::is_default_user_root(root) {
        crate::windows_public_schemas::remove_package(
            core,
            &PackageStore::new(core).package_path(scheme_key).join("files"),
            root,
        ).map_err(CoreError::Other)?;
    }
    // Deploy preparation must not restore sources from the removed legacy selection.
    core.update_state(|state| {
        if state.active_scheme.as_deref() == Some(scheme_key) {
            state.active_scheme = records.keys().next().cloned();
        }
        state.last_deploy = None;
    })?;
    let deployment = if remaining.is_empty() {
        host.write_reload_stamp(root)
            .map(|_| ())
            .map_err(CoreError::Other)
    } else {
        host.deploy().await.map_err(CoreError::Other)
    };
    if let Err(error) = deployment {
        drop(transaction);
        *core.lock_state() = crate::AppState::load(&core.env().data_dir)?;
        let restored = host.deploy().await;
        if let Some(bytes) = previous_lua {
            crate::scheme_files::write_file_atomic(&safe_path(root, "rime.lua").map_err(CoreError::Other)?, &bytes)?;
        }
        restored.map_err(|restore| CoreError::Other(format!("{error}; restoring the previous deployment failed: {restore}")))?;
        return Err(error);
    }
    if let Err(error) = transaction.commit() {
        *core.lock_state() = crate::AppState::load(&core.env().data_dir)?;
        return Err(error);
    }
    Ok(crate::package_activation::retained(&logs))
}

pub(super) async fn activate_scheme(
    core: &Core,
    scheme_key: &str,
    root: &Path,
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
    let (mut files, meta) = store.read_files(scheme_key)?;
    crate::package_schemas::filter_files(scheme_key, &mut files,
        crate::package_schemas::read_selection(&store.checked_metadata_path(scheme_key)?)?.as_deref())?;
    if meta.scheme_key != scheme_key || meta.version != package.version {
        return Err(CoreError::Other(
            "Stored package metadata is inconsistent; re-download this scheme before activation"
                .into(),
        ));
    }
    let result = install::smart_install_files(
        core,
        files,
        root.to_string_lossy().into_owned(),
        meta.source == "local",
        Some(scheme_key),
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
        receipt.from_store = true;
    }
    crate::scheme::record_installed_scheme(core, scheme_key, package.version.as_deref())?;
    Ok(result)
}

#[cfg(test)]
pub(crate) fn cleanup_manifest(
    root: &Path,
    manifest: &BTreeMap<String, String>,
    incoming: &BTreeSet<String>,
) -> Result<Vec<String>, String> {
    cleanup_manifest_with(root, manifest, incoming, |relative| {
        crate::package_fs::remove_file(&safe_path(root, relative)?)
            .map_err(|error| error.to_string())
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

