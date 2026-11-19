#![cfg(unix)]

use keytao_theme::theme_store::{
    apply_theme, delete_theme, export_theme, import_theme, list_themes, preview_theme_import,
};
use serde_json::json;
use std::{fs, os::unix::fs::symlink, time::SystemTime};

#[test]
fn rejected_library_entry_does_not_block_listing_or_unrelated_imports() {
    let directory = std::env::temp_dir().join(format!(
        "keytao-rejected-library-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let theme_path = directory.join("theme.yaml");
    let source = |id| json!({"version":3,"meta":{"id":id,"name":id}}).to_string();
    import_theme(&theme_path, &source("my.good"), false).unwrap();
    let rejected = source("my.rejected");
    let target = directory.join("rejected.yaml");
    let link = directory.join("themes/my.rejected.yaml");
    fs::write(&target, &rejected).unwrap();
    symlink(&target, &link).unwrap();

    let themes = list_themes(&theme_path).unwrap();
    assert_eq!(themes.len(), 2);
    assert!(themes.iter().any(|theme| theme.id == "my.good"));
    let incoming = source("my.incoming");
    assert!(
        !preview_theme_import(&theme_path, &incoming)
            .unwrap()
            .conflict
    );
    assert_eq!(
        import_theme(&theme_path, &incoming, false).unwrap().id,
        "my.incoming"
    );

    assert!(apply_theme(&theme_path, "my.rejected").is_err());
    assert!(export_theme(&theme_path, "my.rejected").is_err());
    assert!(delete_theme(&theme_path, "my.rejected").is_err());
    assert!(preview_theme_import(&theme_path, &rejected).is_err());
    assert!(import_theme(&theme_path, &rejected, true).is_err());
    assert!(fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read_to_string(&target).unwrap(), rejected);
    fs::remove_dir_all(directory).unwrap();
}
