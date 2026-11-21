#![cfg(not(any(target_os = "android", target_os = "windows")))]

use crate::{
    AppEnv, Core, CoreEvent, EventSink, Platform,
    package_activation::Transaction,
    package_store::PackageStore,
    packages::{archive_from_files, sha256_reader, store_cloud_archive},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

struct Sink;
impl EventSink for Sink {
    fn emit(&self, _: CoreEvent) {}
}

struct Host {
    root: PathBuf,
    core: Arc<Core>,
}

impl Host {
    fn new(directory: &Path, sink: Arc<dyn EventSink>) -> Self {
        let root = directory.join("rime");
        fs::create_dir_all(&root).unwrap();
        let core = Core::new(
            AppEnv {
                data_dir: directory.join("state"),
                cache_dir: directory.join("cache"),
                resource_dir: directory.join("resources"),
                app_version: "test".into(),
                platform: Platform::MacOs,
            },
            sink,
        )
        .unwrap();
        Self { root, core }
    }
}

fn write(root: &Path, path: &str, content: &[u8]) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn files(schema: &str) -> Vec<(String, Vec<u8>)> {
    vec![
        (
            format!("{schema}.schema.yaml"),
            format!("schema: {{schema_id: {schema}}}\ntranslator: {{dictionary: {schema}}}\n")
                .into_bytes(),
        ),
        (
            format!("{schema}.custom.yaml"),
            b"patch:\n  menu/page_size: 6\n".to_vec(),
        ),
        (
            format!("lua/{schema}.lua"),
            format!("return '{schema}'\n").into_bytes(),
        ),
        ("shared.txt".into(), b"shared".to_vec()),
        (
            "default.custom.yaml".into(),
            format!("patch:\n  menu/page_size: 6\n  schema_list:\n    - schema: {schema}\n")
                .into_bytes(),
        ),
        (
            "rime.lua".into(),
            format!("require('{schema}')\n").into_bytes(),
        ),
        (
            "squirrel.custom.yaml".into(),
            b"package frontend must not be copied".to_vec(),
        ),
        (
            "weasel.custom.yaml".into(),
            b"package frontend must not be copied".to_vec(),
        ),
    ]
}

fn store(host: &Host, key: &str, entries: Vec<(String, Vec<u8>)>) {
    let bytes = archive_from_files(&entries).unwrap();
    assert_eq!(
        store_cloud_archive(
            &host.core,
            key,
            Some("v1"),
            "official",
            None,
            &bytes,
            &host.root
        )
        .unwrap(),
        key,
    );
}

fn snapshot(root: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, directory: &Path, result: &mut BTreeMap<String, String>) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let kind = entry.file_type().unwrap();
            if kind.is_symlink() {
                result.insert(
                    relative,
                    format!("symlink:{}", fs::read_link(path).unwrap().display()),
                );
            } else if kind.is_dir() {
                walk(root, &path, result);
            } else if kind.is_file() {
                result.insert(
                    relative,
                    sha256_reader(fs::File::open(path).unwrap()).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(root, root, &mut result);
    result
}

#[test]
fn activation_startup_recovers_an_uncommitted_journal() {
    let directory = tempfile::tempdir().unwrap();
    let host = Host::new(directory.path(), Arc::new(Sink));
    host.core
        .update_state(|state| {
            state.active_scheme = Some("keytao".into());
            state.installed_packages.insert(
                "keytao".into(),
                crate::state::InstalledPackage {
                    version: Some("v1".into()),
                },
            );
        })
        .unwrap();
    let original_state = host.core.state();
    write(&host.root, "existing.txt", b"original");
    let store = PackageStore::new(&host.core);
    write(&store.root(), "activation.json", b"original activation");
    let transaction = Transaction::begin(
        &host.core,
        &host.root,
        BTreeSet::from(["existing.txt".into(), "new-directory/copied.txt".into()]),
    )
    .unwrap();
    write(&host.root, "existing.txt", b"partially replaced");
    write(&host.root, "new-directory/copied.txt", b"partial copy");
    write(&store.root(), "activation.json", b"partial activation");
    host.core
        .update_state(|state| {
            state.active_scheme = Some("xmjd".into());
        })
        .unwrap();
    std::mem::forget(transaction);
    let recovered = Core::new(host.core.env().clone(), Arc::new(Sink)).unwrap();
    assert_eq!(
        fs::read(host.root.join("existing.txt")).unwrap(),
        b"original"
    );
    assert!(!host.root.join("new-directory").exists());
    assert_eq!(
        fs::read(PackageStore::new(&recovered).root().join("activation.json")).unwrap(),
        b"original activation"
    );
    assert!(!store.root().join(".activation-transaction").exists());
    assert!(
        recovered.state() == original_state,
        "Startup must load the recovered state into memory"
    );
}

#[test]
fn activation_rollback_restores_the_previous_store_version() {
    for crashed in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let host = Host::new(directory.path(), Arc::new(Sink));
        let mut old_files = files("alpha");
        old_files.push(("old-only.txt".into(), b"old package file".to_vec()));
        store(&host, "keytao", old_files);
        let store = PackageStore::new(&host.core);
        let before = snapshot(&store.package_path("keytao"));
        let mut incoming = files("beta");
        incoming.push(("new/nested.txt".into(), b"new package file".to_vec()));
        let bytes = archive_from_files(&incoming).unwrap();
        let mut transaction = Transaction::begin(&host.core, &host.root, BTreeSet::new()).unwrap();
        transaction
            .snapshot_package(
                "keytao",
                &incoming.iter().map(|(path, _)| path.clone()).collect(),
            )
            .unwrap();
        let mut meta = store.read_meta("keytao").unwrap().unwrap();
        meta.version = Some("v2".into());
        meta.zip_sha256 = sha256_reader(bytes.as_slice()).unwrap();
        meta.manifest = incoming
            .iter()
            .filter(|(path, _)| !crate::packages::excluded_from_manifest(path))
            .map(|(path, bytes)| (path.clone(), sha256_reader(bytes.as_slice()).unwrap()))
            .collect();
        store
            .write_archive(std::io::Cursor::new(&bytes), &meta, None)
            .unwrap();
        assert_eq!(
            store
                .read_meta("keytao")
                .unwrap()
                .unwrap()
                .version
                .as_deref(),
            Some("v2")
        );
        assert!(
            !store
                .package_path("keytao")
                .join("files/old-only.txt")
                .exists()
        );
        if crashed {
            std::mem::forget(transaction);
            Core::new(host.core.env().clone(), Arc::new(Sink)).unwrap();
        } else {
            drop(transaction);
        }
        assert_eq!(snapshot(&store.package_path("keytao")), before);
        assert_eq!(
            store.read_files("keytao").unwrap().1.version.as_deref(),
            Some("v1")
        );
        assert!(!store.package_path("keytao").join("files/new").exists());
    }
}

fn copy_regular_import_source(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name.to_str().is_some_and(|name| {
            matches!(name, "log" | "build" | "sync" | ".git" | ".DS_Store")
                || name.ends_with(".userdb")
        }) {
            continue;
        }
        let destination = crate::packages::safe_path(target, &name.to_string_lossy()).unwrap();
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            copy_regular_import_source(&entry.path(), &destination);
        } else if kind.is_file() {
            fs::copy(entry.path(), destination).unwrap();
        }
    }
}

#[cfg(unix)]
#[test]
fn import_copy_preserves_source_and_skips_runtime_data_and_symlinks() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    for name in [
        "nested/example.schema.yaml",
        "log/warning",
        "build/example.table.bin",
        "sync/device/data",
        "example.userdb/data",
    ] {
        write(&source, name, b"original");
    }
    std::os::unix::fs::symlink(
        source.join("nested/example.schema.yaml"),
        source.join("linked.schema.yaml"),
    )
    .unwrap();
    let before = snapshot(&source);
    let target = directory.path().join("local-source");
    copy_regular_import_source(&source, &target);
    assert_eq!(snapshot(&source), before);
    assert_eq!(
        snapshot(&target)
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["nested/example.schema.yaml"]
    );
    assert_eq!(
        fs::read(target.join("nested/example.schema.yaml")).unwrap(),
        b"original"
    );
}
