use super::*;
use std::fs;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[test]
fn listed_schema_preserves_unchanged_builds_and_rejects_stale_outputs() {
    const CHILD: &str = "KEYTAO_LISTED_SCHEMA_TEST_ROOT";
    const NAME: &str = "desktop::listed_schema_tests::listed_schema_preserves_unchanged_builds_and_rejects_stale_outputs";
    let Some(root) = std::env::var_os(CHILD) else {
        let root = std::env::temp_dir().join(format!(
            "keytao-listed-schema-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME, "--nocapture"])
            .env(CHILD, &root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        print!("{}", String::from_utf8_lossy(&output.stdout));
        fs::remove_dir_all(root).unwrap();
        return;
    };
    let root = PathBuf::from(root);
    let shared = root.join("rime-data");
    fs::create_dir_all(&shared).unwrap();
    let old = UNIX_EPOCH + Duration::from_secs(1_000_000);
    let write = |name: &str, bytes: &[u8]| {
        let path = root.join(name);
        fs::write(&path, bytes).unwrap();
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(old)
            .unwrap();
    };
    write(
        "rime-data/default.yaml",
        b"config_version: '1'\nschema_list: [{schema: keytao_good}]\n",
    );
    write(
        "default.custom.yaml",
        b"patch: {schema_list: [{schema: keytao_good}]}\n",
    );
    write(
        "keytao_good.schema.yaml",
        format!(
            "{}\nmenu:\n  __include: common:/menu\npunctuator:\n  import_preset: symbols\n",
            include_str!("../tests/fixtures/skip_broken/good.schema.yaml")
                .replace("speller:\n", "speller:\n  __patch: extra:/patch\n")
        )
        .as_bytes(),
    );
    write("rime-data/common.yaml", b"menu: {page_size: 6}\n");
    write(
        "rime-data/symbols.yaml",
        include_bytes!("../tests/fixtures/skip_broken/symbols.yaml"),
    );
    assert!(!root.join("default.yaml").exists());
    write(
        "extra.yaml",
        b"patch: {alphabet: abcdefghijklmnopqrstuvwxyz}\n",
    );
    write("keytao_smoke.dict.yaml", "---\nname: keytao_smoke\nversion: '1'\nsort: by_weight\nuse_preset_vocabulary: false\n...\n甲\taa\t100\n".as_bytes());
    fs::create_dir_all(root.join("personal.userdb")).unwrap();
    fs::create_dir_all(root.join("sync")).unwrap();
    let user_files = [
        "personal.user.dict.yaml",
        "personal.userdb/data",
        "personal.custom.yaml",
        "sync/data",
    ];
    for name in user_files {
        write(name, b"keep user data");
    }
    setup_only_for_process(
        root.to_string_lossy().into_owned(),
        shared.to_string_lossy().into_owned(),
        DeployProcess::AndroidDeployer,
    )
    .unwrap();
    // Android setup loads these task modules before its per-schema worker runs.
    unsafe {
        (*rime_get_api()).deployer_initialize.unwrap()(std::ptr::null_mut());
    }
    let deploy = |reuse: bool| {
        let start = Instant::now();
        assert!(deploy_listed_schema_with_reuse(
            &root,
            "keytao_good",
            reuse.then_some(shared.as_path())
        )
        .unwrap());
        crate::record_schema_input_fingerprints(&root, &["keytao_good".into()]).unwrap();
        assert!(schema_inputs::schema_input_is_current(&root, "keytao_good", &shared));
        start.elapsed()
    };
    let first = deploy(true);
    let outputs = [
        "build/keytao_good.schema.yaml",
        "build/keytao_smoke.prism.bin",
    ];
    for name in outputs {
        fs::File::options()
            .write(true)
            .open(root.join(name))
            .unwrap()
            .set_modified(old)
            .unwrap();
    }
    for _ in 0..2 {
        let elapsed = deploy(true);
        println!("listed schema: first={first:?}, unchanged={elapsed:?}");
        for name in outputs {
            assert_eq!(
                fs::metadata(root.join(name)).unwrap().modified().unwrap(),
                old,
                "unchanged deploy rewrote {name}"
            );
        }
    }
    // Same-mtime edits must bypass librime's timestamp-only config check.
    for (name, bytes) in [
        ("rime-data/common.yaml", b"menu: {page_size: 5}\n".as_slice()),
        (
            "rime-data/symbols.yaml",
            b"config_version: '1'\npunctuator:\n  half_shape:\n    '/': [ / ]\n".as_slice(),
        ),
        (
            "extra.yaml",
            b"patch: {alphabet: abcdefghijklmnopqrstuvwxyz0123456789}\n".as_slice(),
        ),
        (
            "keytao_good.custom.yaml",
            b"patch: {menu/page_size: 7}\n".as_slice(),
        ),
        (
            "rime-data/default.yaml",
            b"config_version: '1'\nmenu: {page_size: 8}\nschema_list: [{schema: keytao_good}]\n"
                .as_slice(),
        ),
        (
            "default.custom.yaml",
            b"patch: {schema_list: [{schema: keytao_good}], menu/page_size: 9}\n".as_slice(),
        ),
    ] {
        write(name, bytes);
        let elapsed = deploy(true);
        println!("listed schema: changed {name}={elapsed:?}");
        assert_ne!(
            fs::metadata(root.join(outputs[0]))
                .unwrap()
                .modified()
                .unwrap(),
            old,
            "changed {name} retained compiled schema"
        );
        for output in outputs {
            fs::File::options()
                .write(true)
                .open(root.join(output))
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
    }
    let compiled: Value =
        serde_yaml::from_slice(&fs::read(root.join(outputs[0])).unwrap()).unwrap();
    assert_eq!(compiled["menu"]["page_size"].as_u64(), Some(7));
    write("keytao_smoke.dict.yaml", "---\nname: keytao_smoke\nversion: '1'\nsort: by_weight\nuse_preset_vocabulary: false\n...\n甲\taa\t100\n乙\tbb\t100\n".as_bytes());
    let elapsed = deploy(true);
    println!("listed schema: changed dictionary={elapsed:?}");
    assert_ne!(
        fs::metadata(root.join(outputs[1]))
            .unwrap()
            .modified()
            .unwrap(),
        old
    );
    fs::write(
        root.join("build/.keytao-schema-inputs.json"),
        b"invalid receipt",
    )
    .unwrap();
    fs::File::options()
        .write(true)
        .open(root.join(outputs[0]))
        .unwrap()
        .set_modified(old)
        .unwrap();
    deploy(true);
    assert_ne!(
        fs::metadata(root.join(outputs[0]))
            .unwrap()
            .modified()
            .unwrap(),
        old
    );
    for reuse in [false, true] {
        let mut timings = (0..5).map(|_| deploy(reuse)).collect::<Vec<_>>();
        timings.sort();
        println!("listed schema: reuse={reuse}, median of 5={:?}", timings[2]);
    }
    write(
        "symbols.yaml",
        include_bytes!("../tests/fixtures/skip_broken/symbols.yaml"),
    );
    fs::remove_file(root.join("keytao_good.custom.yaml")).unwrap();
    deploy(true);
    write("rime-data/keytao_good.custom.yaml", b"patch:\n  punctuator:\n    import_preset: symbols\n    half_shape:\n      '/': {commit: /}\n");
    assert!(
        !deploy_listed_schema_with_reuse(&root, "keytao_good", Some(&shared)).unwrap(),
        "a newly added shared patch must not hide its failure behind old compiled YAML"
    );
    assert!(!root.join(outputs[0]).exists());
    fs::remove_file(shared.join("keytao_good.custom.yaml")).unwrap();
    deploy(true);
    // Keep the listed id while introducing the original incompatible punctuator merge.
    let broken = include_str!("../tests/fixtures/skip_broken/broken.schema.yaml")
        .replace("keytao_broken", "keytao_good");
    write("keytao_good.schema.yaml", broken.as_bytes());
    assert!(!deploy_listed_schema_with_reuse(&root, "keytao_good", Some(&shared)).unwrap());
    assert!(!root.join(outputs[0]).exists());
    assert!(crate::deployment_warning(&root)
        .unwrap()
        .contains("keytao_good"));
    for name in user_files {
        assert_eq!(fs::read(root.join(name)).unwrap(), b"keep user data");
    }
    finalize_rime().unwrap();
}
