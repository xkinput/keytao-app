//! Shared ownership and deployment transactions for downloaded Rime schemes.

use crate::{
    Core, CoreError, CoreEvent,
    events::InstallProgress,
    scheme_host::SchemeHost,
    wanxiang::{self, InstallReceipt, InstalledFile, RemotePackage, Status},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::Read,
    path::Path,
};

pub(crate) struct Addon {
    pub id: &'static str,
    pub dictionary: &'static str,
    pub name: &'static str,
    pub manifest: &'static str,
    pub required: &'static [&'static str],
    pub extra_archives: &'static [(&'static str, &'static str)],
    pub installable: fn(&str) -> bool,
    pub extract: fn(&Path, &Path, &RemotePackage) -> Result<Vec<InstalledFile>, String>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Manifest {
    format: u32,
    pub version: String,
    source: String,
    archive_sha256: String,
    files: Vec<InstalledFile>,
}

impl Addon {
    pub(crate) fn read_manifest(&self, root: &Path) -> Result<Option<Manifest>, String> {
        let path = wanxiang::safe_target(root, self.manifest)?;
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        let mut bytes = Vec::new();
        file.take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        self.parse_manifest(root, &bytes).map(Some)
    }

    fn parse_manifest(&self, root: &Path, bytes: &[u8]) -> Result<Manifest, String> {
        if bytes.len() > 1024 * 1024 {
            return Err(format!("{}安装记录过大", self.name));
        }
        let manifest: Manifest = serde_json::from_slice(&bytes)
            .map_err(|e| format!("{}安装记录损坏：{e}", self.name))?;
        let mut seen = BTreeSet::new();
        if manifest.format != 1
            || manifest.files.len() > 4096
            || manifest.archive_sha256.len() != 64
            || !manifest
                .archive_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        {
            return Err(format!("{}安装记录无效", self.name));
        }
        for file in &manifest.files {
            wanxiang::safe_target(root, &file.path)?;
            if !(self.installable)(&file.path)
                || !seen.insert(file.path.to_ascii_lowercase())
                || file.sha256.len() != 64
                || !file.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(format!("{}安装记录含无效文件", self.name));
            }
        }
        Ok(manifest)
    }

    pub(crate) fn status_at(&self, root: &Path) -> Status {
        let manifest = self.read_manifest(root).ok().flatten();
        self.status_with_manifest(root, manifest)
    }

    pub(crate) fn status(&self, core: &Core, root: &Path) -> Result<Status, CoreError> {
        let isolated = !keytao_core::RuntimeDirs::needs_migration(root);
        let source = if isolated {
            crate::addon_store::isolated_files(core, self.id).map_err(CoreError::Other)?
        } else {
            root.to_path_buf()
        };
        let manifest = crate::addon_store::manifest(core, root, self.id)
            .and_then(|bytes| {
                bytes
                    .map(|bytes| self.parse_manifest(&source, &bytes))
                    .transpose()
            })
            .map_err(CoreError::Other)?;
        if isolated {
            let installed = self.files_installed(&source, manifest.as_ref());
            return Ok(Status {
                installed,
                deployed: installed && crate::addon_store::isolated_ready(core, root, self.id),
                version: manifest.map_or_else(String::new, |manifest| manifest.version),
                variant: String::new(),
            });
        }
        Ok(self.status_with_manifest(root, manifest))
    }

    fn files_installed(&self, root: &Path, manifest: Option<&Manifest>) -> bool {
        manifest.is_some_and(|manifest| {
            manifest
                .files
                .iter()
                .map(|file| file.path.as_str())
                .chain(self.required.iter().copied())
                .all(|name| wanxiang::safe_target(root, name).is_ok_and(|path| path.is_file()))
        })
    }

    fn status_with_manifest(&self, root: &Path, manifest: Option<Manifest>) -> Status {
        let dirs = keytao_core::RuntimeDirs::resolve(root, root);
        let configured = crate::addon::addon_default_custom(root).ok().map(|(_, content)| keytao_core::parse_schema_list(&content)).unwrap_or_default();
        let selected: Vec<_> = manifest.as_ref().into_iter().flat_map(|manifest| &manifest.files)
            .filter_map(|file| file.path.strip_suffix(".schema.yaml"))
            .filter(|id| configured.iter().any(|selected| selected == id)).collect();
        let installed = self.files_installed(root, manifest.as_ref());
        Status {
            installed,
            deployed: installed
                && !selected.is_empty()
                && dirs.is_ok_and(|dirs| {
                    selected.iter().all(|id| {
                        dirs.staging_dir.join(format!("{id}.schema.yaml")).is_file()
                    }) && (!selected.contains(&self.id)
                        || dirs.staging_dir.join(format!("{}.table.bin", self.dictionary)).is_file())
                }),
            version: manifest.map_or_else(String::new, |manifest| manifest.version),
            variant: String::new(),
        }
    }

    pub(crate) fn commit_files(
        &self,
        receipt: &mut InstallReceipt,
        files: Vec<InstalledFile>,
        package: &RemotePackage,
        archive_sha256: String,
    ) -> Result<(), String> {
        if receipt.is_isolated() {
            return self.record_manifest(receipt, files, package, archive_sha256);
        }
        #[cfg(windows)]
        {
            receipt.public_sources = files.iter().map(|file| file.path.clone()).collect();
        }
        let previous: BTreeMap<_, _> = receipt
            .manifest_bytes(self.manifest)?
            .map(|bytes| self.parse_manifest(&receipt.root, &bytes))
            .transpose()?
            .into_iter()
            .flat_map(|manifest| manifest.files)
            .map(|file| (file.path.clone(), file))
            .collect();
        let mut owned = Vec::new();
        let mut replacements = Vec::new();
        for file in files {
            receipt.preflight_owner(&file.path)?;
            let destination = wanxiang::safe_target(&receipt.root, &file.path)?;
            if destination.exists() {
                let hash = wanxiang::fingerprint(&destination)?.0;
                if hash == file.sha256 {
                    if previous.contains_key(&file.path) {
                        owned.push(file);
                    }
                    continue;
                }
                if previous
                    .get(&file.path)
                    .is_none_or(|old| old.sha256 != hash)
                {
                    return Err(format!(
                        "已有文件 {} 与{}冲突，已保留原文件；请先备份并移走该文件",
                        file.path, self.name
                    ));
                }
            }
            owned.push(file.clone());
            replacements.push(file);
        }
        for (name, old) in previous {
            if !owned.iter().any(|file| file.path == name) {
                let path = wanxiang::safe_target(&receipt.root, &name)?;
                if path.is_file() && wanxiang::fingerprint(&path)?.0 == old.sha256 {
                    receipt.move_original(&name)?;
                }
            }
        }
        for file in replacements {
            let source = receipt.work.join("files").join(&file.path);
            #[cfg(windows)]
            let source = {
                let copy = receipt.work.join("commit").join(&file.path);
                fs::create_dir_all(copy.parent().unwrap()).map_err(|e| e.to_string())?;
                fs::copy(&source, &copy).map_err(|e| e.to_string())?;
                copy
            };
            receipt.replace(&file.path, &source, file.sha256)?;
        }
        self.record_manifest(receipt, owned, package, archive_sha256)
    }

    fn record_manifest(
        &self,
        receipt: &mut InstallReceipt,
        files: Vec<InstalledFile>,
        package: &RemotePackage,
        archive_sha256: String,
    ) -> Result<(), String> {
        let manifest = Manifest {
            format: 1,
            version: package.version.clone(),
            source: package.source.clone(),
            archive_sha256,
            files,
        };
        let source = receipt.work.join("manifest.json");
        fs::write(
            &source,
            serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        receipt.replace_manifest(self.manifest, &source)
    }

    pub(crate) async fn install(
        &'static self,
        root: &Path,
        package: impl std::future::Future<Output = Result<RemotePackage, String>>,
        progress: impl Fn(u8, &str) + Send + Sync + 'static,
    ) -> Result<InstallReceipt, String> {
        self.install_receipt(InstallReceipt::new(root)?, package, progress)
            .await
    }

    pub(crate) async fn install_receipt(
        &'static self,
        receipt: InstallReceipt,
        package: impl std::future::Future<Output = Result<RemotePackage, String>>,
        progress: impl Fn(u8, &str) + Send + Sync + 'static,
    ) -> Result<InstallReceipt, String> {
        if let Some(bytes) = receipt.manifest_bytes(self.manifest)? {
            self.parse_manifest(&receipt.source_dir()?, &bytes)?;
        }
        let package = package.await?;
        progress(5, &format!("正在下载{}…", self.name));
        let archive = receipt.work.join("download.zip");
        wanxiang::download_with_limit(&package, &archive, &progress, 64 * 1024 * 1024).await?;
        for (name, url) in self.extra_archives {
            progress(45, &format!("正在下载{}依赖词库…", self.name));
            let dependency = RemotePackage {
                version: String::new(),
                source: (*url).into(),
                download_url: (*url).into(),
                download_bytes: None,
            };
            wanxiang::download_with_limit(
                &dependency,
                &receipt.work.join(name),
                &progress,
                64 * 1024 * 1024,
            )
            .await?;
        }
        progress(52, &format!("正在校验并解压{}…", self.name));
        tokio::task::spawn_blocking(move || {
            let mut receipt = receipt;
            let files = (self.extract)(&archive, &receipt.work.join("files"), &package)?;
            progress(60, &format!("正在安装{}…", self.name));
            self.commit_files(
                &mut receipt,
                files,
                &package,
                wanxiang::fingerprint(&archive)?.0,
            )?;
            Ok(receipt)
        })
        .await
        .map_err(|e| format!("{}安装任务失败：{e}", self.name))?
    }

    pub(crate) fn uninstall(&self, root: &Path) -> Result<InstallReceipt, String> {
        let mut receipt = InstallReceipt::new(root)?;
        self.uninstall_receipt(&mut receipt)?;
        Ok(receipt)
    }

    fn uninstall_receipt(&self, receipt: &mut InstallReceipt) -> Result<(), String> {
        if let Some(bytes) = receipt.manifest_bytes(self.manifest)? {
            let manifest = self.parse_manifest(&receipt.root, &bytes)?;
            for file in manifest.files {
                let path = wanxiang::safe_target(&receipt.root, &file.path)?;
                if path.is_file() && wanxiang::fingerprint(&path)?.0 == file.sha256 {
                    receipt.move_original(&file.path)?;
                }
            }
            receipt.move_original(self.manifest)?;
        }
        Ok(())
    }

    pub(crate) async fn manage<Install, InstallFuture>(
        &self,
        core: &Core,
        installed: bool,
        host: &impl SchemeHost,
        install_package: Install,
    ) -> Result<Status, CoreError>
    where
        Install: FnOnce(InstallReceipt) -> InstallFuture,
        InstallFuture: std::future::Future<Output = Result<InstallReceipt, String>>,
    {
        let result: Result<_, String> = async {
            let root = host.user_root()?;
            crate::package_migration::ensure_before_write(core, &root).await.map_err(|error| error.to_string())?;
            let mut receipt = InstallReceipt::new(&root)?;
            receipt.attach_store(core, self.id)?;
            if installed {
                let receipt = install_package(receipt).await?;
                receipt.deploy_isolated(core, self.id, host).await?;
            } else {
                receipt.uninstall_isolated(core, self.id, host).await?;
                receipt.finish()?;
            }
            self.emit_completed(core, installed);
            self.status(core, &root).map_err(|error| error.to_string())
        }
        .await;
        result.map_err(CoreError::Other)
    }

    fn emit_completed(&self, core: &Core, installed: bool) {
        core.emit(CoreEvent::InstallProgress(InstallProgress {
            stage: "done".into(),
            percent: 100,
            message: if installed {
                format!("{}已安装并部署", self.name)
            } else {
                format!("{}已卸载，保留用户自定义和学习记录", self.name)
            },
        }));
    }
}
