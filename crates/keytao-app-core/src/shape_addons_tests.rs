use super::*;
use crate::{addon::write_schema_list_transaction, wanxiang::InstallReceipt};
use std::{
    collections::BTreeMap,
    io::{Cursor, Write},
    path::PathBuf,
};
use zip::{ZipWriter, write::SimpleFileOptions};

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

fn package(kind: Package, version: &str) -> RemotePackage {
    RemotePackage {
        version: version.into(),
        source: format!("{}/commit/{COMMIT}", kind.source()),
        download_url: "https://example.invalid/fixture.zip".into(),
        download_bytes: None,
    }
}

fn zip_at(path: &Path, prefix: &str, entries: BTreeMap<String, String>) {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.set_comment(COMMIT);
    for (name, text) in entries {
        zip.start_file(format!("{prefix}{name}"), SimpleFileOptions::default())
            .unwrap();
        zip.write_all(text.as_bytes()).unwrap();
    }
    fs::write(path, zip.finish().unwrap().into_inner()).unwrap();
}

#[test]
fn snow_preparation_preserves_source_defaults_instead_of_replacing_them() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap_or_else(|error| error.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let host = crate::scheme_host::tests::AddonHost::new(directory.path());
    fs::create_dir_all(&host.core.env().cache_dir).unwrap();
    let path = fixture(Package::Snow, &host.core.env().cache_dir);
    let mut archive = zip::ZipArchive::new(Cursor::new(fs::read(&path).unwrap())).unwrap();
    let mut entries = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let mut content = String::new();
        std::io::Read::read_to_string(&mut entry, &mut content).unwrap();
        entries.insert(entry.name().to_owned(), content);
    }
    entries.insert("default.custom.yaml".into(), "patch: {schema_list: [{schema: snow_jiandao}, {schema: snow_pinyin}]}".into());
    zip_at(&path, "", entries);
    let prepared = crate::rime_snow::prepare_archive(&host.core, fs::read(path).unwrap()).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(prepared)).unwrap();
    let mut defaults = String::new();
    std::io::Read::read_to_string(&mut archive.by_name("default.custom.yaml").unwrap(), &mut defaults).unwrap();
    assert_eq!(crate::scheme_files::parse_schema_list(&defaults), ["snow_jiandao", "snow_pinyin"]);
    assert_eq!(archive.file_names().filter(|name| *name == "default.custom.yaml").count(), 1);
}

#[test]
fn snow_preparation_lists_every_schema_yaml_for_selection() {
    let _serial = crate::wanxiang::TEST_SERIAL.lock().unwrap_or_else(|error| error.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let host = crate::scheme_host::tests::AddonHost::new(directory.path());
    fs::create_dir_all(&host.core.env().cache_dir).unwrap();
    let path = fixture(Package::Snow, &host.core.env().cache_dir);
    let mut archive = zip::ZipArchive::new(Cursor::new(fs::read(&path).unwrap())).unwrap();
    let mut entries = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let mut content = String::new();
        std::io::Read::read_to_string(&mut entry, &mut content).unwrap();
        entries.insert(entry.name().to_owned(), content);
    }
    let extra_schemas = [
        ("snow_bushou", "冰雪部首"),
        ("snow_lingpin", "冰雪零拼"),
        ("snow_qingyun", "冰雪清韵"),
        ("snow_qingyun_chaifen", "冰雪清韵拆分"),
        ("snow_qingyun_xingma", "冰雪清韵形码"),
        ("snow_sipin", "冰雪四拼"),
        ("snow_xingpang", "冰雪形旁"),
        ("snow_yingpin", "冰雪映拼"),
        ("snow_yipin", "冰雪一拼"),
    ];
    for (id, name) in extra_schemas {
        entries.insert(
            format!("{id}.schema.yaml"),
            format!("schema:\n  schema_id: {id}\n  name: {name}\ntranslator:\n  dictionary: snow_pinyin\n"),
        );
    }
    zip_at(&path, "", entries);
    let prepared = crate::rime_snow::prepare_archive(&host.core, fs::read(path).unwrap()).unwrap();
    crate::packages::store_cloud_archive(
        &host.core,
        "snow_jiandao",
        Some("v0.3.14"),
        "official",
        None,
        &prepared,
        &host.root,
    )
    .unwrap();
    let listed = crate::package_schemas::package_schemas(&host.core, "snow_jiandao").unwrap();
    let ids: Vec<_> = listed.iter().map(|s| s.id.as_str()).collect();
    for expected in [
        "snow_jiandao",
        "snow_pinyin",
        "snow_bihua",
        "snow_jiandao_bihua",
        "snow_jiandao_jianpin",
        "snow_jiandao_chaifen",
        "snow_sanpin",
        "radical_pinyin",
        "snow_bushou",
        "snow_lingpin",
        "snow_qingyun",
        "snow_qingyun_chaifen",
        "snow_qingyun_xingma",
        "snow_sipin",
        "snow_xingpang",
        "snow_yingpin",
        "snow_yipin",
    ] {
        assert!(ids.contains(&expected), "missing schema {expected} in {ids:?}");
    }
    assert_eq!(listed.len(), 17);
}

fn fixture(kind: Package, work: &Path) -> PathBuf {
    let mut entries = BTreeMap::new();
    for name in kind.addon().required {
        let source: String = match kind {
            Package::Snow => (*name).into(),
            Package::Lssp if *name == "triple_pinyin_lssp-LICENSE.txt" => "LICENSE".into(),
            Package::Lssp if name.starts_with("lssp_") || name.starts_with("lssp-") => continue,
            Package::Lssp => (*name).into(),
        };
        let text = if let Some(stem) = source.strip_suffix(".schema.yaml") {
            format!("schema:\n  schema_id: {stem}\ntranslator:\n  dictionary: {stem}\n")
        } else if let Some(stem) = source.strip_suffix(".dict.yaml") {
            format!("---\nname: {stem}\n...\n字\tzi4\t1\n")
        } else {
            "fixture data".into()
        };
        entries.insert(source, text);
    }
    entries.insert(
        "default.custom.yaml".into(),
        "never install host settings".into(),
    );
    entries.insert(
        "grammar.custom.yaml".into(),
        "never install a global model".into(),
    );
    let prefix = match kind {
        Package::Snow => {
            for stem in [
                "snow_pinyin",
                "snow_bihua",
                "snow_jiandao_bihua",
                "snow_jiandao_jianpin",
                "snow_jiandao_chaifen",
                "snow_sanpin",
            ] {
                entries.insert(
                    format!("{stem}.schema.yaml"),
                    format!("schema:\n  schema_id: {stem}\ntranslator:\n  dictionary: snow_pinyin\n"),
                );
            }
            for dict in [
                "snow_pinyin",
                "snow_pinyin.base",
                "snow_pinyin.ext",
                "snow_pinyin.tencent",
                "snow_pinyin.user",
                "snow_bihua",
                "snow_jiandao_chaifen",
                "radical_pinyin",
            ] {
                entries.insert(
                    format!("{dict}.dict.yaml"),
                    format!("---\nname: {dict}\n...\n字\tzi4\t1\n"),
                );
            }
            entries.insert("lua/snow/snow.lua".into(), "return {}".into());
            entries.insert("lua/snow/radical_jiandao.txt".into(), "fixture".into());
            entries.insert("snow_jiandao.fixed.txt".into(), "字\tj\n".into());
            entries.insert("radical_pinyin.schema.yaml".into(), "schema:\n  schema_id: radical_pinyin\n  description: https://github.com/mirtlecn/rime_radical_pinyin\ntranslator:\n  dictionary: radical_pinyin\n".into());
            entries.insert("snow_jiandao.schema.yaml".into(), "schema:\n  schema_id: snow_jiandao\n  dependencies: [snow_pinyin, radical_pinyin]\nengine:\n  filters: [lua_filter@*snow.snow]\ntranslator:\n  dictionary: snow_pinyin\nradical:\n  dictionary: radical_pinyin\n  prism: radical_pinyin\n".into());
            entries.insert("snow_sipin.schema.yaml".into(), "schema:\n  schema_id: snow_sipin\n  name: 冰雪四拼\ntranslator:\n  dictionary: snow_pinyin\n".into());
            entries.insert(
                "snow_sanpin.fixed.txt".into(),
                "字\tsan\n".into(),
            );
            entries.insert("squirrel.yaml".into(), "never install host config".into());
            ""
        }
        Package::Lssp => {
            for module in LSSP_LUA {
                entries.insert(
                    format!("lua/{module}.lua"),
                    "return '/stroke.dict.yaml'".into(),
                );
            }
            entries.insert("lua/unrelated.lua".into(), "omit global Lua modules".into());
            entries.insert(
                "triple_pinyin_lsspaeo.schema.yaml".into(),
                "omit alternative layout".into(),
            );
            entries.insert("triple_pinyin_lssp.schema.yaml".into(), "schema:\n  schema_id: triple_pinyin_lssp\n  dependencies:\n    - pinyin_simp\n    - stroke\nengine:\n  processors:\n    - lua_processor@*limited\n  translators:\n    - reverse_lookup_translator@pinyin_lookup\n    - reverse_lookup_translator@stroke_lookup\n    - lua_translator@*triple_pinyin_lssp\ntranslator:\n  dictionary: triple_pinyin\npreedit:\n  __patch: {__append: [one]}\n  __patch: {__append: [two]}\npinyin_lookup:\n  dictionary: pinyin_simp\n  comment_format: &comment [same]\nstroke_lookup:\n  dictionary: stroke\n  comment_format: *comment\npunctuator:\n  import_preset: symbols\nrecognizer:\n  patterns:\n    pinyin_lookup: '`[a-z]*'\n__patch:\n  - grammar:/triple_pinyin_hans?\n".into());
            entries.insert("triple_pinyin.dict.yaml".into(), format!("---\nname: triple_pinyin\nvocabulary: essay-zh-hans\nimport_tables:\n{}  - triple_pinyin.additional\n  - triple_pinyin.ddpy\n  - triple_pinyin.zianyin\n...", LSSP_TABLES.iter().map(|t| format!("  - dicts/{t}\n")).collect::<String>()));
            let dictionaries = LSSP_TABLES
                .iter()
                .map(|t| {
                    (
                        format!("dicts/{t}.dict.yaml"),
                        format!("---\nname: {t}\n...\n字\tzì\t1\n"),
                    )
                })
                .collect();
            zip_at(&work.join("wanxiang.zip"), "", dictionaries);
            zip_at(&work.join("stroke.zip"), "rime-stroke-master/", BTreeMap::from([
                ("stroke.schema.yaml".into(), "schema:\n  schema_id: stroke\n  dependencies: [luna_pinyin]\nengine:\n  translators: [table_translator, reverse_lookup_translator]\ntranslator:\n  dictionary: stroke\nreverse_lookup:\n  dictionary: luna_pinyin\n".into()),
                ("stroke.dict.yaml".into(), "---\nname: stroke\n...\n字\thsp\t1\n".into()),
                ("LICENSE".into(), "LGPL-3.0 fixture".into()),
            ]));
            "rime-triple-pinyin-lssp-main/"
        }
    };
    let archive = work.join("fixture.zip");
    zip_at(&archive, prefix, entries);
    archive
}

fn stage(kind: Package, mut receipt: InstallReceipt, version: &str) -> InstallReceipt {
    let archive = fixture(kind, &receipt.work);
    let package = package(kind, version);
    let files = kind
        .extract(&archive, &receipt.work.join("files"), &package)
        .unwrap();
    assert!(files.iter().all(|f| kind.installable(&f.path)));
    kind.addon()
        .commit_files(
            &mut receipt,
            files,
            &package,
            wanxiang::fingerprint(&archive).unwrap().0,
        )
        .unwrap();
    receipt
}

#[test]
fn version_lookup_parses_only_the_expected_release_or_commit() {
    let snow =
        crate::rime_snow::package_from_location("/rimeinn/rime-snow-pinyin/releases/tag/v0.3.14")
            .unwrap();
    assert_eq!(snow.version, "v0.3.14");
    assert!(snow.download_url.ends_with("/v0.3.14/snow-pinyin.zip"));
    for url in [
        "https://evil.invalid/rimeinn/rime-snow-pinyin/releases/tag/v1",
        "/other/releases/tag/v1",
        "/rimeinn/rime-snow-pinyin/releases/tag/v1?x=y",
    ] {
        assert!(crate::rime_snow::package_from_location(url).is_err());
    }
    let atom = format!(
        "<feed><entry><id>tag:github.com,2008:Grit::Commit/{COMMIT}</id><updated>2026-11-18T00:00:00Z</updated></entry></feed>"
    );
    let lssp = crate::rime_lssp::package_from_atom(&atom).unwrap();
    assert_eq!(lssp.version, "0123456789ab (2026-11-18)");
    assert!(
        lssp.download_url
            .ends_with("/arsenali/rime-triple-pinyin-lssp/zip/refs/heads/main")
    );
    assert!(crate::rime_lssp::package_from_atom("<feed/>").is_err());
}

#[test]
fn packages_are_isolated_and_uninstall_preserves_customizations_and_learning() {
    let _serial = wanxiang::TEST_SERIAL
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for kind in [Package::Snow, Package::Lssp] {
        let root = tempfile::tempdir().unwrap();
        let preserved = [
            "stroke.dict.yaml",
            "wanxiang-dicts/zi.dict.yaml",
            "lua/limited.lua",
        ];
        for name in preserved {
            let path = root.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "another package").unwrap();
        }
        stage(kind, InstallReceipt::new(root.path()).unwrap(), "first")
            .finish()
            .unwrap();
        let addon = kind.addon();
        assert!(addon.status_at(root.path()).installed);
        let custom = format!("{}.custom.yaml", addon.id);
        let learned = format!("{}.userdb", addon.id);
        fs::write(root.path().join(&custom), "private settings").unwrap();
        fs::create_dir(root.path().join(&learned)).unwrap();
        for omitted in [
            "default.custom.yaml",
            "grammar.custom.yaml",
            "squirrel.yaml",
            "triple_pinyin_lsspaeo.schema.yaml",
            "lua/unrelated.lua",
        ] {
            assert!(!root.path().join(omitted).exists(), "{omitted}");
        }
        if matches!(kind, Package::Snow) {
            assert!(root.path().join("snow_sipin.schema.yaml").is_file());
            assert!(root.path().join("snow_sanpin.fixed.txt").is_file());
            assert!(root.path().join("snow_pinyin.user.dict.yaml").is_file());
            let schema = fs::read_to_string(root.path().join("snow_jiandao.schema.yaml")).unwrap();
            assert!(
                schema.contains("dictionary: radical_pinyin")
                    && schema.contains("prism: radical_pinyin")
            );
            assert!(
                fs::read_to_string(root.path().join("radical_pinyin.schema.yaml"))
                    .unwrap()
                    .contains("https://github.com/mirtlecn/rime_radical_pinyin")
            );
        } else {
            let schema =
                fs::read_to_string(root.path().join("triple_pinyin_lssp.schema.yaml")).unwrap();
            assert!(schema.contains("lua_processor@*lssp.limited"));
            assert!(schema.contains("dictionary: lssp_stroke"));
            assert!(
                !schema.contains("grammar:")
                    && !schema.contains("reverse_lookup_translator@pinyin_lookup")
            );
            assert!(
                schema.contains("__patch: {__append: [one]}")
                    && schema.contains("__patch: {__append: [two]}")
            );
            assert!(
                fs::read_to_string(root.path().join("lssp_stroke.dict.yaml"))
                    .unwrap()
                    .contains("name: lssp_stroke")
            );
            assert!(
                !fs::read_to_string(root.path().join("triple_pinyin.dict.yaml"))
                    .unwrap()
                    .contains("zianyin")
            );
            let dictionary =
                fs::read_to_string(root.path().join("triple_pinyin.dict.yaml")).unwrap();
            assert!(dictionary.contains("vocabulary: essay\n"));
            assert!(!dictionary.contains("essay-zh-hans"));
            assert!(
                fs::read_to_string(root.path().join("lua/lssp/aux_code_stroke.lua"))
                    .unwrap()
                    .contains("/lssp_stroke.dict.yaml")
            );
        }
        addon.uninstall(root.path()).unwrap().finish().unwrap();
        assert!(!addon.status_at(root.path()).installed);
        assert_eq!(
            fs::read_to_string(root.path().join(custom)).unwrap(),
            "private settings"
        );
        assert!(root.path().join(learned).is_dir());
        for name in preserved {
            assert_eq!(
                fs::read_to_string(root.path().join(name)).unwrap(),
                "another package"
            );
        }
    }
}

#[test]
fn updates_registration_and_uninstall_roll_back_and_reject_unowned_conflicts() {
    let _serial = wanxiang::TEST_SERIAL
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for kind in [Package::Snow, Package::Lssp] {
        let root = tempfile::tempdir().unwrap();
        let addon = kind.addon();
        stage(kind, InstallReceipt::new(root.path()).unwrap(), "first")
            .finish()
            .unwrap();
        let manifest = fs::read(root.path().join(addon.manifest)).unwrap();
        {
            let mut receipt = stage(kind, InstallReceipt::new(root.path()).unwrap(), "second");
            write_schema_list_transaction(&mut receipt, addon.id, true, root.path()).unwrap();
            assert_eq!(addon.status_at(root.path()).version, "second");
        }
        assert_eq!(
            fs::read(root.path().join(addon.manifest)).unwrap(),
            manifest
        );
        {
            let _receipt = addon.uninstall(root.path()).unwrap();
            assert!(!addon.status_at(root.path()).installed);
        }
        assert!(addon.status_at(root.path()).installed);
        let notice = format!("{}-NOTICE.txt", addon.id);
        fs::write(root.path().join(&notice), "private modification").unwrap();
        let mut receipt = InstallReceipt::new(root.path()).unwrap();
        let archive = fixture(kind, &receipt.work);
        let package = package(kind, "second");
        let files = kind
            .extract(&archive, &receipt.work.join("files"), &package)
            .unwrap();
        assert!(
            addon
                .commit_files(&mut receipt, files, &package, "a".repeat(64))
                .unwrap_err()
                .contains("冲突")
        );
        drop(receipt);
        assert_eq!(
            fs::read_to_string(root.path().join(notice)).unwrap(),
            "private modification"
        );
    }
}

#[test]
fn lssp_archive_must_match_its_advertised_commit() {
    let root = tempfile::tempdir().unwrap();
    let archive = fixture(Package::Lssp, root.path());
    let mut package = package(Package::Lssp, "first");
    package.source = format!("{}/commit/{}", Package::Lssp.source(), "a".repeat(40));
    assert!(
        Package::Lssp
            .extract(&archive, &root.path().join("stage"), &package)
            .unwrap_err()
            .contains("main 分支")
    );
    assert!(!root.path().join("stage").exists());
}

#[cfg(not(any(target_os = "android", target_os = "windows")))]
fn legacy_snow_fixture(root: &Path) -> crate::scheme_host::tests::AddonHost {
    let host = crate::scheme_host::tests::AddonHost::new(root);
    fs::create_dir_all(host.core.env().resource_dir.join("rime-data")).unwrap();
    fs::write(
        host.core.env().resource_dir.join("rime-data/default.yaml"),
        "{}",
    )
    .unwrap();
    fs::write(
        root.join("keytao.schema.yaml"),
        "schema:\n  schema_id: keytao\n",
    )
    .unwrap();
    fs::write(
        root.join("default.custom.yaml"),
        "patch:\n  schema_list:\n    - schema: keytao\n",
    )
    .unwrap();
    let mut receipt = stage(Package::Snow, InstallReceipt::new(root).unwrap(), "v1");
    write_schema_list_transaction(
        &mut receipt,
        SNOW.id,
        true,
        &host.core.env().resource_dir.join("rime-data"),
    )
    .unwrap();
    receipt.finish().unwrap();
    crate::addon_store::migrate(&host.core, root).unwrap();
    host
}

#[cfg(not(any(target_os = "android", target_os = "windows")))]
#[test]
fn snow_package_listing_ignores_modified_legacy_source_after_migration() {
    let _serial = wanxiang::TEST_SERIAL
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let root = tempfile::tempdir().unwrap();
    let host = legacy_snow_fixture(root.path());
    let store = crate::package_store::PackageStore::new(&host.core);
    let legacy_schema = store.addon_path(SNOW.id).join("files/snow_jiandao.schema.yaml");
    let mut bytes = fs::read(&legacy_schema).unwrap();
    bytes[0] ^= 1;
    fs::write(&legacy_schema, bytes).unwrap();

    let installed = crate::packages::list_installed_packages(&host.core).unwrap();
    assert!(installed.iter().any(|package| package.scheme_key == SNOW.id));
}

#[cfg(not(any(target_os = "android", target_os = "windows")))]
#[tokio::test]
async fn snow_builtin_migration_keeps_newer_package_and_prevents_reinstall_after_removal() {
    let _serial = wanxiang::TEST_SERIAL
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let root = tempfile::tempdir().unwrap();
    let host = legacy_snow_fixture(root.path());
    crate::addon_store::migrate_snow_package(&host.core).unwrap();
    let store = crate::package_store::PackageStore::new(&host.core);
    let (files, mut meta) = store.read_files(SNOW.id).unwrap();
    meta.version = Some("v2".into());
    store
        .write_archive(
            Cursor::new(crate::packages::archive_from_files(&files).unwrap()),
            &meta,
            None,
        )
        .unwrap();
    crate::addon_store::migrate_snow_package(&host.core).unwrap();
    assert_eq!(
        host.core.state().installed_packages[SNOW.id]
            .version
            .as_deref(),
        Some("v2")
    );
    let runtime = root.path().join("runtime");
    fs::create_dir(&runtime).unwrap();
    fs::write(
        root.path().join(keytao_core::RuntimeDirs::MIGRATION_MARKER),
        br#"{"version":1,"runtime_package":"builtin/keytao"}"#,
    ).unwrap();
    assert!(host.core.state().active_scheme.is_none());
    crate::packages::remove_scheme_package(&host.core, SNOW.id, &host)
        .await
        .unwrap();
    assert!(!store.package_path(SNOW.id).exists());
    assert!(!store.addon_path(SNOW.id).exists());
    assert!(root.path().join("keytao.schema.yaml").exists());
    for path in SNOW_REQUIRED
        .iter()
        .copied()
        .chain(["snow_jiandao-NOTICE.txt"])
    {
        assert!(root.path().join(path).exists(), "legacy source lost before backup: {path}");
    }
    crate::addon_store::migrate_snow_package(&host.core).unwrap();
    assert!(!host.core.state().installed_packages.contains_key(SNOW.id));
}

#[cfg(not(any(target_os = "android", target_os = "windows")))]
#[test]
fn snow_builtin_migration_recovers_legacy_ownership_after_interrupted_takeover() {
    let _serial = wanxiang::TEST_SERIAL
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let root = tempfile::tempdir().unwrap();
    let host = legacy_snow_fixture(root.path());
    crate::addon_store::migrate_snow_package(&host.core).unwrap();
    let original = crate::addon_store::snow_activation(&host.core, root.path())
        .unwrap()
        .unwrap();
    let paths = original
        .owned
        .iter()
        .chain(&original.generated)
        .map(|file| file.path.clone())
        .collect();
    let mut transaction =
        crate::package_activation::Transaction::begin(&host.core, root.path(), paths).unwrap();
    transaction
        .snapshot_store_file("addons/snow_jiandao/activation.json")
        .unwrap();
    crate::addon_store::take_over_snow(&host.core, root.path()).unwrap();
    assert!(!root.path().join("snow_jiandao.schema.yaml").exists());
    std::mem::forget(transaction);
    drop(host);
    let recovered = crate::scheme_host::tests::AddonHost::new(root.path());
    let restored = crate::addon_store::snow_activation(&recovered.core, root.path())
        .unwrap()
        .unwrap();
    assert_eq!(restored.owned.len(), original.owned.len());
    for file in original.owned.iter().chain(&original.generated) {
        assert_eq!(
            wanxiang::fingerprint(&root.path().join(&file.path))
                .unwrap()
                .0,
            file.sha256
        );
    }
}

#[tokio::test]
async fn isolated_addon_update_rollback_and_uninstall_preserve_package_learning() {
    use crate::scheme_host::tests::AddonHost;
    let _serial = wanxiang::TEST_SERIAL.lock().unwrap_or_else(|error| error.into_inner());
    for kind in [Package::Snow, Package::Lssp] {
        let directory = tempfile::tempdir().unwrap();
        let host = AddonHost::new(directory.path());
        let addon = kind.addon();
        let learned = directory.path().join(format!("runtime/{}/{}.userdb", addon.id, addon.id));
        fs::create_dir_all(learned.parent().unwrap()).unwrap();
        fs::write(&learned, "learned").unwrap();
        let prepare = |version| {
            let mut receipt = InstallReceipt::new(directory.path()).unwrap();
            receipt.attach_store(&host.core, addon.id).unwrap();
            let mut receipt = stage(kind, receipt, version);
            receipt.commit_store(true).unwrap();
            receipt
        };
        prepare("first").finish().unwrap();
        assert!(addon.status(&host.core, directory.path()).unwrap().installed);
        let updated = prepare("second");
        assert_eq!(addon.status(&host.core, directory.path()).unwrap().version, "second");
        updated.rollback().unwrap();
        assert_eq!(addon.status(&host.core, directory.path()).unwrap().version, "first");

        let public = crate::addon_store::public_files(&host.core, addon.id).unwrap();
        assert!(public.iter().any(|(_, path)| path == Path::new(&format!("{}.schema.yaml", addon.id))));
        let status = addon.manage(&host.core, false, &host, |_| async { unreachable!() }).await.unwrap();
        assert!(!status.installed);
        assert_eq!(fs::read_to_string(&learned).unwrap(), "learned");
        assert!(crate::addon_store::public_files(&host.core, addon.id).is_err());
    }
}

fn prepare_official_package(kind: Package) {
    let scratch = PathBuf::from(std::env::var("KEYTAO_SHAPE_SMOKE_SCRATCH").unwrap())
        .canonicalize()
        .unwrap();
    assert!(scratch.starts_with("/private/tmp"));
    let archive = PathBuf::from(std::env::var("KEYTAO_SHAPE_SMOKE_ARCHIVE").unwrap());
    let shared = PathBuf::from(std::env::var("KEYTAO_SHAPE_SMOKE_SHARED").unwrap());
    let _serial = wanxiang::TEST_SERIAL
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let root = tempfile::Builder::new()
        .prefix(kind.addon().id)
        .tempdir_in(&scratch)
        .unwrap();
    let mut receipt = InstallReceipt::new(root.path()).unwrap();
    let local_archive = receipt.work.join("package.zip");
    fs::copy(&archive, &local_archive).unwrap();
    let mut package = package(kind, "isolated-official-smoke");
    if matches!(kind, Package::Lssp) {
        let zip = zip::ZipArchive::new(File::open(&archive).unwrap()).unwrap();
        package.source = format!(
            "{}/commit/{}",
            kind.source(),
            std::str::from_utf8(zip.comment()).unwrap()
        );
        for (variable, name) in [
            ("KEYTAO_SHAPE_SMOKE_WANXIANG", "wanxiang.zip"),
            ("KEYTAO_SHAPE_SMOKE_STROKE", "stroke.zip"),
        ] {
            fs::copy(std::env::var(variable).unwrap(), receipt.work.join(name)).unwrap();
        }
    }
    let files = kind
        .extract(&local_archive, &receipt.work.join("files"), &package)
        .unwrap();
    kind.addon()
        .commit_files(
            &mut receipt,
            files,
            &package,
            wanxiang::fingerprint(&archive).unwrap().0,
        )
        .unwrap();
    write_schema_list_transaction(&mut receipt, kind.addon().id, true, &shared).unwrap();
    receipt.finish().unwrap();
    let root = root.keep();
    println!("{}_smoke_root={}", kind.addon().id, root.display());
    assert!(kind.addon().status_at(&root).installed);
}

#[test]
#[ignore = "requires official local archives; prepares an isolated package without running its Lua"]
fn official_snow_package_prepares_in_isolated_scratch() {
    prepare_official_package(Package::Snow);
}

#[test]
#[ignore = "requires official local archives; prepares an isolated package without running its Lua"]
fn official_lssp_package_prepares_in_isolated_scratch() {
    prepare_official_package(Package::Lssp);
}
