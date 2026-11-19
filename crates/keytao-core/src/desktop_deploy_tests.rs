use super::*;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn desktop_deploy_rebuilds_only_changed_schemas_and_falls_back_safely() {
    const CHILD: &str = "KEYTAO_DESKTOP_SCOPE_TEST_ROOT";
    const NAME: &str = "desktop::desktop_deploy_tests::desktop_deploy_rebuilds_only_changed_schemas_and_falls_back_safely";
    let Some(root) = std::env::var_os(CHILD) else {
        let root = std::env::temp_dir().join(format!(
            "keytao-desktop-scope-{}-{}",
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
        fs::remove_dir_all(root).unwrap();
        return;
    };
    let root = PathBuf::from(root);
    let configure = |ids: &[&str]| {
        let ids = ids
            .iter()
            .map(|id| format!("{{schema: {id}}}"))
            .collect::<Vec<_>>()
            .join(", ");
        fs::write(
            root.join("default.custom.yaml"),
            format!("patch: {{schema_list: [{ids}]}}\n"),
        )
        .unwrap();
    };
    let add = |id: &str| {
        let schema = include_str!("../tests/fixtures/skip_broken/good.schema.yaml")
            .replace("keytao_good", id)
            .replace("keytao_smoke", id);
        fs::write(root.join(format!("{id}.schema.yaml")), schema).unwrap();
        fs::write(root.join(format!("{id}.dict.yaml")), format!("---\nname: {id}\nversion: '1'\nsort: by_weight\nuse_preset_vocabulary: false\n...\n甲\taa\t100\n")).unwrap();
    };
    fs::write(root.join("default.yaml"), "config_version: '1'\n").unwrap();
    fs::write(
        root.join("symbols.yaml"),
        include_bytes!("../tests/fixtures/skip_broken/symbols.yaml"),
    )
    .unwrap();
    for id in ["keytao_one", "keytao_two"] {
        add(id);
    }
    configure(&["keytao_one", "keytao_two"]);
    fs::create_dir_all(root.join("sync")).unwrap();
    fs::create_dir_all(root.join("personal.userdb")).unwrap();
    let user_files = [
        "personal.user.dict.yaml",
        "personal.userdb/data",
        "personal.custom.yaml",
        "sync/data",
    ];
    for name in user_files {
        fs::write(root.join(name), "keep user data").unwrap();
    }
    fs::write(
        root.join("installation.yaml"),
        format!(
            "installation_id: keep-installation\nsync_dir: '{}'\n",
            root.join("sync").display()
        ),
    )
    .unwrap();
    let deploy = || {
        deploy_for_process(
            root.to_string_lossy().into_owned(),
            root.to_string_lossy().into_owned(),
            DeployProcess::DeployChild,
        )
        .unwrap()
    };
    assert!(desktop_deploy_plan(&root, &root).is_none());
    deploy();
    let old = UNIX_EPOCH + Duration::from_secs(1_000_000);
    let stamp = |id: &str| {
        for name in [
            format!("build/{id}.schema.yaml"),
            format!("build/{id}.prism.bin"),
        ] {
            fs::File::options()
                .write(true)
                .open(root.join(name))
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
    };
    let modified = |id: &str| {
        fs::metadata(root.join(format!("build/{id}.schema.yaml")))
            .unwrap()
            .modified()
            .unwrap()
    };
    for id in ["keytao_one", "keytao_two"] {
        stamp(id);
    }
    assert_eq!(desktop_deploy_plan(&root, &root), Some(vec![]));
    let default_before = fs::metadata(root.join("build/default.yaml"))
        .unwrap()
        .modified()
        .unwrap();
    deploy();
    assert_eq!(
        fs::metadata(root.join("build/default.yaml"))
            .unwrap()
            .modified()
            .unwrap(),
        default_before
    );
    assert_eq!(modified("keytao_one"), old);
    assert_eq!(modified("keytao_two"), old);
    assert_eq!(
        validation_schema_list(
            &["keytao_one".into(), "keytao_two".into()],
            &[],
            Some("keytao_one")
        ),
        ["keytao_one"]
    );
    fs::write(
        root.join("keytao_one.custom.yaml"),
        "patch: {menu/page_size: 7}\n",
    )
    .unwrap();
    assert_eq!(
        desktop_deploy_plan(&root, &root),
        Some(vec!["keytao_one".into()])
    );
    deploy();
    assert_ne!(modified("keytao_one"), old);
    assert_eq!(modified("keytao_two"), old);
    stamp("keytao_one");
    add("keytao_three");
    configure(&["keytao_one", "keytao_two", "keytao_three"]);
    assert_eq!(
        desktop_deploy_plan(&root, &root),
        Some(vec!["keytao_three".into()])
    );
    deploy();
    assert_eq!(modified("keytao_one"), old);
    assert_eq!(modified("keytao_two"), old);
    fs::remove_file(root.join("build/keytao_two.prism.bin")).unwrap();
    assert_eq!(
        desktop_deploy_plan(&root, &root),
        Some(vec!["keytao_two".into()])
    );
    deploy();
    configure(&["keytao_one", "keytao_two"]);
    assert!(desktop_deploy_plan(&root, &root).is_none());
    deploy();
    assert_ne!(modified("keytao_one"), old);
    fs::remove_file(root.join("build/.keytao-schema-inputs.json")).unwrap();
    assert!(desktop_deploy_plan(&root, &root).is_none());
    deploy();
    fs::write(root.join("build/.keytao-deploy-failed"), "repair").unwrap();
    assert!(desktop_deploy_plan(&root, &root).is_none());
    deploy();
    // A newly selected package must not certify an inactive dependency's stale build.
    add("keytao_inactive");
    fs::write(
        root.join("keytao_inactive.custom.yaml"),
        "patch: {menu/page_size: 3}\n",
    )
    .unwrap();
    configure(&["keytao_one", "keytao_two", "keytao_inactive"]);
    deploy();
    configure(&["keytao_one", "keytao_two"]);
    deploy();
    fs::write(
        root.join("keytao_inactive.custom.yaml"),
        "patch: {menu/page_size: 9}\n",
    )
    .unwrap();
    add("keytao_new");
    fs::write(
        root.join("keytao_new.custom.yaml"),
        "patch: {schema/dependencies: [keytao_inactive]}\n",
    )
    .unwrap();
    configure(&["keytao_one", "keytao_two", "keytao_new"]);
    assert_eq!(
        desktop_deploy_plan(&root, &root),
        Some(vec!["keytao_new".into()])
    );
    deploy();
    let dependency: Value =
        serde_yaml::from_slice(&fs::read(root.join("build/keytao_inactive.schema.yaml")).unwrap())
            .unwrap();
    assert_eq!(dependency["menu"]["page_size"].as_u64(), Some(9));
    let installation: Value =
        serde_yaml::from_slice(&fs::read(root.join("installation.yaml")).unwrap()).unwrap();
    assert_eq!(
        installation["installation_id"].as_str(),
        Some("keep-installation")
    );
    assert_eq!(
        installation["sync_dir"].as_str(),
        Some(root.join("sync").to_str().unwrap())
    );
    for name in user_files {
        assert_eq!(
            fs::read_to_string(root.join(name)).unwrap(),
            "keep user data"
        );
    }
    finalize_rime().unwrap();
}
