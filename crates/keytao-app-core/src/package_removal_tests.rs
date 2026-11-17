use super::*;
use crate::package_store::PackageStore;
use crate::scheme_host::tests::AddonHost;
use std::{io::Write, sync::atomic::Ordering};

const USER_FILES: &[&str] = &[
    "keytao.user.dict.yaml",
    "keytao.userdb/data",
    "keytao.userdb.txt",
    "user.yaml",
    "user_dict.txt",
    "keytao.user.remove.txt",
    "sync/device/data",
    ".keytao-sync-base-keytao.json",
];
const OWNED_CUSTOM_FILES: &[&str] = &["keytao.custom.yaml", "nested/example.custom.yaml"];
const ADDON_FILES: &[&str] = &[
    "easy_en.schema.yaml",
    "easy_en.dict.yaml",
    "easy_en.custom.yaml",
    "lua/easy_en.lua",
    "easy_en-LICENSE",
    "wanxiang.schema.yaml",
    "wanxiang.dict.yaml",
    "wanxiang_algebra.yaml",
    "wanxiang-dicts/zi.dict.yaml",
    "wanxiang-lts-zh-hans.gram",
    "rime_ice.schema.yaml",
    "rime_ice.dict.yaml",
    "melt_eng.schema.yaml",
    "lua/rime_ice/lunar.db",
    "rime_ice-dicts/cn_dicts/8105.dict.yaml",
    ".keytao-easy-en.json",
    ".keytao-wanxiang.json",
    ".keytao-rime-ice.json",
];

fn fixture(key: &str, addons: bool) -> (tempfile::TempDir, AddonHost) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("rime");
    fs::create_dir_all(&root).unwrap();
    let host = AddonHost::new(&root);
    // Resolve shared assets entirely inside the fixture.
    fs::create_dir_all(host.core.env().resource_dir.join("rime-data")).unwrap();
    fs::write(
        host.core.env().resource_dir.join("rime-data/default.yaml"),
        "{}",
    )
    .unwrap();
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let addon_files = if addons { ADDON_FILES } else { &[] };
    for name in [
        "keytao.schema.yaml",
        "auxiliary.schema.yaml",
        "keytao.dict.yaml",
        "rime.lua",
        "lua/main.lua",
        "modified.txt",
    ]
    .into_iter()
    .chain(USER_FILES.iter().copied())
    .chain(OWNED_CUSTOM_FILES.iter().copied())
    .chain(addon_files.iter().copied())
    {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let content = if name == "user.yaml" {
            "var:\n  previously_selected_schema: keytao\ncustom: keep\n"
        } else {
            "fixture"
        };
        fs::write(path, content).unwrap();
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(content.as_bytes()).unwrap();
    }
    let schemas = if addons {
        "    - schema: easy_en\n    - schema: wanxiang\n    - schema: rime_ice\n"
    } else {
        ""
    };
    let config = format!(
        "patch:\n  menu/page_size: 9\n  schema_list:\n    - schema: keytao\n    - schema: auxiliary\n{schemas}"
    );
    fs::write(root.join("default.custom.yaml"), &config).unwrap();
    zip.start_file(
        "default.custom.yaml",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    zip.write_all(config.as_bytes()).unwrap();
    let bytes = zip.finish().unwrap().into_inner();
    if addons {
        for (id, manifest, installable) in [
            ("easy_en", ".keytao-easy-en.json", crate::addon::is_package_file as fn(&str) -> bool),
            ("wanxiang", ".keytao-wanxiang.json", crate::wanxiang::installable as fn(&str) -> bool),
            ("rime_ice", ".keytao-rime-ice.json", crate::rime_ice::installable as fn(&str) -> bool),
        ] {
            let files: Vec<_> = ADDON_FILES.iter().filter(|path| installable(path)).map(|path| {
                serde_json::json!({"path":path, "sha256":sha256_reader(b"fixture".as_slice()).unwrap(), "size":7})
            }).collect();
            let mut value = serde_json::json!({"format":1, "version":"fixture", "source":"fixture", "archive_sha256":"a".repeat(64), "files":files});
            if id == "wanxiang" {
                value["variant"] = "base".into();
                value["includes_grammar"] = true.into();
                value["grammar"] = serde_json::json!({"version":"fixture", "sha256":sha256_reader(b"fixture".as_slice()).unwrap(), "size":7});
            }
            fs::write(root.join(manifest), serde_json::to_vec(&value).unwrap()).unwrap();
        }
    }
    store_cloud_archive(&host.core, key, Some("v1"), "official", None, &bytes, &root).unwrap();
    let meta = PackageStore::new(&host.core)
        .read_meta(key)
        .unwrap()
        .unwrap();
    stage_install(
        &host.core,
        &bytes,
        meta.manifest,
        root.canonicalize().unwrap().to_string_lossy().into_owned(),
    )
    .unwrap();
    record_package(&host.core, key, Some("v1"), true).unwrap();
    fs::write(root.join("modified.txt"), "user modification").unwrap();
    fs::create_dir_all(root.join("build")).unwrap();
    for name in [
        ".keytao-deployed",
        "default.yaml",
        "keytao.schema.yaml",
        "keytao.table.bin",
    ] {
        fs::write(root.join("build").join(name), "stale").unwrap();
    }
    (dir, host)
}

fn assert_user_files_kept(root: &Path) {
    for name in USER_FILES {
        let expected = if *name == "user.yaml" {
            "var:\n  previously_selected_schema: keytao\ncustom: keep\n"
        } else {
            "fixture"
        };
        assert_eq!(
            fs::read_to_string(root.join(name)).unwrap(),
            expected,
            "{name}"
        );
    }
    assert_eq!(
        fs::read_to_string(root.join("modified.txt")).unwrap(),
        "user modification"
    );
}

#[tokio::test]
async fn removing_active_package_preserves_addons_and_user_files() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    let (_dir, host) = fixture("keytao", true);
    remove_scheme_package(&host.core, "keytao", &host)
        .await
        .unwrap();
    let config = fs::read_to_string(host.root.join("default.custom.yaml")).unwrap();
    assert_eq!(
        crate::scheme_files::parse_schema_list(&config),
        ["easy_en", "wanxiang", "rime_ice"]
    );
    assert!(config.contains("menu/page_size: 9"));
    assert_eq!(
        keytao_core::deployed_schema_id_from_dir(&host.root).as_deref(),
        Some("easy_en")
    );
    for name in ADDON_FILES {
        if name.starts_with(".keytao-") {
            assert!(!host.root.join(name).exists(), "{name}");
            continue;
        }
        assert_eq!(
            fs::read_to_string(host.root.join(name)).unwrap(),
            "fixture",
            "{name}"
        );
    }
    for id in ["easy_en", "wanxiang", "rime_ice"] {
        assert!(PackageStore::new(&host.core).addon_path(id).join("activation.json").is_file());
    }
    assert_user_files_kept(&host.root);
    assert_eq!(host.deploys.load(Ordering::SeqCst), 1);
    assert_eq!(host.reloads.load(Ordering::SeqCst), 0);
    assert_removed(&host, "keytao");
}

fn assert_removed(host: &AddonHost, key: &str) {
    for name in [
        "keytao.schema.yaml",
        "auxiliary.schema.yaml",
        "keytao.dict.yaml",
        "lua/main.lua",
    ]
    .into_iter()
    .chain(OWNED_CUSTOM_FILES.iter().copied())
    {
        assert!(!host.root.join(name).exists(), "{name}");
    }
    assert!(host.core.state().active_scheme.is_none());
    assert!(!host.core.state().installed_packages.contains_key(key));
    let store = PackageStore::new(&host.core);
    for path in [
        store.package_path(key),
        store.metadata_path(key),
        store.active_manifest_path(),
    ] {
        assert!(!path.exists());
    }
}

#[tokio::test]
async fn removing_last_active_package_clears_readiness_and_reloads() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    for key in [
        "keytao",
        "xmjd",
        "xmjd-feiding",
        "txjx",
        "keydo",
        "local-example",
    ] {
        let (_dir, host) = fixture(key, false);
        remove_scheme_package(&host.core, key, &host).await.unwrap();
        assert_removed(&host, key);
        assert_user_files_kept(&host.root);
        let state = keytao_core::schema_install_state(&host.root);
        assert!(state.schemas.is_empty() && !state.installed && !state.deployed);
        assert!(keytao_core::deployed_schema_id_from_dir(&host.root).is_none());
        assert!(!host.root.join("build/.keytao-deployed").exists());
        assert!(!host.root.join("build/default.yaml").exists());
        assert_eq!(host.deploys.load(Ordering::SeqCst), 0);
        assert_eq!(host.reloads.load(Ordering::SeqCst), 1);
        assert!(host.core.state().last_deploy.is_none());
    }
}

#[tokio::test]
async fn removing_inactive_package_only_removes_its_cache_and_state() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    let (_dir, host) = fixture("keytao", false);
    let (archive, _) = stored_archive(&host.core, "keytao").unwrap();
    store_cloud_archive(
        &host.core,
        "xmjd",
        Some("v1"),
        "official",
        None,
        &archive,
        &host.root,
    )
    .unwrap();
    let active_before = fs::read(host.root.join(keytao_core::ACTIVE_SCHEME_MANIFEST)).unwrap();
    remove_scheme_package(&host.core, "xmjd", &host)
        .await
        .unwrap();
    assert_eq!(host.core.state().active_scheme.as_deref(), Some("keytao"));
    assert!(!host.core.state().installed_packages.contains_key("xmjd"));
    assert_eq!(
        fs::read(host.root.join(keytao_core::ACTIVE_SCHEME_MANIFEST)).unwrap(),
        active_before
    );
    assert!(host.root.join("keytao.schema.yaml").is_file());
    assert!(host.root.join("build/.keytao-deployed").is_file());
    let store = PackageStore::new(&host.core);
    assert!(store.package_path("keytao").is_dir());
    assert!(!store.package_path("xmjd").exists());
    assert!(!store.metadata_path("xmjd").exists());
    assert_user_files_kept(&host.root);
    assert_eq!(host.deploys.load(Ordering::SeqCst), 0);
    assert_eq!(host.reloads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn failed_removal_deploy_restores_active_sources_and_state() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    let (_dir, host) = fixture("keytao", true);
    let config_before = fs::read(host.root.join("default.custom.yaml")).unwrap();
    host.fail_deploys.store(1, Ordering::SeqCst);
    assert!(
        remove_scheme_package(&host.core, "keytao", &host)
            .await
            .is_err()
    );
    assert_eq!(host.core.state().active_scheme.as_deref(), Some("keytao"));
    assert!(host.core.state().installed_packages.contains_key("keytao"));
    assert_eq!(
        fs::read(host.root.join("default.custom.yaml")).unwrap(),
        config_before
    );
    assert!(host.root.join("keytao.schema.yaml").is_file());
    assert!(host.root.join("build/keytao.schema.yaml").is_file());
    assert_eq!(host.deploys.load(Ordering::SeqCst), 2);
    assert!(
        PackageStore::new(&host.core)
            .package_path("keytao")
            .is_dir()
    );
    assert_user_files_kept(&host.root);
}

#[tokio::test]
async fn removing_active_package_keeps_user_lua_through_deployment() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    for addons in [false, true] {
        let (_dir, host) = fixture("keytao", addons);
        let lua = "-- User code\r\nuser_component = require('user_module')\r\n";
        fs::write(host.root.join("rime.lua"), lua).unwrap();
        remove_scheme_package(&host.core, "keytao", &host)
            .await
            .unwrap();
        assert_eq!(fs::read_to_string(host.root.join("rime.lua")).unwrap(), lua);
        let active: ActiveManifest = serde_json::from_slice(
            &fs::read(host.root.join(keytao_core::ACTIVE_SCHEME_MANIFEST)).unwrap(),
        )
        .unwrap();
        assert_eq!(active.rime_lua.as_deref(), Some(lua));
        keytao_core::regenerate_active_rime_lua(&host.root).unwrap();
        assert_eq!(fs::read_to_string(host.root.join("rime.lua")).unwrap(), lua);
        assert_user_files_kept(&host.root);
    }
}

#[tokio::test]
async fn failed_removal_keeps_user_lua_after_restoring_deployment() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap();
    let (_dir, host) = fixture("keytao", true);
    let lua = "-- User code\r\nuser_component = require('user_module')\r\n";
    fs::write(host.root.join("rime.lua"), lua).unwrap();
    host.fail_deploys.store(1, Ordering::SeqCst);
    assert!(
        remove_scheme_package(&host.core, "keytao", &host)
            .await
            .is_err()
    );
    assert_eq!(host.deploys.load(Ordering::SeqCst), 2);
    assert_eq!(fs::read_to_string(host.root.join("rime.lua")).unwrap(), lua);
    assert_eq!(host.core.state().active_scheme.as_deref(), Some("keytao"));
}
