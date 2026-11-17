#[path = "support/smoke_fixture.rs"]
mod smoke_fixture;

use keytao_core::{deployed_schema_id_from_dir, ImeRuntime};

#[test]
fn deployment_preserves_selection_or_uses_the_first_package_schema() {
    let dir = smoke_fixture::scratch_dir("deploy-selection");
    smoke_fixture::write(&dir).unwrap();
    let template =
        std::fs::read_to_string(dir.join(format!("{}.schema.yaml", smoke_fixture::SCHEMA_ID)))
            .unwrap();
    for id in ["keytao", "keytao-dz", "keytao-bj"] {
        std::fs::write(
            dir.join(format!("{id}.schema.yaml")),
            template.replace(
                &format!("schema_id: {}", smoke_fixture::SCHEMA_ID),
                &format!("schema_id: {id}"),
            ),
        )
        .unwrap();
    }
    let configure = |schemas: &[&str]| {
        let list = schemas
            .iter()
            .map(|id| format!("    - schema: {id}\n"))
            .collect::<String>();
        std::fs::write(
            dir.join("default.custom.yaml"),
            format!("patch:\n  schema_list:\n{list}"),
        )
        .unwrap();
    };
    configure(&["keytao", "keytao-dz", "keytao-bj"]);
    let runtime = ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned());
    runtime.init().unwrap();
    assert_eq!(deployed_schema_id_from_dir(&dir).as_deref(), Some("keytao"));

    let session = runtime.create_session().unwrap();
    session.select_schema("keytao-dz").unwrap();
    drop(session);
    runtime.reload().unwrap();
    assert_eq!(
        deployed_schema_id_from_dir(&dir).as_deref(),
        Some("keytao-dz")
    );

    configure(&["keytao", "keytao-bj"]);
    runtime.reload().unwrap();
    assert_eq!(deployed_schema_id_from_dir(&dir).as_deref(), Some("keytao"));
    std::fs::remove_dir_all(dir).unwrap();
}

fn broken_schema_fixture(name: &str, schemas: &[&str]) -> std::path::PathBuf {
    let dir = smoke_fixture::scratch_dir(name);
    smoke_fixture::write(&dir).unwrap();
    for (file, content) in [
        (
            "keytao_good.schema.yaml",
            include_str!("fixtures/skip_broken/good.schema.yaml"),
        ),
        (
            "keytao_broken.schema.yaml",
            include_str!("fixtures/skip_broken/broken.schema.yaml"),
        ),
        (
            "symbols.yaml",
            include_str!("fixtures/skip_broken/symbols.yaml"),
        ),
    ] {
        std::fs::write(dir.join(file), content).unwrap();
    }
    let list = schemas
        .iter()
        .map(|id| format!("    - schema: {id}\n"))
        .collect::<String>();
    std::fs::write(
        dir.join("default.custom.yaml"),
        format!("patch:\n  schema_list:\n{list}"),
    )
    .unwrap();
    dir
}

fn librime_schema_ids() -> Vec<String> {
    unsafe {
        let api = librime_sys::rime_get_api();
        let mut list: librime_sys::RimeSchemaList = std::mem::zeroed();
        assert_ne!((*api).get_schema_list.unwrap()(&mut list), 0);
        let ids = std::slice::from_raw_parts(list.list, list.size)
            .iter()
            .map(|schema| {
                std::ffi::CStr::from_ptr(schema.schema_id)
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        (*api).free_schema_list.unwrap()(&mut list);
        ids
    }
}

fn run_in_fresh_process(test_name: &str) -> bool {
    if std::env::var("KEYTAO_BROKEN_SCHEMA_CHILD").as_deref() == Ok(test_name) {
        return false;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test_name, "--nocapture"])
        .env("KEYTAO_BROKEN_SCHEMA_CHILD", test_name)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

#[test]
fn deployment_skips_a_broken_listed_schema_and_selects_the_first_compiled_schema() {
    if run_in_fresh_process(
        "deployment_skips_a_broken_listed_schema_and_selects_the_first_compiled_schema",
    ) {
        return;
    }
    let dir = broken_schema_fixture("deploy-skip-broken", &["keytao_broken", "keytao_good"]);
    std::fs::write(
        dir.join("user.yaml"),
        "var:\n  previously_selected_schema: keytao_broken\n",
    )
    .unwrap();
    let runtime = ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned());
    runtime
        .init()
        .expect("one compiled listed schema is enough");
    assert_eq!(
        keytao_core::deployment_warning(&dir).as_deref(),
        Some("方案 keytao_broken 编译失败，已跳过：copy on write failed; incompatible node type: commit")
    );
    assert!(keytao_core::schema_install_state(&dir).deployed);
    assert_eq!(
        deployed_schema_id_from_dir(&dir).as_deref(),
        Some("keytao_good")
    );
    assert!(!dir.join("build/keytao_broken.schema.yaml").is_file());
    assert_eq!(
        keytao_core::parse_schema_list(
            &std::fs::read_to_string(dir.join("build/default.yaml")).unwrap()
        ),
        ["keytao_good"]
    );
    let session = runtime.create_session().unwrap();
    assert_eq!(session.current_schema().unwrap().id, "keytao_good");
    assert_eq!(
        session
            .list_schemas()
            .unwrap()
            .into_iter()
            .map(|schema| schema.id)
            .collect::<Vec<_>>(),
        ["keytao_good"]
    );
    assert_eq!(librime_schema_ids(), ["keytao_good"]);
    session.select_schema("keytao_good").unwrap();
    for character in smoke_fixture::CODE.chars() {
        session.process_key_result(character as u32, 0).unwrap();
    }
    assert_eq!(session.state().candidates[0].text, smoke_fixture::WORDS[0]);
    drop(session);

    let repaired = include_str!("fixtures/skip_broken/good.schema.yaml")
        .replace("keytao_good", "keytao_broken")
        .replace("version: '1'", "version: '2'");
    std::fs::write(dir.join("keytao_broken.schema.yaml"), repaired).unwrap();
    runtime
        .reload()
        .expect("repairing the source restores the skipped schema");
    assert_eq!(keytao_core::deployment_warning(&dir), None);
    assert!(dir.join("build/keytao_broken.schema.yaml").is_file());
    assert_eq!(
        keytao_core::parse_schema_list(
            &std::fs::read_to_string(dir.join("build/default.yaml")).unwrap()
        ),
        ["keytao_broken", "keytao_good"]
    );
    let session = runtime.create_session().unwrap();
    assert_eq!(session.current_schema().unwrap().id, "keytao_good");
    assert_eq!(librime_schema_ids(), ["keytao_broken", "keytao_good"]);
    drop(session);

    let broken_again = include_str!("fixtures/skip_broken/broken.schema.yaml")
        .replace("version: '1'", "version: '3'");
    std::fs::write(dir.join("keytao_broken.schema.yaml"), broken_again).unwrap();
    runtime
        .reload()
        .expect("an old compiled schema must not hide a new compilation failure");
    assert!(!dir.join("build/keytao_broken.schema.yaml").is_file());
    assert_eq!(
        keytao_core::deployment_warning(&dir).as_deref(),
        Some("方案 keytao_broken 编译失败，已跳过：copy on write failed; incompatible node type: commit")
    );
    let session = runtime.create_session().unwrap();
    assert_eq!(session.current_schema().unwrap().id, "keytao_good");
    assert_eq!(librime_schema_ids(), ["keytao_good"]);
    drop(session);

    std::fs::remove_file(dir.join("keytao_broken.schema.yaml")).unwrap();
    runtime
        .reload()
        .expect("a missing listed source must not block the working schema");
    let state = keytao_core::schema_install_state(&dir);
    assert!(state.installed);
    assert!(state.deployed);
    let warning = keytao_core::deployment_warning(&dir).unwrap();
    assert!(warning.contains("keytao_broken"), "{warning}");
    assert!(warning.contains("missing schema source"), "{warning}");
    let session = runtime.create_session().unwrap();
    assert_eq!(session.current_schema().unwrap().id, "keytao_good");
    assert_eq!(
        session
            .list_schemas()
            .unwrap()
            .into_iter()
            .map(|schema| schema.id)
            .collect::<Vec<_>>(),
        ["keytao_good"]
    );
    assert_eq!(librime_schema_ids(), ["keytao_good"]);
    drop(session);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deployment_fails_when_all_listed_schemas_are_broken() {
    if run_in_fresh_process("deployment_fails_when_all_listed_schemas_are_broken") {
        return;
    }
    let dir = broken_schema_fixture("deploy-all-broken", &["keytao_broken"]);
    let runtime = ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned());
    let error = runtime
        .init()
        .expect_err("no compiled listed schema must fail");
    assert!(error.contains("keytao_broken"), "{error}");
    let state = keytao_core::schema_install_state(&dir);
    assert!(
        state.installed,
        "a failed deployment must remain available to retry"
    );
    assert!(!state.deployed);
    let repaired = include_str!("fixtures/skip_broken/good.schema.yaml")
        .replace("keytao_good", "keytao_broken")
        .replace("version: '1'", "version: '2'");
    std::fs::write(dir.join("keytao_broken.schema.yaml"), repaired).unwrap();
    runtime
        .reload()
        .expect("retry the repaired all-broken package");
    assert!(keytao_core::schema_install_state(&dir).deployed);
    assert_eq!(keytao_core::deployment_warning(&dir), None);
    let session = runtime.create_session().unwrap();
    assert_eq!(session.current_schema().unwrap().id, "keytao_broken");
    assert_eq!(librime_schema_ids(), ["keytao_broken"]);
    drop(session);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deployment_still_fails_for_a_broken_non_listed_dependency() {
    if run_in_fresh_process("deployment_still_fails_for_a_broken_non_listed_dependency") {
        return;
    }
    let dir = broken_schema_fixture("deploy-broken-dependency", &["keytao_good"]);
    let source = std::fs::read_to_string(dir.join("keytao_good.schema.yaml"))
        .unwrap()
        .replace(
            "  version: '1'",
            "  version: '1'\n  dependencies: [keytao_broken]",
        );
    std::fs::write(dir.join("keytao_good.schema.yaml"), source).unwrap();
    let runtime = ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned());
    let error = runtime
        .init()
        .expect_err("non-listed dependencies remain required");
    assert!(error.contains("keytao_broken"), "{error}");
    assert!(!keytao_core::schema_install_state(&dir).deployed);
    std::fs::remove_dir_all(dir).unwrap();
}
