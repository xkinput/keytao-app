use crate::{AppEnv, Core, CoreEvent, EventSink, Platform, install::smart_install};
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
fn manifest_cleanup_deletes_only_unchanged_obsolete_files() {
    use std::collections::{BTreeMap, BTreeSet};
    let dir = tempfile::tempdir().unwrap();
    for name in ["obsolete.lua", "modified.lua", "shared.lua"] {
        fs::write(dir.path().join(name), "abc").unwrap();
    }
    fs::write(dir.path().join("modified.lua"), "user content").unwrap();
    let hash = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    let manifest = BTreeMap::from([
        ("obsolete.lua".into(), hash.into()),
        ("modified.lua".into(), hash.into()),
        ("shared.lua".into(), hash.into()),
    ]);
    let logs = crate::packages::cleanup_manifest(
        dir.path(),
        &manifest,
        &BTreeSet::from(["shared.lua".into()]),
    )
    .unwrap();
    assert!(!dir.path().join("obsolete.lua").exists());
    assert_eq!(
        fs::read_to_string(dir.path().join("modified.lua")).unwrap(),
        "user content"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("shared.lua")).unwrap(),
        "abc"
    );
    assert!(logs.iter().any(|line| line.contains("modified.lua")));
}

#[test]
fn manifest_cleanup_rejects_unsafe_paths_before_deleting_anything() {
    use std::collections::{BTreeMap, BTreeSet};
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("rime");
    fs::create_dir_all(&root).unwrap();
    fs::write(dir.path().join("outside.lua"), "abc").unwrap();
    for unsafe_path in [
        "../outside.lua",
        "/absolute.lua",
        "dir/../outside.lua",
        "C:/outside.lua",
        "..\\outside.lua",
    ] {
        fs::write(root.join("a-safe.lua"), "abc").unwrap();
        let hash = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let manifest = BTreeMap::from([
            ("a-safe.lua".into(), hash.into()),
            (unsafe_path.into(), hash.into()),
        ]);
        assert!(
            crate::packages::cleanup_manifest(&root, &manifest, &BTreeSet::new()).is_err(),
            "{unsafe_path}"
        );
        assert!(root.join("a-safe.lua").exists());
        assert_eq!(
            fs::read_to_string(dir.path().join("outside.lua")).unwrap(),
            "abc"
        );
    }
}

#[tokio::test]
async fn activation_without_stored_zip_requires_redownload() {
    let dir = tempfile::tempdir().unwrap();
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
    crate::scheme::record_installed_scheme(&core, "keytao", Some("v1")).unwrap();
    let error = crate::packages::activate_scheme(
        &core,
        "keytao",
        &dir.path().join("rime"),
        #[cfg(target_os = "windows")]
        |_, _, _, _| Ok(Vec::new()),
    )
    .await
    .err()
    .expect("Activation must reject an unavailable stored archive");
    assert!(error.to_string().contains("re-download"));
    assert_eq!(core.state().active_scheme.as_deref(), Some("keytao"));
}

#[test]
fn package_cleanup_does_not_apply_active_manifest_to_another_directory() {
    let dir = tempfile::tempdir().unwrap();
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
    crate::scheme::record_installed_scheme(&core, "keytao", Some("v1")).unwrap();
    let store = core.env().data_dir.join("scheme-packages");
    fs::create_dir_all(&store).unwrap();
    let active_root = dir.path().join("active");
    let custom_root = dir.path().join("custom");
    fs::create_dir_all(&active_root).unwrap();
    fs::create_dir_all(&custom_root).unwrap();
    fs::write(custom_root.join("obsolete.lua"), "abc").unwrap();
    let manifest = serde_json::json!({"obsolete.lua": "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"});
    fs::write(
        store.join("keytao.json"),
        serde_json::to_vec(&serde_json::json!({
            "scheme_key":"keytao", "version":"v1", "installed_at_unix":0,
            "source":"official", "manifest":manifest,
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        store.join("active-manifest.json"),
        serde_json::to_vec(&serde_json::json!({
            "root":active_root, "manifest":manifest,
        }))
        .unwrap(),
    )
    .unwrap();
    let cursor = std::io::Cursor::new(Vec::new());
    let zip = zip::ZipWriter::new(cursor).finish().unwrap().into_inner();
    crate::packages::cleanup_previous(&core, &custom_root, &zip).unwrap();
    assert_eq!(
        fs::read_to_string(custom_root.join("obsolete.lua")).unwrap(),
        "abc"
    );
}

#[tokio::test]
async fn custom_install_preserves_default_package_and_state() {
    fn write_archive(path: &std::path::Path, version: &str) {
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        for (name, content) in [
            (
                "default.custom.yaml",
                "patch:\n  schema_list:\n    - schema: xmjd6\n",
            ),
            ("xmjd6.schema.yaml", "schema: {schema_id: xmjd6}\n"),
            ("version.txt", version),
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    for custom_scheme in ["keytao", "xmjd"] {
        let dir = tempfile::tempdir().unwrap();
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
        let default_root = dir.path().join("rime");
        let custom_root = dir.path().join("custom");
        let archive = dir.path().join("package.zip");
        write_archive(&archive, "v1");
        smart_install(
            &core,
            archive.to_string_lossy().into_owned(),
            default_root.to_string_lossy().into_owned(),
            #[cfg(target_os = "windows")]
            |_, _, _, _| Ok(Vec::new()),
        )
        .await
        .unwrap();
        crate::packages::record_package(&core, "keytao", Some("v1"), true).unwrap();
        let state = core.state();
        let state_path = core.env().data_dir.join("app-state.json");
        let saved_state = fs::read(&state_path).unwrap();
        let store = core.env().data_dir.join("scheme-packages");
        let saved_store: std::collections::BTreeMap<_, _> = fs::read_dir(&store)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.file_name(), fs::read(entry.path()).unwrap())
            })
            .collect();

        write_archive(&archive, "v2");
        smart_install(
            &core,
            archive.to_string_lossy().into_owned(),
            custom_root.to_string_lossy().into_owned(),
            #[cfg(target_os = "windows")]
            |_, _, _, _| Ok(Vec::new()),
        )
        .await
        .unwrap();
        crate::packages::record_package(&core, custom_scheme, Some("v2"), false).unwrap();

        assert!(core.state() == state, "Custom install changed package state");
        assert_eq!(fs::read(&state_path).unwrap(), saved_state);
        let actual_store: std::collections::BTreeMap<_, _> = fs::read_dir(&store)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.file_name(), fs::read(entry.path()).unwrap())
            })
            .collect();
        assert_eq!(
            actual_store, saved_store,
            "Custom install changed the store or left pending files"
        );
        assert_eq!(
            crate::scheme::check_local_schema(&core, Some(default_root.clone()))
                .unwrap()
                .version
                .as_deref(),
            Some("v1")
        );
        assert_eq!(
            fs::read_to_string(custom_root.join("version.txt")).unwrap(),
            "v2"
        );
        crate::packages::activate_scheme(
            &core,
            "keytao",
            &default_root,
            #[cfg(target_os = "windows")]
            |_, _, _, _| Ok(Vec::new()),
        )
        .await
        .unwrap();
        assert_eq!(
            fs::read_to_string(default_root.join("version.txt")).unwrap(),
            "v1"
        );
        assert_eq!(
            core.state().installed_packages["keytao"].version.as_deref(),
            Some("v1")
        );
    }
}

#[tokio::test]
async fn archive_install_preserves_user_merges_invalidates_build_and_emits_progress() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("rime");
    fs::create_dir_all(dest.join("lua")).unwrap();
    fs::create_dir_all(dest.join("build")).unwrap();
    fs::write(
        dest.join("default.custom.yaml"),
        "patch:\n  schema_list:\n    - schema: personal\n",
    )
    .unwrap();
    fs::write(
        dest.join("rime.lua"),
        "collectgarbage('setpause', 50)\ncollectgarbage ( \"setstepmul\", 400 )\ncustom = require(\"custom\")\nuser_flag = true\n",
    )
    .unwrap();
    fs::write(dest.join("lua/custom.lua"), "user code").unwrap();
    fs::write(dest.join("build/keydo.schema.yaml"), "stale schema").unwrap();
    fs::write(dest.join("build/keydo.table.bin"), "stale dictionary").unwrap();
    fs::write(
        dest.join("build/personal.table.bin"),
        "unrelated dictionary",
    )
    .unwrap();
    let zip_path = dir.path().join("package.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(&zip_path).unwrap());
    let entries = [
        (
            "default.custom.yaml",
            "patch:\n  schema_list:\n    - schema: keydo\n",
        ),
        ("rime.lua", "collectgarbage('setpause', 150)\npackage = require(\"package\")\n"),
        ("lua/custom.lua", "package code"),
        ("lua/package.lua", "package module"),
        ("keydo.schema.yaml", "schema: {schema_id: keydo}\n"),
        ("keydo.dict.yaml", "name: keydo\n"),
    ];
    for (name, content) in entries {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(content.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
    let sink = Arc::new(ProgressSink::default());
    let core = Core::new(
        AppEnv {
            data_dir: dir.path().join("state"),
            cache_dir: dir.path().join("cache"),
            resource_dir: dir.path().join("resources"),
            app_version: "test".into(),
            platform: Platform::MacOs,
        },
        sink.clone(),
    )
    .unwrap();
    let result = smart_install(
        &core,
        zip_path.to_string_lossy().into_owned(),
        dest.to_string_lossy().into_owned(),
        #[cfg(target_os = "windows")]
        |root, schemas, dictionaries, _| {
            keytao_core::invalidate_rime_build_artifacts(root, schemas, dictionaries).map(|paths| {
                paths
                    .into_iter()
                    .map(|p| format!("[INVALIDATED] {p}"))
                    .collect()
            })
        },
    )
    .await
    .unwrap();
    assert_eq!(result.merged_schemas, vec!["personal"]);
    assert_eq!(
        crate::scheme_files::parse_schema_list(
            &fs::read_to_string(dest.join("default.custom.yaml")).unwrap()
        ),
        vec!["personal", "keydo"]
    );
    assert_eq!(
        fs::read_to_string(dest.join("lua/custom_user.lua")).unwrap(),
        "user code"
    );
    assert_eq!(
        fs::read_to_string(dest.join("lua/custom.lua")).unwrap(),
        "package code"
    );
    assert!(
        fs::read_to_string(dest.join("rime.lua"))
            .unwrap()
            .contains("require(\"custom_user\")")
    );
    let rime_lua = fs::read_to_string(dest.join("rime.lua")).unwrap();
    assert!(rime_lua.starts_with(entries[1].1));
    assert!(rime_lua.contains("user_flag = true"));
    assert_eq!(rime_lua.matches("collectgarbage").count(), 1);
    assert!(!dest.join("build/keydo.schema.yaml").exists());
    assert!(!dest.join("build/keydo.table.bin").exists());
    assert_eq!(
        fs::read_to_string(dest.join("build/personal.table.bin")).unwrap(),
        "unrelated dictionary"
    );
    assert!(result.verify.iter().all(|entry| entry.ok));
    assert!(result.logs.iter().any(|line| line.starts_with("[RENAMED]")));
    assert!(!zip_path.exists());
    let progress = sink.0.lock().unwrap();
    assert_eq!(
        progress.first().unwrap(),
        &("extracting".into(), 61, "正在解压...".into())
    );
    assert_eq!(
        progress.last().unwrap(),
        &("done".into(), 100, "安装完成！".into())
    );
    assert_eq!(progress.len(), entries.len() + 2);
    for (index, (_, percent, _)) in progress[1..progress.len() - 1].iter().enumerate() {
        assert_eq!(*percent, 61 + ((index + 1) * 39 / entries.len()) as u32);
    }
}

#[tokio::test]
async fn package_without_rime_lua_rewrites_only_changed_local_content() {
    let preserved = concat!(
        "-- User settings\r\n",
        "--[[\ncollectgarbage('setpause', 75)\n--]]\n",
        "message = [[\ncollectgarbage('setstepmul', 300)\n]]\n",
        "user_flag = true\r\n",
        "collectgarbage('collect')",
    );
    let tuned = format!("collectgarbage ( 'setpause', 50 )\r\n{preserved}");
    for local in [Some(tuned.as_str()), Some(preserved), None] {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("rime");
        fs::create_dir_all(&dest).unwrap();
        let path = dest.join("rime.lua");
        let old_mtime = std::time::UNIX_EPOCH + std::time::Duration::from_secs(10);
        if let Some(local) = local {
            fs::write(&path, local).unwrap();
            fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(old_mtime)
                .unwrap();
        }
        let zip_path = dir.path().join("package.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&zip_path).unwrap());
        zip.start_file("xmjd6.schema.yaml", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"schema: {schema_id: xmjd6}\n").unwrap();
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
        let result = smart_install(
            &core,
            zip_path.to_string_lossy().into_owned(),
            dest.to_string_lossy().into_owned(),
            #[cfg(target_os = "windows")]
            |_, _, _, _| Ok(Vec::new()),
        )
        .await
        .unwrap();
        let changed = local == Some(tuned.as_str());
        if local.is_some() {
            assert_eq!(fs::read_to_string(&path).unwrap(), preserved);
            assert_eq!(fs::metadata(&path).unwrap().modified().unwrap() != old_mtime, changed);
        } else {
            assert!(!path.exists());
        }
        assert_eq!(result.logs.iter().any(|line| line == "[MERGED] rime.lua"), changed);
        assert!(result.verify.iter().all(|entry| entry.ok));
    }
}
