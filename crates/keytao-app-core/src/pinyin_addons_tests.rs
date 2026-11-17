use super::*;
use crate::{addon::write_schema_list_transaction, wanxiang::InstallReceipt};
use std::{
    collections::BTreeMap,
    io::{Cursor, Write},
    path::PathBuf,
    sync::MutexGuard,
};
use zip::{ZipWriter, write::SimpleFileOptions};

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

struct TestRoot {
    dir: tempfile::TempDir,
    _serial: MutexGuard<'static, ()>,
}
impl TestRoot {
    fn new() -> Self {
        let serial = wanxiang::TEST_SERIAL
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        Self {
            dir: tempfile::tempdir().unwrap(),
            _serial: serial,
        }
    }
    fn path(&self) -> &Path {
        self.dir.path()
    }
}

fn package(kind: Package, version: &str) -> RemotePackage {
    RemotePackage {
        version: version.into(),
        source: match kind {
            Package::Mint => format!("{}/commit/{COMMIT}", kind.source()),
            Package::Frost => kind.source().into(),
        },
        download_url: "https://example.invalid/fixture.zip".into(),
        download_bytes: None,
    }
}

fn archive_at(kind: Package, path: &Path, extra: &[(&str, &str)]) {
    let id = kind.addon().id;
    let mut entries = BTreeMap::<String, String>::new();
    for name in kind.addon().required {
        let source = if name == &format!("{id}-LICENSE.txt") {
            "LICENSE".into()
        } else if let Some(table) = name.strip_prefix(&format!("{id}-dicts/")) {
            match kind {
                Package::Mint => format!("dicts/{table}"),
                Package::Frost => table.into(),
            }
        } else if let Some(module) = name.strip_prefix(&format!("lua/{id}/")) {
            format!("lua/{module}")
        } else if let Some(file) = name.strip_prefix(&format!("opencc/{id}_")) {
            format!("opencc/{file}")
        } else if name.starts_with(&format!("{id}_")) && !name.starts_with("rime_frost_aux.") {
            name.strip_prefix(&format!("{id}_")).unwrap().into()
        } else {
            (*name).into()
        };
        let content = if let Some(stem) = source.strip_suffix(".schema.yaml") {
            format!("schema:\n  schema_id: {stem}\ntranslator:\n  dictionary: {stem}\n")
        } else if let Some(stem) = source.strip_suffix(".dict.yaml") {
            format!("---\nname: {stem}\n...\nfixture\tfix\t1\n")
        } else if source.ends_with(".json") {
            let file = source
                .trim_start_matches("opencc/")
                .replace(".json", ".txt");
            format!("{{\"dict\":{{\"file\":\"{file}\"}}}}")
        } else {
            "fixture data".into()
        };
        entries.insert(source, content);
    }
    let tables: &[(&str, &str)] = match kind {
        Package::Mint => &[
            ("dicts/rime_mint.base.dict.yaml", "base"),
            ("dicts/rime_ice.en.dict.yaml", "english"),
            ("dicts/wubi98_base.dict.yaml", "wubi"),
            ("dicts/wubi86_core.dict.yaml", "omit"),
        ],
        Package::Frost => &[
            ("cn_dicts/base.dict.yaml", "base"),
            ("cn_dicts/41448.dict.yaml", "omit"),
            ("cn_dicts/tencent.dict.yaml", "omit"),
            ("cn_dicts_cell/cell.dict.yaml", "omit"),
            ("en_dicts/en.dict.yaml", "english"),
            ("en_dicts/cn_en_flypy.txt", "omit"),
        ],
    };
    for (name, content) in tables {
        entries.insert((*name).into(), (*content).into());
    }
    entries.insert(format!("{id}.dict.yaml"), match kind {
        Package::Mint => "---\nname: rime_mint\nimport_tables:\n  - dicts/rime_mint.base\n...\n",
        Package::Frost => "---\nname: rime_frost\nimport_tables:\n  - cn_dicts/base\n  - cn_dicts/41448\n  - cn_dicts/tencent\n  - cn_dicts_cell/cell\n...\n",
    }.into());
    entries.insert(
        "melt_eng.dict.yaml".into(),
        match kind {
            Package::Mint => "---\nname: melt_eng\nimport_tables:\n  - dicts/rime_ice.en\n...\n",
            Package::Frost => "---\nname: melt_eng\nimport_tables:\n  - en_dicts/en\n...\n",
        }
        .into(),
    );
    if matches!(kind, Package::Mint) {
        entries.insert(
            "wubi98_mint.dict.yaml".into(),
            "---\nname: wubi98_mint\nimport_tables:\n  - dicts/wubi98_base\n...\n".into(),
        );
        entries.insert("stroke.schema.yaml".into(), "schema:\n  schema_id: stroke\n  dependencies: [luna_pinyin]\ntranslator:\n  dictionary: stroke\n".into());
    }
    let mut schema = format!(
        "schema:\n  schema_id: {id}\n  dependencies:\n    - melt_eng\n    - radical_pinyin\n"
    );
    if matches!(kind, Package::Mint) {
        schema.push_str("    - stroke\n    - wubi98_mint\n");
    } else {
        schema.push_str("    - rime_frost_aux\n");
    }
    schema.push_str("engine:\n  processors:\n    - lua_processor@*select_character\n  translators:\n    - table_translator@melt_eng\n  filters:\n    - lua_filter@*reduce_english_filter\ntranslator:\n  dictionary: ");
    schema.push_str(id);
    schema.push_str("\nmelt_eng:\n  dictionary: melt_eng\nradical_reverse_lookup:\n  dictionary: radical_pinyin\nstroke:\n  dictionary: stroke\nemoji:\n  opencc_config: emoji.json\ncn_en:\n  user_dict: ");
    schema.push_str(match kind {
        Package::Mint => "dicts/rime_ice.cn_en",
        Package::Frost => "en_dicts/cn_en",
    });
    schema.push_str("\ncustom_phrase:\n  user_dict: custom_phrase\npunctuator:\n");
    schema.push_str(match kind {
        Package::Mint => "  import_preset: symbols\n",
        Package::Frost => "  __include: default:/punctuator/half_shape\n  symbols:\n    __include: symbols_v:/symbols\ngrammar:\n  language: zh-moqi\n",
    });
    entries.insert(format!("{id}.schema.yaml"), schema);
    for module in kind.lua() {
        entries.insert(
            format!("lua/{module}.lua"),
            match *module {
                "corrector" => "return '/cn_dicts/corrections.dict.yaml'".into(),
                "aux_lookup_filter" => "return '/lua/aux_code/moqi_aux_code.txt'".into(),
                _ => "return {}".into(),
            },
        );
    }
    entries.insert(
        "opencc/emoji.json".into(),
        match kind {
            Package::Mint => {
                r#"{"dicts":[{"file":"emoji.txt"},{"file":"others.txt"},{"file":"spoken.txt"}]}"#
            }
            Package::Frost => r#"{"dicts":[{"file":"emoji.txt"},{"file":"others.txt"}]}"#,
        }
        .into(),
    );
    entries.insert("default.yaml".into(), "never install host config".into());
    entries.insert("squirrel.yaml".into(), "never install host config".into());
    entries.insert("lua/log.lua".into(), "never install debugging".into());
    entries.insert(
        "lua/cold_word_drop/drop_words.lua".into(),
        "never install optional modules".into(),
    );
    entries.insert(
        "opencc/s2t.json".into(),
        "use vendored standard OpenCC".into(),
    );
    for (name, content) in extra {
        entries.insert((*name).into(), (*content).into());
    }
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.set_comment(COMMIT);
    for (name, content) in entries {
        let name = match kind {
            Package::Mint => format!("oh-my-rime-main/{name}"),
            Package::Frost => name,
        };
        zip.start_file(name, SimpleFileOptions::default()).unwrap();
        zip.write_all(content.as_bytes()).unwrap();
    }
    fs::write(path, zip.finish().unwrap().into_inner()).unwrap();
}

fn stage(kind: Package, root: &Path, version: &str) -> InstallReceipt {
    let mut receipt = InstallReceipt::new(root).unwrap();
    let archive = receipt.work.join("fixture.zip");
    archive_at(kind, &archive, &[]);
    let package = package(kind, version);
    let files = kind
        .extract(&archive, &receipt.work.join("files"), &package)
        .unwrap();
    assert!(files.iter().all(|file| kind.installable(&file.path)));
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
fn atom_and_release_redirect_versions_do_not_use_the_api() {
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let atom = format!(
        "<feed><entry><id>tag:github.com,2008:Grit::Commit/{sha}</id><updated>2026-11-14T10:00:00Z</updated></entry><entry><id>invalid</id></entry></feed>"
    );
    let mint = crate::rime_mint::package_from_atom(&atom).unwrap();
    assert_eq!(mint.version, "0123456789ab (2026-11-14)");
    assert_eq!(
        mint.download_url,
        "https://codeload.github.com/Mintimate/oh-my-rime/zip/refs/heads/main"
    );
    assert!(mint.source.ends_with(sha));
    for feed in [
        "<feed/>",
        "<feed><entry><id>bad</id></entry></feed>",
        "<feed><entry><id>tag:github.com,2008:Grit::Commit/short</id><updated>2026-11-14</updated></entry></feed>",
    ] {
        assert!(crate::rime_mint::package_from_atom(feed).is_err());
    }
    for location in [
        "/gaboolic/rime-frost/releases/tag/1.0.4",
        "https://github.com/gaboolic/rime-frost/releases/tag/1.0.4",
    ] {
        let frost = crate::rime_frost::package_from_location(location).unwrap();
        assert_eq!(frost.version, "1.0.4");
        assert!(
            frost
                .download_url
                .ends_with("/1.0.4/rime-frost-schemas.zip")
        );
    }
    for location in [
        "https://evil.invalid/gaboolic/rime-frost/releases/tag/1.0.4",
        "/other/releases/tag/1.0.4",
        "/gaboolic/rime-frost/releases/tag/../bad",
        "/gaboolic/rime-frost/releases/tag/1.0.4?x=y",
        "/gaboolic/rime-frost/releases/tag/1.0.4%2fextra",
    ] {
        assert!(
            crate::rime_frost::package_from_location(location).is_err(),
            "{location}"
        );
    }
}

#[test]
fn mint_branch_archive_must_match_the_advertised_commit() {
    let root = TestRoot::new();
    let archive = root.path().join("fixture.zip");
    archive_at(Package::Mint, &archive, &[]);
    let mut package = package(Package::Mint, "first");
    package.source = format!(
        "https://github.com/Mintimate/oh-my-rime/commit/{}",
        "a".repeat(40)
    );
    let error = Package::Mint
        .extract(&archive, &root.path().join("stage"), &package)
        .unwrap_err();
    assert!(error.contains("main 分支在下载期间发生变化"));
    assert!(!root.path().join("stage").exists());
}

#[test]
fn both_packages_namespace_every_dependency_and_coexist_with_rime_ice() {
    let root = TestRoot::new();
    let preserved = [
        "melt_eng.schema.yaml",
        "radical_pinyin.dict.yaml",
        "stroke.dict.yaml",
        "symbols.yaml",
        "opencc/emoji.json",
        "opencc/rime_ice_emoji.json",
        "lua/rime_ice/select_character.lua",
        "rime_ice-dicts/cn_dicts/base.dict.yaml",
    ];
    for name in preserved {
        let path = root.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "other installed scheme").unwrap();
    }
    for kind in [Package::Mint, Package::Frost] {
        stage(kind, root.path(), "first").finish().unwrap();
        let addon = kind.addon();
        assert!(addon.status_at(root.path()).installed);
        let schema =
            fs::read_to_string(root.path().join(format!("{}.schema.yaml", addon.id))).unwrap();
        for expected in [
            format!("schema_id: {}", addon.id),
            format!("dictionary: {}_melt_eng", addon.id),
            format!("- {}_radical_pinyin", addon.id),
            format!(
                "lua_processor@*{}/select_character@select_character",
                addon.id
            ),
            format!(
                "lua_filter@*{}/reduce_english_filter@reduce_english_filter",
                addon.id
            ),
            format!("opencc_config: {}_emoji.json", addon.id),
        ] {
            assert!(schema.contains(&expected), "missing {expected}: {schema}");
        }
        for name in [
            "default.yaml",
            "squirrel.yaml",
            "lua/log.lua",
            "opencc/s2t.json",
        ] {
            assert!(!root.path().join(name).exists(), "{name}");
        }
        let dictionary =
            fs::read_to_string(root.path().join(format!("{}.dict.yaml", addon.id))).unwrap();
        assert!(
            !dictionary.contains("cn_dicts_cell")
                && !dictionary.contains("41448")
                && !dictionary.contains("tencent")
        );
        assert!(
            fs::read_to_string(root.path().join(format!("{}-NOTICE.txt", addon.id)))
                .unwrap()
                .contains("GPL-3.0-only")
        );
    }
    let mint = fs::read_to_string(root.path().join("rime_mint.schema.yaml")).unwrap();
    assert!(mint.contains("import_preset: rime_mint_symbols"));
    assert!(mint.contains("dictionary: rime_mint_stroke"));
    let stroke = fs::read_to_string(root.path().join("rime_mint_stroke.schema.yaml")).unwrap();
    assert!(!stroke.contains("luna_pinyin"));
    assert!(
        fs::read_to_string(root.path().join("opencc/rime_mint_emoji.json"))
            .unwrap()
            .contains("rime_mint_spoken.txt")
    );
    let frost = fs::read_to_string(root.path().join("rime_frost.schema.yaml")).unwrap();
    assert!(
        frost.contains("rime_frost_symbols_v:/symbols")
            && frost.contains("rime_frost_custom_phrase")
    );
    assert!(
        fs::read_to_string(root.path().join("lua/rime_frost/corrector.lua"))
            .unwrap()
            .contains("/rime_frost-dicts/cn_dicts/corrections.dict.yaml")
    );
    assert!(
        fs::read_to_string(root.path().join("lua/rime_frost/aux_lookup_filter.lua"))
            .unwrap()
            .contains("/lua/rime_frost/aux_code/moqi_aux_code.txt")
    );
    for name in preserved {
        assert_eq!(
            fs::read_to_string(root.path().join(name)).unwrap(),
            "other installed scheme"
        );
    }
    MINT.uninstall(root.path()).unwrap().finish().unwrap();
    assert!(FROST.status_at(root.path()).installed);
}

#[test]
fn update_uninstall_and_registration_roll_back_and_preserve_private_files() {
    let root = TestRoot::new();
    for kind in [Package::Mint, Package::Frost] {
        let addon = kind.addon();
        stage(kind, root.path(), "first").finish().unwrap();
        let manifest = fs::read(root.path().join(addon.manifest)).unwrap();
        fs::write(
            root.path().join("default.custom.yaml"),
            "patch:\n  schema_list:\n    - schema: keytao\n    - schema: rime_ice\n",
        )
        .unwrap();
        let config = fs::read(root.path().join("default.custom.yaml")).unwrap();
        {
            let mut receipt = stage(kind, root.path(), "second");
            write_schema_list_transaction(&mut receipt, addon.id, true, root.path()).unwrap();
            assert_eq!(addon.status_at(root.path()).version, "second");
        }
        assert_eq!(
            fs::read(root.path().join(addon.manifest)).unwrap(),
            manifest
        );
        assert_eq!(
            fs::read(root.path().join("default.custom.yaml")).unwrap(),
            config
        );
        {
            let _receipt = addon.uninstall(root.path()).unwrap();
            assert!(!addon.status_at(root.path()).installed);
        }
        assert!(addon.status_at(root.path()).installed);
        stage(kind, root.path(), "second").finish().unwrap();
        let custom = format!("{}.custom.yaml", addon.id);
        let learned = format!("{}.userdb", addon.id);
        fs::write(root.path().join(&custom), "private settings").unwrap();
        fs::create_dir(root.path().join(&learned)).unwrap();
        let notice = format!("{}-NOTICE.txt", addon.id);
        fs::write(root.path().join(&notice), "private changes").unwrap();
        addon.uninstall(root.path()).unwrap().finish().unwrap();
        assert_eq!(
            fs::read_to_string(root.path().join(custom)).unwrap(),
            "private settings"
        );
        assert!(root.path().join(learned).is_dir());
        assert_eq!(
            fs::read_to_string(root.path().join(notice)).unwrap(),
            "private changes"
        );
        assert!(!root.path().join(addon.manifest).exists());
    }
}

#[test]
fn modified_owned_sources_prevent_update_writes() {
    for kind in [Package::Mint, Package::Frost] {
        let root = TestRoot::new();
        stage(kind, root.path(), "first").finish().unwrap();
        let addon = kind.addon();
        let manifest = fs::read(root.path().join(addon.manifest)).unwrap();
        fs::write(
            root.path().join(format!("{}-NOTICE.txt", addon.id)),
            "private changes",
        )
        .unwrap();
        let mut receipt = InstallReceipt::new(root.path()).unwrap();
        let archive = receipt.work.join("fixture.zip");
        archive_at(kind, &archive, &[]);
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
            fs::read(root.path().join(addon.manifest)).unwrap(),
            manifest
        );
    }
}

#[test]
fn unowned_source_conflicts_leave_every_existing_file_untouched() {
    for kind in [Package::Mint, Package::Frost] {
        let root = TestRoot::new();
        let schema = format!("{}.schema.yaml", kind.addon().id);
        fs::write(root.path().join(&schema), "private source").unwrap();
        let mut receipt = InstallReceipt::new(root.path()).unwrap();
        let archive = receipt.work.join("fixture.zip");
        archive_at(kind, &archive, &[]);
        let package = package(kind, "first");
        let files = kind
            .extract(&archive, &receipt.work.join("files"), &package)
            .unwrap();
        assert!(
            kind.addon()
                .commit_files(&mut receipt, files, &package, "a".repeat(64))
                .unwrap_err()
                .contains("冲突")
        );
        drop(receipt);
        assert_eq!(
            fs::read_to_string(root.path().join(schema)).unwrap(),
            "private source"
        );
        assert!(!root.path().join(kind.addon().manifest).exists());
        assert!(
            !root
                .path()
                .join(format!("{}-LICENSE.txt", kind.addon().id))
                .exists()
        );
    }
}

#[tokio::test]
async fn addon_only_install_update_failure_and_uninstall_use_deploy_transactions() {
    use crate::scheme_host::tests::AddonHost;
    use std::sync::atomic::Ordering;
    for kind in [Package::Mint, Package::Frost] {
        let root = TestRoot::new();
        let host = AddonHost::new(root.path());
        let addon = kind.addon();
        fs::write(root.path().join("rime.lua"), "stale main lua").unwrap();
        let status = addon
            .manage(&host.core, true, &host, |root| async move {
                Ok(stage(kind, &root, "first"))
            })
            .await
            .unwrap();
        assert!(status.installed && status.deployed);
        assert_eq!(host.deploys.load(Ordering::SeqCst), 1);
        assert!(!root.path().join("rime.lua").exists());
        assert_eq!(
            keytao_core::deployed_schema_id_from_dir(root.path()).as_deref(),
            Some(addon.id)
        );
        host.fail_deploys.store(1, Ordering::SeqCst);
        let error = addon
            .manage(&host.core, true, &host, |root| async move {
                Ok(stage(kind, &root, "second"))
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("fixture deploy failure"));
        assert_eq!(addon.status_at(root.path()).version, "first");
        let status = addon
            .manage(&host.core, false, &host, |_| async { unreachable!() })
            .await
            .unwrap();
        assert!(!status.installed && !status.deployed);
        assert_eq!(host.reloads.load(Ordering::SeqCst), 1);
        assert_eq!(keytao_core::deployed_schema_id_from_dir(root.path()), None);
    }
}

#[test]
fn missing_imports_lua_opencc_and_unsafe_archives_are_rejected() {
    for kind in [Package::Mint, Package::Frost] {
        let root = TestRoot::new();
        let schema = format!("{}.schema.yaml", kind.addon().id);
        for (name, content) in [
            ("../escape.txt", "unsafe"),
            ("opencc/emoji.json", r#"{"dict":{"file":"missing.txt"}}"#),
            (
                "melt_eng.dict.yaml",
                "---\nname: melt_eng\nimport_tables:\n  - missing\n...\n",
            ),
            (
                schema.as_str(),
                "schema: {}\nengine:\n  filters:\n    - lua_filter@*missing_module\n",
            ),
            (
                schema.as_str(),
                "schema: {}\nemoji:\n  opencc_config: missing.json\n",
            ),
        ] {
            let work = tempfile::tempdir_in(root.path()).unwrap();
            let archive = work.path().join("fixture.zip");
            archive_at(kind, &archive, &[(name, content)]);
            assert!(
                kind.extract(
                    &archive,
                    &work.path().join("stage"),
                    &package(kind, "first")
                )
                .is_err(),
                "{name}"
            );
        }
        let archive = root.path().join("fixture.zip");
        archive_at(kind, &archive, &[]);
        assert!(
            wanxiang::extract_package_with_limit(
                &archive,
                &root.path().join("small"),
                |name| kind.destination(name),
                |name, path| kind.adapt(name, path),
                1
            )
            .is_err()
        );
        assert!(
            !root
                .path()
                .join(format!("{}.schema.yaml", kind.addon().id))
                .exists()
        );
    }
}

fn official_deploy(kind: Package) {
    let archive = PathBuf::from(std::env::var("KEYTAO_PINYIN_SMOKE_ARCHIVE").unwrap());
    let shared = PathBuf::from(std::env::var("KEYTAO_PINYIN_SMOKE_SHARED").unwrap());
    let scratch = PathBuf::from(std::env::var("KEYTAO_PINYIN_SMOKE_SCRATCH").unwrap())
        .canonicalize()
        .unwrap();
    assert!(scratch.starts_with("/private/tmp"));
    let serial = wanxiang::TEST_SERIAL
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let root = tempfile::Builder::new()
        .prefix(kind.addon().id)
        .tempdir_in(&scratch)
        .unwrap();
    let mut receipt = InstallReceipt::new(root.path()).unwrap();
    let mut package = package(kind, "isolated-official-smoke");
    if matches!(kind, Package::Mint) {
        let zip = zip::ZipArchive::new(File::open(&archive).unwrap()).unwrap();
        package.source = format!(
            "{}/commit/{}",
            kind.source(),
            std::str::from_utf8(zip.comment()).unwrap()
        );
    }
    let files = kind
        .extract(&archive, &receipt.work.join("files"), &package)
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
    let path = root.keep();
    println!("{}_smoke_root={}", kind.addon().id, path.display());
    keytao_core::deploy(
        path.to_string_lossy().into_owned(),
        shared.to_string_lossy().into_owned(),
    )
    .unwrap();
    assert!(kind.addon().status_at(&path).deployed);
    drop(serial);
}

#[test]
#[ignore = "requires the official local archive and vendored librime SDK; deploy only"]
fn official_mint_package_deploys_in_isolated_scratch() {
    official_deploy(Package::Mint);
}

#[test]
#[ignore = "requires the official local archive and vendored librime SDK; deploy only"]
fn official_frost_package_deploys_in_isolated_scratch() {
    official_deploy(Package::Frost);
}
