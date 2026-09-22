use super::*;
use std::{
    io::{Cursor, Write},
    sync::{Mutex as StdMutex, MutexGuard as StdMutexGuard},
};
use zip::{write::SimpleFileOptions, ZipWriter};

static SERIAL: StdMutex<()> = StdMutex::new(());

struct TestRoot {
    path: PathBuf,
    cleanup: bool,
    _serial: StdMutexGuard<'static, ()>,
}

impl TestRoot {
    fn new() -> Self {
        let serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
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
    let mut entries: Vec<_> =
        REQUIRED
            .iter()
            .map(|name| {
                (*name,
        "grammar:\n  language: wanxiang-lts-zh-hans\ntranslator:\n  dictionary: wanxiang\n")
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
    extract(&archive, &receipt.work.join("files")).unwrap()
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
    assert!(extract(&archive, &root.path.join("stage"))
        .unwrap_err()
        .contains("不安全路径"));
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
    assert!(extract(&archive, &root.path.join("stage"))
        .unwrap_err()
        .contains("重复路径"));
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
    assert!(extract(&archive, &root.path.join("stage"))
        .unwrap_err()
        .contains("符号链接"));
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
    commit_files(&mut receipt, files).unwrap();
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
    commit_files(&mut receipt, files).unwrap();
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
    assert!(commit_files(&mut receipt, files)
        .unwrap_err()
        .contains("冲突"));
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
    commit_files(&mut receipt, files).unwrap();
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
    commit_files(&mut receipt, files).unwrap();
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
    commit_files(&mut receipt, files).unwrap();
    let public = receipt.public_source_files();
    let (_, source) = public
        .iter()
        .find(|(p, _)| *p == PathBuf::from("wanxiang.schema.yaml"))
        .unwrap();
    assert_eq!(fs::read_to_string(source).unwrap(), BASIC_SCHEMA);
    assert!(!public
        .iter()
        .any(|(p, _)| *p == PathBuf::from("wanxiang.custom.yaml")));
    assert_eq!(
        fs::read_to_string(root.path.join("wanxiang.custom.yaml")).unwrap(),
        "private customization"
    );
}

/// Opt-in integration check using a locally cached, hash-verified upstream ZIP.
/// All Rime files, logs and learned dictionaries stay in a fresh temporary root.
#[test]
#[ignore = "requires the official archive and a local librime SDK"]
fn official_package_deploys_and_inputs_without_grammar_model() {
    let archive = PathBuf::from(std::env::var("KEYTAO_WANXIANG_SMOKE_ARCHIVE").unwrap());
    let shared = std::env::var("KEYTAO_WANXIANG_SMOKE_SHARED").unwrap();
    assert_eq!(
        fingerprint(&archive).unwrap(),
        (DOWNLOAD_SHA256.into(), DOWNLOAD_BYTES)
    );
    let mut root = TestRoot::new();
    // librime deliberately keeps its process-global runtime/log handles alive
    // after ImeRuntime drops. Retain this opt-in smoke's evidence for inspection
    // and remove it from the invoking process after the test process exits.
    root.cleanup = false;
    println!("wanxiang_smoke_root={}", root.path.display());
    let mut receipt = InstallReceipt::new(&root.path).unwrap();
    let files = extract(&archive, &receipt.work.join("files")).unwrap();
    commit_files(&mut receipt, files).unwrap();
    fs::write(
        root.path.join("default.custom.yaml"),
        "patch:\n  schema_list:\n    - schema: wanxiang\n",
    )
    .unwrap();
    let runtime = keytao_core::ImeRuntime::with_dirs(&root.path, shared);
    runtime.init().unwrap();
    let session = runtime.create_session().unwrap();
    session.select_schema(SCHEMA_ID).unwrap();
    for (input, expected) in [("nihao", "你好"), ("zhongguo", "中国"), ("wanan", "晚安")] {
        session.clear_composition();
        let mut state = session.state();
        for character in input.chars() {
            state = session
                .process_key_result(character as u32, 0)
                .unwrap()
                .state;
        }
        let candidates: Vec<_> = state.candidates.iter().map(|c| c.text.as_str()).collect();
        println!("wanxiang input={input} candidates={candidates:?}");
        assert!(
            candidates.contains(&expected),
            "missing {expected}: {candidates:?}"
        );
    }
    assert!(status_at(&root.path).deployed);
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
