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

fn fixture(kind: Package, work: &Path) -> PathBuf {
    let mut entries = BTreeMap::new();
    for name in kind.addon().required {
        let source = match kind {
            Package::Snow => name
                .replace("snow_radical_pinyin", "radical_pinyin")
                .replace("snow_pinyin.user_pack", "snow_pinyin.user"),
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
            entries.insert("radical_pinyin.schema.yaml".into(), "schema:\n  schema_id: radical_pinyin\n  description: https://github.com/mirtlecn/rime_radical_pinyin\ntranslator:\n  dictionary: radical_pinyin\n".into());
            entries.insert("snow_jiandao.schema.yaml".into(), "schema:\n  schema_id: snow_jiandao\n  dependencies: [snow_pinyin, radical_pinyin]\nengine:\n  filters: [lua_filter@*snow.snow]\ntranslator:\n  dictionary: snow_pinyin\nradical:\n  dictionary: radical_pinyin\n  prism: radical_pinyin\n".into());
            entries.insert("snow_sipin.schema.yaml".into(), "omit other layouts".into());
            entries.insert(
                "snow_sanpin.fixed.txt".into(),
                "omit unused fixed data".into(),
            );
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
    let root = tempfile::tempdir().unwrap();
    let preserved = [
        "radical_pinyin.dict.yaml",
        "snow_pinyin.user.dict.yaml",
        "stroke.dict.yaml",
        "wanxiang-dicts/zi.dict.yaml",
        "lua/limited.lua",
    ];
    for name in preserved {
        let path = root.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "another package").unwrap();
    }
    for kind in [Package::Snow, Package::Lssp] {
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
            "snow_sipin.schema.yaml",
            "snow_sanpin.fixed.txt",
            "triple_pinyin_lsspaeo.schema.yaml",
            "lua/unrelated.lua",
        ] {
            assert!(!root.path().join(omitted).exists(), "{omitted}");
        }
        if matches!(kind, Package::Lssp) {
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
        } else {
            let schema = fs::read_to_string(root.path().join("snow_jiandao.schema.yaml")).unwrap();
            assert!(
                schema.contains("dictionary: snow_radical_pinyin")
                    && schema.contains("prism: snow_radical_pinyin")
            );
            assert!(
                fs::read_to_string(root.path().join("snow_radical_pinyin.schema.yaml"))
                    .unwrap()
                    .contains("https://github.com/mirtlecn/rime_radical_pinyin")
            );
        }
        addon.uninstall(root.path()).unwrap().finish().unwrap();
        assert!(!addon.status_at(root.path()).installed);
        assert_eq!(
            fs::read_to_string(root.path().join(custom)).unwrap(),
            "private settings"
        );
        assert!(root.path().join(learned).is_dir());
    }
    for name in preserved {
        assert_eq!(
            fs::read_to_string(root.path().join(name)).unwrap(),
            "another package"
        );
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

#[tokio::test]
async fn addon_only_install_update_failure_and_uninstall_use_the_shared_transaction() {
    use crate::scheme_host::tests::AddonHost;
    use std::sync::atomic::Ordering;
    let _serial = wanxiang::TEST_SERIAL
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for kind in [Package::Snow, Package::Lssp] {
        let root = tempfile::tempdir().unwrap();
        let host = AddonHost::new(root.path());
        let addon = kind.addon();
        let status = addon
            .manage(&host.core, true, &host, |receipt| async move {
                Ok(stage(kind, receipt, "first"))
            })
            .await
            .unwrap();
        assert!(status.installed && status.deployed);
        assert_eq!(
            keytao_core::deployed_schema_id_from_dir(root.path()).as_deref(),
            Some(addon.id)
        );
        host.fail_deploys.store(1, Ordering::SeqCst);
        assert!(
            addon
                .manage(&host.core, true, &host, |receipt| async move {
                    Ok(stage(kind, receipt, "second"))
                })
                .await
                .unwrap_err()
                .to_string()
                .contains("fixture deploy failure")
        );
        assert_eq!(
            addon.status(&host.core, root.path()).unwrap().version,
            "first"
        );
        let public = crate::addon_store::public_files(&host.core, addon.id).unwrap();
        assert!(
            public
                .iter()
                .any(|(_, name)| name == Path::new(&format!("{}.schema.yaml", addon.id)))
        );
        let learned = format!("{}.userdb", addon.id);
        fs::write(root.path().join(&learned), "learned").unwrap();
        let status = addon
            .manage(&host.core, false, &host, |_| async { unreachable!() })
            .await
            .unwrap();
        assert!(!status.installed && !status.deployed);
        assert_eq!(
            fs::read_to_string(root.path().join(learned)).unwrap(),
            "learned"
        );
        assert!(crate::addon_store::public_files(&host.core, addon.id).is_err());
        assert_eq!(host.reloads.load(Ordering::SeqCst), 1);
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
