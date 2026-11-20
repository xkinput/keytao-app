//! Shift switching follows the configured librime action without host commits.
#[path = "support/smoke_fixture.rs"]
mod smoke_fixture;

use keytao_core::{ImeRuntime, RIME_RELEASE_MASK};

#[test]
fn shift_switch_key_styles_commit_once_and_unset_right_shift_is_noop() {
    for style in ["commit_text", "clear", "commit_code", "noop"] {
        let dir = smoke_fixture::scratch_dir("shift-switch");
        std::fs::create_dir_all(dir.join("lua")).unwrap();
        for (name, contents) in [
            (
                "keytao.schema.yaml",
                include_str!("fixtures/caps_lock/keytao.schema.yaml"),
            ),
            (
                "keytao.extended.dict.yaml",
                include_str!("fixtures/caps_lock/keytao.extended.dict.yaml"),
            ),
            (
                "lua/for_topup.lua",
                include_str!("fixtures/caps_lock/for_topup.lua"),
            ),
            ("rime.lua", "topup_processor = require('for_topup')\n"),
            (
                "default.yaml",
                "config_version: '1'\nschema_list:\n  - schema: keytao\n  - schema: easy_en\n",
            ),
        ] {
            std::fs::write(dir.join(name), contents).unwrap();
        }
        std::fs::write(
            dir.join("default.custom.yaml"),
            format!("patch:\n  schema_list:\n    - schema: keytao\n    - schema: easy_en\n  ascii_composer/good_old_caps_lock: false\n  ascii_composer/switch_key:\n    Caps_Lock: commit_text\n    Shift_L: {style}\n"),
        )
        .unwrap();
        std::fs::write(
            dir.join("easy_en.schema.yaml"),
            include_str!("fixtures/caps_lock/keytao.schema.yaml")
                .replace("schema_id: keytao", "schema_id: easy_en"),
        )
        .unwrap();
        let runtime =
            ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned()).with_ime_host();
        runtime.init().expect("deploy the Shift switch fixture");
        for keysym in [0xffe1, 0xffe2] {
            let session = runtime.create_session().unwrap();
            session.set_context_ascii_mode(Some(false)).unwrap();
            for key in b"fjv" {
                session.process_key_result(u32::from(*key), 0).unwrap();
            }
            assert_eq!(session.state().preedit, "fjv");
            assert_eq!(session.state().candidates[0].text, "甲");
            let press = session.process_key_result(keysym, 0).unwrap();
            let release = session
                .process_key_result(keysym, RIME_RELEASE_MASK)
                .unwrap();
            let commits: Vec<_> = [press.state.committed, release.state.committed.clone()]
                .into_iter()
                .flatten()
                .collect();
            let active = keysym == 0xffe1 && style != "noop";
            let expected = if active {
                match style {
                    "commit_text" => vec!["甲".to_owned()],
                    "commit_code" => vec!["fjv".to_owned()],
                    _ => Vec::new(),
                }
            } else {
                Vec::new()
            };
            assert_eq!(commits, expected, "{style}, keysym={keysym:#x}");
            assert!(commits.len() <= 1, "{style}, keysym={keysym:#x}");
            if matches!(style, "commit_text" | "clear") {
                assert!(commits.iter().all(|text| !text.contains("fjv")));
            }
            assert_eq!(
                release.state.ascii_mode, active,
                "{style}, keysym={keysym:#x}"
            );
            assert_eq!(release.state.preedit, if active { "" } else { "fjv" });
        }
        drop(runtime);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
