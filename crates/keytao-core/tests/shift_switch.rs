//! Solo modifiers follow the configured librime action without host commits.
#[path = "support/smoke_fixture.rs"]
mod smoke_fixture;

use keytao_core::{ImeRuntime, RIME_RELEASE_MASK};

#[test]
fn modifier_switch_key_styles_commit_once_and_unset_shift_is_noop() {
    for (switch_key, switch_keysym, style) in [
        ("Shift_L", 0xffe1, "commit_text"),
        ("Shift_L", 0xffe1, "clear"),
        ("Shift_L", 0xffe1, "commit_code"),
        ("Shift_L", 0xffe1, "noop"),
        ("Control_L", 0xffe3, "commit_text"),
        ("Alt_R", 0xffea, "commit_code"),
    ] {
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
            format!("patch:\n  schema_list:\n    - schema: keytao\n    - schema: easy_en\n  ascii_composer/good_old_caps_lock: false\n  ascii_composer/switch_key:\n    Caps_Lock: commit_text\n    {switch_key}: {style}\n"),
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
        runtime.init().expect("deploy the modifier switch fixture");
        for keysym in [switch_keysym, 0xffe2] {
            let session = runtime.create_session().unwrap();
            session.set_context_ascii_mode(Some(false)).unwrap();
            for key in b"fjv" {
                session.process_key_result(u32::from(*key), 0).unwrap();
            }
            assert_eq!(session.state().preedit, "fjv");
            assert_eq!(session.state().candidates[0].text, "甲");
            let result = session
                .process_keys_result(&[(keysym, 0), (keysym, RIME_RELEASE_MASK)])
                .unwrap();
            let committed = result.state.committed.as_deref();
            let active = keysym == switch_keysym && style != "noop";
            let expected = if active {
                match style {
                    "commit_text" => Some("甲"),
                    "commit_code" => Some("fjv"),
                    _ => None,
                }
            } else {
                None
            };
            assert_eq!(committed, expected, "{style}, keysym={keysym:#x}");
            assert_eq!(
                result.state.ascii_mode, active,
                "{style}, keysym={keysym:#x}"
            );
            assert_eq!(result.state.preedit, if active { "" } else { "fjv" });
        }
        drop(runtime);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
