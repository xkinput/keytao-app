use super::*;
use crate::scheme_host::{SchemeHost, tests::AddonHost};
use keytao_core::RuntimeDirs;

fn fixture() -> (tempfile::TempDir, AddonHost) {
    let directory = tempfile::tempdir().unwrap();
    let host = AddonHost::new(directory.path());
    let root = &host.root;
    let archive = archive_from_files(&[(
        "keytao.schema.yaml".into(),
        b"schema: {schema_id: keytao}\n".to_vec(),
    )])
    .unwrap();
    store_cloud_archive(
        &host.core,
        "keytao",
        Some("v1"),
        "official",
        None,
        &archive,
        root,
    )
    .unwrap();
    let data = root.join("runtime/keytao/keytao.userdb");
    fs::create_dir_all(&data).unwrap();
    fs::write(data.join("words"), b"learned content").unwrap();
    fs::write(
        root.join(RuntimeDirs::MIGRATION_MARKER),
        br#"{"version":1,"runtime_package":"keytao"}"#,
    )
    .unwrap();
    assert_eq!(
        RuntimeDirs::active_package_key(root).as_deref(),
        Some("keytao")
    );
    (directory, host)
}

#[tokio::test]
async fn removing_last_active_package_clears_marker_fallback_and_backs_up_learning_cleanly() {
    let (_directory, host) = fixture();
    let paths = remove_scheme_package(&host.core, "keytao", &host)
        .await
        .unwrap();
    assert!(paths.is_empty());
    assert!(!host.root.join("runtime/keytao").exists());
    let backups: Vec<_> = fs::read_dir(host.root.join("migrations"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(backups.len(), 1);
    assert_eq!(
        fs::read(backups[0].join("runtime/keytao/keytao.userdb/words")).unwrap(),
        b"learned content"
    );
    assert!(RuntimeDirs::active_package_key(&host.root).is_none());
    assert!(RuntimeDirs::migration_complete(&host.root));
    assert!(keytao_core::ReloadStamp::path(&host.root).is_file());
    assert!(
        !PackageStore::new(&host.core)
            .package_path("keytao")
            .exists()
    );
    assert!(host.core.state().installed_packages.is_empty());
}

#[cfg(target_os = "macos")]
#[tokio::test]
#[ignore = "requires bundled ime-jd engine"]
async fn removing_active_ime_jd_packages_skips_unconverted_fallbacks() {
    let (_directory, host) = fixture();
    let root = &host.root;
    for key in ["txjx", "xmjd"] {
        let archive = archive_from_files(&[(
            format!("{key}.schema.yaml"),
            format!("schema: {{schema_id: {key}}}\n").into_bytes(),
        )])
        .unwrap();
        store_cloud_archive(
            &host.core,
            key,
            Some("v1"),
            "official",
            None,
            &archive,
            root,
        )
        .unwrap();
    }
    let library = keytao_core::libjd::bundled_engine_library().unwrap();
    let dictionary =
        keytao_core::libjd::dictionary_directory(&host.core.env().data_dir, "xmjd").unwrap();
    fs::create_dir_all(&dictionary).unwrap();
    unsafe {
        keytao_core::jd_backend::JdBackend::build_dictionary(
            &library,
            "测试词\ta\n".as_bytes(),
            &dictionary.join("dictionary.bin"),
        )
        .unwrap();
    }
    fs::write(
        dictionary.join("schema.json"),
        br#"{"id":"xmjd","name":"Fixture"}"#,
    )
    .unwrap();
    fs::write(
        root.join(keytao_core::english_mode::SETTINGS_FILE),
        br#"{"engine":"libjd_engine","engines":{"keytao":"libjd_engine","xmjd":"libjd_engine"}}"#,
    )
    .unwrap();

    remove_scheme_package(&host.core, "keytao", &host)
        .await
        .unwrap();
    assert_eq!(
        RuntimeDirs::active_package_key(root).as_deref(),
        Some("xmjd")
    );

    remove_scheme_package(&host.core, "xmjd", &host)
        .await
        .unwrap();
    assert!(RuntimeDirs::active_package_key(root).is_none());
    assert!(host.core.state().installed_packages.contains_key("txjx"));
}

#[tokio::test]
async fn removing_local_scheme_never_retains_or_backs_up_modified_files() {
    let (_directory, host) = fixture();
    let source = host.root.join("local-source");
    fs::create_dir_all(&source).unwrap();
    fs::write(
        source.join("my_local.schema.yaml"),
        "schema:\n  schema_id: my_local\n  name: 本地测试\n",
    )
    .unwrap();
    let local_key = import_local_scheme(&host.core, &source, "本地测试", None).unwrap();
    let local_runtime = host.root.join("runtime").join(&local_key);
    fs::create_dir_all(&local_runtime).unwrap();
    fs::write(local_runtime.join("my_local.custom.yaml"), b"modified: true\n").unwrap();

    let paths = remove_scheme_package(&host.core, &local_key, &host)
        .await
        .unwrap();
    assert!(paths.is_empty());
    assert!(!local_runtime.exists());
    assert!(!host.root.join("migrations").exists());
}

#[tokio::test]
async fn repair_package_backs_up_cloud_files() {
    let (_directory, host) = fixture();
    fs::write(
        host.root.join("runtime/keytao/keytao.custom.yaml"),
        b"corrupted: [\n",
    )
    .unwrap();

    let clean_archive = archive_from_files(&[(
        "keytao.schema.yaml".into(),
        b"schema: {schema_id: keytao, name: Clean}\n".to_vec(),
    )])
    .unwrap();
    let backup = repair_package_for_redeploy_with(
        &host.core,
        &host.root,
        "keytao",
        RepairMode::BackUpRuntime,
        |key| {
            let core = host.core.clone();
            let root = host.root.clone();
            let bytes = clean_archive.clone();
            async move {
                assert_eq!(key, "keytao");
                store_cloud_archive(&core, "keytao", Some("v2"), "official", None, &bytes, &root)?;
                Ok(())
            }
        },
    )
    .await
    .unwrap();
    assert!(backup.is_some());
    assert!(!host.root.join("runtime/keytao").exists());
    let (stored, meta) = PackageStore::new(&host.core).read_files("keytao").unwrap();
    assert_eq!(meta.version.as_deref(), Some("v2"));
    assert_eq!(stored.len(), 1);
}

#[tokio::test]
async fn automatic_cloud_repair_leaves_active_runtime_in_place() {
    let (_directory, host) = fixture();
    let learned = host.root.join("runtime/keytao/keytao.userdb/words");
    let backup = repair_package_for_redeploy_with(
        &host.core,
        &host.root,
        "keytao",
        RepairMode::SourceOnly,
        |key| {
            let root = &host.root;
            let core = &host.core;
            let learned = &learned;
            async move {
                assert_eq!(RuntimeDirs::active_package_key(root).as_deref(), Some("keytao"));
                assert_eq!(fs::read(learned).unwrap(), b"learned content");
                let archive = archive_from_files(&[(
                    "keytao.schema.yaml".into(),
                    b"schema: {schema_id: keytao, name: Updated}\n".to_vec(),
                )])?;
                store_cloud_archive(core, &key, Some("v2"), "official", None, &archive, root)?;
                Ok(())
            }
        },
    )
    .await
    .unwrap();
    assert!(backup.is_none());
    assert_eq!(RuntimeDirs::active_package_key(&host.root).as_deref(), Some("keytao"));
    assert_eq!(fs::read(&learned).unwrap(), b"learned content");
    let (_, meta) = PackageStore::new(&host.core).read_files("keytao").unwrap();
    assert_eq!(meta.version.as_deref(), Some("v2"));
}

#[tokio::test]
async fn local_git_repair_preserves_live_runtime_on_download_success_and_failure() {
    for download_succeeds in [false, true] {
        let (directory, host) = fixture();
        let source = directory.path().join("git-source");
        fs::create_dir(&source).unwrap();
        let schema = source.join("fixture.schema.yaml");
        fs::write(&schema, b"schema: {schema_id: fixture, name: Original}\n").unwrap();
        let key = local_import::import_local_scheme_with(
            &host.core,
            &source,
            "Fixture",
            None,
            local_import::LocalImportOptions {
                version_override: Some("v1"),
                git: Some(GitSchemeConfig {
                    repo_url: "https://example.com/fixture.zip".into(),
                    mode: GitSchemeMode::Release,
                    branch: None,
                    asset_name: None,
                }),
                ..Default::default()
            },
        )
        .unwrap();
        fs::write(
            host.root.join(RuntimeDirs::MIGRATION_MARKER),
            serde_json::to_vec(&serde_json::json!({"version": 1, "runtime_package": key})).unwrap(),
        )
        .unwrap();
        let runtime = host.root.join("runtime").join(&key);
        let projected = host.root.join("scheme-packages/local").join(&key);
        let existing = [
            (runtime.join("build/default.yaml"), "last good build"),
            (runtime.join("fixture.userdb/words"), "learned content"),
            (runtime.join("fixture.user.dict.yaml"), "user words"),
            (runtime.join("custom_phrase.txt"), "user phrases"),
            (runtime.join("lua/stats.txt"), "live stats"),
            (projected.join("files/fixture.schema.yaml"), "last projected source"),
        ];
        for (path, contents) in &existing {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }

        let result = repair_package_for_redeploy_with(
            &host.core,
            &host.root,
            &key,
            RepairMode::SourceOnly,
            |key| {
                let core = host.core.clone();
                async move {
                    if !download_succeeds {
                        return Err(CoreError::Other("fixture download failed".into()));
                    }
                    fs::write(&schema, b"schema: {schema_id: fixture, name: Updated}\n")?;
                    local_import::import_local_scheme_with(
                        &core,
                        &source,
                        "Fixture",
                        Some(&key),
                        local_import::LocalImportOptions {
                            version_override: Some("v2"),
                            ..Default::default()
                        },
                    )?;
                    Ok(())
                }
            },
        )
        .await;

        assert_eq!(result.is_ok(), download_succeeds);
        assert_eq!(RuntimeDirs::active_package_key(&host.root).as_deref(), Some(key.as_str()));
        for (path, contents) in existing {
            assert_eq!(fs::read_to_string(path).unwrap(), contents);
        }
        let (_, meta) = PackageStore::new(&host.core).read_files(&key).unwrap();
        assert_eq!(meta.version.as_deref(), Some(if download_succeeds { "v2" } else { "v1" }));
    }
}

#[tokio::test]
async fn failed_replacement_leaves_active_package_and_learning_available() {
    let (_directory, host) = fixture();
    let root = host.user_root().unwrap();
    let archive = archive_from_files(&[(
        "xmjd6.schema.yaml".into(),
        b"schema: {schema_id: xmjd6}\n".to_vec(),
    )])
    .unwrap();
    store_cloud_archive(
        &host.core,
        "xmjd",
        Some("v1"),
        "official",
        None,
        &archive,
        &root,
    )
    .unwrap();

    host.fail_deploys
        .store(1, std::sync::atomic::Ordering::SeqCst);
    assert!(
        remove_scheme_package(&host.core, "keytao", &host)
            .await
            .is_err()
    );
    assert_eq!(
        RuntimeDirs::active_package_key(&root).as_deref(),
        Some("keytao")
    );
    assert!(
        PackageStore::new(&host.core)
            .package_path("keytao")
            .exists()
    );
    assert!(host.core.state().installed_packages.contains_key("keytao"));
    assert_eq!(
        fs::read(root.join("runtime/keytao/keytao.userdb/words")).unwrap(),
        b"learned content"
    );
}
