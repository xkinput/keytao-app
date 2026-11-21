use crate::{
    Core, CoreError, migration_userdb, package_activation,
    package_store::{PackageStore, StoredPackage},
    packages::safe_path,
    runtime_materialize,
};
use keytao_core::{DeployStep, ReloadStamp, RuntimeDirs, isolated_deploy};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
};

const VERSION: u32 = 1;
pub(crate) const BACKUP_DIR: &str = "legacy-backup";
const BACKUP_PENDING: &str = "runtime/.keytao-backup-pending";
const BACKUP_COMPLETE: &str = "runtime/.keytao-backup-complete";
static MIGRATED_HERE: LazyLock<Mutex<BTreeSet<PathBuf>>> = LazyLock::new(Mutex::default);

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
struct Migration {
    version: u32,
    active_package: String,
    runtime_package: String,
    packages: Vec<String>,
    user_files: Vec<String>,
    legacy_user_databases: Vec<String>,
    #[serde(default)]
    build_verified: bool,
    #[serde(default)]
    sync_paths: Vec<PathBuf>,
}

#[derive(Deserialize, Serialize)]
struct BackupJournal {
    timestamp: String,
    sync_paths: Vec<PathBuf>,
}

pub async fn ensure_before_write(core: &Core, root: &Path) -> Result<(), CoreError> {
    let shared = crate::deploy::deploy_shared_data_dir(core, root);
    migrate_if_needed(core, root, &shared).map(|_| ())
}

pub fn start_app(core: &Core, root: &Path) -> Result<(), CoreError> {
    let shared = crate::deploy::deploy_shared_data_dir(core, root);
    if !RuntimeDirs::migration_complete(root) {
        let result = migrate_if_needed(core, root, &shared);
        #[cfg(target_os = "windows")]
        if matches!(result, Ok(true))
            && crate::windows_public_schemas::is_default_user_root(root)
            && let Err(error) = crate::windows_public_schemas::publish_active_if_needed(root)
        {
            crate::windows_public_schemas::public_warning("migration", &error);
        }
        return result.map(|_| ());
    }
    let root = checked_root(root)?;
    let result = isolated_deploy::with_migration_lock(&root, || {
        if MIGRATED_HERE
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains(&root)
        {
            return Ok(());
        }
        // Older markers were published before the first isolated build.
        let marker: Migration = serde_json::from_slice(
            &fs::read(marker_path(&root).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        let active = RuntimeDirs::for_package(&root, &shared, &marker.runtime_package);
        if marker.build_verified
            || active.is_some_and(|dirs| isolated_deploy::package_build_ready(&root, &dirs))
        {
            let sync_dir = checked_sync_dir(core, &root).map_err(|error| error.to_string())?;
            let mut sync_paths = marker.sync_paths;
            sync_paths.extend([
                sync_dir,
                lexical_sync_path(&root).map_err(|error| error.to_string())?,
            ]);
            backup_legacy(&root, sync_paths).map_err(|error| error.to_string())?;
        }
        Ok(())
    });
    if let Err(error) = result {
        tracing::warn!(%error, "Legacy directory backup will retry on startup");
    }
    Ok(())
}

pub fn migrate_if_needed(core: &Core, root: &Path, bundled: &Path) -> Result<bool, CoreError> {
    migrate_with(core, root, bundled, |root, bundled, key| {
        isolated_deploy::deploy_packages(
            root,
            bundled,
            Some(&core.env().data_dir),
            &[isolated_deploy::PackageDeployTarget {
                package_key: key.to_owned(),
                package_name: runtime_materialize::package_name(core, key),
                materialize_error: None,
                staged_inputs: None,
            }],
            |_| {},
        )
    })
}

fn migrate_with(
    core: &Core,
    root: &Path,
    bundled: &Path,
    deploy: impl FnOnce(&Path, &Path, &str) -> Result<(), String>,
) -> Result<bool, CoreError> {
    if !RuntimeDirs::needs_migration(root) {
        return Ok(false);
    }
    let result = (|| {
        let root = checked_root(root)?;
        isolated_deploy::with_migration_lock(&root, || {
            if !RuntimeDirs::needs_migration(&root) {
                return Ok(false);
            }
            crate::deploy::emit_legacy_progress(core, DeployStep::Compile, "正在迁移方案目录…");
            let result = migrate_to_isolated(core, &root, bundled, deploy);
            crate::deploy::emit_legacy_progress(
                core,
                if result.is_ok() {
                    DeployStep::Done
                } else {
                    DeployStep::Failed
                },
                if result.is_ok() {
                    ""
                } else {
                    "迁移失败，重试"
                },
            );
            result.map_err(|error| error.to_string())?;
            MIGRATED_HERE
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(root.clone());
            Ok(true)
        })
        .map_err(CoreError::Other)
    })();
    result.map_err(|error: CoreError| {
        tracing::error!(%error, "Package directory migration failed");
        CoreError::Other("迁移失败，重试".into())
    })
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub fn migrate_default_if_needed(root: &Path, bundled: &Path) -> Result<bool, CoreError> {
    if !RuntimeDirs::needs_migration(root) {
        return Ok(false);
    }
    struct Sink;
    impl crate::EventSink for Sink {
        fn emit(&self, _: crate::CoreEvent) {}
    }
    let data_dir = dirs::data_local_dir()
        .ok_or_else(|| CoreError::Other("Cannot determine app data directory".into()))?
        .join("ink.rea.keytao-app");
    let core = Core::new(
        crate::AppEnv {
            cache_dir: data_dir.join("cache"),
            data_dir,
            resource_dir: bundled.to_owned(),
            app_version: env!("CARGO_PKG_VERSION").into(),
            #[cfg(target_os = "linux")]
            platform: crate::Platform::Linux,
            #[cfg(target_os = "macos")]
            platform: crate::Platform::MacOs,
            #[cfg(target_os = "windows")]
            platform: crate::Platform::Windows,
        },
        std::sync::Arc::new(Sink),
    )?;
    migrate_if_needed(&core, root, bundled)
}

fn migrate_to_isolated(
    core: &Core,
    root: &Path,
    bundled: &Path,
    deploy: impl FnOnce(&Path, &Path, &str) -> Result<(), String>,
) -> Result<(), CoreError> {
    package_activation::recover(core)?;
    let sync_dir = checked_sync_dir(core, root)?;
    let path = marker_path(root)?;
    if path.try_exists()? {
        return Err(CoreError::Other("Invalid package migration marker".into()));
    }
    crate::addon_store::project_legacy_for_isolation(core, root)?;
    let legacy = runtime_materialize::legacy_user_files(core, root)?;
    let mut migration = plan(core, root, &legacy)?;
    let key = &migration.runtime_package;
    let runtimes = runtime_materialize::materialize_migration(core, root, bundled, key, &legacy)?;
    let active = runtimes
        .iter()
        .find(|dirs| dirs.active_package_key.as_ref() == Some(key))
        .ok_or_else(|| CoreError::Other("Missing active package runtime".into()))?;
    let legacy_user = safe_path(root, "user.yaml").map_err(CoreError::Other)?;
    if legacy_user.is_file() {
        let user = safe_path(&active.user_data_dir, "user.yaml").map_err(CoreError::Other)?;
        crate::scheme_files::write_file_atomic(&user, &fs::read(legacy_user)?)?;
    }
    migration_userdb::migrate(root, &runtimes, &sync_dir)?;
    deploy(root, bundled, key).map_err(CoreError::Other)?;
    migration.build_verified = true;
    migration.sync_paths = vec![sync_dir, lexical_sync_path(root)?];
    safe_path(root, keytao_core::RELOAD_STAMP_FILE_NAME).map_err(CoreError::Other)?;
    ReloadStamp::write_package(root, key).map_err(CoreError::Other)?;
    // Changing the marker also changes the reload signature.
    crate::scheme_files::write_file_atomic(&path, &serde_json::to_vec(&migration)?)?;
    Ok(())
}

fn checked_sync_dir(core: &Core, root: &Path) -> Result<PathBuf, CoreError> {
    let sync_dir = runtime_materialize::legacy_sync_dir(root)?;
    let store = PackageStore::new(core).root();
    let canonical_store = core
        .env()
        .data_dir
        .canonicalize()
        .ok()
        .map(|data| keytao_core::package_store::root(&data));
    for protected in [
        root.join("runtime"),
        root.join("scheme-packages"),
        root.join(BACKUP_DIR),
        ReloadStamp::path(root),
        store,
    ]
    .into_iter()
    .chain(canonical_store)
    {
        if protected.starts_with(&sync_dir) || sync_dir.starts_with(&protected) {
            return Err(CoreError::Other("同步目录与方案包迁移目录重叠".into()));
        }
    }
    Ok(sync_dir)
}

fn backup_legacy(root: &Path, mut sync_paths: Vec<PathBuf>) -> Result<(), CoreError> {
    let pending = safe_path(root, BACKUP_PENDING).map_err(CoreError::Other)?;
    let complete = safe_path(root, BACKUP_COMPLETE).map_err(CoreError::Other)?;
    if complete.is_file() {
        return Ok(());
    }
    let timestamp = match fs::read(&pending) {
        Ok(bytes) => {
            let journal: BackupJournal = serde_json::from_slice(&bytes)?;
            sync_paths.extend(journal.sync_paths);
            journal.timestamp
        }
        Err(error) if error.kind() == ErrorKind::NotFound => backup_timestamp(),
        Err(error) => return Err(error.into()),
    };
    sync_paths.sort();
    sync_paths.dedup();
    let backup = backup_path(root, &timestamp)?;
    let journal = BackupJournal {
        timestamp,
        sync_paths,
    };
    crate::scheme_files::write_file_atomic(&pending, &serde_json::to_vec(&journal)?)?;
    fs::create_dir_all(&backup)?;
    move_legacy_entries(root, root, &backup, &journal.sync_paths)?;
    fs::rename(pending, complete)?;
    Ok(())
}

fn backup_timestamp() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}.{:09}Z",
        now.year(),
        now.month() as u8,
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        now.nanosecond(),
    )
}

fn lexical_sync_path(root: &Path) -> Result<PathBuf, CoreError> {
    let configured = crate::rime_sync::read_settings(root).map_err(CoreError::Other)?;
    let configured_path = if configured.sync_dir.is_empty() {
        root.join("sync")
    } else {
        root.join(configured.sync_dir)
    };
    Ok(std::path::absolute(configured_path)?
        .components()
        .fold(PathBuf::new(), |mut path, part| {
            if part == std::path::Component::ParentDir {
                path.pop();
            } else {
                path.push(part);
            }
            path
        }))
}

fn backup_path(root: &Path, timestamp: &str) -> Result<PathBuf, CoreError> {
    if !is_backup_timestamp(timestamp) {
        return Err(CoreError::Other("Invalid legacy backup timestamp".into()));
    }
    safe_path(root, &format!("{BACKUP_DIR}/{timestamp}")).map_err(CoreError::Other)
}

fn is_backup_timestamp(timestamp: &str) -> bool {
    timestamp.len() == 26
        && timestamp
            .bytes()
            .enumerate()
            .all(|(index, byte)| match index {
                8 => byte == b'T',
                15 => byte == b'.',
                25 => byte == b'Z',
                _ => byte.is_ascii_digit(),
            })
}

pub(crate) fn completed_backups(root: &Path) -> Result<Vec<PathBuf>, CoreError> {
    let complete = safe_path(root, BACKUP_COMPLETE).map_err(CoreError::Other)?;
    if !complete.is_file() {
        return Ok(Vec::new());
    }
    let parent = safe_path(root, BACKUP_DIR).map_err(CoreError::Other)?;
    let mut backups = Vec::new();
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(backups),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && entry.file_name().to_str().is_some_and(is_backup_timestamp)
        {
            backups.push(entry.path());
        }
    }
    backups.sort();
    Ok(backups)
}

fn move_legacy_entries(
    root: &Path,
    source: &Path,
    destination: &Path,
    sync_dirs: &[PathBuf],
) -> Result<(), CoreError> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let name = entry.file_name();
        if sync_dirs.iter().any(|dir| from.starts_with(dir))
            || (root == source && control_entry(&name.to_string_lossy()))
        {
            continue;
        }
        let to = destination.join(name);
        if sync_dirs.iter().any(|dir| dir.starts_with(&from)) {
            if entry.file_type()?.is_dir() {
                match fs::create_dir(&to) {
                    Ok(()) => {}
                    Err(error)
                        if error.kind() == ErrorKind::AlreadyExists
                            && fs::symlink_metadata(&to)?.is_dir() => {}
                    Err(error) => return Err(error.into()),
                }
                move_legacy_entries(root, &from, &to, sync_dirs)?;
            }
            continue;
        }
        let to = match fs::symlink_metadata(&to) {
            Err(error) if error.kind() == ErrorKind::NotFound => to,
            Err(error) => return Err(error.into()),
            Ok(_) => {
                // A delayed legacy writer may have recreated an already archived path.
                let retry = backup_path(root, &backup_timestamp())?;
                fs::create_dir(&retry)?;
                let to = retry.join(from.strip_prefix(root).unwrap());
                fs::create_dir_all(to.parent().unwrap())?;
                to
            }
        };
        crate::package_fs::rename(&from, &to)?;
    }
    Ok(())
}

pub(crate) fn control_entry(name: &str) -> bool {
    matches!(
        name,
        "runtime"
            | "scheme-packages"
            | BACKUP_DIR
            | "log"
            | "logs"
            | "sync"
            | "rime-data"
            | "shared"
            | "trash"
            | "cache"
            | "installation.yaml"
            | "theme.yaml"
            | "keyboard.yaml"
            | "keyboard-layout.yaml"
            | "libjd"
            | "runtime-log.json"
            | "android_ime.json"
            | "ios_ime.json"
            | "keytao-librime-upgrade.lock"
            | "keytao-librime.version"
            | "keytao-ime-engine.json"
            | ".rime-data.lock"
            | ".rime-data.staging"
            | ".rime-data.previous"
            | "auth-tokens.json"
            | keytao_core::RELOAD_STAMP_FILE_NAME
            | keytao_core::english_mode::SETTINGS_FILE
            | RuntimeDirs::SETTINGS_FILE
            | crate::account::USER_DICTIONARY
            | "keytao.user.remove.txt"
    ) || name.starts_with("app-state.json")
        || (name.starts_with(".keytao-")
            && name != ".keytao-package-lua"
            && !name.starts_with(".keytao-rime-user-"))
}

fn plan(
    core: &Core,
    root: &Path,
    legacy: &BTreeMap<String, Vec<u8>>,
) -> Result<Migration, CoreError> {
    let activations = package_activation::read_all(core, root)?;
    let store = PackageStore::new(core);
    let installed = runtime_materialize::installed_main_packages(core, &activations);
    let addons = runtime_materialize::installed_addons(&store)?;
    let legacy_addons = crate::addon_store::unprojected_legacy_addons(core, root)?;
    let mut packages = installed.clone();
    packages.extend(addons.iter().map(|key| (*key).to_owned()));
    packages.extend(legacy_addons.keys().cloned());
    packages.remove(crate::addon::EASY_EN_ADDON_ID);
    let schema = legacy_schema(root)?;
    let source = schema
        .as_deref()
        .map(|schema| format!("{schema}.schema.yaml"));
    let mut active = source.as_ref().and_then(|source| {
        activations
            .iter()
            .find_map(|(key, record)| record.manifest.contains_key(source).then(|| key.clone()))
    });
    if let Some(source) = source.as_ref().filter(|_| active.is_none()) {
        for key in &installed {
            if store
                .read_meta(key)?
                .is_some_and(|meta| meta.manifest.contains_key(source))
            {
                active = Some(key.clone());
                break;
            }
        }
    }
    if let Some(source) = source.filter(|_| active.is_none()) {
        for key in addons
            .into_iter()
            .filter(|key| *key != crate::addon::EASY_EN_ADDON_ID)
        {
            let path = safe_path(&store.checked_addon_path(key)?, "package.json")
                .map_err(CoreError::Other)?;
            let metadata: StoredPackage = serde_json::from_slice(&fs::read(path)?)?;
            if metadata.files.iter().any(|file| file.path == source) {
                active = Some(key.to_owned());
                break;
            }
        }
        if active.is_none() {
            active = legacy_addons.into_iter().find_map(|(key, files)| {
                (key != crate::addon::EASY_EN_ADDON_ID
                    && files.iter().any(|file| file.path == source))
                .then_some(key)
            });
        }
    }
    let active_package = active
        .or_else(|| activations.into_keys().next())
        .unwrap_or_else(|| "keytao".into());
    let runtime_package = if active_package == "keytao" && !packages.contains("keytao") {
        "builtin/keytao".to_owned()
    } else {
        active_package.clone()
    };
    packages.insert(runtime_package.clone());
    Ok(Migration {
        version: VERSION,
        active_package,
        runtime_package,
        packages: packages.into_iter().collect(),
        user_files: legacy.keys().cloned().collect(),
        legacy_user_databases: migration_userdb::legacy_dictionary_names(root)?
            .into_iter()
            .collect(),
        build_verified: false,
        sync_paths: Vec::new(),
    })
}

fn legacy_schema(root: &Path) -> Result<Option<String>, CoreError> {
    let path = safe_path(root, "user.yaml").map_err(CoreError::Other)?;
    match fs::read(path) {
        Ok(bytes) => {
            let document: serde_yaml::Value = serde_yaml::from_slice(&bytes)
                .map_err(|error| CoreError::Other(error.to_string()))?;
            Ok(document["var"]["previously_selected_schema"]
                .as_str()
                .map(str::to_owned))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn checked_root(root: &Path) -> Result<PathBuf, CoreError> {
    if !fs::symlink_metadata(root)?.is_dir() {
        return Err(CoreError::Other("迁移目录必须是普通目录".into()));
    }
    Ok(std::path::absolute(root)?)
}

fn marker_path(root: &Path) -> Result<PathBuf, CoreError> {
    safe_path(root, RuntimeDirs::MIGRATION_MARKER).map_err(CoreError::Other)
}

#[cfg(test)]
#[path = "../../keytao-core/tests/support/smoke_fixture.rs"]
mod smoke_fixture;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AppEnv, AppState, CoreEvent, EventSink, Platform,
        package_activation::Activation,
        package_store::{PackageFile, StoredPackage},
        packages::{PackageMeta, sha256_reader},
        runtime_materialize::test_hashes,
        state::InstalledPackage,
    };
    use std::sync::{Arc, Mutex};

    struct Sink;
    impl EventSink for Sink {
        fn emit(&self, _: CoreEvent) {}
    }

    fn fixture(root: &Path) -> Core {
        Core {
            env: AppEnv {
                data_dir: root.to_owned(),
                cache_dir: root.join("cache"),
                resource_dir: root.join("resources"),
                app_version: "test".into(),
                platform: Platform::MacOs,
            },
            sink: Arc::new(Sink),
            state: Mutex::new(AppState::default()),
            download_cancel: tokio::sync::watch::channel(0).0,
            recovered_state_file: None,
            pending_package: Mutex::new(()),
            user_words: Mutex::new(crate::user_words::UserWordsState::default()),
        }
    }

    fn put(root: &Path, relative: &str, bytes: &[u8]) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn package(core: &Core, key: &str, schema: &str, dictionary: &str) -> Activation {
        let store = PackageStore::new(core);
        let dir = store.checked_package_path(key).unwrap();
        let schema_file = format!("{schema}.schema.yaml");
        let text = format!(
            "schema: {{schema_id: {schema}, name: {schema}}}\ntranslator: {{dictionary: {dictionary}}}\n"
        );
        put(&dir, &format!("files/{schema_file}"), text.as_bytes());
        let sha256 = sha256_reader(text.as_bytes()).unwrap();
        let manifest = BTreeMap::from([(schema_file.clone(), sha256.clone())]);
        let metadata = StoredPackage {
            key: key.into(),
            origin: None,
            installed_at: 0,
            files: vec![PackageFile {
                path: schema_file,
                sha256,
                size: text.len() as u64,
            }],
            variant: None,
            grammar: None,
            addon_manifest: None,
            selected_schemas: None,
            meta: PackageMeta {
                scheme_key: key.into(),
                engine: Default::default(),
                version: None,
                installed_at_unix: 0,
                source: "local".into(),
                display_name: None,
                selection_base_version: None,
                manifest: manifest.clone(),
                zip_sha256: "0".repeat(64),
            },
        };
        put(
            &dir,
            "package.json",
            &serde_json::to_vec(&metadata).unwrap(),
        );
        core.lock_state()
            .installed_packages
            .insert(key.into(), InstalledPackage::default());
        Activation {
            key: key.into(),
            root: core
                .env()
                .data_dir
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            scheme_key: key.into(),
            manifest,
            ..Default::default()
        }
    }

    #[test]
    fn migration_is_idempotent_and_archives_on_the_next_start() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let core = fixture(root);
        let a = package(&core, "local-a", "alpha", "alpha");
        let z = package(&core, "local-z", "zulu", "keytao");
        package_activation::write(&core, &a).unwrap();
        package_activation::write(&core, &z).unwrap();
        put(
            root,
            "user.yaml",
            b"var: {previously_selected_schema: zulu}\n",
        );
        put(root, "default.custom.yaml", b"patch: {menu/page_size: 7}\n");
        put(
            root,
            "squirrel.custom.yaml",
            b"patch: {style/color_scheme: custom}\n",
        );
        put(root, "custom_phrase.txt", b"phrase\tcode\n");
        put(root, "personal.dict.yaml", b"---\nname: personal\n...\n");
        put(
            root,
            "keytao.user.dict.yaml",
            b"---\nname: keytao.user\n...\n",
        );
        put(root, "build/legacy.bin", b"legacy build");
        put(root, "nested/sync/private.yaml", b"keep sync");
        put(root, "nested/old.yaml", b"archive sibling");
        put(root, "log/rime.log", b"keep log");
        put(
            root,
            "installation.yaml",
            format!("sync_dir: {:?}\n", root.join("nested/sync")).as_bytes(),
        );
        let before = test_hashes(root);
        assert_eq!(
            RuntimeDirs::resolve(root, root).unwrap(),
            RuntimeDirs::flat(root, root)
        );
        assert!(
            migrate_with(&core, root, root, |_, _, key| {
                assert_eq!(key, "local-z");
                Ok(())
            })
            .unwrap()
        );
        assert!(!migrate_with(&core, root, root, |_, _, _| panic!("already migrated")).unwrap());
        assert_eq!(
            ReloadStamp::package_key(&ReloadStamp::path(root)).as_deref(),
            Some("local-z")
        );
        assert_eq!(
            RuntimeDirs::resolve(root, root).unwrap().user_data_dir,
            root.join("runtime/local-z")
        );
        for key in ["local-a", "local-z"] {
            let runtime = root.join("runtime").join(key);
            for name in [
                "squirrel.custom.yaml",
                "custom_phrase.txt",
                "personal.dict.yaml",
            ] {
                assert_eq!(
                    fs::read(runtime.join(name)).unwrap(),
                    fs::read(root.join(name)).unwrap()
                );
            }
            assert_eq!(
                runtime.join("keytao.user.dict.yaml").is_file(),
                key == "local-z"
            );
        }
        start_app(&core, root).unwrap();
        assert!(root.join("build/legacy.bin").is_file());
        MIGRATED_HERE.lock().unwrap().remove(root);
        start_app(&core, root).unwrap();
        let [backup] = completed_backups(root).unwrap().try_into().unwrap();
        let archived = test_hashes(&backup);
        let after = test_hashes(root);
        for (path, digest) in before {
            if control_entry(path.split('/').next().unwrap()) || path.starts_with("nested/sync/") {
                assert_eq!(after[&path], digest, "{path}");
            } else {
                assert!(!root.join(&path).exists(), "{path}");
                assert_eq!(archived[&path], digest, "{path}");
            }
        }
        assert!(fs::read_dir(root).unwrap().all(|entry| {
            let name = entry.unwrap().file_name();
            control_entry(&name.to_string_lossy()) || name == "nested"
        }));
        let report = crate::package_orphans::report(&core, root).unwrap();
        let entry = report
            .files
            .iter()
            .find(|entry| entry.path.starts_with(BACKUP_DIR))
            .unwrap();
        assert_eq!(root.join(&entry.path), backup);
        assert!(!entry.path.ends_with("legacy.bin"));
    }

    #[test]
    fn migration_builds_the_active_package_before_publishing() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let bundled = root.join("rime-data");
        smoke_fixture::write(&bundled).unwrap();
        let core = fixture(root);
        put(root, "build/legacy.bin", b"legacy build");
        assert!(migrate_if_needed(&core, root, &bundled).unwrap());
        let dirs = RuntimeDirs::resolve(root, &bundled).unwrap();
        assert!(isolated_deploy::package_build_ready(root, &dirs));
        assert_eq!(
            fs::read(root.join("build/legacy.bin")).unwrap(),
            b"legacy build"
        );
    }

    #[test]
    fn interrupted_backup_resumes_without_replacing_moved_files() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let stamp = "20261121T010203.000000000Z";
        put(
            root,
            BACKUP_PENDING,
            &serde_json::to_vec(&BackupJournal {
                timestamp: stamp.into(),
                sync_paths: vec![root.join("original-sync")],
            })
            .unwrap(),
        );
        let backup = backup_path(root, stamp).unwrap();
        put(&backup, "build/legacy.bin", b"already moved");
        put(root, "build/legacy.bin", b"recreated by old IME");
        put(root, "nested/custom.yaml", b"still pending");
        put(root, "sync/data.txt", b"keep sync");
        put(root, "original-sync/data.txt", b"keep original sync");
        backup_legacy(root, vec![root.join("sync")]).unwrap();
        let backups = completed_backups(root).unwrap();
        assert_eq!(backups.len(), 2);
        let recreated = backups.iter().find(|path| **path != backup).unwrap();
        assert_eq!(
            fs::read(recreated.join("build/legacy.bin")).unwrap(),
            b"recreated by old IME"
        );
        assert_eq!(
            fs::read(backup.join("build/legacy.bin")).unwrap(),
            b"already moved"
        );
        assert_eq!(
            fs::read(backup.join("nested/custom.yaml")).unwrap(),
            b"still pending"
        );
        assert_eq!(fs::read(root.join("sync/data.txt")).unwrap(), b"keep sync");
        assert_eq!(
            fs::read(root.join("original-sync/data.txt")).unwrap(),
            b"keep original sync"
        );
        assert!(!root.join("nested").exists());
        assert!(!root.join(BACKUP_PENDING).exists());
    }

    #[test]
    fn migration_stays_flat_until_deploy_and_reload_succeed() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let core = fixture(root);
        let activation = package(&core, "local-a", "alpha", "alpha");
        package_activation::write(&core, &activation).unwrap();
        put(root, "build/legacy.bin", b"still usable");
        let stamp = ReloadStamp::path(root);
        let failure = migrate_with(&core, root, root, |root, _, key| {
            assert_eq!(key, "local-a");
            assert!(RuntimeDirs::needs_migration(root));
            assert!(!stamp.exists());
            Err("fixture deployment failed".into())
        });
        assert_eq!(failure.unwrap_err().to_string(), "迁移失败，重试");
        assert!(RuntimeDirs::needs_migration(root));
        assert!(!stamp.exists());

        fs::create_dir(&stamp).unwrap();
        assert!(migrate_with(&core, root, root, |_, _, _| Ok(())).is_err());
        assert!(!RuntimeDirs::migration_complete(root));
        assert_eq!(
            RuntimeDirs::resolve(root, root).unwrap(),
            RuntimeDirs::flat(root, root)
        );
        assert_eq!(
            fs::read(root.join("build/legacy.bin")).unwrap(),
            b"still usable"
        );

        fs::remove_dir(&stamp).unwrap();
        assert!(migrate_with(&core, root, root, |_, _, _| Ok(())).unwrap());
        assert!(!RuntimeDirs::needs_migration(root));
        assert_eq!(ReloadStamp::package_key(&stamp).as_deref(), Some("local-a"));
    }

    #[test]
    fn failed_migration_keeps_live_flat_files_out_of_orphan_report() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let core = fixture(root);
        let activation = package(&core, "local-a", "alpha", "alpha");
        package_activation::write(&core, &activation).unwrap();
        let legacy = [
            "legacy.schema.yaml",
            "legacy.dict.yaml",
            "legacy.custom.yaml",
            "default.yaml",
            "nested/legacy_include.yaml",
            "alternate.txt",
            "rime.lua",
            "lua/live.lua",
            "build/legacy.table.bin",
            "prebuilt/legacy.prism.bin",
            "opencc/legacy.json",
            "legacy.ocd2",
        ];
        for path in legacy.into_iter().chain(["orphan.bak"]) {
            put(root, path, b"# legacy contents\n");
        }
        let mut deploy_called = false;
        let failure = migrate_with(&core, root, root, |_, _, _| {
            deploy_called = true;
            Err("fixture deployment failed".into())
        });
        assert!(deploy_called);
        assert!(failure.is_err());
        assert!(RuntimeDirs::needs_migration(root));

        let report = crate::package_orphans::report(&core, root).unwrap();
        let paths: Vec<_> = report.files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(paths, ["orphan.bak"]);

        assert!(migrate_with(&core, root, root, |_, _, _| Ok(())).unwrap());
        let report = crate::package_orphans::report(&core, root).unwrap();
        assert!(report.files.iter().any(|file| file.path == "legacy.schema.yaml"));
    }

    #[test]
    fn migration_rejects_sync_directory_in_the_apps_separate_package_store() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("ime");
        let core = fixture(&directory.path().join("app"));
        let store = PackageStore::new(&core).root();
        put(
            &root,
            "installation.yaml",
            format!("sync_dir: {:?}\n", store.join("addons")).as_bytes(),
        );
        assert!(checked_sync_dir(&core, &root).is_err());
    }
}
