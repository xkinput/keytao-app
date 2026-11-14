#[path = "support/smoke_fixture.rs"]
mod smoke_fixture;

use keytao_core::{
    import_published_schema_switches, list_schema_switches, set_schema_switch, ImeRuntime,
    SchemaSwitchSetting, PUBLISHED_SCHEMA_SWITCHES_FILE,
};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

static LIBRIME: Mutex<()> = Mutex::new(());

fn serialized() -> MutexGuard<'static, ()> {
    LIBRIME.lock().unwrap_or_else(|error| error.into_inner())
}

fn fixture(name: &str) -> (ImeRuntime, PathBuf) {
    let dir = smoke_fixture::scratch_dir(name);
    smoke_fixture::write(&dir).unwrap();
    let path = dir.join(format!("{}.schema.yaml", smoke_fixture::SCHEMA_ID));
    let schema = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        path,
        schema.replace(
            "engine:\n",
            "  - name: mars\n    reset: 1\n    states: [普通, 火星文]\n  - name: hidden_option\n    reset: 1\nengine:\n",
        ),
    )
    .unwrap();
    let runtime = ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned());
    runtime.init().unwrap();
    (runtime, dir)
}

fn switch<'a>(switches: &'a [SchemaSwitchSetting], name: &str) -> &'a SchemaSwitchSetting {
    switches.iter().find(|switch| switch.name == name).unwrap()
}

fn protected_files(dir: &Path) -> Vec<Vec<u8>> {
    [
        "default.custom.yaml".to_owned(),
        format!("{}.schema.yaml", smoke_fixture::SCHEMA_ID),
        format!("build/{}.schema.yaml", smoke_fixture::SCHEMA_ID),
    ]
    .iter()
    .map(|path| std::fs::read(dir.join(path)).unwrap())
    .collect()
}

#[test]
fn compiled_switches_include_boolean_and_radio_but_hide_stateless_items() {
    let _serialized = serialized();
    let (_runtime, dir) = fixture("switch-parsing");
    let switches = list_schema_switches(&dir).unwrap();
    assert_eq!(switches.len(), 3);
    assert_eq!(switch(&switches, "ascii_mode").states, ["中文", "西文"]);
    assert_eq!(switch(&switches, "ascii_mode").value, 0);
    assert!(switch(&switches, "ascii_mode").options.is_empty());
    let radio = switch(&switches, "punctuation_ascii");
    assert_eq!(radio.options, ["punctuation_ascii", "punctuation_cjk"]);
    assert_eq!(radio.states, ["西文标点", "中文标点"]);
    assert_eq!(radio.value, 1);
    assert_eq!(switch(&switches, "mars").value, 1);
    assert!(switches
        .iter()
        .all(|switch| switch.schema_id == smoke_fixture::SCHEMA_ID));
    assert!(!switches.iter().any(|switch| switch.name == "hidden_option"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn user_config_api_persists_boolean_and_exclusive_radio_values() {
    let _serialized = serialized();
    let (runtime, dir) = fixture("switch-persistence");
    let protected = protected_files(&dir);
    set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "ascii_mode", 1).unwrap();
    set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "mars", 0).unwrap();
    set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "punctuation_ascii", 0).unwrap();
    let user: serde_yaml::Value =
        serde_yaml::from_slice(&std::fs::read(dir.join("user.yaml")).unwrap()).unwrap();
    let options = &user["var"]["option"];
    assert_eq!(options["ascii_mode"].as_bool(), Some(true));
    assert_eq!(options["mars"].as_bool(), Some(false));
    assert_eq!(options["punctuation_ascii"].as_bool(), Some(true));
    assert_eq!(options["punctuation_cjk"].as_bool(), Some(false));
    assert!(set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "hidden_option", 1).is_err());
    assert!(set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "punctuation_ascii", 2).is_err());
    assert!(set_schema_switch(&dir, "missing_schema", "ascii_mode", 1).is_err());
    runtime.reload_without_deploy().unwrap();
    let switches = list_schema_switches(&dir).unwrap();
    assert_eq!(switch(&switches, "ascii_mode").value, 1);
    assert_eq!(switch(&switches, "mars").value, 0);
    assert_eq!(switch(&switches, "punctuation_ascii").value, 0);
    assert_eq!(protected_files(&dir), protected);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn persisted_options_apply_to_new_sessions_and_preserve_carried_reload_state() {
    let _serialized = serialized();
    let (runtime, dir) = fixture("switch-session-apply");
    let existing = runtime.create_session().unwrap();
    assert!(!existing.get_option("ascii_mode"));
    assert!(existing.get_option("punctuation_cjk"));
    set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "ascii_mode", 1).unwrap();
    set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "punctuation_ascii", 0).unwrap();
    let fresh = runtime.create_session().unwrap();
    assert!(fresh.get_option("ascii_mode"));
    assert!(fresh.get_option("punctuation_ascii"));
    assert!(!fresh.get_option("punctuation_cjk"));
    runtime.reload_without_deploy().unwrap();
    assert!(!existing.get_option("ascii_mode"));
    assert!(fresh.get_option("ascii_mode"));
    assert!(existing.get_option("punctuation_ascii"));
    assert!(!existing.get_option("punctuation_cjk"));
    set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "ascii_mode", 0).unwrap();
    set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "punctuation_ascii", 1).unwrap();
    existing.set_ascii_mode(true).unwrap();
    runtime.reload_without_deploy().unwrap();
    assert!(existing.get_option("ascii_mode"));
    assert!(fresh.get_option("ascii_mode"));
    assert!(!existing.get_option("punctuation_ascii"));
    assert!(existing.get_option("punctuation_cjk"));
    drop(existing);
    drop(fresh);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn published_options_reach_private_sessions_without_copying_desktop_user_config() {
    let _serialized = serialized();
    let (runtime, dir) = fixture("switch-sandbox");
    let protected = protected_files(&dir);
    std::fs::write(
        dir.join("user.yaml"),
        "var:\n  private_marker: keep\n  option:\n    private_option: true\n",
    )
    .unwrap();
    std::fs::write(
        dir.join(PUBLISHED_SCHEMA_SWITCHES_FILE),
        br#"{"mars":false,"punctuation_ascii":true,"punctuation_cjk":false}"#,
    )
    .unwrap();
    import_published_schema_switches(&dir).unwrap();
    let session = runtime.create_session().unwrap();
    assert!(!session.get_option("mars"));
    assert!(session.get_option("punctuation_ascii"));
    assert!(!session.get_option("punctuation_cjk"));
    let user: serde_yaml::Value =
        serde_yaml::from_slice(&std::fs::read(dir.join("user.yaml")).unwrap()).unwrap();
    assert_eq!(user["var"]["private_marker"].as_str(), Some("keep"));
    assert_eq!(user["var"]["option"]["private_option"].as_bool(), Some(true));
    assert_eq!(protected_files(&dir), protected);
    drop(session);
    std::fs::write(
        dir.join(PUBLISHED_SCHEMA_SWITCHES_FILE),
        br#"{"mars":true,"punctuation_ascii":false,"punctuation_cjk":true}"#,
    )
    .unwrap();
    runtime.reload_without_deploy().unwrap();
    import_published_schema_switches(&dir).unwrap();
    let session = runtime.create_session().unwrap();
    assert!(session.get_option("mars"));
    assert!(!session.get_option("punctuation_ascii"));
    assert!(session.get_option("punctuation_cjk"));
    drop(session);
    let saved = std::fs::read(dir.join("user.yaml")).unwrap();
    std::fs::write(
        dir.join(PUBLISHED_SCHEMA_SWITCHES_FILE),
        br#"{"mars":"invalid"}"#,
    )
    .unwrap();
    assert!(import_published_schema_switches(&dir).is_err());
    assert_eq!(std::fs::read(dir.join("user.yaml")).unwrap(), saved);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn reading_without_a_deployed_schema_returns_no_switches() {
    let dir = smoke_fixture::scratch_dir("switch-uninstalled");
    std::fs::create_dir_all(&dir).unwrap();
    assert!(list_schema_switches(&dir).unwrap().is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}
