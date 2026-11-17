#[path = "support/smoke_fixture.rs"]
mod smoke_fixture;

use keytao_core::{ImeRuntime, InputContextPolicy};
use std::{path::PathBuf, sync::Mutex};

static LIBRIME: Mutex<()> = Mutex::new(());

fn fixture(name: &str) -> (ImeRuntime, PathBuf) {
    let dir = smoke_fixture::scratch_dir(name);
    smoke_fixture::write(&dir).unwrap();
    let path = dir.join(format!("{}.schema.yaml", smoke_fixture::SCHEMA_ID));
    let schema = std::fs::read_to_string(&path).unwrap().replace(
        "engine:\n",
        "  - name: full_shape\n    reset: 0\n    states: [Half, Full]\n  - name: simplification\n    reset: 0\n    states: [Traditional, Simplified]\nengine:\n",
    );
    std::fs::write(path, &schema).unwrap();
    for (id, name) in [
        ("keytao_other", "Other Chinese"),
        ("easy_en", "Easy English"),
    ] {
        std::fs::write(
            dir.join(format!("{id}.schema.yaml")),
            schema
                .replace(smoke_fixture::SCHEMA_ID, id)
                .replace(
                    &format!("dictionary: {id}"),
                    &format!("dictionary: {}", smoke_fixture::SCHEMA_ID),
                )
                .replace("KeyTao Smoke", name),
        )
        .unwrap();
    }
    std::fs::write(
        dir.join("default.custom.yaml"),
        format!("patch:\n  schema_list:\n    - schema: {}\n    - schema: keytao_other\n    - schema: easy_en\n  switcher/hotkeys: [F4]\n  menu/page_size: 20\n", smoke_fixture::SCHEMA_ID),
    )
    .unwrap();
    std::fs::write(dir.join("keytao-ime.json"), br#"{"englishMode":"schema"}"#).unwrap();
    let runtime = ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned()).with_ime_host();
    runtime.init().unwrap();
    (runtime, dir)
}

#[test]
fn sessions_and_new_clients_share_language_schema_and_declared_switches() {
    let _serial = LIBRIME.lock().unwrap_or_else(|error| error.into_inner());
    let (runtime, dir) = fixture("global-input-propagation");
    let first = runtime.create_session().unwrap();
    let second = runtime.create_session().unwrap();
    first.set_ascii_mode(true).unwrap();
    assert!(second.state().ascii_mode);
    second.set_ascii_mode(false).unwrap();
    assert!(!first.state().ascii_mode);
    keytao_core::set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "simplification", 0).unwrap();
    runtime.reload_without_deploy().unwrap();
    let menu = first
        .process_key_result(keytao_core::key_policy::XK_F4, 0)
        .unwrap();
    let traditional = menu
        .state
        .candidates
        .iter()
        .position(|c| c.text == "Traditional")
        .unwrap();
    first.select_candidate_on_page(traditional).unwrap();
    assert!(second.get_option("simplification"));
    first.select_schema("keytao_other").unwrap();
    first.set_option("full_shape", true).unwrap();
    first.set_option("simplification", true).unwrap();
    first.set_option("not_a_schema_switch", true).unwrap();
    assert_eq!(second.state().schema_id, "keytao_other");
    assert!(second.get_option("full_shape"));
    assert!(second.get_option("simplification"));
    assert!(!second.get_option("not_a_schema_switch"));
    let fresh = runtime.create_session().unwrap();
    assert_eq!(fresh.state().schema_id, "keytao_other");
    assert!(fresh.get_option("full_shape"));
    first.set_english_mode(true).unwrap();
    assert_eq!(second.state().schema_id, "easy_en");
    second.set_english_mode(false).unwrap();
    assert_eq!(first.state().schema_id, "keytao_other");
    assert!(first.get_option("full_shape"));
    let menu = first
        .process_key_result(keytao_core::key_policy::XK_F4, 0)
        .unwrap();
    let english = menu
        .state
        .candidates
        .iter()
        .position(|c| c.text == "Easy English")
        .unwrap_or_else(|| panic!("schema candidates: {:?}", menu.state.candidates));
    first.select_candidate_global(english).unwrap();
    assert_eq!(second.state().schema_id, "easy_en");
    second.set_english_mode(false).unwrap();
    assert_eq!(first.state().schema_id, "keytao_other");
    runtime.reload_without_deploy().unwrap();
    assert_eq!(fresh.state().schema_id, "keytao_other");
    assert!(fresh.get_option("full_shape"));
    drop((first, second, fresh));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn composing_sessions_defer_updates_and_cannot_publish_stale_fields() {
    let _serial = LIBRIME.lock().unwrap_or_else(|error| error.into_inner());
    let (runtime, dir) = fixture("global-input-composition");
    let first = runtime.create_session().unwrap();
    let second = runtime.create_session().unwrap();
    for key in smoke_fixture::CODE.chars() {
        second.process_key_result(key as u32, 0).unwrap();
    }
    let composing = second.state();
    assert!(!composing.preedit.is_empty());
    first.set_option("full_shape", true).unwrap();
    first.set_ascii_mode(true).unwrap();
    let deferred = second.state();
    assert_eq!(deferred.preedit, composing.preedit);
    assert!(!deferred.ascii_mode);
    second.select_candidate_on_page(0).unwrap();
    assert!(first.state().ascii_mode);
    assert!(first.get_option("full_shape"));
    assert!(second.state().ascii_mode);
    assert!(second.get_option("full_shape"));
    first.set_ascii_mode(false).unwrap();
    let menu = second
        .process_key_result(keytao_core::key_policy::XK_F4, 0)
        .unwrap();
    assert!(!menu.state.candidates.is_empty());
    first.select_schema("keytao_other").unwrap();
    assert_eq!(second.state().schema_id, menu.state.schema_id);
    second
        .process_key_result(keytao_core::key_policy::XK_ESCAPE, 0)
        .unwrap();
    assert_eq!(second.state().schema_id, "keytao_other");
    drop((first, second));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn secure_and_host_context_overrides_never_change_global_input() {
    let _serial = LIBRIME.lock().unwrap_or_else(|error| error.into_inner());
    let (runtime, dir) = fixture("global-input-context");
    let first = runtime.create_session().unwrap();
    let second = runtime.create_session().unwrap();
    second
        .set_input_policy(InputContextPolicy::sensitive())
        .unwrap();
    second.set_ascii_mode(true).unwrap();
    second.set_option("full_shape", true).unwrap();
    assert!(!second.process_key_result(b'a' as u32, 0).unwrap().accepted);
    assert!(!first.state().ascii_mode);
    assert!(!first.get_option("full_shape"));
    let restored = second
        .set_input_policy(InputContextPolicy::default())
        .unwrap();
    assert!(!restored.ascii_mode);
    assert!(!second.get_option("full_shape"));
    second.set_context_ascii_mode(Some(true)).unwrap();
    second.process_key_result(b'a' as u32, 0).unwrap();
    runtime.reload_without_deploy().unwrap();
    assert!(second.state().ascii_mode);
    assert!(!first.state().ascii_mode);
    second.set_context_ascii_mode(None).unwrap();
    assert!(!second.state().ascii_mode);
    second.set_context_ascii_mode(Some(true)).unwrap();
    second.set_english_mode(true).unwrap();
    assert_eq!(first.state().schema_id, "easy_en");
    drop((first, second));
    std::fs::remove_dir_all(dir).unwrap();
}
