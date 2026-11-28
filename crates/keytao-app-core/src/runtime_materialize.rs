use crate::{
    Core, CoreError,
    addon::EASY_EN_ADDON_ID,
    managed_custom,
    package_store::{PackageStore, StoredPackage},
    packages::{safe_path, sha256_reader},
};
use keytao_core::{
    RuntimeDirs,
    isolated_deploy::{
        PackageDeployTarget, STAGED_INPUTS_MANIFEST, STAGED_INPUTS_PREFIX, StagedInput,
        preserves_runtime_data,
    },
    runtime_log::AppOperation,
};
use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, BufRead, BufReader, ErrorKind, Read, Write},
    path::{Path, PathBuf},
};

const MANIFEST: &str = RuntimeDirs::MANIFEST_FILE;
const PENDING: &str = ".keytao-runtime.pending.json";
const COPY_STAGE: &str = ".keytao-materialize-copy";
const DEFAULT_CUSTOM: [&str; 2] = ["default.custom.yaml", "default-custom.yaml"];
const COMMON: [&str; 6] = [
    "default.yaml",
    "key_bindings.yaml",
    "punctuation.yaml",
    "symbols.yaml",
    "essay.txt",
    "opencc",
];

type Files = BTreeMap<String, Vec<u8>>;
type RuntimeFiles = BTreeMap<String, RuntimeFile>;

#[derive(Clone)]
enum RuntimeFile {
    Bytes(Vec<u8>),
    Disk {
        path: PathBuf,
        size: u64,
        sha256: String,
    },
}

impl RuntimeFile {
    fn disk(path: &Path) -> Result<Self, CoreError> {
        let file = fs::File::open(path)?;
        let mut hash = Sha256::new();
        let size = io::copy(&mut BufReader::new(file), &mut hash)?;
        Ok(Self::Disk {
            path: path.to_owned(),
            size,
            sha256: format!("{:x}", hash.finalize()),
        })
    }

    fn sha256(&self) -> Result<Cow<'_, str>, CoreError> {
        match self {
            Self::Bytes(bytes) => Ok(Cow::Owned(sha256_reader(bytes.as_slice())?)),
            Self::Disk { sha256, .. } => Ok(Cow::Borrowed(sha256)),
        }
    }

    fn read(&self) -> Result<Cow<'_, [u8]>, CoreError> {
        match self {
            Self::Bytes(bytes) => Ok(Cow::Borrowed(bytes)),
            source => {
                let mut bytes = Vec::new();
                source.copy_to(&mut bytes)?;
                Ok(Cow::Owned(bytes))
            }
        }
    }

    fn dictionary_header(&self) -> Result<Cow<'_, [u8]>, CoreError> {
        let Self::Disk { path, size, sha256 } = self else {
            return self.read();
        };
        let mut input = BufReader::new(fs::File::open(path)?.take(size.saturating_add(1)));
        let mut hash = Sha256::new();
        let mut header = Vec::new();
        let mut line = String::new();
        let mut length = 0;
        loop {
            let read = input.read_line(&mut line)?;
            if read == 0 {
                break;
            }
            hash.update(line.as_bytes());
            length += read as u64;
            header.extend_from_slice(line.as_bytes());
            if line.trim() == "..." {
                break;
            }
            line.clear();
        }
        let mut buffer = [0u8; 16 * 1024];
        let mut carry = 0;
        loop {
            let read = match input.read(&mut buffer[carry..]) {
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                result => result?,
            };
            if read == 0 {
                utf8(&buffer[..carry])?;
                break;
            }
            let end = carry + read;
            hash.update(&buffer[carry..end]);
            length += read as u64;
            carry = match std::str::from_utf8(&buffer[..end]) {
                Ok(_) => 0,
                Err(error) if error.error_len().is_none() => {
                    let start = error.valid_up_to();
                    buffer.copy_within(start..end, 0);
                    end - start
                }
                Err(error) => return Err(CoreError::Other(error.to_string())),
            };
        }
        Self::check_digest(path, *size, sha256, length, hash)?;
        Ok(Cow::Owned(header))
    }

    fn copy_to(&self, output: &mut impl Write) -> Result<(), CoreError> {
        match self {
            Self::Bytes(bytes) => output.write_all(bytes)?,
            Self::Disk { path, size, sha256 } => {
                let mut input = BufReader::new(fs::File::open(path)?.take(size.saturating_add(1)));
                let mut hash = Sha256::new();
                let mut length = 0;
                loop {
                    let bytes = input.fill_buf()?;
                    if bytes.is_empty() {
                        break;
                    }
                    hash.update(bytes);
                    output.write_all(bytes)?;
                    let read = bytes.len();
                    length += read as u64;
                    input.consume(read);
                }
                Self::check_digest(path, *size, sha256, length, hash)?;
            }
        }
        Ok(())
    }

    fn check_digest(
        path: &Path,
        size: u64,
        sha256: &str,
        length: u64,
        hash: Sha256,
    ) -> Result<(), CoreError> {
        if length != size || format!("{:x}", hash.finalize()) != sha256 {
            return Err(CoreError::Other(format!(
                "Runtime source fingerprint mismatch: {}",
                path.display()
            )));
        }
        Ok(())
    }

    fn write_staged(&self, path: &Path) -> Result<(), CoreError> {
        let parent = path.parent().unwrap();
        fs::create_dir_all(parent)?;

        #[cfg(target_os = "macos")]
        if let Self::Disk { path: source, size, sha256 } = self {
            use std::{
                ffi::CString,
                os::{
                    fd::AsRawFd,
                    unix::{
                        ffi::OsStrExt,
                        fs::{OpenOptionsExt, PermissionsExt},
                    },
                },
            };
            const CLONE_NOOWNERCOPY: u32 = 0x0002;

            let input = fs::File::options()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(source)?;
            if !input.metadata()?.is_file() {
                return Err(io::Error::other("Runtime source is not a regular file").into());
            }
            let directory = fs::File::open(parent)?;
            let name = CString::new(path.file_name().unwrap().as_bytes())
                .map_err(|_| io::Error::from(ErrorKind::InvalidInput))?;
            // The destination is a fresh name in the caller's private staging directory.
            if unsafe {
                libc::fclonefileat(
                    input.as_raw_fd(),
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    CLONE_NOOWNERCOPY,
                )
            } == 0
            {
                let cloned = fs::File::open(path)?;
                let mut hash = Sha256::new();
                let length = io::copy(
                    &mut BufReader::new(&cloned).take(size.saturating_add(1)),
                    &mut hash,
                )?;
                Self::check_digest(source, *size, sha256, length, hash)?;
                cloned.set_permissions(fs::Permissions::from_mode(0o600))?;
                fs::File::options().write(true).open(path)?.sync_all()?;
                return Ok(());
            }
        }

        let mut options = fs::File::options();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut staged = options.open(path)?;
        self.copy_to(&mut staged)?;
        staged.sync_all()?;
        Ok(())
    }
}

fn verified_sources(
    stored: StoredPackage,
    paths: Vec<PathBuf>,
) -> Result<(RuntimeFiles, Option<Vec<String>>), CoreError> {
    let files = stored.files.into_iter().zip(paths).map(|(file, path)| {
        PackageStore::verify_file(&path, &file)?;
        Ok((file.path, RuntimeFile::Disk {
            path,
            size: file.size,
            sha256: file.sha256,
        }))
    }).collect::<Result<_, CoreError>>()?;
    Ok((files, stored.selected_schemas))
}

#[derive(Default, Deserialize, Serialize)]
pub(crate) struct Manifest {
    package: String,
    pub(crate) owned: BTreeMap<String, String>,
    pub(crate) generated: BTreeMap<String, String>,
    pub(crate) user_owned: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    user_default: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    runtime_overrides: BTreeSet<String>,
}

#[derive(Deserialize, Serialize)]
struct Pending {
    previous: Manifest,
    next: Manifest,
}

impl Manifest {
    fn fingerprint(&self, path: &str) -> Option<&String> {
        self.generated
            .get(path)
            .or_else(|| self.owned.get(path))
            .or_else(|| self.user_owned.get(path))
    }

    fn validate(&self, key: &str) -> Result<(), CoreError> {
        if (!self.package.is_empty() && self.package != key)
            || self
                .owned
                .keys()
                .chain(self.generated.keys())
                .chain(self.user_owned.keys())
                .any(|path| !materialized_path(path))
            || self
                .runtime_overrides
                .iter()
                .any(|path| !self.user_owned.contains_key(path))
        {
            return Err(CoreError::Other("Invalid runtime manifest".into()));
        }
        Ok(())
    }
}

#[derive(Default, Deserialize)]
struct Settings {
    #[serde(default)]
    easy_english_packages: BTreeSet<String>,
}

fn read_settings(root: &Path) -> Result<Settings, CoreError> {
    Ok(read_optional(&safe(root, RuntimeDirs::SETTINGS_FILE)?)?
        .map(|bytes| serde_json::from_slice(&bytes))
        .transpose()?
        .unwrap_or_default())
}

pub fn easy_english_enabled(root: &Path, key: &str) -> Result<bool, CoreError> {
    Ok(read_settings(root)?.easy_english_packages.contains(key))
}

pub(crate) fn installed_easy_english_packages(
    core: &Core,
    root: &Path,
) -> Result<Vec<String>, CoreError> {
    let enabled = read_settings(root)?.easy_english_packages;
    if enabled.is_empty() {
        return Ok(Vec::new());
    }
    let shared = crate::deploy::deploy_shared_data_dir(core, root);
    let installed = isolated_package_keys(core, root, &shared)?;
    Ok(enabled
        .into_iter()
        .filter(|key| {
            installed.contains(key)
                || (key == "builtin/keytao"
                    && shared.join("default.yaml").is_file()
                    && RuntimeDirs::isolated_user_data_dir(root, key)
                        .is_some_and(|path| path.is_dir()))
        })
        .collect())
}

pub(crate) fn update_easy_english(root: &Path, key: &str, enabled: bool) -> Result<bool, CoreError> {
    let path = safe(root, RuntimeDirs::SETTINGS_FILE)?;
    let mut settings: serde_json::Map<String, serde_json::Value> = read_optional(&path)?
        .as_deref()
        .map(serde_json::from_slice)
        .transpose()?
        .unwrap_or_default();
    let mut packages: BTreeSet<String> = settings
        .get("easy_english_packages")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();
    let changed = if enabled {
        packages.insert(key.into())
    } else {
        packages.remove(key)
    };
    if changed {
        settings.insert("easy_english_packages".into(), serde_json::to_value(packages)?);
        write_file(&path, &serde_json::to_vec(&settings)?)?;
    }
    Ok(changed)
}

pub fn isolated_packages(
    core: &Core,
    root: &Path,
) -> Result<Vec<keytao_core::switcher_model::Package>, CoreError> {
    let shared = crate::deploy::deploy_shared_data_dir(core, root);
    let installed: BTreeMap<_, _> = isolated_package_keys(core, root, &shared)?
        .into_iter()
        .map(|key| {
            let name = package_name(core, &key);
            (key, name)
        })
        .collect();
    let list = keytao_core::switcher_model::packages_in_store(
        root,
        &shared,
        Some(&core.env().data_dir),
        installed,
    );
    #[cfg(any(target_os = "android", target_os = "ios"))]
    publish_ime_package_catalog_entries(root, &list)?;
    Ok(list)
}

pub(crate) fn isolated_package_keys(
    core: &Core,
    root: &Path,
    shared: &Path,
) -> Result<BTreeSet<String>, CoreError> {
    let store = PackageStore::new(core);
    let mut keys: BTreeSet<_> = core.lock_state().installed_packages.keys().cloned().collect();
    keys.extend(
        installed_addons(&store)?
            .into_iter()
            .filter(|id| *id != EASY_EN_ADDON_ID)
            .map(str::to_owned),
    );
    if let Some(active) =
        RuntimeDirs::active_package_key(root).filter(|key| key.starts_with("builtin/"))
    {
        keys.insert(active);
    } else if keys.is_empty() && shared.join("default.yaml").is_file() {
        keys.insert("builtin/keytao".into());
    }
    Ok(keys)
}

#[cfg(any(target_os = "android", target_os = "ios"))]
fn publish_ime_package_catalog(
    core: &Core,
    root: &Path,
    shared: &Path,
    packages: &BTreeMap<String, String>,
) -> Result<(), CoreError> {
    let list = keytao_core::switcher_model::packages_in_store(
        root,
        shared,
        Some(&core.env().data_dir),
        packages.clone(),
    );
    publish_ime_package_catalog_entries(root, &list)
}

#[cfg(any(target_os = "android", target_os = "ios"))]
fn publish_ime_package_catalog_entries(
    root: &Path,
    packages: &[keytao_core::switcher_model::Package],
) -> Result<(), CoreError> {
    // The keyboard can access the shared control root, but not the app's package store.
    let relative = keytao_core::switcher_model::MOBILE_PACKAGE_CATALOG;
    let catalog: BTreeMap<_, _> = packages
        .iter()
        .map(|pkg| {
            (
                pkg.package_key.clone(),
                keytao_core::switcher_model::CatalogEntry {
                    display_name: pkg.display_name.clone(),
                    schemas: pkg.schemas.clone(),
                },
            )
        })
        .collect();
    let bytes = serde_json::to_vec(&catalog)?;
    if read_optional(&safe(root, relative)?)?.as_deref() != Some(bytes.as_slice()) {
        write_file(&safe(root, relative)?, &bytes)?;
    }
    Ok(())
}

pub fn materialize_all_installed_packages(
    core: &Core,
    control_root: &Path,
    bundled: &Path,
) -> Result<(), CoreError> {
    for target in prepare_targets(core, control_root, bundled, false, None)?.0 {
        if let Some(error) = target.materialize_error {
            return Err(CoreError::Other(error));
        }
    }
    Ok(())
}

#[cfg(target_os = "ios")]
pub(crate) fn materialize_installed_package(
    core: &Core,
    root: &Path,
    bundled: &Path,
    key: &str,
) -> Result<(), CoreError> {
    for target in prepare_targets(core, root, bundled, false, Some(key))?.0 {
        if let Some(error) = target.materialize_error {
            return Err(CoreError::Other(error));
        }
    }
    Ok(())
}

/// Prepare isolated librime inputs for the installed packages when isolation is enabled.
/// `bundled` is the platform's resolved bundled rime-data directory.
pub(crate) fn prepare_deploy_targets(
    core: &Core,
    control_root: &Path,
    bundled: &Path,
    package_key: Option<&str>,
) -> Result<(Vec<PackageDeployTarget>, Vec<tempfile::TempDir>), CoreError> {
    prepare_targets(core, control_root, bundled, true, package_key)
}

fn prepare_targets(
    core: &Core,
    control_root: &Path,
    bundled: &Path,
    stage: bool,
    package_key: Option<&str>,
) -> Result<(Vec<PackageDeployTarget>, Vec<tempfile::TempDir>), CoreError> {
    let _operation = AppOperation::start("materialize_runtime");
    let settings = read_settings(control_root)?;
    let store = PackageStore::new(core);
    let addons: BTreeSet<_> = installed_addons(&store)?
        .into_iter()
        .filter(|id| *id != EASY_EN_ADDON_ID)
        .map(str::to_owned)
        .collect();
    let load_addon = |id: &str| -> Result<_, CoreError> {
        let package = store.checked_addon_path(id)?;
        let (stored, paths) = crate::addon_store::file_inventory(&package, id)?;
        verified_sources(stored, paths)
    };
    let english = std::cell::OnceCell::new();
    let overlay_for = |key: &str| {
        if !settings.easy_english_packages.contains(key) {
            return Ok(None);
        }
        english
            .get_or_init(|| load_addon(EASY_EN_ADDON_ID).map(|(files, _)| files))
            .as_ref()
            .map(Some)
            .map_err(|error| CoreError::Other(error.to_string()))
    };
    let installed = installed_main_packages(core);
    let active = RuntimeDirs::active_package_key(control_root)
        .filter(|key| key != EASY_EN_ADDON_ID);
    let mut keys = installed
        .union(&addons)
        .filter(|key| key.as_str() != EASY_EN_ADDON_ID)
        .cloned()
        .collect::<Vec<_>>();
    if let Some(active) = &active {
        if !keys.contains(active) {
            keys.push(active.clone());
        }
        keys.sort_by_key(|key| key != active);
    }
    if keys.is_empty() && bundled.join("default.yaml").is_file() {
        keys.push("builtin/keytao".into());
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    publish_ime_package_catalog(
        core,
        control_root,
        bundled,
        &keys.iter().map(|key| (key.clone(), package_name(core, key))).collect(),
    )?;
    if let Some(key) = package_key {
        keys.retain(|candidate| candidate == key);
        if key == "builtin/keytao" && keys.is_empty() && bundled.join("default.yaml").is_file() {
            keys.push(key.into());
        }
    } else if stage {
        use keytao_core::engine_choice::{self, PackageEngine};
        keys.retain(|candidate| match engine_choice::read(control_root, Some(candidate)) {
            PackageEngine::Rime => true,
            PackageEngine::Libjd => false,
            PackageEngine::LibjdEngine => !crate::libjd::imported_package_ready(core, candidate),
        });
    }
    let mut stages = Vec::new();
    let targets = keys
        .into_iter()
        .map(|key| {
            let name = package_name(core, &key);
            let result: Result<Option<String>, CoreError> = (|| {
                let (files, selection) = if key.starts_with("builtin/") {
                    let mut files = RuntimeFiles::new();
                    walk_files(bundled, "", private_path, &mut |source, name| {
                        files.insert(name.to_owned(), RuntimeFile::disk(source)?);
                        Ok(())
                    })?;
                    (files, None)
                } else if installed.contains(&key) {
                    let _guard = core.pending_package.lock()
                        .unwrap_or_else(|error| error.into_inner());
                    let (stored, paths) = PackageStore::file_inventory_at(
                        &store.checked_package_path(&key)?, &key, false,
                    )?;
                    verified_sources(stored, paths)?
                } else if addons.contains(&key) {
                    load_addon(&key)?
                } else {
                    return Err(CoreError::Other(format!(
                        "Missing package source for {key}"
                    )));
                };
                #[cfg(any(target_os = "android", target_os = "ios", target_os = "macos", target_os = "linux"))]
                if core.env().data_dir != control_root && !key.starts_with("builtin/") {
                    let (namespace, package) = if installed.contains(&key) {
                        let namespace = if crate::scheme::is_local_scheme_key(&key) {
                            "local"
                        } else {
                            "cloud"
                        };
                        (namespace, store.checked_package_path(&key)?)
                    } else {
                        ("addons", store.checked_addon_path(&key)?)
                    };
                    let source = package.join("files");
                    if RuntimeDirs::for_package(control_root, bundled, &key)
                        .is_none_or(|dirs| dirs.shared_data_dir != source)
                    {
                        publish_package_sources(control_root, namespace, &key, &name, &files)?;
                    }
                }
                let prepared = prepare_runtime(
                    control_root,
                    bundled,
                    &key,
                    files,
                    selection.as_deref(),
                    overlay_for(&key)?,
                    stage,
                    core.env().platform == crate::Platform::Windows,
                )?;
                Ok(prepared.map(|prepared| {
                    let name = prepared
                        .path()
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned();
                    stages.push(prepared);
                    name
                }))
            })();
            let (staged_inputs, materialize_error) = match result {
                Ok(name) => (name, None),
                Err(error) => (None, Some(error.to_string())),
            };
            PackageDeployTarget {
                package_key: key,
                package_name: name,
                materialize_error,
                staged_inputs,
            }
        })
        .collect();
    Ok((targets, stages))
}

pub(crate) fn installed_main_packages(core: &Core) -> BTreeSet<String> {
    core.lock_state()
        .installed_packages
        .keys()
        .cloned()
        .collect()
}

pub(crate) fn installed_addons(store: &PackageStore<'_>) -> Result<Vec<&'static str>, CoreError> {
    let mut addons = Vec::new();
    for &id in crate::addon_store::IDS {
        if store.checked_addon_path(id)?.try_exists()? {
            addons.push(id);
        }
    }
    Ok(addons)
}

#[cfg(any(target_os = "android", target_os = "ios", target_os = "macos", target_os = "linux"))]
fn publish_package_sources(
    root: &Path,
    namespace: &str,
    key: &str,
    display_name: &str,
    files: &RuntimeFiles,
) -> Result<(), CoreError> {
    let destination = safe(root, &format!("scheme-packages/{namespace}/{key}/files"))?;
    let parent = destination.parent().unwrap();
    let previous = safe(root, &format!("scheme-packages/{namespace}/{key}/.old-files"))?;
    let owner = safe(root, &format!("scheme-packages/{namespace}/{key}/.keytao-source-projection"))?;
    if (destination.try_exists()? || previous.try_exists()?)
        && read_optional(&owner)?.as_deref() != Some(b"1\n")
    {
        return Err(CoreError::Other(format!(
            "方案包来源目录已存在：{display_name}"
        )));
    }
    write_file(&owner, b"1\n")?;
    if previous.try_exists()? {
        if destination.try_exists()? {
            fs::remove_dir_all(&previous)?;
        } else {
            fs::rename(&previous, &destination)?;
        }
    }
    if destination.is_dir() {
        let mut actual = BTreeSet::new();
        if crate::package_store::collect_files(&destination, &destination, &mut actual, false).is_ok()
            && actual.iter().eq(files.keys())
            && files.iter().all(|(name, expected)| {
                safe(&destination, name)
                    .ok()
                    .and_then(|path| optional_sha256(&path).ok().flatten())
                    .is_some_and(|digest| expected.sha256().is_ok_and(|sha256| digest == sha256))
            })
        {
            return Ok(());
        }
    }
    let stage = tempfile::Builder::new().prefix(".sources-").tempdir_in(parent)?;
    for (relative, source) in files {
        source.write_staged(&safe(stage.path(), relative)?)?;
    }
    crate::package_store::publish_directory(&destination, stage.path(), crate::package_fs::rename)
}

pub(crate) fn in_use_package(
    root: &Path,
    activations: &BTreeMap<String, crate::package_activation::Activation>,
) -> Option<String> {
    let schema = keytao_core::current_schema_id_from_dir(root)?;
    let source = format!("{schema}.schema.yaml");
    activations
        .iter()
        .find_map(|(key, record)| record.manifest.contains_key(&source).then(|| key.clone()))
}

pub(crate) fn package_name(core: &Core, key: &str) -> String {
    let key = key.strip_prefix("builtin/").unwrap_or(key);
    PackageStore::new(core)
        .read_meta(key)
        .ok()
        .flatten()
        .and_then(|meta| meta.display_name)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| keytao_core::switcher_model::default_package_name(key).into())
}

fn safe(root: &Path, path: &str) -> Result<PathBuf, CoreError> {
    safe_path(root, path).map_err(CoreError::Other)
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, CoreError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn user_path(path: &str) -> bool {
    path != crate::account::USER_DICTIONARY
        && (path.ends_with(".custom.yaml")
        || path.ends_with(".dict.yaml")
        || path.rsplit('/').next() == Some("custom_phrase.txt")
        || path == DEFAULT_CUSTOM[1])
}

pub(crate) fn legacy_sync_dir(root: &Path) -> Result<PathBuf, CoreError> {
    let settings = crate::rime_sync::read_settings(root).map_err(CoreError::Other)?;
    let path = if settings.sync_dir.is_empty() {
        root.join("sync")
    } else {
        root.join(settings.sync_dir)
    };
    match path.canonicalize() {
        Ok(sync) => {
            let canonical_root = root.canonicalize()?;
            Ok(sync.strip_prefix(canonical_root).map_or_else(|_| sync.clone(), |suffix| root.join(suffix)))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let mut normalized = PathBuf::new();
            for part in path.components() {
                if part == std::path::Component::ParentDir {
                    normalized.pop();
                } else {
                    normalized.push(part);
                }
            }
            Ok(normalized)
        }
        Err(error) => Err(error.into()),
    }
}

fn private_path(path: &str) -> bool {
    path.split('/').any(|part| {
        part.starts_with('.')
            || part.contains(".userdb")
            || part.ends_with(".leveldb")
            || matches!(
                part,
                "build"
                    | "sync"
                    | "log"
                    | "runtime"
                    | "prebuilt"
                    | "scheme-packages"
                    | "rime-data"
                    | "shared"
            )
    })
}

#[cfg(test)]
fn user_files(
    root: &Path,
    owners: &BTreeMap<String, BTreeSet<String>>,
    sync_dir: &Path,
) -> Result<Files, CoreError> {
    let mut result = Files::new();
    let skip = |path: &str| private_path(path) || root.join(path).starts_with(sync_dir);
    walk_files(root, "", skip, &mut |path, relative| {
        if user_path(relative) {
            let bytes = strip_generated(relative, fs::read(path)?)?;
            let digest = sha256_reader(bytes.as_slice())?;
            if !bytes.is_empty()
                && !owners
                    .get(relative)
                    .is_some_and(|hashes| hashes.contains(&digest))
            {
                result.insert(relative.to_owned(), bytes);
            }
        }
        Ok(())
    })?;
    Ok(result)
}

#[cfg(test)]
fn restore_user_default(
    current: &[u8],
    applied: &serde_yaml::Value,
    original: Option<&[u8]>,
) -> Result<Vec<u8>, CoreError> {
    use serde_yaml::{Mapping, Value};

    let parse = |bytes| {
        match serde_yaml::from_slice(bytes).map_err(|error| CoreError::Other(error.to_string()))? {
            Value::Mapping(mapping) => Ok(mapping),
            Value::Null => Ok(Mapping::new()),
            _ => Err(CoreError::Other("Default custom must be a mapping".into())),
        }
    };
    let empty = Mapping::new();
    let patch_key = Value::String("patch".into());
    let current = parse(current)?;
    let applied = applied.as_mapping().unwrap_or(&empty);
    let mut original = original.map(parse).transpose()?.unwrap_or_default();
    for key in current
        .keys()
        .chain(applied.keys())
        .filter(|key| *key != &patch_key)
    {
        restore_user_key(&mut original, &current, applied, key);
    }

    let current_patch = current
        .get(&patch_key)
        .and_then(Value::as_mapping)
        .unwrap_or(&empty);
    let applied_patch = applied
        .get(&patch_key)
        .and_then(Value::as_mapping)
        .unwrap_or(&empty);
    let mut patch = original
        .remove(&patch_key)
        .and_then(|value| value.as_mapping().cloned())
        .unwrap_or_default();
    for key in current_patch.keys().chain(applied_patch.keys()) {
        restore_user_key(&mut patch, current_patch, applied_patch, key);
    }
    patch.retain(|key, _| !is_schema_list_key(key));
    if !patch.is_empty() {
        original.insert(patch_key, Value::Mapping(patch));
    }
    if original.is_empty() {
        return Ok(Vec::new());
    }
    serde_yaml::to_string(&original)
        .map(String::into_bytes)
        .map_err(|error| CoreError::Other(error.to_string()))
}

#[cfg(test)]
fn restore_user_key(
    original: &mut serde_yaml::Mapping,
    current: &serde_yaml::Mapping,
    applied: &serde_yaml::Mapping,
    key: &serde_yaml::Value,
) {
    let value = current.get(key);
    if value != applied.get(key) {
        if let Some(value) = value {
            original.insert(key.clone(), value.clone());
        } else {
            original.remove(key);
        }
    }
}

fn is_schema_list_key(key: &serde_yaml::Value) -> bool {
    key.as_str()
        .is_some_and(|key| key == "schema_list" || key.starts_with("schema_list/"))
}

fn walk_files(
    root: &Path,
    prefix: &str,
    skip: impl Fn(&str) -> bool + Copy,
    visit: &mut impl FnMut(&Path, &str) -> Result<(), CoreError>,
) -> Result<(), CoreError> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let relative = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        if skip(&relative) {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_dir() {
            walk_files(&entry.path(), &relative, skip, visit)?;
        } else if kind.is_file() {
            visit(&entry.path(), &relative)?;
        }
    }
    Ok(())
}

fn strip_generated(path: &str, bytes: Vec<u8>) -> Result<Vec<u8>, CoreError> {
    if path.ends_with(".custom.yaml") || path == DEFAULT_CUSTOM[1] {
        return managed_custom::strip(utf8(&bytes)?)
            .map(String::into_bytes)
            .map_err(CoreError::Other);
    }
    Ok(bytes)
}

fn utf8(bytes: &[u8]) -> Result<&str, CoreError> {
    std::str::from_utf8(bytes).map_err(|error| CoreError::Other(error.to_string()))
}

fn materialized_path(path: &str) -> bool {
    path == ".nomedia" || !private_path(path.strip_suffix("/.nomedia").unwrap_or(path))
}

pub(crate) fn lock_runtime(runtime: &Path, wait: bool) -> Result<fs::File, CoreError> {
    Ok(crate::package_fs::file_lease(&runtime.join(".keytao-materialize.lock"), wait)?)
}

fn prepare_runtime(
    control_root: &Path,
    bundled: &Path,
    key: &str,
    sources: RuntimeFiles,
    selection: Option<&[String]>,
    overlay: Option<&RuntimeFiles>,
    stage: bool,
    windows_lua: bool,
) -> Result<Option<tempfile::TempDir>, CoreError> {
    let runtime = RuntimeDirs::isolated_user_data_dir(control_root, key)
        .ok_or_else(|| CoreError::Other("Invalid runtime package key".into()))?;
    safe(control_root, &format!("runtime/{key}"))?;
    fs::create_dir_all(&runtime)?;
    let _lock = lock_runtime(&runtime, true)?;
    match fs::remove_dir_all(safe(&runtime, COPY_STAGE)?) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let previous = current_manifest(&runtime, key)?;

    let mut selected = selected_schemas(key, &sources, selection)?;
    let mut generated = Files::new();
    let mut owned = RuntimeFiles::new();
    let mut schema_sources = Files::new();
    let mut bundled_files = RuntimeFiles::new();
    add_common_files(bundled, &sources, &mut bundled_files)?;
    if let Some(overlay) = overlay {
        for (path, source) in overlay
            .iter()
            .filter(|(path, _)| materialized_path(path) && crate::addon::is_package_file(path))
        {
            match sources.get(path) {
                Some(own) if own.sha256()? == source.sha256()? => continue,
                Some(_) => {
                    return Err(CoreError::Other(format!(
                        "文件 {path} 与 Easy English 冲突"
                    )));
                }
                None => owned.insert(path.clone(), source.clone()),
            };
        }
        if let Some(source) = owned.get("easy_en.schema.yaml") {
            schema_sources.insert("easy_en.schema.yaml".into(), source.read()?.into_owned());
        }
    }
    for (path, source) in sources {
        if !materialized_path(&path) {
            continue;
        }
        if path == "default.yaml" && !has_preset_default(source.read()?.as_ref()) {
            continue;
        }
        if path.ends_with(".schema.yaml") && !path.contains('/') {
            schema_sources.insert(path.clone(), source.read()?.into_owned());
        }
        owned.insert(path, source);
    }
    if overlay.is_some() {
        if !schema_sources.contains_key("easy_en.schema.yaml") {
            return Err(CoreError::Other("Easy English has no schema source".into()));
        }
        if !selected.iter().any(|id| id == EASY_EN_ADDON_ID) {
            selected.push(EASY_EN_ADDON_ID.into());
        }
    }
    if let Some(entry) = owned.remove("rime.lua") {
        if let Some(lua) = crate::install::prepare_rime_lua_install(Some(utf8(&entry.read()?)?), &[]) {
            generated.insert("rime.lua".into(), lua.into_bytes());
        }
    }

    let mut user = Files::new();
    let mut runtime_overrides = previous.runtime_overrides.clone();
    let mut disk_hashes = BTreeMap::new();
    walk_files(&runtime, "", private_path, &mut |path, relative| {
        let tracked = previous.fingerprint(relative);
        let preserve = preserves_runtime_data(Path::new(relative));
        let is_account_user_file = relative == crate::account::USER_DICTIONARY
            || relative == crate::account::USER_WORD_REMOVALS;
        let seeds_file = owned.contains_key(relative)
            || bundled_files.contains_key(relative)
            || generated.contains_key(relative)
            || is_account_user_file
            || relative == crate::remove_filter::SCRIPT_PATH;
        if relative == keytao_core::english_mode::SETTINGS_FILE
            || (tracked.is_none() && !user_path(relative) && !seeds_file)
        {
            return Ok(());
        }
        let digest = sha256_reader(fs::File::open(path)?)?;
        let changed = tracked != Some(&digest);
        disk_hashes.insert(relative.to_owned(), digest);
        if changed {
            runtime_overrides.insert(relative.to_owned());
        }
        if relative == DEFAULT_CUSTOM[0]
            && !runtime_overrides.contains(relative)
            && let Some(default) = &previous.user_default
        {
            user.entry(relative.to_owned())
                .or_insert_with(|| default.as_bytes().to_vec());
            return Ok(());
        }
        if runtime_overrides.contains(relative)
            || (previous.user_owned.contains_key(relative) && !user.contains_key(relative))
        {
            let bytes = fs::read(path)?;
            if preserve {
                disk_hashes.insert(relative.to_owned(), sha256_reader(bytes.as_slice())?);
                user.insert(relative.to_owned(), bytes);
            } else {
                let bytes = strip_generated(relative, bytes)?;
                if !bytes.is_empty() || runtime_overrides.contains(relative) {
                    user.insert(relative.to_owned(), bytes);
                }
            }
        }
        Ok(())
    })?;

    let incoming = take_default_custom(&mut owned);
    let incoming = incoming.as_ref().map(RuntimeFile::read).transpose()?;
    let existing = take_default_custom(&mut user);
    let default = default_custom(
        incoming.as_deref(),
        existing.as_deref().filter(|bytes| !bytes.is_empty()),
        &selected,
    )?;
    if let Some(existing) = existing {
        if runtime_overrides.remove(DEFAULT_CUSTOM[1]) {
            runtime_overrides.insert(DEFAULT_CUSTOM[0].into());
        }
        user.insert(DEFAULT_CUSTOM[0].into(), existing);
    }
    generated.insert(DEFAULT_CUSTOM[0].into(), default);
    for (path, bytes) in &user {
        owned.remove(path);
        bundled_files.remove(path);
        if path != DEFAULT_CUSTOM[0] {
            generated.remove(path);
        }
        if path.ends_with(".schema.yaml") && !path.contains('/') {
            schema_sources.insert(path.clone(), bytes.clone());
        }
    }

    regenerate_custom(
        &runtime,
        &schema_sources,
        &selected,
        crate::ime_settings::read_candidate_page_size(control_root),
        &mut owned,
        &mut generated,
        &user,
    )?;
    let schema_custom = |path: &str| {
        path.strip_suffix(".custom.yaml")
            .is_some_and(|schema| schema_sources.contains_key(&format!("{schema}.schema.yaml")))
    };
    let owned_custom = owned.iter()
        .filter(|(path, _)| schema_custom(path))
        .map(|(_, source)| source.read())
        .collect::<Result<Vec<_>, _>>()?;
    let user_and_generated_custom = user.iter().chain(&generated)
        .filter(|(path, _)| schema_custom(path))
        .map(|(_, bytes)| bytes.as_slice());
    let schema_configs = schema_sources.values().map(Vec::as_slice)
        .chain(owned_custom.iter().map(Cow::as_ref))
        .chain(user_and_generated_custom);
    let used = crate::account::runtime_dictionary_usage(schema_configs, |name| {
        if let Some(bytes) = user.get(name) {
            return Ok(Some(Cow::Borrowed(bytes.as_slice())));
        }
        owned.get(name).map(RuntimeFile::dictionary_header).transpose()
    })?;
    crate::account::add_runtime_user_dictionary(
        control_root,
        |name| {
            if let Some(bytes) = user.get(name) {
                return Ok(Some(Cow::Borrowed(bytes.as_slice())));
            }
            owned.get(name).map(RuntimeFile::read).transpose()
        },
        &used,
        &user,
        &mut generated,
    )?;
    if windows_lua
        && let Some(entry) = user.get("rime.lua").or_else(|| generated.get("rime.lua"))
    {
        // Publish the same Lua inputs that the deployment receipt covers.
        let compatibility = keytao_core::WindowsLuaCompatibility::new(utf8(entry)?);
        for (path, source) in &mut owned {
            if (path.eq_ignore_ascii_case("lua/helpers.lua") || (path.ends_with(".schema.yaml") && !path.contains('/')))
                && let Some(patched) = compatibility.patch_source(path, &source.read()?)
            {
                *source = RuntimeFile::Bytes(patched.into_bytes());
            }
        }
        for (path, bytes) in generated.iter_mut().chain(&mut user) {
            if let Some(patched) = compatibility.patch_source(path, bytes) {
                *bytes = patched.into_bytes();
            }
        }
    }
    runtime_overrides.retain(|path| user.contains_key(path));
    bundled_files.extend(generated.into_iter().map(|(path, bytes)| (path, RuntimeFile::Bytes(bytes))));
    write_runtime(
        &runtime,
        key,
        previous,
        owned,
        bundled_files,
        user,
        runtime_overrides,
        &disk_hashes,
        stage,
    )
}

fn selected_schemas(
    key: &str,
    sources: &RuntimeFiles,
    selection: Option<&[String]>,
) -> Result<Vec<String>, CoreError> {
    let configs: Vec<_> = sources
        .iter()
        .filter(|(path, _)| {
            DEFAULT_CUSTOM.contains(&path.as_str())
                || *path == "default.yaml"
                || (!path.contains('/') && path.ends_with(".schema.yaml"))
        })
        .map(|(path, source)| Ok((path.clone(), source.read()?.into_owned())))
        .collect::<Result<_, CoreError>>()?;
    let available = |id: &str| id != EASY_EN_ADDON_ID && sources.contains_key(&format!("{id}.schema.yaml"));
    let mut schemas: Vec<_> = crate::package_schemas::enumerate(key, &configs, selection)?
        .into_iter()
        .filter(|schema| schema.selected && available(&schema.id))
        .map(|schema| schema.id)
        .collect();
    if schemas.is_empty() {
        schemas.extend(sources.keys().filter_map(|path| {
            (!path.contains('/'))
                .then(|| path.strip_suffix(".schema.yaml"))
                .flatten()
                .filter(|id| *id != EASY_EN_ADDON_ID)
                .map(str::to_owned)
        }));
    }
    if schemas.is_empty() {
        return Err(CoreError::Other(format!(
            "Package {key} has no schema sources"
        )));
    }
    Ok(schemas)
}

fn has_preset_default(bytes: &[u8]) -> bool {
    serde_yaml::from_slice::<serde_yaml::Value>(bytes)
        .ok()
        .and_then(|value| value.as_mapping().cloned())
        .is_some_and(|map| map.contains_key(serde_yaml::Value::String("punctuator".into())))
}

fn add_common_files(
    bundled: &Path,
    sources: &RuntimeFiles,
    generated: &mut RuntimeFiles,
) -> Result<(), CoreError> {
    if bundled.as_os_str().is_empty() && !sources.contains_key("default.yaml") {
        return Ok(());
    }
    for path in COMMON {
        if path == "opencc" {
            if bundled.as_os_str().is_empty() {
                continue;
            }
            walk_files(
                &bundled.join(path),
                path,
                private_path,
                &mut |source, relative| {
                    if !sources.contains_key(relative) {
                        generated.insert(relative.to_owned(), RuntimeFile::disk(source)?);
                    }
                    Ok(())
                },
            )?;
        } else if path == "default.yaml" {
            if !sources.get("default.yaml").map(RuntimeFile::read).transpose()?
                .is_some_and(|bytes| has_preset_default(&bytes))
            {
                if !bundled.as_os_str().is_empty()
                    && let Some(source) = optional_source(&bundled.join(path))?
                {
                    generated.insert(path.to_owned(), source);
                } else if sources.contains_key("default.yaml") {
                    generated.insert(path.to_owned(), RuntimeFile::Bytes(include_bytes!("addon-default.yaml").to_vec()));
                }
            }
        } else if !sources.contains_key(path) && !bundled.as_os_str().is_empty() {
            if let Some(source) = optional_source(&bundled.join(path))? {
                generated.insert(path.to_owned(), source);
            }
        }
    }
    Ok(())
}

fn is_runtime_config(path: &str) -> bool {
    (path.ends_with(".yaml") || path.ends_with(".yml"))
        && !path.ends_with(".dict.yaml")
        && !path.ends_with(".dict.yml")
        && !path.starts_with("opencc/")
}

#[derive(Default)]
struct CommonUsage {
    dictionaries: BTreeMap<String, bool>,
    essay: bool,
    opencc: BTreeSet<String>,
    opencc_fallback: bool,
}

impl CommonUsage {
    fn scan(&mut self, value: &Value, files: &BTreeMap<&str, &RuntimeFile>) -> Option<()> {
        match value {
            Value::Mapping(map) => {
                for (key, value) in map {
                    let key = key.as_str().unwrap_or("").rsplit('/')
                        .find(|part| !part.starts_with('@'))
                        .unwrap_or("");
                    match key {
                        "dictionary" | "packs" | "import_tables" => {
                            let mut add = |name: &str| {
                                *self.dictionaries.entry(name.into()).or_default() |=
                                    key != "import_tables";
                            };
                            match value {
                                Value::String(name) if !name.is_empty() => {
                                    add(name);
                                }
                                Value::Sequence(names) => {
                                    for name in names {
                                        add(name.as_str()?);
                                    }
                                }
                                Value::String(_) | Value::Null => {}
                                _ => return None,
                            }
                        }
                        "__include" | "__patch" | "import_preset" => {
                            self.reference(value, key == "import_preset", files)?;
                        }
                        "dependencies" => {
                            let schemas = value.as_sequence()?;
                            for schema in schemas {
                                let name = format!("{}.schema.yaml", schema.as_str()?);
                                if !files.contains_key(name.as_str()) {
                                    return None;
                                }
                            }
                        }
                        "opencc_config" => {
                            let name = value.as_str()?;
                            if !name.is_empty() {
                                self.opencc.insert(format!("opencc/{name}"));
                            }
                        }
                        _ => self.scan(value, files)?,
                    }
                }
            }
            Value::Sequence(items) => {
                for item in items {
                    self.scan(item, files)?;
                }
            }
            Value::String(text) => {
                self.essay |= text == "essay.txt";
                if text.split('@').next() == Some("simplifier")
                    && files.contains_key("opencc/t2s.json")
                {
                    self.opencc.insert("opencc/t2s.json".into());
                }
                if text.starts_with("opencc/") {
                    self.opencc.insert(text.clone());
                }
            }
            Value::Tagged(_) => return None,
            _ => {}
        }
        Some(())
    }

    fn reference(
        &mut self,
        value: &Value,
        preset: bool,
        files: &BTreeMap<&str, &RuntimeFile>,
    ) -> Option<()> {
        match value {
            Value::String(reference) => {
                let id = if preset {
                    reference.as_str()
                } else {
                    reference.split_once(':').map_or("", |(id, _)| id)
                };
                if id.is_empty()
                    || (is_runtime_config(id) && files.contains_key(id))
                    || files.contains_key(format!("{id}.yaml").as_str())
                    || (preset && files.contains_key(format!("{id}.symbols.yaml").as_str()))
                {
                    return Some(());
                }
                None
            }
            Value::Sequence(items) => {
                for item in items {
                    self.reference(item, preset, files)?;
                }
                Some(())
            }
            Value::Mapping(_) => self.scan(value, files),
            _ => None,
        }
    }
}

fn common_usage(files: &BTreeMap<&str, &RuntimeFile>) -> Result<CommonUsage, CoreError> {
    let mut usage = CommonUsage::default();
    for (&path, source) in files {
        if path.ends_with(".lua") {
            // Lua can choose OpenCC data at runtime.
            usage.opencc_fallback = true;
        }
        if !is_runtime_config(path) {
            continue;
        }
        let config = crate::upload_bundle::yaml(&source.read()?, path);
        if config
            .ok()
            .and_then(|value| usage.scan(&value, files))
            .is_none()
        {
            usage.essay = true;
            usage.opencc_fallback = true;
            return Ok(usage);
        }
    }

    let mut checked = BTreeMap::new();
    while !usage.essay && let Some((id, primary)) = usage.dictionaries.pop_first() {
        if checked.get(&id).is_some_and(|&was_primary| was_primary || !primary) {
            continue;
        }
        checked.insert(id.clone(), primary);
        let name = format!("{id}.dict.yaml");
        let Some(source) = files.get(name.as_str()) else {
            usage.essay = true;
            break;
        };
        let header = crate::upload_bundle::yaml(&source.dictionary_header()?, &name);
        let Some(header) = header.ok().filter(|header| {
            header.get("name").and_then(Value::as_str)
                .is_some_and(|name| !primary || name == id)
        }) else {
            usage.essay = true;
            break;
        };
        if usage.scan(&header, files).is_none() {
            usage.essay = true;
            usage.opencc_fallback = true;
            break;
        }
        usage.essay |= match header.get("vocabulary") {
            Some(value) => value.as_str().is_none_or(|name| {
                name.is_empty() || name.rsplit(['/', '\\']).next()
                    .is_some_and(|name| name.eq_ignore_ascii_case("essay"))
            }),
            None => match &header["use_preset_vocabulary"] {
                Value::Null | Value::Bool(false) => false,
                Value::String(value) => !value.eq_ignore_ascii_case("false"),
                _ => true,
            },
        };
    }
    Ok(usage)
}

fn opencc_assets(
    usage: CommonUsage,
    files: &BTreeMap<&str, &RuntimeFile>,
) -> Result<Option<BTreeSet<String>>, CoreError> {
    fn dependencies<'a>(value: &'a serde_json::Value, names: &mut Vec<&'a str>) -> Option<()> {
        match value {
            serde_json::Value::Object(map) => {
                for (key, value) in map {
                    if key == "file" {
                        names.push(value.as_str()?);
                    } else {
                        dependencies(value, names)?;
                    }
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    dependencies(item, names)?;
                }
            }
            _ => {}
        }
        Some(())
    }

    if usage.opencc_fallback {
        return Ok(None);
    }
    let mut pending = usage.opencc;
    let mut used = BTreeSet::new();
    while let Some(path) = pending.pop_first() {
        if used.contains(&path) {
            continue;
        }
        let Some(source) = files.get(path.as_str()) else {
            return Ok(None);
        };
        if path.ends_with(".json") {
            let Ok(config) = serde_json::from_slice(&source.read()?) else {
                return Ok(None);
            };
            let mut names = Vec::new();
            if dependencies(&config, &mut names).is_none() {
                return Ok(None);
            }
            for name in names {
                let name = name.trim_start_matches("./");
                let (parent, _) = path.rsplit_once('/').unwrap();
                let paths = [format!("{parent}/{name}"), format!("opencc/{name}")];
                let mut found = false;
                for path in paths {
                    if files.contains_key(path.as_str()) {
                        pending.insert(path);
                        found = true;
                    }
                }
                if !found {
                    return Ok(None);
                }
            }
        }
        used.insert(path);
    }
    Ok(Some(used))
}

fn optional_source(path: &Path) -> Result<Option<RuntimeFile>, CoreError> {
    if path.try_exists()? {
        Ok(Some(RuntimeFile::disk(path)?))
    } else {
        Ok(None)
    }
}

fn take_default_custom<T>(files: &mut BTreeMap<String, T>) -> Option<T> {
    let current = files.remove(DEFAULT_CUSTOM[0]);
    let legacy = files.remove(DEFAULT_CUSTOM[1]);
    current.or(legacy)
}

fn default_custom(
    package: Option<&[u8]>,
    user: Option<&[u8]>,
    schemas: &[String],
) -> Result<Vec<u8>, CoreError> {
    let mut package: serde_yaml::Value = serde_yaml::from_slice(package.unwrap_or(b"{}"))
        .map_err(|error| CoreError::Other(error.to_string()))?;
    if package.is_null() {
        package = serde_yaml::Mapping::new().into();
    }
    let mapping = package
        .as_mapping_mut()
        .ok_or_else(|| CoreError::Other("Package default custom must be a mapping".into()))?;
    let patch = crate::scheme_files::yaml_child_mapping(mapping, "patch", "Invalid default patch")
        .map_err(CoreError::Other)?;
    patch.insert(
        "schema_list".into(),
        serde_yaml::Value::Sequence(Vec::new()),
    );
    let package = serde_yaml::to_string(&package).map_err(|e| CoreError::Other(e.to_string()))?;
    let (merged, _) =
        crate::install::merge_local_default_custom(user.map(utf8).transpose()?, &package)
            .map_err(CoreError::Other)?;
    let mut merged: serde_yaml::Mapping =
        serde_yaml::from_str(&merged).map_err(|error| CoreError::Other(error.to_string()))?;
    let patch =
        crate::scheme_files::yaml_child_mapping(&mut merged, "patch", "Invalid default patch")
            .map_err(CoreError::Other)?;
    patch.retain(|key, _| !is_schema_list_key(key));
    let entries: Vec<_> = schemas
        .iter()
        .map(|id| BTreeMap::from([("schema", id)]))
        .collect();
    patch.insert(
        "schema_list".into(),
        serde_yaml::to_value(entries).map_err(|error| CoreError::Other(error.to_string()))?,
    );
    serde_yaml::to_string(&merged)
        .map(String::into_bytes)
        .map_err(|error| CoreError::Other(error.to_string()))
}

fn regenerate_custom(
    runtime: &Path,
    sources: &Files,
    schemas: &[String],
    page_size: Option<u8>,
    owned: &mut RuntimeFiles,
    generated: &mut Files,
    user: &Files,
) -> Result<(), CoreError> {
    let staging = tempfile::Builder::new()
        .prefix(".keytao-materialize-")
        .tempdir_in(runtime)?;
    let stage = staging.path();
    for (path, bytes) in sources
        .iter()
    {
        let destination = safe(stage, path)?;
        fs::create_dir_all(destination.parent().unwrap())?;
        fs::write(destination, bytes)?;
    }
    for (path, source) in owned.iter().filter(|(path, _)|
        path.ends_with(".custom.yaml") || *path == crate::remove_filter::SCRIPT_PATH
    ) {
        let destination = safe(stage, path)?;
        fs::create_dir_all(destination.parent().unwrap())?;
        source.copy_to(&mut fs::File::create(destination)?)?;
    }
    for (path, bytes) in user.iter().filter(|(path, _)| {
        path.ends_with(".custom.yaml") || *path == crate::remove_filter::SCRIPT_PATH
    }) {
        let destination = safe(stage, path)?;
        fs::create_dir_all(destination.parent().unwrap())?;
        fs::write(destination, bytes)?;
    }
    if let Some(page_size) = page_size {
        fs::write(
            stage.join(keytao_core::english_mode::SETTINGS_FILE),
            serde_json::to_vec(&serde_json::json!({ "candidatePageSize": page_size }))?,
        )?;
    }
    // Use the package's selected schemas for both the remove filter and candidate page size.
    managed_custom::regenerate(stage, &schemas.iter().cloned().collect())
        .map_err(CoreError::Other)?;
    for path in managed_custom::generated_paths(stage).map_err(CoreError::Other)? {
        if path == crate::remove_filter::SCRIPT_PATH && user.contains_key(&path) {
            continue;
        }
        let bytes = fs::read(safe(stage, &path)?)?;
        if owned.get(&path).map(RuntimeFile::sha256).transpose()?.as_deref()
            != Some(sha256_reader(bytes.as_slice())?.as_str())
        {
            owned.remove(&path);
            generated.insert(path, bytes);
        }
    }
    Ok(())
}

fn optional_sha256(path: &Path) -> Result<Option<String>, CoreError> {
    match fs::File::open(path) {
        Ok(file) => Ok(Some(sha256_reader(file)?)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn write_runtime(
    runtime: &Path,
    key: &str,
    previous: Manifest,
    mut owned: RuntimeFiles,
    mut generated: RuntimeFiles,
    user: Files,
    runtime_overrides: BTreeSet<String>,
    disk_hashes: &BTreeMap<String, String>,
    stage: bool,
) -> Result<Option<tempfile::TempDir>, CoreError> {
    let user_default = user.get(DEFAULT_CUSTOM[0])
        .filter(|bytes| !bytes.is_empty())
        .map(|bytes| utf8(bytes).map(str::to_owned))
        .transpose()?;
    let user: RuntimeFiles = user.into_iter()
        .map(|(path, bytes)| (path, RuntimeFile::Bytes(bytes)))
        .collect();
    let builtin = key.starts_with("builtin/");
    let common_data = |path: &String| path == "essay.txt" || path.starts_with("opencc/");
    if generated.keys().any(common_data) || builtin && owned.keys().any(common_data) {
        let files = owned.iter().chain(&user).chain(&generated)
            .map(|(path, source)| (path.as_str(), source))
            .collect();
        let usage = common_usage(&files)?;
        let essay = usage.essay;
        let opencc = opencc_assets(usage, &files)?;
        let keep_common = |path: &str| match path {
            "essay.txt" => essay,
            path if path.starts_with("opencc/") => {
                opencc.as_ref().is_none_or(|used| used.contains(path))
            }
            _ => true,
        };
        generated.retain(|path, _| keep_common(path));
        if builtin {
            owned.retain(|path, _| keep_common(path));
        }
    }
    let fingerprint = |files: &RuntimeFiles| -> Result<BTreeMap<_, _>, CoreError> {
        files
            .iter()
            .map(|(name, source)| Ok((name.clone(), source.sha256()?.into_owned())))
            .collect()
    };
    let mut manifest = Manifest {
        package: key.into(),
        owned: fingerprint(&owned)?,
        generated: fingerprint(&generated)?,
        user_owned: fingerprint(&user)?,
        user_default,
        runtime_overrides,
    };
    let is_local = crate::scheme::is_local_scheme_key(key);
    let mut files = owned;
    files.extend(user);
    for (path, source) in generated {
        if let Some(digest) = manifest.user_owned.get_mut(&path) {
            *digest = manifest.generated[&path].clone();
        }
        files.insert(path, source);
    }
    let mut writes = Vec::new();
    for (relative, source) in files {
        let path = safe(runtime, &relative)?;
        let target_sha256 = manifest.fingerprint(&relative).cloned();
        let existing_sha256 = match disk_hashes.get(&relative) {
            Some(digest) => Some(digest.clone()),
            None => optional_sha256(&path)?,
        };
        let previous_sha256 = if let Some(digest) = existing_sha256 {
            if target_sha256.as_deref() == Some(digest.as_str()) {
                continue;
            }
            if !is_local
                && !manifest.user_owned.contains_key(&relative)
                && previous.fingerprint(&relative) != Some(&digest)
            {
                return Err(CoreError::Other(format!(
                    "Modified runtime file: {relative}"
                )));
            }
            Some(digest)
        } else {
            None
        };
        writes.push((path, source, previous_sha256));
    }
    let mut removals = Vec::new();
    for (relative, hash) in previous.owned.iter().chain(&previous.generated).chain(
        previous.user_owned.get_key_value(crate::account::USER_DICTIONARY),
    ) {
        if manifest.fingerprint(relative).is_none()
            && (is_local
                || relative == crate::account::USER_DICTIONARY
                || !previous.user_owned.contains_key(relative))
        {
            let path = safe(runtime, relative)?;
            if let Some(actual) = optional_sha256(&path)? {
                if actual == *hash {
                    removals.push((path, actual));
                }
            }
        }
    }
    let pending_path = safe(runtime, PENDING)?;
    let patch = Pending {
        previous,
        next: manifest,
    };
    let manifest_path = safe(runtime, MANIFEST)?;
    let manifest_bytes = serde_json::to_vec(&patch.next)?;
    if stage {
        if let Some(bytes) = read_optional(&pending_path)? {
            removals.push((pending_path, sha256_reader(bytes.as_slice())?));
        }
        let existing = read_optional(&manifest_path)?;
        if existing.as_deref() != Some(manifest_bytes.as_slice()) {
            let digest = existing
                .map(|bytes| sha256_reader(bytes.as_slice()))
                .transpose()?;
            writes.push((manifest_path, RuntimeFile::Bytes(manifest_bytes), digest));
        }
        if writes.is_empty() && removals.is_empty() {
            return Ok(None);
        }
        let stage = tempfile::Builder::new()
            .prefix(STAGED_INPUTS_PREFIX)
            .tempdir_in(runtime)?;
        let mut edits = BTreeMap::new();
        for (path, source, previous_sha256) in writes {
            let relative = path.strip_prefix(runtime).unwrap();
            source.write_staged(&stage.path().join(relative))?;
            edits.insert(
                relative.to_owned(),
                StagedInput {
                    previous_sha256,
                    replace: true,
                },
            );
        }
        for (path, digest) in removals {
            let relative = path.strip_prefix(runtime).unwrap();
            edits.insert(
                relative.to_owned(),
                StagedInput {
                    previous_sha256: Some(digest),
                    replace: false,
                },
            );
        }
        write_file(
            &stage.path().join(STAGED_INPUTS_MANIFEST),
            &serde_json::to_vec(&edits)?,
        )?;
        return Ok(Some(stage));
    }
    let copy_stage = if writes.is_empty() {
        None
    } else {
        Some(tempfile::Builder::new().prefix(COPY_STAGE).rand_bytes(0).tempdir_in(runtime)?)
    };
    let mut staged_writes = Vec::new();
    if let Some(copy_stage) = &copy_stage {
        for (index, (path, source, _)) in writes.into_iter().enumerate() {
            let staged = copy_stage.path().join(index.to_string());
            source.write_staged(&staged)?;
            staged_writes.push((path, staged));
        }
    }
    if !staged_writes.is_empty() || !removals.is_empty() {
        write_file(&pending_path, &serde_json::to_vec(&patch)?)?;
    }
    for (path, staged) in staged_writes {
        fs::create_dir_all(path.parent().unwrap())?;
        fs::rename(staged, path)?;
    }
    for (path, _) in removals {
        fs::remove_file(&path)?;
        let relative = path.strip_prefix(runtime).unwrap();
        if relative == Path::new("essay.txt") || relative.starts_with("opencc") {
            keytao_core::rt_log!(
                keytao_core::runtime_log::Level::Info,
                "app",
                "runtime_common_file_removed",
                path = path.display().to_string()
            );
        }
    }
    write_file(&manifest_path, &manifest_bytes)?;
    if pending_path.try_exists()? {
        fs::remove_file(pending_path)?;
    }
    Ok(None)
}

pub(crate) fn orphan_manifest(runtime: &Path, key: &str) -> Result<Option<Manifest>, CoreError> {
    let absent = |name| match fs::symlink_metadata(safe(runtime, name)?) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(true),
        Err(error) => Err(CoreError::from(error)),
    };
    if !absent(PENDING)? {
        return Ok(None);
    }
    let path = safe(runtime, MANIFEST)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() || crate::package_fs::is_link(&metadata) {
        return Ok(None);
    }
    let manifest: Manifest = serde_json::from_slice(&fs::read(path)?)?;
    manifest.validate(key)?;
    Ok((manifest.package == key).then_some(manifest))
}

fn current_manifest(runtime: &Path, key: &str) -> Result<Manifest, CoreError> {
    let read = |path| read_optional(&safe(runtime, path)?);
    if let Some(bytes) = read(PENDING)? {
        let Pending { mut previous, next } = serde_json::from_slice(&bytes)?;
        previous.validate(key)?;
        next.validate(key)?;
        for path in next
            .owned
            .keys()
            .chain(next.generated.keys())
            .chain(next.user_owned.keys())
        {
            let current = optional_sha256(&safe(runtime, path)?)?;
            if current.as_ref() != next.fingerprint(path) {
                continue;
            }
            for (previous, next) in [
                (&mut previous.owned, &next.owned),
                (&mut previous.generated, &next.generated),
                (&mut previous.user_owned, &next.user_owned),
            ] {
                previous.remove(path);
                if let Some(digest) = next.get(path) {
                    previous.insert(path.clone(), digest.clone());
                }
            }
            previous.runtime_overrides.remove(path);
            if next.runtime_overrides.contains(path) {
                previous.runtime_overrides.insert(path.clone());
            }
        }
        return Ok(previous);
    }
    let previous: Manifest = read(MANIFEST)?
        .map(|bytes| serde_json::from_slice(&bytes))
        .transpose()?
        .unwrap_or_default();
    previous.validate(key)?;
    Ok(previous)
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), CoreError> {
    if read_optional(path)?.as_deref() != Some(bytes) {
        let parent = path.parent().unwrap();
        fs::create_dir_all(parent)?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        temp.write_all(bytes)?;
        temp.as_file().sync_all()?;
        temp.persist(path).map_err(|error| error.error)?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) use tests::hashes as test_hashes;

#[cfg(test)]
#[path = "../../keytao-core/tests/support/smoke_fixture.rs"]
mod smoke_fixture;

#[cfg(test)]
mod tests {
    use super::*;

    fn materialize(
        control_root: &Path,
        bundled: &Path,
        key: &str,
        sources: Files,
        selection: Option<&[String]>,
        overlay: Option<&Files>,
    ) -> Result<(), CoreError> {
        let overlay = overlay.cloned().map(runtime_files);
        prepare_runtime(
            control_root,
            bundled,
            key,
            runtime_files(sources),
            selection,
            overlay.as_ref(),
            false,
            cfg!(windows),
        )
        .map(|_| ())
    }

    fn runtime_files(files: Files) -> RuntimeFiles {
        files.into_iter().map(|(path, bytes)| (path, RuntimeFile::Bytes(bytes))).collect()
    }

    #[test]
    fn changed_source_is_rejected_in_a_fresh_stage() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let stage = tempfile::tempdir_in(dir.path()).unwrap();
        fs::write(&source, b"original").unwrap();
        let snapshot = RuntimeFile::disk(&source).unwrap();
        let first = stage.path().join("first");
        snapshot.write_staged(&first).unwrap();
        fs::write(&source, b"modified").unwrap();

        assert_eq!(fs::read(first).unwrap(), b"original");
        assert!(snapshot.write_staged(&stage.path().join("candidate")).is_err());
    }

    #[test]
    fn dictionary_body_validation_spans_buffer_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.dict.yaml");
        let header = b"---\nname: sample\n...\n";
        let mut dictionary = header.to_vec();
        dictionary.extend(std::iter::repeat_n(b'a', 16 * 1024 - 1));
        dictionary.extend_from_slice("字\tzi".as_bytes());
        fs::write(&path, &dictionary).unwrap();
        assert_eq!(RuntimeFile::disk(&path).unwrap().dictionary_header().unwrap().as_ref(), header);
        dictionary.push(0xff);
        fs::write(&path, dictionary).unwrap();
        assert!(RuntimeFile::disk(&path).unwrap().dictionary_header().is_err());
    }

    fn put(root: &Path, path: &str, content: &str) {
        let dest = root.join(path);
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        fs::write(dest, content).unwrap();
    }

    fn fixture(root: &Path, key: &str) -> Files {
        let package = root.join(format!("scheme-packages/cloud/{key}/files"));
        put(&package, "symbols.yaml", key);
        put(
            &package,
            &format!("{key}.schema.yaml"),
            &format!("schema: {{schema_id: {key}, name: {key}}}\n"),
        );
        put(&package, "rime.lua", &format!("require('{key}')\n"));
        put(&package, &format!("lua/{key}.lua"), key);
        put(&package, "opencc/owned.json", key);
        let mut files = Files::new();
        walk_files(&package, "", |_| false, &mut |path, relative| {
            files.insert(relative.into(), fs::read(path)?);
            Ok(())
        })
        .unwrap();
        files
    }

    #[test]
    fn materialize_keeps_comment_only_schema_custom() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut sources = fixture(root, "keydo");
        let custom = b"# Optional settings\n";
        sources.insert("keydo.custom.yaml".into(), custom.to_vec());

        materialize(root, &root.join("rime-data"), "keydo", sources, None, None).unwrap();

        let runtime = RuntimeDirs::isolated_user_data_dir(root, "keydo").unwrap();
        assert_eq!(fs::read(runtime.join("keydo.custom.yaml")).unwrap(), custom);
    }

    #[test]
    fn optional_common_data_tracks_dictionary_imports_and_preserves_runtime_edits() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let bundled = root.join("rime-data");
        put(&bundled, "essay.txt", "preset vocabulary");
        put(&bundled, "opencc/t2s.json", r#"{"dicts":[{"file":"obsolete.ocd2"},{"file":"edited.ocd2"}]}"#);
        put(&bundled, "opencc/obsolete.ocd2", "data");
        put(&bundled, "opencc/edited.ocd2", "data");
        put(&bundled, "opencc/unrelated.json", "{}");

        let mut sources = Files::from([
            (
                "sample.schema.yaml".into(),
                b"schema: {schema_id: sample, name: Sample}\n__include: config:/sample\n"
                    .to_vec(),
            ),
            (
                "config.yaml".into(),
                b"sample: {translator: {dictionary: sample}, engine: {filters: [simplifier]}}\n"
                    .to_vec(),
            ),
            ("sample.dict.yaml".into(), b"---\nname: sample\nimport_tables: [tables/extra]\n...\n".to_vec()),
            ("tables/extra.dict.yaml".into(), b"---\nname: tables/extra\nuse_preset_vocabulary: 'TRUE'\n...\n".to_vec()),
            ("opencc/package.json".into(), b"package data".to_vec()),
        ]);
        materialize(root, &bundled, "sample", sources.clone(), None, None).unwrap();
        let runtime = RuntimeDirs::isolated_user_data_dir(root, "sample").unwrap();
        assert!(runtime.join("essay.txt").is_file());
        assert!(runtime.join("opencc/t2s.json").is_file());
        assert!(runtime.join("opencc/obsolete.ocd2").is_file());
        assert!(!runtime.join("opencc/unrelated.json").exists());
        put(&runtime, "opencc/edited.ocd2", "user edit");

        sources.insert("config.yaml".into(), b"sample: {translator: {dictionary: sample}}\n".to_vec());
        sources.insert("tables/extra.dict.yaml".into(), b"---\nname: tables/extra\nuse_preset_vocabulary: false\n...\n".to_vec());
        materialize(root, &bundled, "sample", sources, None, None).unwrap();
        assert!(!runtime.join("essay.txt").exists());
        assert!(!runtime.join("opencc/t2s.json").exists());
        assert!(!runtime.join("opencc/obsolete.ocd2").exists());
        assert_eq!(fs::read(runtime.join("opencc/edited.ocd2")).unwrap(), b"user edit");
        assert_eq!(fs::read(runtime.join("opencc/package.json")).unwrap(), b"package data");
    }

    pub(crate) fn hashes(root: &Path) -> BTreeMap<String, String> {
        let mut digests = BTreeMap::new();
        walk_files(root, "", |_| false, &mut |path, relative| {
            #[cfg(target_os = "macos")]
            let digest = {
                let md5 = std::process::Command::new("md5")
                    .arg("-q")
                    .arg(path)
                    .output()?;
                assert!(
                    md5.status.success(),
                    "{}",
                    String::from_utf8_lossy(&md5.stderr)
                );
                utf8(&md5.stdout)?.trim().to_owned()
            };
            #[cfg(not(target_os = "macos"))]
            let digest = sha256_reader(fs::File::open(path)?)?;
            digests.insert(relative.into(), digest);
            Ok(())
        })
        .unwrap();
        digests
    }

    #[test]
    fn local_redeploy_keeps_runtime_data_and_updates_untouched_sources() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let bundled = root.join("rime-data");
        let key = "local-example";
        let mut sources = fixture(root, key);
        let edited = [
            (
                "white_replace/A_custom_phrases.txt",
                "template\tab\n",
                "new phrase\tab\n",
            ),
            ("lua/TJ/stats.txt", "0", "24"),
            (
                "example.user.dict.yaml",
                "name: example.user\n...\n",
                "name: example.user\n...\n新\tab\n",
            ),
            ("dicts/user.dict", "initial", "edited"),
            ("phrases.txt", "to be cleared", ""),
        ];
        for (path, template, _) in edited {
            sources.insert(path.into(), template.as_bytes().to_vec());
        }
        sources.insert("images/.nomedia".into(), Vec::new());
        sources.insert("dicts/reference.txt".into(), b"older package".to_vec());
        let run = |inputs| {
            materialize(root, &bundled, key, inputs, None, None).unwrap();
        };
        run(sources.clone());
        let runtime = RuntimeDirs::isolated_user_data_dir(root, key).unwrap();
        put(&runtime, &format!("{COPY_STAGE}/orphan"), "interrupted copy");
        for (path, _, current) in edited {
            put(&runtime, path, current);
        }
        put(&runtime, "lua/TJ/generated.yaml", "daily: 7\n");
        put(&runtime, "example.userdb/CURRENT", "learned words");
        sources.insert("dicts/reference.txt".into(), b"updated package".to_vec());
        sources.remove("lua/TJ/stats.txt");

        let stage = prepare_runtime(
            root, &bundled, key, runtime_files(sources.clone()), None, None, true, false,
        )
        .unwrap()
        .unwrap();
        assert!(!runtime.join(COPY_STAGE).exists());
        let staged: BTreeMap<PathBuf, StagedInput> = serde_json::from_slice(
            &fs::read(stage.path().join(STAGED_INPUTS_MANIFEST)).unwrap(),
        )
        .unwrap();
        assert!(staged.contains_key(Path::new("dicts/reference.txt")));
        for (path, _, _) in edited {
            assert!(!staged.contains_key(Path::new(path)), "{path}");
        }
        drop(stage);

        for _ in 0..2 {
            run(sources.clone());
            for (path, _, expected) in edited {
                assert_eq!(
                    fs::read(runtime.join(path)).unwrap(),
                    expected.as_bytes(),
                    "{path}"
                );
            }
        }
        assert_eq!(
            fs::read(runtime.join("dicts/reference.txt")).unwrap(),
            b"updated package"
        );
        assert_eq!(
            fs::read(runtime.join("lua/TJ/generated.yaml")).unwrap(),
            b"daily: 7\n"
        );
        assert_eq!(
            fs::read(runtime.join("example.userdb/CURRENT")).unwrap(),
            b"learned words"
        );
        assert!(runtime.join("images/.nomedia").is_file());
    }

    #[test]
    fn windows_lua_compatibility_does_not_invalidate_the_deployment_receipt() {
        if crate::run_librime_test_in_child(
            "runtime_materialize::tests::windows_lua_compatibility_does_not_invalidate_the_deployment_receipt",
        ) {
            return;
        }
        struct Sink;
        impl crate::EventSink for Sink {
            fn emit(&self, _: crate::CoreEvent) {}
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let bundled = root.join("rime-data");
        let core = Core::new(
            crate::AppEnv {
                data_dir: root.to_owned(),
                cache_dir: root.join("cache"),
                resource_dir: root.to_owned(),
                app_version: "test".into(),
                platform: crate::Platform::Windows,
            },
            std::sync::Arc::new(Sink),
        )
        .unwrap();
        smoke_fixture::write(&bundled).unwrap();
        let schema = format!("{}.schema.yaml", smoke_fixture::SCHEMA_ID);
        let upstream_schema = fs::read_to_string(bundled.join(&schema)).unwrap().replace(
            "- table_translator",
            "- table_translator\n    - lua_translator@sample",
        );
        put(&bundled, &schema, &upstream_schema);
        put(&bundled, "rime.lua", "sample = require('sample')\n");
        put(&bundled, "lua/sample.lua", "return function() end\n");
        let upstream_helpers = concat!(
            "local function is_key(key, key_event)\n",
            "    local target_key = string.char(key_event.keycode) -- 当前按键对应字符\n\n",
            "    -- 若无目标键位，则交由其它函数进行判断\n",
            "    if not key then\n",
            "        return true\n",
            "    end\n",
            "    return target_key == key\n",
            "end\n",
        );
        put(&bundled, "lua/helpers.lua", upstream_helpers);
        let key = "builtin/keytao";
        put(
            root,
            RuntimeDirs::MIGRATION_MARKER,
            &serde_json::json!({"version": 1, "runtime_package": key}).to_string(),
        );
        let (targets, _stages) = prepare_deploy_targets(&core, root, &bundled, Some(key)).unwrap();
        keytao_core::isolated_deploy::deploy_packages(root, &bundled, None, &targets, |_| {})
            .unwrap();
        let runtime = RuntimeDirs::resolve(root, &bundled).unwrap().user_data_dir;
        assert!(keytao_core::schema_install_state_with_shared(root, &bundled).deployed);
        let changed = keytao_core::patch_windows_lua_compatibility(&runtime).unwrap();
        assert!(
            keytao_core::schema_install_state_with_shared(root, &bundled).deployed,
            "IME startup Lua compatibility invalidated the receipt: {changed:?}"
        );
        assert!(changed.is_empty());
        assert!(
            fs::read_to_string(runtime.join(&schema))
                .unwrap()
                .contains("lua_translator@*sample")
        );
        assert_eq!(
            fs::read_to_string(bundled.join(&schema)).unwrap(),
            upstream_schema
        );
        let helpers = fs::read_to_string(runtime.join("lua/helpers.lua")).unwrap();
        assert!(helpers.contains("keycode >= 0x7f"));
        assert_eq!(
            fs::read_to_string(bundled.join("lua/helpers.lua")).unwrap(),
            upstream_helpers
        );
        put(&runtime, "lua/helpers.lua", &(helpers + "-- local edit\n"));
        assert!(!keytao_core::schema_install_state_with_shared(root, &bundled).deployed);
        assert!(keytao_core::schema_build_is_available(root));
    }

    #[test]
    fn package_assets_and_overlays_stay_isolated_on_repeat() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let bundled = root.join("rime-data");
        put(&bundled, "default.yaml", "schema_list: []\n");
        put(&bundled, "symbols.yaml", "not used");
        put(&bundled, "opencc/owned.json", "not used");
        put(&bundled, "opencc/missing.json", "common");
        put(
            root,
            "default.custom.yaml",
            "patch: {schema_list: [{schema: old}], schema_list/@next: {schema: foreign}, schema_list/@0/schema: foreign, menu/page_size: 8}\n",
        );
        put(
            root,
            keytao_core::english_mode::SETTINGS_FILE,
            r#"{"candidatePageSize": 7}"#,
        );
        put(root, "squirrel.custom.yaml", "patch: {user: true}\n");
        put(root, "custom_phrase.txt", "user phrases");
        put(root, "private.dict.yaml", "user dictionary");
        put(root, "owned.dict.yaml", "package dictionary");
        put(root, "dictionary.userdb/no-copy.dict.yaml", "leveldb");
        put(root, "sync/no-copy.dict.yaml", "sync");
        let owners = BTreeMap::from([(
            "owned.dict.yaml".into(),
            BTreeSet::from([sha256_reader("package dictionary".as_bytes()).unwrap()]),
        )]);
        let legacy = user_files(root, &owners, &root.join("sync")).unwrap();
        assert_eq!(legacy.len(), 4);
        let overlay = Files::from([
            (
                "easy_en.schema.yaml".into(),
                b"schema: {schema_id: easy_en, name: Easy English}\n".to_vec(),
            ),
            ("easy_en.dict.yaml".into(), b"english".to_vec()),
            ("lua/easy_en.lua".into(), b"module".to_vec()),
        ]);
        for (key, english) in [("keytao", Some(&overlay)), ("keydo", None)] {
            let runtime = RuntimeDirs::isolated_user_data_dir(root, key).unwrap();
            let mut manifest = Manifest {
                package: key.into(),
                user_default: Some(utf8(&legacy[DEFAULT_CUSTOM[0]]).unwrap().into()),
                ..Default::default()
            };
            for (path, bytes) in &legacy {
                put(&runtime, path, utf8(bytes).unwrap());
                manifest.user_owned.insert(
                    path.clone(),
                    sha256_reader(bytes.as_slice()).unwrap(),
                );
            }
            put(&runtime, MANIFEST, &serde_json::to_string(&manifest).unwrap());
            materialize(root, &bundled, key, fixture(root, key), None, english).unwrap();
        }
        let ta = RuntimeDirs::for_package(root, &bundled, "keytao").unwrap();
        let kd = RuntimeDirs::for_package(root, &bundled, "keydo").unwrap();
        assert_ne!(ta.user_data_dir, kd.user_data_dir);
        for dirs in [&ta, &kd] {
            let key = dirs.active_package_key.as_deref().unwrap();
            let run = &dirs.user_data_dir;
            assert_eq!(
                fs::read_to_string(dirs.shared_data_dir.join("symbols.yaml")).unwrap(),
                key
            );
            assert_eq!(fs::read_to_string(run.join("symbols.yaml")).unwrap(), key);
            assert_eq!(
                fs::read_to_string(run.join("opencc/owned.json")).unwrap(),
                key
            );
            assert!(run.join(format!("{key}.schema.yaml")).is_file());
            assert_eq!(
                fs::read_to_string(run.join("opencc/missing.json")).unwrap(),
                "common"
            );
            assert_eq!(
                fs::read_to_string(run.join("private.dict.yaml")).unwrap(),
                "user dictionary"
            );
            assert_eq!(
                fs::read_to_string(run.join("custom_phrase.txt")).unwrap(),
                "user phrases"
            );
            assert_eq!(
                fs::read_to_string(run.join("squirrel.custom.yaml")).unwrap(),
                "patch: {user: true}\n"
            );
            assert!(!run.join("owned.dict.yaml").exists());
            assert!(!run.join("dictionary.userdb").exists());
            assert!(!run.join("sync").exists());
            let default = fs::read_to_string(run.join("default.custom.yaml")).unwrap();
            assert!(default.contains("menu/page_size: 8"));
            assert!(!default.contains("schema_list/"));
            assert!(
                fs::read_to_string(run.join(format!("{key}.custom.yaml")))
                    .unwrap()
                    .contains("menu/page_size: 7")
            );
            let manifest: Manifest =
                serde_json::from_slice(&fs::read(run.join(MANIFEST)).unwrap()).unwrap();
            assert!(manifest.user_owned.contains_key("squirrel.custom.yaml"));
            assert!(manifest.generated.contains_key("opencc/missing.json"));
            assert_eq!(
                manifest.owned.contains_key("easy_en.schema.yaml"),
                key == "keytao"
            );
            if key == "keytao" {
                assert_eq!(
                    keytao_core::parse_schema_list(&default),
                    ["keytao", "easy_en"]
                );
                assert!(run.join("easy_en.custom.yaml").is_file());
                assert!(!run.join("lua/keydo.lua").exists());
            } else {
                assert_eq!(keytao_core::parse_schema_list(&default), ["keydo"]);
                assert!(!run.join("easy_en.schema.yaml").exists());
                assert!(!run.join("lua/easy_en.lua").exists());
                assert!(!run.join("lua/keytao.lua").exists());
            }
        }
        put(&ta.user_data_dir, "dictionary.userdb/LOCK", "live leveldb");
        let before = hashes(&root.join("runtime"));
        materialize(
            root,
            &bundled,
            "keytao",
            fixture(root, "keytao"),
            None,
            Some(&overlay),
        )
        .unwrap();
        assert_eq!(hashes(&root.join("runtime")), before);

        put(
            &ta.user_data_dir,
            "squirrel.custom.yaml",
            "patch: {runtime_edit: true}\n",
        );
        let other_before = hashes(&kd.user_data_dir);
        materialize(
            root,
            &bundled,
            "keytao",
            fixture(root, "keytao"),
            None,
            None,
        )
        .unwrap();
        assert!(!ta.user_data_dir.join("easy_en.schema.yaml").exists());
        assert!(!ta.user_data_dir.join("easy_en.custom.yaml").exists());
        assert_eq!(
            fs::read_to_string(ta.user_data_dir.join("squirrel.custom.yaml")).unwrap(),
            "patch: {runtime_edit: true}\n"
        );
        assert_eq!(
            fs::read_to_string(ta.user_data_dir.join("dictionary.userdb/LOCK")).unwrap(),
            "live leveldb"
        );
        assert_eq!(hashes(&kd.user_data_dir), other_before);
        let edited = hashes(&ta.user_data_dir);
        materialize(
            root,
            &bundled,
            "keytao",
            fixture(root, "keytao"),
            None,
            None,
        )
        .unwrap();
        assert_eq!(hashes(&ta.user_data_dir), edited);
    }

    #[test]
    fn retries_with_new_inputs_replace_interrupted_generated_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let bundle = root.join("rime-data");
        let package = fixture(root, "keytao");
        let run = root.join("runtime/keytao");
        let interrupted = "require('previous attempt')\n";
        put(&run, "rime.lua", interrupted);
        let pending = Pending {
            previous: Manifest::default(),
            next: Manifest {
                package: "keytao".into(),
                generated: BTreeMap::from([(
                    "rime.lua".into(),
                    sha256_reader(interrupted.as_bytes()).unwrap(),
                )]),
                ..Default::default()
            },
        };
        fs::write(run.join(PENDING), serde_json::to_vec(&pending).unwrap()).unwrap();
        materialize(root, &bundle, "keytao", package, None, None).unwrap();
        assert_eq!(
            fs::read_to_string(run.join("rime.lua")).unwrap(),
            "require('keytao')\n"
        );
        assert!(!run.join(PENDING).exists());
    }

    #[test]
    fn legacy_default_recovers_user_values_without_the_flat_packages_patches() {
        let applied = serde_yaml::from_str(
            "patch: {schema_list: [{schema: a}], menu/page_size: 6, recognizer/patterns: from_a}\n",
        )
        .unwrap();
        let recovered = restore_user_default(
            b"patch: {schema_list: [{schema: a}], menu/page_size: 9, recognizer/patterns: from_a}\n",
            &applied,
            Some(b"patch: {menu/page_size: 7, ascii_composer: from_user}\n"),
        ).unwrap();
        let default = default_custom(
            Some(b"patch: {recognizer/patterns: from_b}\n"),
            Some(&recovered),
            &["b".into()],
        )
        .unwrap();
        let content = utf8(&default).unwrap();
        assert_eq!(keytao_core::parse_schema_list(content), ["b"]);
        assert!(content.contains("menu/page_size: 9"));
        assert!(content.contains("ascii_composer: from_user"));
        assert!(content.contains("recognizer/patterns: from_b"));
        assert!(!content.contains("from_a"));
    }

    #[test]
    fn edited_user_dictionary_and_other_runtime_files_survive_redeploy() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let bundled = root.join("rime-data");
        put(&bundled, "default.yaml", "schema_list: []\npunctuator: {}\n");
        put(&bundled, "symbols.yaml", "bundled symbols\n");
        put(&bundled, "key_bindings.yaml", "bundled bindings\n");
        put(&bundled, "punctuation.yaml", "bundled punctuation\n");
        put(
            root,
            crate::account::USER_DICTIONARY,
            "---\nname: keytao.user\nversion: \"1\"\n...\n原词\tab\n",
        );
        put(root, crate::account::USER_WORD_REMOVALS, "旧删词\tcd\n");

        let mut sources = fixture(root, "keytao");
        sources.remove("symbols.yaml");
        sources.insert(
            "keytao.schema.yaml".into(),
            b"schema: {schema_id: keytao, name: KeyTao}\ntranslator: {dictionary: keytao.extended}\n"
                .to_vec(),
        );
        sources.insert(
            "keytao.extended.dict.yaml".into(),
            b"---\nname: keytao.extended\nimport_tables:\n  - keytao.phrase\n...\n".to_vec(),
        );

        materialize(root, &bundled, "keytao", sources.clone(), None, None).unwrap();
        let runtime = RuntimeDirs::isolated_user_data_dir(root, "keytao").unwrap();
        assert_eq!(
            fs::read_to_string(runtime.join(crate::account::USER_DICTIONARY)).unwrap(),
            "---\nname: keytao.user\nversion: \"1\"\n...\n原词\tab\n"
        );
        assert!(
            fs::read_to_string(runtime.join("keytao.extended.dict.yaml"))
                .unwrap()
                .contains("- keytao.user")
        );

        // Untouched runtime copy still follows control_root updates (e.g. dictionary manager / cloud sync).
        put(
            root,
            crate::account::USER_DICTIONARY,
            "---\nname: keytao.user\nversion: \"1\"\n...\n同步词\tef\n",
        );
        materialize(root, &bundled, "keytao", sources.clone(), None, None).unwrap();
        assert_eq!(
            fs::read_to_string(runtime.join(crate::account::USER_DICTIONARY)).unwrap(),
            "---\nname: keytao.user\nversion: \"1\"\n...\n同步词\tef\n"
        );

        // User edits keytao.user.dict.yaml, keytao.user.remove.txt, bundled YAMLs, schema, dict, lua and custom files.
        let edited_user_dict = "---\nname: keytao.user\nversion: \"1\"\n...\n手改词\tgh\n";
        let edited_removals = "手改删词\tij\n";
        let edited_symbols = "user edited symbols\n";
        let edited_bindings = "user edited bindings\n";
        let edited_schema =
            "schema: {schema_id: keytao, name: KeyTao Custom}\ntranslator: {dictionary: keytao.extended}\n";
        let edited_extended =
            "---\nname: keytao.extended\nimport_tables:\n  - keytao.user\n  - keytao.phrase\n  - extra_user\n...\n";
        let edited_rime_lua = "require('keytao')\n-- user lua\n";
        let edited_filter_lua = "-- custom remove filter\n";
        put(&runtime, crate::account::USER_DICTIONARY, edited_user_dict);
        put(&runtime, crate::account::USER_WORD_REMOVALS, edited_removals);
        put(&runtime, "symbols.yaml", edited_symbols);
        put(&runtime, "key_bindings.yaml", edited_bindings);
        put(&runtime, "keytao.schema.yaml", edited_schema);
        put(&runtime, "keytao.extended.dict.yaml", edited_extended);
        put(&runtime, "rime.lua", edited_rime_lua);
        put(&runtime, crate::remove_filter::SCRIPT_PATH, edited_filter_lua);
        put(&runtime, "extra_user.dict.yaml", "---\nname: extra_user\n...\n扩展\tkl\n");
        put(&runtime, "keytao.custom.yaml", "");
        put(&runtime, "default.custom.yaml", "");

        // Staged deploy preparation and direct materialization both succeed and keep all user edits.
        let stage = prepare_runtime(
            root,
            &bundled,
            "keytao",
            runtime_files(sources.clone()),
            None,
            None,
            true,
            false,
        )
        .unwrap();
        drop(stage);

        for _ in 0..2 {
            materialize(root, &bundled, "keytao", sources.clone(), None, None).unwrap();
            assert_eq!(
                fs::read_to_string(runtime.join(crate::account::USER_DICTIONARY)).unwrap(),
                edited_user_dict
            );
            assert_eq!(
                fs::read_to_string(runtime.join(crate::account::USER_WORD_REMOVALS)).unwrap(),
                edited_removals
            );
            assert_eq!(
                fs::read_to_string(runtime.join("symbols.yaml")).unwrap(),
                edited_symbols
            );
            assert_eq!(
                fs::read_to_string(runtime.join("key_bindings.yaml")).unwrap(),
                edited_bindings
            );
            assert_eq!(
                fs::read_to_string(runtime.join("keytao.schema.yaml")).unwrap(),
                edited_schema
            );
            assert_eq!(
                fs::read_to_string(runtime.join("keytao.extended.dict.yaml")).unwrap(),
                edited_extended
            );
            assert_eq!(
                fs::read_to_string(runtime.join("rime.lua")).unwrap(),
                edited_rime_lua
            );
            assert_eq!(
                fs::read_to_string(runtime.join(crate::remove_filter::SCRIPT_PATH)).unwrap(),
                edited_filter_lua
            );
            assert_eq!(
                fs::read_to_string(runtime.join("extra_user.dict.yaml")).unwrap(),
                "---\nname: extra_user\n...\n扩展\tkl\n"
            );
            assert!(
                keytao_core::parse_schema_list(
                    &fs::read_to_string(runtime.join("default.custom.yaml")).unwrap()
                ) == ["keytao"]
            );
        }

        // Deleting the runtime override falls back to control_root cleanly.
        fs::remove_file(runtime.join(crate::account::USER_DICTIONARY)).unwrap();
        materialize(root, &bundled, "keytao", sources, None, None).unwrap();
        assert_eq!(
            fs::read_to_string(runtime.join(crate::account::USER_DICTIONARY)).unwrap(),
            "---\nname: keytao.user\nversion: \"1\"\n...\n同步词\tef\n"
        );
    }

    #[test]
    fn runtime_created_user_dictionary_without_control_root_patches_extended_import() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let bundled = root.join("rime-data");
        let mut sources = fixture(root, "keytao");
        sources.insert(
            "keytao.schema.yaml".into(),
            b"schema: {schema_id: keytao, name: KeyTao}\ntranslator: {dictionary: keytao.extended}\n"
                .to_vec(),
        );
        sources.insert(
            "keytao.extended.dict.yaml".into(),
            b"---\nname: keytao.extended\nimport_tables:\n  - keytao.phrase\n...\n".to_vec(),
        );

        materialize(root, &bundled, "keytao", sources.clone(), None, None).unwrap();
        let runtime = RuntimeDirs::isolated_user_data_dir(root, "keytao").unwrap();
        assert!(!runtime.join(crate::account::USER_DICTIONARY).exists());

        let created = "---\nname: keytao.user\nversion: \"1\"\n...\n新建词\tmn\n";
        put(&runtime, crate::account::USER_DICTIONARY, created);
        materialize(root, &bundled, "keytao", sources, None, None).unwrap();

        assert_eq!(
            fs::read_to_string(runtime.join(crate::account::USER_DICTIONARY)).unwrap(),
            created
        );
        assert!(
            fs::read_to_string(runtime.join("keytao.extended.dict.yaml"))
                .unwrap()
                .contains("- keytao.user")
        );
    }

    #[test]
    fn local_scheme_preserves_edited_yaml_and_user_added_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let bundled = root.join("rime-data");
        let key = "local-editable";
        let sources = fixture(root, key);

        materialize(root, &bundled, key, sources.clone(), None, None).unwrap();
        let runtime = RuntimeDirs::isolated_user_data_dir(root, key).unwrap();

        let edited_schema = format!("schema: {{schema_id: {key}, name: Local Edited}}\n");
        let added_dict = "---\nname: my_custom\n...\n自定\tab\n";
        let added_custom = "patch:\n  menu/page_size: 9\n";
        put(&runtime, &format!("{key}.schema.yaml"), &edited_schema);
        put(&runtime, "my_custom.dict.yaml", added_dict);
        put(&runtime, &format!("{key}.custom.yaml"), added_custom);

        materialize(root, &bundled, key, sources, None, None).unwrap();
        assert_eq!(
            fs::read_to_string(runtime.join(format!("{key}.schema.yaml"))).unwrap(),
            edited_schema
        );
        assert_eq!(
            fs::read_to_string(runtime.join("my_custom.dict.yaml")).unwrap(),
            added_dict
        );
        assert!(
            fs::read_to_string(runtime.join(format!("{key}.custom.yaml")))
                .unwrap()
                .contains("menu/page_size: 9")
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_runtime_is_rejected_without_writing_through() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let target = root.join("target");
        fs::create_dir_all(&target).unwrap();
        fs::create_dir(root.join("runtime")).unwrap();
        std::os::unix::fs::symlink(&target, root.join("runtime/keytao")).unwrap();
        let package = fixture(root, "keytao");
        assert!(
            materialize(
                root,
                &root.join("rime-data"),
                "keytao",
                package,
                None,
                None,
            )
            .is_err()
        );
        assert_eq!(fs::read_dir(target).unwrap().count(), 0);
    }
}
