use keytao_core::{list_schema_switches, set_schema_switch};

#[test]
fn compiled_schema_settings_work_without_runtime_source_or_deployment_marker() {
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("keytao-config-only-{suffix}"));
    std::fs::create_dir_all(dir.join("build")).unwrap();
    std::fs::write(
        dir.join("build/default.yaml"),
        "schema_list:\n  - schema: keytao_config_only\n",
    )
    .unwrap();
    let schema = "schema:\n  schema_id: keytao_config_only\nswitches:\n  - name: mars\n    states: [普通, 火星文]\n";
    let compiled = dir.join("build/keytao_config_only.schema.yaml");
    std::fs::write(&compiled, schema).unwrap();
    let switches = list_schema_switches(&dir).unwrap();
    assert_eq!(switches.len(), 1);
    assert_eq!(switches[0].value, 0);
    let switches = set_schema_switch(&dir, "keytao_config_only", "mars", 1).unwrap();
    assert_eq!(switches[0].value, 1);
    let request = keytao_core::read_schema_switch_request(&dir).unwrap();
    assert_eq!(request.generation, 1);
    assert_eq!(request.options.get("mars"), Some(&true));
    assert!(!dir.join("user.yaml").exists());
    assert_eq!(std::fs::read_to_string(compiled).unwrap(), schema);
    assert!(!dir.join("default.custom.yaml").exists());
    assert!(!dir.join("keytao_config_only.schema.yaml").exists());
    std::fs::remove_dir_all(dir).unwrap();
}
