use super::*;
use crate::{AppEnv, CoreEvent, EventSink, Platform};
use std::{io::Read, sync::Arc};

struct NullSink;
impl EventSink for NullSink {
    fn emit(&self, _: CoreEvent) {}
}

fn open_core(directory: &Path) -> Arc<Core> {
    Core::new(
        AppEnv {
            data_dir: directory.join("state"),
            cache_dir: directory.join("cache"),
            resource_dir: directory.join("resources"),
            app_version: "test".into(),
            platform: Platform::MacOs,
        },
        Arc::new(NullSink),
    )
    .unwrap()
}

fn source(directory: &Path, name: &str) -> std::path::PathBuf {
    let path = directory.join(name);
    fs::create_dir_all(&path).unwrap();
    fs::write(
        path.join("example.schema.yaml"),
        "schema:\n  schema_id: example\n  name: Example\n",
    )
    .unwrap();
    path
}

fn archive_files(core: &Core, key: &str) -> BTreeMap<String, Vec<u8>> {
    let (bytes, _) = crate::packages::stored_archive(core, key).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut files = BTreeMap::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        files.insert(file.name().into(), bytes);
    }
    files
}

#[test]
fn local_import_requires_directory_with_schema() {
    let directory = tempfile::tempdir().unwrap();
    let core = open_core(directory.path());
    let empty = directory.path().join("empty");
    fs::create_dir_all(empty.join("build")).unwrap();
    fs::write(empty.join("build/example.schema.yaml"), "built schema").unwrap();
    fs::create_dir_all(empty.join("nested")).unwrap();
    fs::write(empty.join("nested/example.schema.yaml"), "nested schema").unwrap();
    assert!(
        import_local_scheme(&core, &empty, "Empty", None)
            .unwrap_err()
            .to_string()
            .contains("未找到")
    );
    assert!(import_local_scheme(&core, &empty.join("missing"), "Missing", None).is_err());
    let file = directory.path().join("file");
    fs::write(&file, "not a directory").unwrap();
    assert!(import_local_scheme(&core, &file, "File", None).is_err());
    assert!(core.state().installed_packages.is_empty());
    let valid = source(directory.path(), "valid");
    assert!(import_local_scheme(&core, &valid, "Invalid", Some("keytao")).is_err());
    assert!(import_local_scheme(&core, &valid, "Invalid", Some("local-missing")).is_err());
    assert!(core.state().installed_packages.is_empty());
}

#[test]
fn local_import_allocates_unique_slugs_and_deterministic_content_versions() {
    let directory = tempfile::tempdir().unwrap();
    let core = open_core(directory.path());
    let path = source(directory.path(), "My Rime_Scheme");
    let first = import_local_scheme(&core, &path, "我的方案", None).unwrap();
    let second = import_local_scheme(&core, &path, "另一个方案", None).unwrap();
    assert_eq!(first, "local-my-rime-scheme");
    assert_eq!(second, "local-my-rime-scheme-2");
    let (first_bytes, first_meta) = crate::packages::stored_archive(&core, &first).unwrap();
    let (second_bytes, second_meta) = crate::packages::stored_archive(&core, &second).unwrap();
    assert_eq!(first_bytes, second_bytes);
    assert_eq!(first_meta.version, second_meta.version);
    assert_eq!(first_meta.version.as_ref().unwrap().len(), 12);
    assert_eq!(first_meta.source, "local");
    assert_eq!(first_meta.display_name.as_deref(), Some("我的方案"));
    assert!(core.state().installed_packages.contains_key(&first));
    assert!(core.state().active_scheme.is_none());
    let unicode = source(directory.path(), "中文方案");
    assert_eq!(
        import_local_scheme(&core, &unicode, "中文方案", None).unwrap(),
        "local-scheme"
    );
    let reopened = open_core(directory.path());
    assert_eq!(reopened.state().installed_packages.len(), 3);
}

#[test]
fn local_import_excludes_top_level_junk_and_recursive_private_files() {
    let directory = tempfile::tempdir().unwrap();
    let core = open_core(directory.path());
    let path = source(directory.path(), "source");
    for name in [
        "build/built.schema.yaml",
        ".git/config",
        "sync/device/data",
        "example.userdb/CURRENT",
        "nested/example.userdb/CURRENT",
        "nested/.git/config",
        "installation.yaml",
        "user.yaml",
        "example.user.yaml",
        "example.custom.yaml",
        "default.custom.yaml",
        "default-custom.yaml",
        "example.user.dict.yaml",
        "user_dict.txt",
        "example_user.txt",
        "nested/example.custom.yaml",
        "nested/example.user.dict.yaml",
        ".keytao-sync-base-example.json",
        ".DS_Store",
    ] {
        let file = path.join(name);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, "private content").unwrap();
    }
    fs::create_dir_all(path.join("lua/nested")).unwrap();
    fs::write(path.join("lua/nested/helper.lua"), "return true").unwrap();
    let nested = [
        "nested/build/data",
        "nested/sync/data",
        "nested/installation.yaml",
        "nested/user.yaml",
    ];
    for name in nested {
        fs::create_dir_all(path.join(name).parent().unwrap()).unwrap();
        fs::write(path.join(name), "package data").unwrap();
    }
    let key = import_local_scheme(&core, &path, "Example", None).unwrap();
    let files = archive_files(&core, &key);
    assert_eq!(
        files.keys().map(String::as_str).collect::<Vec<_>>(),
        vec![
            "default.custom.yaml",
            "example.schema.yaml",
            "lua/nested/helper.lua",
            "nested/build/data",
            "nested/installation.yaml",
            "nested/sync/data",
            "nested/user.yaml",
        ]
    );
    let config = std::str::from_utf8(&files["default.custom.yaml"]).unwrap();
    assert_eq!(keytao_core::parse_schema_list(config), ["example"]);
    assert!(!config.contains("private content"));
    let (_, meta) = crate::packages::stored_archive(&core, &key).unwrap();
    assert_eq!(meta.manifest.len(), 6);
    for name in nested {
        assert_eq!(files[name], b"package data");
    }
}

#[tokio::test]
async fn local_reimport_replaces_package_and_preserves_runtime_user_files() {
    let directory = tempfile::tempdir().unwrap();
    let core = open_core(directory.path());
    let path = source(directory.path(), "source");
    fs::write(path.join("obsolete.yaml"), "obsolete: true\n").unwrap();
    let key = import_local_scheme(&core, &path, "Original", None).unwrap();
    let root = directory.path().join("rime");
    fs::create_dir_all(&root).unwrap();
    crate::packages::activate_scheme(
        &core,
        &key,
        &root,
        #[cfg(target_os = "windows")]
        crate::install::finish_windows_scheme_install,
    )
    .await
    .unwrap();
    let installed = crate::scheme::check_local_schema(&core, Some(root.clone())).unwrap();
    assert!(installed.installed);
    assert!(!installed.deployed);
    assert_eq!(installed.schemas, ["example"]);
    let engine_state = keytao_core::schema_install_state(&root);
    assert!(engine_state.installed);
    assert_eq!(engine_state.schemas, ["example"]);
    fs::create_dir_all(root.join("build")).unwrap();
    fs::write(root.join("build/example.schema.yaml"), "schema: {}\n").unwrap();
    assert!(
        crate::scheme::check_local_schema(&core, Some(root.clone()))
            .unwrap()
            .deployed
    );
    let protected = [
        "example.user.yaml",
        "example.custom.yaml",
        "example.user.dict.yaml",
        "user_dict.txt",
        "example_user.txt",
    ];
    for name in protected {
        fs::write(root.join(name), "user-owned content").unwrap();
        fs::write(path.join(name), "incoming content").unwrap();
    }
    let config = "patch:\n  menu/page_size: 7\n  schema_list:\n    - schema: example\n";
    fs::write(root.join("default.custom.yaml"), config).unwrap();
    crate::deploy::record_result(&core, &root, true).unwrap();
    let previous_manifest = fs::read(
        checked_store_dir(&core)
            .unwrap()
            .join("active-manifest.json"),
    )
    .unwrap();
    let previous_version = core.state().installed_packages[&key].version.clone();
    fs::write(
        path.join("example.schema.yaml"),
        "schema:\n  schema_id: example\n  name: Updated\n",
    )
    .unwrap();
    fs::remove_file(path.join("obsolete.yaml")).unwrap();
    assert_eq!(
        import_local_scheme(&core, &path, "Updated", Some(&key)).unwrap(),
        key
    );
    assert_ne!(
        core.state().installed_packages[&key].version,
        previous_version
    );
    assert!(core.state().last_deploy.is_none());
    assert_eq!(
        fs::read(
            checked_store_dir(&core)
                .unwrap()
                .join("active-manifest.json")
        )
        .unwrap(),
        previous_manifest
    );
    for name in protected {
        assert_eq!(
            fs::read_to_string(root.join(name)).unwrap(),
            "user-owned content"
        );
    }
    crate::packages::activate_scheme(
        &core,
        &key,
        &root,
        #[cfg(target_os = "windows")]
        crate::install::finish_windows_scheme_install,
    )
    .await
    .unwrap();
    for name in protected {
        assert_eq!(
            fs::read_to_string(root.join(name)).unwrap(),
            "user-owned content"
        );
    }
    assert!(!root.join("obsolete.yaml").exists());
    assert!(
        fs::read_to_string(root.join("example.schema.yaml"))
            .unwrap()
            .contains("Updated")
    );
    assert!(
        fs::read_to_string(root.join("default.custom.yaml"))
            .unwrap()
            .contains("menu/page_size: 7")
    );
    let (_, meta) = crate::packages::stored_archive(&core, &key).unwrap();
    assert_eq!(meta.source, "local");
    assert_eq!(meta.display_name.as_deref(), Some("Updated"));
}
