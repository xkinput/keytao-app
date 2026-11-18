//! Shared ownership and deployment transactions for downloaded Rime schemes.

use crate::{
    Core, CoreError, CoreEvent,
    addon::write_schema_list_transaction,
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
    sync::Arc,
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
        let manifest = crate::addon_store::manifest(core, root, self.id)
            .and_then(|bytes| {
                bytes
                    .map(|bytes| self.parse_manifest(root, &bytes))
                    .transpose()
            })
            .map_err(CoreError::Other)?;
        Ok(self.status_with_manifest(root, manifest))
    }

    fn status_with_manifest(&self, root: &Path, manifest: Option<Manifest>) -> Status {
        let installed = manifest.as_ref().is_some_and(|manifest| {
            manifest
                .files
                .iter()
                .map(|file| file.path.as_str())
                .chain(self.required.iter().copied())
                .all(|name| wanxiang::safe_target(root, name).is_ok_and(|path| path.is_file()))
        });
        Status {
            installed,
            deployed: installed
                && root
                    .join(format!("build/{}.schema.yaml", self.id))
                    .is_file()
                && root
                    .join(format!("build/{}.table.bin", self.dictionary))
                    .is_file(),
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
        let manifest = Manifest {
            format: 1,
            version: package.version.clone(),
            source: package.source.clone(),
            archive_sha256,
            files: owned,
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
            self.parse_manifest(&receipt.root, &bytes)?;
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
        core: &Arc<Core>,
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
            let mut receipt = InstallReceipt::new(&root)?;
            receipt.attach_store(core, self.id)?;
            let previous = receipt
                .manifest_bytes(self.manifest)?
                .map(|bytes| self.parse_manifest(&root, &bytes))
                .transpose()?;
            let removed_sources = crate::addon::removed_sources(
                &root,
                previous
                    .as_ref()
                    .into_iter()
                    .flat_map(|manifest| &manifest.files),
            )?;
            #[cfg(target_os = "windows")]
            let package_paths = previous
                .as_ref()
                .into_iter()
                .flat_map(|manifest| &manifest.files)
                .map(|file| std::path::PathBuf::from(&file.path))
                .collect::<Vec<_>>();
            let mut receipt = if installed {
                install_package(receipt).await?
            } else {
                self.uninstall_receipt(&mut receipt)?;
                receipt
            };
            #[cfg(target_os = "windows")]
            let mut public_update = PublicRollback::default();
            let deployed = async {
                write_schema_list_transaction(
                    &mut receipt,
                    self.id,
                    installed,
                    &crate::addon::shared_data_dir(core, &root),
                )?;
                #[cfg(target_os = "windows")]
                let public_source_warning = {
                    match crate::windows_public_schemas::update_public_schemas(|| {
                        if installed {
                            host.publish_wanxiang_sources(&receipt.public_source_files())
                        } else {
                            host.remove_public_schema(self.id, &package_paths)
                        }
                    }) {
                        Ok(update) => {
                            public_update.0 = Some(update);
                            None
                        }
                        Err(error) => Some(crate::windows_public_schemas::public_warning(
                            &format!("{}-public-sources", self.id.replace('_', "-")),
                            &error,
                        )),
                    }
                };
                if installed || keytao_core::schema_install_state(&root).installed {
                    host.deploy().await?;
                } else {
                    let _ = host.write_reload_stamp(&root)?;
                }
                #[cfg(target_os = "windows")]
                if let Some(warning) = public_source_warning {
                    core.emit(CoreEvent::DeployProgress(format!(
                        "系统搜索框方案未更新：{warning}"
                    )));
                }
                let pending = receipt
                    .manifest_bytes(self.manifest)?
                    .map(|bytes| self.parse_manifest(&root, &bytes))
                    .transpose()?;
                if installed && !self.status_with_manifest(&root, pending).deployed {
                    return Err(format!("{}部署产物缺失", self.name));
                }
                if !installed {
                    crate::addon::cleanup_removed(&root, &removed_sources)?;
                }
                receipt.commit_store(installed)?;
                Ok(())
            }
            .await;
            if let Err(error) = deployed {
                let rollback = receipt.rollback();
                #[cfg(target_os = "windows")]
                let public_rollback = public_update
                    .0
                    .take()
                    .map_or(Ok(()), |update| update.rollback());
                rollback.map_err(|restore| format!("{error}；恢复词库失败：{restore}"))?;
                #[cfg(target_os = "windows")]
                public_rollback
                    .map_err(|restore| format!("{error}；恢复公共方案失败：{restore}"))?;
                host.deploy()
                    .await
                    .map_err(|restore| format!("{error}；已恢复文件，请重新部署：{restore}"))?;
                return Err(error);
            }
            #[cfg(target_os = "windows")]
            public_update.0.take();
            receipt.finish()?;
            crate::packages::sync_generated(core, &root).map_err(|error| error.to_string())?;
            core.emit(CoreEvent::InstallProgress(InstallProgress {
                stage: "done".into(),
                percent: 100,
                message: if installed {
                    format!("{}已安装并部署", self.name)
                } else {
                    format!("{}已卸载，保留用户自定义和学习记录", self.name)
                },
            }));
            self.status(core, &root).map_err(|error| error.to_string())
        }
        .await;
        result.map_err(CoreError::Other)
    }
}

#[cfg(target_os = "windows")]
#[derive(Default)]
struct PublicRollback(Option<crate::windows_public_schemas::PublicSchemaUpdate>);

#[cfg(target_os = "windows")]
impl Drop for PublicRollback {
    fn drop(&mut self) {
        if let Some(update) = self.0.take() {
            if let Err(error) = update.rollback() {
                tracing::error!(%error, "add-on public source rollback failed");
            }
        }
    }
}
