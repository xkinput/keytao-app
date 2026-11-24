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
async fn repair_download_failure_preserves_installed_and_active_package() {
    let (_directory, host) = fixture();
    let root = &host.root;
    let store = PackageStore::new(&host.core);
    let package = store.package_path("keytao");
    let (original_files, original_meta) = store.read_files("keytao").unwrap();
    host.core
        .update_state(|state| state.active_scheme = Some("keytao".into()))
        .unwrap();
    let original_state = host.core.state();
    let original_active = RuntimeDirs::active_package_key(root);
    let runtime_words = root.join("runtime/keytao/keytao.userdb/words");
    let projected = root.join("scheme-packages/cloud/keytao");
    fs::create_dir_all(&projected).unwrap();
    let projected_file = projected.join("last-good");
    fs::write(&projected_file, b"last projected source").unwrap();

    for publish_before_error in [false, true] {
        let error = repair_package_for_redeploy_with(
            &host.core,
            root,
            "keytao",
            RepairMode::BackUpRuntime,
            async |core, root, key| {
                if publish_before_error {
                    let archive = archive_from_files(&original_files)?;
                    store_cloud_archive(core, &key, Some("v2"), "official", None, &archive, root)?;
                }
                Err(CoreError::Other("fixture download failed".into()))
            },
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("fixture download failed"));
        assert!(package.is_dir(), "the original package directory disappeared");
        let (files, meta) = store.read_files("keytao").unwrap();
        assert_eq!(files, original_files);
        assert_eq!(meta.zip_sha256, original_meta.zip_sha256);
        assert_eq!(meta.version, original_meta.version);
        let state = host.core.state();
        assert_eq!(state.installed_packages, original_state.installed_packages);
        assert_eq!(state.active_scheme, original_state.active_scheme);
        assert_eq!(RuntimeDirs::active_package_key(root), original_active);
        assert_eq!(fs::read(&runtime_words).unwrap(), b"learned content");
        assert_eq!(fs::read(&projected_file).unwrap(), b"last projected source");
        assert!(!root.join("migrations").exists());
    }
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
        async |core, root, key| {
            assert_eq!(key, "keytao");
            store_cloud_archive(core, &key, Some("v2"), "official", None, &clean_archive, root)?;
            Ok(())
        },
    )
    .await
    .unwrap()
    .unwrap();
    let backup = Path::new(&backup);
    assert!(backup.is_dir());
    assert_eq!(
        fs::read(backup.join("runtime/keytao/keytao.userdb/words")).unwrap(),
        b"learned content"
    );
    let (_, old_meta) =
        PackageStore::read_files_at(&backup.join("package-store/keytao"), "keytao").unwrap();
    assert_eq!(old_meta.version.as_deref(), Some("v1"));
    assert!(!host.root.join("runtime/keytao").exists());
    assert_eq!(host.core.state().installed_packages["keytao"].version.as_deref(), Some("v2"));
    let (stored, meta) = PackageStore::new(&host.core).read_files("keytao").unwrap();
    assert_eq!(meta.version.as_deref(), Some("v2"));
    assert_eq!(stored.len(), 1);
}

#[tokio::test]
async fn repair_error_after_download_restores_previous_package_and_runtime() {
    assert_repair_rolls_back_after_download(true).await;
}

#[tokio::test]
async fn repair_state_save_failure_restores_previous_package_and_active_manifest() {
    assert_repair_rolls_back_after_download(false).await;
}

async fn assert_repair_rolls_back_after_download(corrupt_manifest: bool) {
    let (_directory, host) = fixture();
    let root = &host.root;
    let store = PackageStore::new(&host.core);
    let (original_files, original_meta) = store.read_files("keytao").unwrap();
    host.core
        .update_state(|state| {
            state.active_scheme = Some("keytao".into());
            state.scheme_cloud_ids.insert("keytao".into(), "fixture-id".into());
        })
        .unwrap();
    let original_state = host.core.state();
    let state_path = host.core.env().data_dir.join("app-state.json");
    let persisted_state = fs::read(&state_path).unwrap();
    let saved_state_path = state_path.with_extension("json.fixture");
    let archive = archive_from_files(&[(
        "keytao.schema.yaml".into(),
        b"schema: {schema_id: keytao, name: Fresh}\n".to_vec(),
    )])
    .unwrap();
    let projected = root.join("scheme-packages/cloud/keytao");
    fs::create_dir_all(&projected).unwrap();
    let projected_file = projected.join("last-good");
    fs::write(&projected_file, b"last projected source").unwrap();
    let active_manifest = root.join("scheme-packages/active-manifest.json");
    let (manifest_contents, expected_error): (&[u8], _) = if corrupt_manifest {
        (b"{", "EOF".into())
    } else {
        fs::rename(&state_path, &saved_state_path).unwrap();
        fs::create_dir(&state_path).unwrap();
        let error = host.core.update_state(|state| state.active_scheme = None).unwrap_err();
        (br#"{"active_package":"keytao","runtime_migration_version":1}"#, error.to_string())
    };
    fs::write(&active_manifest, manifest_contents).unwrap();

    let error = repair_package_for_redeploy_with(
        &host.core,
        root,
        "keytao",
        RepairMode::BackUpRuntime,
        async |core, root, key| {
            store_cloud_archive(core, &key, Some("v2"), "official", None, &archive, root)?;
            Ok(())
        },
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains(&expected_error), "{error}");
    assert_eq!(
        fs::read(root.join("runtime/keytao/keytao.userdb/words")).unwrap(),
        b"learned content"
    );
    assert_eq!(fs::read(projected_file).unwrap(), b"last projected source");
    let (files, meta) = store.read_files("keytao").unwrap();
    assert_eq!(files, original_files);
    assert_eq!(meta.zip_sha256, original_meta.zip_sha256);
    assert_eq!(meta.version, original_meta.version);
    let state = host.core.state();
    assert_eq!(state.installed_packages, original_state.installed_packages);
    assert_eq!(state.active_scheme, original_state.active_scheme);
    assert_eq!(state.scheme_cloud_ids, original_state.scheme_cloud_ids);
    if !corrupt_manifest {
        fs::remove_dir(&state_path).unwrap();
        fs::rename(saved_state_path, &state_path).unwrap();
    }
    assert_eq!(fs::read(state_path).unwrap(), persisted_state);
    assert_eq!(fs::read(active_manifest).unwrap(), manifest_contents);
    let migrations = root.join("migrations");
    assert!(!migrations.exists() || fs::read_dir(migrations).unwrap().next().is_none());
}

#[test]
fn repair_cross_filesystem_backup_keeps_original_for_rollback() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("store/keytao");
    let backup = directory.path().join("migrations/repair/package-store/keytao");
    let relative = "files/keytao.schema.yaml";
    fs::create_dir_all(source.join("files")).unwrap();
    fs::write(source.join(relative), b"original package").unwrap();

    let moved = backup_repair_path(source.clone(), &backup, |from, to| {
        assert_eq!(from, source);
        assert_eq!(to, backup);
        Err(std::io::ErrorKind::CrossesDevices.into())
    })
    .unwrap();

    assert!(!source.exists());
    assert_eq!(fs::read(backup.join(relative)).unwrap(), b"original package");
    fs::write(backup.join(relative), b"changed backup").unwrap();
    moved.restore().unwrap();
    assert_eq!(fs::read(source.join(relative)).unwrap(), b"original package");
}

#[cfg(unix)]
#[tokio::test]
async fn repair_removes_only_stale_staging_directories() {
    let (_directory, host) = fixture();
    let cache = host.root.join("cache");
    let stale = cache.join(".repair-stale");
    let recent = cache.join(".repair-recent");
    let in_use = cache.join(".repair-in-use");
    let unrelated = cache.join("unrelated");
    let symlink = cache.join(".repair-link");
    let regular = cache.join(".repair-file");
    for path in [&stale, &recent, &in_use, &unrelated] {
        fs::create_dir_all(path).unwrap();
        fs::write(path.join("contents"), b"fixture").unwrap();
    }
    fs::write(stale.join("lease"), b"").unwrap();
    let lease = fs::File::create(in_use.join("lease")).unwrap();
    keytao_core::lock_file(&lease, false).unwrap();
    fs::write(&regular, b"fixture").unwrap();
    std::os::unix::fs::symlink(&unrelated, &symlink).unwrap();
    for path in [&stale, &in_use, &unrelated, &regular] {
        fs::File::open(path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))
            .unwrap();
    }

    let error = repair_package_for_redeploy_with(
        &host.core,
        &host.root,
        "keytao",
        RepairMode::BackUpRuntime,
        async |_, _, _| {
            assert!(!stale.exists());
            Err(CoreError::Other("fixture download failed".into()))
        },
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("fixture download failed"));
    for path in [&recent, &in_use, &unrelated, &symlink] {
        assert_eq!(fs::read(path.join("contents")).unwrap(), b"fixture");
    }
    assert_eq!(fs::read(regular).unwrap(), b"fixture");
    drop(lease);
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
        async |core, root, key| {
            assert_eq!(RuntimeDirs::active_package_key(root).as_deref(), Some("keytao"));
            assert_eq!(fs::read(&learned).unwrap(), b"learned content");
            let archive = archive_from_files(&[(
                "keytao.schema.yaml".into(),
                b"schema: {schema_id: keytao, name: Updated}\n".to_vec(),
            )])?;
            store_cloud_archive(core, &key, Some("v2"), "official", None, &archive, root)?;
            Ok(())
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
            async |core, _, key| {
                if !download_succeeds {
                    return Err(CoreError::Other("fixture download failed".into()));
                }
                fs::write(&schema, b"schema: {schema_id: fixture, name: Updated}\n")?;
                local_import::import_local_scheme_with(
                    core,
                    &source,
                    "Fixture",
                    Some(&key),
                    local_import::LocalImportOptions {
                        version_override: Some("v2"),
                        ..Default::default()
                    },
                )?;
                Ok(())
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
