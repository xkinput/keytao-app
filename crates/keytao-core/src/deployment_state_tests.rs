use super::{begin_deployment, complete_deployment, schema_install_state};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

fn installed_schema() -> PathBuf {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "keytao-deployment-state-{}-{suffix}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(dir.join("build")).unwrap();
    fs::write(
        dir.join("default.custom.yaml"),
        "patch:\n  schema_list:\n    - schema: keytao\n",
    )
    .unwrap();
    fs::write(
        dir.join("keytao.schema.yaml"),
        "schema:\n  schema_id: keytao\ntranslator:\n  dictionary: keytao_dict\n",
    )
    .unwrap();
    dir
}

fn compiled_schema(dir: &Path) {
    fs::copy(
        dir.join("keytao.schema.yaml"),
        dir.join("build/keytao.schema.yaml"),
    )
    .unwrap();
    fs::write(dir.join("build/keytao_dict.table.bin"), b"table").unwrap();
    fs::write(dir.join("build/keytao_dict.prism.bin"), b"prism").unwrap();
}

#[test]
fn schema_install_state_rejects_partial_and_interrupted_builds() {
    let dir = installed_schema();
    fs::copy(
        dir.join("keytao.schema.yaml"),
        dir.join("build/keytao.schema.yaml"),
    )
    .unwrap();
    assert!(!schema_install_state(&dir).deployed);
    fs::write(dir.join("build/.keytao-deploying"), "interrupted").unwrap();
    let partial = schema_install_state(&dir);
    assert!(partial.installed);
    assert!(!partial.deployed);
    assert!(!dir.join("build/.keytao-deployed").exists());
    compiled_schema(&dir);
    fs::write(dir.join("build/.keytao-deployed"), r#"["keytao"]"#).unwrap();
    assert!(!schema_install_state(&dir).deployed);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn schema_install_state_accepts_successful_deployment_marker() {
    let dir = installed_schema();
    compiled_schema(&dir);
    complete_deployment(&dir).unwrap();
    let marker: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("build/.keytao-deployed")).unwrap()).unwrap();
    assert_eq!(
        marker["fingerprint"],
        format!("keytao-core/{};bundled/", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(marker["schemas"], serde_json::json!(["keytao"]));
    assert!(schema_install_state(&dir).deployed);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deployment_marker_rejects_mismatched_fingerprint_without_rewriting_it() {
    let dir = installed_schema();
    compiled_schema(&dir);
    complete_deployment(&dir).unwrap();
    let path = dir.join("build/.keytao-deployed");
    let mut marker: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    marker["fingerprint"] = "previous-engine-version".into();
    let content = serde_json::to_vec(&marker).unwrap();
    fs::write(&path, &content).unwrap();
    assert!(!schema_install_state(&dir).deployed);
    assert_eq!(fs::read(&path).unwrap(), content);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deployment_marker_rejects_old_plain_schema_list() {
    let dir = installed_schema();
    compiled_schema(&dir);
    let marker = r#"["keytao"]"#;
    fs::write(dir.join("build/.keytao-deployed"), marker).unwrap();
    assert!(!schema_install_state(&dir).deployed);
    assert_eq!(
        fs::read_to_string(dir.join("build/.keytao-deployed")).unwrap(),
        marker
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deployment_sentinel_overrides_matching_fingerprint() {
    let dir = installed_schema();
    compiled_schema(&dir);
    complete_deployment(&dir).unwrap();
    fs::write(dir.join("build/.keytao-deploying"), "interrupted").unwrap();
    assert!(!schema_install_state(&dir).deployed);
    assert!(dir.join("build/.keytao-deploying").is_file());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deployment_marker_rejects_bundled_apk_version_change() {
    let dir = installed_schema();
    compiled_schema(&dir);
    fs::create_dir(dir.join("rime-data")).unwrap();
    let stamp = dir.join("rime-data/.bundled-version");
    fs::write(&stamp, "42/1.2.5").unwrap();
    complete_deployment(&dir).unwrap();
    assert!(schema_install_state(&dir).deployed);
    fs::write(&stamp, "43/1.2.6").unwrap();
    assert!(!schema_install_state(&dir).deployed);
    complete_deployment(&dir).unwrap();
    assert!(schema_install_state(&dir).deployed);
    fs::remove_file(&stamp).unwrap();
    assert!(!schema_install_state(&dir).deployed);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deploy_start_invalidates_previous_ready_build_until_completion() {
    let dir = installed_schema();
    compiled_schema(&dir);
    fs::write(dir.join("build/.keytao-deployed"), r#"["keytao"]"#).unwrap();
    begin_deployment(&dir).unwrap();
    assert!(dir.join("build/.keytao-deploying").is_file());
    assert!(!dir.join("build/.keytao-deployed").exists());
    assert!(!schema_install_state(&dir).deployed);
    complete_deployment(&dir).unwrap();
    assert!(!dir.join("build/.keytao-deploying").exists());
    assert!(schema_install_state(&dir).deployed);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn schema_install_state_requires_redeployment_of_complete_legacy_build() {
    let dir = installed_schema();
    compiled_schema(&dir);
    assert!(!dir.join("build/.keytao-deploying").exists());
    assert!(!dir.join("build/.keytao-deployed").exists());
    assert!(!schema_install_state(&dir).deployed);
    assert!(!dir.join("build/.keytao-deployed").exists());
    assert!(!dir.join("build/.keytao-deploying").exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deployment_marker_requires_compiled_schema() {
    let dir = installed_schema();
    compiled_schema(&dir);
    complete_deployment(&dir).unwrap();
    assert!(schema_install_state(&dir).deployed);
    fs::remove_file(dir.join("build/keytao.schema.yaml")).unwrap();
    assert!(!schema_install_state(&dir).deployed);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deployment_completion_keeps_sentinel_when_marker_write_fails() {
    let dir = installed_schema();
    compiled_schema(&dir);
    begin_deployment(&dir).unwrap();
    fs::create_dir(dir.join(format!("build/.keytao-deployed.{}.tmp", std::process::id()))).unwrap();
    assert!(complete_deployment(&dir).is_err());
    assert!(!schema_install_state(&dir).deployed);
    assert!(!dir.join("build/.keytao-deployed").exists());
    assert!(dir.join("build/.keytao-deploying").is_file());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn schema_install_state_ignores_optional_dependency_without_source() {
    let dir = installed_schema();
    fs::write(
        dir.join("keytao.schema.yaml"),
        "schema:\n  schema_id: keytao\n  dependencies: [optional_missing]\ntranslator:\n  dictionary: keytao_dict\n",
    )
    .unwrap();
    compiled_schema(&dir);
    assert!(!dir.join("optional_missing.schema.yaml").exists());
    assert!(!schema_install_state(&dir).deployed);
    begin_deployment(&dir).unwrap();
    complete_deployment(&dir).unwrap();
    let marker: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("build/.keytao-deployed")).unwrap()).unwrap();
    assert_eq!(marker["schemas"], serde_json::json!(["keytao"]));
    assert!(schema_install_state(&dir).deployed);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn schema_install_state_requires_marker_to_list_every_managed_primary() {
    let dir = installed_schema();
    compiled_schema(&dir);
    complete_deployment(&dir).unwrap();
    let path = dir.join("build/.keytao-deployed");
    let mut marker: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    marker["schemas"] = serde_json::json!(["stale"]);
    fs::write(&path, serde_json::to_vec(&marker).unwrap()).unwrap();
    assert!(!schema_install_state(&dir).deployed);
    fs::write(dir.join("build/.keytao-deployed"), "invalid json").unwrap();
    assert!(!schema_install_state(&dir).deployed);
    fs::remove_dir_all(dir).unwrap();
}
