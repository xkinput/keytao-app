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
    fs::write(dir.join("build/.keytao-deployed"), r#"["keytao"]"#).unwrap();
    assert!(schema_install_state(&dir).deployed);
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
fn schema_install_state_upgrades_complete_legacy_build_once() {
    let dir = installed_schema();
    compiled_schema(&dir);
    assert!(!dir.join("build/.keytao-deploying").exists());
    assert!(!dir.join("build/.keytao-deployed").exists());
    assert!(schema_install_state(&dir).deployed);
    let marker: Vec<String> =
        serde_json::from_slice(&fs::read(dir.join("build/.keytao-deployed")).unwrap()).unwrap();
    assert_eq!(marker, ["keytao"]);
    assert!(!dir.join("build/.keytao-deploying").exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn legacy_upgrade_defaults_prism_to_dictionary_name() {
    let dir = installed_schema();
    fs::write(
        dir.join("keytao.schema.yaml"),
        "schema:\n  schema_id: keytao\ntranslator:\n  dictionary: keytao.extended\n",
    )
    .unwrap();
    fs::copy(
        dir.join("keytao.schema.yaml"),
        dir.join("build/keytao.schema.yaml"),
    )
    .unwrap();
    fs::write(dir.join("build/keytao.extended.table.bin"), b"table").unwrap();
    fs::write(dir.join("build/keytao.extended.prism.bin"), b"prism").unwrap();
    assert!(!dir.join("build/keytao.prism.bin").exists());
    assert!(!dir.join("build/.keytao-deploying").exists());
    assert!(!dir.join("build/.keytao-deployed").exists());
    assert!(schema_install_state(&dir).deployed);
    let marker: Vec<String> =
        serde_json::from_slice(&fs::read(dir.join("build/.keytao-deployed")).unwrap()).unwrap();
    assert_eq!(marker, ["keytao"]);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn legacy_upgrade_stays_ready_when_marker_write_fails() {
    let dir = installed_schema();
    compiled_schema(&dir);
    fs::create_dir(dir.join(format!("build/.keytao-deployed.{}.tmp", std::process::id()))).unwrap();
    assert!(schema_install_state(&dir).deployed);
    assert!(!dir.join("build/.keytao-deployed").exists());
    assert!(!dir.join("build/.keytao-deploying").exists());
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
    assert!(schema_install_state(&dir).deployed);
    begin_deployment(&dir).unwrap();
    complete_deployment(&dir).unwrap();
    let marker: Vec<String> =
        serde_json::from_slice(&fs::read(dir.join("build/.keytao-deployed")).unwrap()).unwrap();
    assert_eq!(marker, ["keytao"]);
    assert!(schema_install_state(&dir).deployed);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn schema_install_state_requires_marker_to_list_every_managed_primary() {
    let dir = installed_schema();
    compiled_schema(&dir);
    fs::write(dir.join("build/.keytao-deployed"), r#"["stale"]"#).unwrap();
    assert!(!schema_install_state(&dir).deployed);
    fs::write(dir.join("build/.keytao-deployed"), "invalid json").unwrap();
    assert!(!schema_install_state(&dir).deployed);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn legacy_upgrade_requires_dictionary_and_custom_prism_artifacts() {
    let dir = installed_schema();
    fs::write(
        dir.join("keytao.schema.yaml"),
        "schema:\n  schema_id: keytao\ntranslator:\n  dictionary: keytao_dict\n  prism: keytao_custom\n",
    )
    .unwrap();
    compiled_schema(&dir);
    fs::write(dir.join("build/keytao.prism.bin"), b"prism").unwrap();
    assert!(!schema_install_state(&dir).deployed);
    fs::write(dir.join("build/keytao_custom.prism.bin"), b"prism").unwrap();
    fs::remove_file(dir.join("build/keytao_dict.table.bin")).unwrap();
    assert!(!schema_install_state(&dir).deployed);
    fs::write(dir.join("build/keytao_dict.table.bin"), b"table").unwrap();
    assert!(schema_install_state(&dir).deployed);
    fs::remove_dir_all(dir).unwrap();
}
