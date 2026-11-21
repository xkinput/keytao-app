use crate::{install::*, addon::*, scheme_files::*};

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
    fn test_merge_dc_keeps_other_installed_keytao_schemas() {
        let existing =
            "patch:\n  schema_list:\n    - schema: my_schema\n    - schema: keytao_b\n    - schema: txjx\n";
        let zip = "patch:\n  schema_list:\n    - schema: keytao_b\n    - schema: keytao_bg\n";
        let (merged, user) = merge_default_custom(Some(existing), zip);
        assert_eq!(user, vec!["my_schema"]);
        assert!(merged.contains("- schema: keytao_b"));
        assert!(merged.contains("- schema: keytao_bg"));
        assert!(merged.contains("- schema: txjx"));
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
        assert!(merged.contains("- schema: keytao"));
        assert!(merged.contains("- schema: xmjd6"));
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
    fn rime_lua_install_preserves_package_verbatim() {
        assert_eq!(
            prepare_rime_lua_install(Some(KEYTAO_RIME_LUA), &[]).as_deref(),
            Some(KEYTAO_RIME_LUA)
        );
        assert_eq!(prepare_rime_lua_install(None, &[]), None);
    }

    #[test]
    fn rime_lua_install_appends_addons_once_without_rebinding_scheme_globals() {
        let package = "number_translator = require(\"xnumber\")\n";
        let addon = "addon_filter = require(\"addon_filter\")";
        let generated = prepare_rime_lua_install(
            Some(package),
            &[
                "number_translator = require(\"translators.xnumber\")",
                addon,
                addon,
            ],
        )
        .unwrap();
        assert_eq!(generated, format!("{package}{addon}\n"));
        assert_eq!(
            prepare_rime_lua_install(Some(&generated), &[addon]),
            Some(generated)
        );
    }
