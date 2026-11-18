use super::*;
use crate::{AppEnv, CoreEvent, EventSink, Platform};
use std::sync::Arc;

struct NullSink;
impl EventSink for NullSink {
    fn emit(&self, _: CoreEvent) {}
}

fn core(path: &Path) -> Arc<Core> {
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

fn manifest_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "format": 1, "version": "fixture-v1", "source": "fixture", "archive_sha256": "a".repeat(64),
        "includes_grammar": false,
        "files": entries.iter().map(|(path,bytes)| serde_json::json!({
            "path": path, "sha256": sha256_reader(*bytes).unwrap(), "size": bytes.len(),
        })).collect::<Vec<_>>(),
    }))
    .unwrap()
}

fn put(root: &Path, relative: &str, bytes: &[u8]) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

#[test]
fn snow_migration_releases_addon_lock_before_waiting_for_package_lock() {
    use std::time::{Duration, Instant};

    let directory = tempfile::tempdir().unwrap();
    let core = core(directory.path());
    let root = directory.path().join("rime");
    let source = directory.path().join("source");
    fs::create_dir(&root).unwrap();
    let schema = "snow_jiandao.schema.yaml";
    put(&source, schema, b"schema: {schema_id: snow_jiandao}");
    put(&root, schema, b"schema: {schema_id: snow_jiandao}");
    AddonStore::new(&core, &root, crate::rime_snow::SCHEMA_ID)
        .unwrap()
        .commit(
            &source,
            &manifest_bytes(&[(schema, b"schema: {schema_id: snow_jiandao}")]),
        )
        .unwrap();
    let metadata = PackageStore::new(&core).metadata_path(crate::rime_snow::SCHEMA_ID);
    let pending = core.pending_package.lock().unwrap();
    let migrating = core.clone();
    let worker = std::thread::spawn(move || migrate_snow_package(&migrating));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !metadata.is_file() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let published = metadata.is_file();
    let mut addon_lock_available = false;
    while published && Instant::now() < deadline {
        if STORE_OPERATION.try_lock().is_ok() {
            addon_lock_available = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(pending);
    worker.join().unwrap().unwrap();
    assert!(published, "migration did not publish package metadata");
    assert!(
        addon_lock_available,
        "migration must not hold the addon lock while waiting for the package lock"
    );
}

#[test]
fn invalid_wanxiang_metadata_is_isolated_and_retained_for_retry() {
    for invalid in ["variant", "grammar", "json", "path", "oversized"] {
        let directory = tempfile::tempdir().unwrap();
        let core = core(directory.path());
        let root = directory.path().join("rime");
        fs::create_dir(&root).unwrap();
        put(&root, "wanxiang.schema.yaml", b"official");
        put(&root, GRAMMAR, b"fake model");
        let mut value: serde_json::Value =
            serde_json::from_slice(&manifest_bytes(&[("wanxiang.schema.yaml", b"official")]))
                .unwrap();
        value["variant"] = if invalid == "variant" {
            "invalid"
        } else {
            "base"
        }
        .into();
        if invalid == "grammar" {
            value["grammar"] = serde_json::json!({"version":"fixture", "size":0, "sha256":"bad"});
        }
        if invalid == "path" {
            value["files"][0]["path"] = "../outside".into();
        }
        let bytes = if invalid == "json" {
            b"{".to_vec()
        } else if invalid == "oversized" {
            vec![b' '; 1024 * 1024 + 1]
        } else {
            serde_json::to_vec(&value).unwrap()
        };
        put(&root, ".keytao-wanxiang.json", &bytes);
        put(&root, "rime_ice.schema.yaml", b"official");
        put(
            &root,
            ".keytao-rime-ice.json",
            &manifest_bytes(&[("rime_ice.schema.yaml", b"official")]),
        );
        assert_eq!(
            ownership(&core, &root).unwrap()["rime_ice.schema.yaml"],
            "rime_ice"
        );
        assert!(manifest(&core, &root, "rime_ice").unwrap().is_some());
        assert!(manifest(&core, &root, "wanxiang").unwrap().is_none());
        assert!(
            report_paths(&core, &root)
                .unwrap()
                .contains("rime_ice.schema.yaml")
        );
        crate::package_orphans::report(&core, &root).unwrap();
        migrate(&core, &root).unwrap();
        assert_eq!(fs::read(root.join(".keytao-wanxiang.json")).unwrap(), bytes);
        assert_eq!(fs::read(root.join(GRAMMAR)).unwrap(), b"fake model");
        let store = PackageStore::new(&core);
        assert!(!store.addon_path("wanxiang").exists());
        assert!(!store.root().join("addons/.layout-v1").exists());
        put(
            &root,
            ".keytao-wanxiang.json",
            &manifest_bytes(&[("wanxiang.schema.yaml", b"official")]),
        );
        migrate(&core, &root).unwrap();
        assert!(manifest(&core, &root, "wanxiang").unwrap().is_some());
        assert!(store.root().join("addons/.layout-v1").is_file());
        assert_eq!(fs::read(root.join(GRAMMAR)).unwrap(), b"fake model");
    }
}

#[test]
fn each_addon_stores_latest_sources_and_separate_activation() {
    let directory = tempfile::tempdir().unwrap();
    let core = core(directory.path());
    let root = directory.path().join("rime");
    assert!(manifest(&core, &root, "easy_en").unwrap().is_none());
    assert!(ownership(&core, &root).unwrap().is_empty());
    assert!(
        !PackageStore::new(&core)
            .root()
            .join("addons/.layout-v1")
            .exists()
    );
    fs::create_dir(&root).unwrap();
    for id in IDS {
        let source = tempfile::tempdir().unwrap();
        let relative = format!("{id}.schema.yaml");
        for bytes in [b"old".as_slice(), b"new".as_slice()] {
            put(source.path(), &relative, bytes);
            put(&root, &relative, bytes);
            let addon = AddonStore::new(&core, &root, id).unwrap();
            addon
                .commit(source.path(), &manifest_bytes(&[(&relative, bytes)]))
                .unwrap();
            assert_eq!(
                fs::read(addon.destination.join("files").join(&relative)).unwrap(),
                bytes
            );
            assert!(addon.destination.join("activation.json").is_file());
            assert!(
                !addon
                    .destination
                    .with_file_name(format!(".old-{id}"))
                    .exists()
            );
        }
    }
    assert_eq!(ownership(&core, &root).unwrap().len(), IDS.len());
    let first = AddonStore::new(&core, &root, IDS[0]).unwrap();
    first.remove().unwrap();
    first.remove().unwrap();
    assert!(!first.destination.exists());
    for id in &IDS[1..] {
        assert!(manifest(&core, &root, id).unwrap().is_some());
    }
}

#[test]
fn legacy_migration_is_idempotent_and_adopts_only_matching_sources() {
    let directory = tempfile::tempdir().unwrap();
    let core = core(directory.path());
    let root = directory.path().join("rime");
    fs::create_dir(&root).unwrap();
    put(&root, "rime_ice.schema.yaml", b"original");
    put(&root, "rime_ice.dict.yaml", b"user changed");
    put(
        &root,
        ".keytao-rime-ice.json",
        &manifest_bytes(&[
            ("rime_ice.schema.yaml", b"original"),
            ("rime_ice.dict.yaml", b"original"),
        ]),
    );
    migrate(&core, &root).unwrap();
    let addon = AddonStore::new(&core, &root, "rime_ice").unwrap();
    let before = fs::read(addon.destination.join("package.json")).unwrap();
    assert!(!root.join(".keytao-rime-ice.json").exists());
    assert_eq!(addon.activation().unwrap().unwrap().owned.len(), 1);
    assert!(!addon.destination.join("files/rime_ice.dict.yaml").exists());
    assert_eq!(
        fs::read(root.join("rime_ice.dict.yaml")).unwrap(),
        b"user changed"
    );
    assert!(addon.destination.join("migration-modified.json").is_file());
    assert!(
        !ownership(&core, &root)
            .unwrap()
            .contains_key("rime_ice.dict.yaml")
    );
    assert!(
        report_paths(&core, &root)
            .unwrap()
            .contains("rime_ice.dict.yaml")
    );
    migrate(&core, &root).unwrap();
    assert_eq!(
        fs::read(addon.destination.join("package.json")).unwrap(),
        before
    );
}

#[test]
fn grammar_is_owned_in_place_on_migration() {
    let directory = tempfile::tempdir().unwrap();
    let core = core(directory.path());
    let root = directory.path().join("rime");
    fs::create_dir(&root).unwrap();
    let grammar = b"fake grammar fixture";
    put(&root, GRAMMAR, grammar);
    put(
        &root,
        "wanxiang.schema.yaml",
        b"language: wanxiang-lts-zh-hans\n",
    );
    let mut value: serde_json::Value = serde_json::from_slice(&manifest_bytes(&[(
        "wanxiang.schema.yaml",
        b"language: wanxiang-lts-zh-hans\n",
    )]))
    .unwrap();
    value["variant"] = "base".into();
    value["includes_grammar"] = true.into();
    value["grammar"] = serde_json::json!({"version":"fixture", "size":grammar.len(), "sha256":sha256_reader(grammar.as_slice()).unwrap()});
    put(
        &root,
        ".keytao-wanxiang.json",
        &serde_json::to_vec(&value).unwrap(),
    );
    migrate(&core, &root).unwrap();
    let addon = AddonStore::new(&core, &root, "wanxiang").unwrap();
    let active = addon.activation().unwrap().unwrap();
    assert!(
        active
            .owned
            .iter()
            .any(|f| f.path == GRAMMAR && f.move_file)
    );
    assert!(!addon.destination.join("files").join(GRAMMAR).exists());
    assert_eq!(fs::read(root.join(GRAMMAR)).unwrap(), grammar);
    assert_eq!(
        fs::read(addon.destination.join("files/wanxiang.schema.yaml")).unwrap(),
        b"language: \"\"\n"
    );
    let manifest_before = addon.read_manifest().unwrap().unwrap();
    put(
        &root,
        ".keytao-wanxiang.json",
        &serde_json::to_vec(&value).unwrap(),
    );
    fs::remove_file(PackageStore::new(&core).root().join("addons/.layout-v1")).unwrap();
    migrate(&core, &root).unwrap();
    assert_eq!(addon.read_manifest().unwrap().unwrap(), manifest_before);
}

#[test]
fn stale_legacy_grammar_does_not_block_other_packages() {
    for scenario in ["missing", "size", "hash", "claimed"] {
        let directory = tempfile::tempdir().unwrap();
        let core = core(directory.path());
        let root = directory.path().join("rime");
        fs::create_dir(&root).unwrap();
        let grammar = b"fake grammar fixture";
        let model = match scenario {
            "missing" => None,
            "size" => Some(b"short".to_vec()),
            "hash" => Some(vec![b'x'; grammar.len()]),
            _ => Some(grammar.to_vec()),
        };
        if let Some(bytes) = &model {
            put(&root, GRAMMAR, bytes);
        }
        let entries = [(
            "wanxiang.schema.yaml",
            b"language: wanxiang-lts-zh-hans\n".as_slice(),
        )];
        put(&root, entries[0].0, entries[0].1);
        let mut value: serde_json::Value =
            serde_json::from_slice(&manifest_bytes(&entries)).unwrap();
        value["variant"] = "base".into();
        value["includes_grammar"] = true.into();
        value["grammar"] = serde_json::json!({"version":"fixture", "size":grammar.len(), "sha256":sha256_reader(grammar.as_slice()).unwrap()});
        let bytes = serde_json::to_vec(&value).unwrap();
        put(&root, ".keytao-wanxiang.json", &bytes);
        put(&root, "rime_ice.schema.yaml", b"official");
        put(
            &root,
            ".keytao-rime-ice.json",
            &manifest_bytes(&[("rime_ice.schema.yaml", b"official")]),
        );
        if scenario == "claimed" {
            crate::package_activation::write(
                &core,
                &Activation {
                    key: "main".into(),
                    root: root.canonicalize().unwrap().to_string_lossy().into_owned(),
                    owned: vec![OwnedFile {
                        path: GRAMMAR.into(),
                        sha256: sha256_reader(grammar.as_slice()).unwrap(),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            )
            .unwrap();
        }
        let owned = crate::package_activation::addon_ownership(&core, &root).unwrap();
        assert!(!owned.contains_key(GRAMMAR), "{scenario}");
        assert_eq!(owned["rime_ice.schema.yaml"], "rime_ice");
        let current: serde_json::Value =
            serde_json::from_slice(&manifest(&core, &root, "wanxiang").unwrap().unwrap()).unwrap();
        assert!(current["grammar"].is_null(), "{scenario}");
        assert_eq!(current["includes_grammar"], false);
        assert!(manifest(&core, &root, "rime_ice").unwrap().is_some());
        assert!(report_paths(&core, &root).unwrap().contains(GRAMMAR));
        let addon = AddonStore::new(&core, &root, "wanxiang").unwrap();
        let modified: Vec<String> = serde_json::from_slice(
            &fs::read(addon.destination.join("migration-modified.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(modified, [GRAMMAR]);
        assert!(!addon.destination.join("files").join(GRAMMAR).exists());
        assert!(!root.join(".keytao-wanxiang.json").exists());
        assert!(
            PackageStore::new(&core)
                .root()
                .join("addons/.layout-v1")
                .is_file()
        );
        crate::package_orphans::report(&core, &root).unwrap();
        let before = fs::read(addon.destination.join("activation.json")).unwrap();
        migrate(&core, &root).unwrap();
        assert_eq!(
            fs::read(addon.destination.join("activation.json")).unwrap(),
            before
        );
        assert_eq!(fs::read(root.join(GRAMMAR)).ok(), model);
        if scenario != "claimed" {
            assert!(
                addon
                    .commit(&addon.destination.join("files"), &bytes)
                    .is_err()
            );
            assert_eq!(
                fs::read(addon.destination.join("activation.json")).unwrap(),
                before
            );
        }
    }
}

#[test]
fn crashed_swap_recovers_without_touching_rime_or_model() {
    for incomplete in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let core = core(directory.path());
        let root = directory.path().join("rime");
        fs::create_dir(&root).unwrap();
        put(&root, GRAMMAR, b"keep model");
        put(&root, "rime_mint.schema.yaml", b"old");
        let source = tempfile::tempdir().unwrap();
        put(source.path(), "rime_mint.schema.yaml", b"old");
        let addon = AddonStore::new(&core, &root, "rime_mint").unwrap();
        addon
            .commit(
                source.path(),
                &manifest_bytes(&[("rime_mint.schema.yaml", b"old")]),
            )
            .unwrap();
        fs::rename(
            &addon.destination,
            addon.destination.with_file_name(".old-rime_mint"),
        )
        .unwrap();
        if incomplete {
            put(&addon.destination, "package.json", b"partial");
        }
        let recovered = AddonStore::new(&core, &root, "rime_mint").unwrap();
        assert!(recovered.read_manifest().unwrap().is_some());
        assert_eq!(fs::read(root.join(GRAMMAR)).unwrap(), b"keep model");
        assert_eq!(
            fs::read(root.join("rime_mint.schema.yaml")).unwrap(),
            b"old"
        );
    }
}

#[test]
fn generated_files_are_separate_from_official_public_sources() {
    let directory = tempfile::tempdir().unwrap();
    let core = core(directory.path());
    let root = directory.path().join("rime");
    fs::create_dir(&root).unwrap();
    let entries = [
        ("rime_frost.schema.yaml", b"schema".as_slice()),
        ("rime_frost-NOTICE.txt", b"notice".as_slice()),
    ];
    let source = tempfile::tempdir().unwrap();
    for (path, bytes) in entries {
        put(&root, path, bytes);
        put(source.path(), path, bytes);
    }
    let addon = AddonStore::new(&core, &root, "rime_frost").unwrap();
    addon
        .commit(source.path(), &manifest_bytes(&entries))
        .unwrap();
    let active = addon.activation().unwrap().unwrap();
    assert_eq!(active.owned.len(), 1);
    assert_eq!(active.generated[0].path, "rime_frost-NOTICE.txt");
    let public = public_files(&core, "rime_frost").unwrap();
    assert_eq!(public.len(), 2);
    assert!(
        public
            .iter()
            .any(|(_, path)| path == Path::new("rime_frost.schema.yaml"))
    );
    assert!(
        public
            .iter()
            .any(|(_, path)| path == Path::new("rime_frost-NOTICE.txt"))
    );
}

#[test]
fn commit_refuses_changed_sources_and_keeps_previous_package() {
    let directory = tempfile::tempdir().unwrap();
    let core = core(directory.path());
    let root = directory.path().join("rime");
    fs::create_dir(&root).unwrap();
    let source = tempfile::tempdir().unwrap();
    put(source.path(), "rime_ice.schema.yaml", b"old");
    put(&root, "rime_ice.schema.yaml", b"old");
    let addon = AddonStore::new(&core, &root, "rime_ice").unwrap();
    addon
        .commit(
            source.path(),
            &manifest_bytes(&[("rime_ice.schema.yaml", b"old")]),
        )
        .unwrap();
    put(source.path(), "rime_ice.schema.yaml", b"new");
    put(&root, "rime_ice.schema.yaml", b"user edit");
    assert!(
        addon
            .commit(
                source.path(),
                &manifest_bytes(&[("rime_ice.schema.yaml", b"new")])
            )
            .is_err()
    );
    assert_eq!(
        fs::read(addon.destination.join("files/rime_ice.schema.yaml")).unwrap(),
        b"old"
    );
    assert_eq!(
        fs::read(root.join("rime_ice.schema.yaml")).unwrap(),
        b"user edit"
    );
}

#[test]
fn migration_does_not_adopt_main_owned_sources() {
    let directory = tempfile::tempdir().unwrap();
    let core = core(directory.path());
    let root = directory.path().join("rime");
    fs::create_dir(&root).unwrap();
    put(&root, "rime_ice.schema.yaml", b"main owns this");
    put(
        &root,
        ".keytao-rime-ice.json",
        &manifest_bytes(&[("rime_ice.schema.yaml", b"main owns this")]),
    );
    let active = Activation {
        key: "main".into(),
        root: root.canonicalize().unwrap().to_string_lossy().into_owned(),
        owned: vec![OwnedFile {
            path: "rime_ice.schema.yaml".into(),
            sha256: sha256_reader(b"main owns this".as_slice()).unwrap(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let store = PackageStore::new(&core);
    fs::create_dir_all(store.root()).unwrap();
    fs::write(
        store.checked_activation_path().unwrap(),
        serde_json::to_vec(&active).unwrap(),
    )
    .unwrap();
    migrate(&core, &root).unwrap();
    let addon = AddonStore::new(&core, &root, "rime_ice").unwrap();
    assert!(addon.activation().unwrap().unwrap().owned.is_empty());
    assert!(addon.destination.join("migration-modified.json").is_file());
    assert_eq!(
        fs::read(root.join("rime_ice.schema.yaml")).unwrap(),
        b"main owns this"
    );
}

#[test]
fn easy_migration_generates_public_defaults_without_adopting_private_custom() {
    let directory = tempfile::tempdir().unwrap();
    let core = core(directory.path());
    let root = directory.path().join("rime");
    fs::create_dir(&root).unwrap();
    let official = crate::addon::official_custom().unwrap();
    let private = b"patch:\n  user_setting: keep\n";
    put(&root, "easy_en.schema.yaml", b"schema");
    put(&root, "easy_en.custom.yaml", private);
    put(
        &root,
        ".keytao-easy-en.json",
        &manifest_bytes(&[
            ("easy_en.schema.yaml", b"schema"),
            ("easy_en.custom.yaml", official.as_slice()),
        ]),
    );
    put(
        directory.path(),
        "easy-en-public/old/current.txt",
        b"old cache",
    );
    migrate(&core, &root).unwrap();
    let addon = AddonStore::new(&core, &root, "easy_en").unwrap();
    assert_eq!(fs::read(root.join("easy_en.custom.yaml")).unwrap(), private);
    assert_eq!(
        fs::read(addon.destination.join("files/easy_en.custom.yaml")).unwrap(),
        official
    );
    assert!(addon.activation().unwrap().unwrap().generated.is_empty());
    assert!(!directory.path().join("easy-en-public").exists());
}

#[test]
fn status_recovery_waits_for_in_progress_swap() {
    let directory = tempfile::tempdir().unwrap();
    let core = core(directory.path());
    let root = directory.path().join("rime");
    fs::create_dir(&root).unwrap();
    put(&root, "rime_mint.schema.yaml", b"schema");
    let source = tempfile::tempdir().unwrap();
    put(source.path(), "rime_mint.schema.yaml", b"schema");
    let addon = AddonStore::new(&core, &root, "rime_mint").unwrap();
    addon
        .commit(
            source.path(),
            &manifest_bytes(&[("rime_mint.schema.yaml", b"schema")]),
        )
        .unwrap();
    let guard = operation();
    let old = addon.destination.with_file_name(".old-rime_mint");
    fs::rename(&addon.destination, &old).unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        let present = AddonStore::new(&core, &root, "rime_mint")
            .unwrap()
            .read_manifest()
            .unwrap()
            .is_some();
        done_tx.send(present).unwrap();
    });
    started_rx.recv().unwrap();
    assert!(
        done_rx
            .recv_timeout(std::time::Duration::from_millis(30))
            .is_err()
    );
    assert!(!addon.destination.exists());
    fs::rename(&old, &addon.destination).unwrap();
    drop(guard);
    assert!(done_rx.recv().unwrap());
    thread.join().unwrap();
}
