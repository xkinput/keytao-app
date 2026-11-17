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
                if content.ends_with('\n') { "" } else { "\n" }
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
