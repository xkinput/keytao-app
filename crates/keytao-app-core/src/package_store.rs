use crate::{
    Core, CoreError,
    packages::{InstalledPackageInfo, PackageMeta, safe_path, sha256_reader},
};
use std::{
    fs,
    io::{BufReader, Seek},
    path::PathBuf,
};

pub(crate) enum Staging {
    Cloud,
    Metadata,
    Pending,
    Archive,
    ActiveManifest,
    Local,
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

    pub(crate) fn archive_path(&self, key: &str) -> PathBuf {
        self.root().join(format!("{key}.zip"))
    }

    pub(crate) fn metadata_path(&self, key: &str) -> PathBuf {
        self.root().join(format!("{key}.json"))
    }

    pub(crate) fn read_metadata_bytes(&self, key: &str) -> std::io::Result<Vec<u8>> {
        fs::read(self.metadata_path(key))
    }

    pub(crate) fn open_archive_file(&self, key: &str) -> std::io::Result<fs::File> {
        fs::File::open(self.archive_path(key))
    }

    pub(crate) fn checked_archive_path(&self, key: &str) -> Result<PathBuf, CoreError> {
        safe_path(&self.root(), &format!("{key}.zip")).map_err(CoreError::Other)
    }

    pub(crate) fn checked_metadata_path(&self, key: &str) -> Result<PathBuf, CoreError> {
        safe_path(&self.root(), &format!("{key}.json")).map_err(CoreError::Other)
    }

    pub(crate) fn active_manifest_path(&self) -> PathBuf {
        keytao_core::package_store::active_manifest(&self.core.env().data_dir)
    }

    pub(crate) fn checked_active_manifest_path(&self) -> Result<PathBuf, CoreError> {
        self.checked_root()?;
        Ok(self.active_manifest_path())
    }

    pub(crate) fn staging_file(
        &self,
        staging: Staging,
    ) -> std::io::Result<tempfile::NamedTempFile> {
        let prefix = match staging {
            Staging::Cloud => ".cloud-",
            Staging::Metadata => ".meta-",
            Staging::Pending => ".pending-",
            Staging::Archive => ".archive-",
            Staging::ActiveManifest => ".active-",
            Staging::Local => ".local-",
        };
        tempfile::Builder::new()
            .prefix(prefix)
            .tempfile_in(self.root())
    }

    pub(crate) fn read_meta(&self, scheme_key: &str) -> Result<Option<PackageMeta>, CoreError> {
        let scheme_key =
            crate::scheme::validate_scheme_key(scheme_key).map_err(CoreError::Other)?;
        let _store_guard = self
            .core
            .pending_package
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.checked_root()?;
        let metadata = self.checked_metadata_path(scheme_key)?;
        let bytes = match fs::read(metadata) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let meta: PackageMeta = serde_json::from_slice(&bytes)?;
        if meta.scheme_key != scheme_key
            || meta.zip_sha256.len() != 64
            || !meta.zip_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(CoreError::Other("本地方案包信息无效".into()));
        }
        Ok(Some(meta))
    }

    pub(crate) fn open_archive(
        &self,
        scheme_key: &str,
    ) -> Result<(BufReader<fs::File>, PackageMeta), CoreError> {
        let scheme_key =
            crate::scheme::validate_scheme_key(scheme_key).map_err(CoreError::Other)?;
        let _store_guard = self
            .core
            .pending_package
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.checked_root()?;
        let archive = self.checked_archive_path(scheme_key)?;
        let metadata = self.checked_metadata_path(scheme_key)?;
        // The upload size limit applies to the rebuilt dependency zip, not this distribution.
        let mut archive = BufReader::new(fs::File::open(archive)?);
        let meta: PackageMeta = serde_json::from_slice(&fs::read(metadata)?)?;
        if meta.scheme_key != scheme_key || meta.zip_sha256 != sha256_reader(&mut archive)? {
            return Err(CoreError::Other("本地方案包校验失败".into()));
        }
        archive.rewind()?;
        Ok((archive, meta))
    }

    pub(crate) fn list_installed_packages(&self) -> Result<Vec<InstalledPackageInfo>, CoreError> {
        self.checked_root()?;
        let core = self.core;
        let state = core.state();
        Ok(state
            .installed_packages
            .into_iter()
            .filter_map(|(scheme_key, package)| {
                let has_package_zip = crate::scheme::validate_scheme_key(&scheme_key).is_ok()
                    && fs::symlink_metadata(self.archive_path(&scheme_key))
                        .is_ok_and(|meta| meta.is_file());
                let meta = if crate::scheme::validate_scheme_key(&scheme_key).is_ok() {
                    match self.read_meta(&scheme_key) {
                        Ok(meta) => meta,
                        Err(error) => {
                            tracing::warn!(%scheme_key, %error, "Skipping unreadable package metadata");
                            return None;
                        }
                    }
                } else {
                    None
                };
                Some(InstalledPackageInfo {
                    logo_path: if has_package_zip {
                        crate::packages::package_logos::package_logo_path(core, &scheme_key)
                            .map(|path| path.to_string_lossy().into_owned())
                    } else {
                        None
                    },
                    active: state.active_scheme.as_deref() == Some(&scheme_key),
                    supports_user_words: crate::user_words::supports_user_words(core, &scheme_key)
                        .unwrap_or_else(|error| {
                            tracing::warn!(%scheme_key, %error, "Cannot read user dictionary support");
                            false
                        }),
                    scheme_key,
                    version: package.version,
                    has_package_zip,
                    source: meta
                        .as_ref()
                        .map(|meta| meta.source.clone())
                        .unwrap_or_else(|| "official".into()),
                    display_name: meta.and_then(|meta| meta.display_name),
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppEnv, CoreEvent, EventSink, Platform};
    use std::{collections::BTreeMap, io::Read, sync::Arc};

    struct NullSink;

    impl EventSink for NullSink {
        fn emit(&self, _: CoreEvent) {}
    }

    #[test]
    fn legacy_package_store_layout_golden() {
        let dir = tempfile::tempdir().unwrap();
        let core = Core::new(
            AppEnv {
                data_dir: dir.path().into(),
                cache_dir: dir.path().join("cache"),
                resource_dir: dir.path().join("resources"),
                app_version: "test".into(),
                platform: Platform::Android,
            },
            Arc::new(NullSink),
        )
        .unwrap();
        let store = PackageStore::new(&core);
        let expected_root = dir.path().join("scheme-packages");
        assert_eq!(store.root(), expected_root);
        assert_eq!(store.checked_root().unwrap(), expected_root);
        for key in ["keytao", "local-example"] {
            assert_eq!(
                store.archive_path(key),
                expected_root.join(format!("{key}.zip"))
            );
            assert_eq!(
                store.metadata_path(key),
                expected_root.join(format!("{key}.json"))
            );
            assert_eq!(
                store.checked_archive_path(key).unwrap(),
                store.archive_path(key)
            );
            assert_eq!(
                store.checked_metadata_path(key).unwrap(),
                store.metadata_path(key)
            );
        }
        assert_eq!(
            store.active_manifest_path(),
            expected_root.join("active-manifest.json")
        );
        assert_eq!(
            store.checked_active_manifest_path().unwrap(),
            store.active_manifest_path()
        );
        fs::create_dir_all(store.root()).unwrap();
        for (staging, prefix) in [
            (Staging::Cloud, ".cloud-"),
            (Staging::Metadata, ".meta-"),
            (Staging::Pending, ".pending-"),
            (Staging::Archive, ".archive-"),
            (Staging::ActiveManifest, ".active-"),
            (Staging::Local, ".local-"),
        ] {
            let file = store.staging_file(staging).unwrap();
            assert_eq!(file.path().parent().unwrap(), expected_root);
            assert!(
                file.path()
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with(prefix)
            );
        }
        let meta = PackageMeta {
            scheme_key: "local-example".into(),
            version: Some("v1".into()),
            installed_at_unix: 7,
            source: "local".into(),
            display_name: Some("Example".into()),
            selection_base_version: Some("v0".into()),
            manifest: BTreeMap::from([("example.schema.yaml".into(), "hash".into())]),
            zip_sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
        };
        let expected_metadata = r#"{
  "scheme_key": "local-example",
  "version": "v1",
  "installed_at_unix": 7,
  "source": "local",
  "display_name": "Example",
  "selection_base_version": "v0",
  "manifest": {
    "example.schema.yaml": "hash"
  },
  "zip_sha256": "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
}"#;
        let mut staged = store.staging_file(Staging::Metadata).unwrap();
        serde_json::to_writer_pretty(staged.as_file_mut(), &meta).unwrap();
        staged.as_file().sync_all().unwrap();
        staged
            .persist(store.checked_metadata_path("local-example").unwrap())
            .unwrap();
        assert_eq!(
            store.read_metadata_bytes("local-example").unwrap(),
            expected_metadata.as_bytes()
        );
        let read = store.read_meta("local-example").unwrap().unwrap();
        assert_eq!(
            serde_json::to_string_pretty(&read).unwrap(),
            expected_metadata
        );
        assert!(store.read_meta("keytao").unwrap().is_none());
        fs::write(store.archive_path("local-example"), b"abc").unwrap();
        let (mut archive, read) = store.open_archive("local-example").unwrap();
        let mut bytes = Vec::new();
        archive.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"abc");
        assert_eq!(
            serde_json::to_string_pretty(&read).unwrap(),
            expected_metadata
        );
    }
}
