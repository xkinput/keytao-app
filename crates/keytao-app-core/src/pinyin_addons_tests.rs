use super::*;
use crate::{
    addon::write_schema_list_transaction,
    wanxiang::{self, InstallReceipt},
};
use std::{
    collections::BTreeMap,
    fs,
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
    for stem in [id, "melt_eng", "radical_pinyin"] {
        entries.insert(
            format!("{stem}.schema.yaml"),
            format!("schema:\n  schema_id: {stem}\ntranslator:\n  dictionary: {stem}\n"),
        );
        entries.insert(
            format!("{stem}.dict.yaml"),
            format!("---\nname: {stem}\n...\nfixture\tfix\t1\n"),
        );
    }
    entries.insert("LICENSE".into(), "GPL-3.0-only fixture".into());
    entries.insert("opencc/emoji.txt".into(), "fixture".into());
    entries.insert("opencc/others.txt".into(), "fixture".into());
    let tables: &[(&str, &str)] = match kind {
        Package::Mint => &[
            ("wubi98_mint.schema.yaml", "schema:\n  schema_id: wubi98_mint\n"),
            ("wubi98_mint.dict.yaml", "---\nname: wubi98_mint\nimport_tables:\n  - dicts/wubi98_base\n...\n"),
            ("stroke.schema.yaml", "schema:\n  schema_id: stroke\n  dependencies: [luna_pinyin]\ntranslator:\n  dictionary: stroke\n"),
            ("stroke.dict.yaml", "---\nname: stroke\n...\n"),
            ("symbols.yaml", "symbols: {}\n"),
            ("opencc/spoken.txt", "fixture"),
            ("dicts/rime_mint.base.dict.yaml", "base"),
            ("dicts/rime_ice.en.dict.yaml", "english"),
            ("dicts/rime_ice.cn_en.txt", "fixture"),
            ("dicts/wubi98_base.dict.yaml", "wubi"),
            ("dicts/wubi86_core.dict.yaml", "wubi86"),
        ],
        Package::Frost => &[
            ("rime_frost_aux.schema.yaml", "schema:\n  schema_id: rime_frost_aux\n"),
            ("rime_frost_aux.dict.yaml", "---\nname: rime_frost_aux\n...\n"),
            ("symbols_v.yaml", "symbols: {}\n"),
            ("zh-moqi.gram", "grammar"),
            ("lua/aux_code/moqi_aux_code.txt", "aux"),
            ("cn_dicts/base.dict.yaml", "base"),
            ("cn_dicts/corrections.dict.yaml", "corrections"),
            ("cn_dicts/41448.dict.yaml", "41448"),
            ("cn_dicts/tencent.dict.yaml", "tencent"),
            ("cn_dicts_cell/cell.dict.yaml", "cell"),
            ("en_dicts/en.dict.yaml", "english"),
            ("en_dicts/cn_en.txt", "cn_en"),
        ],
    };
    for (name, content) in tables {
        entries.insert((*name).into(), (*content).into());
    }
    entries.insert(
        format!("{id}.dict.yaml"),
        match kind {
            Package::Mint => "---\nname: rime_mint\nimport_tables:\n  - dicts/rime_mint.base\n...\n",
            Package::Frost => "---\nname: rime_frost\nimport_tables:\n  - cn_dicts/base\n  - cn_dicts/41448\n  - cn_dicts/tencent\n  - cn_dicts_cell/cell\n...\n",
        }
        .into(),
    );
    entries.insert(
        "melt_eng.dict.yaml".into(),
        match kind {
            Package::Mint => "---\nname: melt_eng\nimport_tables:\n  - dicts/rime_ice.en\n...\n",
            Package::Frost => "---\nname: melt_eng\nimport_tables:\n  - en_dicts/en\n...\n",
        }
        .into(),
    );
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
    for module in ["select_character", "reduce_english_filter", "corrector", "aux_lookup_filter"] {
        entries.insert(
            format!("lua/{module}.lua"),
            match module {
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
    entries.insert("default.yaml".into(), "schema_list:\n  - schema: rime_mint\n".into());
    entries.insert("squirrel.yaml".into(), "never install host config".into());
    entries.insert("weasel.yaml".into(), "never install host config".into());
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
    stage_receipt(kind, InstallReceipt::new(root).unwrap(), version)
}

fn stage_receipt(kind: Package, mut receipt: InstallReceipt, version: &str) -> InstallReceipt {
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
fn both_packages_preserve_generic_rime_layout_in_isolated_roots() {
    for kind in [Package::Mint, Package::Frost] {
        let root = TestRoot::new();
        stage(kind, root.path(), "first").finish().unwrap();
        let addon = kind.addon();
        assert!(addon.status_at(root.path()).installed);
        let schema =
            fs::read_to_string(root.path().join(format!("{}.schema.yaml", addon.id))).unwrap();
        for expected in [
            format!("schema_id: {}", addon.id),
            "dictionary: melt_eng".into(),
            "- radical_pinyin".into(),
            "lua_processor@*select_character".into(),
            "lua_filter@*reduce_english_filter".into(),
            "opencc_config: emoji.json".into(),
        ] {
            assert!(schema.contains(&expected), "missing {expected}: {schema}");
        }
        for name in ["default.yaml", "squirrel.yaml", "weasel.yaml"] {
            assert!(!root.path().join(name).exists(), "{name}");
        }
        assert!(
            fs::read_to_string(root.path().join(format!("{}-NOTICE.txt", addon.id)))
                .unwrap()
                .contains("GPL-3.0-only")
        );
        if matches!(kind, Package::Mint) {
            assert!(schema.contains("import_preset: symbols"));
            assert!(schema.contains("dictionary: stroke"));
            assert!(root.path().join("dicts/rime_mint.base.dict.yaml").is_file());
            assert!(
                fs::read_to_string(root.path().join("opencc/emoji.json"))
                    .unwrap()
                    .contains("spoken.txt")
            );
        } else {
            assert!(schema.contains("symbols_v:/symbols") && schema.contains("custom_phrase"));
            assert!(root.path().join("zh-moqi.gram").is_file());
            assert!(
                fs::read_to_string(root.path().join("lua/corrector.lua"))
                    .unwrap()
                    .contains("/cn_dicts/corrections.dict.yaml")
            );
            assert!(
                fs::read_to_string(root.path().join("lua/aux_lookup_filter.lua"))
                    .unwrap()
                    .contains("/lua/aux_code/moqi_aux_code.txt")
            );
        }
        addon.uninstall(root.path()).unwrap().finish().unwrap();
        assert!(!addon.status_at(root.path()).installed);
    }
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
                .join(format!("{}-NOTICE.txt", kind.addon().id))
                .exists()
        );
    }
}

fn stage_deployable(kind: Package, mut receipt: InstallReceipt, version: &str) -> InstallReceipt {
    let addon = kind.addon();
    let names = [
        format!("{}.schema.yaml", addon.id),
        format!("{}.dict.yaml", addon.id),
        "melt_eng.schema.yaml".into(),
        "melt_eng.dict.yaml".into(),
        "radical_pinyin.schema.yaml".into(),
        "radical_pinyin.dict.yaml".into(),
        "stroke.schema.yaml".into(),
        "stroke.dict.yaml".into(),
    ];
    let files = names
        .iter()
        .map(|name| {
            let content = if let Some(id) = name.strip_suffix(".schema.yaml") {
                format!(
                    "schema: {{schema_id: {id}, name: Fixture {version}}}\n\
                     engine: {{translators: [table_translator]}}\n\
                     translator: {{dictionary: {id}, enable_user_dict: false}}\n"
                )
            } else if let Some(id) = name.strip_suffix(".dict.yaml") {
                format!("---\nname: {id}\nversion: {version}\n...\n词\tci\n")
            } else {
                "fixture".into()
            };
            let path = receipt.work.join("files").join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, content).unwrap();
            let (sha256, size) = wanxiang::fingerprint(&path).unwrap();
            InstalledFile {
                path: name.clone(),
                sha256,
                size,
            }
        })
        .collect();
    addon
        .commit_files(&mut receipt, files, &package(kind, version), "a".repeat(64))
        .unwrap();
    receipt
}

#[tokio::test]
async fn addon_only_install_update_failure_and_uninstall_use_deploy_transactions() {
    use crate::scheme_host::tests::AddonHost;
    use keytao_core::{ReloadStamp, RuntimeDirs};
    use std::sync::atomic::Ordering;

    for kind in [Package::Mint, Package::Frost] {
        let root = TestRoot::new();
        let host = AddonHost::new(root.path());
        let addon = kind.addon();
        host.write_bundled_keytao();
        let status = addon
            .manage(&host.core, true, &host, |receipt| async move {
                Ok(stage_deployable(kind, receipt, "first"))
            })
            .await
            .unwrap();
        assert!(status.installed && status.deployed);
        assert_eq!(host.deploys.load(Ordering::SeqCst), 1);
        assert!(!root.path().join("build").exists());
        assert_eq!(
            RuntimeDirs::active_package_key(root.path()).as_deref(),
            Some(addon.id),
        );
        assert_eq!(
            keytao_core::deployed_schema_id_from_dir(root.path()).as_deref(),
            Some(addon.id)
        );
        let stored = crate::addon_store::isolated_files(&host.core, addon.id).unwrap();
        let schema = stored.join(format!("{}.schema.yaml", addon.id));
        let first_schema = fs::read(&schema).unwrap();
        host.fail_deploys.store(1, Ordering::SeqCst);
        let error = addon
            .manage(&host.core, true, &host, |receipt| async move {
                Ok(stage_deployable(kind, receipt, "second"))
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("fixture deploy failure"));
        assert_eq!(
            addon.status(&host.core, root.path()).unwrap().version,
            "first"
        );
        assert_eq!(fs::read(&schema).unwrap(), first_schema);
        assert!(!root.path().join(addon.manifest).exists());
        let status = addon
            .manage(&host.core, true, &host, |receipt| async move {
                Ok(stage_deployable(kind, receipt, "second"))
            })
            .await
            .unwrap();
        assert_eq!(status.version, "second");
        let official = crate::addon_store::public_files(&host.core, addon.id).unwrap();
        assert!(
            official
                .iter()
                .any(|(_, path)| path == Path::new(&format!("{}.schema.yaml", addon.id)))
        );
        let runtime = RuntimeDirs::isolated_user_data_dir(root.path(), addon.id).unwrap();
        let private = runtime.join(format!("{}.dict.yaml", addon.id));
        let userdb = runtime.join(format!("{}.userdb", addon.id));
        fs::write(&private, "private dictionary").unwrap();
        fs::write(&userdb, "learned").unwrap();
        let reload = ReloadStamp::path(root.path());
        let previous_reload = fs::read(&reload).unwrap();
        let status = addon
            .manage(&host.core, false, &host, |_| async { unreachable!() })
            .await
            .unwrap();
        assert!(!status.installed && !status.deployed);
        assert_eq!(
            fs::read_to_string(private).unwrap(),
            "private dictionary"
        );
        assert_eq!(
            fs::read_to_string(userdb).unwrap(),
            "learned"
        );
        assert!(crate::addon_store::public_files(&host.core, addon.id).is_err());
        assert_ne!(fs::read(reload).unwrap(), previous_reload);
        assert_eq!(
            RuntimeDirs::active_package_key(root.path()).as_deref(),
            Some("builtin/keytao"),
        );
        assert_eq!(
            keytao_core::deployed_schema_id_from_dir(root.path()).as_deref(),
            Some("keytao"),
        );
    }
}

#[test]
fn unsafe_archives_and_expansion_limits_are_rejected() {
    for kind in [Package::Mint, Package::Frost] {
        let root = TestRoot::new();
        for (name, content) in [
            ("../escape.txt", "unsafe"),
            ("lua/CON.lua", "unsafe"),
            ("lua/evil.lua:stream", "unsafe"),
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
            crate::generic_rime::extract_archive(
                &archive,
                &root.path().join("small"),
                Some(kind.addon().id),
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

#[cfg(target_os = "macos")]
#[tokio::test]
async fn interactive_manage_defers_deploy_only_when_schema_count_exceeds_three() {
    use crate::scheme_host::tests::AddonHost;
    use std::sync::atomic::Ordering;

    for kind in [Package::Mint, Package::Frost] {
        let root = TestRoot::new();
        let host = AddonHost::new(root.path());
        let addon = kind.addon();
        host.write_bundled_keytao();
        for version in ["initial", "update"] {
            let status = addon
                .manage_interactive(&host.core, true, &host, |receipt| async move {
                    Ok(stage_deployable(kind, receipt, version))
                })
                .await
                .unwrap();
            let schemas = crate::package_schemas::package_schemas(&host.core, addon.id).unwrap();
            assert!(schemas.len() > 3, "{} schemas: {}", addon.id, schemas.len());
            assert!(status.installed);
            assert_eq!(status.version, version);
            assert!(!status.deployed);
            assert_eq!(host.deploys.load(Ordering::SeqCst), 0);
        }
    }
}
