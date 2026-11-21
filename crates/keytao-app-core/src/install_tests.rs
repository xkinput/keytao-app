use crate::{
    AppEnv, Core, CoreEvent, EventSink, Platform, install::export_scheme,
    package_store::PackageStore,
};
use std::{
    fs,
    io::Write,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct ProgressSink(Mutex<Vec<(String, u32, String)>>);

impl EventSink for ProgressSink {
    fn emit(&self, event: CoreEvent) {
        if let CoreEvent::InstallProgress(progress) = event {
            self.0
                .lock()
                .unwrap()
                .push((progress.stage, progress.percent, progress.message));
        }
    }
}

#[test]
fn protected_user_file_matcher_only_matches_top_level_user_files() {
    for name in [
        "keytao.user.dict.yaml",
        "keytao.user.yaml",
        "user.yaml",
        "user_dict.txt",
        "keytao.user.remove.txt",
        "xmjd6_user.txt",
        ".keytao-sync-base-42.json",
    ] {
        assert!(crate::install::is_protected_user_file(name), "{name}");
        assert!(crate::packages::excluded_from_manifest(name), "{name}");
    }
    for name in [
        "default.custom.yaml",
        "default-custom.yaml",
        "xmjd6.custom.yaml",
        "lua/x.custom.yaml",
        "lua/keytao.user.dict.yaml",
        "User.yaml",
        "xmjd6_USER.txt",
        "keytao.dict.yaml",
        "lua/.keytao-sync-base-42.json",
        ".keytao-sync-base-42.yaml",
    ] {
        assert!(!crate::install::is_protected_user_file(name), "{name}");
    }
}

#[test]
fn sync_base_and_markers_are_never_written_by_package_install() {
    let dir = tempfile::tempdir().unwrap();
    for relative in [".keytao-sync-base-42.json", "keytao.user.remove.txt"] {
        let path = dir.path().join(relative);
        let mut logs = Vec::new();

        for existing in [
            Some(b"[\"saved\"]".as_slice()),
            Some(b"\xff\xfe".as_slice()),
            None,
        ] {
            if let Some(content) = existing {
                fs::write(&path, content).unwrap();
            } else {
                fs::remove_file(&path).unwrap();
            }
            crate::install::write_export_file(dir.path(), relative, b"[]", &mut logs).unwrap();
            assert_eq!(fs::read(&path).ok().as_deref(), existing);
        }
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}

#[test]
fn protected_file_install_keeps_utf8_bom_dictionary() {
    let dir = tempfile::tempdir().unwrap();
    let relative = "keytao.user.dict.yaml";
    let user = b"\xef\xbb\xbf---\nname: k\n...\nword\tabcd\n";
    fs::write(dir.path().join(relative), user).unwrap();
    let mut logs = Vec::new();

    crate::install::write_export_file(dir.path(), relative, b"entries: []\n", &mut logs).unwrap();

    assert_eq!(fs::read(dir.path().join(relative)).unwrap(), user);
    assert_eq!(logs, ["[KEPT] keytao.user.dict.yaml"]);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn protected_file_install_keeps_duplicate_key_settings() {
    let dir = tempfile::tempdir().unwrap();
    let relative = "x.user.yaml";
    let user = b"patch:\n  a: 1\n  a: 2\n";
    fs::write(dir.path().join(relative), user).unwrap();
    let mut logs = Vec::new();

    crate::install::write_export_file(dir.path(), relative, b"patch: {}\n", &mut logs).unwrap();

    assert_eq!(fs::read(dir.path().join(relative)).unwrap(), user);
    assert_eq!(logs, ["[KEPT] x.user.yaml"]);
}

#[test]
fn protected_file_install_keeps_utf8_bom_settings() {
    let dir = tempfile::tempdir().unwrap();
    let relative = "x.user.yaml";
    let user = b"\xef\xbb\xbfcustomization:\n  v: 1\npatch:\n  a: 1\n";
    fs::write(dir.path().join(relative), user).unwrap();
    let mut logs = Vec::new();

    crate::install::write_export_file(dir.path(), relative, b"patch: {}\n", &mut logs).unwrap();

    assert_eq!(fs::read(dir.path().join(relative)).unwrap(), user);
    assert_eq!(logs, ["[KEPT] x.user.yaml"]);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn protected_file_install_decides_missing_valid_and_broken_with_backup() {
    let dir = tempfile::tempdir().unwrap();
    let relative = "keytao.user.dict.yaml";
    let path = dir.path().join(relative);
    let package = b"entries: []\n";
    let mut logs = Vec::new();

    crate::install::write_export_file(dir.path(), relative, package, &mut logs).unwrap();
    assert_eq!(fs::read(&path).unwrap(), package);

    let valid = b"---\nname: keytao.user\nversion: \"1\"\n...\nuser_word\tabcd\t1\n";
    fs::write(&path, valid).unwrap();
    logs.clear();
    crate::install::write_export_file(dir.path(), relative, package, &mut logs).unwrap();
    assert_eq!(fs::read(&path).unwrap(), valid);
    assert_eq!(logs, ["[KEPT] keytao.user.dict.yaml"]);

    for broken in [b"entries: [\n".as_slice(), b"\xff\xfe".as_slice()] {
        fs::write(&path, broken).unwrap();
        logs.clear();
        crate::install::write_export_file(dir.path(), relative, package, &mut logs).unwrap();
        assert_eq!(fs::read(&path).unwrap(), package);
        let backup = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("keytao.user.dict.yaml.broken-")
            })
            .expect("Broken content must be backed up");
        assert_eq!(fs::read(&backup).unwrap(), broken);
        let backup_name = backup.file_name().unwrap().to_str().unwrap();
        assert!(
            backup_name
                .strip_prefix("keytao.user.dict.yaml.broken-")
                .unwrap()
                .parse::<u64>()
                .is_ok()
        );
        assert!(
            logs.iter()
                .any(|line| line.starts_with("[BROKEN] keytao.user.dict.yaml: ")
                    && line.ends_with(&format!("(backup: {backup_name})")))
        );
        fs::remove_file(backup).unwrap();
    }

    let relative = format!("{}.user.yaml", "x".repeat(240));
    let path = dir.path().join(&relative);
    let broken = b"patch: [\n";
    fs::write(&path, broken).unwrap();
    logs.clear();
    crate::install::write_export_file(dir.path(), &relative, package, &mut logs).unwrap();
    assert_eq!(fs::read(&path).unwrap(), broken);
    assert!(
        logs.iter()
            .any(|line| line.starts_with("[BROKEN] ") && line.contains("left untouched"))
    );
}

#[cfg(target_os = "windows")]
#[test]
fn package_install_replaces_readonly_files_and_keeps_writable_broken_backups() {
    let directory = tempfile::tempdir().unwrap();
    for relative in ["example.schema.yaml", "example.user.yaml"] {
        let path = directory.path().join(relative);
        fs::write(&path, b"patch: [\n").unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).unwrap();

        let mut logs = Vec::new();
        crate::install::write_export_file(directory.path(), relative, b"patch: {}\n", &mut logs)
            .unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"patch: {}\n");
        assert!(!fs::metadata(&path).unwrap().permissions().readonly());
    }
    let backup = fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.to_string_lossy().contains(".broken-"))
        .unwrap();
    assert_eq!(fs::read(&backup).unwrap(), b"patch: [\n");
    assert!(!fs::metadata(backup).unwrap().permissions().readonly());
}

#[test]
fn directory_export_keeps_user_dictionary_and_verifies_it() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("rime");
    fs::create_dir_all(&dest).unwrap();
    let user = b"\xef\xbb\xbf---\nname: keytao.user\nversion: \"1\"\n...\nuser_word\tabcd\t1\n";
    fs::write(dest.join("keytao.user.dict.yaml"), user).unwrap();
    let zip_path = dir.path().join("package.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(&zip_path).unwrap());
    zip.start_file(
        "keytao.user.dict.yaml",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    zip.write_all(b"name: keytao.user\nentries: []\n").unwrap();
    zip.finish().unwrap();
    let core = Core::new(
        AppEnv {
            data_dir: dir.path().join("state"),
            cache_dir: dir.path().join("cache"),
            resource_dir: dir.path().join("resources"),
            app_version: "test".into(),
            platform: Platform::MacOs,
        },
        Arc::new(ProgressSink::default()),
    )
    .unwrap();
    let result = export_scheme(&core, zip_path.to_string_lossy().into_owned(), &dest).unwrap();
    assert_eq!(fs::read(dest.join("keytao.user.dict.yaml")).unwrap(), user);
    assert!(
        result
            .logs
            .iter()
            .any(|line| line == "[KEPT] keytao.user.dict.yaml")
    );
    assert!(
        result
            .verify
            .iter()
            .any(|entry| entry.path == "keytao.user.dict.yaml" && entry.ok)
    );
    assert!(core.state().installed_packages.is_empty());
    assert!(!PackageStore::new(&core).package_path("keytao").exists());
}

#[test]
fn package_custom_yaml_replaces_the_installed_copy() {
    let dir = tempfile::tempdir().unwrap();
    let relative = "xmjd6.custom.yaml";
    fs::write(
        dir.path().join(relative),
        b"patch:\n  switches/@4/reset: 1\n",
    )
    .unwrap();
    let mut logs = Vec::new();

    crate::install::write_export_file(dir.path(), relative, b"patch: {}\n", &mut logs).unwrap();

    assert_eq!(fs::read(dir.path().join(relative)).unwrap(), b"patch: {}\n");
}
