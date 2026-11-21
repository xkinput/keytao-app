use super::*;
use std::{
    fs,
    io::{Cursor, Write},
    path::PathBuf,
    sync::MutexGuard,
};
use zip::{ZipWriter, write::SimpleFileOptions};

#[cfg(target_os = "macos")]
#[path = "../../keytao-core/tests/support/smoke_fixture.rs"]
mod smoke_fixture;

const ARCHIVE_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const RELEASES_ATOM_FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <id>tag:github.com,2008:https://github.com/iDvel/rime-ice/releases</id>
  <entry>
    <id>tag:github.com,2008:Repository/334110599/nightly</id>
    <link href="https://github.com/iDvel/rime-ice/releases/tag/nightly"/>
  </entry>
  <entry><id>tag:github.com,2008:Repository/334110599/2026.03.01</id></entry>
  <entry>
    <id>
      tag:github.com,2008:Repository/334110599/2026.06.30
    </id>
    <link href="https://github.com/iDvel/rime-ice/releases/tag/2026.06.30"/>
    <content type="html">&lt;a href="/releases/tag/2099.01.01"&gt;ignore&lt;/a&gt;</content>
  </entry>
  <entry><id>tag:github.com,2008:Repository/334110599/v2026.12.01</id></entry>
  <entry><id>tag:github.com,2008:Repository/334110599/2026.13.01</id></entry>
  <entry><id>tag:github.com,2008:Repository/334110599/2026.05.31</id></entry>
</feed>"#;
const UNDATED_ATOM_FIXTURE: &str = r#"<feed xmlns="http://www.w3.org/2005/Atom">
  <entry><id>tag:github.com,2008:Repository/334110599/nightly</id></entry>
  <entry><id>tag:github.com,2008:Repository/334110599/v1.0</id></entry>
</feed>"#;
const RELEASES_API_FIXTURE: &str = r#"[{"tag_name":"2026.06.30","assets":[{"name":"full.zip"}]}]"#;

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

fn package(version: &str) -> RemotePackage {
    RemotePackage {
        version: version.into(),
        source: format!("https://github.com/iDvel/rime-ice/tree/{version}"),
        download_url: "https://example.invalid/full.zip".into(),
        download_bytes: None,
    }
}

fn archive_at(path: &Path, extra: &[(&str, &str)]) {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let entries = [
        (
            "rime_ice.schema.yaml",
            "schema:\n  schema_id: rime_ice\nengine:\n  processors:\n    - lua_processor@*select_character\n  translators:\n    - lua_translator@*date_translator\n  filters:\n    - lua_filter@*reduce_english_filter\n    - lua_filter@*search@radical_pinyin\nemoji:\n  opencc_config: emoji.json\ncustom_phrase:\n  user_dict: custom_phrase\ncn_en:\n  user_dict: en_dicts/cn_en\npunctuator:\n  full_shape:\n    __include: default:/punctuator/full_shape\n",
        ),
        (
            "rime_ice.dict.yaml",
            "---\nname: rime_ice\nimport_tables:\n  - cn_dicts/base\n  - cn_dicts/tencent # optional\n  # - cn_dicts/41448\n...\n",
        ),
        ("melt_eng.schema.yaml", "schema:\n  schema_id: melt_eng\n"),
        (
            "melt_eng.dict.yaml",
            "---\nname: melt_eng\nimport_tables:\n  - en_dicts/en\n...\n",
        ),
        (
            "radical_pinyin.schema.yaml",
            "schema:\n  schema_id: radical_pinyin\n",
        ),
        (
            "radical_pinyin.dict.yaml",
            "---\nname: radical_pinyin\n...\n",
        ),
        ("symbols_v.yaml", "symbols: {}\n"),
        ("custom_phrase.txt", "fixture\tfix\t1\n"),
        (
            "opencc/emoji.json",
            "{\"dict\": {\"file\": \"emoji.txt\"}, \"others\": {\"file\": \"others.txt\"}}",
        ),
        ("opencc/emoji.txt", "fixture emoji"),
        ("opencc/others.txt", "fixture others"),
        ("LICENSE", "GPL-3.0-only fixture"),
        (
            "lua/date_translator.lua",
            "local _ = require(\"convert_ar_num_to_zh\")\nreturn function() end",
        ),
        ("lua/convert_ar_num_to_zh.lua", "return {}"),
        ("lua/select_character.lua", "return function() return 2 end"),
        ("lua/reduce_english_filter.lua", "return function(input) for cand in input:iter() do yield(cand) end end"),
        ("lua/search.lua", "return function(input) for cand in input:iter() do yield(cand) end end"),
        ("lua/lunar.lua", "return ReverseDb('lua/lunar.db')"),
        ("lua/lunar.db", "fixture database"),
        (
            "lua/cold_word_drop/processor.lua",
            "require(\"cold_word_drop.string\")\nlocal ok, words = pcall(require, \"cold_word_drop/drop_words\")\nreturn 'lua/cold_word_drop/drop_words.lua'",
        ),
        ("lua/cold_word_drop/string.lua", "return {}"),
        ("lua/cold_word_drop/drop_words.lua", "return {}"),
        ("cn_dicts/base.dict.yaml", "base dictionary"),
        ("cn_dicts/tencent.dict.yaml", "large optional dictionary"),
        ("cn_dicts/41448.dict.yaml", "optional dictionary"),
        ("en_dicts/en.dict.yaml", "english dictionary"),
        ("en_dicts/cn_en.txt", "fixture\tfix\t1\n"),
        (
            "default.yaml",
            "schema_list:\n  - schema: rime_ice\n  - schema: double_pinyin_flypy\nmenu:\n  page_size: 9\n",
        ),
        ("squirrel.yaml", "forbidden host settings"),
        ("weasel.yaml", "forbidden host settings"),
        (
            "double_pinyin_flypy.schema.yaml",
            "schema:\n  schema_id: double_pinyin_flypy\n  name: 小鹤双拼\n",
        ),
    ];
    for (name, content) in entries.into_iter().chain(extra.iter().copied()) {
        zip.start_file(name, SimpleFileOptions::default()).unwrap();
        zip.write_all(content.as_bytes()).unwrap();
    }
    fs::write(path, zip.finish().unwrap().into_inner()).unwrap();
}

fn stage(receipt: &InstallReceipt, package: &RemotePackage) -> Vec<InstalledFile> {
    let archive = receipt.work.join("fixture.zip");
    archive_at(&archive, &[]);
    extract(&archive, &receipt.work.join("files"), package).unwrap()
}

fn install_fixture(root: &Path, version: &str) {
    let mut receipt = InstallReceipt::new(root).unwrap();
    let package = package(version);
    let files = stage(&receipt, &package);
    commit_files(&mut receipt, files, &package, ARCHIVE_HASH.into()).unwrap();
    receipt.finish().unwrap();
}

#[test]
fn atom_discovers_newest_dated_release_and_builds_package_urls() {
    let release = release_package_from_atom(RELEASES_ATOM_FIXTURE).unwrap();
    assert_eq!(release.version, "2026.06.30");
    assert_eq!(
        release.source,
        "https://github.com/iDvel/rime-ice/tree/2026.06.30"
    );
    assert_eq!(
        release.download_url,
        "https://github.com/iDvel/rime-ice/releases/download/2026.06.30/full.zip"
    );
    assert_eq!(release.download_bytes, None);
}

#[test]
fn atom_without_dated_releases_returns_a_clear_error() {
    assert_eq!(
        release_package_from_atom(UNDATED_ATOM_FIXTURE)
            .err()
            .unwrap(),
        "雾凇拼音发布订阅中没有 YYYY.MM.DD 版本"
    );
}

#[tokio::test]
async fn atom_success_does_not_query_the_api() {
    let release = latest_package_with(|url| {
        assert_eq!(url, RELEASES_ATOM);
        std::future::ready(Ok(RELEASES_ATOM_FIXTURE.into()))
    })
    .await
    .unwrap();
    assert_eq!(release.version, "2026.06.30");
}

#[tokio::test]
async fn atom_fetch_or_parse_failure_falls_back_to_the_api_once() {
    for feed in [Err("HTTP 502".into()), Ok(UNDATED_ATOM_FIXTURE.into())] {
        let calls = std::cell::RefCell::new(Vec::new());
        let release = latest_package_with(|url| {
            calls.borrow_mut().push(url);
            std::future::ready(if url == RELEASES_ATOM {
                feed.clone()
            } else {
                Ok(RELEASES_API_FIXTURE.into())
            })
        })
        .await
        .unwrap();
        assert_eq!(release.version, "2026.06.30");
        assert_eq!(*calls.borrow(), [RELEASES_ATOM, RELEASES]);
    }
}

#[tokio::test]
async fn failed_api_fallback_preserves_both_errors_without_retrying() {
    for api in [Err("HTTP 403 rate limit exceeded".into()), Ok("[]".into())] {
        let calls = std::cell::RefCell::new(Vec::new());
        let error = latest_package_with(|url| {
            calls.borrow_mut().push(url);
            std::future::ready(if url == RELEASES_ATOM {
                Ok(UNDATED_ATOM_FIXTURE.into())
            } else {
                api.clone()
            })
        })
        .await
        .err()
        .unwrap();
        assert!(error.contains("没有 YYYY.MM.DD 版本"), "{error}");
        assert!(error.contains("API 回退"), "{error}");
        assert!(
            error.contains("HTTP 403 rate limit exceeded") || error.contains("full.zip"),
            "{error}"
        );
        assert_eq!(*calls.borrow(), [RELEASES_ATOM, RELEASES]);
    }
}

#[test]
fn discovers_newest_dated_full_release_without_the_mutable_nightly_tag() {
    let release = release_package(
        r#"[
        {"tag_name":"nightly","assets":[{"name":"full.zip"}]},
        {"tag_name":"2026.03.01","assets":[{"name":"full.zip"}]},
        {"tag_name":"2026.06.30","assets":[{"name":"full.zip"}]},
        {"tag_name":"2026.07.01","draft":true,"assets":[{"name":"full.zip"}]},
        {"tag_name":"2026.08.01","prerelease":true,"assets":[{"name":"full.zip"}]},
        {"tag_name":"2026.09.01","assets":[{"name":"other.zip"}]}
    ]"#,
    )
    .unwrap();
    assert_eq!(release.version, "2026.06.30");
    assert_eq!(
        release.download_url,
        "https://github.com/iDvel/rime-ice/releases/download/2026.06.30/full.zip"
    );
    for tag in [
        "nightly",
        "../../2026.01.01",
        "2026.00.12",
        "2026.13.01",
        "2026.01.00",
    ] {
        assert!(!valid_version(tag));
    }
    assert!(release_package(r#"[{"tag_name":"nightly","assets":[{"name":"full.zip"}]}]"#).is_err());
}

#[test]
fn installed_sources_keep_generic_rime_layout_and_skip_host_files() {
    let root = TestRoot::new();
    fs::write(root.path().join("default.yaml"), "existing default").unwrap();
    install_fixture(root.path(), "2026.06.30");
    assert!(status_at(root.path()).installed);
    assert!(!status_at(root.path()).deployed);
    let schema = fs::read_to_string(root.path().join("rime_ice.schema.yaml")).unwrap();
    for expected in [
        "lua_translator@*date_translator",
        "lua_processor@*select_character",
        "lua_filter@*reduce_english_filter",
        "lua_filter@*search@radical_pinyin",
        "opencc_config: emoji.json",
        "user_dict: custom_phrase",
        "en_dicts/cn_en",
    ] {
        assert!(schema.contains(expected), "{expected}");
    }
    assert!(
        fs::read_to_string(root.path().join("rime_ice.dict.yaml"))
            .unwrap()
            .contains("  - cn_dicts/base")
    );
    assert!(root.path().join("double_pinyin_flypy.schema.yaml").is_file());
    assert!(root.path().join("cn_dicts/tencent.dict.yaml").is_file());
    assert!(root.path().join("cn_dicts/41448.dict.yaml").is_file());
    for excluded in ["squirrel.yaml", "weasel.yaml"] {
        assert!(!root.path().join(excluded).exists(), "{excluded}");
    }
    for (name, expected) in [
        (
            "lua/date_translator.lua",
            "require(\"convert_ar_num_to_zh\")",
        ),
        ("lua/lunar.lua", "lua/lunar.db"),
        (
            "lua/cold_word_drop/processor.lua",
            "cold_word_drop.string",
        ),
        (
            "lua/cold_word_drop/processor.lua",
            "cold_word_drop/drop_words",
        ),
        ("opencc/emoji.json", "emoji.txt"),
        (NOTICE, "GPL-3.0-only"),
    ] {
        assert!(
            fs::read_to_string(root.path().join(name))
                .unwrap()
                .contains(expected),
            "{name}: {expected}"
        );
    }
    assert_eq!(
        fs::read_to_string(root.path().join("default.yaml")).unwrap(),
        "existing default"
    );
}

#[test]
fn conflict_preflight_preserves_unowned_files_and_existing_install() {
    let root = TestRoot::new();
    fs::write(root.path().join("melt_eng.schema.yaml"), "another package").unwrap();
    let mut receipt = InstallReceipt::new(root.path()).unwrap();
    let package = package("2026.06.30");
    let files = stage(&receipt, &package);
    assert!(
        commit_files(&mut receipt, files, &package, ARCHIVE_HASH.into())
            .unwrap_err()
            .contains("冲突")
    );
    drop(receipt);
    assert_eq!(
        fs::read_to_string(root.path().join("melt_eng.schema.yaml")).unwrap(),
        "another package"
    );
    assert!(!root.path().join("rime_ice.schema.yaml").exists());
    assert!(!root.path().join(MANIFEST).exists());
}

#[test]
fn updates_and_uninstalls_roll_back_and_preserve_user_files() {
    let root = TestRoot::new();
    install_fixture(root.path(), "2026.03.01");
    let original = fs::read(root.path().join(MANIFEST)).unwrap();
    let notice = fs::read(root.path().join(NOTICE)).unwrap();
    {
        let mut receipt = InstallReceipt::new(root.path()).unwrap();
        let package = package("2026.06.30");
        let files = stage(&receipt, &package);
        commit_files(&mut receipt, files, &package, ARCHIVE_HASH.into()).unwrap();
        assert_eq!(status_at(root.path()).version, "2026.06.30");
    }
    assert_eq!(fs::read(root.path().join(MANIFEST)).unwrap(), original);
    assert_eq!(fs::read(root.path().join(NOTICE)).unwrap(), notice);
    {
        let _receipt = uninstall(root.path()).unwrap();
        assert!(!status_at(root.path()).installed);
    }
    assert_eq!(fs::read(root.path().join(MANIFEST)).unwrap(), original);
    install_fixture(root.path(), "2026.06.30");
    fs::write(
        root.path().join("rime_ice_custom_phrase.txt"),
        "private phrase",
    )
    .unwrap();
    fs::write(root.path().join("rime_ice.custom.yaml"), "private custom").unwrap();
    fs::create_dir(root.path().join("rime_ice.userdb")).unwrap();
    uninstall(root.path()).unwrap().finish().unwrap();
    assert!(!root.path().join(MANIFEST).exists());
    assert!(!root.path().join("rime_ice.schema.yaml").exists());
    assert_eq!(
        fs::read_to_string(root.path().join("rime_ice_custom_phrase.txt")).unwrap(),
        "private phrase"
    );
    assert!(root.path().join("rime_ice.custom.yaml").is_file());
    assert!(root.path().join("rime_ice.userdb").is_dir());
}

#[test]
fn updates_refuse_locally_modified_owned_sources() {
    let root = TestRoot::new();
    install_fixture(root.path(), "2026.03.01");
    fs::write(root.path().join(NOTICE), "private edits").unwrap();
    let original_manifest = fs::read(root.path().join(MANIFEST)).unwrap();
    let mut receipt = InstallReceipt::new(root.path()).unwrap();
    let package = package("2026.06.30");
    let files = stage(&receipt, &package);
    assert!(commit_files(&mut receipt, files, &package, ARCHIVE_HASH.into()).is_err());
    drop(receipt);
    assert_eq!(
        fs::read(root.path().join(MANIFEST)).unwrap(),
        original_manifest
    );
    assert_eq!(
        fs::read_to_string(root.path().join(NOTICE)).unwrap(),
        "private edits"
    );
}

#[test]
fn schema_registration_preserves_primary_and_other_addons() {
    let root = TestRoot::new();
    let path = root.path().join("default.custom.yaml");
    fs::write(&path, "patch:\n  schema_list:\n    - schema: keytao\n    - schema: wanxiang\n    - schema: easy_en\n  menu/page_size: 7\n").unwrap();
    for installed in [true, true, false] {
        crate::addon::write_addon_schema_list(root.path(), SCHEMA_ID, installed).unwrap();
        let content = fs::read_to_string(&path).unwrap();
        let schemas = crate::scheme_files::parse_schema_list(&content);
        assert_eq!(
            schemas.iter().filter(|schema| *schema == SCHEMA_ID).count(),
            usize::from(installed)
        );
        assert!(schemas.iter().any(|schema| schema == "keytao"));
        assert!(schemas.iter().any(|schema| schema == "wanxiang"));
        assert!(schemas.iter().any(|schema| schema == "easy_en"));
        assert!(content.contains("menu/page_size: 7"));
    }
}

#[test]
fn dropping_receipt_restores_schema_list_and_sources_together() {
    let root = TestRoot::new();
    let config = root.path().join("default.custom.yaml");
    let original = "patch:\n  schema_list:\n    - schema: keytao\n    - schema: wanxiang\n";
    fs::write(&config, original).unwrap();
    {
        let mut receipt = InstallReceipt::new(root.path()).unwrap();
        let package = package("2026.06.30");
        let files = stage(&receipt, &package);
        commit_files(&mut receipt, files, &package, ARCHIVE_HASH.into()).unwrap();
        write_schema_list_transaction(&mut receipt, SCHEMA_ID, true, root.path()).unwrap();
        assert!(
            crate::scheme_files::parse_schema_list(&fs::read_to_string(&config).unwrap())
                .iter()
                .any(|schema| schema == SCHEMA_ID)
        );
    }
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert!(!root.path().join("rime_ice.schema.yaml").exists());
    assert!(!root.path().join(MANIFEST).exists());
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn standalone_rime_ice_installs_deploys_and_uninstalls_without_a_main_scheme() {
    use crate::scheme_host::tests::AddonHost;
    use std::sync::atomic::Ordering;
    let root = TestRoot::new();
    let host = AddonHost::new(root.path());
    smoke_fixture::write(&host.core.env().resource_dir.join("rime-data")).unwrap();
    fs::write(
        host.core.env().resource_dir.join("rime-data/default.yaml"),
        include_str!("addon-default.yaml"),
    )
    .unwrap();
    let runtime = root.path().join("runtime").join(SCHEMA_ID);
    let status = manage_rime_ice_with(&host.core, true, &host, |mut receipt| async move {
        let package = package("2026.06.30");
        let files = stage(&receipt, &package);
        commit_files(&mut receipt, files, &package, ARCHIVE_HASH.into())?;
        Ok(receipt)
    })
    .await
    .unwrap();
    assert!(status.installed && !status.deployed);
    assert_eq!(host.deploys.load(Ordering::SeqCst), 0);
    crate::package_schemas::set_package_schemas(
        &host.core,
        SCHEMA_ID,
        &[SCHEMA_ID.into(), "double_pinyin_flypy".into()],
    )
    .unwrap();
    host.deploy_package(SCHEMA_ID).await.unwrap();
    keytao_core::isolated_deploy::switch_ready_package_in_store(
        root.path(),
        &host.core.env().resource_dir.join("rime-data"),
        Some(&host.core.env().data_dir),
        SCHEMA_ID,
        None,
    )
    .unwrap();
    let status = rime_ice_status(&host.core, root.path()).unwrap();
    assert!(status.installed && status.deployed);
    assert_eq!(host.deploys.load(Ordering::SeqCst), 1);
    assert_eq!(
        crate::scheme_files::parse_schema_list(
            &fs::read_to_string(runtime.join("default.custom.yaml")).unwrap()
        ),
        [SCHEMA_ID, "double_pinyin_flypy"]
    );
    assert_eq!(
        keytao_core::deployed_schema_id_from_dir(root.path()).as_deref(),
        Some(SCHEMA_ID)
    );
    assert!(!root.path().join("default.custom.yaml").exists());
    assert!(!root.path().join(MANIFEST).exists());
    let status = manage_rime_ice_with(&host.core, true, &host, |mut receipt| async move {
        let package = package("2026.07.01");
        let files = stage(&receipt, &package);
        commit_files(&mut receipt, files, &package, ARCHIVE_HASH.into())?;
        Ok(receipt)
    })
    .await
    .unwrap();
    assert_eq!(status.version, "2026.07.01");
    fs::write(runtime.join("rime_ice.user.dict.yaml"), "private dictionary").unwrap();
    fs::write(runtime.join("rime_ice.userdb"), "learned").unwrap();
    let status = manage_rime_ice(&host.core, false, &host).await.unwrap();
    assert!(!status.installed && !status.deployed);
    assert_eq!(host.deploys.load(Ordering::SeqCst), 3);
    assert_eq!(
        fs::read_to_string(runtime.join("rime_ice.user.dict.yaml")).unwrap(),
        "private dictionary"
    );
    assert_eq!(
        fs::read_to_string(runtime.join("rime_ice.userdb")).unwrap(),
        "learned"
    );
    assert!(crate::addon_store::public_files(&host.core, SCHEMA_ID).is_err());
    assert!(keytao_core::ReloadStamp::path(root.path()).is_file());
    assert_eq!(keytao_core::RuntimeDirs::active_package_key(root.path()).as_deref(), Some("builtin/keytao"));
    assert!(!root.path().join("default.yaml").exists());
}

#[test]
fn unsafe_archive_paths_and_expansion_limit_leave_live_sources_untouched() {
    let root = TestRoot::new();
    for unsafe_name in [
        "../escape.txt",
        "lua/../escape.lua",
        "lua/CON.lua",
        "lua/evil.lua:stream",
    ] {
        let archive = root.path().join("invalid.zip");
        archive_at(&archive, &[(unsafe_name, "unsafe")]);
        assert!(extract(&archive, &root.path().join("stage"), &package("2026.06.30")).is_err());
    }
    let archive = root.path().join("limit.zip");
    archive_at(&archive, &[]);
    assert!(
        crate::generic_rime::extract_archive(
            &archive,
            &root.path().join("small"),
            Some(SCHEMA_ID),
            1
        )
        .is_err()
    );
    assert!(!root.path().join("rime_ice.schema.yaml").exists());
}

#[test]
#[ignore = "requires the official local archive and a local librime SDK"]
fn official_package_deploys_and_inputs_with_namespaced_lua() {
    let archive = PathBuf::from(std::env::var("KEYTAO_RIME_ICE_SMOKE_ARCHIVE").unwrap());
    let shared = std::env::var("KEYTAO_RIME_ICE_SMOKE_SHARED").unwrap();
    let root = TestRoot::new();
    let mut receipt = InstallReceipt::new(root.path()).unwrap();
    let package = package("2026.06.30");
    let files = extract(&archive, &receipt.work.join("files"), &package).unwrap();
    commit_files(
        &mut receipt,
        files,
        &package,
        wanxiang::fingerprint(&archive).unwrap().0,
    )
    .unwrap();
    receipt.finish().unwrap();
    fs::write(
        root.path().join("default.custom.yaml"),
        "patch:\n  schema_list:\n    - schema: rime_ice\n",
    )
    .unwrap();
    fs::write(
        root.path().join(keytao_core::ACTIVE_SCHEME_MANIFEST),
        serde_json::to_vec(
            &serde_json::json!({"root":root.path().canonicalize().unwrap(),
            "scheme_key":"local-rime-ice-smoke", "manifest":{"rime_ice.schema.yaml":
                wanxiang::fingerprint(&root.path().join("rime_ice.schema.yaml")).unwrap().0}}),
        )
        .unwrap(),
    )
    .unwrap();
    fs::write(
        root.path().join("rime_ice.custom.yaml"),
        "patch:\n  date_translator/date: keytaodate\n",
    )
    .unwrap();
    let runtime = keytao_core::ImeRuntime::with_dirs(root.path(), shared);
    runtime.init().unwrap();
    let session = runtime.create_session().unwrap();
    session.select_schema(SCHEMA_ID).unwrap();
    for (input, expected) in [("nihao", "你好"), ("zhongguo", "中国"), ("wanan", "晚安")] {
        session.clear_composition();
        let mut state = session.state();
        for key in input.chars() {
            state = session.process_key_result(key as u32, 0).unwrap().state;
        }
        assert!(
            state
                .candidates
                .iter()
                .any(|candidate| candidate.text == expected),
            "{input}: {:?}",
            state.candidates
        );
    }
    session.clear_composition();
    let mut state = session.state();
    for key in "keytaodate".chars() {
        state = session.process_key_result(key as u32, 0).unwrap().state;
    }
    assert!(
        state.candidates.iter().any(|candidate| {
            let bytes = candidate.text.as_bytes();
            bytes.len() == 10
                && bytes[4] == b'-'
                && bytes[7] == b'-'
                && bytes
                    .iter()
                    .enumerate()
                    .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
        }),
        "custom date trigger must retain its namespace: {:?}",
        state.candidates
    );
    session.clear_composition();
    let mut state = session.state();
    for key in "uuid".chars() {
        state = session.process_key_result(key as u32, 0).unwrap().state;
    }
    assert!(state.candidates.iter().any(|candidate| candidate.text.len() == 36
        && candidate.text.matches('-').count() == 4), "UUID Lua translator: {:?}", state.candidates);
    assert!(status_at(root.path()).deployed);
    if let Ok(logs) = fs::read_dir(root.path().join("log")) {
        for log in logs {
            let path = log.unwrap().path();
            if let Ok(content) = fs::read_to_string(&path) {
                assert!(
                    !content.contains("lua_gears.cc"),
                    "Lua runtime error in {}: {content}",
                    path.display()
                );
            }
        }
    }
    println!("rime_ice_smoke_root={}", root.dir.keep().display());
}
