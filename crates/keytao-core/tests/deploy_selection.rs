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
