#[path = "support/smoke_fixture.rs"]
mod smoke_fixture;

use keytao_core::{
    list_schema_switches, read_schema_switch_request, schema_switch_applied_generation,
    set_schema_switch, ImeRuntime, SchemaSwitchSetting, SCHEMA_SWITCH_APPLIED_FILE,
    SCHEMA_SWITCH_REQUEST_FILE,
};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

static LIBRIME: Mutex<()> = Mutex::new(());

fn serialized() -> MutexGuard<'static, ()> {
    LIBRIME.lock().unwrap_or_else(|error| error.into_inner())
}

fn fixture(name: &str) -> (ImeRuntime, PathBuf) {
    fixture_with_mars_reset(name, true)
}

fn fixture_with_mars_reset(name: &str, reset: bool) -> (ImeRuntime, PathBuf) {
    fixture_with_config(name, reset, |_| {})
}

fn fixture_with_config(
    name: &str,
    reset: bool,
    configure: impl FnOnce(&Path),
) -> (ImeRuntime, PathBuf) {
    let dir = smoke_fixture::scratch_dir(name);
    smoke_fixture::write(&dir).unwrap();
    let path = dir.join(format!("{}.schema.yaml", smoke_fixture::SCHEMA_ID));
    let schema = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        path,
        schema.replace(
            "engine:\n",
            &format!("  - name: mars\n{}    states: [普通, 火星文]\n  - name: hidden_option\n    reset: 1\nengine:\n", if reset { "    reset: 1\n" } else { "" }),
        ),
    ).unwrap();
    std::fs::write(
        dir.join("default.yaml"),
        format!("config_version: \"1\"\nschema_list:\n  - schema: {}\nswitcher:\n  hotkeys: [F4]\n  fold_options: false\n  save_options: [mars, ascii_mode, punctuation_ascii, punctuation_cjk]\n", smoke_fixture::SCHEMA_ID),
    ).unwrap();
    configure(&dir);
    let runtime = ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned()).with_ime_host();
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
fn app_switch_write_does_not_touch_the_ime_user_config() {
    let _serialized = serialized();
    let (_runtime, dir) = fixture("switch-single-writer");
    let before = std::fs::read(dir.join("user.yaml")).ok();
    set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "mars", 0).unwrap();
    assert_eq!(std::fs::read(dir.join("user.yaml")).ok(), before);
    let request = read_schema_switch_request(&dir).unwrap();
    assert_eq!(request.generation, 1);
    assert_eq!(request.options["mars"], false);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn app_runtime_sessions_and_reloads_leave_switch_requests_for_the_ime() {
    let _serialized = serialized();
    let (ime, dir) = fixture("switch-app-runtime");
    let app = ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned());
    app.init_without_deploy().unwrap();
    let warmup = app.create_session().unwrap();
    drop(warmup);
    let user_config = || {
        let mut user: serde_yaml::Value =
            serde_yaml::from_slice(&std::fs::read(dir.join("user.yaml")).unwrap()).unwrap();
        // librime updates these timestamps during ordinary sessions/deployments.
        user["var"]["last_build_time"] = serde_yaml::Value::Null;
        user["var"]["schema_access_time"] = serde_yaml::Value::Null;
        user
    };
    let user = user_config();
    set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "mars", 0).unwrap();
    let request = std::fs::read(dir.join(SCHEMA_SWITCH_REQUEST_FILE)).unwrap();
    let unchanged = || {
        assert_eq!(user_config(), user);
        assert!(!dir.join(SCHEMA_SWITCH_APPLIED_FILE).exists());
        assert_eq!(
            std::fs::read(dir.join(SCHEMA_SWITCH_REQUEST_FILE)).unwrap(),
            request
        );
    };

    let session = app.create_session().unwrap();
    unchanged();
    assert!(session.get_option("mars"));
    app.reload_without_deploy().unwrap();
    unchanged();
    assert!(session.get_option("mars"));
    app.reload().unwrap();
    unchanged();
    assert!(session.get_option("mars"));
    keytao_core::reinitialize(
        dir.to_string_lossy().into_owned(),
        dir.to_string_lossy().into_owned(),
    )
    .unwrap();
    unchanged();
    assert!(session.get_option("mars"));
    let ime_session = ime.create_session().unwrap();
    assert!(!ime_session.get_option("mars"));
    assert_eq!(schema_switch_applied_generation(&dir).unwrap(), 1);
    assert!(session.get_option("mars"));
    drop(ime_session);
    drop(session);
    std::fs::remove_dir_all(dir).unwrap();
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
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn request_round_trip_merges_boolean_and_exclusive_radio_values() {
    let _serialized = serialized();
    let (_runtime, dir) = fixture("switch-requests");
    let protected = protected_files(&dir);
    let user = std::fs::read(dir.join("user.yaml")).ok();
    set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "ascii_mode", 1).unwrap();
    set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "mars", 0).unwrap();
    set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "punctuation_ascii", 0).unwrap();
    let request = read_schema_switch_request(&dir).unwrap();
    assert_eq!(request.generation, 3);
    assert_eq!(request.options["ascii_mode"], true);
    assert_eq!(request.options["mars"], false);
    assert_eq!(request.options["punctuation_ascii"], true);
    assert_eq!(request.options["punctuation_cjk"], false);
    assert_eq!(std::fs::read(dir.join("user.yaml")).ok(), user);
    assert!(set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "hidden_option", 1).is_err());
    assert!(set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "punctuation_ascii", 2).is_err());
    assert!(set_schema_switch(&dir, "missing_schema", "ascii_mode", 1).is_err());
    assert_eq!(read_schema_switch_request(&dir).unwrap(), request);
    assert_eq!(protected_files(&dir), protected);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn reload_applies_each_generation_to_all_live_sessions() {
    let _serialized = serialized();
    let (runtime, dir) = fixture("switch-broadcast");
    let other_runtime =
        ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned()).with_ime_host();
    other_runtime.init().unwrap();
    let first = runtime.create_session().unwrap();
    let second = other_runtime.create_session().unwrap();
    set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "mars", 1).unwrap();
    runtime.reload_without_deploy().unwrap();
    assert!(first.get_option("mars"));
    assert!(second.get_option("mars"));
    assert_eq!(schema_switch_applied_generation(&dir).unwrap(), 1);

    set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "mars", 0).unwrap();
    set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "ascii_mode", 1).unwrap();
    set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "punctuation_ascii", 0).unwrap();
    runtime.reload_without_deploy().unwrap();
    // Read the second session first, after the generation has already been acknowledged.
    assert_eq!(schema_switch_applied_generation(&dir).unwrap(), 4);
    for session in [&second, &first] {
        assert!(!session.get_option("mars"));
        assert!(session.get_option("ascii_mode"));
        assert!(session.get_option("punctuation_ascii"));
        assert!(!session.get_option("punctuation_cjk"));
    }
    let user: serde_yaml::Value =
        serde_yaml::from_slice(&std::fs::read(dir.join("user.yaml")).unwrap()).unwrap();
    assert_eq!(user["var"]["option"]["mars"].as_bool(), Some(false));
    drop(first);
    drop(second);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn applied_request_does_not_override_later_session_choices_on_rebuild() {
    let _serialized = serialized();
    let (runtime, dir) = fixture_with_mars_reset("switch-apply-once", false);
    set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "mars", 0).unwrap();
    let session = runtime.create_session().unwrap();
    assert!(!session.get_option("mars"));
    assert_eq!(schema_switch_applied_generation(&dir).unwrap(), 1);
    let menu = session.process_key_result(0xffc1, 0).unwrap(); // F4
    assert!(menu.accepted);
    let index = menu
        .state
        .candidates
        .iter()
        .position(|candidate| candidate.text == "普通")
        .unwrap();
    session.select_candidate_on_page(index).unwrap();
    assert!(session.get_option("mars"));
    let fresh = runtime.create_session().unwrap();
    assert!(fresh.get_option("mars"));
    runtime.reload_without_deploy().unwrap();
    assert!(session.get_option("mars"));
    assert!(fresh.get_option("mars"));
    assert_eq!(schema_switch_applied_generation(&dir).unwrap(), 1);
    set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "mars", 0).unwrap();
    runtime.reload().unwrap();
    assert!(!session.get_option("mars"));
    assert!(!fresh.get_option("mars"));
    assert_eq!(schema_switch_applied_generation(&dir).unwrap(), 2);
    drop(session);
    drop(fresh);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn reset_requests_survive_acknowledgment_in_a_new_process_and_engine() {
    const CHILD_ROOT: &str = "KEYTAO_RESET_SWITCH_CHILD_ROOT";
    let _serialized = serialized();
    if let Some(dir) = std::env::var_os(CHILD_ROOT) {
        let dir = PathBuf::from(dir);
        let runtime =
            ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned()).with_ime_host();
        runtime.init_without_deploy().unwrap();
        assert_eq!(schema_switch_applied_generation(&dir).unwrap(), 3);
        let first = runtime.create_session().unwrap();
        assert!(!first.get_option("mars"));
        assert!(first.get_option("ascii_mode"));
        assert!(first.get_option("punctuation_ascii"));
        assert!(!first.get_option("punctuation_cjk"));
        first.set_option("mars", true);
        first.set_option("ascii_mode", false);
        first.set_option("punctuation_ascii", false);
        first.set_option("punctuation_cjk", true);
        let second = runtime.create_session().unwrap();
        assert!(!second.get_option("mars"));
        assert!(second.get_option("ascii_mode"));
        assert!(second.get_option("punctuation_ascii"));
        assert!(!second.get_option("punctuation_cjk"));
        assert!(first.get_option("mars"));
        runtime.reload_without_deploy().unwrap();
        for session in [&first, &second] {
            assert!(!session.get_option("mars"));
            assert!(session.get_option("ascii_mode"));
            assert!(session.get_option("punctuation_ascii"));
            assert!(!session.get_option("punctuation_cjk"));
        }
        runtime.reload().unwrap();
        assert!(!first.get_option("mars"));
        assert!(first.get_option("ascii_mode"));
        assert_eq!(schema_switch_applied_generation(&dir).unwrap(), 3);
        return;
    }

    let (runtime, dir) = fixture("switch-reset-restart");
    set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "mars", 0).unwrap();
    set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "ascii_mode", 1).unwrap();
    set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "punctuation_ascii", 0).unwrap();
    let session = runtime.create_session().unwrap();
    assert!(!session.get_option("mars"));
    assert_eq!(schema_switch_applied_generation(&dir).unwrap(), 3);
    drop(session);
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "reset_requests_survive_acknowledgment_in_a_new_process_and_engine",
            "--nocapture",
        ])
        .env(CHILD_ROOT, &dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child process failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn reset_requests_are_defaults_after_schema_and_english_mode_changes() {
    let _serialized = serialized();
    let (runtime, dir) = fixture_with_config("switch-reset-schema-load", true, |dir| {
        let default = std::fs::read_to_string(dir.join("default.yaml")).unwrap();
        std::fs::write(
            dir.join("default.yaml"),
            format!("{default}menu:\n  page_size: 20\n"),
        )
        .unwrap();
        let schema =
            std::fs::read_to_string(dir.join(format!("{}.schema.yaml", smoke_fixture::SCHEMA_ID)))
                .unwrap();
        std::fs::write(
            dir.join("easy_en.schema.yaml"),
            schema
                .replace(
                    &format!("schema_id: {}", smoke_fixture::SCHEMA_ID),
                    "schema_id: easy_en",
                )
                .replace("name: KeyTao Smoke", "name: Easy English"),
        )
        .unwrap();
        std::fs::write(
            dir.join("default.custom.yaml"),
            format!(
                "patch:\n  schema_list:\n    - schema: {}\n    - schema: easy_en\n",
                smoke_fixture::SCHEMA_ID
            ),
        )
        .unwrap();
        std::fs::write(dir.join("keytao-ime.json"), br#"{"englishMode":"schema"}"#).unwrap();
    });
    runtime.reload_without_deploy().unwrap();
    let initial = runtime.create_session().unwrap();
    initial.select_schema(smoke_fixture::SCHEMA_ID).unwrap();
    drop(initial);
    set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "mars", 0).unwrap();
    set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "punctuation_ascii", 0).unwrap();
    let session = runtime.create_session().unwrap();
    let menu = session.process_key_result(0xffc1, 0).unwrap(); // F4
    let index = menu
        .state
        .candidates
        .iter()
        .position(|candidate| candidate.text == "Easy English")
        .unwrap_or_else(|| panic!("schema candidates: {:?}", menu.state.candidates));
    session.select_candidate_on_page(index).unwrap();
    assert_eq!(session.current_schema().unwrap().id, "easy_en");
    assert!(!session.get_option("mars"));
    session.select_schema(smoke_fixture::SCHEMA_ID).unwrap();
    session.set_option("mars", true);
    session.select_schema("easy_en").unwrap();
    assert!(!session.get_option("mars"));
    session.set_option("mars", true);
    session.select_schema(smoke_fixture::SCHEMA_ID).unwrap();
    assert!(!session.get_option("mars"));
    assert!(session.get_option("punctuation_ascii"));
    assert!(!session.get_option("punctuation_cjk"));
    session.set_option("mars", true);
    session.set_english_mode(true).unwrap();
    assert_eq!(session.current_schema().unwrap().id, "easy_en");
    session.set_english_mode(false).unwrap();
    assert_eq!(
        session.current_schema().unwrap().id,
        smoke_fixture::SCHEMA_ID
    );
    assert!(!session.get_option("mars"));
    assert_eq!(schema_switch_applied_generation(&dir).unwrap(), 2);
    drop(session);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn ui_reads_non_reset_pending_then_user_config_then_default_without_writing() {
    let _serialized = serialized();
    let (_runtime, dir) = fixture_with_mars_reset("switch-ui", false);
    assert_eq!(
        switch(&list_schema_switches(&dir).unwrap(), "mars").value,
        0
    );
    let user = b"var:\n  option:\n    mars: false\n";
    std::fs::write(dir.join("user.yaml"), user).unwrap();
    assert_eq!(
        switch(&list_schema_switches(&dir).unwrap(), "mars").value,
        0
    );
    // File reads must bypass librime's cached user configuration.
    std::fs::write(dir.join("user.yaml"), b"var:\n  option:\n    mars: true\n").unwrap();
    assert_eq!(
        switch(&list_schema_switches(&dir).unwrap(), "mars").value,
        1
    );
    set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "mars", 0).unwrap();
    let switches = list_schema_switches(&dir).unwrap();
    assert_eq!(switch(&switches, "mars").value, 0);
    assert_eq!(switch(&switches, "ascii_mode").value, 0);
    std::fs::write(dir.join(SCHEMA_SWITCH_APPLIED_FILE), "1").unwrap();
    assert_eq!(
        switch(&list_schema_switches(&dir).unwrap(), "mars").value,
        1
    );
    std::fs::write(dir.join("user.yaml"), user).unwrap();
    assert_eq!(
        switch(&list_schema_switches(&dir).unwrap(), "mars").value,
        0
    );
    assert_eq!(std::fs::read(dir.join("user.yaml")).unwrap(), user);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sandbox_request_is_consumed_once_and_ignores_options_outside_the_schema() {
    let _serialized = serialized();
    let (runtime, dir) = fixture("switch-sandbox");
    let protected = protected_files(&dir);
    std::fs::write(
        dir.join("user.yaml"),
        "var:\n  private_marker: keep\n  option:\n    private_option: true\n",
    )
    .unwrap();
    let write_request = |generation, mars| {
        std::fs::write(
            dir.join(SCHEMA_SWITCH_REQUEST_FILE),
            serde_json::to_vec(&serde_json::json!({
                "generation": generation,
                "options": {"mars": mars, "punctuation_ascii": true,
                            "punctuation_cjk": false, "unknown_option": true}
            }))
            .unwrap(),
        )
        .unwrap();
    };
    write_request(7, false);
    let session = runtime.create_session().unwrap();
    assert!(!session.get_option("mars"));
    assert!(session.get_option("punctuation_ascii"));
    assert!(!session.get_option("punctuation_cjk"));
    assert!(!session.get_option("unknown_option"));
    assert_eq!(schema_switch_applied_generation(&dir).unwrap(), 7);
    let user: serde_yaml::Value =
        serde_yaml::from_slice(&std::fs::read(dir.join("user.yaml")).unwrap()).unwrap();
    assert_eq!(user["var"]["private_marker"].as_str(), Some("keep"));
    assert_eq!(
        user["var"]["option"]["private_option"].as_bool(),
        Some(true)
    );
    write_request(6, true);
    let fresh = runtime.create_session().unwrap();
    assert_eq!(schema_switch_applied_generation(&dir).unwrap(), 7);
    write_request(8, false);
    runtime.reload_without_deploy().unwrap();
    assert!(!fresh.get_option("mars"));
    assert_eq!(schema_switch_applied_generation(&dir).unwrap(), 8);
    assert_eq!(protected_files(&dir), protected);
    let saved = std::fs::read(dir.join("user.yaml")).unwrap();
    std::fs::write(
        dir.join(SCHEMA_SWITCH_REQUEST_FILE),
        br#"{"generation":9,"options":{"mars":"invalid"}}"#,
    )
    .unwrap();
    assert!(runtime.create_session().is_err());
    assert_eq!(std::fs::read(dir.join("user.yaml")).unwrap(), saved);
    assert_eq!(schema_switch_applied_generation(&dir).unwrap(), 8);
    drop(session);
    drop(fresh);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn reading_without_a_deployed_schema_returns_no_switches() {
    let dir = smoke_fixture::scratch_dir("switch-uninstalled");
    std::fs::create_dir_all(&dir).unwrap();
    assert!(list_schema_switches(&dir).unwrap().is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}
