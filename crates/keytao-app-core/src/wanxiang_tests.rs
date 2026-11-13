use super::*;
use std::{
    io::{Cursor, Write},
    sync::MutexGuard as StdMutexGuard,
};
use zip::{ZipWriter, write::SimpleFileOptions};

const VERSION: &str = "fixture-version";
const SOURCE_URL: &str = "https://github.com/amzxyz/rime-wanxiang/releases/tag/fixture-version";
const DOWNLOAD_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn fixture_package() -> RemotePackage {
    RemotePackage {
        version: VERSION.into(),
        source: SOURCE_URL.into(),
        download_url: "https://example.invalid/package.zip".into(),
        download_bytes: None,
    }
}

fn commit_fixture(receipt: &mut InstallReceipt, files: Vec<InstalledFile>) -> Result<(), String> {
    commit_files(
        receipt,
        files,
        &fixture_package(),
        DOWNLOAD_SHA256.into(),
        "base",
    )
}

#[test]
fn download_requires_complete_bounded_bytes_without_pinning_a_release_hash() {
    assert!(verify_download(42, Some(42)).is_ok());
    assert!(verify_download(42, None).is_ok());
    for size in [0, 41, 43, MAX_DOWNLOAD_BYTES + 1] {
        assert!(verify_download(size, Some(42)).is_err());
    }
}

#[test]
fn latest_release_redirect_preserves_upstream_tag_and_selects_base_asset() {
    for location in [
        "https://github.com/amzxyz/rime-wanxiang/releases/tag/v99.1",
        "/amzxyz/rime-wanxiang/releases/tag/v99.1",
    ] {
        let tag = release_tag_from_location(location).unwrap();
        let package = release_package(&tag, "base").unwrap();
        assert_eq!(package.version, "v99.1");
        assert_eq!(
            package.source,
            "https://github.com/amzxyz/rime-wanxiang/releases/tag/v99.1"
        );
        assert_eq!(
            package.download_url,
            "https://github.com/amzxyz/rime-wanxiang/releases/download/v99.1/rime-wanxiang-base.zip"
        );
        assert_eq!(package.download_bytes, None);
    }
}

#[test]
fn variants_map_to_official_release_assets() {
    for (variant, asset) in [
        ("base", "rime-wanxiang-base.zip"),
        ("pure", "rime-wanxiang-pure.zip"),
        ("lite", "rime-wanxiang-lite.zip"),
        ("flypy", "rime-wanxiang-flypy-fuzhu.zip"),
        ("hanxin", "rime-wanxiang-hanxin-fuzhu.zip"),
        ("moqi", "rime-wanxiang-moqi-fuzhu.zip"),
        ("shouyou", "rime-wanxiang-shouyou-fuzhu.zip"),
        ("shyplus", "rime-wanxiang-shyplus-fuzhu.zip"),
        ("tiger", "rime-wanxiang-tiger-fuzhu.zip"),
        ("wubi", "rime-wanxiang-wubi-fuzhu.zip"),
        ("wx", "rime-wanxiang-wx-fuzhu.zip"),
        ("zrm", "rime-wanxiang-zrm-fuzhu.zip"),
    ] {
        assert_eq!(variant_asset(variant).unwrap(), asset);
        let package = release_package("v99.1", variant).unwrap();
        assert_eq!(
            package.download_url,
            format!("https://github.com/amzxyz/rime-wanxiang/releases/download/v99.1/{asset}")
        );
        let fallback = package_for_install(Err("offline".into()), variant).unwrap();
        assert_eq!(
            fallback.download_url,
            format!("{RELEASE_LATEST}/download/{asset}")
        );
    }
}

#[test]
fn unknown_variants_fail_closed_for_assets_profiles_and_fallbacks() {
    for variant in [
        "",
        "BASE",
        "base ",
        "../base",
        "base.zip",
        "flypy-fuzhu",
        "nightly",
        "基础版",
    ] {
        assert!(variant_asset(variant).is_err(), "accepted {variant}");
        assert!(variant_source(variant).is_err(), "accepted {variant}");
        assert!(release_package("v99.1", variant).is_err());
        assert!(package_for_install(Err("offline".into()), variant).is_err());
        assert!(package_for_install(Ok(fixture_package()), variant).is_err());
    }
}

#[test]
fn latest_release_redirect_rejects_missing_tags_and_unexpected_targets() {
    for location in [
        "",
        "https://github.com/amzxyz/rime-wanxiang/releases/tag/",
        "https://github.com/amzxyz/rime-wanxiang/releases/tag/v1/extra",
        "https://github.com/other/repository/releases/tag/v1",
        "https://example.invalid/amzxyz/rime-wanxiang/releases/tag/v1",
        "http://github.com/amzxyz/rime-wanxiang/releases/tag/v1",
        "https://user@github.com/amzxyz/rime-wanxiang/releases/tag/v1",
        "https://github.com/amzxyz/rime-wanxiang/releases/tag/v1?query=1",
        "https://github.com/amzxyz/rime-wanxiang/releases/tag/v1#fragment",
    ] {
        assert!(
            release_tag_from_location(location).is_err(),
            "accepted {location}"
        );
    }
}

#[test]
fn install_survives_a_failed_version_lookup_and_records_an_unknown_version() {
    let package = package_for_install(Err("version lookup unavailable".into()), "base").unwrap();
    assert!(package.version.is_empty());
    assert_eq!(package.source, RELEASE_LATEST);
    assert_eq!(
        package.download_url,
        "https://github.com/amzxyz/rime-wanxiang/releases/latest/download/rime-wanxiang-base.zip"
    );
    let root = TestRoot::new();
    let mut receipt = InstallReceipt::new(&root.path).unwrap();
    let files = stage_fixture(&receipt, &[]);
    commit_files(
        &mut receipt,
        files,
        &package,
        DOWNLOAD_SHA256.into(),
        "base",
    )
    .unwrap();
    receipt.finish().unwrap();
    let status = status_at(&root.path);
    assert!(status.installed);
    assert!(status.version.is_empty());
}

struct TestRoot {
    path: PathBuf,
    cleanup: bool,
    _serial: StdMutexGuard<'static, ()>,
}

impl TestRoot {
    fn new() -> Self {
        let serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let path = std::env::temp_dir().join(format!(
            "keytao-wanxiang-test-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self {
            path,
            cleanup: true,
            _serial: serial,
        }
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        if !self.cleanup {
            return;
        }
        assert_eq!(
            self.path.parent().unwrap().canonicalize().unwrap(),
            std::env::temp_dir().canonicalize().unwrap()
        );
        fs::remove_dir_all(&self.path).unwrap();
    }
}

fn archive_at(path: &Path, entries: &[(&str, &str)]) {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, text) in entries {
        zip.start_file(*name, SimpleFileOptions::default()).unwrap();
        zip.write_all(text.as_bytes()).unwrap();
    }
    fs::write(path, zip.finish().unwrap().into_inner()).unwrap();
}

fn stage_fixture(receipt: &InstallReceipt, extra: &[(&str, &str)]) -> Vec<InstalledFile> {
    let dictionaries: Vec<_> = DICTIONARIES
        .iter()
        .map(|name| format!("dicts/{name}.dict.yaml"))
        .collect();
    let dictionary = format!(
        "---\nname: wanxiang\nimport_tables:\n{}...\n",
        DICTIONARIES
            .iter()
            .map(|name| format!("  - dicts/{name}\n"))
            .collect::<String>()
    );
    let mut entries: Vec<_> = REQUIRED
        .iter()
        .map(|name| {
            (
                *name,
                if *name == "wanxiang.dict.yaml" {
                    dictionary.as_str()
                } else {
                    "grammar:\n  language: wanxiang-lts-zh-hans\ntranslator:\n  dictionary: wanxiang\n"
                },
            )
        })
        .collect();
    entries.extend(
        dictionaries
            .iter()
            .map(|name| (name.as_str(), "dictionary fixture")),
    );
    entries.extend(extra.iter().copied());
    let archive = receipt.work.join("fixture.zip");
    archive_at(&archive, &entries);
    extract(
        &archive,
        &receipt.work.join("files"),
        &fixture_package(),
        "base",
    )
    .unwrap()
}

#[test]
fn variant_archive_profiles_preserve_dictionary_imports_and_matching_algebra() {
    for (variant, source, suffix, algebra) in [
        ("base", "wanxiang", "", &["base/全拼"][..]),
        (
            "pure",
            "wanxiang_pure",
            ".pro",
            &["pro/全拼", "pro/间接辅助"][..],
        ),
        ("lite", "wanxiang_lite", ".lite", &["lite/全拼"][..]),
        (
            "flypy",
            "wanxiang_pro",
            ".pro",
            &["pro/自然码", "pro/直接辅助"][..],
        ),
        (
            "moqi",
            "wanxiang_pro",
            ".pro",
            &["pro/自然码", "pro/直接辅助"][..],
        ),
    ] {
        assert_eq!(variant_source(variant).unwrap(), source);
        let root = TestRoot::new();
        let mut receipt = InstallReceipt::new(&root.path).unwrap();
        let archive = receipt.work.join("fixture.zip");
        let schema_name = format!("{source}.schema.yaml");
        let dictionary_name = format!("{source}.dict.yaml");
        let dictionary = format!(
            "---\nname: {source}\nimport_tables:\n  - dicts/zi{suffix}\n  - dicts/jichu{suffix}\n...\n"
        );
        let character_name = format!("dicts/zi{suffix}.dict.yaml");
        let phrase_name = format!("dicts/jichu{suffix}.dict.yaml");
        archive_at(
            &archive,
            &[
                (&schema_name, "upstream schema fixture"),
                (&dictionary_name, &dictionary),
                ("wanxiang_algebra.yaml", "upstream algebra fixture"),
                (&character_name, "character fixture"),
                (&phrase_name, "phrase fixture"),
                ("dicts/private.pro.dict.yaml", "excluded private table"),
            ],
        );
        let files = extract(
            &archive,
            &receipt.work.join("files"),
            &fixture_package(),
            variant,
        )
        .unwrap();
        commit_files(
            &mut receipt,
            files,
            &fixture_package(),
            DOWNLOAD_SHA256.into(),
            variant,
        )
        .unwrap();
        receipt.finish().unwrap();
        let status = status_at(&root.path);
        assert!(status.installed, "not installed: {variant}");
        assert_eq!(status.variant, variant);
        let dictionary = fs::read_to_string(root.path.join("wanxiang.dict.yaml")).unwrap();
        assert!(dictionary.contains("name: wanxiang\n"));
        for name in ["zi", "jichu"] {
            let table = format!("wanxiang-dicts/{name}{suffix}");
            assert!(dictionary.contains(&table));
            assert!(root.path.join(format!("{table}.dict.yaml")).is_file());
        }
        assert!(
            !root
                .path
                .join("wanxiang-dicts/private.pro.dict.yaml")
                .exists()
        );
        let schema = fs::read_to_string(root.path.join("wanxiang.schema.yaml")).unwrap();
        for path in algebra {
            assert!(schema.contains(&format!("wanxiang_algebra:/{path}")));
        }
        assert!(!schema.contains("lua_"));
        if variant != "base" {
            assert!(!schema.contains("wanxiang_algebra:/base/全拼"));
        }
    }
}

#[test]
fn manifest_round_trip_preserves_variant_and_old_records_default_to_base() {
    let root = TestRoot::new();
    let mut receipt = InstallReceipt::new(&root.path).unwrap();
    let files = stage_fixture(&receipt, &[]);
    commit_files(
        &mut receipt,
        files,
        &fixture_package(),
        DOWNLOAD_SHA256.into(),
        "moqi",
    )
    .unwrap();
    receipt.finish().unwrap();
    let path = root.path.join(MANIFEST);
    let bytes = fs::read(&path).unwrap();
    let manifest: Manifest = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(manifest.variant, "moqi");
    assert_eq!(status_at(&root.path).variant, "moqi");
    let mut old: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    old.as_object_mut().unwrap().remove("variant");
    fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
    let manifest = read_manifest(&root.path).unwrap().unwrap();
    assert_eq!(manifest.variant, "base");
    let status = status_at(&root.path);
    assert!(status.installed);
    assert_eq!(status.variant, "base");
}

#[test]
fn manifest_with_unknown_variant_cannot_be_used_for_uninstall() {
    let root = TestRoot::new();
    let mut receipt = InstallReceipt::new(&root.path).unwrap();
    let files = stage_fixture(&receipt, &[]);
    commit_fixture(&mut receipt, files).unwrap();
    receipt.finish().unwrap();
    let path = root.path.join(MANIFEST);
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["variant"] = "unknown".into();
    fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(read_manifest(&root.path).is_err());
    assert!(uninstall(&root.path).is_err());
    assert!(root.path.join("wanxiang.schema.yaml").is_file());
}

#[test]
fn replacing_variant_preserves_modified_dictionaries_and_rolls_back_removed_files() {
    let root = TestRoot::new();
    let mut receipt = InstallReceipt::new(&root.path).unwrap();
    let files = stage_fixture(&receipt, &[]);
    commit_fixture(&mut receipt, files).unwrap();
    receipt.finish().unwrap();
    let modified = root.path.join("wanxiang-dicts/jichu.dict.yaml");
    fs::write(&modified, "private changes").unwrap();
    let original_manifest = fs::read(root.path.join(MANIFEST)).unwrap();
    for finish in [false, true] {
        let mut receipt = InstallReceipt::new(&root.path).unwrap();
        let archive = receipt.work.join("fixture.zip");
        archive_at(
            &archive,
            &[
                ("wanxiang_pro.schema.yaml", "schema fixture"),
                (
                    "wanxiang_pro.dict.yaml",
                    "---\nname: wanxiang_pro\nimport_tables:\n  - dicts/zi.pro\n  - dicts/jichu.pro\n...\n",
                ),
                ("wanxiang_algebra.yaml", "algebra fixture"),
                ("dicts/zi.pro.dict.yaml", "pro character fixture"),
                ("dicts/jichu.pro.dict.yaml", "pro phrase fixture"),
            ],
        );
        let files = extract(
            &archive,
            &receipt.work.join("files"),
            &fixture_package(),
            "moqi",
        )
        .unwrap();
        commit_files(
            &mut receipt,
            files,
            &fixture_package(),
            DOWNLOAD_SHA256.into(),
            "moqi",
        )
        .unwrap();
        assert_eq!(status_at(&root.path).variant, "moqi");
        assert!(!root.path.join("wanxiang-dicts/zi.dict.yaml").exists());
        assert_eq!(fs::read_to_string(&modified).unwrap(), "private changes");
        assert!(root.path.join("wanxiang-dicts/zi.pro.dict.yaml").is_file());
        if finish {
            receipt.finish().unwrap();
            let manifest = read_manifest(&root.path).unwrap().unwrap();
            assert_eq!(manifest.variant, "moqi");
            assert!(manifest.files.iter().all(|file| {
                !file.path.starts_with("wanxiang-dicts/") || file.path.ends_with(".pro.dict.yaml")
            }));
        } else {
            drop(receipt);
            assert_eq!(
                fs::read(root.path.join(MANIFEST)).unwrap(),
                original_manifest
            );
            assert_eq!(status_at(&root.path).variant, "base");
            assert!(root.path.join("wanxiang-dicts/zi.dict.yaml").is_file());
            assert!(!root.path.join("wanxiang-dicts/zi.pro.dict.yaml").exists());
            assert_eq!(fs::read_to_string(&modified).unwrap(), "private changes");
        }
    }
    uninstall(&root.path).unwrap().finish().unwrap();
    assert_eq!(fs::read_to_string(modified).unwrap(), "private changes");
    assert!(!root.path.join("wanxiang-dicts/zi.pro.dict.yaml").exists());
}

#[test]
fn package_without_lianxiang_installs_only_provided_dictionaries() {
    let root = TestRoot::new();
    let mut receipt = InstallReceipt::new(&root.path).unwrap();
    let archive = receipt.work.join("fixture.zip");
    archive_at(
        &archive,
        &[
            ("wanxiang.schema.yaml", "schema fixture"),
            (
                "wanxiang.dict.yaml",
                "---\nname: wanxiang\nimport_tables:\n  - dicts/zi # base table\n  - 'dicts/jichu' # phrase table\n# - dicts/lianxiang\n  # - dicts/shici\n  - other/unused\n...\n",
            ),
            ("wanxiang_algebra.yaml", "algebra fixture"),
            ("dicts/zi.dict.yaml", "character fixture"),
            ("dicts/jichu.dict.yaml", "phrase fixture"),
            ("dicts/cuoyin.dict.yaml", "unreferenced dictionary fixture"),
            ("dicts/en.dict.yaml", "excluded dictionary fixture"),
        ],
    );
    let files = extract(
        &archive,
        &receipt.work.join("files"),
        &fixture_package(),
        "base",
    )
    .unwrap();
    commit_fixture(&mut receipt, files).unwrap();
    receipt.finish().unwrap();
    assert!(status_at(&root.path).installed);
    for name in ["zi", "jichu", "cuoyin"] {
        assert!(
            root.path
                .join(format!("wanxiang-dicts/{name}.dict.yaml"))
                .is_file()
        );
    }
    for name in ["lianxiang", "shici", "en"] {
        assert!(
            !root
                .path
                .join(format!("wanxiang-dicts/{name}.dict.yaml"))
                .exists()
        );
    }
    let dictionary = fs::read_to_string(root.path.join("wanxiang.dict.yaml")).unwrap();
    assert!(dictionary.contains("  - wanxiang-dicts/zi # base table"));
    assert!(dictionary.contains("  - 'wanxiang-dicts/jichu' # phrase table"));
}

#[test]
fn package_missing_a_listed_dictionary_is_rejected() {
    let root = TestRoot::new();
    let archive = root.path.join("fixture.zip");
    archive_at(
        &archive,
        &[
            ("wanxiang.schema.yaml", "schema fixture"),
            (
                "wanxiang.dict.yaml",
                "---\nname: wanxiang\nimport_tables:\n  - dicts/zi\n  - dicts/lianxiang # required table\n...\n",
            ),
            ("wanxiang_algebra.yaml", "algebra fixture"),
            ("dicts/zi.dict.yaml", "character fixture"),
        ],
    );
    assert_eq!(
        extract(
            &archive,
            &root.path.join("stage"),
            &fixture_package(),
            "base",
        )
        .unwrap_err(),
        "万象发布包缺少必要文件：wanxiang-dicts/lianxiang.dict.yaml"
    );
}

#[test]
fn installed_status_requires_manifest_files_and_core_files() {
    let root = TestRoot::new();
    let mut receipt = InstallReceipt::new(&root.path).unwrap();
    let files = stage_fixture(&receipt, &[]);
    commit_fixture(&mut receipt, files).unwrap();
    receipt.finish().unwrap();
    assert!(status_at(&root.path).installed);
    let mut manifest = read_manifest(&root.path).unwrap().unwrap();
    manifest
        .files
        .retain(|file| file.path != "wanxiang_algebra.yaml");
    fs::write(
        root.path.join(MANIFEST),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let algebra = fs::read(root.path.join("wanxiang_algebra.yaml")).unwrap();
    fs::remove_file(root.path.join("wanxiang_algebra.yaml")).unwrap();
    assert!(!status_at(&root.path).installed);
    fs::write(root.path.join("wanxiang_algebra.yaml"), algebra).unwrap();
    assert!(status_at(&root.path).installed);
    fs::remove_file(root.path.join(NOTICE)).unwrap();
    assert!(!status_at(&root.path).installed);
}

#[test]
fn portable_archive_paths_reject_traversal_and_windows_aliases() {
    for name in [
        "../escape",
        "dicts/../../escape",
        "/absolute",
        "C:/escape",
        "lua\\evil",
        "lua/data/file:stream",
        "lua//file",
        "lua/./file",
        "dicts/NUL.yaml",
        "dicts/COM1.txt",
        "dicts/name.",
        "dicts/name ",
    ] {
        assert!(validate_relative(name).is_err(), "accepted {name}");
    }
    assert!(validate_relative("dicts/jichu.dict.yaml").is_ok());
}

#[test]
fn extraction_rejects_traversal_even_in_excluded_files() {
    let root = TestRoot::new();
    let archive = root.path.join("fixture.zip");
    archive_at(&archive, &[("../outside.txt", "bad")]);
    assert!(
        extract(
            &archive,
            &root.path.join("stage"),
            &fixture_package(),
            "base"
        )
        .unwrap_err()
        .contains("不安全路径")
    );
    assert!(!root.path.join("outside.txt").exists());
}

#[test]
fn extraction_rejects_case_insensitive_duplicate_files() {
    let root = TestRoot::new();
    let archive = root.path.join("fixture.zip");
    archive_at(
        &archive,
        &[("wanxiang.schema.yaml", "a"), ("WANXIANG.schema.yaml", "b")],
    );
    assert!(
        extract(
            &archive,
            &root.path.join("stage"),
            &fixture_package(),
            "base"
        )
        .unwrap_err()
        .contains("重复路径")
    );
}

#[test]
fn extraction_rejects_symlinks() {
    let root = TestRoot::new();
    let archive = root.path.join("fixture.zip");
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.add_symlink(
        "lua/wanxiang/evil.lua",
        "../../outside",
        SimpleFileOptions::default(),
    )
    .unwrap();
    fs::write(&archive, zip.finish().unwrap().into_inner()).unwrap();
    assert!(
        extract(
            &archive,
            &root.path.join("stage"),
            &fixture_package(),
            "base"
        )
        .unwrap_err()
        .contains("符号链接")
    );
}

#[test]
fn install_preserves_host_configuration_and_customizations() {
    let root = TestRoot::new();
    for name in [
        "default.yaml",
        "default.custom.yaml",
        "weasel.yaml",
        "version.txt",
        "wanxiang.custom.yaml",
        "custom_phrase.dict.yaml",
    ] {
        fs::write(root.path.join(name), "user-owned").unwrap();
    }
    let mut receipt = InstallReceipt::new(&root.path).unwrap();
    let files = stage_fixture(
        &receipt,
        &[
            ("default.yaml", "upstream"),
            ("weasel.yaml", "upstream"),
            ("version.txt", "upstream"),
            ("wanxiang.custom.yaml", "upstream"),
        ],
    );
    commit_fixture(&mut receipt, files).unwrap();
    receipt.finish().unwrap();
    assert!(status_at(&root.path).installed);
    assert!(!status_at(&root.path).deployed);
    for name in [
        "default.yaml",
        "default.custom.yaml",
        "weasel.yaml",
        "version.txt",
        "wanxiang.custom.yaml",
        "custom_phrase.dict.yaml",
    ] {
        assert_eq!(
            fs::read_to_string(root.path.join(name)).unwrap(),
            "user-owned"
        );
    }
    let schema = fs::read_to_string(root.path.join("wanxiang.schema.yaml")).unwrap();
    assert!(schema.contains("language: \"\""));
    assert!(!schema.contains("language: wanxiang-lts-zh-hans"));
}

#[test]
fn dropping_receipt_rolls_back_all_installed_sources() {
    let root = TestRoot::new();
    fs::write(root.path.join("default.custom.yaml"), "user-owned").unwrap();
    let mut receipt = InstallReceipt::new(&root.path).unwrap();
    let files = stage_fixture(&receipt, &[]);
    commit_fixture(&mut receipt, files).unwrap();
    assert!(status_at(&root.path).installed);
    drop(receipt);
    assert!(!root.path.join(MANIFEST).exists());
    assert!(!root.path.join("wanxiang.schema.yaml").exists());
    assert_eq!(
        fs::read_to_string(root.path.join("default.custom.yaml")).unwrap(),
        "user-owned"
    );
}

#[test]
fn conflicting_existing_file_prevents_all_commit_writes() {
    let root = TestRoot::new();
    fs::create_dir(root.path.join("wanxiang-dicts")).unwrap();
    fs::write(
        root.path.join("wanxiang-dicts/jichu.dict.yaml"),
        "private vocabulary",
    )
    .unwrap();
    let mut receipt = InstallReceipt::new(&root.path).unwrap();
    let files = stage_fixture(&receipt, &[]);
    assert!(
        commit_fixture(&mut receipt, files)
            .unwrap_err()
            .contains("冲突")
    );
    drop(receipt);
    assert!(!root.path.join("wanxiang.schema.yaml").exists());
    assert_eq!(
        fs::read_to_string(root.path.join("wanxiang-dicts/jichu.dict.yaml")).unwrap(),
        "private vocabulary"
    );
}

#[test]
fn uninstall_preserves_modified_files_and_learned_vocabulary() {
    let root = TestRoot::new();
    fs::write(root.path.join("wanxiang.custom.yaml"), "patch: {}").unwrap();
    fs::write(root.path.join("custom_phrase.dict.yaml"), "private phrases").unwrap();
    let mut receipt = InstallReceipt::new(&root.path).unwrap();
    let files = stage_fixture(&receipt, &[("wanxiang.custom.yaml", "patch: {}")]);
    commit_fixture(&mut receipt, files).unwrap();
    receipt.finish().unwrap();
    fs::write(
        root.path.join("wanxiang-dicts/jichu.dict.yaml"),
        "user modifications",
    )
    .unwrap();
    fs::create_dir(root.path.join("wanxiang.userdb")).unwrap();
    fs::write(root.path.join("wanxiang.userdb/00001.log"), "learned").unwrap();
    uninstall(&root.path).unwrap().finish().unwrap();
    assert!(!status_at(&root.path).installed);
    assert!(!root.path.join("wanxiang.schema.yaml").exists());
    assert!(root.path.join("wanxiang.custom.yaml").is_file());
    assert!(root.path.join("custom_phrase.dict.yaml").is_file());
    assert_eq!(
        fs::read_to_string(root.path.join("wanxiang-dicts/jichu.dict.yaml")).unwrap(),
        "user modifications"
    );
    assert_eq!(
        fs::read_to_string(root.path.join("wanxiang.userdb/00001.log")).unwrap(),
        "learned"
    );
}

#[test]
fn failed_uninstall_can_restore_previous_installation() {
    let root = TestRoot::new();
    let mut receipt = InstallReceipt::new(&root.path).unwrap();
    let files = stage_fixture(&receipt, &[]);
    commit_fixture(&mut receipt, files).unwrap();
    receipt.finish().unwrap();
    let removal = uninstall(&root.path).unwrap();
    assert!(!status_at(&root.path).installed);
    removal.rollback().unwrap();
    assert!(status_at(&root.path).installed);
}

#[test]
fn untrusted_manifest_cannot_delete_outside_package() {
    let root = TestRoot::new();
    let manifest = Manifest {
        format: 1,
        version: VERSION.into(),
        variant: "base".into(),
        source: SOURCE_URL.into(),
        archive_sha256: DOWNLOAD_SHA256.into(),
        includes_grammar: false,
        files: vec![InstalledFile {
            path: "default.custom.yaml".into(),
            sha256: "a".repeat(64),
            size: 0,
        }],
    };
    fs::write(
        root.path.join(MANIFEST),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    fs::write(root.path.join("default.custom.yaml"), "user-owned").unwrap();
    assert!(uninstall(&root.path).is_err());
    assert_eq!(
        fs::read_to_string(root.path.join("default.custom.yaml")).unwrap(),
        "user-owned"
    );
}

#[cfg(windows)]
#[test]
fn public_sources_contain_official_content_instead_of_private_override() {
    let root = TestRoot::new();
    fs::write(
        root.path.join("wanxiang.custom.yaml"),
        "private customization",
    )
    .unwrap();
    let mut receipt = InstallReceipt::new(&root.path).unwrap();
    let files = stage_fixture(&receipt, &[("wanxiang.custom.yaml", "patch: {}")]);
    commit_fixture(&mut receipt, files).unwrap();
    let public = receipt.public_source_files();
    let (_, source) = public
        .iter()
        .find(|(p, _)| *p == PathBuf::from("wanxiang.schema.yaml"))
        .unwrap();
    assert_eq!(fs::read_to_string(source).unwrap(), BASIC_SCHEMA);
    assert!(
        !public
            .iter()
            .any(|(p, _)| *p == PathBuf::from("wanxiang.custom.yaml"))
    );
    assert_eq!(
        fs::read_to_string(root.path.join("wanxiang.custom.yaml")).unwrap(),
        "private customization"
    );
}

/// Opt-in integration check using a locally cached upstream ZIP.
/// All Rime files, logs and learned dictionaries stay in a fresh temporary root.
#[test]
#[ignore = "requires the official archive and a local librime SDK"]
fn official_package_deploys_and_inputs_without_grammar_model() {
    let archive = PathBuf::from(std::env::var("KEYTAO_WANXIANG_SMOKE_ARCHIVE").unwrap());
    let shared = std::env::var("KEYTAO_WANXIANG_SMOKE_SHARED").unwrap();
    let variant = std::env::var("KEYTAO_WANXIANG_SMOKE_VARIANT").unwrap_or_else(|_| "base".into());
    let mut root = TestRoot::new();
    // librime deliberately keeps its process-global runtime/log handles alive
    // after ImeRuntime drops. Retain this opt-in smoke's evidence for inspection
    // and remove it from the invoking process after the test process exits.
    root.cleanup = false;
    println!("wanxiang_smoke_root={}", root.path.display());
    let mut receipt = InstallReceipt::new(&root.path).unwrap();
    let files = extract(
        &archive,
        &receipt.work.join("files"),
        &fixture_package(),
        &variant,
    )
    .unwrap();
    commit_files(
        &mut receipt,
        files,
        &fixture_package(),
        DOWNLOAD_SHA256.into(),
        &variant,
    )
    .unwrap();
    fs::write(
        root.path.join("default.custom.yaml"),
        "patch:\n  schema_list:\n    - schema: wanxiang\n",
    )
    .unwrap();
    // Register this isolated schema as a local primary for runtime readiness.
    fs::write(
        root.path.join(keytao_core::ACTIVE_SCHEME_MANIFEST),
        serde_json::to_vec(&serde_json::json!({
            "root": root.path.canonicalize().unwrap(),
            "scheme_key": "local-wanxiang-smoke",
            "manifest": {"wanxiang.schema.yaml": fingerprint(&root.path.join("wanxiang.schema.yaml")).unwrap().0},
        }))
        .unwrap(),
    )
    .unwrap();
    let runtime = keytao_core::ImeRuntime::with_dirs(&root.path, shared);
    runtime.init().unwrap();
    let session = runtime.create_session().unwrap();
    session.select_schema(SCHEMA_ID).unwrap();
    let mut inputs = if variant_source(&variant).unwrap() == "wanxiang_pro" {
        vec![("nihk", "你好"), ("vsgo", "中国"), ("wjan", "晚安")]
    } else {
        vec![("nihao", "你好"), ("zhongguo", "中国"), ("wanan", "晚安")]
    };
    match variant.as_str() {
        "pure" => inputs.push(("ni/re", "你")),
        "flypy" => inputs.push(("vsll", "中")),
        "moqi" => inputs.push(("vskl", "中")),
        _ => {}
    }
    for (input, expected) in inputs {
        session.clear_composition();
        let mut state = session.state();
        for character in input.chars() {
            state = session
                .process_key_result(character as u32, 0)
                .unwrap()
                .state;
        }
        let candidates: Vec<_> = state.candidates.iter().map(|c| c.text.as_str()).collect();
        println!("wanxiang variant={variant} input={input} candidates={candidates:?}");
        assert!(
            candidates.contains(&expected),
            "missing {expected}: {candidates:?}"
        );
    }
    assert!(status_at(&root.path).deployed);
    assert_eq!(status_at(&root.path).variant, variant);
    assert!(!root.path.join("wanxiang-lts-zh-hans.gram").exists());
    assert!(!root.path.join("lua").exists());
    let schema = fs::read_to_string(root.path.join("build/wanxiang.schema.yaml")).unwrap();
    assert!(!schema.contains("lua_processor"));
    assert!(!schema.contains("lua_translator"));
    assert!(!schema.contains("lua_filter"));
    drop(session);
    drop(runtime);
    receipt.finish().unwrap();
}
