use crate::{install::*, addon::*, scheme_files::*};
use std::collections::HashSet;

    const KEYTAO_RIME_LUA: &str = concat!(
        "--[[\n",
        "librime-lua 样例\n",
        "```\n",
        "  engine:\n",
        "    translators:\n",
        "      - lua_translator@lua_function3\n",
        "      - lua_translator@lua_function4\n",
        "    filters:\n",
        "      - lua_filter@lua_function1\n",
        "      - lua_filter@lua_function2\n",
        "```\n",
        "其中各 `lua_function` 为在本文件所定义变量名。\n",
        "--]]\n",
        "\n",
        "--[[\n",
        "本文件的后面是若干个例子，按照由简单到复杂的顺序示例了 librime-lua 的用法。\n",
        "每个例子都被组织在 `lua` 目录下的单独文件中，打开对应文件可看到实现和注解。\n",
        "\n",
        "各例可使用 `require` 引入。\n",
        "```\n",
        "  foo = require(\"bar\")\n",
        "```\n",
        "可认为是载入 `lua/bar.lua` 中的例子，并起名为 `foo`。\n",
        "配方文件中的引用方法为：`...@foo`。\n",
        "--]]\n",
        "\n",
        "date_time_translator = require(\"date_time\")\n",
        "\n",
        "\n",
        "-- single_char_filter: 候选项重排序，使单字优先\n",
        "-- 详见 `lua/single_char.lua`\n",
        "-- single_char_filter = require(\"single_char\")\n",
        "\n",
        "\n",
        "-- keytao_filter: 单字模式 & 630 即 ss 词组提示\n",
        "-- 详见 `lua/keytao_filter.lua`\n",
        "keytao_filter = require(\"keytao_filter\")\n",
        "\n",
        "-- 顶功处理器\n",
        "topup_processor = require(\"for_topup\")\n",
        "\n",
        "-- 声笔笔简码提示 | 顶功提示 | 补全处理\n",
        "hint_filter = require(\"for_hint\")\n",
        "\n",
        "-- number_translator: 将 `=` + 阿拉伯数字 翻译为大小写汉字\n",
        "number_translator = require(\"xnumber\")\n",
        "\n",
        "-- 用 ' 作为次选键\n",
        "smart_2 = require(\"smart_2\")\n",
    );

    #[test]
    fn english_availability_includes_packaged_and_named_schemas_only_after_deploy() {
        let root = std::env::temp_dir().join(format!("keytao-english-status-{}", std::process::id()));
        std::fs::create_dir_all(root.join("build")).unwrap();
        std::fs::write(root.join("default.custom.yaml"), "patch:\n  schema_list:\n    - schema: english\n").unwrap();
        assert!(!deployed_english_schema_available(&root));
        std::fs::write(root.join("build/english.schema.yaml"), "schema: {schema_id: english, name: English}\n").unwrap();
        assert!(deployed_english_schema_available(&root));
        std::fs::write(root.join("default.custom.yaml"), "patch:\n  schema_list:\n    - schema: custom_english\n").unwrap();
        assert!(!deployed_english_schema_available(&root));
        std::fs::write(root.join("build/custom_english.schema.yaml"), "schema: {schema_id: custom_english, name: Easy English}\n").unwrap();
        assert!(deployed_english_schema_available(&root));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn test_collect_rime_package_build_ids() {
        let mut schemas = HashSet::new();
        let mut dictionaries = HashSet::new();
        for path in [
            "keydo.schema.yaml",
            "nested/pinyin_simp.schema.yaml",
            "keydo.dict.yaml",
            "keydo.phrases.dict.yaml",
            "default.custom.yaml",
            "lua/keydo.lua",
        ] {
            collect_rime_package_build_id(path, &mut schemas, &mut dictionaries);
        }

        assert_eq!(
            schemas,
            HashSet::from(["keydo".to_string(), "pinyin_simp".to_string()])
        );
        assert_eq!(
            dictionaries,
            HashSet::from(["keydo".to_string(), "keydo.phrases".to_string()])
        );
    }

    #[test]
    fn test_parse_requires_basic() {
        let content = "keytao_filter = require(\"keytao_filter\")\nfoo = require('bar')\n";
        let r = keytao_core::parse_rime_lua_requires(content);
        assert_eq!(r, vec!["keytao_filter", "bar"]);
    }

    #[test]
    fn test_parse_requires_skips_single_line_comments() {
        let content = "-- foo = require(\"foo\")\nreal = require(\"real\")\n";
        let r = keytao_core::parse_rime_lua_requires(content);
        assert_eq!(r, vec!["real"]);
    }

    #[test]
    fn test_parse_requires_skips_block_comment_content() {
        let content = "--[[\n  foo = require(\"bar\")\n--]]\nreal = require(\"real\")\n";
        let r = keytao_core::parse_rime_lua_requires(content);
        assert_eq!(r, vec!["real"]);
    }

    #[test]
    fn test_merge_appends_unique_local_require() {
        let local = "my_mod = require(\"my_mod\")\n";
        let zip = "keytao_filter = require(\"keytao_filter\")\n";
        let (merged, renames) = merge_rime_lua(local, zip, &HashSet::new());
        assert!(merged.contains("require(\"keytao_filter\")"));
        assert!(merged.contains("require(\"my_mod\")"));
        assert!(renames.is_empty());
    }

    #[test]
    fn test_merge_skips_require_already_in_zip() {
        let local = "keytao_filter = require(\"keytao_filter\")\n";
        let zip = "keytao_filter = require(\"keytao_filter\")\n";
        let (merged, _) = merge_rime_lua(local, zip, &HashSet::new());
        assert_eq!(merged.matches("require(\"keytao_filter\")").count(), 1);
    }

    #[test]
    fn test_merge_renames_conflicting_module() {
        let local = "my_mod = require(\"my_mod\")\n";
        let zip = "keytao = require(\"keytao\")\n";
        let filenames: HashSet<String> = ["my_mod.lua".to_string()].into();
        let (merged, renames) = merge_rime_lua(local, zip, &filenames);
        assert_eq!(
            renames,
            vec![("my_mod".to_string(), "my_mod_user".to_string())]
        );
        assert!(merged.contains("require(\"my_mod_user\")"));
        assert!(!merged.contains("require(\"my_mod\")"));
    }

    #[test]
    fn test_merge_ignores_block_comment_content() {
        // Reproduces the Android bug: block comment lines such as ``` were
        // appended verbatim to the merged output because the loop did not
        // track --[[ ... --]] state.
        let local = concat!(
            "--[[\n",
            "librime-lua 样例\n",
            "```\n",
            "  engine:\n",
            "    translators:\n",
            "```\n",
            "--]]\n",
            "--[[\n",
            "各例可使用 `require` 引入。\n",
            "```\n",
            "  foo = require(\"bar\")\n",
            "```\n",
            "--]]\n",
            "my_mod = require(\"my_mod\")\n",
        );
        let zip = "keytao_filter = require(\"keytao_filter\")\n";
        let (merged, renames) = merge_rime_lua(local, zip, &HashSet::new());
        assert!(!merged.contains("librime-lua"), "block comment line leaked");
        assert!(!merged.contains("engine:"), "block comment line leaked");
        assert!(!merged.contains("```"), "block comment backticks leaked");
        assert!(
            !merged.contains("require(\"bar\")"),
            "in-comment require leaked"
        );
        assert!(merged.contains("require(\"my_mod\")"));
        assert!(renames.is_empty());
    }

    #[test]
    fn test_update_addon_schema_list_appends_after_package_schemas_and_preserves_patch() {
        let existing = concat!(
            "patch:\n",
            "  schema_list:\n",
            "    - schema: keytao\n",
            "    - schema: keytao-dz\n",
            "  switcher:\n",
            "    hotkeys:\n",
            "      - F4\n",
            "  menu:\n",
            "    page_size: 7\n",
            "  ascii_composer:\n",
            "    good_old_caps_lock: true\n",
        );

        let updated = update_addon_schema_list_content(existing, "easy_en", true)
            .expect("append add-on schema");
        assert_eq!(
            parse_schema_list(&updated),
            vec!["keytao", "keytao-dz", "easy_en"]
        );

        let value: serde_yaml::Value = serde_yaml::from_str(&updated).expect("parse updated yaml");
        let patch = value
            .get("patch")
            .and_then(serde_yaml::Value::as_mapping)
            .expect("patch mapping");
        assert_eq!(
            patch.get("switcher").and_then(|value| value.get("hotkeys")),
            serde_yaml::from_str::<serde_yaml::Value>("[F4]")
                .ok()
                .as_ref()
        );
        assert_eq!(
            patch
                .get("menu")
                .and_then(|value| value.get("page_size"))
                .and_then(serde_yaml::Value::as_i64),
            Some(7)
        );
        assert_eq!(
            patch
                .get("ascii_composer")
                .and_then(|value| value.get("good_old_caps_lock"))
                .and_then(serde_yaml::Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn test_write_addon_schema_list_updates_the_existing_hyphenated_path() {
        let dir = std::env::temp_dir().join(format!(
            "keytao-addon-schema-list-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create add-on schema test dir");
        let path = dir.join("default-custom.yaml");
        std::fs::write(
            &path,
            "patch:\n  schema_list:\n    - schema: keytao\n    - schema: keytao-dz\n",
        )
        .expect("write hyphenated default custom");

        write_addon_schema_list(&dir, "easy_en", true).expect("write add-on schema list");

        assert_eq!(
            parse_schema_list(&std::fs::read_to_string(&path).expect("read updated config")),
            vec!["keytao", "keytao-dz", "easy_en"]
        );
        assert!(!dir.join("default.custom.yaml").exists());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn test_parse_schema_list_basic() {
        let content = "patch:\n  schema_list:\n    - schema: keytao_b\n    - schema: keytao_bg\n";
        assert_eq!(parse_schema_list(content), vec!["keytao_b", "keytao_bg"]);
    }

    #[test]
    fn test_parse_schema_list_strips_inline_comments() {
        let content = "patch:\n  schema_list:\n    - schema: keydo # 键道·我流\n";
        assert_eq!(parse_schema_list(content), vec!["keydo"]);
    }

    #[test]
    fn test_parse_schema_list_stops_at_non_schema() {
        let content = "patch:\n  schema_list:\n    - schema: foo\n  other_key: val\n";
        assert_eq!(parse_schema_list(content), vec!["foo"]);
    }

    #[test]
    fn test_merge_dc_preserves_user_schemas() {
        let existing = "patch:\n  schema_list:\n    - schema: my_schema\n    - schema: another\n";
        let zip = "patch:\n  schema_list:\n    - schema: keytao_b\n    - schema: keytao_bg\n";
        let (merged, user) = merge_default_custom(Some(existing), zip);
        assert!(merged.contains("- schema: my_schema"));
        assert!(merged.contains("- schema: another"));
        assert!(merged.contains("- schema: keytao_b"));
        assert_eq!(user, vec!["my_schema", "another"]);
    }

    #[test]
    fn test_merge_dc_excludes_user_keytao_schemas() {
        let existing =
            "patch:\n  schema_list:\n    - schema: my_schema\n    - schema: keytao_b\n    - schema: txjx\n";
        let zip = "patch:\n  schema_list:\n    - schema: keytao_b\n    - schema: keytao_bg\n";
        let (merged, user) = merge_default_custom(Some(existing), zip);
        assert_eq!(user, vec!["my_schema"]);
        assert!(merged.contains("- schema: keytao_b"));
        assert!(merged.contains("- schema: keytao_bg"));
        assert!(!merged.contains("- schema: txjx"));
    }

    #[test]
    fn test_merge_dc_no_existing_file() {
        let zip = "patch:\n  schema_list:\n    - schema: keytao_b\n";
        let (merged, user) = merge_default_custom(None, zip);
        assert!(user.is_empty());
        assert!(merged.contains("- schema: keytao_b"));
    }

    #[test]
    fn test_merge_dc_supports_non_keytao_scheme_package() {
        let existing =
            "patch:\n  schema_list:\n    - schema: my_schema\n    - schema: keytao\n    - schema: xmjd6\n";
        let zip = "patch:\n  schema_list:\n    - schema: txjx\n";
        let (merged, user) = merge_default_custom(Some(existing), zip);
        assert_eq!(user, vec!["my_schema"]);
        assert!(merged.contains("- schema: my_schema"));
        assert!(merged.contains("- schema: txjx"));
        assert!(!merged.contains("- schema: keytao"));
        assert!(!merged.contains("- schema: xmjd6"));
    }

    #[test]
    fn test_parse_requires_keytao_rime_lua() {
        // Block comment contains `foo = require("bar")` which must NOT be included.
        let requires = keytao_core::parse_rime_lua_requires(KEYTAO_RIME_LUA);
        assert_eq!(
            requires,
            vec![
                "date_time",
                "keytao_filter",
                "for_topup",
                "for_hint",
                "xnumber",
                "smart_2"
            ]
        );
        assert!(
            !requires.contains(&"bar".to_string()),
            "in-comment require must not be parsed"
        );
    }

    #[test]
    fn test_merge_reinstall_no_duplicates() {
        // Installing over an existing identical rime.lua should produce the same file.
        let (merged, renames) = merge_rime_lua(KEYTAO_RIME_LUA, KEYTAO_RIME_LUA, &HashSet::new());
        assert!(renames.is_empty());
        // Every require should appear exactly once.
        for module in &[
            "date_time",
            "keytao_filter",
            "for_topup",
            "for_hint",
            "xnumber",
            "smart_2",
        ] {
            let needle = format!("require(\"{module}\")");
            assert_eq!(
                merged.matches(needle.as_str()).count(),
                1,
                "require(\"{module}\") duplicated after reinstall"
            );
        }
    }

    #[test]
    fn test_merge_user_extra_module_appended() {
        // User has the keytao rime.lua as local, plus one extra module.
        let local = format!("{KEYTAO_RIME_LUA}my_custom = require(\"my_custom\")\n");
        let (merged, renames) = merge_rime_lua(&local, KEYTAO_RIME_LUA, &HashSet::new());
        assert!(renames.is_empty());
        assert!(merged.contains("require(\"my_custom\")"));
        // Keytao requires still appear exactly once.
        assert_eq!(merged.matches("require(\"keytao_filter\")").count(), 1);
    }

    #[test]
    fn test_merge_user_extra_module_conflict_renamed() {
        // User has a custom `date_time.lua` that would be overwritten by zip.
        let local = format!("{KEYTAO_RIME_LUA}my_dt = require(\"my_dt\")\n");
        let filenames: HashSet<String> = ["my_dt.lua".to_string()].into();
        let (merged, renames) = merge_rime_lua(&local, KEYTAO_RIME_LUA, &filenames);
        assert_eq!(
            renames,
            vec![("my_dt".to_string(), "my_dt_user".to_string())]
        );
        assert!(merged.contains("require(\"my_dt_user\")"));
        assert!(!merged.contains("require(\"my_dt\")"));
    }

    #[test]
    fn test_merge_zip_is_base_local_keytao_no_duplicates() {
        // Local already has the same keytao rime.lua; merged must equal zip exactly.
        let (merged, renames) = merge_rime_lua(KEYTAO_RIME_LUA, KEYTAO_RIME_LUA, &HashSet::new());
        assert_eq!(merged, KEYTAO_RIME_LUA);
        assert!(renames.is_empty());
    }

    #[test]
    fn test_merge_old_keytao_missing_module_zip_provides_it() {
        // Local = older keytao rime.lua without smart_2.
        // Zip = new keytao rime.lua with smart_2.
        // smart_2 must appear exactly once in merged output.
        let old_local: String = KEYTAO_RIME_LUA
            .lines()
            .filter(|l| !l.trim_start().starts_with("smart_2"))
            .collect::<Vec<_>>()
            .join("\n");
        let (merged, renames) = merge_rime_lua(&old_local, KEYTAO_RIME_LUA, &HashSet::new());
        assert_eq!(merged.matches("require(\"smart_2\")").count(), 1);
        assert!(renames.is_empty());
    }

    #[test]
    fn test_merge_user_extra_preserved_zip_overwrites_keytao_no_dups() {
        // Local = keytao rime.lua + user-defined module.
        // Zip = same keytao rime.lua (re-install / upgrade).
        // merged must start with zip content; user module appended once;
        // every keytao module appears exactly once.
        let local = format!("{KEYTAO_RIME_LUA}user_plugin = require(\"user_plugin\")\n");
        let (merged, renames) = merge_rime_lua(&local, KEYTAO_RIME_LUA, &HashSet::new());
        assert!(merged.starts_with(KEYTAO_RIME_LUA));
        assert!(merged.contains("require(\"user_plugin\")"));
        for module in &[
            "date_time",
            "keytao_filter",
            "for_topup",
            "for_hint",
            "xnumber",
            "smart_2",
        ] {
            assert_eq!(
                merged.matches(&format!("require(\"{module}\")")).count(),
                1,
                "require(\"{module}\") must appear exactly once"
            );
        }
        assert!(renames.is_empty());
    }

    #[test]
    fn test_merge_keytao_rime_lua_no_block_comment_leak() {
        // Using actual keytao rime.lua as local; merged result must not contain
        // any content from the --[[ ]] header blocks.
        let local = KEYTAO_RIME_LUA;
        let zip = "keytao_filter = require(\"keytao_filter\")\n";
        let (merged, _) = merge_rime_lua(local, zip, &HashSet::new());
        assert!(
            !merged.contains("librime-lua"),
            "block comment header leaked"
        );
        assert!(!merged.contains("engine:"), "block comment content leaked");
        assert!(
            !merged.contains("```"),
            "backticks from block comment leaked"
        );
        assert!(
            !merged.contains("require(\"bar\")"),
            "in-comment require leaked"
        );
    }
