use crate::{
    Core, CoreError,
    package_activation::{Activation, OwnedFile},
    package_store::{PackageFile, PackageStore, StoredPackage},
    packages::{PackageMeta, safe_path, sha256_reader},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

pub(crate) const IDS: &[&str] = &["easy_en", "wanxiang", "rime_ice", "rime_mint", "rime_frost"];
const GRAMMAR: &str = "wanxiang-lts-zh-hans.gram";
static STORE_OPERATION: Mutex<()> = Mutex::new(());

fn operation() -> MutexGuard<'static, ()> {
    STORE_OPERATION
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

#[derive(Serialize, Deserialize)]
struct ManifestFile {
    path: String,
    sha256: String,
    size: u64,
}

pub(crate) struct AddonStore {
    id: String,
    root: PathBuf,
    destination: PathBuf,
    staging: PathBuf,
}

impl AddonStore {
    pub(crate) fn new(core: &Core, root: &Path, id: &str) -> Result<Self, String> {
        let _guard = operation();
        Self::new_inner(core, root, id)
    }

    fn new_inner(core: &Core, root: &Path, id: &str) -> Result<Self, String> {
        let store = PackageStore::new(core);
        let destination = store.checked_addon_path(id).map_err(|e| e.to_string())?;
        store.recover_addon(id).map_err(|e| e.to_string())?;
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        let staging = safe_path(
            &store.checked_root().map_err(|e| e.to_string())?,
            ".staging",
        )?;
        Ok(Self {
            id: id.into(),
            root,
            destination,
            staging,
        })
    }

    fn activation(&self) -> Result<Option<Activation>, String> {
        let path = safe_path(&self.destination, "activation.json")?;
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        let active: Activation = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if active.key != self.id || Path::new(&active.root) != self.root {
            return Ok(None);
        }
        for file in active.owned.iter().chain(&active.generated) {
            validate_file(&self.id, &file.path, &file.sha256)?;
            safe_path(&self.root, &file.path)?;
        }
        Ok(Some(active))
    }

    pub(crate) fn read_manifest(&self) -> Result<Option<Vec<u8>>, String> {
        let _guard = operation();
        self.read_manifest_inner()
    }

    fn read_manifest_inner(&self) -> Result<Option<Vec<u8>>, String> {
        let Some(active) = self.activation()? else {
            return Ok(None);
        };
        let stored = read_stored(&self.destination, &self.id).map_err(|e| e.to_string())?;
        let mut manifest = stored.addon_manifest.ok_or("Add-on manifest is missing")?;
        if self.id == "wanxiang" && !active.owned.iter().any(|f| f.path == GRAMMAR) {
            manifest["grammar"] = serde_json::Value::Null;
            manifest["includes_grammar"] = false.into();
        }
        let original: BTreeSet<String> = parse_manifest(&manifest, &self.id)?
            .into_iter()
            .map(|f| f.path)
            .collect();
        let mut files = Vec::new();
        for file in active
            .owned
            .iter()
            .chain(&active.generated)
            .filter(|f| f.path != GRAMMAR && original.contains(&f.path))
        {
            let size = fs::metadata(safe_path(&self.root, &file.path)?)
                .map(|m| m.len())
                .unwrap_or_else(|_| {
                    stored
                        .files
                        .iter()
                        .find(|f| f.path == file.path)
                        .map_or(0, |f| f.size)
                });
            files.push(ManifestFile {
                path: file.path.clone(),
                sha256: file.sha256.clone(),
                size,
            });
        }
        manifest["files"] = serde_json::to_value(files).map_err(|e| e.to_string())?;
        serde_json::to_vec(&manifest)
            .map(Some)
            .map_err(|e| e.to_string())
    }

    pub(crate) fn commit(&self, official_files: &Path, manifest: &[u8]) -> Result<(), String> {
        let _guard = operation();
        self.commit_inner(official_files, manifest, &[])
            .map_err(|e| e.to_string())
    }

    fn commit_inner(
        &self,
        official_files: &Path,
        manifest: &[u8],
        modified: &[String],
    ) -> Result<(), CoreError> {
        let value: serde_json::Value = serde_json::from_slice(manifest)?;
        let manifest_files = parse_manifest(&value, &self.id).map_err(CoreError::Other)?;
        fs::create_dir_all(&self.staging)?;
        let stage = tempfile::tempdir_in(&self.staging)?;
        let files_root = stage.path().join("files");
        fs::create_dir(&files_root)?;
        let mut files = Vec::new();
        let mut names = BTreeSet::new();
        collect_official(official_files, official_files, &mut names).map_err(CoreError::Other)?;
        for relative in names {
            if relative == GRAMMAR {
                continue;
            }
            let source = safe_path(official_files, &relative).map_err(CoreError::Other)?;
            let mut bytes = fs::read(&source)?;
            if self.id == "wanxiang" && relative == "wanxiang.schema.yaml" {
                bytes = String::from_utf8(bytes)
                    .map_err(|e| CoreError::Other(e.to_string()))?
                    .replace("language: wanxiang-lts-zh-hans", "language: \"\"")
                    .into_bytes();
            }
            let target = safe_path(&files_root, &relative).map_err(CoreError::Other)?;
            fs::create_dir_all(target.parent().unwrap())?;
            write_synced(&target, &bytes)?;
            files.push(PackageFile {
                path: relative,
                sha256: sha256_reader(bytes.as_slice())?,
                size: bytes.len() as u64,
            });
        }
        let mut active = Activation {
            key: self.id.clone(),
            root: self.root.to_string_lossy().into_owned(),
            scheme_key: self.id.clone(),
            ..Default::default()
        };
        for file in manifest_files {
            if modified.contains(&file.path) {
                continue;
            }
            let path = safe_path(&self.root, &file.path).map_err(CoreError::Other)?;
            let metadata = fs::symlink_metadata(&path)?;
            if !metadata.is_file() {
                return Err(CoreError::Other("Add-on path is not a file".into()));
            }
            let digest = sha256_reader(fs::File::open(path)?)?;
            if !(self.id == "easy_en" && file.path == "easy_en.custom.yaml")
                && (metadata.len() != file.size || digest != file.sha256)
            {
                return Err(CoreError::Other(format!(
                    "Add-on source changed before commit: {}",
                    file.path
                )));
            }
            let owned = OwnedFile {
                path: file.path.clone(),
                sha256: digest,
                ..Default::default()
            };
            if generated(&file.path) {
                active.generated.push(owned);
            } else {
                active.owned.push(owned);
            }
        }
        if self.id == "easy_en"
            && !modified.iter().any(|p| p == "easy_en.custom.yaml")
            && !active
                .generated
                .iter()
                .any(|f| f.path == "easy_en.custom.yaml")
        {
            if let Some(file) =
                crate::package_activation::fingerprint(&self.root, "easy_en.custom.yaml")?
            {
                active.generated.push(file);
            }
        }
        if self.id == "wanxiang"
            && !modified.iter().any(|p| p == GRAMMAR)
            && value["includes_grammar"].as_bool() == Some(true)
            && value["grammar"].is_object()
        {
            let model = &value["grammar"];
            let path = safe_path(&self.root, GRAMMAR).map_err(CoreError::Other)?;
            let metadata = fs::symlink_metadata(&path)?;
            let expected = model["sha256"]
                .as_str()
                .ok_or_else(|| CoreError::Other("Grammar checksum is missing".into()))?;
            if !metadata.is_file()
                || model["size"].as_u64() != Some(metadata.len())
                || sha256_reader(fs::File::open(&path)?)? != expected
            {
                return Err(CoreError::Other(
                    "Grammar fingerprint mismatch; model retained in place".into(),
                ));
            }
            active.owned.push(OwnedFile {
                path: GRAMMAR.into(),
                sha256: expected.into(),
                move_file: true,
            });
        }
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| CoreError::Other(e.to_string()))?
            .as_secs();
        let stored = StoredPackage {
            key: self.id.clone(),
            origin: value["source"].as_str().map(str::to_owned),
            installed_at: timestamp,
            files,
            variant: value["variant"].as_str().map(str::to_owned),
            grammar: value.get("grammar").cloned().filter(|v| !v.is_null()),
            addon_manifest: Some(value.clone()),
            meta: PackageMeta {
                scheme_key: self.id.clone(),
                version: value["version"].as_str().map(str::to_owned),
                installed_at_unix: timestamp,
                source: value["source"].as_str().unwrap_or_default().into(),
                display_name: None,
                selection_base_version: None,
                manifest: BTreeMap::new(),
                zip_sha256: value["archive_sha256"].as_str().unwrap_or_default().into(),
            },
        };
        write_synced(
            &stage.path().join("package.json"),
            &serde_json::to_vec_pretty(&stored)?,
        )?;
        write_synced(
            &stage.path().join("activation.json"),
            &serde_json::to_vec_pretty(&active)?,
        )?;
        if !modified.is_empty() {
            write_synced(
                &stage.path().join("migration-modified.json"),
                &serde_json::to_vec_pretty(modified)?,
            )?;
        }
        PackageStore::publish_addon_at(&self.destination, &self.id, stage.path())
    }

    pub(crate) fn remove(&self) -> Result<(), String> {
        let _guard = operation();
        PackageStore::recover_addon_at(&self.destination, &self.id).map_err(|e| e.to_string())?;
        if !self.destination.try_exists().map_err(|e| e.to_string())? {
            return Ok(());
        }
        fs::create_dir_all(&self.staging).map_err(|e| e.to_string())?;
        let stage = tempfile::tempdir_in(&self.staging).map_err(|e| e.to_string())?;
        fs::rename(&self.destination, stage.path().join("removed")).map_err(|e| e.to_string())
    }
}

fn generated(path: &str) -> bool {
    path == "easy_en.custom.yaml"
        || path == "keytao_wanxiang_flypy.yaml"
        || path.ends_with("-NOTICE.txt")
}

fn validate_path(id: &str, path: &str) -> Result<(), String> {
    crate::packages::validate_relative(path)?;
    if crate::package_activation::user_file(path) {
        return Err("Add-on manifest contains protected user data".into());
    }
    let allowed = match id {
        "easy_en" => crate::addon::is_package_file(path) || path == "easy_en.custom.yaml",
        "wanxiang" => crate::wanxiang::installable(path) || path == GRAMMAR,
        "rime_ice" => crate::rime_ice::installable(path),
        "rime_mint" => (crate::pinyin_addons::MINT.installable)(path),
        "rime_frost" => (crate::pinyin_addons::FROST.installable)(path),
        _ => false,
    };
    if !allowed {
        return Err(format!("File does not belong to add-on {id}: {path}"));
    }
    Ok(())
}

fn validate_file(id: &str, path: &str, hash: &str) -> Result<(), String> {
    validate_path(id, path)?;
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Invalid add-on file checksum".into());
    }
    Ok(())
}

fn parse_manifest(value: &serde_json::Value, id: &str) -> Result<Vec<ManifestFile>, String> {
    if id == "wanxiang" {
        crate::wanxiang::validate_manifest(&serde_json::to_vec(value).map_err(|e| e.to_string())?)?;
    }
    if value["format"].as_u64() != Some(1)
        || value["version"].as_str().is_none()
        || value["source"].as_str().is_none()
        || !value["archive_sha256"]
            .as_str()
            .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err("Invalid add-on manifest".into());
    }
    let files: Vec<ManifestFile> =
        serde_json::from_value(value["files"].clone()).map_err(|e| e.to_string())?;
    if files.len() > 4096 {
        return Err("Add-on manifest contains too many files".into());
    }
    let mut seen = BTreeSet::new();
    for file in &files {
        validate_file(id, &file.path, &file.sha256)?;
        if !seen.insert(file.path.to_lowercase()) || file.path == GRAMMAR {
            return Err("Duplicate or misplaced add-on file".into());
        }
    }
    Ok(files)
}

fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn collect_official(root: &Path, path: &Path, files: &mut BTreeSet<String>) -> Result<(), String> {
    for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_dir() {
            collect_official(root, &entry.path(), files)?;
        } else if kind.is_file() {
            let relative = entry
                .path()
                .strip_prefix(root)
                .map_err(|e| e.to_string())?
                .to_str()
                .ok_or("Non-UTF-8 add-on file")?
                .replace('\\', "/");
            crate::packages::validate_relative(&relative)?;
            files.insert(relative);
        } else {
            return Err("Add-on package contains a symlink or special file".into());
        }
    }
    Ok(())
}

fn read_stored(path: &Path, id: &str) -> Result<StoredPackage, CoreError> {
    let bytes = fs::read(safe_path(path, "package.json").map_err(CoreError::Other)?)?;
    let stored: StoredPackage = serde_json::from_slice(&bytes)?;
    if stored.key != id || stored.meta.scheme_key != id {
        return Err(CoreError::Other("Add-on package id mismatch".into()));
    }
    let value = stored
        .addon_manifest
        .as_ref()
        .ok_or_else(|| CoreError::Other("Add-on manifest is missing".into()))?;
    parse_manifest(value, id).map_err(CoreError::Other)?;
    Ok(stored)
}

pub(crate) fn validate_directory(path: &Path, id: &str) -> Result<(), CoreError> {
    let stored = read_stored(path, id)?;
    let root = safe_path(path, "files").map_err(CoreError::Other)?;
    let mut expected = BTreeSet::new();
    let mut total = 0u64;
    for file in &stored.files {
        validate_file(id, &file.path, &file.sha256).map_err(CoreError::Other)?;
        if file.path == GRAMMAR || !expected.insert(file.path.clone()) {
            return Err(CoreError::Other("Invalid add-on source inventory".into()));
        }
        let source = safe_path(&root, &file.path).map_err(CoreError::Other)?;
        let metadata = fs::symlink_metadata(&source)?;
        total = total.saturating_add(metadata.len());
        if !metadata.is_file()
            || metadata.len() != file.size
            || total > 512 * 1024 * 1024
            || sha256_reader(fs::File::open(source)?)? != file.sha256
        {
            return Err(CoreError::Other(
                "Add-on source fingerprint mismatch".into(),
            ));
        }
    }
    let mut actual = BTreeSet::new();
    collect_official(&root, &root, &mut actual).map_err(CoreError::Other)?;
    if actual != expected {
        return Err(CoreError::Other("Add-on source inventory mismatch".into()));
    }
    let active: Activation = serde_json::from_slice(&fs::read(
        safe_path(path, "activation.json").map_err(CoreError::Other)?,
    )?)?;
    if active.key != id {
        return Err(CoreError::Other("Add-on activation id mismatch".into()));
    }
    for file in active.owned.iter().chain(&active.generated) {
        validate_file(id, &file.path, &file.sha256).map_err(CoreError::Other)?;
    }
    Ok(())
}

pub(crate) fn manifest(core: &Core, root: &Path, id: &str) -> Result<Option<Vec<u8>>, String> {
    let _guard = operation();
    if !root.try_exists().map_err(|e| e.to_string())? {
        return Ok(None);
    }
    migrate_inner(core, root).map_err(|e| e.to_string())?;
    AddonStore::new_inner(core, root, id)?.read_manifest_inner()
}

pub(crate) fn ownership(core: &Core, root: &Path) -> Result<BTreeMap<String, String>, CoreError> {
    let _guard = operation();
    if !root.try_exists()? {
        return Ok(BTreeMap::new());
    }
    migrate_inner(core, root)?;
    let mut owned = BTreeMap::new();
    for id in IDS {
        let store = AddonStore::new_inner(core, root, id).map_err(CoreError::Other)?;
        if let Some(active) = store.activation().map_err(CoreError::Other)? {
            for file in active.owned.into_iter().chain(active.generated) {
                if owned.insert(file.path.clone(), (*id).into()).is_some() {
                    return Err(CoreError::Other(format!(
                        "Add-on ownership collision: {}",
                        file.path
                    )));
                }
            }
        }
    }
    Ok(owned)
}

pub(crate) fn report_paths(core: &Core, root: &Path) -> Result<BTreeSet<String>, CoreError> {
    let _guard = operation();
    if !root.try_exists()? {
        return Ok(BTreeSet::new());
    }
    migrate_inner(core, root)?;
    let mut paths = BTreeSet::new();
    for id in IDS {
        let addon = AddonStore::new_inner(core, root, id).map_err(CoreError::Other)?;
        let Some(active) = addon.activation().map_err(CoreError::Other)? else {
            continue;
        };
        paths.extend(
            active
                .owned
                .into_iter()
                .chain(active.generated)
                .map(|f| f.path),
        );
        let stored = read_stored(&addon.destination, id)?;
        if let Some(manifest) = stored.addon_manifest {
            paths.extend(
                parse_manifest(&manifest, id)
                    .map_err(CoreError::Other)?
                    .into_iter()
                    .map(|f| f.path),
            );
        }
        let report =
            safe_path(&addon.destination, "migration-modified.json").map_err(CoreError::Other)?;
        match fs::read(report) {
            Ok(bytes) => {
                let modified: Vec<String> = serde_json::from_slice(&bytes)?;
                for path in modified {
                    validate_path(id, &path).map_err(CoreError::Other)?;
                    paths.insert(path);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(paths)
}

fn files_path(core: &Core, id: &str) -> Result<PathBuf, String> {
    let store = PackageStore::new(core);
    store.recover_addon(id).map_err(|e| e.to_string())?;
    let path = store.checked_addon_path(id).map_err(|e| e.to_string())?;
    validate_directory(&path, id).map_err(|e| e.to_string())?;
    safe_path(&path, "files")
}

pub(crate) fn public_files(core: &Core, id: &str) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let _guard = operation();
    let root = files_path(core, id)?;
    let mut files = BTreeSet::new();
    collect_official(&root, &root, &mut files)?;
    files
        .into_iter()
        .map(|relative| Ok((safe_path(&root, &relative)?, PathBuf::from(relative))))
        .collect()
}

pub(crate) fn migrate(core: &Core, root: &Path) -> Result<(), CoreError> {
    let _guard = operation();
    migrate_inner(core, root)
}

fn migrate_inner(core: &Core, root: &Path) -> Result<(), CoreError> {
    if !root.try_exists()? {
        return Ok(());
    }
    let store = PackageStore::new(core);
    let addons = safe_path(&store.checked_root()?, "addons").map_err(CoreError::Other)?;
    let marker = safe_path(&addons, ".layout-v1").map_err(CoreError::Other)?;
    for id in IDS {
        store.recover_addon(id)?;
    }
    if marker.try_exists()? {
        return Ok(());
    }
    fs::create_dir_all(&addons)?;
    let mut claimed = BTreeSet::new();
    if let Some(active) = crate::package_activation::read(core, root)? {
        claimed.extend(
            active
                .owned
                .into_iter()
                .chain(active.generated)
                .map(|f| f.path),
        );
    }
    for id in IDS {
        if let Some(active) = AddonStore::new_inner(core, root, id)
            .map_err(CoreError::Other)?
            .activation()
            .map_err(CoreError::Other)?
        {
            claimed.extend(
                active
                    .owned
                    .into_iter()
                    .chain(active.generated)
                    .map(|f| f.path),
            );
        }
    }
    let mut complete = true;
    for (id, name) in IDS.iter().zip([
        ".keytao-easy-en.json",
        ".keytao-wanxiang.json",
        ".keytao-rime-ice.json",
        ".keytao-rime-mint.json",
        ".keytao-rime-frost.json",
    ]) {
        let legacy = safe_path(root, name).map_err(CoreError::Other)?;
        if fs::metadata(&legacy).is_ok_and(|m| m.len() > 1024 * 1024) {
            tracing::warn!(
                addon = *id,
                "Legacy add-on manifest is too large; retained for retry"
            );
            complete = false;
            continue;
        }
        let bytes = match fs::read(&legacy) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if *id == "easy_en" {
                    crate::addon::legacy_manifest(root).map_err(CoreError::Other)?
                } else {
                    None
                }
            }
            Err(error) => return Err(error.into()),
        };
        let Some(bytes) = bytes else { continue };
        let addon = AddonStore::new_inner(core, root, id).map_err(CoreError::Other)?;
        if addon
            .read_manifest_inner()
            .map_err(CoreError::Other)?
            .is_none()
        {
            let parsed = serde_json::from_slice::<serde_json::Value>(&bytes)
                .map_err(|e| e.to_string())
                .and_then(|value| parse_manifest(&value, id).map(|files| (value, files)));
            let (value, mut files) = match parsed {
                Ok(parsed) => parsed,
                Err(error) => {
                    tracing::warn!(addon = *id, %error, "Invalid legacy add-on manifest retained for retry");
                    complete = false;
                    continue;
                }
            };
            if *id == "wanxiang"
                && value["includes_grammar"].as_bool() == Some(true)
                && value["grammar"].is_object()
            {
                files.push(ManifestFile {
                    path: GRAMMAR.into(),
                    sha256: value["grammar"]["sha256"].as_str().unwrap().into(),
                    size: value["grammar"]["size"].as_u64().unwrap(),
                });
            }
            let stage = store.addon_stage()?;
            let mut modified = Vec::new();
            for file in files {
                if claimed.contains(&file.path) {
                    modified.push(file.path);
                    continue;
                }
                let source = safe_path(root, &file.path).map_err(CoreError::Other)?;
                let matching = match fs::symlink_metadata(&source) {
                    Ok(metadata) => {
                        metadata.is_file()
                            && metadata.len() == file.size
                            && sha256_reader(fs::File::open(&source)?)? == file.sha256
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                    Err(error) => return Err(error.into()),
                };
                if !matching {
                    modified.push(file.path);
                    continue;
                }
                if file.path == GRAMMAR {
                    continue;
                }
                let target = safe_path(stage.path(), &file.path).map_err(CoreError::Other)?;
                fs::create_dir_all(target.parent().unwrap())?;
                fs::copy(source, target)?;
            }
            if *id == "easy_en" && claimed.contains("easy_en.custom.yaml") {
                modified.push("easy_en.custom.yaml".into());
            }
            if *id == "easy_en" {
                write_synced(
                    &stage.path().join("easy_en.custom.yaml"),
                    &crate::addon::official_custom().map_err(CoreError::Other)?,
                )?;
            }
            if *id == "wanxiang" && value["variant"].as_str() == Some("flypy") {
                write_synced(
                    &stage.path().join("keytao_wanxiang_flypy.yaml"),
                    crate::wanxiang::FLYPY_CUSTOM.as_bytes(),
                )?;
            }
            addon.commit_inner(stage.path(), &bytes, &modified)?;
            if let Some(active) = addon.activation().map_err(CoreError::Other)? {
                claimed.extend(
                    active
                        .owned
                        .into_iter()
                        .chain(active.generated)
                        .map(|f| f.path),
                );
            }
        }
        validate_directory(&addon.destination, id)?;
        match fs::remove_file(&legacy) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    if AddonStore::new_inner(core, root, "easy_en")
        .map_err(CoreError::Other)?
        .activation()
        .map_err(CoreError::Other)?
        .is_some()
    {
        let cache = safe_path(&core.env().data_dir, "easy-en-public").map_err(CoreError::Other)?;
        if cache.try_exists()? {
            fs::remove_dir_all(cache)?;
        }
    }
    if complete {
        crate::scheme_files::write_file_atomic(&marker, b"1\n")?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "addon_store_tests.rs"]
mod tests;
