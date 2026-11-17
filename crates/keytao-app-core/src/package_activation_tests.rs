#![cfg(not(any(target_os = "android", target_os = "windows")))]

use crate::{
    AppEnv, Core, CoreEvent, EventSink, Platform,
    package_activation::{self, Transaction},
    package_store::PackageStore,
    packages::{
        activate_scheme, archive_from_files, remove_scheme_package, sha256_reader,
        store_cloud_archive,
    },
    scheme_host::SchemeHost,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

struct Sink;
impl EventSink for Sink {
    fn emit(&self, _: CoreEvent) {}
}

struct Host {
    root: PathBuf,
    core: Arc<Core>,
}

impl Host {
    fn new(directory: &Path, sink: Arc<dyn EventSink>) -> Self {
        let root = directory.join("rime");
        fs::create_dir_all(&root).unwrap();
        let resources = directory.join("resources");
        fs::create_dir_all(resources.join("rime-data")).unwrap();
        fs::write(resources.join("rime-data/default.yaml"), b"{}").unwrap();
        let core = Core::new(
            AppEnv {
                data_dir: directory.join("app-state"),
                cache_dir: directory.join("cache"),
                resource_dir: resources,
                app_version: "test".into(),
                platform: Platform::MacOs,
            },
            sink,
        )
        .unwrap();
        Self { root, core }
    }
}

impl SchemeHost for Host {
    fn user_root(&self) -> Result<PathBuf, String> {
        Ok(self.root.clone())
    }
    async fn deploy(&self) -> Result<(), String> {
        Ok(())
    }
    fn write_reload_stamp(&self, _: &Path) -> Result<Option<PathBuf>, String> {
        Ok(None)
    }
}

fn write(root: &Path, path: &str, content: &[u8]) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn files(schema: &str) -> Vec<(String, Vec<u8>)> {
    vec![
        (
            format!("{schema}.schema.yaml"),
            format!("schema: {{schema_id: {schema}}}\ntranslator: {{dictionary: {schema}}}\n")
                .into_bytes(),
        ),
        (
            format!("{schema}.custom.yaml"),
            b"patch:\n  menu/page_size: 6\n".to_vec(),
        ),
        (
            format!("lua/{schema}.lua"),
            format!("return '{schema}'\n").into_bytes(),
        ),
        ("shared.txt".into(), schema.as_bytes().to_vec()),
        (
            "default.custom.yaml".into(),
            format!("patch:\n  menu/page_size: 6\n  schema_list:\n    - schema: {schema}\n")
                .into_bytes(),
        ),
        (
            "rime.lua".into(),
            format!("require('{schema}')\n").into_bytes(),
        ),
        (
            "squirrel.custom.yaml".into(),
            b"package frontend must not be copied".to_vec(),
        ),
        (
            "weasel.custom.yaml".into(),
            b"package frontend must not be copied".to_vec(),
        ),
    ]
}

fn store(host: &Host, key: &str, entries: Vec<(String, Vec<u8>)>) {
    let bytes = archive_from_files(&entries).unwrap();
    assert_eq!(
        store_cloud_archive(
            &host.core,
            key,
            Some("v1"),
            "official",
            None,
            &bytes,
            &host.root
        )
        .unwrap(),
        key,
    );
}

fn snapshot(root: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, directory: &Path, result: &mut BTreeMap<String, String>) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let kind = entry.file_type().unwrap();
            if kind.is_symlink() {
                result.insert(
                    relative,
                    format!("symlink:{}", fs::read_link(path).unwrap().display()),
                );
            } else if kind.is_dir() {
                walk(root, &path, result);
            } else if kind.is_file() {
                result.insert(
                    relative,
                    sha256_reader(fs::File::open(path).unwrap()).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(root, root, &mut result);
    result
}

fn package_state(root: &Path) -> BTreeMap<String, String> {
    snapshot(root)
        .into_iter()
        .filter(|(path, _)| {
            !path.starts_with("build/")
                && !matches!(
                    path.as_str(),
                    "default.custom.yaml"
                        | "default-custom.yaml"
                        | "rime.lua"
                        | "user.yaml"
                        | keytao_core::ACTIVE_SCHEME_MANIFEST
                )
        })
        .collect()
}

#[tokio::test]
async fn activation_a_b_a_restores_files_and_unowned_backups() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let host = Host::new(directory.path(), Arc::new(Sink));
    for (name, bytes) in [
        ("shared.txt", b"original collision".as_slice()),
        ("unowned.custom.yaml", b"patch: {menu/page_size: 9}\n"),
        ("lua/user.lua", b"return 'user'\n"),
        ("squirrel.custom.yaml", b"style: user\n"),
        ("weasel.custom.yaml", b"style: user\n"),
        ("alpha.userdb/data", b"user dictionary"),
        ("sync/device/alpha.userdb.txt", b"user sync"),
        (
            "default.custom.yaml",
            b"patch:\n  menu/page_size: 9\n  schema_list: []\n",
        ),
    ] {
        write(&host.root, name, bytes);
    }
    let original = package_state(&host.root);
    store(&host, "keytao", files("alpha"));
    store(&host, "xmjd", files("beta"));
    activate_scheme(&host.core, "keytao", &host.root)
        .await
        .unwrap();
    let initial_a = package_state(&host.root);
    let activation = package_activation::read(&host.core, &host.root)
        .unwrap()
        .unwrap();
    assert_eq!(activation.key, "keytao");
    assert!(
        activation
            .owned
            .iter()
            .any(|file| file.path == "alpha.custom.yaml")
    );
    assert!(
        activation
            .owned
            .iter()
            .any(|file| file.path == "lua/alpha.lua")
    );
    let backup = activation
        .backups
        .iter()
        .find(|file| file.path == "shared.txt")
        .unwrap();
    assert_eq!(
        fs::read(PackageStore::new(&host.core).root().join(&backup.backup)).unwrap(),
        b"original collision"
    );
    activate_scheme(&host.core, "xmjd", &host.root)
        .await
        .unwrap();
    assert!(!host.root.join("alpha.custom.yaml").exists());
    assert!(!host.root.join("lua/alpha.lua").exists());
    activate_scheme(&host.core, "keytao", &host.root)
        .await
        .unwrap();
    assert_eq!(package_state(&host.root), initial_a);
    assert!(!host.root.join("beta.custom.yaml").exists());
    assert!(!host.root.join("lua/beta.lua").exists());
    remove_scheme_package(&host.core, "keytao", &host)
        .await
        .unwrap();
    assert_eq!(package_state(&host.root), original);
    let default: serde_yaml::Value =
        serde_yaml::from_slice(&fs::read(host.root.join("default.custom.yaml")).unwrap()).unwrap();
    assert_eq!(default["patch"]["menu/page_size"].as_i64(), Some(9));
}

#[tokio::test]
async fn legacy_owned_files_are_not_reclassified_as_unowned_activation_backups() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let host = Host::new(directory.path(), Arc::new(Sink));
    let entries = files("alpha");
    store(&host, "keytao", entries.clone());
    for (path, bytes) in &entries {
        if !matches!(path.as_str(), "squirrel.custom.yaml" | "weasel.custom.yaml") {
            write(&host.root, path, bytes);
        }
    }
    write(
        &host.root,
        "unowned.custom.yaml",
        b"patch: {menu/page_size: 9}\n",
    );
    host.core
        .update_state(|state| state.active_scheme = Some("keytao".into()))
        .unwrap();
    let store = PackageStore::new(&host.core);
    let legacy_owned = store.read_meta("keytao").unwrap().unwrap().manifest;
    let marker = serde_json::to_vec_pretty(&serde_json::json!({
        "root": host.root.canonicalize().unwrap(), "scheme_key": "keytao",
        "manifest": legacy_owned, "rime_lua": "require('alpha')\n",
    }))
    .unwrap();
    write(&store.root(), "active-manifest.json", &marker);
    write(&host.root, keytao_core::ACTIVE_SCHEME_MANIFEST, &marker);
    assert!(!store.checked_activation_path().unwrap().exists());
    activate_scheme(&host.core, "keytao", &host.root)
        .await
        .unwrap();
    let active = package_activation::read(&host.core, &host.root)
        .unwrap()
        .unwrap();
    for path in legacy_owned.keys() {
        assert!(
            !active.backups.iter().any(|backup| &backup.path == path),
            "Legacy owned file was backed up as unowned: {path}"
        );
        assert!(
            active.owned.iter().any(|file| &file.path == path),
            "Legacy owned file was not activated: {path}"
        );
    }
    remove_scheme_package(&host.core, "keytao", &host)
        .await
        .unwrap();
    for path in legacy_owned.keys() {
        assert!(
            !host.root.join(path).exists(),
            "Legacy owned file was restored after package deletion: {path}"
        );
    }
    assert!(
        !fs::read_to_string(host.root.join("rime.lua"))
            .unwrap_or_default()
            .contains("require('alpha')"),
        "The legacy package Lua entry must not be restored after its owned modules are removed"
    );
    assert_eq!(
        fs::read(host.root.join("unowned.custom.yaml")).unwrap(),
        b"patch: {menu/page_size: 9}\n"
    );
}

#[tokio::test]
async fn activation_keeps_and_reports_modified_owned_custom_and_lua() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let host = Host::new(directory.path(), Arc::new(Sink));
    write(
        &host.root,
        "unowned.custom.yaml",
        b"patch: {menu/page_size: 9}\n",
    );
    store(&host, "keytao", files("alpha"));
    activate_scheme(&host.core, "keytao", &host.root)
        .await
        .unwrap();
    write(
        &host.root,
        "alpha.custom.yaml",
        b"patch: {menu/page_size: 8}\n",
    );
    write(&host.root, "lua/alpha.lua", b"return 'user edit'\n");
    let retained = remove_scheme_package(&host.core, "keytao", &host)
        .await
        .unwrap();
    assert!(retained.contains(&"alpha.custom.yaml".into()));
    assert!(retained.contains(&"lua/alpha.lua".into()));
    assert_eq!(
        fs::read(host.root.join("alpha.custom.yaml")).unwrap(),
        b"patch: {menu/page_size: 8}\n"
    );
    assert_eq!(
        fs::read(host.root.join("lua/alpha.lua")).unwrap(),
        b"return 'user edit'\n"
    );
    assert_eq!(
        fs::read(host.root.join("unowned.custom.yaml")).unwrap(),
        b"patch: {menu/page_size: 9}\n"
    );
    assert!(!host.root.join("alpha.schema.yaml").exists());
    assert!(!host.root.join("squirrel.custom.yaml").exists());
    assert!(!host.root.join("weasel.custom.yaml").exists());
}

#[tokio::test]
async fn switching_preserves_modified_owned_paths_also_shipped_by_the_next_package() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let host = Host::new(directory.path(), Arc::new(Sink));
    store(&host, "keytao", files("alpha"));
    let mut incoming = files("alpha");
    for (name, bytes) in &mut incoming {
        if name == "alpha.custom.yaml" {
            *bytes = b"patch: {menu/page_size: 7}\n".to_vec();
        }
        if name == "lua/alpha.lua" {
            *bytes = b"return 'next package'\n".to_vec();
        }
    }
    store(&host, "xmjd", incoming);
    activate_scheme(&host.core, "keytao", &host.root)
        .await
        .unwrap();
    write(
        &host.root,
        "alpha.custom.yaml",
        b"patch: {menu/page_size: 8}\n",
    );
    write(&host.root, "lua/alpha.lua", b"return 'user edit'\n");
    let result = activate_scheme(&host.core, "xmjd", &host.root)
        .await
        .unwrap();
    let retained = package_activation::retained(&result.logs);
    for path in ["alpha.custom.yaml", "lua/alpha.lua"] {
        assert!(retained.contains(&path.into()), "{path}");
    }
    assert_eq!(
        fs::read(host.root.join("alpha.custom.yaml")).unwrap(),
        b"patch: {menu/page_size: 8}\n"
    );
    assert_eq!(
        fs::read(host.root.join("lua/alpha.lua")).unwrap(),
        b"return 'user edit'\n"
    );
    remove_scheme_package(&host.core, "xmjd", &host)
        .await
        .unwrap();
    assert_eq!(
        fs::read(host.root.join("alpha.custom.yaml")).unwrap(),
        b"patch: {menu/page_size: 8}\n"
    );
    assert_eq!(
        fs::read(host.root.join("lua/alpha.lua")).unwrap(),
        b"return 'user edit'\n"
    );
}

#[tokio::test]
async fn activating_identical_unowned_file_preserves_it_after_deactivation() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let host = Host::new(directory.path(), Arc::new(Sink));
    write(&host.root, "shared.txt", b"alpha");
    let before = package_state(&host.root);
    store(&host, "keytao", files("alpha"));
    activate_scheme(&host.core, "keytao", &host.root)
        .await
        .unwrap();
    let retained = remove_scheme_package(&host.core, "keytao", &host)
        .await
        .unwrap();
    assert!(retained.is_empty());
    assert_eq!(package_state(&host.root), before);
    assert_eq!(fs::read(host.root.join("shared.txt")).unwrap(), b"alpha");
}

#[tokio::test]
async fn switching_keeps_shared_dictionary_binaries_and_removes_a_only_outputs() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let host = Host::new(directory.path(), Arc::new(Sink));
    let dictionary = b"---\nname: shared\nversion: '1'\nsort: by_weight\n...\n";
    for (key, schema) in [("keytao", "alpha"), ("xmjd", "beta")] {
        let mut entries = files(schema);
        let source = entries
            .iter_mut()
            .find(|(path, _)| path == &format!("{schema}.schema.yaml"))
            .unwrap();
        source.1 = format!("schema: {{schema_id: {schema}}}\ntranslator: {{dictionary: shared}}\n")
            .into_bytes();
        if schema == "alpha" {
            source.1.extend_from_slice(
                b"extra_translator: {dictionary: alpha_only, prism: alpha_only}\n",
            );
            entries.push((
                "alpha_only.dict.yaml".into(),
                b"---\nname: alpha_only\nversion: '1'\n...\n".to_vec(),
            ));
        } else {
            source
                .1
                .extend_from_slice(b"extra_translator: {dictionary: beta_only}\n");
            entries.push((
                "beta_only.dict.yaml".into(),
                b"---\nname: beta_only\nversion: '1'\n...\n".to_vec(),
            ));
        }
        entries.push(("shared.dict.yaml".into(), dictionary.to_vec()));
        store(&host, key, entries);
    }
    write(&host.root, "shared.userdb/data", b"user dictionary");
    write(&host.root, "sync/device/shared.userdb.txt", b"user sync");
    activate_scheme(&host.core, "keytao", &host.root)
        .await
        .unwrap();
    let schema_source = fs::read(host.root.join("alpha.schema.yaml")).unwrap();
    write(&host.root, "build/alpha.schema.yaml", &schema_source);
    for dictionary in ["shared", "alpha_only"] {
        for extension in ["table", "prism", "reverse"] {
            write(
                &host.root,
                &format!("build/{dictionary}.{extension}.bin"),
                b"compiled dictionary",
            );
        }
    }
    for extension in ["table", "reverse"] {
        write(
            &host.root,
            &format!("build/beta_only.{extension}.bin"),
            b"stale incoming dictionary",
        );
    }
    let result = activate_scheme(&host.core, "xmjd", &host.root)
        .await
        .unwrap();
    assert!(!host.root.join("build/alpha.schema.yaml").exists());
    for extension in ["table", "prism", "reverse"] {
        assert!(
            !host
                .root
                .join(format!("build/alpha_only.{extension}.bin"))
                .exists()
        );
    }
    for extension in ["table", "reverse"] {
        assert!(
            !host
                .root
                .join(format!("build/beta_only.{extension}.bin"))
                .exists(),
            "Incoming private dictionary must still be invalidated: {extension}"
        );
    }
    assert_eq!(
        fs::read(host.root.join("shared.userdb/data")).unwrap(),
        b"user dictionary"
    );
    assert_eq!(
        fs::read(host.root.join("sync/device/shared.userdb.txt")).unwrap(),
        b"user sync"
    );
    let lost: Vec<_> = ["table", "prism", "reverse"]
        .into_iter()
        .map(|extension| format!("build/shared.{extension}.bin"))
        .filter(|path| !host.root.join(path).is_file())
        .collect();
    assert!(
        lost.is_empty(),
        "Switch removed shared dictionary outputs: {lost:?}; logs: {:?}",
        result.logs
    );
    for extension in ["table", "prism", "reverse"] {
        assert_eq!(
            fs::read(host.root.join(format!("build/shared.{extension}.bin"))).unwrap(),
            b"compiled dictionary"
        );
    }
}

#[tokio::test]
async fn deactivation_reverts_package_globals_and_keeps_later_user_global_edits() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let host = Host::new(directory.path(), Arc::new(Sink));
    write(&host.root, "default.custom.yaml", b"patch:\n  menu/page_size: 9\n  ascii_composer/good_old_caps_lock: true\n  user_setting: before\n  schema_list: []\n");
    let mut incoming = files("alpha");
    incoming.iter_mut().find(|(name, _)| name == "default.custom.yaml").unwrap().1 = b"patch:\n  menu/page_size: 6\n  ascii_composer/good_old_caps_lock: false\n  schema_list:\n    - schema: alpha\n".to_vec();
    store(&host, "keytao", incoming);
    activate_scheme(&host.core, "keytao", &host.root)
        .await
        .unwrap();
    let mut current: serde_yaml::Value =
        serde_yaml::from_slice(&fs::read(host.root.join("default.custom.yaml")).unwrap()).unwrap();
    assert_eq!(current["patch"]["menu/page_size"].as_i64(), Some(6));
    assert_eq!(
        current["patch"]["ascii_composer/good_old_caps_lock"].as_bool(),
        Some(false)
    );
    current["patch"]["user_setting"] = "after activation".into();
    current["patch"]["new_user_setting"] = true.into();
    write(
        &host.root,
        "default.custom.yaml",
        serde_yaml::to_string(&current).unwrap().as_bytes(),
    );
    remove_scheme_package(&host.core, "keytao", &host)
        .await
        .unwrap();
    let final_config: serde_yaml::Value =
        serde_yaml::from_slice(&fs::read(host.root.join("default.custom.yaml")).unwrap()).unwrap();
    assert_eq!(final_config["patch"]["menu/page_size"].as_i64(), Some(9));
    assert_eq!(
        final_config["patch"]["ascii_composer/good_old_caps_lock"].as_bool(),
        Some(true)
    );
    assert_eq!(
        final_config["patch"]["user_setting"].as_str(),
        Some("after activation")
    );
    assert_eq!(
        final_config["patch"]["new_user_setting"].as_bool(),
        Some(true)
    );
    assert!(
        crate::scheme_files::parse_schema_list(&serde_yaml::to_string(&final_config).unwrap())
            .is_empty()
    );
}

struct CopyFailure {
    root: PathBuf,
    fired: AtomicBool,
}
impl EventSink for CopyFailure {
    fn emit(&self, event: CoreEvent) {
        if let CoreEvent::InstallProgress(progress) = event {
            if progress.message.ends_with("a-first.txt") && !self.fired.swap(true, Ordering::SeqCst)
            {
                assert!(self.root.join("a-first.txt").is_file());
                panic!("injected failure after the first package file was copied");
            }
        }
    }
}

#[tokio::test]
async fn activation_partial_copy_rolls_back_on_task_failure() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let host = Host::new(
        directory.path(),
        Arc::new(CopyFailure {
            root: directory.path().join("rime"),
            fired: AtomicBool::new(false),
        }),
    );
    write(&host.root, "unowned.txt", b"keep");
    let before = snapshot(&host.root);
    let mut entries = files("alpha");
    entries.push(("a-first.txt".into(), b"first copied file".to_vec()));
    entries.push(("z-last.txt".into(), b"never copied".to_vec()));
    store(&host, "keytao", entries);
    let core = host.core.clone();
    let root = host.root.clone();
    let task = tokio::spawn(async move { activate_scheme(&core, "keytao", &root).await });
    assert!(matches!(task.await, Err(error) if error.is_panic()));
    assert_eq!(snapshot(&host.root), before);
    assert!(host.core.state().active_scheme.is_none());
    assert!(
        !PackageStore::new(&host.core)
            .root()
            .join(".activation-transaction")
            .exists()
    );
}

#[test]
fn activation_startup_recovers_an_uncommitted_journal() {
    let directory = tempfile::tempdir().unwrap();
    let host = Host::new(directory.path(), Arc::new(Sink));
    host.core
        .update_state(|state| {
            state.active_scheme = Some("keytao".into());
            state.installed_packages.insert(
                "keytao".into(),
                crate::state::InstalledPackage {
                    version: Some("v1".into()),
                },
            );
        })
        .unwrap();
    let original_state = host.core.state();
    let original_state_bytes = fs::read(host.core.env().data_dir.join("app-state.json")).unwrap();
    write(&host.root, "existing.txt", b"original");
    let store = PackageStore::new(&host.core);
    write(&store.root(), "activation.json", b"original activation");
    let transaction = Transaction::begin(
        &host.core,
        &host.root,
        BTreeSet::from(["existing.txt".into(), "new-directory/copied.txt".into()]),
    )
    .unwrap();
    write(&host.root, "existing.txt", b"partially replaced");
    write(&host.root, "new-directory/copied.txt", b"partial copy");
    write(&store.root(), "activation.json", b"partial activation");
    host.core
        .update_state(|state| {
            state.active_scheme = Some("xmjd".into());
        })
        .unwrap();
    std::mem::forget(transaction);
    let recovered = Core::new(host.core.env().clone(), Arc::new(Sink)).unwrap();
    assert_eq!(
        fs::read(host.root.join("existing.txt")).unwrap(),
        b"original"
    );
    assert!(!host.root.join("new-directory").exists());
    assert_eq!(
        fs::read(PackageStore::new(&recovered).root().join("activation.json")).unwrap(),
        b"original activation"
    );
    assert!(!store.root().join(".activation-transaction").exists());
    assert_eq!(
        fs::read(recovered.env().data_dir.join("app-state.json")).unwrap(),
        original_state_bytes
    );
    assert!(
        recovered.state() == original_state,
        "Startup must load the recovered state into memory"
    );
}

#[test]
fn failed_store_publication_preserves_previous_activation_markers() {
    let directory = tempfile::tempdir().unwrap();
    let host = Host::new(directory.path(), Arc::new(Sink));
    let store = PackageStore::new(&host.core);
    let bytes = archive_from_files(&files("alpha")).unwrap();
    crate::packages::stage_install(
        &host.core,
        &bytes,
        BTreeMap::new(),
        host.root
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    )
    .unwrap();
    write(
        &host.root,
        keytao_core::ACTIVE_SCHEME_MANIFEST,
        br#"{"scheme_key":"previous"}"#,
    );
    write(
        &store.root(),
        "active-manifest.json",
        b"previous app marker",
    );
    write(&store.root(), "activation.json", b"previous ownership");
    write(&store.root(), "cloud", b"publication blocked by a file");
    let error = crate::packages::finalize_pending(&host.core, "keytao", Some("v1"), true).unwrap_err();
    assert!(error.to_string().to_lowercase().contains("directory"), "{error}");
    assert_eq!(
        fs::read(host.root.join(keytao_core::ACTIVE_SCHEME_MANIFEST)).unwrap(),
        br#"{"scheme_key":"previous"}"#
    );
    assert_eq!(
        fs::read(store.active_manifest_path()).unwrap(),
        b"previous app marker"
    );
    assert_eq!(
        fs::read(store.root().join("activation.json")).unwrap(),
        b"previous ownership"
    );
}

#[test]
fn activation_rollback_restores_the_previous_store_version() {
    for crashed in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let host = Host::new(directory.path(), Arc::new(Sink));
        let mut old_files = files("alpha");
        old_files.push(("old-only.txt".into(), b"old package file".to_vec()));
        store(&host, "keytao", old_files);
        let store = PackageStore::new(&host.core);
        let before = snapshot(&store.package_path("keytao"));
        let mut incoming = files("beta");
        incoming.push(("new/nested.txt".into(), b"new package file".to_vec()));
        let bytes = archive_from_files(&incoming).unwrap();
        let mut transaction = Transaction::begin(&host.core, &host.root, BTreeSet::new()).unwrap();
        transaction
            .snapshot_package(
                "keytao",
                &incoming.iter().map(|(path, _)| path.clone()).collect(),
            )
            .unwrap();
        let mut meta = store.read_meta("keytao").unwrap().unwrap();
        meta.version = Some("v2".into());
        meta.zip_sha256 = sha256_reader(bytes.as_slice()).unwrap();
        meta.manifest = incoming
            .iter()
            .filter(|(path, _)| !crate::packages::excluded_from_manifest(path))
            .map(|(path, bytes)| (path.clone(), sha256_reader(bytes.as_slice()).unwrap()))
            .collect();
        store
            .write_archive(std::io::Cursor::new(&bytes), &meta, None)
            .unwrap();
        assert_eq!(
            store
                .read_meta("keytao")
                .unwrap()
                .unwrap()
                .version
                .as_deref(),
            Some("v2")
        );
        assert!(
            !store
                .package_path("keytao")
                .join("files/old-only.txt")
                .exists()
        );
        if crashed {
            std::mem::forget(transaction);
            Core::new(host.core.env().clone(), Arc::new(Sink)).unwrap();
        } else {
            drop(transaction);
        }
        assert_eq!(snapshot(&store.package_path("keytao")), before);
        assert_eq!(
            store.read_files("keytao").unwrap().1.version.as_deref(),
            Some("v1")
        );
        assert!(!store.package_path("keytao").join("files/new").exists());
    }
}

fn copy_regular_import_source(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name.to_str().is_some_and(|name| {
            matches!(name, "log" | "build" | "sync" | ".git" | ".DS_Store")
                || name.ends_with(".userdb")
        }) {
            continue;
        }
        let destination = crate::packages::safe_path(target, &name.to_string_lossy()).unwrap();
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            copy_regular_import_source(&entry.path(), &destination);
        } else if kind.is_file() {
            fs::copy(entry.path(), destination).unwrap();
        }
    }
}

#[cfg(unix)]
#[test]
fn import_copy_preserves_source_and_skips_runtime_data_and_symlinks() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    for name in [
        "nested/example.schema.yaml",
        "log/warning",
        "build/example.table.bin",
        "sync/device/data",
        "example.userdb/data",
    ] {
        write(&source, name, b"original");
    }
    std::os::unix::fs::symlink(
        source.join("nested/example.schema.yaml"),
        source.join("linked.schema.yaml"),
    )
    .unwrap();
    let before = snapshot(&source);
    let target = directory.path().join("local-source");
    copy_regular_import_source(&source, &target);
    assert_eq!(snapshot(&source), before);
    assert_eq!(
        snapshot(&target)
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["nested/example.schema.yaml"]
    );
    assert_eq!(
        fs::read(target.join("nested/example.schema.yaml")).unwrap(),
        b"original"
    );
}

#[tokio::test]
#[ignore = "Requires KEYTAO_B2_COPY_DIR containing isolated rime/ and app-state/ copies"]
async fn activation_real_data_copy_keytao_local_delete_reports_leftovers() {
    let directory = PathBuf::from(
        std::env::var_os("KEYTAO_B2_COPY_DIR").expect("Explicit isolated copy directory required"),
    );
    let directory = directory.canonicalize().unwrap();
    assert!(directory.starts_with("/private/tmp/claude-501/-Users-rea-code-keytao-org-keytao-app/348229fe-6d80-4d02-b020-e4d903e04588/scratchpad/pkgb2/"));
    assert!(
        directory.join("rime").is_dir() && directory.join("app-state/scheme-packages").is_dir()
    );
    for child in ["rime", "app-state", "resources", "cache", "local-source"] {
        let path = directory.join(child);
        if path.exists() {
            assert!(
                path.canonicalize().unwrap().starts_with(&directory),
                "Copy paths must not resolve outside the isolated directory"
            );
        }
    }
    assert!(
        !directory
            .join("app-state/scheme-packages/.activation-transaction")
            .exists(),
        "An unfinished copied activation must be rebased to the copy before startup recovery"
    );
    let host = Host::new(&directory, Arc::new(Sink));
    // Copied legacy manifests must refer to the isolated Rime directory.
    for path in [
        host.root.join(keytao_core::ACTIVE_SCHEME_MANIFEST),
        PackageStore::new(&host.core).active_manifest_path(),
        PackageStore::new(&host.core)
            .checked_activation_path()
            .unwrap(),
    ] {
        if let Ok(bytes) = fs::read(&path) {
            let mut marker: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            marker["root"] = host
                .root
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .as_ref()
                .into();
            fs::write(path, serde_json::to_vec_pretty(&marker).unwrap()).unwrap();
        }
    }
    let before = snapshot(&host.root);
    let existing_local = host
        .core
        .state()
        .installed_packages
        .keys()
        .find(|key| crate::scheme::is_local_scheme_key(key))
        .cloned();
    let (local, local_source) = match existing_local {
        Some(key) => (key, "existing copied store".to_owned()),
        None => {
            let source = directory.join("local-source");
            copy_regular_import_source(&host.root, &source);
            (
                crate::packages::import_local_scheme(&host.core, &source, "Local data copy", None)
                    .unwrap(),
                format!(
                    "import_local_scheme: {} (copied Rime regular files; log/build/sync/userdb and symlinks skipped)",
                    source.display()
                ),
            )
        }
    };
    let store = PackageStore::new(&host.core);
    let keytao_files = store.read_files("keytao").unwrap().0;
    let local_files = store.read_files(&local).unwrap().0;
    let mut touched: BTreeSet<String> = keytao_files
        .iter()
        .chain(&local_files)
        .map(|(path, _)| path.clone())
        .collect();
    touched.extend(
        [
            "default.custom.yaml",
            "default-custom.yaml",
            "rime.lua",
            "user.yaml",
            keytao_core::ACTIVE_SCHEME_MANIFEST,
            "keytao.custom.yaml",
            "keytao-dz.custom.yaml",
            "keytao-bj.custom.yaml",
            "lua/keytao_remove_filter.lua",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    let keytao_result = activate_scheme(&host.core, "keytao", &host.root)
        .await
        .unwrap();
    let local_result = activate_scheme(&host.core, &local, &host.root)
        .await
        .unwrap();
    let retained = remove_scheme_package(&host.core, &local, &host)
        .await
        .unwrap();
    let after = snapshot(&host.root);
    let leftovers: BTreeMap<_, _> = local_files
        .iter()
        .filter_map(|(path, _)| after.get(path).map(|hash| (path, hash)))
        .collect();
    let added: BTreeMap<_, _> = after
        .iter()
        .filter(|(path, _)| !before.contains_key(*path))
        .collect();
    let changed: BTreeMap<_, _> = after
        .iter()
        .filter(|(path, hash)| before.get(*path).is_some_and(|old| old != *hash))
        .collect();
    let removed: Vec<_> = before
        .keys()
        .filter(|path| !after.contains_key(*path))
        .collect();
    let unowned_changed: Vec<_> = before
        .iter()
        .filter(|(path, hash)| {
            !touched.contains(*path)
                && !path.starts_with("build/")
                && after.get(*path) != Some(*hash)
        })
        .map(|(path, _)| path)
        .collect();
    let report = serde_json::json!({
        "sequence": ["keytao", local, "delete"], "local_source": local_source, "before_count": before.len(), "after_count": after.len(),
        "kept_during_sequence": [package_activation::retained(&keytao_result.logs), package_activation::retained(&local_result.logs)],
        "leftovers": leftovers, "retained_user_modified": retained, "added": added, "changed": changed,
        "removed": removed, "unowned_changed": unowned_changed,
    });
    fs::write(
        directory.join("e2e-report.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    println!("B2_REAL_DATA_COPY {}", report);
    assert!(
        unowned_changed.is_empty(),
        "Unowned files changed; see e2e-report.json"
    );
    assert!(!store.package_path(&local).exists());
    assert!(host.core.state().active_scheme.is_none());
}
