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
async fn repair_package_backs_up_cloud_files_and_wipes_local_runtime_cleanly() {
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
    let backup = repair_package_for_redeploy_with(&host.core, &host.root, "keytao", |key| {
        let core = host.core.clone();
        let root = host.root.clone();
        let bytes = clean_archive.clone();
        async move {
            assert_eq!(key, "keytao");
            store_cloud_archive(&core, "keytao", Some("v2"), "official", None, &bytes, &root)?;
            Ok(())
        }
    })
    .await
    .unwrap();
    assert!(backup.is_some());
    assert!(!host.root.join("runtime/keytao").exists());
    let (stored, meta) = PackageStore::new(&host.core).read_files("keytao").unwrap();
    assert_eq!(meta.version.as_deref(), Some("v2"));
    assert_eq!(stored.len(), 1);
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
