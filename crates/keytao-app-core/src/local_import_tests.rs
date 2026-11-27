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
fn local_import_archive_rejects_source_growth_after_collection() {
    let directory = tempfile::tempdir().unwrap();
    let path = source(directory.path(), "source");
    let dictionary = path.join("example.dict.yaml");
    fs::write(&dictionary, b"dictionary").unwrap();
    let mut files = BTreeMap::new();
    collect(&path, "", &mut files, &mut 0, false).unwrap();
    assert!(matches!(files.get("example.dict.yaml"), Some(ImportFile::Disk(10))));
    fs::write(&dictionary, b"larger dictionary").unwrap();

    let mut archive = tempfile::tempfile_in(directory.path()).unwrap();
    let error = build_archive(&path, files, &mut archive).unwrap_err().to_string();
    assert!(error.contains("导入期间方案文件已更改"), "{error}");
    assert!(error.contains(&dictionary.display().to_string()), "{error}");
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
        let expected: &[&str] = if config_name.is_some() {
            &["keytao", "keytao-dz", "keytao-bj"]
        } else {
            &["keytao", "keytao-aa", "keytao-bj", "keytao-cx", "keytao-dz"]
        };
        // All `keytao*` variants belong to the same scheme family and stay grouped in ONE package.
        assert_eq!(key, "local-keytao-backslash-test");
        let installed: Vec<String> = core.state().installed_packages.keys().cloned().collect();
        assert_eq!(installed, vec![key.clone()]);
        let files = stored_files(&core, &key);
        let config = std::str::from_utf8(&files["default.custom.yaml"]).unwrap();
        assert_eq!(crate::scheme_files::parse_schema_list(config), expected);
    }
}

#[test]
fn local_import_groups_same_family_and_splits_distinct_families_with_selection() {
    let directory = tempfile::tempdir().unwrap();
    let core = open_core(directory.path());
    let path = directory.path().join("mixed-rime");
    fs::create_dir_all(path.join("lua")).unwrap();
    fs::write(
        path.join("keytao.schema.yaml"),
        "schema:\n  schema_id: keytao\n  name: 键道6\ntranslator:\n  dictionary: xkjd6\n",
    )
    .unwrap();
    fs::write(
        path.join("keytao-bj.schema.yaml"),
        "schema:\n  schema_id: keytao-bj\n  name: 键道6并击\ntranslator:\n  dictionary: xkjd6\n",
    )
    .unwrap();
    fs::write(
        path.join("keytao-dz.schema.yaml"),
        "schema:\n  schema_id: keytao-dz\n  name: 键道6单字\ntranslator:\n  dictionary: xkjd6dz\n",
    )
    .unwrap();
    fs::write(
        path.join("rime_ice.schema.yaml"),
        "schema:\n  schema_id: rime_ice\n  name: 雾凇拼音\ntranslator:\n  dictionary: rime_ice\n",
    )
    .unwrap();
    fs::write(
        path.join("double_pinyin_flypy.schema.yaml"),
        "schema:\n  schema_id: double_pinyin_flypy\n  name: 小鹤双拼\ntranslator:\n  dictionary: rime_ice\n",
    )
    .unwrap();
    fs::write(path.join("xkjd6.dict.yaml"), "---\nname: xkjd6\n...\n键\tj\n").unwrap();
    fs::write(path.join("xkjd6dz.dict.yaml"), "---\nname: xkjd6dz\n...\n单\td\n").unwrap();
    fs::write(path.join("rime_ice.dict.yaml"), "---\nname: rime_ice\n...\n雾\twu\n").unwrap();
    fs::write(path.join("lua/common.lua"), "return {}\n").unwrap();

    // Inspection groups keytao* into 1 family and rime_ice + double_pinyin_flypy into 1 family.
    let inspection = inspect_local_scheme(&core, &path).unwrap();
    assert_eq!(inspection.total_schemas, 5);
    assert_eq!(inspection.groups.len(), 2);
    let keytao_group = inspection
        .groups
        .iter()
        .find(|g| g.primary_schema_id == "keytao")
        .unwrap();
    assert_eq!(keytao_group.display_name, "键道6");
    assert_eq!(
        keytao_group
            .schemas
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        vec!["keytao", "keytao-bj", "keytao-dz"]
    );
    let ice_group = inspection
        .groups
        .iter()
        .find(|g| g.primary_schema_id != "keytao")
        .unwrap();
    assert_eq!(ice_group.schemas.len(), 2);

    // Selecting only keytao + keytao-bj imports ONLY the keytao family package (skipping rime_ice).
    let selected = vec!["keytao".to_owned(), "keytao-bj".to_owned()];
    let imported = import_local_scheme_selected(&core, &path, "Mixed", None, Some(&selected)).unwrap();
    assert_eq!(imported, "local-keytao");
    assert_eq!(
        core.state().installed_packages.keys().cloned().collect::<Vec<_>>(),
        vec!["local-keytao"]
    );
    let files = stored_files(&core, "local-keytao");
    assert!(files.contains_key("keytao.schema.yaml"));
    assert!(files.contains_key("keytao-bj.schema.yaml"));
    assert!(files.contains_key("keytao-dz.schema.yaml"));
    assert!(files.contains_key("xkjd6.dict.yaml"));
    assert!(files.contains_key("xkjd6dz.dict.yaml"));
    assert!(files.contains_key("lua/common.lua"));
    assert!(!files.contains_key("rime_ice.schema.yaml"));
    assert!(!files.contains_key("rime_ice.dict.yaml"));
    let config = std::str::from_utf8(&files["default.custom.yaml"]).unwrap();
    assert_eq!(
        crate::scheme_files::parse_schema_list(config),
        vec!["keytao", "keytao-bj"]
    );
}

#[test]
fn local_import_splits_multi_schema_directory_separating_dicts_and_sharing_lua() {
    let directory = tempfile::tempdir().unwrap();
    let core = open_core(directory.path());
    let path = directory.path().join("multi-rime");
    fs::create_dir_all(path.join("lua")).unwrap();
    fs::create_dir_all(path.join("cn_dicts")).unwrap();
    fs::write(
        path.join("alpha.schema.yaml"),
        "schema:\n  schema_id: alpha\n  name: 方案甲\ntranslator:\n  dictionary: alpha\n",
    )
    .unwrap();
    fs::write(
        path.join("beta.schema.yaml"),
        "schema:\n  schema_id: beta\n  name: 方案乙\ntranslator:\n  dictionary: beta\n",
    )
    .unwrap();
    fs::write(
        path.join("alpha.dict.yaml"),
        "---\nname: alpha\nversion: '1'\nimport_tables:\n  - alpha_extra\n...\n甲\ta\n",
    )
    .unwrap();
    fs::write(
        path.join("cn_dicts/alpha_extra.dict.yaml"),
        "---\nname: alpha_extra\n...\n额外甲\taa\n",
    )
    .unwrap();
    fs::write(
        path.join("beta.dict.yaml"),
        "---\nname: beta\nversion: '1'\n...\n乙\tb\n",
    )
    .unwrap();
    fs::write(path.join("rime.lua"), "return {}\n").unwrap();
    fs::write(path.join("lua/common.lua"), "-- shared lua\n").unwrap();

    let first = import_local_scheme(&core, &path, "多方案目录", None).unwrap();
    assert_eq!(first, "local-alpha");

    let alpha_files = stored_files(&core, "local-alpha");
    assert!(alpha_files.contains_key("alpha.schema.yaml"));
    assert!(alpha_files.contains_key("alpha.dict.yaml"));
    assert!(alpha_files.contains_key("cn_dicts/alpha_extra.dict.yaml"));
    assert!(alpha_files.contains_key("rime.lua"));
    assert!(alpha_files.contains_key("lua/common.lua"));
    assert!(!alpha_files.contains_key("beta.schema.yaml"));
    assert!(!alpha_files.contains_key("beta.dict.yaml"));

    let beta_files = stored_files(&core, "local-beta");
    assert!(beta_files.contains_key("beta.schema.yaml"));
    assert!(beta_files.contains_key("beta.dict.yaml"));
    assert!(beta_files.contains_key("rime.lua"));
    assert!(beta_files.contains_key("lua/common.lua"));
    assert!(!beta_files.contains_key("alpha.schema.yaml"));
    assert!(!beta_files.contains_key("alpha.dict.yaml"));
    assert!(!beta_files.contains_key("cn_dicts/alpha_extra.dict.yaml"));

    let store = PackageStore::new(&core);
    assert_eq!(
        store.read_meta("local-alpha").unwrap().unwrap().display_name.as_deref(),
        Some("方案甲")
    );
    assert_eq!(
        store.read_meta("local-beta").unwrap().unwrap().display_name.as_deref(),
        Some("方案乙")
    );
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

fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
    let mut writer = zip::ZipWriter::new(fs::File::create(path).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    for (name, content) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(content).unwrap();
    }
    writer.finish().unwrap();
}

#[test]
fn local_import_filters_all_build_artifacts_including_orphan_bins() {
    let directory = tempfile::tempdir().unwrap();
    let core = open_core(directory.path());
    let path = source(directory.path(), "with-build");
    fs::create_dir_all(path.join("build")).unwrap();
    fs::create_dir_all(path.join("logs")).unwrap();
    fs::create_dir_all(path.join("example.leveldb")).unwrap();
    fs::write(path.join("build/example.table.bin"), b"compiled table").unwrap();
    fs::write(path.join("build/orphan.reverse.bin"), b"orphan compiled bin").unwrap();
    fs::write(path.join("build/example.schema.yaml"), b"built schema").unwrap();
    fs::write(path.join("logs/rime.log"), b"runtime log").unwrap();
    fs::write(path.join("example.leveldb/CURRENT"), b"leveldb").unwrap();

    let key = import_local_scheme(&core, &path, "WithBuild", None).unwrap();
    let files = stored_files(&core, &key);
    assert!(files.contains_key("example.schema.yaml"));
    assert!(
        files.keys().all(|name| !name.starts_with("build/")),
        "expected build/ to be filtered out, got {:?}",
        files.keys().collect::<Vec<_>>()
    );
    assert!(!files.contains_key("logs/rime.log"));
    assert!(!files.contains_key("example.leveldb/CURRENT"));
}

#[test]
fn local_import_reports_size_limit_when_generated_config_exceeds_it() {
    let directory = tempfile::tempdir().unwrap();
    let core = open_core(directory.path());
    let path = source(directory.path(), "at-limit");
    let schema_bytes = fs::metadata(path.join("example.schema.yaml")).unwrap().len();
    fs::File::create(path.join("example.dict.yaml"))
        .unwrap()
        .set_len(DEFAULT_MAX_EXPANDED_BYTES - schema_bytes)
        .unwrap();

    let error = import_local_scheme(&core, &path, "AtLimit", None).unwrap_err();
    let message = error.to_string();
    assert!(message.contains("512 MB"), "{message}");
    assert!(message.contains("default.custom.yaml"), "{message}");
}

#[test]
fn local_zip_import_preserves_non_ascii_display_name() {
    let directory = tempfile::tempdir().unwrap();
    let core = open_core(directory.path());
    let zip = directory.path().join("中文.ZIP");
    write_zip(
        &zip,
        &[(
            "demo.schema.yaml",
            b"schema:\n  schema_id: demo\n  name: Demo\n",
        )],
    );

    let key = import_local_scheme(&core, &zip, "中文", None).unwrap();
    let (_, meta) = PackageStore::new(&core).read_files(&key).unwrap();
    assert_eq!(meta.display_name.as_deref(), Some("中文"));
}

#[test]
fn local_zip_import_rejects_unicode_case_conflicts() {
    let directory = tempfile::tempdir().unwrap();
    let core = open_core(directory.path());
    let zip = directory.path().join("duplicate.zip");
    write_zip(
        &zip,
        &[
            ("demo.schema.yaml", b"schema:\n  schema_id: demo\n  name: Demo\n"),
            ("É.dict.yaml", b"first"),
            ("é.dict.yaml", b"second"),
        ],
    );

    let error = import_local_scheme(&core, &zip, "Duplicate", None).unwrap_err();
    assert!(error.to_string().contains("大小写冲突文件"));
}

#[test]
fn local_import_auto_extracts_zip_and_filters_build_before_size_limit() {
    let directory = tempfile::tempdir().unwrap();
    let core = open_core(directory.path());

    // 1. Wrapped zip (with __MACOSX junk, default.custom.yaml, and build/ dir) auto-unwraps and filters build/.
    let wrapped_zip = directory.path().join("My-Rime-Pack.zip");
    write_zip(
        &wrapped_zip,
        &[
            (
                "my-rime-main/demo.schema.yaml",
                b"schema:\n  schema_id: demo\n  name: Demo\n",
            ),
            (
                "my-rime-main/default.custom.yaml",
                b"patch:\n  schema_list:\n    - schema: demo\n",
            ),
            ("my-rime-main/lua/helper.lua", b"return {}\n"),
            ("my-rime-main/build/demo.table.bin", b"compiled huge binary"),
            ("my-rime-main/build/orphan.reverse.bin", b"orphan binary"),
            ("my-rime-main/logs/rime.log", b"log"),
            ("__MACOSX/my-rime-main/._demo.schema.yaml", b"apple double"),
        ],
    );

    let key = import_local_scheme(&core, &wrapped_zip, "My-Rime-Pack.zip", None).unwrap();
    assert_eq!(key, "local-my-rime-pack");
    let (file_list, meta) = PackageStore::new(&core).read_files(&key).unwrap();
    let files: BTreeMap<String, Vec<u8>> = file_list.into_iter().collect();
    assert_eq!(meta.display_name.as_deref(), Some("My-Rime-Pack"));
    assert!(files.contains_key("demo.schema.yaml"));
    assert!(files.contains_key("lua/helper.lua"));
    assert!(files.keys().all(|name| !name.starts_with("build/")));
    assert!(!files.contains_key("logs/rime.log"));

    // 2. Zip whose build/ claims > 512 MiB in central directory still imports because build/ is skipped before size counting.
    let mut build_bomb_zip = {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            let options = zip::write::SimpleFileOptions::default();
            writer
                .start_file("wrapper/build/huge.table.bin", options)
                .unwrap();
            writer.write_all(b"x").unwrap();
            writer
                .start_file("wrapper/demo.schema.yaml", options)
                .unwrap();
            writer
                .write_all(b"schema:\n  schema_id: demo\n  name: Demo\n")
                .unwrap();
            writer.finish().unwrap();
        }
        cursor.into_inner()
    };
    let entry_pos = build_bomb_zip
        .windows(4)
        .position(|window| window == b"PK\x01\x02")
        .unwrap();
    build_bomb_zip[entry_pos + 24..entry_pos + 28]
        .copy_from_slice(&(512u32 * 1024 * 1024 + 1).to_le_bytes());
    let build_bomb_path = directory.path().join("with-huge-build.zip");
    fs::write(&build_bomb_path, build_bomb_zip).unwrap();
    let key2 = import_local_scheme(&core, &build_bomb_path, "", None).unwrap();
    assert_eq!(key2, "local-with-huge-build");

    // 3. Zip whose non-build file claims > 512 MiB fails with explicit 512 MB limit + pure Rime/zip hint.
    let mut oversized_zip = {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("demo.schema.yaml", options).unwrap();
            writer
                .write_all(b"schema:\n  schema_id: demo\n  name: Demo\n")
                .unwrap();
            writer.start_file("huge.dict.yaml", options).unwrap();
            writer.write_all(b"x").unwrap();
            writer.finish().unwrap();
        }
        cursor.into_inner()
    };
    let second_cd = oversized_zip
        .windows(4)
        .enumerate()
        .filter_map(|(idx, window)| (window == b"PK\x01\x02").then_some(idx))
        .nth(1)
        .unwrap();
    oversized_zip[second_cd + 24..second_cd + 28]
        .copy_from_slice(&(512u32 * 1024 * 1024 + 1).to_le_bytes());
    let oversized_path = directory.path().join("oversized.zip");
    fs::write(&oversized_path, oversized_zip).unwrap();
    let err = import_local_scheme(&core, &oversized_path, "Oversized", None)
        .unwrap_err()
        .to_string();
    assert!(err.contains("512 MB"), "{err}");
    assert!(err.contains("huge.dict.yaml"), "{err}");
    assert!(err.contains("纯 Rime 文件夹"), "{err}");
    assert!(err.contains(".zip"), "{err}");
    assert!(err.contains("build"), "{err}");

    // 4. Non-zip file returns clear prompt.
    let plain_file = directory.path().join("notes.txt");
    fs::write(&plain_file, b"not a zip").unwrap();
    let err = import_local_scheme(&core, &plain_file, "Notes", None)
        .unwrap_err()
        .to_string();
    assert!(err.contains("纯 Rime 文件夹"), "{err}");
    assert!(err.contains(".zip"), "{err}");

    // 5. Zip containing only build/*.schema.yaml is rejected as missing real schema.
    let build_only_zip = directory.path().join("build-only.zip");
    write_zip(
        &build_only_zip,
        &[("build/demo.schema.yaml", b"schema: {schema_id: demo}\n")],
    );
    let err = import_local_scheme(&core, &build_only_zip, "BuildOnly", None)
        .unwrap_err()
        .to_string();
    assert!(err.contains("未找到"), "{err}");
    assert!(err.contains("纯 Rime 文件夹"), "{err}");
}
