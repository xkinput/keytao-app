use super::{
    patch_neutral_grammar, wanxiang_grammar_enabled, NEUTRAL_GRAMMAR_PENALTY, WANXIANG_GRAMMAR_FILE,
};
use serde_yaml::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

fn scratch_dir() -> PathBuf {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "keytao-grammar-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(dir.join("build")).unwrap();
    dir
}

fn enable_wanxiang(dir: &Path) {
    fs::write(dir.join(WANXIANG_GRAMMAR_FILE), b"fixture").unwrap();
    fs::write(
        dir.join("build/wanxiang.schema.yaml"),
        "schema:\n  schema_id: wanxiang\ngrammar:\n  language: wanxiang-lts-zh-hans\n",
    )
    .unwrap();
}

#[test]
fn module_gate_requires_model_and_deployed_wanxiang_language() {
    let dir = scratch_dir();
    assert!(!wanxiang_grammar_enabled(&dir));
    fs::write(
        dir.join("wanxiang.schema.yaml"),
        "grammar:\n  language: wanxiang-lts-zh-hans\n",
    )
    .unwrap();
    fs::write(dir.join(WANXIANG_GRAMMAR_FILE), b"fixture").unwrap();
    assert!(!wanxiang_grammar_enabled(&dir));

    for content in [
        "schema:\n  schema_id: wanxiang\n",
        "grammar:\n  language: ''\n",
        "grammar:\n  language: null\n",
        "grammar:\n  language: 42\n",
        "grammar: [invalid yaml",
    ] {
        fs::write(dir.join("build/wanxiang.schema.yaml"), content).unwrap();
        assert!(!wanxiang_grammar_enabled(&dir), "{content}");
    }

    enable_wanxiang(&dir);
    assert_eq!(wanxiang_grammar_enabled(&dir), !cfg!(target_os = "ios"));
    fs::remove_file(dir.join(WANXIANG_GRAMMAR_FILE)).unwrap();
    assert!(!wanxiang_grammar_enabled(&dir));
    fs::create_dir(dir.join(WANXIANG_GRAMMAR_FILE)).unwrap();
    assert!(!wanxiang_grammar_enabled(&dir));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
#[cfg(not(target_os = "ios"))]
fn neutral_grammar_appends_without_changing_existing_bytes() {
    let dir = scratch_dir();
    enable_wanxiang(&dir);
    let active = "patch:\n  schema_list:\n    - schema: wanxiang\n    - schema: keydo\n    - schema: xmjd\n    - schema: scalar\n    - schema: other_grammar\n    - schema: not_deployed\n";
    fs::write(dir.join("default.custom.yaml"), active).unwrap();
    let untouched = [
        (
            "wanxiang.schema.yaml",
            "grammar:\n  language: user-source\n",
        ),
        (
            "keydo.schema.yaml",
            "# User source\ntranslator:\n  dictionary: keydo\n",
        ),
        (
            "keydo.custom.yaml",
            "patch:\n  translator/enable_sentence: true\n",
        ),
        (
            "xmjd.custom.yaml",
            "patch:\n  grammar/non_collocation_penalty: -42\n",
        ),
        (
            "build/inactive.schema.yaml",
            "# Inactive\ntranslator:\n  dictionary: inactive\n",
        ),
        (
            "build/other_grammar.schema.yaml",
            "# Keep this grammar\ngrammar:\n  language: 'other-language' # Enabled\n  non_collocation_penalty: -9\n",
        ),
        (
            "build/scalar.schema.yaml",
            "grammar: false\n",
        ),
    ];
    for (path, content) in untouched {
        fs::write(dir.join(path), content).unwrap();
    }
    let originals = [
        (
            "keydo",
            "# Compiled scalars must stay byte-identical\nspeller:\n  alphabet: 0123456789\n  select_keys: 0x10\n  version: 1.10\n  quoted: 'on'\n  unquoted: off\n  double_quoted: \"0123\" # Keep comment\n  grammar: nested\n# grammar: comment",
        ),
        ("xmjd", "# Preserve CRLF\r\ntranslator:\r\n  dictionary: xmjd\r\n"),
    ];
    for (id, content) in originals {
        fs::write(dir.join(format!("build/{id}.schema.yaml")), content).unwrap();
    }
    let wanxiang = fs::read(dir.join("build/wanxiang.schema.yaml")).unwrap();

    patch_neutral_grammar(&dir);

    for (id, content) in originals {
        let bytes = fs::read(dir.join(format!("build/{id}.schema.yaml"))).unwrap();
        assert_eq!(
            bytes,
            format!(
                "{content}{}grammar:\n  language: \"\"\n  non_collocation_penalty: -13.815510557964274\n",
                if content.ends_with('\n') {
                    "# KeyTao neutral grammar\n"
                } else {
                    "\n# KeyTao neutral grammar (added newline)\n"
                }
            ).as_bytes(),
            "{id}"
        );
    }
    assert_eq!(
        fs::read_to_string(dir.join("default.custom.yaml")).unwrap(),
        active
    );
    assert_eq!(
        fs::read(dir.join("build/wanxiang.schema.yaml")).unwrap(),
        wanxiang
    );
    for (path, content) in untouched {
        assert_eq!(
            fs::read_to_string(dir.join(path)).unwrap(),
            content,
            "{path}"
        );
    }
    let patched: Vec<_> = ["keydo", "xmjd"]
        .map(|id| dir.join(format!("build/{id}.schema.yaml")))
        .into_iter()
        .map(|path| {
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    patch_neutral_grammar(&dir);
    for (path, bytes) in patched {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    super::update_neutral_grammar(&dir, false);
    for (id, content) in originals {
        assert_eq!(
            fs::read(dir.join(format!("build/{id}.schema.yaml"))).unwrap(),
            content.as_bytes()
        );
    }
    fs::remove_dir_all(dir).unwrap();
}

#[test]
#[cfg(not(target_os = "ios"))]
fn neutral_grammar_removes_legacy_separator_without_matching_nested_keys() {
    let dir = scratch_dir();
    enable_wanxiang(&dir);
    fs::write(
        dir.join("default.custom.yaml"),
        "patch:\n  schema_list:\n    - schema: keydo\n",
    )
    .unwrap();
    let path = dir.join("build/keydo.schema.yaml");
    let grammar = "grammar:\n  language: \"\"\n  non_collocation_penalty: -13.815510557964274\n";
    fs::write(&path, format!("schema: {{schema_id: keydo}}\n{grammar}")).unwrap();
    super::update_neutral_grammar(&dir, false);
    assert_eq!(fs::read(&path).unwrap(), b"schema: {schema_id: keydo}");
    let nested = format!("my{grammar}");
    fs::write(&path, &nested).unwrap();
    super::update_neutral_grammar(&dir, false);
    assert_eq!(fs::read(&path).unwrap(), nested.as_bytes());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
#[cfg(not(target_os = "ios"))]
fn neutral_grammar_leaves_existing_top_level_grammar_untouched() {
    let dir = scratch_dir();
    enable_wanxiang(&dir);
    fs::write(
        dir.join("default.custom.yaml"),
        "patch:\n  schema_list:\n    - schema: keydo\n",
    )
    .unwrap();
    for content in [
        "grammar:\n  language: ''\n  non_collocation_penalty: -12\n  collocation_max_length: 4\n",
        "grammar:\n  language: \"\"\n  non_collocation_penalty: -13.815510557964274\n",
        "grammar: {language: another-language}\n",
        "grammar: {}\n",
        "grammar: [invalid yaml\n",
    ] {
        let path = dir.join("build/keydo.schema.yaml");
        fs::write(&path, content).unwrap();
        patch_neutral_grammar(&dir);
        assert_eq!(fs::read(&path).unwrap(), content.as_bytes());
    }
    fs::remove_dir_all(dir).unwrap();
}

#[test]
#[cfg(all(unix, not(target_os = "ios")))]
fn neutral_grammar_skips_io_and_symlink_errors_and_patches_other_schemas() {
    let dir = scratch_dir();
    enable_wanxiang(&dir);
    fs::write(
        dir.join("default.custom.yaml"),
        "patch:\n  schema_list:\n    - schema: bad_io\n    - schema: bad_link\n    - schema: healthy\n",
    )
    .unwrap();
    fs::create_dir(dir.join("build/bad_io.schema.yaml")).unwrap();
    let original = b"# Never follow the link\nspeller:\n  alphabet: 0123456789\n";
    let source = dir.join("source.schema.yaml");
    fs::write(&source, original).unwrap();
    let link = dir.join("build/bad_link.schema.yaml");
    std::os::unix::fs::symlink(&source, &link).unwrap();
    let healthy = dir.join("build/healthy.schema.yaml");
    fs::write(&healthy, original).unwrap();

    patch_neutral_grammar(&dir);

    assert!(fs::symlink_metadata(link).unwrap().file_type().is_symlink());
    assert_eq!(fs::read(source).unwrap(), original);
    let bytes = fs::read(healthy).unwrap();
    assert!(bytes.starts_with(original));
    assert!(bytes.ends_with(b"  non_collocation_penalty: -13.815510557964274\n"));
    fs::remove_dir_all(dir).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn real_librime_setup_and_reinitialize_continue_after_grammar_errors() {
    const CHILD: &str = "KEYTAO_GRAMMAR_ERRORS_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let dir = scratch_dir();
        enable_wanxiang(&dir);
        fs::write(
            dir.join("default.custom.yaml"),
            "patch:\n  schema_list:\n    - schema: bad_io\n    - schema: bad_link\n    - schema: healthy\n",
        )
        .unwrap();
        fs::create_dir(dir.join("build/bad_io.schema.yaml")).unwrap();
        let link = dir.join("build/bad_link.schema.yaml");
        std::os::unix::fs::symlink(dir.join("missing.schema.yaml"), &link).unwrap();
        let healthy = dir.join("build/healthy.schema.yaml");
        let original = b"schema:\n  schema_id: healthy\n";
        fs::write(&healthy, original).unwrap();
        let root = dir.to_string_lossy().into_owned();
        super::setup_only(root.clone(), root.clone()).expect("grammar errors must not abort setup");
        assert!(fs::read(&healthy).unwrap().len() > original.len());
        fs::write(&healthy, original).unwrap();
        super::reinitialize(root.clone(), root.clone())
            .expect("grammar errors must not abort reinitialize");
        assert!(fs::read(&healthy).unwrap().len() > original.len());

        // A symlinked build directory must also be refused without stopping setup.
        fs::rename(dir.join("build"), dir.join("compiled")).unwrap();
        std::os::unix::fs::symlink(dir.join("compiled"), dir.join("build")).unwrap();
        fs::write(&healthy, original).unwrap();
        super::reinitialize(root.clone(), root).expect("build symlink must not abort reinitialize");
        assert_eq!(fs::read(&healthy).unwrap(), original);
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "grammar_tests::real_librime_setup_and_reinitialize_continue_after_grammar_errors",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "librime child failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn neutral_grammar_is_noop_without_enabled_model() {
    let dir = scratch_dir();
    fs::write(
        dir.join("default.custom.yaml"),
        "patch:\n  schema_list:\n    - schema: keydo\n",
    )
    .unwrap();
    let content = "grammar: [invalid yaml";
    fs::write(dir.join("build/keydo.schema.yaml"), content).unwrap();
    fs::write(
        dir.join("build/wanxiang.schema.yaml"),
        "grammar:\n  language: wanxiang-lts-zh-hans\n",
    )
    .unwrap();
    patch_neutral_grammar(&dir);
    assert_eq!(
        fs::read_to_string(dir.join("build/keydo.schema.yaml")).unwrap(),
        content
    );
    fs::write(dir.join(WANXIANG_GRAMMAR_FILE), b"fixture").unwrap();
    fs::write(
        dir.join("build/wanxiang.schema.yaml"),
        "grammar:\n  language: ''\n",
    )
    .unwrap();
    patch_neutral_grammar(&dir);
    assert_eq!(
        fs::read_to_string(dir.join("build/keydo.schema.yaml")).unwrap(),
        content
    );
    fs::remove_dir_all(dir).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn real_librime_grammar_redeploy_preserves_builds_and_rebuilds_changed_inputs() {
    const CHILD: &str = "KEYTAO_GRAMMAR_REDEPLOY_CHILD";
    if let Some(root) = std::env::var_os(CHILD) {
        let dir = PathBuf::from(root);
        enable_wanxiang(&dir);
        let sources = [
            ("default.yaml", "config_version: '1'\nschema_list:\n  - schema: keytao\n"),
            ("default.custom.yaml", "patch:\n  schema_list:\n    - schema: keytao\n"),
            ("keytao.custom.yaml", "patch:\n  menu/page_size: 7\n"),
            ("keytao.schema.yaml", "schema:\n  schema_id: keytao\n  name: Grammar Redeploy\n  version: '1'\n\
                engine:\n  processors: [speller, selector, express_editor]\n  \
                segmentors: [abc_segmentor, fallback_segmentor]\n  translators: [table_translator]\n\
                speller:\n  alphabet: abcde\ntranslator:\n  dictionary: keytao\n  enable_user_dict: false\n"),
            ("keytao.dict.yaml", "---\nname: keytao\nversion: '1'\nsort: by_weight\n\
                use_preset_vocabulary: false\n...\n甲\ta\t100\n乙\tb\t100\n"),
        ];
        let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        for (name, content) in sources {
            let path = dir.join(name);
            fs::write(&path, content).unwrap();
            fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(old))
                .unwrap();
        }
        let user_files = [
            ("personal.user.dict.yaml", "# User dictionary\n"),
            ("personal.userdb/data", "user database fixture"),
            ("personal.custom.yaml", "patch: {menu/page_size: 9}\n"),
        ];
        fs::create_dir_all(dir.join("personal.userdb")).unwrap();
        for (name, content) in user_files {
            fs::write(dir.join(name), content).unwrap();
        }
        let deploy = || {
            let started = std::time::Instant::now();
            super::deploy(
                dir.to_string_lossy().into_owned(),
                dir.to_string_lossy().into_owned(),
            )
            .expect("deploy with grammar enabled");
            started.elapsed()
        };
        let first = deploy();
        let binaries = ["keytao.table.bin", "keytao.prism.bin", "keytao.reverse.bin"]
            .map(|name| dir.join("build").join(name));
        for path in &binaries {
            fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(old))
                .unwrap();
        }
        let second = deploy();
        println!("grammar deploy: first={first:?}, unchanged={second:?}");
        for path in &binaries {
            assert_eq!(
                fs::metadata(path).unwrap().modified().unwrap(),
                old,
                "unchanged grammar deploy rebuilt {}",
                path.display()
            );
        }
        let compiled = || -> Value {
            serde_yaml::from_slice(&fs::read(dir.join("build/keytao.schema.yaml")).unwrap())
                .unwrap()
        };
        assert_eq!(
            compiled()["grammar"]["non_collocation_penalty"].as_f64(),
            Some(NEUTRAL_GRAMMAR_PENALTY)
        );
        fs::write(
            dir.join("keytao.dict.yaml"),
            format!("{}丙\tc\t100\n", sources[4].1),
        )
        .unwrap();
        deploy();
        for path in &binaries {
            assert_ne!(
                fs::metadata(path).unwrap().modified().unwrap(),
                old,
                "dictionary change did not rebuild {}",
                path.display()
            );
        }
        fs::File::options()
            .write(true)
            .open(&binaries[1])
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(old))
            .unwrap();
        fs::write(
            dir.join("keytao.custom.yaml"),
            "patch:\n  menu/page_size: 8\n",
        )
        .unwrap();
        fs::File::options()
            .write(true)
            .open(dir.join("keytao.custom.yaml"))
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(old))
            .unwrap();
        deploy();
        assert_ne!(fs::metadata(&binaries[1]).unwrap().modified().unwrap(), old);
        assert_eq!(compiled()["menu"]["page_size"].as_u64(), Some(8));
        let runtime = super::ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned());
        runtime.reload_without_deploy().unwrap();
        let session = runtime.create_session().unwrap();
        session.select_schema("keytao").unwrap();
        session.process_key_result(u32::from(b'a'), 0).unwrap();
        let state = session.state();
        assert_eq!(state.page_size, 8);
        assert_eq!(state.candidates.first().unwrap().text, "甲");
        drop(session);
        runtime.shutdown().unwrap();
        for (name, content) in user_files.into_iter().chain(sources[..2].iter().copied()) {
            assert_eq!(
                fs::read(dir.join(name)).unwrap(),
                content.as_bytes(),
                "{name}"
            );
        }
        assert_eq!(
            fs::read_to_string(dir.join("keytao.custom.yaml")).unwrap(),
            "patch:\n  menu/page_size: 8\n"
        );
        let write_old = |name: &str, content: &str| {
            let path = dir.join(name);
            fs::write(&path, content).unwrap();
            fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(old))
                .unwrap();
        };
        write_old(
            "keytao_dep.schema.yaml",
            &sources[3]
                .1
                .replace("schema_id: keytao", "schema_id: keytao_dep"),
        );
        write_old("keytao_dep.custom.yaml", "patch: {menu/page_size: 3}\n");
        write_old(
            "default.custom.yaml",
            "patch: {schema_list: [{schema: keytao}, {schema: keytao_dep}]}\n",
        );
        deploy();
        write_old("default.custom.yaml", sources[1].1);
        deploy();
        write_old("keytao_dep.custom.yaml", "patch: {menu/page_size: 9}\n");
        write_old(
            "keytao.custom.yaml",
            "patch: {menu/page_size: 8, schema/dependencies: [keytao_dep]}\n",
        );
        deploy();
        let dependency: Value =
            serde_yaml::from_slice(&fs::read(dir.join("build/keytao_dep.schema.yaml")).unwrap())
                .unwrap();
        assert_eq!(
            dependency["menu"]["page_size"].as_u64(),
            Some(9),
            "a newly referenced inactive dependency must compile its same-mtime custom patch"
        );
        return;
    }
    let dir = scratch_dir();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "grammar_tests::real_librime_grammar_redeploy_preserves_builds_and_rebuilds_changed_inputs", "--nocapture"])
        .env(CHILD, &dir)
        .output().unwrap();
    println!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success(),
        "librime child failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(dir).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn real_librime_non_grammar_sentence_parity() {
    const CHILD_MODE: &str = "KEYTAO_GRAMMAR_PARITY_CHILD";
    const RESULT: &str = "KEYTAO_GRAMMAR_SENTENCE=";
    if let Ok(mode) = std::env::var(CHILD_MODE) {
        let dir = scratch_dir();
        fs::write(
            dir.join("default.yaml"),
            "config_version: '1'\nschema_list:\n  - schema: keytao_parity\n  - schema: wanxiang\n",
        )
        .unwrap();
        fs::write(
            dir.join("default.custom.yaml"),
            "patch:\n  schema_list:\n    - schema: keytao_parity\n    - schema: wanxiang\n",
        )
        .unwrap();
        let schema =
            "schema:\n  schema_id: keytao_parity\n  name: Sentence Parity\n  version: '1'\n\
            engine:\n  processors: [speller, selector, express_editor]\n  \
            segmentors: [abc_segmentor, fallback_segmentor]\n  translators: [table_translator]\n\
            speller:\n  alphabet: abcd\n  max_code_length: 8\n\
            translator:\n  dictionary: keytao_parity\n  enable_sentence: true\n  \
            enable_completion: false\n  enable_user_dict: false\n";
        fs::write(dir.join("keytao_parity.schema.yaml"), schema).unwrap();
        fs::write(
            dir.join("wanxiang.schema.yaml"),
            schema.replace("schema_id: keytao_parity", "schema_id: wanxiang")
                + "grammar:\n  language: wanxiang-lts-zh-hans\n",
        )
        .unwrap();
        fs::write(
            dir.join("keytao_parity.dict.yaml"),
            "---\nname: keytao_parity\nversion: '1'\nsort: by_weight\n\
            use_preset_vocabulary: false\n...\n甲\ta\t1e9\n乙\tb\t1e9\n丙\tc\t1e9\n丁\td\t1e9\n替代\tabc\t1\n",
        )
        .unwrap();
        let runtime = super::ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned());
        runtime
            .init()
            .expect("deploy two tiny schemas without octagram");
        if mode == "octagram" {
            // Only the presence gate needs this fixture; no Wanxiang session reads it.
            fs::write(dir.join(WANXIANG_GRAMMAR_FILE), b"fixture").unwrap();
            runtime
                .reload_without_deploy()
                .expect("register octagram and patch the generated non-grammar schema");
            let compiled: Value = serde_yaml::from_slice(
                &fs::read(dir.join("build/keytao_parity.schema.yaml")).unwrap(),
            )
            .unwrap();
            assert_eq!(compiled["grammar"]["language"].as_str(), Some(""));
            assert_eq!(
                compiled["grammar"]["non_collocation_penalty"].as_f64(),
                Some(NEUTRAL_GRAMMAR_PENALTY)
            );
        }
        unsafe {
            let api = librime_sys::rime_get_api();
            let registered = !((*api).find_module.unwrap())(c"octagram".as_ptr()).is_null();
            assert_eq!(registered, mode == "octagram");
        }
        let session = runtime
            .create_session()
            .expect("create the non-grammar session");
        session.select_schema("keytao_parity").unwrap();
        for key in b"abcd" {
            session.process_key_result(u32::from(*key), 0).unwrap();
        }
        let state = session.state();
        let top = &state.candidates.first().expect("assembled sentence").text;
        // -12 favors four words; librime's original penalty favors two here.
        assert_eq!(top, "替代丁");
        println!("{RESULT}{top}");
        // glog keeps open handles until this isolated child exits.
        return;
    }

    let plugin = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor/librime/macos-universal/lib/rime-plugins/librime-octagram.dylib");
    if !plugin.is_file() {
        eprintln!("skipping real-librime parity: vendored octagram is absent");
        return;
    }
    // Registration and librime's registry survive finalize; isolate both module sets.
    let sentences: Vec<_> = ["plain", "octagram"]
        .into_iter()
        .map(|mode| {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "grammar_tests::real_librime_non_grammar_sentence_parity",
                    "--nocapture",
                ])
                .env(CHILD_MODE, mode)
                .output()
                .expect("run isolated librime comparison");
            assert!(
                output.status.success(),
                "{mode} child failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .find_map(|line| line.strip_prefix(RESULT))
                .expect("child reported its sentence candidate")
                .to_owned()
        })
        .collect();
    // This fixture has a unique best sentence; BeamSearch tie order is not covered.
    assert_eq!(sentences[0], sentences[1]);
}

#[test]
#[cfg(not(target_os = "ios"))]
fn module_gate_accepts_any_deployed_grammar_schema_and_custom_model() {
    let dir = scratch_dir();
    fs::write(
        dir.join("default.custom.yaml"),
        "patch:\n  schema_list:\n    - schema: my_pinyin\n    - schema: wanxiang\n    - schema: plain\n",
    )
    .unwrap();
    fs::write(
        dir.join("build/my_pinyin.schema.yaml"),
        "schema:\n  schema_id: my_pinyin\ngrammar:\n  language: custom-zh-hans\n",
    )
    .unwrap();
    fs::write(
        dir.join("build/wanxiang.schema.yaml"),
        "schema:\n  schema_id: wanxiang\ntranslator:\n  dictionary: wanxiang\n",
    )
    .unwrap();
    fs::write(
        dir.join("build/plain.schema.yaml"),
        "schema:\n  schema_id: plain\ntranslator:\n  dictionary: plain\n",
    )
    .unwrap();

    // No custom-zh-hans.gram yet -> disabled.
    assert!(!wanxiang_grammar_enabled(&dir));

    // Path traversal language is rejected even if a file exists.
    fs::write(
        dir.join("build/bad.schema.yaml"),
        "schema:\n  schema_id: bad\ngrammar:\n  language: ../escape\n",
    )
    .unwrap();
    assert!(!wanxiang_grammar_enabled(&dir));

    // Once custom-zh-hans.gram exists, octagram gate enables without wanxiang.schema.yaml grammar.
    fs::write(dir.join("custom-zh-hans.gram"), b"fixture").unwrap();
    assert!(wanxiang_grammar_enabled(&dir));

    patch_neutral_grammar(&dir);
    let my_pinyin = fs::read_to_string(dir.join("build/my_pinyin.schema.yaml")).unwrap();
    assert!(my_pinyin.contains("language: custom-zh-hans"));
    assert!(!my_pinyin.contains("non_collocation_penalty"));

    // Both sibling non-grammar schemas (including wanxiang when it has no grammar) get neutral penalty.
    for id in ["wanxiang", "plain"] {
        let compiled: Value = serde_yaml::from_slice(
            &fs::read(dir.join(format!("build/{id}.schema.yaml"))).unwrap(),
        )
        .unwrap();
        assert_eq!(compiled["grammar"]["language"].as_str(), Some(""), "{id}");
        assert_eq!(
            compiled["grammar"]["non_collocation_penalty"].as_f64(),
            Some(NEUTRAL_GRAMMAR_PENALTY),
            "{id}"
        );
    }

    fs::remove_dir_all(dir).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn real_librime_generic_grammar_loads_octagram_and_marks_taiji_comment() {
    const CHILD_MODE: &str = "KEYTAO_GENERIC_GRAMMAR_TAIJI_CHILD";
    const GRAMMAR_HINT_LUA: &str =
        include_str!("../../keytao-app-core/src/keytao_grammar_hint.lua");

    if std::env::var(CHILD_MODE).as_deref() == Ok("run") {
        let dir = scratch_dir();
        fs::create_dir_all(dir.join("lua")).unwrap();
        fs::write(dir.join("lua/keytao_grammar_hint.lua"), GRAMMAR_HINT_LUA).unwrap();
        fs::write(
            dir.join("lua/test_comment.lua"),
            r#"local M = {}
function M.func(translation, env)
  local preset = env.engine.schema.config:get_string("test_preset_comment") or ""
  for cand in translation:iter() do
    local genuine = (cand.get_genuine and cand:get_genuine()) or cand
    if genuine.type == "sentence" and preset ~= "" then
      cand.comment = preset
    end
    yield(cand)
  end
end
return M
"#,
        )
        .unwrap();

        let schema_list = "\
schema_list:
  - schema: custom_gram
  - schema: custom_gram_comment
  - schema: custom_gram_dedup
  - schema: custom_gram_off
  - schema: plain_sibling
";
        fs::write(
            dir.join("default.yaml"),
            format!("config_version: '1'\n{schema_list}"),
        )
        .unwrap();
        fs::write(
            dir.join("default.custom.yaml"),
            format!("patch:\n  {}", schema_list.replace('\n', "\n  ")),
        )
        .unwrap();

        let script_schema = |id: &str, filters: &str, extra: &str| {
            format!(
                "schema:\n  schema_id: {id}\n  name: {id}\n  version: '1'\n\
engine:\n  processors: [speller, selector, express_editor]\n  \
segmentors: [abc_segmentor, fallback_segmentor]\n  \
translators: [script_translator]\n  \
filters: [{filters}]\n\
speller:\n  alphabet: abcd\n  delimiter: \" '\"\n\
translator:\n  dictionary: custom_gram\n  prism: custom_gram\n  enable_sentence: true\n  \
enable_user_dict: false\n{extra}"
            )
        };
        let table_schema = |id: &str| {
            format!(
                "schema:\n  schema_id: {id}\n  name: {id}\n  version: '1'\n\
engine:\n  processors: [speller, selector, express_editor]\n  \
segmentors: [abc_segmentor, fallback_segmentor]\n  \
translators: [table_translator]\n  \
filters: [uniquifier]\n\
speller:\n  alphabet: abcd\n  max_code_length: 8\n\
translator:\n  dictionary: custom_gram\n  enable_sentence: true\n  \
enable_completion: false\n  enable_user_dict: false\n"
            )
        };

        fs::write(
            dir.join("custom_gram.schema.yaml"),
            script_schema(
                "custom_gram",
                "uniquifier, lua_filter@*keytao_grammar_hint",
                "grammar:\n  language: custom-model\n",
            ),
        )
        .unwrap();
        fs::write(
            dir.join("custom_gram_comment.schema.yaml"),
            script_schema(
                "custom_gram_comment",
                "uniquifier, lua_filter@*test_comment, lua_filter@*keytao_grammar_hint",
                "grammar:\n  language: custom-model\ntest_preset_comment: '注释'\n",
            ),
        )
        .unwrap();
        fs::write(
            dir.join("custom_gram_dedup.schema.yaml"),
            script_schema(
                "custom_gram_dedup",
                "uniquifier, lua_filter@*test_comment, lua_filter@*keytao_grammar_hint",
                "grammar:\n  language: custom-model\ntest_preset_comment: '已有☯'\n",
            ),
        )
        .unwrap();
        fs::write(
            dir.join("custom_gram_off.schema.yaml"),
            script_schema(
                "custom_gram_off",
                "uniquifier, lua_filter@*keytao_grammar_hint",
                "grammar:\n  language: custom-model\nkeytao:\n  grammar_hint: false\n",
            ),
        )
        .unwrap();
        fs::write(
            dir.join("plain_sibling.schema.yaml"),
            table_schema("plain_sibling"),
        )
        .unwrap();
        fs::write(
            dir.join("custom_gram.dict.yaml"),
            "---\nname: custom_gram\nversion: '1'\nsort: by_weight\n\
use_preset_vocabulary: false\n...\n甲\ta\t1e9\n乙\tb\t1e9\n丙\tc\t1e9\n丁\td\t1e9\n替代\tabc\t1\n",
        )
        .unwrap();

        fs::write(
            dir.join(super::ACTIVE_SCHEME_MANIFEST),
            serde_json::to_vec(&serde_json::json!({
                "root": dir.canonicalize().unwrap(),
                "scheme_key": "local-custom-gram",
                "manifest": {
                    "custom_gram.schema.yaml": "fixture",
                    "custom_gram_comment.schema.yaml": "fixture",
                    "custom_gram_dedup.schema.yaml": "fixture",
                    "custom_gram_off.schema.yaml": "fixture",
                    "plain_sibling.schema.yaml": "fixture",
                }
            }))
            .unwrap(),
        )
        .unwrap();

        // Deploy first without .gram, then place custom-model.gram (non-wanxiang!) and reload.
        let runtime = super::ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned());
        runtime.init().expect("deploy schemas");
        fs::write(dir.join("custom-model.gram"), b"fixture").unwrap();
        runtime
            .reload_without_deploy()
            .expect("reload with custom .gram model");

        unsafe {
            let api = librime_sys::rime_get_api();
            let has_octagram = !((*api).find_module.unwrap())(c"octagram".as_ptr()).is_null();
            let has_lua = !((*api).find_module.unwrap())(c"lua".as_ptr()).is_null();
            assert!(has_octagram, "octagram module should load for non-wanxiang .gram");
            assert!(has_lua, "lua module should load");
        }

        let sibling: Value = serde_yaml::from_slice(
            &fs::read(dir.join("build/plain_sibling.schema.yaml")).unwrap(),
        )
        .unwrap();
        assert_eq!(sibling["grammar"]["language"].as_str(), Some(""));
        assert_eq!(
            sibling["grammar"]["non_collocation_penalty"].as_f64(),
            Some(NEUTRAL_GRAMMAR_PENALTY)
        );

        let session = runtime.create_session().expect("create session");
        let type_and_top = |schema_id: &str, keys: &[u8]| -> (String, String) {
            session.select_schema(schema_id).unwrap();
            for key in keys {
                session.process_key_result(u32::from(*key), 0).unwrap();
            }
            let state = session.state();
            let top = state
                .candidates
                .first()
                .expect("expected at least one candidate");
            let pair = (top.text.clone(), top.comment.clone().unwrap_or_default());
            // Escape (0xff1b) clears composition before the next check.
            session.process_key_result(0xff1b, 0).unwrap();
            pair
        };

        // 1. Multi-segment script_translator sentence gets monochrome Taiji comment, text stays clean.
        // With octagram active (default -12 penalty), 4-word sentence "甲乙丙丁" wins.
        let (text, comment) = type_and_top("custom_gram", b"abcd");
        assert_eq!(text, "甲乙丙丁");
        assert_eq!(comment, "☯");

        // 2. Single dictionary word (non-sentence) does NOT get Taiji comment.
        let (text, comment) = type_and_top("custom_gram", b"a");
        assert_eq!(text, "甲");
        assert_eq!(comment, "");

        // 3. Existing comment is preserved with space + Taiji.
        let (text, comment) = type_and_top("custom_gram_comment", b"abcd");
        assert_eq!(text, "甲乙丙丁");
        assert_eq!(comment, "注释 ☯");

        // 4. Existing Taiji is not duplicated.
        let (text, comment) = type_and_top("custom_gram_dedup", b"abcd");
        assert_eq!(text, "甲乙丙丁");
        assert_eq!(comment, "已有☯");

        // 5. Explicit opt-out keytao/grammar_hint: false suppresses Taiji on script_translator.
        let (text, comment) = type_and_top("custom_gram_off", b"abcd");
        assert_eq!(text, "甲乙丙丁");
        assert_eq!(comment, "");

        // 6. Sibling non-grammar table_translator schema keeps neutral penalty ("替代丁").
        let (text, _comment) = type_and_top("plain_sibling", b"abcd");
        assert_eq!(text, "替代丁");

        return;
    }

    let plugins_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor/librime/macos-universal/lib/rime-plugins");
    if !plugins_dir.join("librime-octagram.dylib").is_file()
        || !plugins_dir.join("librime-lua.dylib").is_file()
    {
        eprintln!("skipping real-librime generic grammar + taiji test: vendored plugins absent");
        return;
    }

    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "grammar_tests::real_librime_generic_grammar_loads_octagram_and_marks_taiji_comment",
            "--nocapture",
        ])
        .env(CHILD_MODE, "run")
        .output()
        .expect("run isolated librime generic grammar + taiji test");
    assert!(
        output.status.success(),
        "child failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
