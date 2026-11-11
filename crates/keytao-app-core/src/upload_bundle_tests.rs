use super::*;
use std::{fs, io::Read};

fn write(root: &Path, name: &str, text: &str) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "main.schema.yaml",
        "schema:\n  schema_id: main\n  dependencies: [helper]\nspeller:\n  alphabet: abc\ntranslator:\n  dictionary: main\ncustom_phrase:\n  db_class: stabledb\n  user_dict: phrases\nengine:\n  filters: [lua_filter@*mods.star, lua_filter@plain]\npunctuator:\n  import_preset: symbols\nsimplifier:\n  opencc_config: demo.json\n",
    );
    write(
        root,
        "main.custom.yaml",
        "patch:\n  __include: extras:/patch\n",
    );
    write(
        root,
        "helper.schema.yaml",
        "schema:\n  schema_id: helper\nspeller:\n  alphabet: abc\ntranslator:\n  dictionary: helper\n",
    );
    write(
        root,
        "main.dict.yaml",
        "---\nname: main\nimport_tables: [base, main.user]\n...\nword\ta\n",
    );
    write(root, "base.dict.yaml", "---\nname: base\n...\nbase\tb\n");
    write(
        root,
        "main.user.dict.yaml",
        "---\nname: main.user\nimport_tables: [personal]\n...\n",
    );
    write(
        root,
        "personal.dict.yaml",
        "---\nname: personal\n...\nmyword\tc\n",
    );
    write(root, "helper.dict.yaml", "---\nname: helper\n...\n");
    write(root, "phrases.txt", "custom\ta\n");
    write(root, "symbols.yaml", "punctuator: {}\n");
    write(root, "extras.yaml", "patch: {}\n");
    write(
        root,
        "lua/mods/star.lua",
        "local x = require('shared.child')\nreturn x\n",
    );
    write(
        root,
        "lua/shared/child/init.lua",
        "return require 'shared/leaf'\n",
    );
    write(root, "lua/shared/leaf.lua", "return {}\n");
    write(
        root,
        "lua/plain_impl.lua",
        "return require('shared.leaf')\n",
    );
    write(
        root,
        "rime.lua",
        "plain = require('plain_impl')\nother = require('unrelated')\n",
    );
    write(
        root,
        "opencc/demo.json",
        r#"{"conversion_chain":[{"dict":{"type":"group","dicts":[{"type":"ocd2","file":"demo.ocd2"},{"type":"text","file":"demo.txt"}]}}]}"#,
    );
    write(root, "opencc/demo.ocd2", "binary");
    write(root, "opencc/demo.txt", "one\ttwo\n");
    for name in [
        "other.schema.yaml",
        "lua/unrelated.lua",
        "build/main.table.bin",
        "sync/a",
        "main.userdb/a",
        "installation.yaml",
        "user.yaml",
        ".keytao-sync-base-a",
        "main.broken-old",
    ] {
        write(root, name, "unrelated");
    }
    dir
}

#[test]
fn closure_contains_only_dependencies_and_deployed_customizations() {
    let dir = fixture();
    let bundle = build(dir.path(), None, &["main".into()]).unwrap();
    let expected = [
        "base.dict.yaml",
        "extras.yaml",
        "helper.dict.yaml",
        "helper.schema.yaml",
        "lua/mods/star.lua",
        "lua/plain_impl.lua",
        "lua/shared/child/init.lua",
        "lua/shared/leaf.lua",
        "main.custom.yaml",
        "main.dict.yaml",
        "main.schema.yaml",
        "main.user.dict.yaml",
        "opencc/demo.json",
        "opencc/demo.ocd2",
        "opencc/demo.txt",
        "personal.dict.yaml",
        "phrases.txt",
        "rime.lua",
        "symbols.yaml",
    ];
    let mut expected = expected.to_vec();
    expected.push("default.custom.yaml");
    expected.sort();
    assert_eq!(bundle.files, expected);
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&bundle.bytes)).unwrap();
    let mut lua = String::new();
    zip.by_name("rime.lua")
        .unwrap()
        .read_to_string(&mut lua)
        .unwrap();
    assert_eq!(lua, "plain = require(\"plain_impl\")\n");
}

fn compiled(root: &Path) {
    for id in ["main", "helper"] {
        let text = fs::read_to_string(root.join(format!("{id}.schema.yaml"))).unwrap();
        write(root, &format!("build/{id}.schema.yaml"), &text);
        for suffix in ["prism.bin", "table.bin", "reverse.bin"] {
            write(root, &format!("build/{id}.{suffix}"), "compiled");
        }
    }
    fn times(root: &Path, time: std::time::SystemTime) {
        for entry in fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                times(&path, time);
            } else {
                fs::File::open(path)
                    .unwrap()
                    .set_times(fs::FileTimes::new().set_modified(time))
                    .unwrap();
            }
        }
    }
    times(
        root,
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(1000),
    );
    times(
        &root.join("build"),
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(2000),
    );
}

#[test]
fn artifacts_follow_translator_dictionary_and_prism_names() {
    for prism in [None, Some("custom_prism")] {
        let dir = fixture();
        write(
            dir.path(),
            "main.schema.yaml",
            &format!(
                "schema:\n  schema_id: main\nspeller:\n  alphabet: abc\nengine:\n  translators: [table_translator, table_translator@extra, reverse_lookup_translator@reverse]\n  filters: [reverse_lookup_filter@comments]\ntranslator:\n  dictionary: extended\n{}extra:\n  dictionary: supplemental\n  prism: extra_prism\nreverse:\n  dictionary: lookup\n  prism: lookup_prism\ncomments:\n  dictionary: annotations\n",
                prism.map_or(String::new(), |name| format!("  prism: {name}\n"))
            ),
        );
        for dictionary in ["extended", "supplemental", "lookup", "annotations"] {
            write(
                dir.path(),
                &format!("{dictionary}.dict.yaml"),
                &format!("---\nname: {dictionary}\n...\nword\ta\n"),
            );
        }
        compiled(dir.path());
        for suffix in ["table.bin", "reverse.bin", "prism.bin"] {
            fs::remove_file(dir.path().join(format!("build/main.{suffix}"))).unwrap();
        }
        for dictionary in ["extended", "supplemental", "lookup"] {
            for suffix in ["table.bin", "reverse.bin"] {
                write(
                    dir.path(),
                    &format!("build/{dictionary}.{suffix}"),
                    "compiled",
                );
            }
        }
        write(dir.path(), "build/annotations.reverse.bin", "compiled");
        for name in [prism.unwrap_or("extended"), "extra_prism", "lookup_prism"] {
            write(dir.path(), &format!("build/{name}.prism.bin"), "compiled");
        }
        verified_artifacts(dir.path(), None, &["main".into()]).unwrap();
        fs::remove_file(dir.path().join("build/extra_prism.prism.bin")).unwrap();
        let error = verified_artifacts(dir.path(), None, &["main".into()])
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("main") && error.contains("build/extra_prism.prism.bin"),
            "{error}"
        );
    }
}

#[test]
fn optional_user_text_tables_are_included_only_when_present() {
    let dir = fixture();
    write(
        dir.path(),
        "main.schema.yaml",
        "schema:\n  schema_id: main\ntranslator:\n  dictionary: main\n  db_class: stabledb\n  user_dict: user_dict\ncustom_phrase:\n  db_class: tabledb\n  user_dict: custom_phrase.txt\n",
    );
    let absent = build(dir.path(), None, &["main".into()]).unwrap();
    for name in ["user_dict.txt", "custom_phrase.txt"] {
        assert!(!absent.files.contains(&name.into()));
        write(dir.path(), name, "custom\ta\n");
    }
    let present = build(dir.path(), None, &["main".into()]).unwrap();
    for name in ["user_dict.txt", "custom_phrase.txt"] {
        assert!(present.files.contains(&name.into()));
    }
}

#[test]
fn shared_dictionary_artifacts_need_not_exist_in_user_build() {
    for shared_build in [false, true] {
        let user = fixture();
        let shared = tempfile::tempdir().unwrap();
        let schema = "schema:\n  schema_id: main\ntranslator:\n  dictionary: luna_pinyin\n";
        write(user.path(), "main.schema.yaml", schema);
        compiled(user.path());
        write(
            shared.path(),
            "luna_pinyin.dict.yaml",
            "name: luna_pinyin\n",
        );
        if shared_build {
            for suffix in ["table.bin", "reverse.bin", "prism.bin"] {
                write(
                    shared.path(),
                    &format!("build/luna_pinyin.{suffix}"),
                    "compiled",
                );
            }
        }
        let bundle =
            verified_artifacts(user.path(), Some(shared.path()), &["main".into()]).unwrap();
        assert!(!bundle.files.contains(&"luna_pinyin.dict.yaml".into()));
    }
}

#[test]
fn reverse_lookup_translator_only_requires_reverse_bin() {
    let user = fixture();
    let schema = "schema:\n  schema_id: main\nengine:\n  translators: [reverse_lookup_translator]\nreverse_lookup:\n  dictionary: lookup\n";
    write(user.path(), "main.schema.yaml", schema);
    write(user.path(), "lookup.dict.yaml", "name: lookup\n");
    compiled(user.path());
    write(user.path(), "build/lookup.reverse.bin", "compiled");
    verified_artifacts(user.path(), None, &["main".into()]).unwrap();
    fs::remove_file(user.path().join("build/lookup.reverse.bin")).unwrap();
    assert!(
        verified_artifacts(user.path(), None, &["main".into()])
            .unwrap_err()
            .to_string()
            .contains("lookup.reverse.bin")
    );
}

#[test]
fn shared_dictionary_is_satisfied_without_uploading_it() {
    let user = tempfile::tempdir().unwrap();
    let shared = tempfile::tempdir().unwrap();
    write(
        user.path(),
        "main.schema.yaml",
        "schema:\n  schema_id: main\ntranslator:\n  dictionary: luna_pinyin\n",
    );
    write(
        shared.path(),
        "luna_pinyin.dict.yaml",
        "---\nname: luna_pinyin\n...\n",
    );
    let bundle = build(user.path(), Some(shared.path()), &["main".into()]).unwrap();
    assert_eq!(bundle.files, ["default.custom.yaml", "main.schema.yaml"]);
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&bundle.bytes)).unwrap();
    assert!(zip.by_name("luna_pinyin.dict.yaml").is_err());
    fs::remove_file(shared.path().join("luna_pinyin.dict.yaml")).unwrap();
    let error = build(user.path(), Some(shared.path()), &["main".into()])
        .unwrap_err()
        .to_string();
    assert!(error.contains("luna_pinyin.dict.yaml"), "{error}");
}

#[test]
fn shared_dependencies_are_excluded_and_user_files_take_precedence() {
    for name in [
        "main.dict.yaml",
        "base.dict.yaml",
        "helper.schema.yaml",
        "symbols.yaml",
        "extras.yaml",
        "lua/mods/star.lua",
        "lua/shared/child/init.lua",
        "rime.lua",
        "opencc/demo.json",
        "opencc/demo.ocd2",
    ] {
        let user = fixture();
        compiled(user.path());
        let shared = tempfile::tempdir().unwrap();
        let bytes = fs::read(user.path().join(name)).unwrap();
        write(shared.path(), name, "invalid shared override");
        let bundle = build(user.path(), Some(shared.path()), &["main".into()]).unwrap();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&bundle.bytes)).unwrap();
        let mut uploaded = Vec::new();
        zip.by_name(name)
            .unwrap()
            .read_to_end(&mut uploaded)
            .unwrap();
        if name != "rime.lua" {
            assert_eq!(uploaded, bytes, "{name}");
        }
        fs::write(shared.path().join(name), bytes).unwrap();
        fs::remove_file(user.path().join(name)).unwrap();
        let bundle =
            verified_artifacts(user.path(), Some(shared.path()), &["main".into()]).unwrap();
        assert!(!bundle.files.contains(&name.into()), "{name}");
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&bundle.bytes)).unwrap();
        assert!(zip.by_name(name).is_err(), "{name}");
        fs::remove_file(shared.path().join(name)).unwrap();
        let error = build(user.path(), Some(shared.path()), &["main".into()])
            .unwrap_err()
            .to_string();
        assert!(error.contains(name), "{name}: {error}");
    }
}

#[test]
fn shared_presets_resolve_default_and_alternate_symbol_names() {
    for (preset, name) in [
        ("default", "default.yaml"),
        ("custom", "custom.symbols.yaml"),
    ] {
        let user = tempfile::tempdir().unwrap();
        let shared = tempfile::tempdir().unwrap();
        write(
            user.path(),
            "main.schema.yaml",
            &format!("schema:\n  schema_id: main\npunctuator:\n  import_preset: {preset}\n"),
        );
        write(shared.path(), name, "punctuator: {}\n");
        let bundle = build(user.path(), Some(shared.path()), &["main".into()]).unwrap();
        assert_eq!(bundle.files, ["default.custom.yaml", "main.schema.yaml"]);
        fs::remove_file(shared.path().join(name)).unwrap();
        let error = build(user.path(), Some(shared.path()), &["main".into()])
            .unwrap_err()
            .to_string();
        assert!(error.contains(&format!("{preset}.yaml")), "{error}");
    }
}

#[test]
fn shared_lua_still_rejects_unsafe_reference_paths() {
    let user = tempfile::tempdir().unwrap();
    let shared = tempfile::tempdir().unwrap();
    write(
        user.path(),
        "main.schema.yaml",
        "schema:\n  schema_id: main\nengine:\n  filters: [lua_filter@*mods//star]\n",
    );
    write(shared.path(), "lua/mods/star.lua", "return {}\n");
    let error = build(user.path(), Some(shared.path()), &["main".into()])
        .unwrap_err()
        .to_string();
    assert!(error.contains("Unsafe package path"), "{error}");
}

#[test]
fn missing_referenced_file_refuses_upload_with_filename() {
    let dir = fixture();
    compiled(dir.path());
    fs::remove_file(dir.path().join("personal.dict.yaml")).unwrap();
    let error = verified(dir.path(), None, &["main".into()], |_, _| Ok(()))
        .unwrap_err()
        .to_string();
    assert!(error.contains("personal.dict.yaml"), "{error}");
}

#[test]
fn missing_build_artifact_refuses_upload_with_schema_and_filename() {
    let dir = fixture();
    compiled(dir.path());
    fs::remove_file(dir.path().join("build/helper.reverse.bin")).unwrap();
    let error = verified(dir.path(), None, &["main".into()], |_, _| Ok(()))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("helper") && error.contains("build/helper.reverse.bin"),
        "{error}"
    );
}

#[test]
fn stale_build_artifact_refuses_upload_with_source_and_filename() {
    let dir = fixture();
    compiled(dir.path());
    fs::File::open(dir.path().join("personal.dict.yaml"))
        .unwrap()
        .set_times(
            fs::FileTimes::new()
                .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(3000)),
        )
        .unwrap();
    let error = verified(dir.path(), None, &["main".into()], |_, _| Ok(()))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("main")
            && error.contains("build/main.")
            && error.contains("personal.dict.yaml"),
        "{error}"
    );
}

#[test]
fn verified_archive_is_deterministic_and_smokes_every_schema() {
    let dir = fixture();
    compiled(dir.path());
    let mut smoked = Vec::new();
    let first = verified(dir.path(), None, &["main".into()], |id, key| {
        smoked.push((id.to_owned(), key));
        Ok(())
    })
    .unwrap();
    fs::File::open(dir.path().join("main.schema.yaml"))
        .unwrap()
        .set_times(
            fs::FileTimes::new()
                .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(500)),
        )
        .unwrap();
    let second = verified(dir.path(), None, &["main".into()], |_, _| Ok(())).unwrap();
    assert_eq!(
        smoked,
        [
            ("helper".into(), u32::from('a')),
            ("main".into(), u32::from('a'))
        ]
    );
    assert_eq!(first.bytes, second.bytes);
    assert_eq!(first.sha256, second.sha256);
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&first.bytes)).unwrap();
    for index in 0..zip.len() {
        let timestamp = zip.by_index(index).unwrap().last_modified().unwrap();
        assert_eq!(
            (
                timestamp.year(),
                timestamp.month(),
                timestamp.day(),
                timestamp.hour(),
                timestamp.minute(),
                timestamp.second()
            ),
            (1980, 1, 1, 0, 0, 0)
        );
    }
}

#[test]
fn smoke_failure_refuses_upload_with_schema() {
    let dir = fixture();
    compiled(dir.path());
    let error = verified(dir.path(), None, &["main".into()], |_, _| {
        Err("session error".into())
    })
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("helper") && error.contains("session error"),
        "{error}"
    );
}

#[test]
fn deployed_custom_patch_exports_only_the_effective_dictionary_and_text_table() {
    let dir = fixture();
    write(
        dir.path(),
        "main.custom.yaml",
        "patch:\n  translator/dictionary: selected\n  custom_phrase/user_dict: custom_notes\n",
    );
    write(
        dir.path(),
        "selected.dict.yaml",
        "---\nname: selected\n...\ncustom\ta\n",
    );
    write(dir.path(), "custom_notes.txt", "custom\ta\n");
    compiled(dir.path());
    let effective = fs::read_to_string(dir.path().join("main.schema.yaml"))
        .unwrap()
        .replace("dictionary: main", "dictionary: selected")
        .replace("user_dict: phrases", "user_dict: custom_notes");
    write(dir.path(), "build/main.schema.yaml", &effective);
    write(dir.path(), "build/selected.table.bin", "compiled");
    write(dir.path(), "build/selected.reverse.bin", "compiled");
    write(dir.path(), "build/selected.prism.bin", "compiled");
    fs::remove_file(dir.path().join("main.dict.yaml")).unwrap();
    let bundle = verified(dir.path(), None, &["main".into()], |_, _| Ok(())).unwrap();
    assert!(bundle.files.contains(&"selected.dict.yaml".into()));
    assert!(bundle.files.contains(&"custom_notes.txt".into()));
    assert!(!bundle.files.contains(&"phrases.txt".into()));
    assert!(!bundle.files.contains(&"personal.dict.yaml".into()));
}

#[test]
fn includes_follow_only_the_referenced_section() {
    let dir = fixture();
    write(
        dir.path(),
        "extras.yaml",
        "patch:\n  __include: nested:/needed\nother:\n  dictionary: missing_other_scheme\n",
    );
    write(
        dir.path(),
        "nested.yaml",
        "needed: {}\nother:\n  dictionary: missing_other_scheme\n",
    );
    let bundle = build(dir.path(), None, &["main".into()]).unwrap();
    assert!(bundle.files.contains(&"nested.yaml".into()));
    assert!(
        !bundle
            .files
            .iter()
            .any(|file| file.contains("missing_other_scheme"))
    );
}

#[test]
fn nested_opencc_config_resolves_its_relative_dictionary() {
    let dir = fixture();
    write(
        dir.path(),
        "main.custom.yaml",
        "patch:\n  simplifier/opencc_config: sub/nested.json\n",
    );
    write(
        dir.path(),
        "opencc/sub/nested.json",
        r#"{"conversion_chain":[{"dict":{"type":"text","file":"./nested.txt"}}]}"#,
    );
    write(dir.path(), "opencc/sub/nested.txt", "one\ttwo\n");
    let bundle = build(dir.path(), None, &["main".into()]).unwrap();
    assert!(bundle.files.contains(&"opencc/sub/nested.txt".into()));
    let shared = tempfile::tempdir().unwrap();
    write(shared.path(), "opencc/nested.txt", "one\ttwo\n");
    fs::remove_file(dir.path().join("opencc/sub/nested.txt")).unwrap();
    let bundle = build(dir.path(), Some(shared.path()), &["main".into()]).unwrap();
    assert!(!bundle.files.contains(&"opencc/sub/nested.txt".into()));
    assert!(!bundle.files.contains(&"opencc/nested.txt".into()));
}

#[test]
fn lua_long_strings_and_comments_do_not_add_fake_dependencies() {
    let dir = fixture();
    write(
        dir.path(),
        "lua/mods/star.lua",
        "local text = [=[require('missing_fake')]=]\n--[==[ require('missing_comment') ]==]\nreturn require('shared.child')\n",
    );
    let bundle = build(dir.path(), None, &["main".into()]).unwrap();
    assert!(bundle.files.contains(&"lua/shared/child/init.lua".into()));
}

#[test]
fn pcall_requires_skip_missing_modules_but_include_present_modules_transitively() {
    let dir = fixture();
    write(
        dir.path(),
        "lua/mods/star.lua",
        "local ok, x = pcall(require, 'shared.optional')\npcall(require, \"lfs\")\npcall(require, 'missing_native')\nreturn x\n",
    );
    write(
        dir.path(),
        "lua/shared/optional/init.lua",
        "return require('shared.leaf')\n",
    );
    let bundle = build(dir.path(), None, &["main".into()]).unwrap();
    assert!(
        bundle
            .files
            .contains(&"lua/shared/optional/init.lua".into())
    );
    assert!(bundle.files.contains(&"lua/shared/leaf.lua".into()));
    assert!(
        !bundle
            .files
            .iter()
            .any(|name| name.contains("lfs") || name.contains("missing_native"))
    );
    write(
        dir.path(),
        "lua/mods/star.lua",
        "pcall(require, 'lfs')\nreturn require('lfs')\n",
    );
    let error = build(dir.path(), None, &["main".into()])
        .unwrap_err()
        .to_string();
    assert!(error.contains("lua/lfs.lua"), "{error}");
}

#[test]
fn runtime_resources_do_not_make_unrelated_compiled_outputs_stale() {
    let dir = fixture();
    compiled(dir.path());
    write(
        dir.path(),
        "lua/shared/leaf.lua",
        "return { updated = true }\n",
    );
    write(dir.path(), "phrases.txt", "new phrase\ta\n");
    write(dir.path(), "opencc/demo.txt", "updated\tword\n");
    let bundle = verified(dir.path(), None, &["main".into()], |_, _| Ok(())).unwrap();
    assert!(bundle.files.contains(&"phrases.txt".into()));
}

#[test]
fn reverse_lookup_filter_requires_only_its_reverse_artifact() {
    let dir = fixture();
    let schema = fs::read_to_string(dir.path().join("main.schema.yaml"))
        .unwrap()
        .replace(
            "lua_filter@plain]",
            "lua_filter@plain, reverse_lookup_filter@comments]",
        );
    write(
        dir.path(),
        "main.schema.yaml",
        &format!("{schema}\ncomments:\n  dictionary: comments\n"),
    );
    write(
        dir.path(),
        "comments.dict.yaml",
        "---\nname: comments\n...\nword\ta\n",
    );
    compiled(dir.path());
    write(dir.path(), "build/comments.reverse.bin", "compiled");
    let bundle = verified(dir.path(), None, &["main".into()], |_, _| Ok(())).unwrap();
    assert!(bundle.files.contains(&"comments.dict.yaml".into()));
}

#[test]
#[ignore = "requires KEYTAO_UPLOAD_FIXTURE pointing at a temporary copy of local Rime data"]
fn copied_local_liangfen_resolves_shared_reverse_lookup() {
    let root = std::path::PathBuf::from(
        std::env::var_os("KEYTAO_UPLOAD_FIXTURE").expect("temporary fixture path"),
    );
    let shared = std::path::PathBuf::from(
        std::env::var_os("KEYTAO_UPLOAD_SHARED").expect("shared fixture path"),
    );
    let bundle = verified_artifacts(&root, Some(&shared), &["liangfen".into()]).unwrap();
    assert!(!bundle.files.contains(&"luna_pinyin.dict.yaml".into()));
    println!(
        "COPIED_LIANGFEN files={} source_bytes={} zip_bytes={}",
        bundle.files.len(),
        bundle.total_size,
        bundle.bytes.len()
    );
}

#[test]
fn real_xmjd6_sample_is_read_only_and_never_executed() {
    let source = Path::new(
        "/private/tmp/claude-501/-Users-rea-code-keytao-org-keytao-app/348229fe-6d80-4d02-b020-e4d903e04588/scratchpad/lua-inv/xmjd6",
    );
    if !source.is_dir() {
        eprintln!("SKIPPED: external xmjd6 sample is unavailable");
        return;
    }
    fn copy(source: &Path, dest: &Path) {
        fs::create_dir_all(dest).unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            assert!(!entry.file_type().unwrap().is_symlink());
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &dest.join(entry.file_name()));
            } else {
                fs::copy(entry.path(), dest.join(entry.file_name())).unwrap();
            }
        }
    }
    let dir = tempfile::tempdir().unwrap();
    copy(source, dir.path());
    fn inventory(root: &Path, dir: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                inventory(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    inventory(dir.path(), dir.path(), &mut files);
    let full_size: usize = files.values().map(Vec::len).sum();
    let count = files.len();
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in &files {
        zip.start_file(
            name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .last_modified_time(zip::DateTime::default()),
        )
        .unwrap();
        zip.write_all(bytes).unwrap();
    }
    let full_zip_size = zip.finish().unwrap().into_inner().len();
    // Complete missing distribution resources only in the temp fixture, never the source.
    let shared = tempfile::tempdir().unwrap();
    for (name, text) in [
        ("luna_pinyin.dict.yaml", "---\nname: luna_pinyin\n...\n"),
        ("stroke.dict.yaml", "---\nname: stroke\n...\n"),
    ] {
        write(shared.path(), name, text);
    }
    let mut added_fixtures = Vec::new();
    if !dir.path().join("opencc/emoji.txt").exists() {
        write(dir.path(), "opencc/emoji.txt", "");
        added_fixtures.push("opencc/emoji.txt");
    }
    let mut copy_before = BTreeMap::new();
    inventory(dir.path(), dir.path(), &mut copy_before);
    let bundle = build(dir.path(), Some(shared.path()), &["xmjd6".into()]).unwrap();
    assert!(!bundle.files.contains(&"luna_pinyin.dict.yaml".into()));
    assert!(!bundle.files.contains(&"stroke.dict.yaml".into()));
    let mut source_after = BTreeMap::new();
    inventory(source, source, &mut source_after);
    assert_eq!(
        source_after, files,
        "the sample source must remain unchanged"
    );
    let mut copy_after = BTreeMap::new();
    inventory(dir.path(), dir.path(), &mut copy_after);
    assert_eq!(
        copy_after, copy_before,
        "building must not modify the temp input"
    );
    println!(
        "XMJD6_CLOSURE files={} source_bytes={} zip_bytes={} temp_fixtures={added_fixtures:?} full_files={count} full_source_bytes={full_size} full_zip_bytes={full_zip_size}",
        bundle.files.len(),
        bundle.total_size,
        bundle.bytes.len()
    );
    println!("XMJD6_FILES {:?}", bundle.files);
}
