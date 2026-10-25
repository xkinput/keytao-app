//! Regression coverage for characters lost after TSF claims a key that Rime declines.

use keytao_core::{
    key_policy, Candidate, ImeRuntime, ImeState, KeyProcessResult, RIME_MOD_ALT, RIME_MOD_CONTROL,
    RIME_MOD_HYPER, RIME_MOD_LOCK, RIME_MOD_META, RIME_MOD_SHIFT, RIME_MOD_SUPER,
    RIME_RELEASE_MASK,
};
use windows::Win32::UI::Input::KeyboardAndMouse::VK_NUMPAD0;

use super::{complete_unhandled_key, has_visible_state, should_consume_processed_state};
use crate::key_map::vk_to_keysym;

#[path = "../../keytao-core/tests/support/smoke_fixture.rs"]
mod smoke_fixture;

fn declined(state: ImeState) -> KeyProcessResult {
    KeyProcessResult {
        accepted: false,
        state,
    }
}

fn preedit() -> ImeState {
    ImeState {
        preedit: "aa".into(),
        ..ImeState::empty()
    }
}

fn candidates() -> ImeState {
    ImeState {
        candidates: vec![Candidate {
            text: "甲".into(),
            comment: None,
        }],
        ..ImeState::empty()
    }
}

#[test]
fn declined_digits_from_number_row_and_numpad_are_committed_once() {
    let before = ImeState::empty();
    for digit in 0..=9 {
        let expected = char::from(b'0' + digit as u8);
        // The number row's layout conversion yields the character keysym;
        // keypad VKs have their own conversion and must reach the same path.
        for keysym in [
            expected as u32,
            vk_to_keysym(VK_NUMPAD0.0 + digit, 0, 0).unwrap(),
        ] {
            assert_eq!(key_policy::char_for_keysym(keysym), Some(expected));
            for mods in [0, RIME_MOD_LOCK] {
                let mut result = declined(ImeState::empty());
                complete_unhandled_key(keysym, mods, &mut result);
                assert_eq!(result.state.committed, Some(expected.to_string()));
                assert!(!result.accepted, "Rime's result must retain its meaning");
                assert!(should_consume_processed_state(
                    result.accepted,
                    &before,
                    &result.state
                ));
            }
        }
    }

    // Cached commits describe the preceding key. They must not prevent the
    // next declined digit from receiving its own commit or replay the old one.
    let mut before = ImeState::empty();
    let mut output = String::new();
    for digit in "01234567890123456789".chars() {
        let mut result = declined(ImeState::empty());
        complete_unhandled_key(digit as u32, 0, &mut result);
        output.push_str(result.state.committed.as_deref().unwrap());
        assert!(should_consume_processed_state(
            result.accepted,
            &before,
            &result.state
        ));
        before = result.state;
    }
    assert_eq!(output, "01234567890123456789");
}

#[test]
fn declined_printable_keys_preserve_shift_and_unicode_characters() {
    for (character, mods) in [
        ('!', RIME_MOD_SHIFT),
        ('A', RIME_MOD_LOCK),
        ('a', RIME_MOD_LOCK | RIME_MOD_SHIFT),
        ('é', 0),
        ('键', 0),
    ] {
        let mut result = declined(ImeState::empty());
        complete_unhandled_key(
            key_policy::keysym_for_char(character).unwrap(),
            mods,
            &mut result,
        );
        assert_eq!(result.state.committed, Some(character.to_string()));
    }
}

#[test]
fn schema_handled_digits_and_full_width_commits_are_preserved() {
    for mut result in [
        KeyProcessResult {
            accepted: true,
            state: ImeState::empty(),
        },
        KeyProcessResult {
            accepted: true,
            state: preedit(),
        },
        KeyProcessResult {
            accepted: true,
            state: ImeState {
                committed: Some("甲".into()),
                ..ImeState::empty()
            },
        },
        KeyProcessResult {
            accepted: true,
            state: ImeState {
                committed: Some("１".into()),
                ..ImeState::empty()
            },
        },
    ] {
        let accepted = result.accepted;
        let committed = result.state.committed.clone();
        let preedit = result.state.preedit.clone();
        complete_unhandled_key('1' as u32, 0, &mut result);
        assert_eq!(result.accepted, accepted);
        assert_eq!(result.state.committed, committed);
        assert_eq!(result.state.preedit, preedit);
    }
}

#[test]
fn ongoing_compositions_do_not_gain_a_raw_character() {
    for after in [preedit(), candidates()] {
        let mut result = declined(after);
        complete_unhandled_key('1' as u32, 0, &mut result);
        assert_eq!(result.state.committed, None);
    }
}

#[test]
fn declined_printable_key_follows_the_previous_composition_commit() {
    let mut result = declined(ImeState {
        committed: Some("甲".into()),
        ..ImeState::empty()
    });
    complete_unhandled_key('!' as u32, RIME_MOD_SHIFT, &mut result);
    assert_eq!(result.state.committed.as_deref(), Some("甲!"));
    assert!(should_consume_processed_state(
        false,
        &preedit(),
        &result.state
    ));
}

#[test]
fn shortcuts_releases_and_non_printable_keys_are_never_synthesized() {
    for modifier in [
        RIME_MOD_CONTROL,
        RIME_MOD_ALT,
        RIME_MOD_SUPER,
        RIME_MOD_HYPER,
        RIME_MOD_META,
        RIME_RELEASE_MASK,
    ] {
        for mods in [modifier, modifier | RIME_MOD_SHIFT | RIME_MOD_LOCK] {
            let mut result = declined(ImeState::empty());
            complete_unhandled_key('1' as u32, mods, &mut result);
            assert_eq!(result.state.committed, None, "modifier {mods:#x}");
        }
    }
    for keysym in [
        0,
        '\t' as u32,
        '\r' as u32,
        0x7f,
        key_policy::XK_RETURN,
        key_policy::XK_BACK_SPACE,
        key_policy::XK_LEFT,
        key_policy::XK_F4,
        0x0100_d800,
        0x0111_0000,
    ] {
        let mut result = declined(ImeState::empty());
        complete_unhandled_key(keysym, 0, &mut result);
        assert_eq!(result.state.committed, None, "keysym {keysym:#x}");
    }
}

#[test]
fn real_rime_declines_idle_digits_but_keeps_digits_inside_a_schema_code() {
    const CHILD_DIR: &str = "KEYTAO_TEST_UNHANDLED_KEY_CHILD_DIR";
    let dir = match std::env::var_os(CHILD_DIR) {
        Some(dir) => std::path::PathBuf::from(dir),
        None => {
            // Keep librime's global initialization and DLL mapping out of the
            // parent process: COM-only tests assert that no DLL was loaded.
            // Waiting for child exit also releases its mapped fixture files.
            let dir = smoke_fixture::scratch_dir("windows-unhandled-key");
            smoke_fixture::write(&dir).expect("write the deterministic Rime fixture");
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "key_event_sink::unhandled_key_tests::real_rime_declines_idle_digits_but_keeps_digits_inside_a_schema_code",
                    "--nocapture",
                ])
                .env(CHILD_DIR, &dir)
                .output()
                .expect("run isolated Rime regression");
            let cleanup = std::fs::remove_dir_all(dir);
            assert!(
                output.status.success(),
                "Rime regression failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            cleanup.expect("remove only this test's fixture after child exit");
            return;
        }
    };
    let runtime = ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned());
    runtime.init().expect("deploy the fixture");
    let session = runtime.create_session().expect("create a Rime session");

    for digit in '0'..='9' {
        session.reset().unwrap();
        let before = session.state();
        assert!(!has_visible_state(&before));
        let mut result = session.process_key_result(digit as u32, 0).unwrap();
        assert!(
            !result.accepted,
            "fixture initials exclude idle digit {digit}"
        );
        assert!(!has_visible_state(&result.state));
        assert_eq!(result.state.committed, None);
        complete_unhandled_key(digit as u32, 0, &mut result);
        assert_eq!(result.state.committed, Some(digit.to_string()));
    }

    session.reset().unwrap();
    session.process_key_result('a' as u32, 0).unwrap();
    let before = session.state();
    assert!(has_visible_state(&before));
    let mut result = session.process_key_result('1' as u32, 0).unwrap();
    assert!(
        result.accepted,
        "the fixture allows digits after an initial letter"
    );
    let original_preedit = result.state.preedit.clone();
    assert!(original_preedit.contains('1'));
    complete_unhandled_key('1' as u32, 0, &mut result);
    assert_eq!(result.state.preedit, original_preedit);
    assert_eq!(result.state.committed, None);

    for (character, mods) in [('!', RIME_MOD_SHIFT), ('A', 0)] {
        session.reset().unwrap();
        for ch in smoke_fixture::CODE.chars() {
            session.process_key_result(ch as u32, 0).unwrap();
        }
        assert!(!session.state().candidates.is_empty());
        let mut result = session.process_key_result(character as u32, mods).unwrap();
        assert!(
            !result.accepted,
            "ExpressEditor leaves this character to the host"
        );
        assert!(!has_visible_state(&result.state));
        assert_eq!(
            result.state.committed.as_deref(),
            Some(smoke_fixture::WORDS[0])
        );
        complete_unhandled_key(character as u32, mods, &mut result);
        assert_eq!(
            result.state.committed,
            Some(format!("{}{character}", smoke_fixture::WORDS[0]))
        );
    }

    drop(session);
    drop(runtime);
}
