use crate::{
    Core, CoreError,
    package_store::PackageStore,
    packages::{safe_path, sha256_reader},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct OwnedFile {
    pub path: String,
    pub sha256: String,
    #[serde(default, rename = "move", skip_serializing_if = "std::ops::Not::not")]
    pub move_file: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Backup {
    pub path: String,
    pub backup: String,
    pub sha256: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct Activation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activations: Option<BTreeMap<String, Activation>>,
    pub key: String,
    pub root: String,
    pub owned: Vec<OwnedFile>,
    pub generated: Vec<OwnedFile>,
    pub backups: Vec<Backup>,
    #[serde(default)]
    pub kept_backups: Vec<Backup>,
    // Native IMEs also consume the legacy marker shape.
    pub scheme_key: String,
    pub manifest: BTreeMap<String, String>,
    pub rime_lua: Option<String>,
    #[serde(default)]
    pub default_applied: BTreeMap<String, serde_yaml::Value>,
}

pub(crate) fn read(core: &Core, root: &Path) -> Result<Option<Activation>, CoreError> {
    let store = PackageStore::new(core);
    let path = store.checked_activation_path()?;
    match fs::read(&path) {
        Ok(bytes) => {
            let active: Activation = serde_json::from_slice(&bytes)?;
            if Path::new(&active.root)
                != root.canonicalize()
                    .map_err(|error| crate::package_fs::context("解析目录", root, error))?
            {
                return Ok(None);
            }
            for record in std::iter::once(&active).chain(active.activations.iter().flat_map(|records| records.values())) {
            for file in record.owned.iter().chain(&record.generated) {
                safe_path(root, &file.path).map_err(CoreError::Other)?;
            }
            for backup in record.backups.iter().chain(&record.kept_backups) {
                safe_path(root, &backup.path).map_err(CoreError::Other)?;
                safe_path(&store.root(), &backup.backup).map_err(CoreError::Other)?;
            }
            }
            Ok(Some(active))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(crate::package_fs::context("读取文件", &path, e).into()),
    }
}

pub(crate) fn read_all(core: &Core, root: &Path) -> Result<BTreeMap<String, Activation>, CoreError> {
    let mut active = read(core, root)?.unwrap_or_default();
    let legacy = active.activations.is_none();
    let mut records = active.activations.take().unwrap_or_default();
    if !active.key.is_empty() {
        records.entry(active.key.clone()).or_insert(active);
    }
    if records.is_empty() && legacy {
        let path = safe_path(root, keytao_core::ACTIVE_SCHEME_MANIFEST).map_err(CoreError::Other)?;
        if let Ok(bytes) = fs::read(path) {
            let marker: serde_json::Value = serde_json::from_slice(&bytes)?;
            if marker.get("packages").is_none() && marker["root"].as_str().map(Path::new) == Some(root.canonicalize()?.as_path()) {
                let key = marker["scheme_key"].as_str().map(str::to_owned).or_else(|| core.state().active_scheme);
                if key.as_deref().is_some_and(keytao_core::is_addon_schema) {
                    return Ok(records);
                }
                if let Some(key) = key.filter(|key| !key.is_empty()) {
                    crate::scheme::validate_scheme_key(&key).map_err(CoreError::Other)?;
                    let manifest: BTreeMap<String, String> = serde_json::from_value(marker["manifest"].clone())?;
                    for path in manifest.keys() { safe_path(root, path).map_err(CoreError::Other)?; }
                    records.insert(key.clone(), Activation {
                        key: key.clone(), scheme_key: key,
                        root: root.canonicalize()?.to_string_lossy().into_owned(),
                        owned: manifest.iter().map(|(path, sha256)| OwnedFile { path: path.clone(), sha256: sha256.clone(), move_file: false }).collect(),
                        manifest,
                        rime_lua: marker["rime_lua"].as_str().map(str::to_owned),
                        ..Default::default()
                    });
                }
            }
        }
    }
    Ok(records)
}

pub(crate) fn read_package(core: &Core, root: &Path, key: &str) -> Result<Option<Activation>, CoreError> {
    Ok(read_all(core, root)?.remove(key))
}

pub(crate) fn user_file(path: &str) -> bool {
    crate::install::is_protected_user_file(path)
        || path
            .split('/')
            .any(|part| part == "sync" || part.contains(".userdb") || part.starts_with(".keytao-"))
        || matches!(path, "squirrel.custom.yaml" | "weasel.custom.yaml")
}

#[cfg(any(target_os = "windows", test))]
pub(crate) fn addon_ownership(core: &Core, root: &Path) -> Result<BTreeMap<String, String>, CoreError> {
    crate::addon_store::ownership(core, root)
}

#[cfg(test)]
pub(crate) fn backup(
    core: &Core,
    root: &Path,
    relative: &str,
) -> Result<Option<Backup>, CoreError> {
    let path = safe_path(root, relative).map_err(CoreError::Other)?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(crate::package_fs::context("读取文件", &path, e).into()),
    };
    let digest = sha256_reader(bytes.as_slice())?;
    let name = format!("backups/{digest}/{relative}");
    let destination =
        safe_path(&PackageStore::new(core).checked_root()?, &name).map_err(CoreError::Other)?;
    let parent = destination.parent().unwrap();
    fs::create_dir_all(parent)
        .map_err(|error| crate::package_fs::context("创建目录", parent, error))?;
    crate::scheme_files::write_file_atomic(&destination, &bytes)?;
    Ok(Some(Backup {
        path: relative.into(),
        backup: name,
        sha256: digest,
    }))
}

pub(crate) fn fingerprint(root: &Path, relative: &str) -> Result<Option<OwnedFile>, CoreError> {
    let path = safe_path(root, relative).map_err(CoreError::Other)?;
    match fs::File::open(&path) {
        Ok(file) => Ok(Some(OwnedFile {
            path: relative.into(),
            sha256: sha256_reader(file)?,
            move_file: false,
        })),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(crate::package_fs::context("读取文件", &path, e).into()),
    }
}

#[cfg(test)]
pub(crate) fn write(core: &Core, active: &Activation) -> Result<(), CoreError> {
    let store = PackageStore::new(core).checked_root()?;
    fs::create_dir_all(&store)
        .map_err(|error| crate::package_fs::context("创建目录", &store, error))?;
    let mut combined = active.clone();
    let mut records = read_all(core, Path::new(&active.root))?;
    if !active.key.is_empty() {
        let mut record = active.clone();
        record.activations = None;
        records.insert(active.key.clone(), record);
    }
    combined.activations = Some(records);
    crate::scheme_files::write_file_atomic(
        &PackageStore::new(core).checked_activation_path()?,
        &serde_json::to_vec_pretty(&combined)?,
    )?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn backup_bytes(core: &Core, backup: &Backup) -> Result<Vec<u8>, CoreError> {
    let path = safe_path(&PackageStore::new(core).checked_root()?, &backup.backup)
        .map_err(CoreError::Other)?;
    let bytes = fs::read(&path)
        .map_err(|error| crate::package_fs::context("读取备份文件", &path, error))?;
    if sha256_reader(bytes.as_slice())? != backup.sha256 {
        return Err(CoreError::Other(
            "Activation backup checksum mismatch".into(),
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
fn restore_backup(core: &Core, root: &Path, backup: &Backup) -> Result<(), CoreError> {
    let path = safe_path(root, &backup.path).map_err(CoreError::Other)?;
    let parent = path.parent().unwrap();
    fs::create_dir_all(parent)
        .map_err(|error| crate::package_fs::context("创建目录", parent, error))?;
    crate::scheme_files::write_file_atomic(&path, &backup_bytes(core, backup)?)?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct Snapshot {
    path: String,
    existed: bool,
    store: bool,
}
#[derive(Serialize, Deserialize)]
struct Journal {
    root: PathBuf,
    snapshots: Vec<Snapshot>,
    #[serde(default)]
    build_paths: Option<BTreeSet<String>>,
    #[cfg(any(target_os = "windows", test))]
    #[serde(default)]
    public_schemas: Option<crate::windows_public_schemas::PublicSchemaCheckpoint>,
    committed: bool,
    state: Option<Vec<u8>>,
}

pub(crate) struct Transaction {
    directory: PathBuf,
    store: PathBuf,
    journal: Journal,
    finished: bool,
}

impl Transaction {
    #[cfg(test)]
    pub(crate) fn begin(
        core: &Core,
        root: &Path,
        mut paths: BTreeSet<String>,
    ) -> Result<Self, CoreError> {
        let store = PackageStore::new(core).checked_root()?;
        fs::create_dir_all(root)
            .map_err(|error| crate::package_fs::context("创建目录", root, error))?;
        fs::create_dir_all(&store)
            .map_err(|error| crate::package_fs::context("创建目录", &store, error))?;
        let build_paths = build_files(root)?;
        paths.extend(build_paths.iter().cloned());
        let directory = safe_path(&store, ".activation-transaction").map_err(CoreError::Other)?;
        if directory.try_exists()? {
            return Err(CoreError::Other(
                "An unfinished activation requires recovery".into(),
            ));
        }
        fs::create_dir(&directory)
            .map_err(|error| crate::package_fs::context("创建目录", &directory, error))?;
        let state = match fs::read(core.env().data_dir.join("app-state.json")) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        let mut transaction = Self {
            directory,
            store,
            journal: Journal {
                root: root.canonicalize()?,
                snapshots: Vec::new(),
                build_paths: Some(build_paths),
                #[cfg(any(target_os = "windows", test))]
                public_schemas: None,
                committed: false,
                state,
            },
            finished: false,
        };
        for path in paths {
            transaction.snapshot(&path, false)?;
        }
        for path in ["activation.json", "active-manifest.json"] {
            transaction.snapshot(path, true)?;
        }
        #[cfg(target_os = "windows")]
        if crate::windows_public_schemas::is_default_user_root(root) {
            transaction.journal.public_schemas = Some(
                crate::windows_public_schemas::checkpoint(
                    &transaction.directory.join("public-schemas"),
                )
                .map_err(CoreError::Other)?,
            );
        }
        transaction.persist()?;
        Ok(transaction)
    }

    #[cfg(test)]
    fn snapshot(&mut self, relative: &str, store: bool) -> Result<(), CoreError> {
        let root = if store {
            &self.store
        } else {
            &self.journal.root
        };
        let path = safe_path(root, relative).map_err(CoreError::Other)?;
        let bytes = match fs::read(&path) {
            Ok(b) => Some(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(crate::package_fs::context("读取文件", &path, e).into()),
        };
        if let Some(bytes) = &bytes {
            let backup = self
                .directory
                .join(self.journal.snapshots.len().to_string());
            use std::io::Write;
            (|| {
                let file = fs::File::create(&backup)?;
                (&file).write_all(bytes)?;
                file.sync_all()
            })()
            .map_err(|error| crate::package_fs::context("写入备份文件", &backup, error))?;
        }
        self.journal.snapshots.push(Snapshot {
            path: relative.into(),
            existed: bytes.is_some(),
            store,
        });
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn snapshot_store_file(&mut self, relative: &str) -> Result<(), CoreError> {
        self.snapshot(relative, true)?;
        self.persist()
    }

    #[cfg(test)]
    pub(crate) fn snapshot_package(
        &mut self,
        key: &str,
        incoming: &BTreeSet<String>,
    ) -> Result<(), CoreError> {
        let namespace = if crate::scheme::is_local_scheme_key(key) {
            "local"
        } else {
            "cloud"
        };
        let prefix = format!("{namespace}/{key}");
        let directory = safe_path(&self.store, &prefix).map_err(CoreError::Other)?;
        let mut paths: BTreeSet<_> = incoming
            .iter()
            .map(|path| format!("{prefix}/files/{path}"))
            .collect();
        paths.insert(format!("{prefix}/package.json"));
        fn collect(
            root: &Path,
            directory: &Path,
            paths: &mut BTreeSet<String>,
        ) -> Result<(), CoreError> {
            if !directory.try_exists()? {
                return Ok(());
            }
            for entry in fs::read_dir(directory)
                .map_err(|error| crate::package_fs::context("读取目录", directory, error))?
            {
                let entry = entry
                    .map_err(|error| crate::package_fs::context("读取目录项", directory, error))?;
                let relative = entry
                    .path()
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                let path = safe_path(root, &relative).map_err(CoreError::Other)?;
                if entry.file_type()
                    .map_err(|error| crate::package_fs::context("读取文件属性", &path, error))?
                    .is_dir()
                {
                    collect(root, &path, paths)?;
                } else {
                    paths.insert(relative);
                }
            }
            Ok(())
        }
        collect(&self.store, &directory, &mut paths)?;
        for path in paths {
            self.snapshot(&path, true)?;
        }
        self.persist()
    }

    #[cfg(test)]
    fn persist(&self) -> Result<(), CoreError> {
        crate::scheme_files::write_file_atomic(
            &self.directory.join("journal.json"),
            &serde_json::to_vec(&self.journal)?,
        )?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn commit(mut self) -> Result<(), CoreError> {
        self.journal.committed = true;
        if let Err(error) = self.persist() {
            self.journal.committed = false;
            return Err(error);
        }
        self.finished = true;
        if let Err(error) = crate::package_fs::remove_dir_all(&self.directory) {
            tracing::warn!(%error, "Committed activation cleanup deferred until startup");
        }
        Ok(())
    }

    fn rollback(&self) -> Result<(), CoreError> {
        self.rollback_with_remove(crate::package_fs::remove_file)
    }

    fn rollback_with_remove(
        &self,
        mut remove_file: impl FnMut(&Path) -> std::io::Result<()>,
    ) -> Result<(), CoreError> {
        let mut errors = Vec::new();
        let mut attempt = |step: &str, result: Result<(), CoreError>| {
            if let Err(error) = result {
                errors.push(format!("{step}: {error}"));
            }
        };
        if let Some(original) = &self.journal.build_paths {
            match build_files(&self.journal.root) {
                Ok(current) => {
                    for relative in current.difference(original) {
                        attempt(
                            relative,
                            (|| {
                                let path = safe_path(&self.journal.root, relative)
                                    .map_err(CoreError::Other)?;
                                remove_file(&path)?;
                                let mut parent = path.parent();
                                while let Some(directory) =
                                    parent.filter(|dir| *dir != self.journal.root)
                                {
                                    if crate::package_fs::retry_windows(|| {
                                        crate::package_fs::clear_readonly(directory)?;
                                        fs::remove_dir(directory)
                                    })
                                    .is_err()
                                    {
                                        break;
                                    }
                                    parent = directory.parent();
                                }
                                Ok(())
                            })(),
                        );
                    }
                }
                Err(error) => attempt("List build outputs", Err(error)),
            }
        }
        for (index, snapshot) in self.journal.snapshots.iter().enumerate().rev() {
            attempt(
                &snapshot.path,
                (|| {
                    let root = if snapshot.store {
                        &self.store
                    } else {
                        &self.journal.root
                    };
                    let path = safe_path(root, &snapshot.path).map_err(CoreError::Other)?;
                    if snapshot.existed {
                        let parent = path.parent().unwrap();
                        fs::create_dir_all(parent)
                            .map_err(|error| crate::package_fs::context("创建目录", parent, error))?;
                        let backup = self.directory.join(index.to_string());
                        let bytes = fs::read(&backup)
                            .map_err(|error| crate::package_fs::context("读取备份文件", &backup, error))?;
                        crate::scheme_files::write_file_atomic(&path, &bytes)?;
                    } else if path.try_exists()? {
                        remove_file(&path)?;
                    }
                    let mut parent = path.parent();
                    while let Some(dir) = parent.filter(|dir| *dir != root) {
                        if crate::package_fs::retry_windows(|| {
                            crate::package_fs::clear_readonly(dir)?;
                            fs::remove_dir(dir)
                        })
                        .is_err()
                        {
                            break;
                        }
                        parent = dir.parent();
                    }
                    Ok(())
                })(),
            );
        }
        attempt(
            "Restore app state",
            (|| {
                let state = self.store.parent().unwrap().join("app-state.json");
                if let Some(bytes) = &self.journal.state {
                    crate::scheme_files::write_file_atomic(&state, bytes)?;
                } else if state.try_exists()? {
                    remove_file(&state)?;
                }
                Ok(())
            })(),
        );
        #[cfg(any(target_os = "windows", test))]
        if let Some(checkpoint) = &self.journal.public_schemas {
            attempt(
                "Restore public schemas",
                checkpoint
                    .restore(&self.directory.join("public-schemas"))
                    .map_err(CoreError::Other),
            );
        }
        if !errors.is_empty() {
            return Err(CoreError::Other(format!(
                "Activation rollback failed: {}",
                errors.join("; ")
            )));
        }
        crate::package_fs::remove_dir_all(&self.directory)?;
        Ok(())
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        if !self.finished {
            if let Err(error) = self.rollback() {
                tracing::error!(%error, "Activation rollback deferred until startup");
            }
        }
    }
}

pub(crate) fn recover(core: &Core) -> Result<(), CoreError> {
    let store = PackageStore::new(core).checked_root()?;
    let directory = safe_path(&store, ".activation-transaction").map_err(CoreError::Other)?;
    if !directory.try_exists()? {
        return Ok(());
    }
    let journal_path = directory.join("journal.json");
    let bytes = match fs::read(&journal_path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            crate::package_fs::remove_dir_all(&directory)?;
            return Ok(());
        }
        Err(e) => return Err(crate::package_fs::context("读取事务日志", &journal_path, e).into()),
    };
    let journal: Journal = serde_json::from_slice(&bytes)?;
    if journal.committed {
        crate::package_fs::remove_dir_all(&directory)?;
        return Ok(());
    }
    let mut transaction = Transaction {
        directory,
        store,
        journal,
        finished: true,
    };
    transaction.rollback()?;
    transaction.finished = true;
    Ok(())
}

fn build_files(root: &Path) -> Result<BTreeSet<String>, CoreError> {
    fn collect(
        root: &Path,
        directory: &Path,
        paths: &mut BTreeSet<String>,
    ) -> Result<(), CoreError> {
        if !directory.try_exists()? {
            return Ok(());
        }
        for entry in fs::read_dir(directory)
            .map_err(|error| crate::package_fs::context("读取目录", directory, error))?
        {
            let entry = entry
                .map_err(|error| crate::package_fs::context("读取目录项", directory, error))?;
            let relative = entry
                .path()
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if relative
                .split('/')
                .any(|part| part == "sync" || part.contains(".userdb"))
            {
                continue;
            }
            let path = safe_path(root, &relative).map_err(CoreError::Other)?;
            if entry.file_type()
                .map_err(|error| crate::package_fs::context("读取文件属性", &path, error))?
                .is_dir()
            {
                collect(root, &path, paths)?;
            } else {
                paths.insert(relative);
            }
        }
        Ok(())
    }
    let mut paths = BTreeSet::new();
    collect(
        root,
        &safe_path(root, "build").map_err(CoreError::Other)?,
        &mut paths,
    )?;
    Ok(paths)
}

#[cfg(test)]
mod transaction_tests {
    use super::*;
    use crate::{AppEnv, CoreEvent, EventSink, Platform};
    use std::sync::Arc;

    struct Sink;
    impl EventSink for Sink {
        fn emit(&self, _: CoreEvent) {}
    }

    fn core(directory: &Path) -> Arc<Core> {
        Core::new(
            AppEnv {
                data_dir: directory.join("app"),
                cache_dir: directory.join("cache"),
                resource_dir: directory.join("resources"),
                app_version: "test".into(),
                platform: Platform::MacOs,
            },
            Arc::new(Sink),
        )
        .unwrap()
    }

    fn write(root: &Path, relative: &str, bytes: &[u8]) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn backup_and_restore_create_writable_copies_of_readonly_sources() {
        let directory = tempfile::tempdir().unwrap();
        let core = core(directory.path());
        let root = directory.path().join("rime");
        let relative = "example.schema.yaml";
        write(&root, relative, b"original");
        let path = root.join(relative);
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).unwrap();

        let backup = backup(&core, &root, relative).unwrap().unwrap();
        let stored = PackageStore::new(&core).root().join(&backup.backup);
        assert!(!fs::metadata(&stored).unwrap().permissions().readonly());
        assert!(fs::metadata(&path).unwrap().permissions().readonly());
        crate::package_fs::remove_file(&path).unwrap();
        let mut permissions = fs::metadata(&stored).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&stored, permissions).unwrap();

        restore_backup(&core, &root, &backup).unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"original");
        assert!(!fs::metadata(&path).unwrap().permissions().readonly());
        assert!(fs::metadata(&stored).unwrap().permissions().readonly());
        crate::package_fs::clear_readonly(&stored).unwrap();
    }

    #[test]
    fn rollback_restores_build_and_removes_new_outputs_after_failure_or_crash() {
        for crashed in [false, true] {
            for existing_build in [false, true] {
                let directory = tempfile::tempdir().unwrap();
                let core = core(directory.path());
                let root = directory.path().join("rime");
                fs::create_dir(&root).unwrap();
                if existing_build {
                    write(&root, "build/alpha.schema.yaml", b"previous schema");
                    write(&root, "build/.keytao-deployed", b"previous stamp");
                    write(&root, "build/alpha.userdb/data", b"learned words");
                }
                let transaction = Transaction::begin(&core, &root, BTreeSet::new()).unwrap();
                write(&root, "build/alpha.schema.yaml", b"new schema");
                write(&root, "build/.keytao-deployed", b"new stamp");
                write(&root, "build/nested/beta.table.bin", b"compiled output");
                write(&root, "build/beta.userdb/data", b"new learned words");
                write(&root, "build/sync/user.yaml", b"synchronized data");
                if crashed {
                    std::mem::forget(transaction);
                    recover(&core).unwrap();
                } else {
                    drop(transaction);
                }
                assert!(!root.join("build/nested").exists());
                assert_eq!(
                    root.join("build/alpha.schema.yaml").exists(),
                    existing_build
                );
                assert_eq!(root.join("build/.keytao-deployed").exists(), existing_build);
                if existing_build {
                    assert_eq!(
                        fs::read(root.join("build/alpha.schema.yaml")).unwrap(),
                        b"previous schema"
                    );
                    assert_eq!(
                        fs::read(root.join("build/.keytao-deployed")).unwrap(),
                        b"previous stamp"
                    );
                    assert_eq!(
                        fs::read(root.join("build/alpha.userdb/data")).unwrap(),
                        b"learned words"
                    );
                }
                assert_eq!(
                    fs::read(root.join("build/beta.userdb/data")).unwrap(),
                    b"new learned words"
                );
                assert_eq!(
                    fs::read(root.join("build/sync/user.yaml")).unwrap(),
                    b"synchronized data"
                );
                assert!(
                    !PackageStore::new(&core)
                        .root()
                        .join(".activation-transaction")
                        .exists()
                );
            }
        }
    }

    #[test]
    fn committed_build_outputs_survive_startup_recovery() {
        let directory = tempfile::tempdir().unwrap();
        let core = core(directory.path());
        let root = directory.path().join("rime");
        let transaction = Transaction::begin(&core, &root, BTreeSet::new()).unwrap();
        write(&root, "build/beta.table.bin", b"compiled output");
        transaction.commit().unwrap();
        recover(&core).unwrap();
        assert_eq!(
            fs::read(root.join("build/beta.table.bin")).unwrap(),
            b"compiled output"
        );
    }

    #[test]
    fn rollback_restores_snapshots_state_and_public_after_removal_failure() {
        for fail_snapshot_removal in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let core = core(directory.path());
            let root = directory.path().join("rime");
            let public = directory.path().join("public");
            let previous = "a".repeat(64);
            let published = "b".repeat(64);
            core.update_state(|state| state.active_scheme = Some("keytao".into()))
                .unwrap();
            write(&root, "alpha.schema.yaml", b"previous source");
            let root = root.canonicalize().unwrap();
            let store = PackageStore::new(&core).root();
            write(&store, "activation.json", b"previous activation");
            write(
                &public,
                &format!("{previous}/alpha.schema.yaml"),
                b"previous public schema",
            );
            write(&public, "current.txt", previous.as_bytes());
            let mut transaction = Transaction::begin(
                &core,
                &root,
                BTreeSet::from(["alpha.schema.yaml".into(), "new.txt".into()]),
            )
            .unwrap();
            transaction.journal.public_schemas = Some(
                crate::windows_public_schemas::checkpoint_at(
                    public.clone(),
                    &transaction.directory.join("public-schemas"),
                )
                .unwrap(),
            );
            transaction.persist().unwrap();
            write(&root, "alpha.schema.yaml", b"new source");
            write(&root, "new.txt", b"new source");
            write(&root, "build/beta.prism.bin", b"mapped output");
            write(&root, "build/zeta.table.bin", b"new output");
            write(&store, "activation.json", b"new activation");
            core.update_state(|state| state.active_scheme = Some("xmjd".into()))
                .unwrap();
            fs::remove_dir_all(public.join(&previous)).unwrap();
            write(
                &public,
                &format!("{published}/beta.schema.yaml"),
                b"new public schema",
            );
            write(&public, "current.txt", published.as_bytes());

            let error = transaction
                .rollback_with_remove(|path| {
                    if path == root.join("build/beta.prism.bin")
                        || (fail_snapshot_removal && path == root.join("new.txt"))
                    {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            "injected sharing violation",
                        ));
                    }
                    fs::remove_file(path)
                })
                .unwrap_err()
                .to_string();
            transaction.finished = true;
            assert!(error.contains("build/beta.prism.bin"));
            assert_eq!(error.contains("new.txt"), fail_snapshot_removal);
            assert!(root.join("build/beta.prism.bin").exists());
            assert!(!root.join("build/zeta.table.bin").exists());
            assert_eq!(root.join("new.txt").exists(), fail_snapshot_removal);
            assert_eq!(
                fs::read(root.join("alpha.schema.yaml")).unwrap(),
                b"previous source"
            );
            assert_eq!(
                fs::read(store.join("activation.json")).unwrap(),
                b"previous activation"
            );
            assert_eq!(
                crate::AppState::load(&core.env().data_dir)
                    .unwrap()
                    .active_scheme
                    .as_deref(),
                Some("keytao")
            );
            assert_eq!(
                fs::read_to_string(public.join("current.txt")).unwrap(),
                previous
            );
            assert_eq!(
                fs::read(public.join(&previous).join("alpha.schema.yaml")).unwrap(),
                b"previous public schema"
            );
            assert!(!public.join(published).exists());
            assert!(transaction.directory.exists());
            recover(&core).unwrap();
            assert!(!root.join("build/beta.prism.bin").exists());
            assert!(!transaction.directory.exists());
        }
    }

    #[test]
    fn public_checkpoint_is_restored_after_failure_crash_and_failed_commit() {
        for failure in ["drop", "crash", "commit"] {
            let directory = tempfile::tempdir().unwrap();
            let core = core(directory.path());
            let root = directory.path().join("rime");
            let public = directory.path().join("public");
            let previous = "a".repeat(64);
            let published = "b".repeat(64);
            write(
                &public,
                &format!("{previous}/alpha.schema.yaml"),
                b"previous public schema",
            );
            write(&public, "current.txt", previous.as_bytes());
            let mut transaction = Transaction::begin(&core, &root, BTreeSet::new()).unwrap();
            transaction.journal.public_schemas = Some(
                crate::windows_public_schemas::checkpoint_at(
                    public.clone(),
                    &transaction.directory.join("public-schemas"),
                )
                .unwrap(),
            );
            transaction.persist().unwrap();
            fs::remove_dir_all(public.join(&previous)).unwrap();
            write(
                &public,
                &format!("{published}/beta.schema.yaml"),
                b"new public schema",
            );
            write(&public, "current.txt", published.as_bytes());
            match failure {
                "crash" => {
                    std::mem::forget(transaction);
                    Core::new(core.env().clone(), Arc::new(Sink)).unwrap();
                }
                "commit" => {
                    fs::remove_file(transaction.directory.join("journal.json")).unwrap();
                    fs::create_dir(transaction.directory.join("journal.json")).unwrap();
                    assert!(transaction.commit().is_err());
                }
                _ => drop(transaction),
            }
            assert_eq!(
                fs::read_to_string(public.join("current.txt")).unwrap(),
                previous
            );
            assert_eq!(
                fs::read(public.join(previous).join("alpha.schema.yaml")).unwrap(),
                b"previous public schema"
            );
            assert!(!public.join(published).exists());
            assert!(
                !PackageStore::new(&core)
                    .root()
                    .join(".activation-transaction")
                    .exists()
            );
        }
    }
}
