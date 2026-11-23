//! Candidate selection, paging and input-context policy against a fixture that
//! makes the synthesized-key fallbacks visibly wrong.
//!
//! The fixture publishes no `menu/select_keys`, keeps `-`, `=` and the digits
//! in the speller's alphabet and has no `key_binder/bindings`, so every
//! fallback stroke lands in the composition instead of doing what it was meant
//! to do. A page also holds more candidates than `DEFAULT_SELECT_KEYS`, so the
//! last one of a page cannot be reached by a select key at all. An assertion
//! below can therefore only hold when librime's own entry points ran.
//!
//! Needs the librime the crate links against at run time, e.g.
//! `DYLD_FALLBACK_LIBRARY_PATH="$RIME_LIB_DIR" cargo test -p keytao-core`.

#[path = "support/smoke_fixture.rs"]
mod smoke_fixture;

use keytao_core::{
    ImeRuntime, ImeRuntimeSession, ImeState, InputContextPolicy, SchemaSwitch, DEFAULT_SELECT_KEYS,
};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// There is one librime per process, so the tests in this binary take turns.
static LIBRIME: Mutex<()> = Mutex::new(());

fn serialized() -> MutexGuard<'static, ()> {
    LIBRIME.lock().unwrap_or_else(PoisonError::into_inner)
}

fn deployed_runtime(name: &str) -> (ImeRuntime, std::path::PathBuf) {
    let dir = smoke_fixture::scratch_dir(name);
    smoke_fixture::write(&dir).expect("write the fixture");
    let runtime = ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned());
    runtime.init().expect("deploy the fixture");
    (runtime, dir)
}

fn deployed_runtime_with_user_dictionary(name: &str) -> (ImeRuntime, std::path::PathBuf) {
    let dir = smoke_fixture::scratch_dir(name);
    smoke_fixture::write_with_user_dictionary(&dir, true).expect("write the fixture");
    let runtime = ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned());
    runtime.init().expect("deploy the fixture");
    (runtime, dir)
}

fn type_code(session: &ImeRuntimeSession) -> ImeState {
    session.reset().expect("session has no engine");
    let mut state = ImeState::empty();
    for character in smoke_fixture::CODE.chars() {
        state = session
            .process_key_result(character as u32, 0)
            .expect("session has no engine")
            .state;
    }
    state
}

#[test]
fn smoke_key_sequence_matches_rime_golden_states() {
    let _serialized = serialized();
    let (runtime, dir) = deployed_runtime("rime-golden");
    let session = runtime.create_session().expect("create a session");
    let states = b"aa ".map(|key| session.process_key_result(u32::from(key), 0).unwrap().state);
    drop(session);
    drop(runtime);
    std::fs::remove_dir_all(dir).unwrap();

    let golden: serde_json::Value =
        serde_json::from_str(include_str!("support/rime_smoke_golden.json")).unwrap();
    assert_eq!(serde_json::to_value(states).unwrap(), golden);
}

#[cfg(target_os = "macos")]
#[test]
fn runtime_uses_bundled_engine_only_for_converted_schemes_in_ime_jd_mode() {
    use keytao_core::libjd;

    let _serialized = serialized();
    let (_, dir) = deployed_runtime("libjd-routing");
    let package_store = keytao_core::package_store::root(&dir);
    std::fs::create_dir_all(&package_store).unwrap();
    std::fs::write(
        package_store.join("active-manifest.json"),
        br#"{"active_package":"keytao"}"#,
    )
    .unwrap();
    let settings = dir.join(keytao_core::english_mode::SETTINGS_FILE);
    std::fs::write(&settings, br#"{"engines":{"keytao":"libjd"}}"#).unwrap();
    let runtime = ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned()).with_ime_host();
    runtime.reload_without_deploy().unwrap();
    let session = runtime.create_session().unwrap();
    assert_eq!(type_code(&session).schema_id, smoke_fixture::SCHEMA_ID);

    std::fs::write(
        &settings,
        br#"{"engine":"libjd_engine"}"#,
    )
    .unwrap();
    runtime.reload_without_deploy().unwrap();

    if let Some(library) = libjd::bundled_engine_library() {
        assert!(!runtime.is_initialized());
        assert!(session.process_key_result(b'a'.into(), 0).is_none());
        unsafe {
            let api = &*librime_sys::rime_get_api();
            assert_eq!((api.create_session.unwrap())(), 0);
        }
        runtime.init_without_deploy().unwrap();
        let idle = runtime.create_session().unwrap();
        assert!(idle.process_key_result(b'a'.into(), 0).is_none());
        std::fs::write(&settings, br#"{"engine":"rime"}"#).unwrap();
        runtime.reload_without_deploy().unwrap();
        assert_eq!(type_code(&session).schema_id, smoke_fixture::SCHEMA_ID);
        assert_eq!(idle.state().schema_id, smoke_fixture::SCHEMA_ID);

        std::fs::create_dir_all(package_store.join("cloud/keytao/files")).unwrap();
        let dictionary = libjd::dictionary_directory(&dir, "keytao").unwrap();
        std::fs::create_dir_all(&dictionary).unwrap();
        let blob = dictionary.join("dictionary.bin");
        std::fs::write(
            dictionary.join("schema.json"),
            br#"{"id":"converted_fixture","name":"Converted Fixture"}"#,
        )
        .unwrap();
        let composing = type_code(&session);
        assert_eq!(composing.candidates[0].text, smoke_fixture::WORDS[0]);
        let rime_generation = runtime.generation();
        std::fs::write(&blob, b"invalid dictionary").unwrap();
        std::fs::write(
            &settings,
            br#"{"engine":"libjd_engine","engines":{"keytao":"libjd_engine"}}"#,
        )
        .unwrap();
        let failed_reload = runtime.reload_without_deploy();
        assert_eq!(runtime.generation(), rime_generation);
        let retained = session.state();
        assert_eq!(retained.preedit, composing.preedit);
        assert_eq!(retained.candidates[0].text, smoke_fixture::WORDS[0]);
        assert!(failed_reload.is_err());

        unsafe {
            keytao_core::jd_backend::JdBackend::build_dictionary(&library, b"converted\ta\n", &blob)
        }
        .unwrap();
        runtime.reload_without_deploy().unwrap();
        assert!(runtime.generation() > rime_generation);
        assert_eq!(session.state().schema_id, "converted_fixture");
        assert!(session.state().preedit.is_empty());
        unsafe {
            let api = &*librime_sys::rime_get_api();
            assert_eq!((api.create_session.unwrap())(), 0);
        }
        assert_eq!(keytao_core::deployed_schemas(&dir)[0].id, "converted_fixture");
        assert_eq!(
            keytao_core::current_schema_id_from_dir(&dir).as_deref(),
            Some("converted_fixture")
        );
        let typed = session.process_key_result(b'a'.into(), 0).unwrap().state;
        assert_eq!(typed.candidates[0].text, "converted");
        let jd_generation = runtime.generation();
        let good_blob = std::fs::read(&blob).unwrap();
        std::fs::write(&blob, b"invalid dictionary").unwrap();
        assert!(runtime.reload_without_deploy().is_err());
        assert_eq!(runtime.generation(), jd_generation);
        let retained = session.state();
        assert_eq!(retained.preedit, typed.preedit);
        assert_eq!(retained.candidates[0].text, "converted");
        std::fs::write(&blob, good_blob).unwrap();

        std::fs::write(
            &settings,
            br#"{"engine":"rime","engines":{"keytao":"libjd_engine"}}"#,
        )
        .unwrap();
        runtime.reload_without_deploy().unwrap();
        assert!(runtime.generation() > jd_generation);
        assert!(session.state().preedit.is_empty());
        unsafe {
            let api = &*librime_sys::rime_get_api();
            let native = (api.create_session.unwrap())();
            assert_ne!(native, 0);
            (api.destroy_session.unwrap())(native);
        }
        let typed = type_code(&session);
        assert_eq!(typed.schema_id, smoke_fixture::SCHEMA_ID);
        assert_eq!(typed.candidates[0].text, smoke_fixture::WORDS[0]);
        assert!(blob.is_file());

        std::fs::write(
            &settings,
            br#"{"engine":"libjd_engine","engines":{"keytao":"libjd_engine"}}"#,
        )
        .unwrap();
        runtime.reload_without_deploy().unwrap();
        assert_eq!(session.state().schema_id, "converted_fixture");
        let saved = dir.join("test-saved-build");
        std::fs::rename(dir.join("build"), &saved).unwrap();
        std::fs::write(
            &settings,
            br#"{"engine":"rime","engines":{"keytao":"libjd_engine"}}"#,
        )
        .unwrap();
        runtime.reload_without_deploy().unwrap();
        assert!(!runtime.is_initialized());
        assert!(session.process_key_result(b'a'.into(), 0).is_none());
        assert!(!keytao_core::schema_install_state(&dir).deployed);
        assert!(blob.is_file());
        std::fs::rename(saved, dir.join("build")).unwrap();
        runtime.reload_without_deploy().unwrap();
        assert_eq!(type_code(&session).schema_id, smoke_fixture::SCHEMA_ID);
    } else {
        assert!(runtime.is_initialized());
        assert_eq!(type_code(&session).schema_id, smoke_fixture::SCHEMA_ID);
    }
    runtime.shutdown().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn chord_keys_accumulate_until_release_and_schema_switches_update_capability() {
    let _serialized = serialized();
    let dir = smoke_fixture::scratch_dir("chord-release");
    smoke_fixture::write(&dir).expect("write the fixture");
    let normal =
        std::fs::read_to_string(dir.join(format!("{}.schema.yaml", smoke_fixture::SCHEMA_ID)))
            .unwrap();
    let chord = normal
        .replace(smoke_fixture::SCHEMA_ID, "keytao_chord")
        .replace("KeyTao Smoke", "KeyTao Chord")
        .replace("    - speller", "    - chord_composer\n    - speller")
        + "chord_composer:\n  alphabet: 'abs '\n  algebra:\n    - xform/^ab$/aa/\n";
    std::fs::write(dir.join("keytao_chord.schema.yaml"), chord).unwrap();
    let dictionary =
        std::fs::read_to_string(dir.join(format!("{}.dict.yaml", smoke_fixture::SCHEMA_ID)))
            .unwrap();
    std::fs::write(
        dir.join("keytao_chord.dict.yaml"),
        dictionary.replace(smoke_fixture::SCHEMA_ID, "keytao_chord"),
    )
    .unwrap();
    std::fs::write(
        dir.join("default.custom.yaml"),
        format!(
            "patch:\n  schema_list:\n    - schema: {}\n    - schema: keytao_chord\n  switcher/hotkeys: [F4]\n",
            smoke_fixture::SCHEMA_ID
        ),
    )
    .unwrap();
    let runtime = ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned());
    runtime.init().expect("deploy the chord fixture");
    let session = runtime.create_session().expect("create a session");
    session.select_schema("keytao_chord").unwrap();
    let unmatched = session
        .process_key_result(b'a' as u32, keytao_core::RIME_RELEASE_MASK)
        .unwrap();
    assert!(!unmatched.accepted);
    assert!(unmatched.state.preedit.is_empty());
    assert!(unmatched.state.candidates.is_empty());
    assert!(unmatched.state.committed.is_none());
    for key in [b'a', b'b'] {
        let result = session.process_key_result(key as u32, 0).unwrap();
        assert!(result.accepted);
        assert!(result.state.committed.is_none());
        assert!(result.state.candidates.is_empty());
    }
    for key in [b'a', b'b'] {
        let state = session.state();
        assert!(keytao_core::key_policy::should_forward_chord_release(
            key as u32, 0, &state
        ));
        let result = session
            .process_key_result(key as u32, keytao_core::RIME_RELEASE_MASK)
            .unwrap();
        assert!(result.accepted);
        assert!(result.state.committed.is_none());
        if key == b'a' {
            assert!(result.state.candidates.is_empty());
        }
    }
    assert_eq!(session.state().preedit, smoke_fixture::CODE);
    assert_eq!(session.state().candidates.len(), smoke_fixture::PAGE_SIZE);
    let committed = session.select_candidate_on_page(0).unwrap();
    assert_eq!(
        committed.committed.as_deref(),
        Some(smoke_fixture::WORDS[0])
    );
    session.process_key_result(b'a' as u32, 0).unwrap();
    let control_l = 0xffe3;
    session
        .process_key_result(control_l, keytao_core::RIME_MOD_CONTROL)
        .unwrap();
    assert!(keytao_core::key_policy::should_forward_chord_release(
        b'a' as u32,
        keytao_core::RIME_MOD_CONTROL,
        &session.state(),
    ));
    let modified_release = session
        .process_key_result(
            b'a' as u32,
            keytao_core::RIME_MOD_CONTROL | keytao_core::RIME_RELEASE_MASK,
        )
        .unwrap();
    assert!(!modified_release.accepted);
    assert!(modified_release.state.preedit.is_empty());
    session
        .process_key_result(control_l, keytao_core::RIME_RELEASE_MASK)
        .unwrap();
    let pressed = session.process_key_result(b's' as u32, 0).unwrap();
    assert!(pressed.accepted);
    let released = session
        .process_key_result(b's' as u32, keytao_core::RIME_RELEASE_MASK)
        .unwrap();
    assert!(released.accepted);
    assert_eq!(released.state.preedit, "s");
    let normal = session.select_schema(smoke_fixture::SCHEMA_ID).unwrap();
    assert!(!normal.chord_composer);
    let chord = session.select_schema("keytao_chord").unwrap();
    assert!(chord.chord_composer);
    for (name, schema_id, has_chord) in [
        ("KeyTao Smoke", smoke_fixture::SCHEMA_ID, false),
        ("KeyTao Chord", "keytao_chord", true),
    ] {
        let menu = session
            .process_key_result(keytao_core::key_policy::XK_F4, 0)
            .unwrap();
        assert!(menu.accepted);
        let index = menu
            .state
            .candidates
            .iter()
            .position(|candidate| candidate.text == name)
            .expect("schema appears in the Rime switcher");
        let state = session.select_candidate_on_page(index).unwrap();
        assert_eq!(state.schema_id, schema_id);
        assert_eq!(state.chord_composer, has_chord);
        assert_eq!(session.state().chord_composer, has_chord);
    }
    drop(session);
    drop(runtime);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn panel_arrows_navigate_without_caret_motion_commits_or_wrapping() {
    use keytao_core::key_policy::{self, CandidatePanelLayout::*, *};
    let _serialized = serialized();
    let (runtime, dir) = deployed_runtime("panel-arrows");
    let session = runtime.create_session().expect("create a session");
    for (panel, previous, next, previous_page, next_page) in [
        (Vertical, XK_UP, XK_DOWN, XK_LEFT, XK_RIGHT),
        (Horizontal, XK_LEFT, XK_RIGHT, XK_UP, XK_DOWN),
    ] {
        for option in ["_horizontal", "_linear", "_vertical"] {
            session.set_option(option, true).unwrap();
        }
        type_code(&session);
        let press = |key| {
            let before = session.state();
            let mapped = key_policy::candidate_arrow_key(key, 0, &before, panel);
            let result = session.process_key_result(mapped, 0).unwrap();
            assert!(result.accepted);
            assert_eq!(result.state.preedit, before.preedit);
            assert_eq!(result.state.cursor, before.cursor);
            assert!(result.state.committed.is_none());
            result.state
        };
        assert_eq!(press(previous).highlighted_candidate_index, 0);
        assert_eq!(press(previous_page).page, 0);
        assert_eq!(press(next).highlighted_candidate_index, 1);
        assert_eq!(press(previous).highlighted_candidate_index, 0);
        assert_eq!(press(next_page).page, 1);
        assert_eq!(press(next_page).page, 1);
        assert_eq!(press(previous_page).page, 0);
        session
            .highlight_candidate_on_page(smoke_fixture::PAGE_SIZE - 1)
            .unwrap();
        let last = press(next);
        assert_eq!(last.page, 0);
        assert_eq!(
            last.highlighted_candidate_index,
            smoke_fixture::PAGE_SIZE - 1
        );
        press(next_page);
        let last_index = session.state().candidates.len() - 1;
        session.highlight_candidate_on_page(last_index).unwrap();
        assert_eq!(press(next).highlighted_candidate_index, last_index);

        type_code(&session);
        let highlighted = press(next);
        let committed = session.process_key_result(XK_SPACE, 0).unwrap();
        assert_eq!(
            committed.state.committed.as_deref(),
            Some(highlighted.candidates[1].text.as_str())
        );
        for key in [previous, next, previous_page, next_page] {
            let empty = session.state();
            assert_eq!(key_policy::candidate_arrow_key(key, 0, &empty, panel), key);
            assert!(!session.process_key_result(key, 0).unwrap().accepted);
        }
    }
    for option in ["_horizontal", "_linear", "_vertical"] {
        session.set_option(option, false).unwrap();
    }
    type_code(&session);
    let before = session.state();
    let key = key_policy::candidate_arrow_key(XK_LEFT, 0, &before, Hidden);
    let result = session.process_key_result(key, 0).unwrap();
    assert!(result.state.cursor < before.cursor);
    drop(session);
    drop(runtime);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn configured_switch_chords_are_advertised_and_noop_shift_preserves_mode() {
    use keytao_core::{RIME_MOD_ALT, RIME_MOD_CONTROL, RIME_MOD_SHIFT, RIME_RELEASE_MASK};

    let _serialized = serialized();
    let dir = smoke_fixture::scratch_dir("schema-switch-chords");
    smoke_fixture::write(&dir).unwrap();
    let schema_path = dir.join(format!("{}.schema.yaml", smoke_fixture::SCHEMA_ID));
    let schema = std::fs::read_to_string(&schema_path).unwrap();
    let schema = schema
        .replace("- ascii_composer\n", "- ascii_composer\n    - key_binder\n")
        .replace("Shift_L: noop", "Shift_L: commit_code")
        .replace("engine:\n", "  - name: equal_commit\n    reset: 0\n    states: [等号直出, 等号上屏]\nengine:\n");
    let bindings = "key_binder:
  bindings:
    - { when: always, accept: Control+period, toggle: ascii_mode }
    - { when: always, accept: Control+space, toggle: ascii_mode }
    - { when: always, accept: Shift+space, toggle: ascii_mode }
    - { when: always, accept: Control+Alt+F8, toggle: ascii_mode }
    - { when: always, accept: Control+equal, toggle: equal_commit }
    - { when: always, accept: Control+minus, set_option: equal_commit }
    - { when: composing, accept: Control+v, send: Page_Down }
    - { when: paging, accept: Control+j, toggle: ascii_mode }
    - { when: predicting, accept: Control+k, toggle: ascii_mode }
";
    std::fs::write(schema_path, format!("{schema}\n{bindings}")).unwrap();
    std::fs::write(
        dir.join(format!("{}.custom.yaml", smoke_fixture::SCHEMA_ID)),
        "patch:\n  ascii_composer/switch_key/Shift_L: noop\n",
    )
    .unwrap();
    let defaults = dir.join("default.custom.yaml");
    let config = std::fs::read_to_string(&defaults).unwrap();
    std::fs::write(
        defaults,
        format!("{config}  switcher/hotkeys: [Control+Shift+F8]\n"),
    )
    .unwrap();
    let runtime = ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned()).with_ime_host();
    runtime.init().expect("deploy the switch shortcut fixture");
    let session = runtime.create_session().unwrap();

    let state = session.state();
    assert_eq!(state.schema_shortcuts.len(), 7);
    let composing = type_code(&session);
    assert!(!composing.candidates.is_empty());
    assert!(!composing.schema_shortcuts.iter().any(|shortcut| {
        shortcut.modifiers == RIME_MOD_CONTROL
            && [b'j', b'k'].into_iter().any(|key| shortcut.key == u32::from(key))
    }));
    session.clear_composition();
    for (keysym, mask) in [
        (b'.' as u32, RIME_MOD_CONTROL),
        (0x20, RIME_MOD_CONTROL),
        (0x20, RIME_MOD_SHIFT),
        (0xffc5, RIME_MOD_CONTROL | RIME_MOD_ALT),
    ] {
        assert!(!keytao_core::key_policy::should_bypass_empty_composition(keysym, mask, &state));
        assert!(state.schema_shortcuts.iter().any(|shortcut| {
            (shortcut.key, shortcut.modifiers) == (keysym, mask) && shortcut.applies(&state)
        }));
        for before in [false, true] {
            session.set_ascii_mode(before).unwrap();
            let toggled = session.process_key_result(keysym, mask).unwrap();
            assert!(toggled.accepted);
            assert_eq!(toggled.state.ascii_mode, !before);
            assert!(toggled.state.switch_hint.is_none());
            for shift in [0xffe1, 0xffe2] {
                let released = session
                    .process_keys_result(&[(shift, 0), (shift, RIME_RELEASE_MASK)])
                    .unwrap();
                assert!(!released.accepted);
                assert_eq!(released.state.ascii_mode, !before);
            }
        }
    }
    session.set_ascii_mode(false).unwrap();
    let toggle = || session.process_key_result(b'='.into(), RIME_MOD_CONTROL).unwrap();
    let on = toggle();
    assert!(on.accepted);
    assert_eq!(on.state.switch_hint.as_deref(), Some("等号上屏"));
    assert!(on.state.committed.is_none() && on.state.candidates.is_empty());
    assert!(session.state().switch_hint.is_none());
    let noop = session.process_key_result(b'-'.into(), RIME_MOD_CONTROL).unwrap();
    assert!(noop.accepted && noop.state.switch_hint.is_none());
    let off = session
        .process_keys_result(&[
            (b'='.into(), RIME_MOD_CONTROL),
            (b'='.into(), RIME_MOD_CONTROL | RIME_RELEASE_MASK),
        ])
        .unwrap();
    assert!(off.accepted);
    assert_eq!(off.state.switch_hint.as_deref(), Some("等号直出"));
    let hotkey = (0xffc5, RIME_MOD_CONTROL | RIME_MOD_SHIFT);
    assert!(state.schema_shortcuts.iter().any(|shortcut| {
        (shortcut.key, shortcut.modifiers) == hotkey && shortcut.applies(&state)
    }));
    assert!(session.process_key_result(hotkey.0, hotkey.1).unwrap().accepted);
    drop(session);
    cleanup(&dir);
}

#[test]
fn easy_english_keeps_both_shift_keys_disabled_by_the_chinese_schema() {
    use keytao_core::english_mode::{write_english_mode, EnglishMode};
    use keytao_core::RIME_RELEASE_MASK;

    let _serialized = serialized();
    let dir = smoke_fixture::scratch_dir("easy-english-shift-noop");
    smoke_fixture::write(&dir).unwrap();
    let chinese =
        std::fs::read_to_string(dir.join(format!("{}.schema.yaml", smoke_fixture::SCHEMA_ID)))
            .unwrap();
    std::fs::write(
        dir.join("easy_en.schema.yaml"),
        chinese
            .replace(&format!("schema_id: {}", smoke_fixture::SCHEMA_ID), "schema_id: easy_en")
            .replace("name: KeyTao Smoke", "name: Easy English")
            .replace("Shift_L: noop", "Shift_L: commit_code")
            .replace("Shift_R: noop", "Shift_R: commit_code"),
    )
    .unwrap();
    let defaults = dir.join("default.custom.yaml");
    let content = std::fs::read_to_string(&defaults).unwrap();
    std::fs::write(defaults, format!("{content}    - schema: easy_en\n")).unwrap();
    write_english_mode(&dir, EnglishMode::Schema).unwrap();
    let runtime = ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned()).with_ime_host();
    runtime.init().unwrap();
    let session = runtime.create_session().unwrap();
    assert_eq!(session.set_english_mode(true).unwrap().schema_id, "easy_en");
    for shift in [0xffe1, 0xffe2] {
        let released = session
            .process_keys_result(&[(shift, 0), (shift, RIME_RELEASE_MASK)])
            .unwrap();
        assert_eq!(released.state.schema_id, "easy_en");
        assert!(!released.accepted);
    }
    assert_eq!(
        session.set_english_mode(false).unwrap().schema_id,
        smoke_fixture::SCHEMA_ID
    );
    drop(session);
    cleanup(&dir);
}

#[test]
fn a_shift_ascii_toggle_clears_the_host_context_override() {
    let _serialized = serialized();
    let dir = smoke_fixture::scratch_dir("context-ascii-toggle");
    smoke_fixture::write(&dir).unwrap();
    let schema_path = dir.join(format!("{}.schema.yaml", smoke_fixture::SCHEMA_ID));
    let schema = std::fs::read_to_string(&schema_path).unwrap();
    std::fs::write(
        schema_path,
        schema.replace("Shift_L: noop", "Shift_L: commit_code"),
    )
    .unwrap();
    let runtime = ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned()).with_ime_host();
    runtime.init().unwrap();
    let session = runtime.create_session().unwrap();
    assert!(
        session
            .set_context_ascii_mode(Some(true))
            .unwrap()
            .ascii_mode
    );
    session.process_key_result(0xffe1, 0).unwrap();
    let toggled = session
        .process_key_result(0xffe1, keytao_core::RIME_RELEASE_MASK)
        .unwrap();
    assert!(!toggled.state.ascii_mode);
    let next = session.process_key_result(b'a' as u32, 0).unwrap();
    assert!(!next.state.ascii_mode);
    assert!(next.accepted);
    assert_eq!(next.state.preedit, "a");
    drop(session);
    cleanup(&dir);
}

#[test]
fn a_shift_mode_switch_can_commit_text_even_when_rime_rejects_the_release() {
    let _serialized = serialized();
    let dir = smoke_fixture::scratch_dir("shift-release-commit");
    smoke_fixture::write(&dir).expect("write the fixture");
    let schema_path = dir.join(format!("{}.schema.yaml", smoke_fixture::SCHEMA_ID));
    let schema = std::fs::read_to_string(&schema_path).unwrap();
    assert!(schema.contains("Shift_L: noop"));
    std::fs::write(
        &schema_path,
        schema.replace("Shift_L: noop", "Shift_L: commit_code"),
    )
    .unwrap();
    let runtime = ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned());
    runtime.init().expect("deploy the fixture");
    let session = runtime.create_session().expect("create a session");
    assert!(!type_code(&session).preedit.is_empty());
    session.process_key_result(0xffe1, 0).unwrap();
    let result = session
        .process_key_result(0xffe1, keytao_core::RIME_RELEASE_MASK)
        .unwrap();
    assert!(
        !result.accepted,
        "the client must still receive the key release"
    );
    assert_eq!(result.state.committed.as_deref(), Some(smoke_fixture::CODE));
    assert!(result.state.preedit.is_empty());
    assert!(result.state.candidates.is_empty());
    assert!(result.state.ascii_mode);

    // The editor can gain focus while Ctrl/Alt/Super is already held, so Rime
    // need not have seen that modifier's press. A Shift event must retain the
    // full chord, or the release would wrongly commit and switch languages.
    for modifier in [
        keytao_core::RIME_MOD_CONTROL,
        keytao_core::RIME_MOD_ALT,
        keytao_core::RIME_MOD_SUPER,
    ] {
        session.set_ascii_mode(false).unwrap();
        type_code(&session);
        session
            .process_key_result(0xffe1, modifier | keytao_core::RIME_MOD_SHIFT)
            .unwrap();
        let result = session
            .process_key_result(0xffe1, modifier | keytao_core::RIME_RELEASE_MASK)
            .unwrap();
        assert!(!result.state.ascii_mode);
        assert!(result.state.committed.is_none());
        assert_eq!(result.state.preedit, smoke_fixture::CODE);
    }
    drop(session);
    cleanup(&dir);
}

#[test]
fn explicit_uppercase_text_does_not_start_a_lowercase_schema_code() {
    let _serialized = serialized();
    let (runtime, dir) = deployed_runtime("uppercase-text-keys");
    let session = runtime.create_session().expect("create a session");
    for text in ["A", "Z"] {
        session.reset().unwrap();
        let keysym = keytao_core::key_policy::keysym_for_text(text).unwrap();
        let result = session.process_key_result(keysym, 0).unwrap();
        assert!(!result.accepted, "uppercase text belongs to the host");
        assert!(result.state.preedit.is_empty());
        assert!(result.state.candidates.is_empty());
        assert!(result.state.committed.is_none());
    }
    let result = session.process_key_result(b'a' as u32, 0).unwrap();
    assert!(result.accepted);
    assert!(!result.state.preedit.is_empty());
    drop(session);
    cleanup(&dir);
}

fn page_words(state: &ImeState) -> Vec<&str> {
    state
        .candidates
        .iter()
        .map(|candidate| candidate.text.as_str())
        .collect()
}

#[test]
fn commit_preview_follows_highlight_and_converted_segments() {
    let _serialized = serialized();
    let dir = smoke_fixture::scratch_dir("commit-preview");
    smoke_fixture::write(&dir).unwrap();
    let schema_path = dir.join(format!("{}.schema.yaml", smoke_fixture::SCHEMA_ID));
    let schema = std::fs::read_to_string(&schema_path).unwrap();
    std::fs::write(
        schema_path,
        schema.replace("- express_editor", "- fluency_editor"),
    )
    .unwrap();
    let runtime = ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned());
    runtime.init().expect("deploy the fixture");
    let session = runtime.create_session().expect("create a session");

    let first = type_code(&session);
    assert_eq!(first.commit_preview, smoke_fixture::WORDS[0]);
    let highlighted = session.highlight_candidate_on_page(1).unwrap();
    assert_eq!(highlighted.commit_preview, smoke_fixture::WORDS[1]);

    for character in smoke_fixture::CODE.chars() {
        session.process_key_result(character as u32, 0).unwrap();
    }
    assert_eq!(session.state().preedit, smoke_fixture::CODE.repeat(2));
    // Select the first code at the caret, leaving the second code unconverted.
    for _ in smoke_fixture::CODE.chars() {
        session
            .process_key_result(keytao_core::key_policy::XK_LEFT, 0)
            .unwrap();
    }
    let converted = session.select_candidate_on_page(1).unwrap();
    assert!(converted.committed.is_none());
    assert_eq!(
        converted.preedit,
        format!("{}{}", smoke_fixture::WORDS[1], smoke_fixture::CODE)
    );
    assert_eq!(converted.highlighted_candidate_index, 0);
    assert_eq!(
        converted.commit_preview,
        format!(
            "{}{}",
            smoke_fixture::WORDS[1],
            converted.candidates[0].text
        )
    );
    let highlighted = session.highlight_candidate_on_page(2).unwrap();
    assert!(highlighted.committed.is_none());
    assert_eq!(highlighted.preedit, converted.preedit);
    assert_eq!(highlighted.highlighted_candidate_index, 2);
    assert_eq!(
        highlighted.commit_preview,
        format!(
            "{}{}",
            smoke_fixture::WORDS[1],
            highlighted.candidates[2].text
        )
    );
    assert_ne!(highlighted.commit_preview, converted.commit_preview);
    let committed = session.commit_composition().unwrap();
    assert_eq!(
        committed.committed.as_deref(),
        Some(highlighted.commit_preview.as_str())
    );

    session.reset().unwrap();
    let no_candidate = session.process_key_result(b'z' as u32, 0).unwrap().state;
    assert_eq!(no_candidate.preedit, "z");
    assert!(no_candidate.candidates.is_empty());
    assert!(no_candidate.commit_preview.is_empty());
    drop(session);
    cleanup(&dir);
}

#[test]
fn schema_switches_follow_the_active_schema() {
    let _serialized = serialized();
    let (runtime, dir) = deployed_runtime("schema-switches");
    let session = runtime.create_session().expect("create a session");

    assert_eq!(
        session.schema_switches().expect("session has no engine"),
        vec![
            SchemaSwitch {
                name: Some("ascii_mode".into()),
                options: vec![],
                states: vec!["中文".into(), "西文".into()],
                reset: Some(0),
            },
            SchemaSwitch {
                name: None,
                options: vec!["punctuation_ascii".into(), "punctuation_cjk".into()],
                states: vec!["西文标点".into(), "中文标点".into()],
                reset: Some(1),
            },
        ]
    );

    drop(session);
    cleanup(&dir);
}

#[test]
fn candidate_deletion_is_limited_to_user_dictionary_entries() {
    const USER_PHRASE: &str = smoke_fixture::WORDS[0];

    let _serialized = serialized();
    let (runtime, dir) = deployed_runtime_with_user_dictionary("candidate-user-source");
    let session = runtime.create_session().expect("create a session");

    let initial = type_code(&session);
    let initial_index = initial
        .candidates
        .iter()
        .position(|candidate| candidate.text == USER_PHRASE)
        .expect("built-in phrase is not on the first page");
    assert_eq!(
        session.candidate_is_user_phrase_on_page(initial_index),
        Some(false)
    );
    let committed = session
        .select_candidate_on_page(initial_index)
        .expect("select_candidate_on_page");
    assert_eq!(committed.committed.as_deref(), Some(USER_PHRASE));

    // A newly created Engine must recover learned-word provenance from the
    // persisted user dictionary, not only from commits seen by this session.
    drop(session);
    runtime
        .reload_without_deploy()
        .expect("reinitialize librime for a cold-restart check");
    let session = runtime.create_session().expect("recreate a session");

    let state = type_code(&session);
    let index = state
        .candidates
        .iter()
        .position(|candidate| candidate.text == USER_PHRASE)
        .expect("learned phrase is not on the first page");
    assert_eq!(session.candidate_is_user_phrase_on_page(index), Some(true));

    let (_, deleted) = session
        .delete_candidate_on_page_result(index)
        .expect("delete_candidate_on_page_result");
    assert!(deleted);
    let after = type_code(&session);
    let system_index = after
        .candidates
        .iter()
        .position(|candidate| candidate.text == USER_PHRASE)
        .expect("the built-in dictionary entry should remain after forgetting its user record");
    assert_eq!(
        session.candidate_is_user_phrase_on_page(system_index),
        Some(false)
    );

    drop(session);
    cleanup(&dir);
}

#[test]
fn selection_and_paging_go_through_librime() {
    let _serialized = serialized();
    let (runtime, dir) = deployed_runtime("candidate-api");

    let capabilities = runtime.capabilities();
    assert!(
        capabilities.supports_candidate_selection(),
        "the linked librime cannot select candidates: {capabilities:?}"
    );
    assert!(
        capabilities.supports_native_paging(),
        "the linked librime cannot turn pages: {capabilities:?}"
    );
    assert!(
        capabilities.supports_candidate_highlight(),
        "the linked librime cannot move the highlight: {capabilities:?}"
    );
    assert!(
        capabilities.supports_candidate_deletion(),
        "the linked librime cannot classify candidate deletion: {capabilities:?}"
    );

    let session = runtime.create_session().expect("create a session");
    assert!(session.supports_candidate_selection());
    assert!(session.supports_native_paging());

    let first = type_code(&session);
    assert_eq!(first.preedit, smoke_fixture::CODE);
    assert_eq!(
        first.select_keys.as_deref().unwrap_or_default(),
        "",
        "the fixture must not publish select keys"
    );
    assert_eq!(first.page, 0);
    assert!(
        !first.is_last_page,
        "the fixture must produce several pages"
    );
    assert_eq!(
        page_words(&first),
        &smoke_fixture::WORDS[..smoke_fixture::PAGE_SIZE]
    );
    assert!(
        first.candidates.len() > DEFAULT_SELECT_KEYS.chars().count(),
        "a page must outgrow the select-key fallback"
    );

    // A synthesized `=` would land in the speller instead of turning the page.
    let second = session.change_page(false).expect("change_page(forward)");
    assert_eq!(second.page, 1);
    assert_eq!(second.preedit, first.preedit);
    assert!(second.is_last_page);
    assert_eq!(
        page_words(&second),
        &smoke_fixture::WORDS[smoke_fixture::PAGE_SIZE..]
    );

    let back = session.change_page(true).expect("change_page(backward)");
    assert_eq!(back.page, 0);
    assert_eq!(back.preedit, first.preedit);
    assert_eq!(page_words(&back), page_words(&first));

    // The highlight moves without committing and without editing the code.
    let last = smoke_fixture::PAGE_SIZE - 1;
    let highlighted = session
        .highlight_candidate_on_page(last)
        .expect("highlight_candidate_on_page");
    assert_eq!(highlighted.highlighted_candidate_index, last);
    assert_eq!(highlighted.committed, None);
    assert_eq!(highlighted.preedit, first.preedit);

    // The last candidate of the page has no select key, so a fallback stroke
    // could not reach it at all.
    let selected = session
        .select_candidate_on_page(last)
        .expect("select_candidate_on_page");
    assert_eq!(
        selected.committed.as_deref(),
        Some(smoke_fixture::WORDS[last])
    );
    assert!(selected.preedit.is_empty());

    // The first candidate is the one a synthesized `1` would have aimed at; the
    // digit is part of the alphabet, so a fallback would extend the code.
    type_code(&session);
    let head = session
        .select_candidate_on_page(0)
        .expect("select_candidate_on_page");
    assert_eq!(head.committed.as_deref(), Some(smoke_fixture::WORDS[0]));
    assert!(head.preedit.is_empty());

    // Built-in table entries are not learned phrases. The native deletion
    // result is the source-of-truth distinction used by the mobile menu.
    type_code(&session);
    let (unchanged, deleted) = session
        .delete_candidate_on_page_result(0)
        .expect("delete_candidate_on_page_result");
    assert!(!deleted);
    assert_eq!(page_words(&unchanged), page_words(&first));
    assert_eq!(unchanged.preedit, first.preedit);

    // Discarding and committing are the focus-change paths.
    type_code(&session);
    let cleared = session.clear_composition().expect("clear_composition");
    assert!(cleared.preedit.is_empty());
    assert!(cleared.candidates.is_empty());
    assert_eq!(cleared.committed, None);

    type_code(&session);
    let committed = session.commit_composition().expect("commit_composition");
    assert!(
        committed.committed.is_some(),
        "commit_composition produced no text"
    );
    assert!(committed.preedit.is_empty());

    type_code(&session);
    let raw = session.raw_input().expect("raw_input");
    assert_eq!(raw, smoke_fixture::CODE);
    let raw_commit = session.commit_raw_input().expect("commit_raw_input");
    assert_eq!(raw_commit.committed.as_deref(), Some(smoke_fixture::CODE));
    assert!(raw_commit.preedit.is_empty());

    drop(session);
    cleanup(&dir);
}

/// The policy check and the librime call it guards share one critical section,
/// so a context that has turned sensitive can never end up composing — not even
/// for the key another thread was already handling when it turned.
#[test]
fn a_sensitive_context_never_composes_under_concurrent_typing() {
    let _serialized = serialized();
    let (runtime, dir) = deployed_runtime("candidate-api-policy");
    let session = runtime.create_session().expect("create a session");

    let stop = AtomicBool::new(false);
    let failures: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let composed = AtomicU64::new(0);

    std::thread::scope(|scope| {
        let typist = &session;
        let typing_stop = &stop;
        scope.spawn(move || {
            while !typing_stop.load(Ordering::Relaxed) {
                let _ = typist.reset();
                for character in smoke_fixture::CODE.chars() {
                    let Some(result) = typist.process_key_result(character as u32, 0) else {
                        continue;
                    };
                    if !result.accepted && !result.state.preedit.is_empty() {
                        // A rejected key must not have composed anything.
                        break;
                    }
                }
            }
        });
        // Any exit from here on, a panic included, stops the typist; otherwise
        // the scope would wait for it forever.
        let _stop_typing = StopOnDrop(&stop);

        for _ in 0..300 {
            let switched = retry_busy("set_input_policy(sensitive)", || {
                session.set_input_policy(InputContextPolicy::sensitive())
            });
            if !switched.preedit.is_empty() || !switched.candidates.is_empty() {
                failures
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(format!(
                        "turning sensitive left preedit {:?}",
                        switched.preedit
                    ));
            }
            for _ in 0..16 {
                let state = engine_state(&session);
                if !state.preedit.is_empty() || !state.candidates.is_empty() {
                    failures
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(format!("sensitive context composed {:?}", state.preedit));
                    break;
                }
            }
            retry_busy("set_input_policy(default)", || {
                session.set_input_policy(InputContextPolicy::default())
            });
            for _ in 0..16 {
                if !session.state().preedit.is_empty() {
                    composed.fetch_add(1, Ordering::Relaxed);
                    break;
                }
            }
            std::thread::sleep(Duration::from_micros(200));
        }
    });

    let failures = failures.lock().unwrap_or_else(PoisonError::into_inner);
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(
        composed.load(Ordering::Relaxed) > 0,
        "the typing thread never composed, so the test proved nothing"
    );

    drop(session);
    cleanup(&dir);
}

struct StopOnDrop<'a>(&'a AtomicBool);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Session calls only try-lock the engine and return `None` while another
/// thread holds it; retry for a bounded time instead of failing on contention.
fn retry_busy<T>(what: &str, mut call: impl FnMut() -> Option<T>) -> T {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(value) = call() {
            return value;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{what} stayed busy for 5s"
        );
        std::thread::sleep(Duration::from_micros(20));
    }
}

/// `state()` reports an empty state, without a schema, when the engine is
/// busy; only a state that carries the schema was actually read from librime.
fn engine_state(session: &ImeRuntimeSession) -> ImeState {
    retry_busy("state", || {
        Some(session.state()).filter(|state| !state.schema_id.is_empty())
    })
}

fn cleanup(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}
