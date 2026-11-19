use keytao_theme::theme_store::{read_theme_for_update, refresh_current_theme};
use serde_json::Value;
use std::fs;

#[test]
fn empty_sections_do_not_block_refresh_or_create_manual_backups() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("null-sections-{}-{nonce}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let theme_path = dir.join("theme.yaml");
    let yaml = "ui:\npanel:\nlight:\n  font:\ndark:\n  candidate:\n  mobile:\n    keyboard:\n      key:\n      keyTypes:\n        function:\n      toolbar:\n    candidateBar:\nnight:\nkeyboard:\n  floating:\n    portrait:\n  layers:\nresolvedUi:\n  light:\n  dark:\n";
    fs::write(&theme_path, yaml).unwrap();

    let base: Value = serde_json::to_value(read_theme_for_update(&theme_path).unwrap()).unwrap();
    for path in [
        "/ui",
        "/panel",
        "/light/font",
        "/dark/candidate",
        "/dark/mobile/keyboard/key",
        "/dark/mobile/keyboard/keyTypes/function",
        "/dark/mobile/keyboard/toolbar",
        "/dark/mobile/candidateBar",
        "/night",
        "/keyboard/floating/portrait",
        "/keyboard/layers",
        "/resolvedUi/light",
        "/resolvedUi/dark",
    ] {
        assert_eq!(base.pointer(path), Some(&serde_json::json!({})), "{path}");
    }
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    assert_eq!(fs::read_to_string(&theme_path).unwrap(), yaml);
    assert!(refresh_current_theme(&theme_path).unwrap());
    assert!(!refresh_current_theme(&theme_path).unwrap());

    let source_path = dir.join("themes/my.current.yaml");
    let mut source: Value =
        serde_yaml::from_str(&fs::read_to_string(&source_path).unwrap()).unwrap();
    source["common"]["accentColor"] = serde_json::json!("#123456FF");
    fs::write(&source_path, serde_yaml::to_string(&source).unwrap()).unwrap();
    let mut compiled: Value =
        serde_yaml::from_str(&fs::read_to_string(&theme_path).unwrap()).unwrap();
    compiled["ui"] = Value::Null;
    fs::write(&theme_path, serde_yaml::to_string(&compiled).unwrap()).unwrap();
    assert!(refresh_current_theme(&theme_path).unwrap());
    assert!(!refresh_current_theme(&theme_path).unwrap());
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 3);
    assert_eq!(fs::read_dir(dir.join("themes")).unwrap().count(), 1);
    fs::remove_dir_all(dir).unwrap();
}
