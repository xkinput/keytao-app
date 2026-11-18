//! Real keytao uppercase recognition and for_topup, with a tiny dictionary.
#[path = "support/smoke_fixture.rs"]
mod smoke_fixture;

use keytao_core::{
    key_policy, ImeRuntime, ImeRuntimeSession, RIME_MOD_LOCK, RIME_MOD_SHIFT, RIME_RELEASE_MASK,
};

fn assert_topup(session: &ImeRuntimeSession) {
    for key in b"fjv" {
        assert!(
            session
                .process_key_result(u32::from(*key), 0)
                .unwrap()
                .accepted
        );
    }
    let state = session.state();
    assert_eq!(state.preedit, "fjv");
    assert_eq!(state.candidates[0].text, "甲");
    let topped = session.process_key_result(u32::from(b'f'), 0).unwrap();
    assert_eq!(topped.state.committed.as_deref(), Some("甲"));
    assert_eq!(topped.state.preedit, "f");
    session.reset().unwrap();
}

#[test]
fn caps_lock_commits_pending_code_and_restores_topup_across_sessions() {
    for preference in ["ascii", "schema"] {
        let dir = smoke_fixture::scratch_dir("caps-lock");
        std::fs::create_dir_all(dir.join("lua")).unwrap();
        for (name, contents) in [
            ("keytao.schema.yaml", include_str!("fixtures/caps_lock/keytao.schema.yaml")),
            ("keytao.extended.dict.yaml", include_str!("fixtures/caps_lock/keytao.extended.dict.yaml")),
            ("lua/for_topup.lua", include_str!("fixtures/caps_lock/for_topup.lua")),
            ("rime.lua", "topup_processor = require('for_topup')\n"),
            ("default.yaml", "config_version: '1'\nschema_list:\n  - schema: keytao\n  - schema: easy_en\n"),
            ("default.custom.yaml", "patch:\n  schema_list:\n    - schema: keytao\n    - schema: easy_en\n  ascii_composer/good_old_caps_lock: true\n  ascii_composer/switch_key:\n    Caps_Lock: commit_text\n    Shift_L: commit_text\n"),
        ] {
            std::fs::write(dir.join(name), contents).unwrap();
        }
        std::fs::write(
            dir.join("easy_en.schema.yaml"),
            include_str!("fixtures/caps_lock/keytao.schema.yaml")
                .replace("schema_id: keytao", "schema_id: easy_en"),
        )
        .unwrap();
        std::fs::write(
            dir.join("keytao-ime.json"),
            format!(r#"{{"englishMode":"{preference}"}}"#),
        )
        .unwrap();
        let runtime =
            ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned()).with_ime_host();
        runtime
            .init()
            .expect("deploy the real keytao Caps Lock fixture");
        let session = runtime.create_session().unwrap();
        let other_app = runtime.create_session().unwrap();
        // Rime deliberately preserves ASCII selected manually before Caps Lock.
        session.set_ascii_mode(true).unwrap();
        for mask in [0, RIME_MOD_LOCK] {
            assert!(
                session
                    .process_key_result(0xffe5, mask)
                    .unwrap()
                    .state
                    .ascii_mode
            );
            assert!(other_app.state().ascii_mode);
        }
        session.set_ascii_mode(false).unwrap();
        assert_topup(&session);

        // An already enabled Caps Lock may arrive without its original toggle.
        for key in b"abc" {
            let result = session
                .process_key_result(u32::from(*key), RIME_MOD_LOCK)
                .unwrap();
            assert!(!result.accepted);
            assert!(result.state.preedit.is_empty());
            assert!(result.state.candidates.is_empty());
        }
        assert_topup(&session);

        for key in b"fjv" {
            session.process_key_result(u32::from(*key), 0).unwrap();
        }
        let arrow = key_policy::candidate_arrow_key(
            key_policy::XK_DOWN,
            RIME_MOD_LOCK,
            &session.state(),
            key_policy::CandidatePanelLayout::Vertical,
        );
        let selected = session.process_key_result(arrow, RIME_MOD_LOCK).unwrap();
        assert!(selected.accepted);
        assert_eq!(selected.state.highlighted_candidate_index, 1);
        session.reset().unwrap();

        // macOS/Windows synthesize this pre-toggle mask; IBus/Wayland provide it.
        for release_events in [false, true] {
            session.set_context_ascii_mode(Some(false)).unwrap();
            for key in b"fjv" {
                session.process_key_result(u32::from(*key), 0).unwrap();
            }
            let on = session.process_key_result(0xffe5, 0).unwrap();
            assert_eq!(on.state.committed.as_deref(), Some("甲"));
            assert!(on.state.ascii_mode);
            assert_eq!(on.state.schema_id, "keytao");
            assert!(on.state.preedit.is_empty());
            assert!(!other_app.state().ascii_mode);
            assert_eq!(other_app.state().schema_id, "keytao");
            if release_events {
                let release = session
                    .process_key_result(0xffe5, RIME_MOD_LOCK | RIME_RELEASE_MASK)
                    .unwrap();
                assert!(release.state.ascii_mode);
            }
            // macOS supplies lowercase keysyms; Windows/Linux supply uppercase.
            for key in b"abcABC" {
                let result = session
                    .process_key_result(u32::from(*key), RIME_MOD_LOCK)
                    .unwrap();
                assert!(!result.accepted);
                assert!(result.state.ascii_mode);
                assert_eq!(result.state.schema_id, "keytao");
                assert!(result.state.preedit.is_empty());
                assert!(result.state.candidates.is_empty());
                assert!(result.state.committed.is_none());
            }
            let shifted = session
                .process_key_result(u32::from(b'A'), RIME_MOD_LOCK | RIME_MOD_SHIFT)
                .unwrap();
            assert!(!shifted.accepted);
            assert!(shifted.state.preedit.is_empty());
            assert!(!other_app.state().ascii_mode);
            // Caps Lock is system-wide, including when another app turns it off.
            other_app.set_context_ascii_mode(Some(true)).unwrap();
            let off = other_app.process_key_result(0xffe5, RIME_MOD_LOCK).unwrap();
            assert!(!off.state.ascii_mode);
            assert_eq!(off.state.schema_id, "keytao");
            if release_events {
                assert!(
                    !other_app
                        .process_key_result(0xffe5, RIME_RELEASE_MASK)
                        .unwrap()
                        .state
                        .ascii_mode
                );
            }
            assert!(!other_app.state().ascii_mode);
            assert!(!session.state().ascii_mode);
            assert_topup(&session);
        }
        drop((session, other_app, runtime));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
