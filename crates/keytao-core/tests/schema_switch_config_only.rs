use keytao_core::{list_schema_switches, set_schema_switch};

#[test]
fn reset_switch_settings_ignore_saved_options_even_after_acknowledgment() {
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("keytao-reset-config-{suffix}"));
    std::fs::create_dir_all(dir.join("build")).unwrap();
    std::fs::write(
        dir.join("build/default.yaml"),
        "schema_list:\n  - schema: keytao_reset_config\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("build/keytao_reset_config.schema.yaml"),
        "schema:\n  schema_id: keytao_reset_config\nswitches:\n  - name: mars\n    reset: 0\n    states: [普通, 火星文]\n  - options: [punctuation_ascii, punctuation_cjk]\n    reset: 1\n    states: [西文标点, 中文标点]\n",
    )
    .unwrap();
    let user = b"var:\n  option:\n    mars: true\n    punctuation_ascii: true\n    punctuation_cjk: false\n";
    std::fs::write(dir.join("user.yaml"), user).unwrap();
    let switches = list_schema_switches(&dir).unwrap();
    assert_eq!(switches[0].value, 0);
    assert_eq!(switches[1].value, 1);
    set_schema_switch(&dir, "keytao_reset_config", "mars", 1).unwrap();
    set_schema_switch(&dir, "keytao_reset_config", "punctuation_ascii", 0).unwrap();
    std::fs::write(dir.join(keytao_core::SCHEMA_SWITCH_APPLIED_FILE), "2").unwrap();
    let stale = b"var:\n  option:\n    mars: false\n    punctuation_ascii: false\n    punctuation_cjk: true\n";
    std::fs::write(dir.join("user.yaml"), stale).unwrap();
    let switches = list_schema_switches(&dir).unwrap();
    assert_eq!(switches[0].value, 1);
    assert_eq!(switches[1].value, 0);
    assert_eq!(std::fs::read(dir.join("user.yaml")).unwrap(), stale);
    std::fs::remove_dir_all(dir).unwrap();
}

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
