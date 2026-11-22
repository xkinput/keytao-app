use crate::{
    Core, CoreError,
    package_fs::{self, context},
    packages::{InstalledPackageInfo, PackageMeta, safe_path, sha256_reader},
};
use serde::{Deserialize, Serialize};
use keytao_core::engine_choice::{PACKAGE_KEY, PackageEngine};
use std::{
    collections::BTreeSet,
    fs,
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
};

pub(crate) enum Staging {
    Metadata,
    #[cfg(test)]
    Pending,
    #[cfg(test)]
    ActiveManifest,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct PackageFile {
    pub(crate) path: String,
    pub(crate) sha256: String,
    pub(crate) size: u64,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct StoredPackage {
    pub(crate) key: String,
    pub(crate) origin: Option<String>,
    pub(crate) installed_at: u64,
    pub(crate) files: Vec<PackageFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) variant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) grammar: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) addon_manifest: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) selected_schemas: Option<Vec<String>>,
    #[serde(flatten)]
    pub(crate) meta: PackageMeta,
}

struct PackageStage(PathBuf);

impl Drop for PackageStage {
    fn drop(&mut self) {
        let _ = package_fs::remove_dir_all(&self.0);
    }
}

pub(crate) struct StagingFile {
    file: tempfile::NamedTempFile,
    _stage: PackageStage,
}

impl std::ops::Deref for StagingFile {
    type Target = tempfile::NamedTempFile;
    fn deref(&self) -> &Self::Target {
        &self.file
    }
}

impl std::ops::DerefMut for StagingFile {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.file
    }
}

impl Write for StagingFile {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.file
            .write(bytes)
            .map_err(|error| context("写入", self.file.path(), error))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file
            .flush()
            .map_err(|error| context("刷新", self.file.path(), error))
    }
}

impl StagingFile {
    pub(crate) fn persist(
        self,
        path: impl AsRef<Path>,
    ) -> Result<fs::File, tempfile::PersistError> {
        let path = path.as_ref();
        if let Err(error) = package_fs::clear_readonly(path) {
            return Err(tempfile::PersistError {
                error,
                file: self.file,
            });
        }
        let source =
            std::path::absolute(self.file.path()).unwrap_or_else(|_| self.file.path().to_owned());
        self.file.persist(path).map_err(|mut error| {
            error.error = context(&format!("重命名 {} →", source.display()), path, error.error);
            error
        })
    }
}

pub(crate) struct PackageStore<'a> {
    core: &'a Core,
}

impl<'a> PackageStore<'a> {
    pub(crate) fn new(core: &'a Core) -> Self {
        Self { core }
    }

    pub(crate) fn root(&self) -> PathBuf {
        keytao_core::package_store::root(&self.core.env().data_dir)
    }

    pub(crate) fn checked_root(&self) -> Result<PathBuf, CoreError> {
        let dir = self.root();
        if fs::symlink_metadata(&dir).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err(CoreError::Other(
                "Package store must not be a symlink".into(),
            ));
        }
        Ok(dir)
    }

    pub(crate) fn package_path(&self, key: &str) -> PathBuf {
        self.root()
            .join(if crate::scheme::is_local_scheme_key(key) {
                "local"
            } else {
                "cloud"
            })
            .join(key)
    }

    pub(crate) fn metadata_path(&self, key: &str) -> PathBuf {
        self.package_path(key).join("package.json")
    }

    pub(crate) fn addon_path(&self, id: &str) -> PathBuf {
        self.root().join("addons").join(id)
    }

    pub(crate) fn checked_addon_path(&self, id: &str) -> Result<PathBuf, CoreError> {
        if !crate::addon_store::IDS.contains(&id) {
            return Err(CoreError::Other("Unknown add-on package".into()));
        }
        safe_path(&self.checked_root()?, &format!("addons/{id}")).map_err(CoreError::Other)?;
        Ok(self.addon_path(id))
    }

    #[cfg(any(target_os = "windows", test))]
    pub(crate) fn addon_stage(&self) -> Result<tempfile::TempDir, CoreError> {
        let root = self.staging_root()?;
        Ok(tempfile::tempdir_in(&root).map_err(|error| context("创建临时目录", &root, error))?)
    }

    pub(crate) fn recover_addon(&self, id: &str) -> Result<(), CoreError> {
        let destination = self.checked_addon_path(id)?;
        Self::recover_addon_at(&destination, id)
    }

    pub(crate) fn recover_addon_at(destination: &Path, id: &str) -> Result<(), CoreError> {
        let old = destination.with_file_name(format!(".old-{id}"));
        safe_path(destination.parent().unwrap(), &format!(".old-{id}"))
            .map_err(CoreError::Other)?;
        if !old
            .try_exists()
            .map_err(|error| context("读取文件属性", &old, error))?
        {
            return Ok(());
        }
        if destination
            .try_exists()
            .map_err(|error| context("读取文件属性", destination, error))?
            && crate::addon_store::validate_directory(&destination, id).is_ok()
        {
            package_fs::remove_dir_all(&old)?;
        } else {
            if destination
                .try_exists()
                .map_err(|error| context("读取文件属性", destination, error))?
            {
                package_fs::remove_dir_all(destination)?;
            }
            package_fs::rename(&old, destination)?;
        }
        Ok(())
    }

    pub(crate) fn publish_addon_at(
        destination: &Path,
        id: &str,
        stage: &Path,
    ) -> Result<(), CoreError> {
        Self::recover_addon_at(destination, id)?;
        crate::addon_store::validate_directory(stage, id)?;
        sync_package_directories(stage)?;
        let parent = destination.parent().unwrap();
        fs::create_dir_all(parent).map_err(|error| context("创建目录", parent, error))?;
        publish_directory(destination, stage, package_fs::rename)
    }

    pub(crate) fn checked_package_path(&self, key: &str) -> Result<PathBuf, CoreError> {
        crate::scheme::validate_scheme_key(key).map_err(CoreError::Other)?;
        self.checked_root()?;
        let namespace = if crate::scheme::is_local_scheme_key(key) {
            "local"
        } else {
            "cloud"
        };
        safe_path(&self.root(), &format!("{namespace}/{key}")).map_err(CoreError::Other)
    }

    pub(crate) fn read_metadata_bytes(&self, key: &str) -> std::io::Result<Vec<u8>> {
        let path = self
            .checked_metadata_path(key)
            .map_err(std::io::Error::other)?;
        fs::read(&path).map_err(|error| context("读取", &path, error))
    }

    pub(crate) fn checked_metadata_path(&self, key: &str) -> Result<PathBuf, CoreError> {
        safe_path(&self.checked_package_path(key)?, "package.json").map_err(CoreError::Other)
    }

    #[cfg(test)]
    pub(crate) fn active_manifest_path(&self) -> PathBuf {
        keytao_core::package_store::active_manifest(&self.core.env().data_dir)
    }

    pub(crate) fn checked_activation_path(&self) -> Result<PathBuf, CoreError> {
        safe_path(&self.checked_root()?, "activation.json").map_err(CoreError::Other)
    }

    fn staging_root(&self) -> Result<PathBuf, CoreError> {
        self.checked_root()?;
        let path = safe_path(&self.root(), ".staging").map_err(CoreError::Other)?;
        fs::create_dir_all(&path).map_err(|error| context("创建目录", &path, error))?;
        Ok(path)
    }

    pub(crate) fn staging_file(&self, staging: Staging) -> std::io::Result<StagingFile> {
        let prefix = match staging {
            Staging::Metadata => ".meta-",
            #[cfg(test)]
            Staging::Pending => ".pending-",
            #[cfg(test)]
            Staging::ActiveManifest => ".active-",
        };
        let stage = self.stage().map_err(std::io::Error::other)?;
        let file = tempfile::Builder::new()
            .prefix(prefix)
            .tempfile_in(&stage.0)
            .map_err(|error| context("创建临时文件", &stage.0, error))?;
        Ok(StagingFile {
            file,
            _stage: stage,
        })
    }

    fn stage(&self) -> Result<PackageStage, CoreError> {
        let root = self.staging_root()?;
        let temporary =
            tempfile::tempdir_in(&root).map_err(|error| context("创建临时目录", &root, error))?;
        let digest = sha256_reader(temporary.path().as_os_str().as_encoded_bytes())?;
        let name = format!(
            "{}-{}-4{}-a{}-{}",
            &digest[..8],
            &digest[8..12],
            &digest[13..16],
            &digest[17..20],
            &digest[20..32]
        );
        let destination = root.join(name);
        package_fs::rename(temporary.path(), &destination)?;
        Ok(PackageStage(destination))
    }

    pub(crate) fn read_meta(&self, key: &str) -> Result<Option<PackageMeta>, CoreError> {
        let _guard = self
            .core
            .pending_package
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let bytes = match self.read_metadata_bytes(key) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let stored: StoredPackage = serde_json::from_slice(&bytes)?;
        validate_metadata(key, &stored)?;
        Ok(Some(stored.meta))
    }

    pub(crate) fn set_engine(
        &self,
        root: &Path,
        key: &str,
        engine: PackageEngine,
    ) -> Result<(), CoreError> {
        use crate::scheme_files::write_file_atomic;

        let _pending = self.core.pending_package.lock().unwrap_or_else(|error| error.into_inner());
        let path = self.checked_metadata_path(key)?;
        let before = fs::read(&path)?;
        validate_metadata(key, &serde_json::from_slice(&before)?)?;
        if key != PACKAGE_KEY {
            return Ok(());
        }
        let mut metadata: serde_json::Value = serde_json::from_slice(&before)?;
        if engine.is_rime() {
            metadata.as_object_mut().unwrap().remove("engine");
        } else {
            metadata["engine"] = serde_json::to_value(engine)?;
        }
        let settings_path = safe_path(root, keytao_core::english_mode::SETTINGS_FILE)
            .map_err(CoreError::Other)?;
        let mut settings: serde_json::Map<String, serde_json::Value> = match fs::read(&settings_path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Default::default(),
            Err(error) => return Err(error.into()),
        };
        let engines = settings
            .entry("engines")
            .or_insert_with(|| serde_json::json!({}));
        let engines = engines
            .as_object_mut()
            .ok_or_else(|| CoreError::Other("输入引擎设置格式无效".into()))?;
        if engine.is_rime() {
            engines.remove(key);
        } else {
            engines.insert(key.into(), serde_json::to_value(engine)?);
        }
        let settings = serde_json::to_vec_pretty(&settings)?;
        let metadata = serde_json::to_vec_pretty(&metadata)?;
        write_file_atomic(&path, &metadata)?;
        if let Err(error) = write_file_atomic(&settings_path, &settings) {
            write_file_atomic(&path, &before)
                .map_err(|restore| CoreError::Other(format!("{error}; {restore}")))?;
            return Err(error.into());
        }
        Ok(())
    }

    pub(crate) fn read_files_at(
        path: &Path,
        key: &str,
    ) -> Result<(Vec<(String, Vec<u8>)>, PackageMeta), CoreError> {
        Self::read_files_at_inner(path, key, false)
    }

    #[cfg(test)]
    pub(crate) fn read_lua_files_at(
        path: &Path,
        key: &str,
    ) -> Result<(Vec<(String, Vec<u8>)>, PackageMeta), CoreError> {
        Self::read_files_at_inner(path, key, true)
    }

    fn read_files_at_inner(
        path: &Path,
        key: &str,
        lua_only: bool,
    ) -> Result<(Vec<(String, Vec<u8>)>, PackageMeta), CoreError> {
        let (stored, paths) = Self::file_inventory_at(path, key, lua_only)?;
        let entries = stored
            .files
            .into_iter()
            .zip(paths)
            .map(|(file, path)| {
                let bytes = Self::read_verified_file(&path, &file)?;
                Ok((file.path, bytes))
            })
            .collect::<Result<_, CoreError>>()?;
        Ok((entries, stored.meta))
    }

    pub(crate) fn file_inventory_at(
        path: &Path,
        key: &str,
        lua_only: bool,
    ) -> Result<(StoredPackage, Vec<PathBuf>), CoreError> {
        let metadata_path = safe_path(path, "package.json").map_err(CoreError::Other)?;
        let mut stored: StoredPackage = serde_json::from_slice(
            &fs::read(&metadata_path).map_err(|error| context("读取", &metadata_path, error))?,
        )?;
        validate_metadata(key, &stored)?;
        if lua_only {
            stored.files.retain(|entry| entry.path == "rime.lua" || entry.path.starts_with("lua/"));
            if stored.files.is_empty() {
                return Ok((stored, Vec::new()));
            }
        }
        let root = safe_path(path, "files").map_err(CoreError::Other)?;
        let mut expected = BTreeSet::new();
        let mut entries = Vec::with_capacity(stored.files.len());
        let mut total = 0u64;
        for entry in &stored.files {
            if !expected.insert(entry.path.clone()) {
                return Err(CoreError::Other("Duplicate package file".into()));
            }
            let path = safe_path(&root, &entry.path).map_err(CoreError::Other)?;
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| context("读取文件属性", &path, error))?;
            total = total.saturating_add(metadata.len());
            if !metadata.is_file() || metadata.len() != entry.size || total > 512 * 1024 * 1024 {
                return Err(CoreError::Other("Package file size mismatch".into()));
            }
            entries.push(path);
        }
        let mut actual = BTreeSet::new();
        collect_files(&root, &root, &mut actual, lua_only)?;
        if actual != expected {
            return Err(CoreError::Other("Package file count mismatch".into()));
        }
        Ok((stored, entries))
    }

    pub(crate) fn read_verified_file(path: &Path, file: &PackageFile) -> Result<Vec<u8>, CoreError> {
        let mut bytes = Vec::new();
        fs::File::open(path)
            .and_then(|input| input.take(file.size.saturating_add(1)).read_to_end(&mut bytes))
            .map_err(|error| context("读取", path, error))?;
        if bytes.len() as u64 != file.size || sha256_reader(bytes.as_slice())? != file.sha256 {
            return Err(CoreError::Other("Package file checksum mismatch".into()));
        }
        Ok(bytes)
    }

    pub(crate) fn read_files(
        &self,
        key: &str,
    ) -> Result<(Vec<(String, Vec<u8>)>, PackageMeta), CoreError> {
        let _guard = self
            .core
            .pending_package
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        Self::read_files_at(&self.checked_package_path(key)?, key)
    }

    #[cfg(test)]
    pub(crate) fn read_lua_files(
        &self,
        key: &str,
    ) -> Result<(Vec<(String, Vec<u8>)>, PackageMeta), CoreError> {
        let _guard = self
            .core
            .pending_package
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        Self::read_lua_files_at(&self.checked_package_path(key)?, key)
    }

    pub(crate) fn write_archive(
        &self,
        reader: impl Read + Seek,
        meta: &PackageMeta,
        origin: Option<&str>,
    ) -> Result<(), CoreError> {
        self.write_archive_with(reader, meta, origin, package_fs::rename)
    }

    fn write_archive_with(
        &self,
        reader: impl Read + Seek,
        meta: &PackageMeta,
        origin: Option<&str>,
        rename_new: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
    ) -> Result<(), CoreError> {
        let destination = self.checked_package_path(&meta.scheme_key)?;
        let stage = self.stage()?;
        let files_root = stage.0.join("files");
        fs::create_dir(&files_root).map_err(|error| context("创建目录", &files_root, error))?;
        let mut archive =
            zip::ZipArchive::new(reader).map_err(|error| CoreError::Other(error.to_string()))?;
        if archive.len() > 10_000 {
            return Err(CoreError::Other("Package contains too many files".into()));
        }
        let mut files = Vec::new();
        let mut names = BTreeSet::new();
        let mut total = 0u64;
        for index in 0..archive.len() {
            let mut entry = archive
                .by_index(index)
                .map_err(|error| CoreError::Other(error.to_string()))?;
            let relative = entry.name().trim_end_matches('/').to_owned();
            let target = safe_path(&files_root, &relative).map_err(CoreError::Other)?;
            if entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 == 0o120000)
            {
                return Err(CoreError::Other("Package contains a symlink".into()));
            }
            if !names.insert(relative.to_lowercase()) {
                return Err(CoreError::Other(format!(
                    "Package contains case-insensitive duplicate path: {relative}"
                )));
            }
            if entry.is_dir() {
                continue;
            }
            total = total.saturating_add(entry.size());
            if total > 512 * 1024 * 1024 {
                return Err(CoreError::Other("Expanded package is too large".into()));
            }
            let parent = target.parent().unwrap();
            fs::create_dir_all(parent).map_err(|error| context("创建目录", parent, error))?;
            let mut output =
                fs::File::create(&target).map_err(|error| context("创建文件", &target, error))?;
            package_fs::clear_readonly(&target)?;
            let expected_size = entry.size();
            let size = std::io::copy(
                &mut entry.by_ref().take(expected_size.saturating_add(1)),
                &mut output,
            )
            .map_err(|error| context("解压写入", &target, error))?;
            if size != expected_size {
                return Err(CoreError::Other("Package file size mismatch".into()));
            }
            output
                .sync_all()
                .map_err(|error| context("同步文件", &target, error))?;
            files.push(PackageFile {
                path: relative,
                sha256: sha256_reader(
                    fs::File::open(&target).map_err(|error| context("打开文件", &target, error))?,
                )
                .map_err(|error| context("读取文件校验和", &target, error))?,
                size,
            });
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let previous = self.read_metadata_bytes(&meta.scheme_key).ok()
            .map(|bytes| serde_json::from_slice::<StoredPackage>(&bytes))
            .transpose()?;
        let selected_schemas = previous.as_ref().and_then(|stored| stored.selected_schemas.as_ref())
            .map(|selected| {
                let incoming = files.iter().map(|file| {
                    Ok((file.path.clone(), fs::read(files_root.join(&file.path))?))
                }).collect::<Result<Vec<_>, CoreError>>()?;
                crate::package_schemas::reconcile_selection(
                    &meta.scheme_key, &incoming, selected,
                    &previous.as_ref().unwrap().files.iter().map(|file| file.path.clone()).collect::<Vec<_>>(),
                )
            }).transpose()?;
        let mut meta: PackageMeta = serde_json::from_value(serde_json::to_value(meta)?)?;
        if let Some(previous) = previous.as_ref() {
            meta.engine = previous.meta.engine;
        }
        let stored = StoredPackage {
            key: meta.scheme_key.clone(),
            origin: origin.map(str::to_owned).or_else(|| previous.and_then(|stored| stored.origin)),
            installed_at: meta.installed_at_unix,
            files,
            variant: None,
            grammar: None,
            addon_manifest: None,
            selected_schemas,
            meta,
        };
        let meta = &stored.meta;
        validate_metadata(&meta.scheme_key, &stored)?;
        let metadata_path = stage.0.join("package.json");
        let mut metadata = fs::File::create(&metadata_path)
            .map_err(|error| context("创建文件", &metadata_path, error))?;
        package_fs::clear_readonly(&metadata_path)?;
        serde_json::to_writer_pretty(&mut metadata, &stored).map_err(|error| {
            CoreError::Other(format!("写入 {} 失败：{error}", metadata_path.display()))
        })?;
        metadata
            .sync_all()
            .map_err(|error| context("同步文件", &metadata_path, error))?;
        // Windows refuses to rename a directory while any file beneath it is open.
        drop(metadata);
        Self::read_files_at(&stage.0, &meta.scheme_key)?;
        sync_package_directories(&stage.0)?;
        let parent = destination.parent().unwrap();
        fs::create_dir_all(parent).map_err(|error| context("创建目录", parent, error))?;
        sync_directory(&self.root())?;
        self.recover_old(&meta.scheme_key)?;
        publish_directory(&destination, &stage.0, rename_new)
    }

    fn recover_old(&self, key: &str) -> Result<(), CoreError> {
        let destination = self.checked_package_path(key)?;
        let old = destination.with_file_name(format!(".old-{key}"));
        if !old
            .try_exists()
            .map_err(|error| context("读取文件属性", &old, error))?
        {
            return Ok(());
        }
        safe_path(
            destination.parent().unwrap(),
            old.file_name().unwrap().to_str().unwrap(),
        )
        .map_err(CoreError::Other)?;
        if destination
            .try_exists()
            .map_err(|error| context("读取文件属性", &destination, error))?
            && Self::read_files_at(&destination, key).is_ok()
        {
            package_fs::remove_dir_all(&old)?;
        } else {
            if destination
                .try_exists()
                .map_err(|error| context("读取文件属性", &destination, error))?
            {
                package_fs::remove_dir_all(&destination)?;
            }
            package_fs::rename(&old, &destination)?;
        }
        Ok(())
    }

    pub(crate) fn remove(&self, key: &str) -> Result<(), CoreError> {
        self.remove_with(key, package_fs::remove_dir_all)
    }

    fn remove_with(
        &self,
        key: &str,
        delete: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> Result<(), CoreError> {
        let _guard = self
            .core
            .pending_package
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let destination = self.checked_package_path(key)?;
        let tombstone = self.stage()?;
        for (path, name) in [
            (destination.with_file_name(format!(".old-{key}")), "old"),
            (destination.clone(), "package"),
        ] {
            match package_fs::rename(&path, &tombstone.0.join(name)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        sync_directory(&tombstone.0)?;
        sync_directory(tombstone.0.parent().unwrap())?;
        if destination.parent().unwrap().exists() {
            sync_directory(destination.parent().unwrap())?;
        }
        delete(&tombstone.0).map_err(|error| context("删除目录", &tombstone.0, error))?;
        sync_directory(tombstone.0.parent().unwrap())?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn export_archive(
        &self,
        key: &str,
    ) -> Result<(StagingFile, PackageMeta), CoreError> {
        let (entries, meta) = self.read_files(key)?;
        let stage = self.stage()?;
        let file = tempfile::Builder::new()
            .prefix(&format!("{key}-"))
            .suffix(".zip")
            .tempfile_in(&stage.0)?;
        let mut temporary = StagingFile {
            file,
            _stage: stage,
        };
        {
            let mut archive = zip::ZipWriter::new(temporary.as_file_mut());
            for (path, bytes) in entries {
                archive
                    .start_file(path, zip::write::SimpleFileOptions::default())
                    .map_err(|error| CoreError::Other(error.to_string()))?;
                archive.write_all(&bytes)?;
            }
            archive
                .finish()
                .map_err(|error| CoreError::Other(error.to_string()))?;
        }
        temporary.as_file_mut().rewind()?;
        Ok((temporary, meta))
    }

    pub(crate) fn migrate(&self) -> Result<(), CoreError> {
        let _guard = self
            .core
            .pending_package
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let root = self.checked_root()?;
        if !root
            .try_exists()
            .map_err(|error| context("读取文件属性", &root, error))?
        {
            return Ok(());
        }
        let staging = root.join(".staging");
        match fs::symlink_metadata(&staging) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                package_fs::remove_dir_all(&staging)?;
            }
            Ok(_) => package_fs::remove_file(&staging)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(context("读取文件属性", &staging, error).into()),
        }
        for namespace in ["cloud", "local"] {
            let directory = safe_path(&root, namespace).map_err(CoreError::Other)?;
            if directory.exists() {
                for entry in fs::read_dir(&directory)
                    .map_err(|error| context("读取目录", &directory, error))?
                {
                    let name = entry
                        .map_err(|error| context("读取目录项", &directory, error))?
                        .file_name();
                    if let Some(key) = name.to_str().and_then(|name| name.strip_prefix(".old-")) {
                        self.recover_old(key)?;
                    }
                }
            }
        }
        for entry in fs::read_dir(&root).map_err(|error| context("读取目录", &root, error))? {
            let entry = entry.map_err(|error| context("读取目录项", &root, error))?;
            let name = entry.file_name();
            let Some(key) = name.to_str().and_then(|name| name.strip_suffix(".json")) else {
                continue;
            };
            if crate::scheme::validate_scheme_key(key).is_ok()
                && !root
                    .join(format!("{key}.zip"))
                    .try_exists()
                    .map_err(|error| {
                        context("读取文件属性", &root.join(format!("{key}.zip")), error)
                    })?
                && Self::read_files_at(&self.checked_package_path(key)?, key).is_ok()
            {
                let path = safe_path(&root, &format!("{key}.json")).map_err(CoreError::Other)?;
                let legacy: PackageMeta = serde_json::from_slice(
                    &fs::read(&path).map_err(|error| context("读取", &path, error))?,
                )?;
                if legacy.scheme_key == key {
                    package_fs::remove_file(&path)?;
                }
            }
        }
        let marker = safe_path(&root, ".layout-v2").map_err(CoreError::Other)?;
        if marker.exists() {
            return Ok(());
        }
        let mut failed = false;
        for entry in fs::read_dir(&root).map_err(|error| context("读取目录", &root, error))? {
            let entry = entry.map_err(|error| context("读取目录项", &root, error))?;
            let name = entry.file_name();
            let Some(key) = name.to_str().and_then(|name| name.strip_suffix(".zip")) else {
                continue;
            };
            let result = (|| {
                crate::scheme::validate_scheme_key(key).map_err(CoreError::Other)?;
                let zip_path = safe_path(&root, &format!("{key}.zip")).map_err(CoreError::Other)?;
                let json_path =
                    safe_path(&root, &format!("{key}.json")).map_err(CoreError::Other)?;
                let installed = Self::read_files_at(&self.checked_package_path(key)?, key).is_ok();
                let metadata = match fs::read(&json_path) {
                    Ok(bytes) => Some(bytes),
                    // A published directory is authoritative, including updates after migration.
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound && installed => None,
                    Err(error) => return Err(context("读取", &json_path, error).into()),
                };
                if let Some(metadata) = &metadata {
                    let meta: PackageMeta = serde_json::from_slice(metadata)?;
                    if meta.scheme_key != key
                        || sha256_reader(
                            fs::File::open(&zip_path)
                                .map_err(|error| context("打开文件", &zip_path, error))?,
                        )
                        .map_err(|error| context("读取文件校验和", &zip_path, error))?
                            != meta.zip_sha256
                    {
                        return Err(CoreError::Other("Legacy package checksum mismatch".into()));
                    }
                    if !installed {
                        let report_path = safe_path(&root, &format!("{key}.migration-error.json"))
                            .map_err(CoreError::Other)?;
                        if fs::read(report_path)
                            .ok()
                            .and_then(|bytes| {
                                serde_json::from_slice::<serde_json::Value>(&bytes).ok()
                            })
                            .is_some_and(|report| {
                                report["status"] == "rejected"
                                    && report["archive_sha256"] == meta.zip_sha256
                            })
                        {
                            return Ok(());
                        }
                        self.write_archive(
                            fs::File::open(&zip_path)
                                .map_err(|error| context("打开文件", &zip_path, error))?,
                            &meta,
                            None,
                        )?;
                    }
                }
                // Keep the archive until the new directory has passed a complete read-back.
                Self::read_files_at(&self.checked_package_path(key)?, key)?;
                match package_fs::remove_file(&json_path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
                if let Err(error) = package_fs::remove_file(&zip_path) {
                    if let Some(metadata) = metadata {
                        if let Err(restore) = fs::write(&json_path, metadata)
                            .map_err(|error| context("写入", &json_path, error))
                        {
                            tracing::warn!(%restore, "Legacy metadata cleanup will resume from published package");
                        }
                    }
                    return Err(error.into());
                }
                Ok::<_, CoreError>(())
            })();
            if let Err(error) = result {
                if matches!(&error, CoreError::Other(message) if message.starts_with("Package contains case-insensitive duplicate path:"))
                {
                    let report_path = safe_path(&root, &format!("{key}.migration-error.json"))
                        .map_err(CoreError::Other)?;
                    let mut report = self.staging_file(Staging::Metadata)?;
                    let zip_path = root.join(format!("{key}.zip"));
                    let archive_sha256 = sha256_reader(
                        fs::File::open(&zip_path)
                            .map_err(|error| context("打开文件", &zip_path, error))?,
                    )
                    .map_err(|error| context("读取文件校验和", &zip_path, error))?;
                    serde_json::to_writer_pretty(
                        &mut report,
                        &serde_json::json!({
                            "key": key,
                            "error": error.to_string(),
                            "archive": format!("{key}.zip"),
                            "archive_sha256": archive_sha256,
                            "status": "rejected",
                            "recovery": "Rename duplicate paths and import the package again. The original archive and metadata have been retained.",
                        }),
                    )?;
                    report
                        .as_file()
                        .sync_all()
                        .map_err(|error| context("同步文件", report.path(), error))?;
                    report
                        .persist(report_path)
                        .map_err(|error| CoreError::Io(error.error))?;
                    sync_directory(&root)?;
                    tracing::warn!(%key, %error, "Legacy package rejected; see migration-error.json report");
                    continue;
                }
                failed = true;
                tracing::warn!(%key, %error, "Package migration deferred; legacy archive retained");
            }
        }
        if !failed {
            let mut temporary = self.staging_file(Staging::Metadata)?;
            temporary.write_all(b"2\n")?;
            temporary
                .as_file()
                .sync_all()
                .map_err(|error| context("同步文件", temporary.path(), error))?;
            temporary
                .persist(marker)
                .map_err(|error| CoreError::Io(error.error))?;
            sync_directory(&root)?;
        }
        Ok(())
    }

    pub(crate) fn list_installed_packages(&self) -> Result<Vec<InstalledPackageInfo>, CoreError> {
        self.checked_root()?;
        let core = self.core;
        let state = core.state();
        Ok(state.installed_packages.into_iter().filter_map(|(scheme_key, package)| {
            let has_package_zip = self.checked_package_path(&scheme_key).is_ok_and(|path| path.is_dir());
            let meta = if crate::scheme::validate_scheme_key(&scheme_key).is_ok() {
                match self.read_meta(&scheme_key) {
                    Ok(meta) => meta,
                    Err(error) => {
                        tracing::warn!(%scheme_key, %error, "Skipping unreadable package metadata");
                        return None;
                    }
                }
            } else { None };
            let active = state.active_scheme.as_deref() == Some(&scheme_key);
            let engine = meta.as_ref().map_or(PackageEngine::Rime, |meta| meta.engine);
            let git = meta.as_ref().and_then(|meta| meta.git.clone());
            Some(InstalledPackageInfo {
                logo_path: if has_package_zip {
                    crate::packages::package_logos::package_logo_path(core, &scheme_key).map(|path| path.to_string_lossy().into_owned())
                } else { None },
                active,
                in_use: active,
                supports_user_words: engine.is_rime() && crate::user_words::supports_user_words(core, &scheme_key).unwrap_or_else(|error| {
                    tracing::warn!(%scheme_key, %error, "Cannot read user dictionary support");
                    false
                }),
                scheme_key,
                engine: engine.as_str().into(),
                version: package.version,
                has_package_zip,
                source: meta.as_ref().map(|meta| meta.source.clone()).unwrap_or_else(|| "official".into()),
                display_name: meta.and_then(|meta| meta.display_name),
                git,
            })
        }).collect())
    }
}

fn validate_metadata(key: &str, stored: &StoredPackage) -> Result<(), CoreError> {
    crate::scheme::validate_scheme_key(key).map_err(CoreError::Other)?;
    if stored.key != key
        || stored.meta.scheme_key != key
        || (key != PACKAGE_KEY && !stored.meta.engine.is_rime())
        || stored.files.len() > 10_000
        || stored.meta.zip_sha256.len() != 64
        || !stored
            .meta
            .zip_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || stored.installed_at != stored.meta.installed_at_unix
    {
        return Err(CoreError::Other("Invalid package metadata".into()));
    }
    Ok(())
}

pub(crate) fn collect_files(
    root: &Path,
    directory: &Path,
    files: &mut BTreeSet<String>,
    lua_only: bool,
) -> Result<(), CoreError> {
    for entry in fs::read_dir(directory).map_err(|error| context("读取目录", directory, error))?
    {
        let entry = entry.map_err(|error| context("读取目录项", directory, error))?;
        if lua_only && directory == root
            && !matches!(entry.file_name().to_str(), Some("rime.lua" | "lua"))
        {
            continue;
        }
        let path = entry.path();
        let metadata =
            fs::symlink_metadata(&path).map_err(|error| context("读取文件属性", &path, error))?;
        if metadata.file_type().is_symlink() {
            return Err(CoreError::Other("Package contains a symlink".into()));
        }
        if metadata.is_dir() {
            collect_files(root, &entry.path(), files, lua_only)?;
        } else if metadata.is_file() {
            files.insert(
                entry
                    .path()
                    .strip_prefix(root)
                    .unwrap()
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/"),
            );
        } else {
            return Err(CoreError::Other("Package contains a special file".into()));
        }
    }
    Ok(())
}

pub(crate) fn publish_directory(
    destination: &Path,
    stage: &Path,
    rename_new: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> Result<(), CoreError> {
    let name = destination.file_name().unwrap().to_string_lossy();
    let old = destination.with_file_name(format!(".old-{name}"));
    let had_old = destination
        .try_exists()
        .map_err(|error| context("读取文件属性", destination, error))?;
    if had_old {
        package_fs::rename(destination, &old)?;
        if let Err(error) = sync_directory(destination.parent().unwrap()) {
            package_fs::rename(&old, destination)?;
            return Err(error.into());
        }
    }
    if let Err(error) = rename_new(stage, destination) {
        let source = std::path::absolute(stage).unwrap_or_else(|_| stage.to_owned());
        let error = context(
            &format!("重命名 {} →", source.display()),
            destination,
            error,
        );
        if had_old {
            package_fs::rename(&old, destination).map_err(|rollback| {
                CoreError::Other(format!(
                    "Package update failed: {error}; rollback failed: {rollback}"
                ))
            })?;
        }
        return Err(error.into());
    }
    if let Err(error) = sync_directory(destination.parent().unwrap()) {
        package_fs::remove_dir_all(destination)?;
        if had_old {
            package_fs::rename(&old, destination)?;
        }
        return Err(error.into());
    }
    if had_old {
        if let Err(error) = package_fs::remove_dir_all(&old) {
            tracing::warn!(%error, "Old package cleanup deferred until next launch");
        }
    }
    Ok(())
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| context("同步目录", path, error))?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn sync_package_directories(path: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(path).map_err(|error| context("读取目录", path, error))? {
        let entry = entry.map_err(|error| context("读取目录项", path, error))?;
        if entry
            .file_type()
            .map_err(|error| context("读取文件属性", &entry.path(), error))?
            .is_dir()
        {
            sync_package_directories(&entry.path())?;
        }
    }
    sync_directory(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppEnv, CoreEvent, EventSink, Platform};
    use std::{collections::BTreeMap, io::Cursor, sync::Arc};

    struct NullSink;
    impl EventSink for NullSink {
        fn emit(&self, _: CoreEvent) {}
    }

    fn core(path: &Path) -> Arc<Core> {
        Core::new(
            AppEnv {
                data_dir: path.into(),
                cache_dir: path.join("cache"),
                resource_dir: path.join("resources"),
                app_version: "test".into(),
                platform: Platform::Android,
            },
            Arc::new(NullSink),
        )
        .unwrap()
    }

    fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (path, bytes) in entries {
            writer
                .start_file(*path, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn metadata(key: &str, zip: &[u8]) -> PackageMeta {
        PackageMeta {
            scheme_key: key.into(),
            engine: PackageEngine::Rime,
            version: Some("v1".into()),
            installed_at_unix: 7,
            source: if key.starts_with("local-") {
                "local"
            } else {
                "official"
            }
            .into(),
            display_name: Some("Example".into()),
            selection_base_version: None,
            git: None,
            manifest: BTreeMap::new(),
            zip_sha256: sha256_reader(zip).unwrap(),
        }
    }

    #[test]
    fn package_engine_mirror_preserves_settings_and_rolls_back_on_write_failure() {
        let directory = tempfile::tempdir().unwrap();
        let core = core(directory.path());
        let store = PackageStore::new(&core);
        let package = store.package_path(PACKAGE_KEY);
        fs::create_dir_all(&package).unwrap();
        let meta = metadata(PACKAGE_KEY, b"fixture");
        let mut record = serde_json::to_value(&meta).unwrap();
        record["key"] = PACKAGE_KEY.into();
        record["files"] = serde_json::json!([]);
        record["installed_at"] = meta.installed_at_unix.into();
        let path = package.join("package.json");
        let original = serde_json::to_vec(&record).unwrap();
        fs::write(&path, &original).unwrap();

        let ime = directory.path().join("ime");
        assert!(store.set_engine(&ime, PACKAGE_KEY, PackageEngine::Libjd).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);

        fs::create_dir(&ime).unwrap();
        let settings = ime.join(keytao_core::english_mode::SETTINGS_FILE);
        fs::write(&settings, br#"{"candidatePageSize":7}"#).unwrap();
        for engine in [PackageEngine::Libjd, PackageEngine::Rime] {
            store.set_engine(&ime, PACKAGE_KEY, engine).unwrap();
            assert_eq!(store.read_meta(PACKAGE_KEY).unwrap().unwrap().engine, engine);
            assert_eq!(keytao_core::engine_choice::read(&ime, Some(PACKAGE_KEY)), engine);
            assert_eq!(crate::ime_settings::read_candidate_page_size(&ime), Some(7));
        }
    }

    fn legacy_fixture(store: &PackageStore<'_>, key: &str, bytes: &[u8]) {
        fs::create_dir_all(store.root()).unwrap();
        fs::write(store.root().join(format!("{key}.zip")), bytes).unwrap();
        fs::write(
            store.root().join(format!("{key}.json")),
            serde_json::to_vec_pretty(&metadata(key, bytes)).unwrap(),
        )
        .unwrap();
        let _ = fs::remove_file(store.root().join(".layout-v2"));
    }

    #[test]
    fn lua_reads_skip_non_lua_files_and_verify_lua_contents() {
        let directory = tempfile::tempdir().unwrap();
        let core = core(directory.path());
        let store = PackageStore::new(&core);
        for with_lua in [false, true] {
            let mut entries: Vec<(&str, &[u8])> = vec![("large.dict.yaml", b"dictionary")];
            if with_lua {
                entries.extend([("rime.lua", b"return 1".as_slice()), ("lua/nested/data.tsv", b"data".as_slice())]);
            }
            let bytes = archive(&entries);
            store.write_archive(Cursor::new(&bytes), &metadata("keytao", &bytes), None).unwrap();
            let root = store.package_path("keytao").join("files");
            fs::remove_file(root.join("large.dict.yaml")).unwrap();
            assert!(store.read_files("keytao").is_err());
            let files = store.read_lua_files("keytao").unwrap().0;
            assert_eq!(files.len(), if with_lua { 2 } else { 0 });
            if with_lua {
                assert!(files.contains(&("rime.lua".into(), b"return 1".to_vec())));
                assert!(files.contains(&("lua/nested/data.tsv".into(), b"data".to_vec())));
                fs::write(root.join("rime.lua"), b"return 2").unwrap();
                assert!(store.read_lua_files("keytao").err().unwrap().to_string().contains("checksum mismatch"));
                fs::write(root.join("rime.lua"), b"short").unwrap();
                assert!(store.read_lua_files("keytao").err().unwrap().to_string().contains("size mismatch"));
                fs::write(root.join("rime.lua"), b"return 1").unwrap();
                fs::write(root.join("lua/nested/extra.lua"), b"unlisted").unwrap();
                assert!(store.read_lua_files("keytao").err().unwrap().to_string().contains("count mismatch"));
            }
        }
    }

    #[test]
    #[ignore = "manual large-package timing fixture"]
    fn lua_read_large_non_lua_package_timing() {
        let directory = tempfile::tempdir().unwrap();
        let core = core(directory.path());
        let store = PackageStore::new(&core);
        let bytes = archive(&[("large.dict.yaml", b"")]);
        store.write_archive(Cursor::new(&bytes), &metadata("keytao", &bytes), None).unwrap();
        let path = store.package_path("keytao").join("files/large.dict.yaml");
        let size = 128 * 1024 * 1024;
        fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(size).unwrap();
        let mut stored: StoredPackage = serde_json::from_slice(&store.read_metadata_bytes("keytao").unwrap()).unwrap();
        stored.files[0].size = size;
        stored.files[0].sha256 = sha256_reader(fs::File::open(&path).unwrap()).unwrap();
        fs::write(store.checked_metadata_path("keytao").unwrap(), serde_json::to_vec(&stored).unwrap()).unwrap();
        for lua_only in [false, true] {
            let mut elapsed = Vec::new();
            for _ in 0..5 {
                let start = std::time::Instant::now();
                let files = if lua_only { store.read_lua_files("keytao") } else { store.read_files("keytao") }.unwrap().0;
                elapsed.push(start.elapsed());
                assert_eq!(files.len(), if lua_only { 0 } else { 1 });
            }
            elapsed.sort();
            eprintln!("128 MiB non-Lua package, lua_only={lua_only}, median of 5: {:?}", elapsed[2]);
        }
    }

    #[test]
    fn directory_package_store_layout_golden() {
        let directory = tempfile::tempdir().unwrap();
        let core = core(directory.path());
        let store = PackageStore::new(&core);
        let bytes = archive(&[("example.schema.yaml", b"abc")]);
        let meta = metadata("local-example", &bytes);
        store
            .write_archive(Cursor::new(&bytes), &meta, Some("/import/example"))
            .unwrap();
        assert_eq!(store.root(), directory.path().join("scheme-packages"));
        assert_eq!(
            store.package_path("keytao"),
            store.root().join("cloud/keytao")
        );
        assert_eq!(
            store.package_path("local-example"),
            store.root().join("local/local-example")
        );
        assert_eq!(
            store.metadata_path("local-example"),
            store.package_path("local-example").join("package.json")
        );
        assert_eq!(
            store.active_manifest_path(),
            store.root().join("active-manifest.json")
        );
        let stored: serde_json::Value =
            serde_json::from_slice(&store.read_metadata_bytes("local-example").unwrap()).unwrap();
        assert_eq!(
            stored,
            serde_json::json!({
                "key": "local-example", "origin": "/import/example", "installed_at": 7,
                "files": [{"path": "example.schema.yaml", "size": 3, "sha256": "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"}],
                "scheme_key": "local-example", "version": "v1", "installed_at_unix": 7,
                "source": "local", "display_name": "Example", "manifest": {}, "zip_sha256": meta.zip_sha256,
            })
        );
        assert_eq!(
            store.read_files("local-example").unwrap().0,
            vec![("example.schema.yaml".into(), b"abc".to_vec())]
        );
        assert!(store.read_meta("keytao").unwrap().is_none());
        assert!(!store.root().join("local-example.zip").exists());
        assert!(!store.root().join("local-example.json").exists());
        assert_eq!(
            fs::read_dir(store.root().join(".staging")).unwrap().count(),
            0
        );
        for kind in [Staging::Metadata, Staging::Pending, Staging::ActiveManifest] {
            let staged = store.staging_file(kind).unwrap();
            let parent = staged.path().parent().unwrap();
            assert_eq!(parent.parent().unwrap(), store.root().join(".staging"));
            assert_eq!(parent.file_name().unwrap().to_str().unwrap().len(), 36);
        }
    }

    #[test]
    fn legacy_layout_fixture_migrates_idempotently() {
        let directory = tempfile::tempdir().unwrap();
        let core = core(directory.path());
        let store = PackageStore::new(&core);
        let bytes = archive(&[
            ("example.schema.yaml", b"schema"),
            ("lua/example.lua", b"return {}"),
        ]);
        legacy_fixture(&store, "keytao", &bytes);
        legacy_fixture(&store, "local-example", &bytes);
        fs::write(store.active_manifest_path(), b"preserved activation").unwrap();
        store.migrate().unwrap();
        let before = store.read_metadata_bytes("keytao").unwrap();
        assert!(store.root().join(".layout-v2").is_file());
        for key in ["keytao", "local-example"] {
            assert_eq!(store.read_files(key).unwrap().0.len(), 2);
            assert!(!store.root().join(format!("{key}.zip")).exists());
            assert!(!store.root().join(format!("{key}.json")).exists());
        }
        store.migrate().unwrap();
        assert_eq!(store.read_metadata_bytes("keytao").unwrap(), before);
        assert_eq!(
            fs::read(store.active_manifest_path()).unwrap(),
            b"preserved activation"
        );
    }

    #[test]
    fn migration_failure_after_extraction_keeps_zip_and_retries() {
        let directory = tempfile::tempdir().unwrap();
        let core = core(directory.path());
        let store = PackageStore::new(&core);
        let bad = archive(&[
            ("first.schema.yaml", b"copied before failure"),
            ("../escape", b"unsafe"),
        ]);
        legacy_fixture(&store, "keytao", &bad);
        store.migrate().unwrap();
        assert_eq!(fs::read(store.root().join("keytao.zip")).unwrap(), bad);
        assert!(store.root().join("keytao.json").is_file());
        assert!(!store.root().join(".layout-v2").exists());
        assert!(!store.package_path("keytao").exists());
        assert_eq!(
            fs::read_dir(store.root().join(".staging")).unwrap().count(),
            0
        );
        let good = archive(&[("first.schema.yaml", b"retry succeeds")]);
        legacy_fixture(&store, "keytao", &good);
        store.migrate().unwrap();
        assert_eq!(
            store.read_files("keytao").unwrap().0[0].1,
            b"retry succeeds"
        );
        assert!(store.root().join(".layout-v2").is_file());
    }

    #[test]
    fn migration_checksum_failure_preserves_legacy_files() {
        let directory = tempfile::tempdir().unwrap();
        let core = core(directory.path());
        let store = PackageStore::new(&core);
        let bytes = archive(&[("example.schema.yaml", b"schema")]);
        legacy_fixture(&store, "keytao", &bytes);
        fs::write(store.root().join("keytao.zip"), b"corrupt").unwrap();
        store.migrate().unwrap();
        assert_eq!(
            fs::read(store.root().join("keytao.zip")).unwrap(),
            b"corrupt"
        );
        assert!(!store.root().join(".layout-v2").exists());
    }

    #[test]
    fn migration_reports_case_duplicates_once_and_keeps_originals() {
        let directory = tempfile::tempdir().unwrap();
        let core = core(directory.path());
        let store = PackageStore::new(&core);
        let duplicate = archive(&[
            ("Example.schema.yaml", b"first"),
            ("example.schema.yaml", b"second"),
        ]);
        legacy_fixture(&store, "keytao", &duplicate);
        legacy_fixture(&store, "local-retry", &archive(&[("../unsafe", b"retry")]));
        store.migrate().unwrap();
        assert!(!store.root().join(".layout-v2").exists());
        assert!(!store.package_path("keytao").exists());
        assert_eq!(
            fs::read(store.root().join("keytao.zip")).unwrap(),
            duplicate
        );
        assert!(store.root().join("keytao.json").is_file());
        let report_path = store.root().join("keytao.migration-error.json");
        let mut report: serde_json::Value =
            serde_json::from_slice(&fs::read(&report_path).unwrap()).unwrap();
        assert_eq!(report["status"], "rejected");
        assert!(
            report["error"]
                .as_str()
                .unwrap()
                .contains("case-insensitive duplicate")
        );
        assert_eq!(
            report["archive_sha256"],
            sha256_reader(duplicate.as_slice()).unwrap()
        );
        report["sentinel"] = true.into();
        let report_bytes = serde_json::to_vec(&report).unwrap();
        fs::write(&report_path, &report_bytes).unwrap();
        store.migrate().unwrap();
        assert_eq!(fs::read(&report_path).unwrap(), report_bytes);
        legacy_fixture(
            &store,
            "local-retry",
            &archive(&[("valid.schema.yaml", b"valid")]),
        );
        store.migrate().unwrap();
        assert!(store.root().join(".layout-v2").is_file());
        assert_eq!(store.read_files("local-retry").unwrap().0.len(), 1);
        assert!(!store.package_path("keytao").exists());
        assert_eq!(fs::read(&report_path).unwrap(), report_bytes);
    }

    #[test]
    fn startup_sweeps_staging_and_tombstones_after_layout_marker() {
        let directory = tempfile::tempdir().unwrap();
        let core = core(directory.path());
        let store = PackageStore::new(&core);
        let bytes = archive(&[("example.schema.yaml", b"deleted")]);
        store
            .write_archive(Cursor::new(&bytes), &metadata("keytao", &bytes), None)
            .unwrap();
        store.migrate().unwrap();
        let tombstone = store.root().join(".staging/interrupted-delete");
        fs::create_dir_all(&tombstone).unwrap();
        fs::rename(store.package_path("keytao"), tombstone.join("package")).unwrap();
        fs::write(tombstone.join("partial-download"), b"stale").unwrap();
        store.migrate().unwrap();
        assert!(!tombstone.exists());
        assert!(!store.package_path("keytao").exists());
        assert!(store.root().join(".layout-v2").exists());
    }

    #[cfg(unix)]
    #[test]
    fn startup_staging_sweep_does_not_follow_symlinks() {
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("user-file"), b"keep").unwrap();
        let core = core(directory.path());
        let store = PackageStore::new(&core);
        fs::create_dir_all(store.root().join(".staging/stale")).unwrap();
        std::os::unix::fs::symlink(outside.path(), store.root().join(".staging/stale/link"))
            .unwrap();
        store.migrate().unwrap();
        assert_eq!(fs::read(outside.path().join("user-file")).unwrap(), b"keep");
        fs::remove_dir_all(store.root().join(".staging")).unwrap();
        std::os::unix::fs::symlink(outside.path(), store.root().join(".staging")).unwrap();
        store.migrate().unwrap();
        assert!(fs::symlink_metadata(store.root().join(".staging")).is_err());
        assert_eq!(fs::read(outside.path().join("user-file")).unwrap(), b"keep");
    }

    #[test]
    fn migration_retries_crash_between_removing_metadata_and_archive() {
        let directory = tempfile::tempdir().unwrap();
        let core = core(directory.path());
        let store = PackageStore::new(&core);
        let bytes = archive(&[("example.schema.yaml", b"schema")]);
        legacy_fixture(&store, "keytao", &bytes);
        store
            .write_archive(Cursor::new(&bytes), &metadata("keytao", &bytes), None)
            .unwrap();
        fs::remove_file(store.root().join("keytao.json")).unwrap();
        store.migrate().unwrap();
        assert!(store.root().join(".layout-v2").is_file());
        assert!(!store.root().join("keytao.zip").exists());
        assert_eq!(store.read_files("keytao").unwrap().0[0].1, b"schema");
    }

    #[test]
    fn migration_does_not_overwrite_newer_published_package() {
        for legacy_metadata_present in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let core = core(directory.path());
            let store = PackageStore::new(&core);
            let old = archive(&[("example.schema.yaml", b"old")]);
            let new = archive(&[("example.schema.yaml", b"new")]);
            legacy_fixture(&store, "keytao", &old);
            store
                .write_archive(
                    Cursor::new(&new),
                    &metadata("keytao", &new),
                    Some("https://example/new"),
                )
                .unwrap();
            if !legacy_metadata_present {
                fs::remove_file(store.root().join("keytao.json")).unwrap();
            }
            store.migrate().unwrap();
            assert_eq!(store.read_files("keytao").unwrap().0[0].1, b"new");
            assert!(store.root().join(".layout-v2").is_file());
            assert!(!store.root().join("keytao.zip").exists());
            assert!(!store.root().join("keytao.json").exists());
        }
    }

    #[test]
    fn migration_cleans_orphan_legacy_metadata_after_marker() {
        let directory = tempfile::tempdir().unwrap();
        let core = core(directory.path());
        let store = PackageStore::new(&core);
        let bytes = archive(&[("example.schema.yaml", b"abc")]);
        store
            .write_archive(Cursor::new(&bytes), &metadata("keytao", &bytes), None)
            .unwrap();
        fs::write(
            store.root().join("keytao.json"),
            serde_json::to_vec(&metadata("keytao", &bytes)).unwrap(),
        )
        .unwrap();
        store.migrate().unwrap();
        assert!(!store.root().join("keytao.json").exists());
    }

    #[test]
    fn update_is_atomic_and_rolls_back_failed_rename() {
        let directory = tempfile::tempdir().unwrap();
        let core = core(directory.path());
        let store = PackageStore::new(&core);
        let old = archive(&[("example.schema.yaml", b"old")]);
        let new = archive(&[("example.schema.yaml", b"new")]);
        store
            .write_archive(Cursor::new(&old), &metadata("keytao", &old), None)
            .unwrap();
        let result = store.write_archive_with(
            Cursor::new(&new),
            &metadata("keytao", &new),
            None,
            |source, destination| {
                assert!(source.join("package.json").is_file());
                assert!(!destination.exists());
                assert!(destination.with_file_name(".old-keytao").is_dir());
                Err(std::io::Error::other("injected publication failure"))
            },
        );
        let error = result.unwrap_err().to_string();
        assert!(error.contains("重命名"));
        assert!(error.contains(&store.package_path("keytao").display().to_string()));
        assert!(error.contains("injected publication failure"));
        assert_eq!(store.read_files("keytao").unwrap().0[0].1, b"old");
        assert!(!store.root().join("cloud/.old-keytao").exists());
        store
            .write_archive(Cursor::new(&new), &metadata("keytao", &new), None)
            .unwrap();
        assert_eq!(store.read_files("keytao").unwrap().0[0].1, b"new");
        assert!(!store.root().join("cloud/.old-keytao").exists());
    }

    #[test]
    fn readonly_archives_and_stored_files_can_be_reimported_and_removed() {
        let directory = tempfile::tempdir().unwrap();
        let core = core(directory.path());
        let store = PackageStore::new(&core);
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file(
                "nested/example.schema.yaml",
                zip::write::SimpleFileOptions::default().unix_permissions(0o444),
            )
            .unwrap();
        writer.write_all(b"old").unwrap();
        let old = writer.finish().unwrap().into_inner();
        store
            .write_archive(Cursor::new(&old), &metadata("local-example", &old), None)
            .unwrap();
        let destination = store.package_path("local-example");
        let payload = destination.join("files/nested/example.schema.yaml");
        assert!(!fs::metadata(&payload).unwrap().permissions().readonly());
        for path in [&payload, &destination.join("package.json")] {
            let mut permissions = fs::metadata(path).unwrap().permissions();
            permissions.set_readonly(true);
            fs::set_permissions(path, permissions).unwrap();
        }
        let new = archive(&[("example.schema.yaml", b"new")]);
        store
            .write_archive(Cursor::new(&new), &metadata("local-example", &new), None)
            .unwrap();
        let payload = destination.join("files/example.schema.yaml");
        assert!(!fs::metadata(&payload).unwrap().permissions().readonly());
        assert_eq!(fs::read(&payload).unwrap(), b"new");
        let mut permissions = fs::metadata(&payload).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&payload, permissions).unwrap();
        store.remove("local-example").unwrap();
        assert!(!destination.exists());
        assert!(!destination.with_file_name(".old-local-example").exists());
        assert_eq!(
            fs::read_dir(store.root().join(".staging")).unwrap().count(),
            0
        );
    }

    #[test]
    fn startup_recovers_crashed_update_with_or_without_new_directory() {
        for incomplete_new in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let core = core(directory.path());
            let store = PackageStore::new(&core);
            let bytes = archive(&[("example.schema.yaml", b"recover")]);
            store
                .write_archive(Cursor::new(&bytes), &metadata("keytao", &bytes), None)
                .unwrap();
            let path = store.package_path("keytao");
            fs::rename(&path, path.with_file_name(".old-keytao")).unwrap();
            if incomplete_new {
                fs::create_dir(&path).unwrap();
                fs::write(path.join("package.json"), b"partial").unwrap();
            }
            store.migrate().unwrap();
            assert_eq!(store.read_files("keytao").unwrap().0[0].1, b"recover");
            assert!(!path.with_file_name(".old-keytao").exists());
        }
    }

    #[test]
    fn directory_reader_checks_hash_size_and_file_count() {
        let directory = tempfile::tempdir().unwrap();
        let core = core(directory.path());
        let store = PackageStore::new(&core);
        let bytes = archive(&[("example.schema.yaml", b"abc")]);
        store
            .write_archive(Cursor::new(&bytes), &metadata("keytao", &bytes), None)
            .unwrap();
        let files = store.package_path("keytao").join("files");
        fs::write(files.join("example.schema.yaml"), b"abd").unwrap();
        assert!(store.read_files("keytao").is_err());
        fs::write(files.join("example.schema.yaml"), b"abcd").unwrap();
        assert!(store.read_files("keytao").is_err());
        fs::write(files.join("example.schema.yaml"), b"abc").unwrap();
        fs::write(files.join("extra"), b"extra").unwrap();
        assert!(store.read_files("keytao").is_err());
    }

    #[test]
    fn delete_removes_directory_and_ephemeral_archive_roundtrips() {
        let directory = tempfile::tempdir().unwrap();
        let core = core(directory.path());
        let store = PackageStore::new(&core);
        let bytes = archive(&[("example.schema.yaml", b"abc")]);
        store
            .write_archive(
                Cursor::new(&bytes),
                &metadata("local-example", &bytes),
                None,
            )
            .unwrap();
        let (temporary, _) = store.export_archive("local-example").unwrap();
        let name = temporary.path().file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("local-example-"));
        assert!(name.ends_with(".zip"));
        let mut archive = zip::ZipArchive::new(temporary.reopen().unwrap()).unwrap();
        let mut result = Vec::new();
        archive
            .by_name("example.schema.yaml")
            .unwrap()
            .read_to_end(&mut result)
            .unwrap();
        assert_eq!(result, b"abc");
        drop(archive);
        drop(temporary);
        store.remove("local-example").unwrap();
        assert!(!store.package_path("local-example").exists());
        store.remove("local-example").unwrap();
    }

    #[test]
    fn delete_unpublishes_package_and_old_version_before_removing_files() {
        let directory = tempfile::tempdir().unwrap();
        let core = core(directory.path());
        let store = PackageStore::new(&core);
        let bytes = archive(&[("example.schema.yaml", b"deleted")]);
        store
            .write_archive(Cursor::new(&bytes), &metadata("keytao", &bytes), None)
            .unwrap();
        let destination = store.package_path("keytao");
        let old = destination.with_file_name(".old-keytao");
        fs::create_dir(&old).unwrap();
        fs::write(old.join("partial-old-file"), b"old").unwrap();
        let result = store.remove_with("keytao", |tombstone| {
            assert!(!destination.exists());
            assert!(!old.exists());
            assert!(tombstone.join("old/partial-old-file").is_file());
            assert_eq!(
                PackageStore::read_files_at(&tombstone.join("package"), "keytao")
                    .unwrap()
                    .0[0]
                    .1,
                b"deleted"
            );
            Err(std::io::Error::other("injected cleanup failure"))
        });
        let error = result.unwrap_err().to_string();
        assert!(error.contains("删除目录"));
        assert!(error.contains(&store.root().join(".staging").display().to_string()));
        assert!(error.contains("injected cleanup failure"));
        store.migrate().unwrap();
        assert!(!destination.exists());
        assert!(!old.exists());
    }

    #[test]
    #[ignore = "Requires PKGB1_MIGRATION_DATA_DIR pointing to an isolated store copy"]
    fn migrate_copy_from_environment() {
        let directory = PathBuf::from(
            std::env::var_os("PKGB1_MIGRATION_DATA_DIR")
                .expect("Explicit isolated data directory required"),
        );
        assert!(
            directory.starts_with("/private/tmp/"),
            "Only an isolated /private/tmp copy is allowed"
        );
        let core = core(&directory);
        let store = PackageStore::new(&core);
        store.migrate().unwrap();
        assert!(store.root().join(".layout-v2").is_file());
        let mut packages = 0;
        let mut files = 0;
        for namespace in ["cloud", "local"] {
            let path = store.root().join(namespace);
            if path.is_dir() {
                for entry in fs::read_dir(path).unwrap() {
                    let entry = entry.unwrap();
                    let name = entry.file_name();
                    let key = name.to_str().unwrap();
                    let (entries, _) = store.read_files(key).unwrap();
                    packages += 1;
                    files += entries.len();
                }
            }
        }
        println!("MIGRATION_COPY packages={packages} files={files}");
    }
}
