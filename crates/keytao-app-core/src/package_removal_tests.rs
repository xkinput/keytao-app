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
async fn removing_last_active_package_clears_marker_fallback_and_preserves_learning() {
    let (_directory, host) = fixture();
    let paths = remove_scheme_package(&host.core, "keytao", &host)
        .await
        .unwrap();
    assert_eq!(paths, ["runtime/keytao"]);
    assert_eq!(
        fs::read(host.root.join("runtime/keytao/keytao.userdb/words")).unwrap(),
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
