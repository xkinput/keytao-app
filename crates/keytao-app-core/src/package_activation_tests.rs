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
        Self::for_platform(directory, sink, Platform::MacOs)
    }

    fn for_platform(directory: &Path, sink: Arc<dyn EventSink>, platform: Platform) -> Self {
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
                platform,
            },
            sink,
        )
        .unwrap();
        Self { root, core }
    }
}

#[tokio::test]
async fn mobile_install_switch_and_delete_share_owned_generated_transactions() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    for platform in [Platform::Android, Platform::Ios] {
        let directory = tempfile::tempdir().unwrap();
        let host = Host::for_platform(directory.path(), Arc::new(Sink), platform);
        write(&host.root, "shared.txt", b"user original");
        let original = package_state(&host.root);
        for (key, schema) in [("keytao", "alpha"), ("xmjd", "beta")] {
            let entries = files(schema);
            if platform == Platform::Android {
                let archive = directory.path().join("download.zip");
                fs::write(&archive, archive_from_files(&entries).unwrap()).unwrap();
                let result = crate::install::finish_android_install(&host.core, &serde_json::json!({
                    "coreInstall": true,
                    "packageZipPath": archive,
                    "packageRoot": host.root,
                })).unwrap();
                assert!(result.verify.iter().all(|entry| entry.ok));
            } else {
                crate::install::smart_install_files(
                    &host.core, entries, host.root.to_string_lossy().into_owned(), false,
                ).await.unwrap();
            }
            {
                let pending = host.core.pending_package.lock().unwrap();
                let receipt = pending.as_ref().unwrap();
                assert!(receipt.transaction.is_some());
                assert!(receipt.activation.is_some());
            }
            crate::packages::record_package(&host.core, key, Some("v1"), true).unwrap();
            let active = package_activation::read(&host.core, &host.root).unwrap().unwrap();
            assert_eq!(active.key, key);
            assert!(active.owned.iter().any(|file| file.path == format!("{schema}.custom.yaml")));
            assert!(!active.generated.is_empty());
            assert!(!host.root.join("squirrel.custom.yaml").exists());
            assert!(!host.root.join("weasel.custom.yaml").exists());
        }
        for path in ["alpha.schema.yaml", "alpha.custom.yaml", "lua/alpha.lua"] {
            assert!(host.root.join(path).exists(), "{platform:?}: {path}");
        }
        activate_scheme(&host.core, "keytao", &host.root).await.unwrap();
        assert!(host.root.join("beta.custom.yaml").exists());
        remove_scheme_package(&host.core, "keytao", &host).await.unwrap();
        remove_scheme_package(&host.core, "xmjd", &host).await.unwrap();
        assert_eq!(package_state(&host.root), original, "{platform:?}");
    }
}

#[tokio::test]
async fn android_core_install_failure_restores_previous_activation() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let host = Host::for_platform(directory.path(), Arc::new(Sink), Platform::Android);
    store(&host, "keytao", files("alpha"));
    activate_scheme(&host.core, "keytao", &host.root).await.unwrap();
    let before = snapshot(&host.root);
    let mut entries = files("beta");
    entries.iter_mut().find(|(path, _)| path == "beta.custom.yaml").unwrap().1 = vec![0xff];
    let archive = directory.path().join("download.zip");
    fs::write(&archive, archive_from_files(&entries).unwrap()).unwrap();
    assert!(crate::install::finish_android_install(&host.core, &serde_json::json!({
        "coreInstall": true,
        "packageZipPath": archive,
        "packageRoot": host.root,
    })).is_err());
    assert_eq!(snapshot(&host.root), before);
    assert_eq!(host.core.state().active_scheme.as_deref(), Some("keytao"));
    assert_eq!(package_activation::read(&host.core, &host.root).unwrap().unwrap().key, "keytao");
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

#[tokio::test]
async fn reactivation_preserves_identical_mtimes_and_rewrites_changed_files() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let host = Host::new(directory.path(), Arc::new(Sink));
    let mut entries = files("alpha");
    store(&host, "keytao", entries.clone());
    activate_scheme(&host.core, "keytao", &host.root).await.unwrap();
    // The first reactivation also normalizes the active Lua manifest.
    activate_scheme(&host.core, "keytao", &host.root).await.unwrap();
    let mut paths: Vec<_> = [
        "alpha.schema.yaml", "alpha.custom.yaml", "lua/alpha.lua", "shared.txt",
        "default.custom.yaml", "rime.lua", keytao_core::ACTIVE_SCHEME_MANIFEST,
        ".keytao-package-lua/keytao/lua/alpha.lua",
    ].into_iter().map(|path| host.root.join(path)).collect();
    paths.push(PackageStore::new(&host.core).checked_activation_path().unwrap());
    let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
    for path in &paths {
        fs::File::options().write(true).open(path).unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified)).unwrap();
    }
    let before = snapshot(&host.root);
    activate_scheme(&host.core, "keytao", &host.root).await.unwrap();
    assert_eq!(snapshot(&host.root), before);
    for path in &paths {
        assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), modified, "{}", path.display());
    }

    for (path, content) in [
        ("alpha.user.dict.yaml", b"name: alpha.user\n".as_slice()),
        ("alpha.userdb/data", b"user database"),
        ("user.custom.yaml", b"patch: {menu/page_size: 9}\n"),
    ] {
        write(&host.root, path, content);
    }
    let user_files = snapshot(&host.root);
    entries.iter_mut().find(|(path, _)| path == "alpha.schema.yaml").unwrap().1
        .extend_from_slice(b"menu: {page_size: 8}\n");
    write(&host.root, "build/alpha.schema.yaml", b"stale schema");
    write(&host.root, "build/alpha.prism.bin", b"stale prism");
    store(&host, "keytao", entries.clone());
    activate_scheme(&host.core, "keytao", &host.root).await.unwrap();
    let schema = host.root.join("alpha.schema.yaml");
    assert_eq!(fs::read(&schema).unwrap(), entries[0].1);
    assert_ne!(fs::metadata(schema).unwrap().modified().unwrap(), modified);
    assert!(!host.root.join("build/alpha.schema.yaml").exists());
    assert!(!host.root.join("build/alpha.prism.bin").exists());
    let after = snapshot(&host.root);
    for path in ["alpha.user.dict.yaml", "alpha.userdb/data", "user.custom.yaml"] {
        assert_eq!(after[path], user_files[path], "{path}");
    }
}

#[tokio::test]
async fn reactivation_invalidates_only_changed_or_unknown_schema_builds() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let host = Host::new(directory.path(), Arc::new(Sink));
    let mut entries = files("alpha");
    entries.push(("beta.schema.yaml".into(), b"schema: {schema_id: beta}\n".to_vec()));
    store(&host, "keytao", entries.clone());
    activate_scheme(&host.core, "keytao", &host.root).await.unwrap();
    activate_scheme(&host.core, "keytao", &host.root).await.unwrap();
    let compiled = |id: &str| {
        let custom_stamp = u8::from(host.root.join(format!("{id}.custom.yaml")).is_file());
        let mut bytes = format!("__build_info: {{timestamps: {{{id}.schema: 1, {id}.custom: {custom_stamp}}}}}\n").into_bytes();
        bytes.extend_from_slice(&fs::read(host.root.join(format!("{id}.schema.yaml"))).unwrap());
        write(&host.root, &format!("build/{id}.schema.yaml"), &bytes);
        write(&host.root, &format!("build/{id}.prism.bin"), b"compiled prism");
    };
    compiled("alpha");
    compiled("beta");
    keytao_core::record_schema_input_fingerprints(&host.root, &["alpha".into(), "beta".into()]).unwrap();
    let build_paths: Vec<_> = ["alpha", "beta"].into_iter()
        .flat_map(|id| [format!("build/{id}.schema.yaml"), format!("build/{id}.prism.bin")])
        .map(|name| host.root.join(name)).collect();
    let before: Vec<_> = build_paths.iter().map(|path| fs::metadata(path).unwrap().modified().unwrap()).collect();
    activate_scheme(&host.core, "keytao", &host.root).await.unwrap();
    for (path, modified) in build_paths.iter().zip(before) {
        assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), modified, "{}", path.display());
    }
    for (name, bytes) in [
        ("alpha.user.dict.yaml", b"keep words".as_slice()),
        ("alpha.userdb/data", b"keep database"),
        ("user.custom.yaml", b"patch: {keep: true}\n"),
    ] {
        write(&host.root, name, bytes);
    }
    entries.iter_mut().find(|(name, _)| name == "alpha.schema.yaml").unwrap().1.extend_from_slice(b"menu: {page_size: 9}\n");
    store(&host, "keytao", entries);
    let result = activate_scheme(&host.core, "keytao", &host.root).await.unwrap();
    assert!(!host.root.join("build/alpha.schema.yaml").exists(), "{:?}", result.logs);
    assert!(!host.root.join("build/alpha.prism.bin").exists());
    assert!(host.root.join("build/beta.schema.yaml").exists(), "{:?}", result.logs);
    assert!(host.root.join("build/beta.prism.bin").exists());
    compiled("alpha");
    keytao_core::record_schema_input_fingerprints(&host.root, &["alpha".into(), "beta".into()]).unwrap();
    write(&host.root, "build/alpha.schema.yaml", b"schema: {schema_id: alpha}\n");
    activate_scheme(&host.core, "keytao", &host.root).await.unwrap();
    assert!(!host.root.join("build/alpha.schema.yaml").exists());
    assert!(!host.root.join("build/alpha.prism.bin").exists());
    assert!(host.root.join("build/beta.schema.yaml").exists());
    assert!(host.root.join("build/beta.prism.bin").exists());
    compiled("alpha");
    write(&host.root, "beta.custom.yaml", b"patch: {user_value: 2}\n");
    // A successful alpha-only deployment must not certify beta's stale build.
    keytao_core::record_schema_input_fingerprints(&host.root, &["alpha".into()]).unwrap();
    activate_scheme(&host.core, "keytao", &host.root).await.unwrap();
    assert!(host.root.join("build/alpha.schema.yaml").exists());
    assert!(host.root.join("build/alpha.prism.bin").exists());
    assert!(!host.root.join("build/beta.schema.yaml").exists());
    assert!(!host.root.join("build/beta.prism.bin").exists());
    assert!(fs::read_to_string(host.root.join("beta.custom.yaml")).unwrap().contains("user_value: 2"));
    for (name, bytes) in [
        ("alpha.user.dict.yaml", b"keep words".as_slice()),
        ("alpha.userdb/data", b"keep database"),
        ("user.custom.yaml", b"patch: {keep: true}\n"),
    ] {
        assert_eq!(fs::read(host.root.join(name)).unwrap(), bytes, "{name}");
    }
}

#[tokio::test]
async fn installed_package_updates_preserve_selected_schema_and_other_activations() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    for updated in ["keytao", "xmjd"] {
        let directory = tempfile::tempdir().unwrap();
        let host = Host::new(directory.path(), Arc::new(Sink));
        for (key, schema) in [("keytao", "alpha"), ("xmjd", "beta")] {
            store(&host, key, files(schema));
            activate_scheme(&host.core, key, &host.root).await.unwrap();
        }
        write(&host.root, "user.yaml", b"var: {previously_selected_schema: beta}\n");
        let mut updated_files = files(if updated == "keytao" { "alpha" } else { "beta" });
        updated_files.push(("version.txt".into(), b"v2".to_vec()));
        let bytes = archive_from_files(&updated_files).unwrap();
        store_cloud_archive(&host.core, updated, Some("v2"), "official", None, &bytes, &host.root).unwrap();
        activate_scheme(&host.core, updated, &host.root).await.unwrap();
        assert_eq!(fs::read_to_string(host.root.join("version.txt")).unwrap(), "v2");
        assert_eq!(fs::read_to_string(host.root.join("user.yaml")).unwrap(), "var: {previously_selected_schema: beta}\n");
        assert_eq!(host.core.state().active_scheme.as_deref(), Some("xmjd"));
        assert_eq!(package_activation::read_all(&host.core, &host.root).unwrap().len(), 2);
        assert_eq!(host.core.state().installed_packages[updated].version.as_deref(), Some("v2"));
    }
}

#[tokio::test]
async fn updating_package_keeps_removed_files_owned_by_another_activation() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let host = Host::new(directory.path(), Arc::new(Sink));
    for (key, schema) in [("keytao", "alpha"), ("xmjd", "beta")] {
        let mut entries = files(schema);
        entries.iter_mut().find(|(path, _)| path == "shared.txt").unwrap().1 = b"shared".to_vec();
        store(&host, key, entries);
        activate_scheme(&host.core, key, &host.root).await.unwrap();
        assert!(package_activation::read_package(&host.core, &host.root, key).unwrap().unwrap()
            .owned.iter().any(|file| file.path == "shared.txt"));
    }
    let mut entries = files("alpha");
    entries.retain(|(path, _)| path != "shared.txt");
    store(&host, "keytao", entries);
    activate_scheme(&host.core, "keytao", &host.root).await.unwrap();
    assert_eq!(fs::read(host.root.join("shared.txt")).unwrap(), b"shared");
    assert!(!package_activation::read_package(&host.core, &host.root, "keytao").unwrap().unwrap()
        .owned.iter().any(|file| file.path == "shared.txt"));
    assert!(package_activation::read_package(&host.core, &host.root, "xmjd").unwrap().unwrap()
        .owned.iter().any(|file| file.path == "shared.txt"));
    remove_scheme_package(&host.core, "keytao", &host).await.unwrap();
    assert_eq!(fs::read(host.root.join("shared.txt")).unwrap(), b"shared");
    remove_scheme_package(&host.core, "xmjd", &host).await.unwrap();
    assert!(!host.root.join("shared.txt").exists());
}

#[tokio::test]
async fn installed_package_in_use_tracks_current_schema_when_lua_entry_points_conflict() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let host = Host::new(directory.path(), Arc::new(Sink));
    for (key, schema) in [("keytao", "alpha"), ("txjx", "gamma"), ("xmjd", "beta")] {
        let mut entries = files(schema);
        if key == "txjx" {
            entries.retain(|(path, _)| path != "rime.lua");
        }
        store(&host, key, entries);
        activate_scheme(&host.core, key, &host.root).await.unwrap();
        if key == "txjx" {
            assert!(crate::packages::list_installed_packages(&host.core).unwrap()
                .iter().all(|package| package.active && package.in_use));
        }
    }
    write(&host.root, "english.schema.yaml", b"schema: {schema_id: english}\n");
    crate::addon::write_addon_schema_list(&host.root, "english", true).unwrap();
    for selected in ["beta", "alpha", "gamma", "english"] {
        keytao_core::write_current_schema(&host.root, selected).unwrap();
        let packages = crate::packages::list_installed_packages(&host.core).unwrap();
        for package in packages {
            assert!(package.active, "{} remains activated", package.scheme_key);
            let expected = match package.scheme_key.as_str() {
                "keytao" => selected == "alpha",
                "xmjd" => selected == "beta",
                "txjx" => true,
                _ => unreachable!(),
            };
            assert_eq!(package.in_use, expected, "{} with {selected}", package.scheme_key);
        }
    }
}

#[tokio::test]
async fn installed_packages_coexist_with_lua_snapshots_and_independent_removal() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let host = Host::new(directory.path(), Arc::new(Sink));
    for (key, schema) in [("keytao", "alpha"), ("xmjd", "beta")] {
        let mut entries = files(schema);
        entries.push(("lua/shared.lua".into(), format!("return '{schema}'\n").into_bytes()));
        store(&host, key, entries);
        activate_scheme(&host.core, key, &host.root).await.unwrap();
    }
    let schemas = crate::scheme_files::parse_schema_list(
        &fs::read_to_string(host.root.join("default.custom.yaml")).unwrap(),
    );
    assert_eq!(schemas, ["alpha", "beta"]);
    for schema in ["alpha", "beta"] {
        assert!(host.root.join(format!("{schema}.schema.yaml")).is_file());
    }
    let marker: serde_json::Value = serde_json::from_slice(
        &fs::read(host.root.join(keytao_core::ACTIVE_SCHEME_MANIFEST)).unwrap(),
    ).unwrap();
    for (key, schema) in [("keytao", "alpha"), ("xmjd", "beta")] {
        assert_eq!(marker["packages"][key]["schemas"], serde_json::json!([schema]));
        let snapshot = marker["packages"][key]["lua_files"]["lua/shared.lua"].as_str().unwrap();
        assert_eq!(fs::read_to_string(host.root.join(snapshot)).unwrap(), format!("return '{schema}'\n"));
    }
    for _ in 0..2 {
        let result = activate_scheme(&host.core, "keytao", &host.root).await.unwrap();
        assert!(!package_activation::retained(&result.logs).contains(&"lua/shared.lua".into()));
        assert!(package_activation::read_package(&host.core, &host.root, "keytao").unwrap().unwrap()
            .owned.iter().any(|file| file.path == "lua/shared.lua"));
    }
    remove_scheme_package(&host.core, "keytao", &host).await.unwrap();
    assert!(!host.root.join("alpha.schema.yaml").exists());
    assert!(host.root.join("beta.schema.yaml").is_file());
    assert_eq!(crate::scheme_files::parse_schema_list(
        &fs::read_to_string(host.root.join("default.custom.yaml")).unwrap()), ["beta"]);
}

#[tokio::test]
async fn deselected_local_user_schema_is_not_retained_after_reactivation_or_removal() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let host = Host::new(directory.path(), Arc::new(Sink));
    let mut entries = files("alpha");
    entries.push(("beta.schema.yaml".into(), b"schema: {schema_id: beta, name: Beta}".to_vec()));
    entries.iter_mut().find(|(path, _)| path == "default.custom.yaml").unwrap().1 = b"patch: {schema_list: [{schema: alpha}, {schema: beta}]}".to_vec();
    store(&host, "local-demo", entries);
    activate_scheme(&host.core, "local-demo", &host.root).await.unwrap();
    write(&host.root, "alpha.schema.yaml", b"schema: {schema_id: alpha, name: User edit}");
    crate::package_schemas::set_package_schemas(&host.core, "local-demo", &["beta".into()]).unwrap();
    activate_scheme(&host.core, "local-demo", &host.root).await.unwrap();
    assert_eq!(crate::scheme_files::parse_schema_list(&fs::read_to_string(host.root.join("default.custom.yaml")).unwrap()), ["beta"]);
    remove_scheme_package(&host.core, "local-demo", &host).await.unwrap();
    let config = fs::read_to_string(host.root.join("default.custom.yaml")).unwrap();
    assert!(crate::scheme_files::parse_schema_list(&config).is_empty());
}

#[tokio::test]
async fn main_package_switch_preserves_each_addon_record_and_sources() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    for id in crate::addon_store::IDS
        .iter()
        .filter(|id| keytao_core::is_addon_schema(id))
    {
        let directory = tempfile::tempdir().unwrap();
        let host = Host::new(directory.path(), Arc::new(Sink));
        store(&host, "keytao", files("alpha"));
        store(&host, "xmjd", files("beta"));
        activate_scheme(&host.core, "keytao", &host.root).await.unwrap();
        let sources = directory.path().join("official");
        fs::create_dir(&sources).unwrap();
        let module = match *id {
            "easy_en" => "lua/easy_en.lua".into(),
            "wanxiang" => "wanxiang_algebra.yaml".into(),
            "triple_pinyin_lssp" => "lua/lssp/limited.lua".into(),
            _ => format!("lua/{id}/select_character.lua"),
        };
        let dictionary = match *id {
            "triple_pinyin_lssp" => "triple_pinyin",
            _ => id,
        };
        let entries = [
            (format!("{id}.schema.yaml"), format!("schema: {{schema_id: {id}}}\ntranslator: {{dictionary: {dictionary}}}\n").into_bytes()),
            (format!("{dictionary}.dict.yaml"), format!("name: {dictionary}\nversion: '1'\n...\n").into_bytes()),
            (module, b"return true\n".to_vec()),
        ];
        let mut inventory = Vec::new();
        for (path, bytes) in &entries {
            write(&sources, path, bytes);
            write(&host.root, path, bytes);
            inventory.push(serde_json::json!({"path":path, "sha256":sha256_reader(bytes.as_slice()).unwrap(), "size":bytes.len()}));
        }
        let manifest = serde_json::to_vec(&serde_json::json!({
            "format":1, "version":"fixture", "source":"fixture", "archive_sha256":"a".repeat(64), "files":inventory, "includes_grammar":false,
        })).unwrap();
        let addon = crate::addon_store::AddonStore::new(&host.core, &host.root, id).unwrap();
        addon.commit(&sources, &manifest).unwrap();
        let default = host.root.join("default.custom.yaml");
        let content = crate::addon::update_addon_schema_list_content(&fs::read_to_string(&default).unwrap(), id, true).unwrap();
        fs::write(&default, content).unwrap();
        let record = fs::read(PackageStore::new(&host.core).addon_path(id).join("activation.json")).unwrap();
        for key in ["xmjd", "keytao"] {
            activate_scheme(&host.core, key, &host.root).await.unwrap();
            for (path, bytes) in &entries {
                assert_eq!(fs::read(host.root.join(path)).unwrap(), *bytes, "{id}: {path}");
            }
            assert_eq!(fs::read(PackageStore::new(&host.core).addon_path(id).join("activation.json")).unwrap(), record);
            let report = crate::package_orphans::report(&host.core, &host.root).unwrap();
            assert!(report.files.iter().all(|file| !entries.iter().any(|(path, _)| *path == file.path)));
        }
        remove_scheme_package(&host.core, "keytao", &host).await.unwrap();
        for (path, bytes) in &entries {
            assert_eq!(fs::read(host.root.join(path)).unwrap(), *bytes, "{id}: {path}");
        }
        assert!(crate::scheme_files::parse_schema_list(&fs::read_to_string(default).unwrap()).contains(&id.to_string()));
    }
}

fn sync_legacy_package(host: &Host) {
    let entries = files("alpha");
    store(host, "keytao", entries.clone());
    for (path, bytes) in &entries {
        if !matches!(path.as_str(), "squirrel.custom.yaml" | "weasel.custom.yaml") {
            write(&host.root, path, bytes);
        }
    }
    host.core
        .update_state(|state| state.active_scheme = Some("keytao".into()))
        .unwrap();
    let store = PackageStore::new(&host.core);
    let marker = serde_json::to_vec(&serde_json::json!({
        "root": host.root.canonicalize().unwrap(), "scheme_key": "keytao",
        "manifest": store.read_meta("keytao").unwrap().unwrap().manifest,
        "rime_lua": "require('alpha')\n",
    }))
    .unwrap();
    write(&store.root(), "active-manifest.json", &marker);
    write(&host.root, keytao_core::ACTIVE_SCHEME_MANIFEST, &marker);
    assert!(!store.checked_activation_path().unwrap().exists());
    crate::packages::sync_generated(&host.core, &host.root).unwrap();
    let active = package_activation::read(&host.core, &host.root)
        .unwrap()
        .unwrap();
    assert!(active.key.is_empty());
    let generated = active
        .generated
        .iter()
        .find(|file| file.path == "default.custom.yaml")
        .unwrap();
    assert_eq!(
        generated.sha256,
        sha256_reader(fs::File::open(host.root.join(&generated.path)).unwrap()).unwrap()
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
    assert!(host.root.join("alpha.custom.yaml").exists());
    assert!(host.root.join("lua/alpha.lua").exists());
    activate_scheme(&host.core, "keytao", &host.root)
        .await
        .unwrap();
    for path in ["alpha.schema.yaml", "alpha.custom.yaml", "lua/alpha.lua"] {
        assert_eq!(package_state(&host.root)[path], initial_a[path]);
    }
    assert!(host.root.join("beta.custom.yaml").exists());
    assert!(host.root.join("lua/beta.lua").exists());
    remove_scheme_package(&host.core, "keytao", &host)
        .await
        .unwrap();
    remove_scheme_package(&host.core, "xmjd", &host).await.unwrap();
    assert_eq!(package_state(&host.root), original);
    let default: serde_yaml::Value =
        serde_yaml::from_slice(&fs::read(host.root.join("default.custom.yaml")).unwrap()).unwrap();
    assert_eq!(default["patch"]["menu/page_size"].as_i64(), Some(9));
}

#[tokio::test]
async fn generated_sections_follow_active_set_and_restore_package_bytes() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let host = Host::new(directory.path(), Arc::new(Sink));
    crate::ime_settings::set_candidate_page_size(&host.root, Some(4)).unwrap();
    let original = b"# Package comment\r\npatch: {menu/page_size: 9}";
    let mut entries = files("keytao");
    entries
        .iter_mut()
        .find(|(path, _)| path == "keytao.schema.yaml")
        .unwrap()
        .1 = b"schema: {schema_id: keytao}\nengine: {filters: [lua_filter@*existing]}\n".to_vec();
    entries
        .iter_mut()
        .find(|(path, _)| path == "keytao.custom.yaml")
        .unwrap()
        .1 = original.to_vec();
    store(&host, "keytao", entries);
    store(&host, "xmjd", files("beta"));
    for key in ["keytao", "xmjd", "keytao"] {
        activate_scheme(&host.core, key, &host.root).await.unwrap();
        let active = package_activation::read(&host.core, &host.root)
            .unwrap()
            .unwrap();
        if key == "keytao" {
            let custom = fs::read_to_string(host.root.join("keytao.custom.yaml")).unwrap();
            assert_eq!(
                crate::managed_custom::strip(&custom).unwrap().as_bytes(),
                original
            );
            assert!(
                active
                    .generated
                    .iter()
                    .any(|f| f.path == "keytao.custom.yaml")
            );
            assert!(host.root.join("lua/keytao_remove_filter.lua").is_file());
            if package_activation::read_package(&host.core, &host.root, "xmjd").unwrap().is_some() {
                assert!(host.root.join("beta.custom.yaml").exists());
            }
        } else {
            assert!(host.root.join("keytao.custom.yaml").exists());
            assert!(host.root.join("lua/keytao_remove_filter.lua").exists());
        }
    }
    remove_scheme_package(&host.core, "keytao", &host)
        .await
        .unwrap();
    remove_scheme_package(&host.core, "xmjd", &host).await.unwrap();
    assert!(
        crate::managed_custom::generated_paths(&host.root)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn legacy_package_generated_entries_are_not_user_customizations() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let host = Host::new(directory.path(), Arc::new(Sink));
    let mut entries = files("alpha");
    entries
        .iter_mut()
        .find(|(path, _)| path == "alpha.custom.yaml")
        .unwrap()
        .1 =
        b"patch:\n  menu/page_size: 9\n  engine/filters/@next: lua_filter@*keytao_remove_filter\n"
            .to_vec();
    store(&host, "keytao", entries);
    activate_scheme(&host.core, "keytao", &host.root)
        .await
        .unwrap();
    assert_eq!(
        fs::read_to_string(host.root.join("alpha.custom.yaml")).unwrap(),
        "patch:\n  menu/page_size: 9\n"
    );
    assert!(
        remove_scheme_package(&host.core, "keytao", &host)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(!host.root.join("alpha.custom.yaml").exists());
}

#[tokio::test]
async fn deactivation_does_not_regenerate_for_restored_unowned_schema() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let host = Host::new(directory.path(), Arc::new(Sink));
    write(
        &host.root,
        "keytao.schema.yaml",
        b"engine: {filters: [lua_filter@*user]}\n",
    );
    write(
        &host.root,
        "default.custom.yaml",
        b"patch:\n  schema_list: [{schema: keytao}]\n",
    );
    store(&host, "keytao", files("keytao"));
    activate_scheme(&host.core, "keytao", &host.root)
        .await
        .unwrap();
    remove_scheme_package(&host.core, "keytao", &host)
        .await
        .unwrap();
    assert!(host.root.join("keytao.schema.yaml").exists());
    assert!(
        crate::managed_custom::generated_paths(&host.root)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn user_lua_and_kept_backups_survive_delete_and_reinstall() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let host = Host::new(directory.path(), Arc::new(Sink));
    let original_lua = b"manual_original = true\r\n";
    write(&host.root, "rime.lua", original_lua);
    write(&host.root, "shared.txt", b"original collision");
    store(&host, "keytao", files("alpha"));
    store(&host, "xmjd", files("beta"));
    activate_scheme(&host.core, "keytao", &host.root)
        .await
        .unwrap();
    write(&host.root, "shared.txt", b"manual edit");
    let manual_lua = b"require('alpha')\nmanual_edit = true\n";
    write(&host.root, "rime.lua", manual_lua);
    crate::ime_settings::set_candidate_page_size(&host.root, Some(4)).unwrap();
    crate::packages::sync_generated(&host.core, &host.root).unwrap();
    remove_scheme_package(&host.core, "keytao", &host)
        .await
        .unwrap();
    let inactive = package_activation::read(&host.core, &host.root)
        .unwrap()
        .unwrap();
    for (path, expected) in [
        ("shared.txt", b"original collision".as_slice()),
        ("rime.lua", manual_lua),
    ] {
        let backup = inactive
            .kept_backups
            .iter()
            .find(|b| b.path == path)
            .unwrap();
        assert_eq!(
            fs::read(PackageStore::new(&host.core).root().join(&backup.backup)).unwrap(),
            expected
        );
    }
    assert_eq!(fs::read(host.root.join("rime.lua")).unwrap(), original_lua);
    activate_scheme(&host.core, "xmjd", &host.root)
        .await
        .unwrap();
    assert_eq!(
        fs::read_to_string(host.root.join("rime.lua")).unwrap(),
        "require('beta')\n"
    );
    remove_scheme_package(&host.core, "xmjd", &host)
        .await
        .unwrap();
    assert_eq!(fs::read(host.root.join("rime.lua")).unwrap(), original_lua);
    assert!(
        package_activation::read(&host.core, &host.root)
            .unwrap()
            .unwrap()
            .kept_backups
            .iter()
            .any(|b| b.path == "shared.txt")
    );
}

#[tokio::test]
async fn legacy_generated_activation_switch_preserves_previous_package() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let host = Host::new(directory.path(), Arc::new(Sink));
    sync_legacy_package(&host);
    store(&host, "xmjd", files("beta"));
    let result = activate_scheme(&host.core, "xmjd", &host.root)
        .await
        .unwrap();
    for path in ["alpha.schema.yaml", "alpha.custom.yaml", "lua/alpha.lua"] {
        assert!(host.root.join(path).exists(), "{path}");
        assert!(!result.logs.contains(&format!("[REMOVED] {path}")), "{path}");
    }
    assert!(host.root.join("beta.schema.yaml").is_file());
    remove_scheme_package(&host.core, "xmjd", &host)
        .await
        .unwrap();
    assert!(!host.root.join("beta.schema.yaml").exists());
    assert_eq!(host.core.state().active_scheme.as_deref(), Some("keytao"));
}

#[tokio::test]
async fn legacy_generated_activation_allows_active_package_deletion() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let host = Host::new(directory.path(), Arc::new(Sink));
    sync_legacy_package(&host);
    assert!(
        remove_scheme_package(&host.core, "keytao", &host)
            .await
            .unwrap()
            .is_empty()
    );
    for path in [
        "alpha.schema.yaml",
        "alpha.custom.yaml",
        "lua/alpha.lua",
        "shared.txt",
    ] {
        assert!(!host.root.join(path).exists(), "{path}");
    }
    assert!(
        !PackageStore::new(&host.core)
            .package_path("keytao")
            .exists()
    );
    assert!(!host.core.state().installed_packages.contains_key("keytao"));
    assert!(host.core.state().active_scheme.is_none());
    assert!(
        package_activation::read(&host.core, &host.root)
            .unwrap()
            .is_some()
    );
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
async fn regenerate_preserves_modified_non_colliding_package_lua() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let host = Host::new(directory.path(), Arc::new(Sink));
    store(&host, "keytao", files("alpha"));
    activate_scheme(&host.core, "keytao", &host.root)
        .await
        .unwrap();
    write(&host.root, "lua/alpha.lua", b"return 'user edit'\n");

    for _ in 0..2 {
        keytao_core::regenerate_active_rime_lua(&host.root).unwrap();
        assert_eq!(
            fs::read(host.root.join("lua/alpha.lua")).unwrap(),
            b"return 'user edit'\n"
        );
    }
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
async fn switching_keeps_installed_package_dictionary_binaries() {
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
    let mut compiled = b"__build_info: {timestamps: {alpha.schema: 1, alpha.custom: 1}}\n".to_vec();
    compiled.extend_from_slice(&schema_source);
    write(&host.root, "build/alpha.schema.yaml", &compiled);
    keytao_core::record_schema_input_fingerprints(&host.root, &["alpha".into()]).unwrap();
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
    write(&host.root, "build/beta.schema.yaml",
        b"schema: {schema_id: beta}\ntranslator: {dictionary: shared}\nextra_translator: {dictionary: beta_only}\n");
    let result = activate_scheme(&host.core, "xmjd", &host.root)
        .await
        .unwrap();
    assert!(host.root.join("build/alpha.schema.yaml").exists());
    for extension in ["table", "prism", "reverse"] {
        assert!(
            host
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
    let error =
        crate::packages::finalize_pending(&host.core, "keytao", Some("v1"), true).unwrap_err();
    assert!(
        error.to_string().to_lowercase().contains("directory"),
        "{error}"
    );
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
    assert!(directory.starts_with("/private/tmp/claude-501/-Users-rea-code-keytao-org-keytao-app/348229fe-6d80-4d02-b020-e4d903e04588/scratchpad/pkgb3/"));
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
    let generated_leftovers = crate::managed_custom::generated_paths(&host.root).unwrap();
    let report = serde_json::json!({
        "sequence": ["keytao", local, "delete"], "local_source": local_source, "before_count": before.len(), "after_count": after.len(),
        "kept_during_sequence": [package_activation::retained(&keytao_result.logs), package_activation::retained(&local_result.logs)],
        "leftovers": leftovers, "retained_user_modified": retained, "added": added, "changed": changed,
        "removed": removed, "unowned_changed": unowned_changed,
        "generated_leftovers": generated_leftovers,
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
    assert!(
        generated_leftovers.is_empty(),
        "KeyTao generated customs/filter remained after deletion"
    );
}
