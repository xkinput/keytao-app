use super::*;
use crate::{AppEnv, CoreEvent, EventSink, Platform, package_store::PackageStore};
use std::sync::Arc;

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

fn stored_files(core: &Core, key: &str) -> BTreeMap<String, Vec<u8>> {
    PackageStore::new(core)
        .read_files(key)
        .unwrap()
        .0
        .into_iter()
        .collect()
}

#[test]
fn local_import_without_defaults_lists_all_schemas_and_selects_primary_only() {
    let directory = tempfile::tempdir().unwrap();
    let core = open_core(directory.path());
    let path = source(directory.path(), "source");
    fs::write(path.join("example.schema.yaml"), "schema: {schema_id: example, name: Example, dependencies: [reverse]}").unwrap();
    fs::write(path.join("reverse.schema.yaml"), "schema: {schema_id: reverse, name: Reverse}").unwrap();
    let key = import_local_scheme(&core, &path, "Local", None).unwrap();
    let schemas = crate::package_schemas::package_schemas(&core, &key).unwrap();
    assert_eq!(
        schemas
            .iter()
            .map(|schema| (schema.id.as_str(), schema.selected))
            .collect::<Vec<_>>(),
        [("example", true), ("reverse", false)]
    );
    assert!(stored_files(&core, &key).contains_key("reverse.schema.yaml"));
}

#[test]
fn local_import_skips_protected_and_hidden_entries() {
    let directory = tempfile::tempdir().unwrap();
    let path = source(directory.path(), "source");
    for name in [
        "System Volume Information/private.yaml",
        "$RECYCLE.BIN/deleted.schema.yaml",
        "nested/sYsTeM vOlUmE iNfOrMaTiOn/private.yaml",
        "nested/$recycle.bin/deleted.yaml",
        ".hidden/private.yaml",
        "nested/.hidden.yaml",
        "app-state.json",
    ] {
        let file = path.join(name);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, b"must not import").unwrap();
    }
    let mut files = BTreeMap::new();
    collect(&path, "", &mut files, &mut 0, false).unwrap();
    assert_eq!(files.keys().collect::<Vec<_>>(), [&"example.schema.yaml"]);
}

#[test]
fn local_import_keeps_nomedia_as_a_regular_file() {
    let directory = tempfile::tempdir().unwrap();
    let core = open_core(directory.path());
    let path = source(directory.path(), "source");
    for (name, contents) in [
        (".nomedia", ""),
        ("resources/.nomedia", "marker"),
        (".git/.nomedia", "excluded"),
        ("nested/.nomedia/.nomedia", "excluded"),
        ("nested/.keytao-generated", "excluded"),
    ] {
        let file = path.join(name);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, contents).unwrap();
    }
    let key = import_local_scheme(&core, &path, "Example", None).unwrap();
    let (files, meta) = PackageStore::new(&core).read_files(&key).unwrap();
    assert_eq!(
        files.iter().map(|(path, _)| path.as_str()).collect::<Vec<_>>(),
        [".nomedia", "default.custom.yaml", "example.schema.yaml", "resources/.nomedia"]
    );
    assert!(meta.manifest.contains_key(".nomedia"));
    assert!(meta.manifest.contains_key("resources/.nomedia"));

    #[cfg(unix)]
    {
        fs::remove_file(path.join(".nomedia")).unwrap();
        std::os::unix::fs::symlink("example.schema.yaml", path.join(".nomedia")).unwrap();
        assert!(
            import_local_scheme(&core, &path, "Example", Some(&key))
                .unwrap_err()
                .to_string()
                .contains("符号链接")
        );
        assert!(stored_files(&core, &key)[".nomedia"].is_empty());
    }
}

#[test]
fn scheme_zip_keeps_nomedia_from_root_or_wrapper() {
    for (prefix, wrapper_marker, root_marker) in [
        ("", true, false),
        ("wrapper/", true, false),
        ("wrapper/", false, true),
        ("wrapper/", true, true),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("scheme.zip");
        let mut archive = zip::ZipWriter::new(fs::File::create(&archive_path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for (name, contents) in [
            ("example.schema.yaml", "schema: {schema_id: example}"),
            ("resources/.nomedia", ""),
            (".git/.nomedia", "excluded"),
            ("nested/.nomedia/.nomedia", "excluded"),
            ("nested/.keytao-generated", "excluded"),
        ] {
            archive.start_file(format!("{prefix}{name}"), options).unwrap();
            archive.write_all(contents.as_bytes()).unwrap();
        }
        if wrapper_marker {
            archive.start_file(format!("{prefix}.nomedia"), options).unwrap();
            archive.write_all(b"preferred marker").unwrap();
        }
        if root_marker {
            archive.start_file(".nomedia", options).unwrap();
            archive.write_all(b"outer marker").unwrap();
        }
        archive.finish().unwrap();

        let stage = directory.path().join("stage");
        let files = crate::generic_rime::extract_archive(&archive_path, &stage, None, 4096).unwrap();
        assert_eq!(
            files.iter().map(|file| file.path.as_str()).collect::<Vec<_>>(),
            [".nomedia", "example.schema.yaml", "resources/.nomedia"]
        );
        assert_eq!(
            fs::read_to_string(stage.join(".nomedia")).unwrap(),
            if wrapper_marker { "preferred marker" } else { "outer marker" }
        );
    }
}

#[test]
fn downloaded_import_skips_trime_assets_before_parsing_theme_customs() {
    let directory = tempfile::tempdir().unwrap();
    let path = source(directory.path(), "source");
    fs::create_dir(path.join("backgrounds")).unwrap();
    fs::write(path.join("backgrounds/image.png"), b"theme image").unwrap();
    fs::write(path.join("trime.yaml"), b"theme").unwrap();
    fs::write(path.join("theme.trime.custom.yaml"), b"\xff").unwrap();
    let mut files = BTreeMap::new();
    collect(&path, "", &mut files, &mut 0, true).unwrap();
    assert_eq!(files.keys().collect::<Vec<_>>(), [&"example.schema.yaml"]);
    assert!(path.join("theme.trime.custom.yaml").exists());
}

#[test]
fn local_import_root_error_names_operation_and_path() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing");
    let error = collect(&missing, "", &mut BTreeMap::new(), &mut 0, false)
        .unwrap_err()
        .to_string();
    assert!(error.contains("读取目录"), "{error}");
    assert!(error.contains(&missing.display().to_string()), "{error}");
}

#[test]
fn local_import_propagates_permission_denied_and_other_errors() {
    use std::io::{Error, ErrorKind};
    for path in [Path::new("source"), Path::new("source/child")] {
        for operation in ["读取目录", "读取目录项", "读取文件属性", "读取文件"] {
            for kind in [ErrorKind::PermissionDenied, ErrorKind::NotFound] {
                let message = import_io::<()>(Err(Error::from(kind)), operation, path)
                    .unwrap_err()
                    .to_string();
                assert!(message.contains(operation), "{message}");
                assert!(
                    message.contains(&std::path::absolute(path).unwrap().display().to_string()),
                    "{message}"
                );
            }
        }
    }
}

#[test]
fn local_import_windows_attributes_exclude_hidden_and_system_but_not_readonly() {
    for flags in [0x2, 0x4, 0x6, 0x13, 0x24] {
        assert!(excluded_attributes(flags, false));
    }
    for flags in [0, 0x1, 0x10, 0x20, 0x21] {
        assert!(!excluded_attributes(flags, false));
    }
    assert!(!excluded_attributes(0x2, true));
    assert!(excluded_attributes(0x6, true));
}

#[cfg(windows)]
#[test]
fn local_import_propagates_windows_access_errors() {
    for code in [5, 32, 33] {
        for (operation, path) in [
            ("读取文件", Path::new("source/locked.yaml")),
            ("读取目录", Path::new("source")),
            ("读取目录", Path::new("source/lua")),
        ] {
            let message = import_io::<()>(
                Err(std::io::Error::from_raw_os_error(code)),
                operation,
                path,
            )
            .unwrap_err()
            .to_string();
            assert!(message.contains(operation), "{message}");
            assert!(
                message.contains(&std::path::absolute(path).unwrap().display().to_string()),
                "{message}"
            );
        }
    }
}

#[cfg(windows)]
#[test]
fn local_import_skips_windows_hidden_and_system_attributes() {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_FLAGS_AND_ATTRIBUTES};
    let directory = tempfile::tempdir().unwrap();
    let path = source(directory.path(), "source");
    for (name, flags) in [("hidden.yaml", 0x2), ("system", 0x4), (".nomedia", 0x2)] {
        let entry = path.join(name);
        if name == "system" {
            fs::create_dir(&entry).unwrap();
            fs::write(entry.join("private.yaml"), b"skip").unwrap();
        } else {
            fs::write(&entry, b"skip").unwrap();
        }
        let name: Vec<u16> = entry.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe { SetFileAttributesW(windows::core::PCWSTR(name.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(flags)) }.unwrap();
    }
    let mut files = BTreeMap::new();
    collect(&path, "", &mut files, &mut 0, false).unwrap();
    assert_eq!(files.keys().collect::<Vec<_>>(), [&".nomedia", &"example.schema.yaml"]);
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
    fs::write(empty.join("unrelated.custom.yaml"), b"\xff").unwrap();
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
fn local_import_preserves_package_schema_order() {
    for config_name in [
        Some("default.custom.yaml"),
        Some("default-custom.yaml"),
        None,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let core = open_core(directory.path());
        let path = directory.path().join("keytao-backslash-test");
        fs::create_dir_all(&path).unwrap();
        for schema in ["keytao", "keytao-dz", "keytao-bj", "keytao-cx", "keytao-aa"] {
            fs::write(
                path.join(format!("{schema}.schema.yaml")),
                format!("schema:\n  schema_id: {schema}\n"),
            )
            .unwrap();
        }
        if let Some(name) = config_name {
            fs::write(
                path.join(name),
                "patch:\n  schema_list:\n    - schema: keytao\n    - schema: keytao-dz\n    - schema: keytao-bj\n    - schema: missing\n    - schema: keytao\n",
            )
            .unwrap();
        }
        let key = import_local_scheme(&core, &path, "Local", None).unwrap();
        let files = stored_files(&core, &key);
        let config = std::str::from_utf8(&files["default.custom.yaml"]).unwrap();
        let expected: &[&str] = if config_name.is_some() {
            &["keytao", "keytao-dz", "keytao-bj"]
        } else {
            &["keytao-aa", "keytao-bj", "keytao-cx", "keytao-dz", "keytao"]
        };
        assert_eq!(crate::scheme_files::parse_schema_list(config), expected);
    }
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
    let store = PackageStore::new(&core);
    let (first_files, first_meta) = store.read_files(&first).unwrap();
    let (second_files, second_meta) = store.read_files(&second).unwrap();
    assert_eq!(first_files, second_files);
    assert_eq!(first_meta.version, second_meta.version);
    assert_eq!(first_meta.version.as_ref().unwrap().len(), 12);
    assert_eq!(first_meta.source, "local");
    assert_eq!(first_meta.display_name.as_deref(), Some("我的方案"));
    assert_eq!(
        store.package_path(&first),
        store.root().join("local").join(&first)
    );
    assert!(
        store
            .package_path(&first)
            .join("files/example.schema.yaml")
            .is_file()
    );
    assert!(!store.root().join(format!("{first}.zip")).exists());
    assert!(!store.root().join(format!("{first}.json")).exists());
    let metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(store.metadata_path(&first)).unwrap()).unwrap();
    let origin = path.canonicalize().unwrap();
    assert_eq!(metadata["origin"], origin.to_str().unwrap());
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
    let files = stored_files(&core, &key);
    assert_eq!(
        files.keys().map(String::as_str).collect::<Vec<_>>(),
        vec![
            "default.custom.yaml",
            "example.custom.yaml",
            "example.schema.yaml",
            "example.user.dict.yaml",
            "example_user.txt",
            "lua/nested/helper.lua",
            "nested/build/data",
            "nested/example.user.dict.yaml",
            "nested/installation.yaml",
            "nested/sync/data",
            "nested/user.yaml",
            "user_dict.txt",
        ]
    );
    let config = std::str::from_utf8(&files["default.custom.yaml"]).unwrap();
    assert_eq!(keytao_core::parse_schema_list(config), ["example"]);
    assert!(!config.contains("private content"));
    let (_, meta) = PackageStore::new(&core).read_files(&key).unwrap();
    assert_eq!(meta.manifest.len(), 7);
    assert!(!meta.manifest.contains_key("nested/sync/data"));
    assert_eq!(files["nested/sync/data"], b"package data");
    for name in nested {
        assert_eq!(files[name], b"package data");
    }
}

#[test]
fn local_import_only_keeps_root_customs_for_package_schemas() {
    let directory = tempfile::tempdir().unwrap();
    let core = open_core(directory.path());
    let path = source(directory.path(), "source");
    for name in [
        "example.custom.yaml",
        "orphan.custom.yaml",
        "squirrel.custom.yaml",
        "weasel.custom.yaml",
        "nested/example.custom.yaml",
    ] {
        fs::create_dir_all(path.join(name).parent().unwrap()).unwrap();
        fs::write(path.join(name), "patch: {keep: true}\n").unwrap();
    }
    // Frontend configuration remains excluded even if a schema has the same name.
    fs::write(path.join("squirrel.schema.yaml"), "schema: {}\n").unwrap();
    fs::write(path.join("weasel.schema.yaml"), "schema: {}\n").unwrap();
    let key = import_local_scheme(&core, &path, "Example", None).unwrap();
    let files = stored_files(&core, &key);
    assert_eq!(files["example.custom.yaml"], b"patch: {keep: true}\n");
    for name in [
        "orphan.custom.yaml",
        "squirrel.custom.yaml",
        "weasel.custom.yaml",
        "nested/example.custom.yaml",
    ] {
        assert!(!files.contains_key(name), "unexpected custom: {name}");
    }
}

#[cfg(target_os = "macos")]
#[test]
fn local_reimport_replaces_sources_and_preserves_runtime_user_files() {
    let directory = tempfile::tempdir().unwrap();
    let core = open_core(directory.path());
    let path = source(directory.path(), "source");
    fs::write(path.join("obsolete.yaml"), "obsolete: true\n").unwrap();
    let key = import_local_scheme(&core, &path, "Original", None).unwrap();
    let root = directory.path().join("rime");
    let runtime = root.join("runtime").join(&key);
    fs::create_dir_all(&root).unwrap();
    let shared = directory.path().join("shared");
    fs::create_dir(&shared).unwrap();
    crate::runtime_materialize::materialize_all_installed_packages(&core, &root, &shared).unwrap();

    let learned = runtime.join("example.userdb");
    fs::create_dir(&learned).unwrap();
    fs::write(learned.join("words"), "learned content").unwrap();
    fs::write(runtime.join("example_user.txt"), "user-owned content").unwrap();
    let previous_version = core.state().installed_packages[&key].version.clone();

    fs::write(path.join("example.schema.yaml"), "schema: {schema_id: example, name: Updated}\n").unwrap();
    fs::remove_file(path.join("obsolete.yaml")).unwrap();
    assert_eq!(import_local_scheme(&core, &path, "Updated", Some(&key)).unwrap(), key);
    assert_ne!(core.state().installed_packages[&key].version, previous_version);
    crate::runtime_materialize::materialize_all_installed_packages(&core, &root, &shared).unwrap();

    assert_eq!(fs::read_to_string(learned.join("words")).unwrap(), "learned content");
    assert_eq!(fs::read_to_string(runtime.join("example_user.txt")).unwrap(), "user-owned content");
    let files = stored_files(&core, &key);
    assert!(!files.contains_key("obsolete.yaml"));
    assert!(std::str::from_utf8(&files["example.schema.yaml"]).unwrap().contains("Updated"));
    assert!(!root.join("example.schema.yaml").exists());
    let (_, meta) = PackageStore::new(&core).read_files(&key).unwrap();
    assert_eq!(meta.display_name.as_deref(), Some("Updated"));
    let metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(PackageStore::new(&core).metadata_path(&key)).unwrap()).unwrap();
    assert_eq!(metadata["origin"], path.canonicalize().unwrap().to_str().unwrap());
}
