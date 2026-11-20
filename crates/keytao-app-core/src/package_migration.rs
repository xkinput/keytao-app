use crate::{
    Core, CoreError, migration_userdb, package_activation,
    package_orphans::{self, OrphanReport},
    package_store::{PackageStore, StoredPackage},
    packages::safe_path,
    runtime_materialize,
};
use keytao_core::{ReloadStamp, RuntimeDirs};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

const VERSION: u32 = 1;

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
struct Migration {
    version: u32,
    active_package: String,
    runtime_package: String,
    packages: Vec<String>,
    user_files: Vec<String>,
    legacy_user_databases: Vec<String>,
}

#[derive(Serialize)]
pub struct MigrationReport {
    completed: bool,
    #[serde(flatten)]
    migration: Migration,
    orphans: OrphanReport,
}

pub fn preview(core: &Core, root: &Path) -> Result<MigrationReport, CoreError> {
    let root = checked_root(root)?;
    let sync_dir = checked_sync_dir(core, &root)?;
    let previous = read_marker(&marker_path(&root)?)?;
    let completed = previous.is_some();
    let migration = match previous {
        Some(migration) => migration,
        None => plan(
            core,
            &root,
            &runtime_materialize::legacy_user_files(core, &root)?,
        )?,
    };
    report(core, &root, &sync_dir, migration, completed)
}

pub fn migrate_to_isolated(core: &Core, root: &Path) -> Result<MigrationReport, CoreError> {
    let root = checked_root(root)?;
    if !RuntimeDirs::isolation_enabled(&root) {
        return Err(CoreError::Other("方案包隔离未启用".into()));
    }
    checked_sync_dir(core, &root)?;
    keytao_core::isolated_deploy::with_isolated_package_lock(&root, || {
        migrate_locked(core, &root).map_err(|error| error.to_string())
    })
    .map_err(CoreError::Other)
}

fn migrate_locked(core: &Core, root: &Path) -> Result<MigrationReport, CoreError> {
    package_activation::recover(core)?;
    let sync_dir = checked_sync_dir(core, root)?;
    let path = marker_path(root)?;
    if let Some(migration) = read_marker(&path)? {
        return report(core, root, &sync_dir, migration, true);
    }
    crate::addon_store::project_legacy_for_isolation(core, root)?;
    let legacy = runtime_materialize::legacy_user_files(core, root)?;
    let migration = plan(core, root, &legacy)?;
    let report = report(core, root, &sync_dir, migration, true)?;
    let bundled = crate::deploy::deploy_shared_data_dir(core, root);
    let runtimes = runtime_materialize::materialize_migration(
        core,
        root,
        &bundled,
        &report.migration.runtime_package,
        &legacy,
    )?;
    migration_userdb::migrate(root, &runtimes, &sync_dir)?;
    crate::scheme_files::write_file_atomic(&path, &serde_json::to_vec(&report.migration)?)?;
    if let Err(error) = ReloadStamp::write_package(root, &report.migration.runtime_package) {
        let error = match crate::package_fs::remove_file(&path) {
            Ok(()) => error,
            Err(rollback) => format!("{error}; remove incomplete migration marker: {rollback}"),
        };
        return Err(CoreError::Other(error));
    }
    Ok(report)
}

fn report(
    core: &Core,
    root: &Path,
    sync_dir: &Path,
    migration: Migration,
    completed: bool,
) -> Result<MigrationReport, CoreError> {
    let orphans = package_orphans::report_migration(
        core,
        root,
        sync_dir,
        migration.user_files.iter().cloned().collect(),
    )?;
    Ok(MigrationReport {
        completed,
        migration,
        orphans,
    })
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

fn read_marker(path: &Path) -> Result<Option<Migration>, CoreError> {
    match fs::read(path) {
        Ok(bytes) => {
            let migration: Migration = serde_json::from_slice(&bytes)?;
            if migration.version != VERSION {
                return Err(CoreError::Other(format!(
                    "Unsupported migration version: {}",
                    migration.version,
                )));
            }
            Ok(Some(migration))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keytao_core::RELOAD_STAMP_FILE_NAME;
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
            pending_package: Mutex::new(None),
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
    fn migration_preserves_legacy_and_is_idempotent() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let core = fixture(root);
        let a = package(&core, "local-a", "alpha", "alpha");
        let mut z = package(&core, "local-z", "zulu", "keytao");
        package(&core, "local-inactive", "inactive", "inactive");
        put(root, "default.custom.yaml", b"patch: {menu/page_size: 7}\n");
        z.backups.push(
            package_activation::backup(&core, root, "default.custom.yaml")
                .unwrap()
                .unwrap(),
        );
        let applied = b"patch: {menu/page_size: 7, recognizer/patterns: generated}\n";
        put(root, "default.custom.yaml", applied);
        z.generated.push(
            package_activation::fingerprint(root, "default.custom.yaml")
                .unwrap()
                .unwrap(),
        );
        z.default_applied.insert(
            "default.custom.yaml".into(),
            serde_yaml::from_slice(applied).unwrap(),
        );
        package_activation::write(&core, &a).unwrap();
        package_activation::write(&core, &z).unwrap();
        put(
            root,
            "scheme-packages/active-manifest.json",
            br#"{"active_package":"local-a"}"#,
        );
        put(
            root,
            RuntimeDirs::SETTINGS_FILE,
            br#"{"runtime_isolation":true}"#,
        );
        put(
            root,
            "user.yaml",
            b"var: {previously_selected_schema: zulu}\n",
        );
        put(
            root,
            "squirrel.custom.yaml",
            b"patch: {style/color_scheme: custom}\n",
        );
        put(
            root,
            "weasel.custom.yaml",
            b"patch: {style/horizontal: true}\n",
        );
        put(root, "custom_phrase.txt", b"phrase\tcode\n");
        put(root, "personal.dict.yaml", b"---\nname: personal\n...\n");
        put(
            root,
            "keytao.user.dict.yaml",
            b"---\nname: keytao.user\n...\n",
        );
        put(root, "keytao.user.remove.txt", b"removed\n");
        put(root, "orphan.yaml", b"leave me here\n");
        put(root, "build/legacy.bin", b"legacy build");
        put(
            root,
            "cloud-backup/private.custom.yaml",
            b"patch: {sync_only: true}\n",
        );
        put(
            root,
            "installation.yaml",
            format!(
                "sync_dir: {:?}\n",
                root.join("cloud-backup/../cloud-backup")
            )
            .as_bytes(),
        );
        let user_custom = crate::managed_custom::set_patch(
            "patch: {user: true}\n",
            "fixture",
            Some("{migration_generated: true}"),
        )
        .unwrap();
        put(root, "another.custom.yaml", user_custom.as_bytes());

        let legacy = test_hashes(root);
        let planned = preview(&core, root).unwrap();
        assert!(!planned.completed);
        assert_eq!(planned.migration.active_package, "local-z");
        assert_eq!(test_hashes(root), legacy);
        assert_eq!(
            planned
                .orphans
                .files
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            ["orphan.yaml"]
        );

        let stamp = ReloadStamp::write_package(root, "local-a").unwrap();
        let result = migrate_to_isolated(&core, root).unwrap();
        assert!(result.completed);
        assert_eq!(result.migration, planned.migration);
        assert_eq!(
            RuntimeDirs::active_package_key(root).as_deref(),
            Some("local-z")
        );
        assert_eq!(
            ReloadStamp::package_key(&stamp).as_deref(),
            Some("local-z")
        );
        for key in &result.migration.packages {
            let run = root.join("runtime").join(key);
            let marker: serde_json::Value =
                serde_json::from_slice(&fs::read(run.join(RuntimeDirs::MANIFEST_FILE)).unwrap())
                    .unwrap();
            for name in &result.migration.user_files {
                assert!(run.join(name).is_file(), "{key}/{name}");
                assert!(marker["user_owned"][name].is_string(), "{key}/{name}");
            }
            let default = fs::read_to_string(run.join("default.custom.yaml")).unwrap();
            assert!(default.contains("menu/page_size: 7"));
            assert!(!default.contains("recognizer/patterns"));
            assert_eq!(
                fs::read(run.join("another.custom.yaml")).unwrap(),
                b"patch: {user: true}\n"
            );
            assert_eq!(
                run.join("keytao.user.dict.yaml").is_file(),
                key == "local-z"
            );
            assert_eq!(
                run.join("keytao.user.remove.txt").is_file(),
                key == "local-z"
            );
            assert!(!run.join("cloud-backup").exists());
        }
        let migrated = test_hashes(root);
        assert_eq!(
            migrated
                .iter()
                .filter(|(path, _)| {
                    !path.starts_with("runtime/") && path.as_str() != RELOAD_STAMP_FILE_NAME
                })
                .map(|(path, hash)| (path.clone(), hash.clone()))
                .collect::<BTreeMap<_, _>>(),
            legacy,
        );
        assert!(migrate_to_isolated(&core, root).unwrap().completed);
        assert_eq!(test_hashes(root), migrated);
        put(
            root,
            "scheme-packages/active-manifest.json",
            br#"{"active_package":"local-a","runtime_migration_version":1}"#,
        );
        assert_eq!(
            RuntimeDirs::active_package_key(root).as_deref(),
            Some("local-a")
        );
        put(
            root,
            RuntimeDirs::SETTINGS_FILE,
            br#"{"runtime_isolation":false}"#,
        );
        assert_eq!(
            RuntimeDirs::resolve(root, root).unwrap(),
            RuntimeDirs::flat(root, root)
        );
    }

    #[test]
    fn active_choice_falls_back_to_first_activated_main_then_keytao() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let core = fixture(root);
        let a = package(&core, "local-a", "alpha", "alpha");
        let z = package(&core, "local-z", "zulu", "zulu");
        put(
            root,
            "user.yaml",
            b"var: {previously_selected_schema: unknown}\n",
        );
        let fallback = preview(&core, root).unwrap();
        assert_eq!(fallback.migration.active_package, "keytao");
        assert_eq!(fallback.migration.runtime_package, "builtin/keytao");
        package_activation::write(&core, &z).unwrap();
        package_activation::write(&core, &a).unwrap();
        assert_eq!(
            preview(&core, root).unwrap().migration.active_package,
            "local-a"
        );
    }

    #[test]
    fn legacy_addon_can_own_the_current_schema_without_changing_legacy_files() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let core = fixture(root);
        put(
            root,
            RuntimeDirs::SETTINGS_FILE,
            br#"{"runtime_isolation":true}"#,
        );
        put(
            root,
            "user.yaml",
            b"var: {previously_selected_schema: rime_ice}\n",
        );
        let source = b"schema: {schema_id: rime_ice, name: Rime Ice}\n";
        put(root, "rime_ice.schema.yaml", source);
        put(root, "legacy/nested/untouched.yaml", b"untouched\n");
        let receipt = serde_json::to_vec(&serde_json::json!({
            "format": 1, "version": "1", "source": "fixture", "archive_sha256": "0".repeat(64),
            "files": [{
                "path": "rime_ice.schema.yaml", "size": source.len(),
                "sha256": sha256_reader(source.as_slice()).unwrap(),
            }],
        }))
        .unwrap();
        put(root, ".keytao-rime-ice.json", &receipt);
        let before = test_hashes(root);
        assert_eq!(
            crate::addon_store::manifest(&core, root, "rime_ice").unwrap(),
            Some(receipt.clone())
        );
        package_orphans::report(&core, root).unwrap();
        assert_eq!(test_hashes(root), before);
        assert_eq!(
            preview(&core, root).unwrap().migration.active_package,
            "rime_ice"
        );
        assert_eq!(test_hashes(root), before);
        assert_eq!(
            migrate_to_isolated(&core, root)
                .unwrap()
                .migration
                .active_package,
            "rime_ice"
        );
        assert!(root.join("runtime/rime_ice").is_dir());
        let mut after = test_hashes(root);
        after.retain(|path, _| {
            !path.starts_with("runtime/")
                && !path.starts_with("scheme-packages/")
                && path != RELOAD_STAMP_FILE_NAME
        });
        assert_eq!(after, before);

        put(
            root,
            RuntimeDirs::SETTINGS_FILE,
            br#"{"runtime_isolation":false}"#,
        );
        let rolled_back = test_hashes(root);
        assert_eq!(
            crate::addon_store::manifest(&core, root, "rime_ice").unwrap(),
            Some(receipt)
        );
        package_orphans::report(&core, root).unwrap();
        assert_eq!(test_hashes(root), rolled_back);

        put(
            root,
            RuntimeDirs::SETTINGS_FILE,
            br#"{"runtime_isolation":true}"#,
        );
        fs::remove_dir_all(root.join("scheme-packages/addons/rime_ice")).unwrap();
        assert!(crate::addon_store::manifest(&core, root, "rime_ice")
            .unwrap()
            .is_none());
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

        assert!(preview(&core, &root).is_err());
    }

    #[test]
    fn migration_can_retry_after_a_reload_stamp_write_fails() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let core = fixture(root);
        let activation = package(&core, "local-a", "alpha", "alpha");
        package_activation::write(&core, &activation).unwrap();
        put(
            root,
            RuntimeDirs::SETTINGS_FILE,
            br#"{"runtime_isolation":true}"#,
        );
        let stamp = ReloadStamp::path(root);
        fs::create_dir(&stamp).unwrap();
        assert!(migrate_to_isolated(&core, root).is_err());
        assert!(!preview(&core, root).unwrap().completed);

        fs::remove_dir(&stamp).unwrap();
        assert!(migrate_to_isolated(&core, root).unwrap().completed);
        assert_eq!(
            ReloadStamp::package_key(&stamp).as_deref(),
            Some("local-a")
        );
    }
}
