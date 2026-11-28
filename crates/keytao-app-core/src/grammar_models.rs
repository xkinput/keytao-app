//! Shared Rime `.gram` language model catalog and per-package selection.

use std::{
    collections::BTreeMap,
    fs,
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use keytao_core::{RuntimeDirs, runtime_log::AppOperation};
use serde::Serialize;

use crate::{
    Core, CoreError, CoreEvent,
    events::InstallProgress,
    scheme_host::SchemeHost,
    wanxiang::{self, GrammarModel, InstallReceipt, RemotePackage},
};

pub const WANXIANG_HANS_ID: &str = "wanxiang-lts-zh-hans";
pub const WANXIANG_HANT_ID: &str = "wanxiang-lts-zh-hant";
pub const OCTAGRAM_HANS_ID: &str = "zh-hans-t-essay-bgw";
pub const OCTAGRAM_HANT_ID: &str = "zh-hant-t-essay-bgw";

const WANXIANG_RELEASE_API: &str =
    "https://api.github.com/repos/amzxyz/rime-wanxiang-data/releases/tags/LTS";

#[derive(Clone, Copy, Debug)]
pub struct GrammarModelSpec {
    pub id: &'static str,
    pub file_name: &'static str,
    pub manifest_file: &'static str,
    pub name: &'static str,
    pub short_name: &'static str,
    pub description: &'static str,
    pub repo_url: &'static str,
    pub release_api: Option<&'static str>,
    pub download_url: &'static str,
    pub default_size: u64,
    pub default_version: &'static str,
    pub wanxiang_tuned: bool,
}

pub const CATALOG: &[GrammarModelSpec] = &[
    GrammarModelSpec {
        id: WANXIANG_HANS_ID,
        file_name: "wanxiang-lts-zh-hans.gram",
        manifest_file: ".keytao-wanxiang-grammar.json",
        name: "万象语法模型（简体）",
        short_name: "万象·简体",
        description: "简体大语料模型，整句与长句准确率高，适合万象、雾凇、白霜及各类双拼主力使用。",
        repo_url: "https://github.com/amzxyz/rime-wanxiang-data",
        release_api: Some(WANXIANG_RELEASE_API),
        download_url: "https://github.com/amzxyz/rime-wanxiang-data/releases/download/LTS/wanxiang-lts-zh-hans.gram",
        default_size: 390_786_092,
        default_version: "LTS",
        wanxiang_tuned: true,
    },
    GrammarModelSpec {
        id: WANXIANG_HANT_ID,
        file_name: "wanxiang-lts-zh-hant.gram",
        manifest_file: ".keytao-grammar-wanxiang-lts-zh-hant.json",
        name: "万象语法模型（繁体）",
        short_name: "万象·繁体",
        description: "万象同源繁体大语料模型，适合繁体拼音、注音、仓颉等繁体整句输入场景。",
        repo_url: "https://github.com/amzxyz/rime-wanxiang-data",
        release_api: Some(WANXIANG_RELEASE_API),
        download_url: "https://github.com/amzxyz/rime-wanxiang-data/releases/download/LTS/wanxiang-lts-zh-hant.gram",
        default_size: 409_859_116,
        default_version: "LTS",
        wanxiang_tuned: true,
    },
    GrammarModelSpec {
        id: OCTAGRAM_HANS_ID,
        file_name: "zh-hans-t-essay-bgw.gram",
        manifest_file: ".keytao-grammar-zh-hans-t-essay-bgw.json",
        name: "八股文·简体词级（Rime 官方）",
        short_name: "八股文·简体",
        description: "Rime 官方轻量简体词级模型，体积小、内存与流量占用低，适合移动端或轻量整句纠错。",
        repo_url: "https://github.com/lotem/rime-octagram-data",
        release_api: None,
        download_url: "https://raw.githubusercontent.com/lotem/rime-octagram-data/hans/zh-hans-t-essay-bgw.gram",
        default_size: 40_925_228,
        default_version: "hans",
        wanxiang_tuned: false,
    },
    GrammarModelSpec {
        id: OCTAGRAM_HANT_ID,
        file_name: "zh-hant-t-essay-bgw.gram",
        manifest_file: ".keytao-grammar-zh-hant-t-essay-bgw.json",
        name: "八股文·繁体词级（Rime 官方）",
        short_name: "八股文·繁体",
        description: "Rime 官方轻量繁体词级模型，搭配朙月、地球拼音等繁体方案开箱即用。",
        repo_url: "https://github.com/lotem/rime-octagram-data",
        release_api: None,
        download_url: "https://raw.githubusercontent.com/lotem/rime-octagram-data/hant/zh-hant-t-essay-bgw.gram",
        default_size: 41_415_724,
        default_version: "hant",
        wanxiang_tuned: false,
    },
];

pub fn find_spec(id: &str) -> Option<&'static GrammarModelSpec> {
    CATALOG.iter().find(|spec| spec.id == id)
}

pub(crate) fn managed_patch_for(id: &str) -> Option<String> {
    let spec = find_spec(id)?;
    if spec.wanxiang_tuned {
        Some(format!(
            "{{grammar/language: {}, grammar/collocation_max_length: 6, grammar/collocation_min_length: 2, grammar/collocation_penalty: -14, grammar/non_collocation_penalty: -6, grammar/weak_collocation_penalty: -100, grammar/rear_penalty: -18, translator/contextual_suggestions: true, translator/max_homophones: 8, translator/max_homographs: 7}}",
            spec.id
        ))
    } else {
        Some(format!(
            "{{grammar/language: {}, grammar/collocation_max_length: 5, grammar/collocation_min_length: 2, translator/contextual_suggestions: true, translator/max_homophones: 7, translator/max_homographs: 7}}",
            spec.id
        ))
    }
}

#[derive(Debug, Serialize)]
pub struct GrammarModelItem {
    pub id: String,
    pub name: String,
    pub short_name: String,
    pub description: String,
    pub repo_url: String,
    pub installed: bool,
    pub version: String,
    pub size: u64,
}

#[derive(Debug, Serialize)]
pub struct GrammarModelsOverview {
    pub available: bool,
    pub models: Vec<GrammarModelItem>,
    pub package_models: BTreeMap<String, String>,
}

pub(crate) fn model_path(root: &Path, id: &str) -> Option<PathBuf> {
    let spec = find_spec(id)?;
    if spec.id == WANXIANG_HANS_ID {
        return wanxiang::shared_grammar_model_path(root);
    }
    let path = wanxiang::safe_target(root, spec.file_name).ok()?;
    fs::metadata(&path)
        .is_ok_and(|meta| meta.is_file() && meta.len() > 0)
        .then_some(path)
}

fn read_model_manifest(root: &Path, spec: &GrammarModelSpec) -> Result<Option<GrammarModel>, String> {
    let path = wanxiang::safe_target(root, spec.manifest_file)?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("读取语法模型记录失败：{error}")),
    };
    let model: GrammarModel =
        serde_json::from_slice(&bytes).map_err(|error| format!("语法模型记录损坏：{error}"))?;
    if model.size == 0 || model.size > wanxiang::MAX_GRAMMAR_BYTES || !wanxiang::valid_hash(&model.sha256) {
        return Err("语法模型安装记录无效".into());
    }
    Ok(Some(model))
}

fn write_model_manifest(
    root: &Path,
    spec: &GrammarModelSpec,
    model: Option<&GrammarModel>,
) -> Result<(), String> {
    let path = wanxiang::safe_target(root, spec.manifest_file)?;
    if let Some(model) = model {
        let bytes = serde_json::to_vec_pretty(model).map_err(|error| error.to_string())?;
        crate::scheme_files::write_file_atomic(&path, &bytes)
            .map_err(|error| format!("保存语法模型记录失败：{error}"))?;
    } else {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("删除语法模型记录失败：{error}")),
        }
    }
    Ok(())
}

fn remove_package_linked_model(
    root: &Path,
    package_key: &str,
    spec: &GrammarModelSpec,
    shared_path: Option<&Path>,
    shared_model: Option<&GrammarModel>,
) {
    let Some(runtime) = RuntimeDirs::isolated_user_data_dir(root, package_key) else {
        return;
    };
    let target = runtime.join(spec.file_name);
    if !fs::symlink_metadata(&target).is_ok_and(|meta| meta.is_file()) {
        return;
    }
    let matches_shared = shared_path.is_some_and(|shared| wanxiang::same_file_identity(shared, &target))
        || shared_model.is_some_and(|model| {
            wanxiang::fingerprint(&target)
                .ok()
                .is_some_and(|(sha, size)| sha == model.sha256 && size == model.size)
        });
    if matches_shared {
        let _ = fs::remove_file(&target);
    }
}

pub fn status(core: &Core, root: &Path) -> Result<GrammarModelsOverview, CoreError> {
    let result = || -> Result<_, String> {
        let available = wanxiang::grammar_supported();
        let package_models = crate::runtime_materialize::all_package_grammar_models(root)
            .map_err(|error| error.to_string())?;
        let mut models = Vec::with_capacity(CATALOG.len());
        for spec in CATALOG {
            if spec.id == WANXIANG_HANS_ID {
                let wx = wanxiang::grammar_status_for(core, root)
                    .map_err(|error| error.to_string())?;
                models.push(GrammarModelItem {
                    id: spec.id.into(),
                    name: spec.name.into(),
                    short_name: spec.short_name.into(),
                    description: spec.description.into(),
                    repo_url: spec.repo_url.into(),
                    installed: wx.installed,
                    version: wx.version,
                    size: if wx.installed { wx.size } else { spec.default_size },
                });
                continue;
            }
            let path = wanxiang::safe_target(root, spec.file_name)?;
            let size = match fs::metadata(&path) {
                Ok(meta) if meta.is_file() => meta.len(),
                Ok(_) => return Err(format!("语法模型路径不是文件：{}", spec.file_name)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
                Err(error) => return Err(format!("读取语法模型失败：{error}")),
            };
            let manifest = read_model_manifest(root, spec)?;
            let installed = size > 0;
            models.push(GrammarModelItem {
                id: spec.id.into(),
                name: spec.name.into(),
                short_name: spec.short_name.into(),
                description: spec.description.into(),
                repo_url: spec.repo_url.into(),
                installed,
                version: manifest.map_or_else(String::new, |m| m.version),
                size: if installed { size } else { spec.default_size },
            });
        }
        Ok(GrammarModelsOverview {
            available,
            models,
            package_models,
        })
    };
    result().map_err(CoreError::Other)
}

fn release_asset_package(json: &str, spec: &GrammarModelSpec) -> Result<RemotePackage, String> {
    let release: serde_json::Value =
        serde_json::from_str(json).map_err(|error| error.to_string())?;
    let asset = release["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|asset| asset["name"].as_str() == Some(spec.file_name))
        .ok_or_else(|| format!("语法模型发布包不存在：{}", spec.file_name))?;
    let size = asset["size"]
        .as_u64()
        .filter(|size| *size > 0 && *size <= wanxiang::MAX_GRAMMAR_BYTES)
        .ok_or("语法模型发布大小无效")?;
    Ok(RemotePackage {
        version: format!(
            "{}@{}",
            spec.default_version,
            asset["updated_at"].as_str().unwrap_or("unknown")
        ),
        source: spec.release_api.unwrap_or(spec.download_url).into(),
        download_url: spec.download_url.into(),
        download_bytes: Some(size),
    })
}

async fn download_spec_model(
    core: &Core,
    root: &Path,
    spec: &GrammarModelSpec,
    cancel: &mut tokio::sync::watch::Receiver<u64>,
) -> Result<(tempfile::TempPath, RemotePackage), String> {
    let _operation = AppOperation::start("download_grammar_model");
    wanxiang::verify_disk_space(wanxiang::available_disk_space(root)?, spec.default_size)?;
    let mut package = if let Some(api) = spec.release_api {
        let lookup = tokio::select! {
            biased;
            _ = cancel.changed() => return Err(crate::download::DOWNLOAD_CANCELLED.into()),
            result = wanxiang::github_text(api) => result.and_then(|json| release_asset_package(&json, spec)),
        };
        lookup.unwrap_or_else(|error| {
            tracing::warn!(model = spec.id, %error, "Grammar release lookup failed; requiring Content-Length");
            RemotePackage {
                version: spec.default_version.into(),
                source: api.into(),
                download_url: spec.download_url.into(),
                download_bytes: None,
            }
        })
    } else {
        RemotePackage {
            version: spec.default_version.into(),
            source: spec.download_url.into(),
            download_url: spec.download_url.into(),
            download_bytes: None,
        }
    };
    wanxiang::verify_disk_space(
        wanxiang::available_disk_space(root)?,
        package.download_bytes.unwrap_or(spec.default_size),
    )?;
    let client = wanxiang::addon_client(Duration::from_secs(24 * 60 * 60), true)?;
    let response = tokio::select! {
        biased;
        _ = cancel.changed() => return Err(crate::download::DOWNLOAD_CANCELLED.into()),
        response = client.get(&package.download_url).send() => response
            .map_err(|error| format!("语法模型下载失败：{error}"))?
            .error_for_status()
            .map_err(|error| format!("语法模型下载失败：{error}"))?,
    };
    let expected = package
        .download_bytes
        .or(response.content_length())
        .ok_or("语法模型下载缺少可校验的文件大小")?;
    if response
        .content_length()
        .is_some_and(|size| size != expected)
    {
        return Err("语法模型发布大小与下载大小不符".into());
    }
    package.download_bytes = Some(expected);
    let path = wanxiang::save_grammar_stream(
        root,
        response.bytes_stream(),
        expected,
        cancel,
        &|percent, message| {
            core.emit(CoreEvent::InstallProgress(InstallProgress {
                stage: "wanxiang-grammar-download".into(),
                percent: u32::from(percent),
                message: message.into(),
            }));
        },
    )
    .await?;
    Ok((path, package))
}

pub async fn manage_model(
    core: &Arc<Core>,
    model_id: &str,
    installed: bool,
    host: &impl SchemeHost,
) -> Result<GrammarModelsOverview, CoreError> {
    let spec = find_spec(model_id)
        .ok_or_else(|| CoreError::Other(format!("不支持的语法模型：{model_id}")))?;
    if spec.id == WANXIANG_HANS_ID {
        wanxiang::manage_grammar(core, installed, host).await?;
        let root = host.user_root().map_err(CoreError::Other)?;
        return status(core, &root);
    }

    let result: Result<_, String> = async {
        if installed && !wanxiang::grammar_supported() {
            return Err("当前平台未加载语法模型插件".into());
        }
        let root = host.user_root()?;
        if installed {
            crate::deploy::ensure_not_using_libjd(core, &root)
                .map_err(|error| error.to_string())?;
        }
        crate::package_migration::ensure_before_write(core, &root)
            .await
            .map_err(|error| error.to_string())?;
        let mut cancel = core.download_cancel.subscribe();
        cancel.borrow_and_update();
        core.emit(CoreEvent::InstallProgress(InstallProgress {
            stage: "wanxiang-grammar-download".into(),
            percent: 0,
            message: if installed {
                format!("正在准备{}…", spec.short_name)
            } else {
                format!("正在删除{}…", spec.short_name)
            },
        }));

        let target_path = wanxiang::safe_target(&root, spec.file_name)?;
        if installed {
            let mut receipt = InstallReceipt::new(&root)?;
            let model = if target_path.is_file() {
                let (sha256, size) = wanxiang::fingerprint(&target_path)?;
                wanxiang::verify_download_with_limit(size, Some(size), wanxiang::MAX_GRAMMAR_BYTES)?;
                read_model_manifest(&root, spec)?.unwrap_or(GrammarModel {
                    version: "local".into(),
                    size,
                    sha256,
                })
            } else {
                let (path, package) = download_spec_model(core, &receipt.work, spec, &mut cancel).await?;
                if cancel.has_changed().unwrap_or(false) {
                    return Err(crate::download::DOWNLOAD_CANCELLED.into());
                }
                let (sha256, size) = wanxiang::fingerprint(path.as_ref())?;
                wanxiang::verify_download_with_limit(
                    size,
                    package.download_bytes,
                    wanxiang::MAX_GRAMMAR_BYTES,
                )?;
                receipt.replace(spec.file_name, path.as_ref(), sha256.clone())?;
                GrammarModel {
                    version: package.version,
                    size,
                    sha256,
                }
            };
            write_model_manifest(&root, spec, Some(&model))?;
            receipt.finish()?;
            for pkg in crate::runtime_materialize::installed_packages_using_grammar_model(
                core, &root, spec.id,
            )
            .map_err(|error| error.to_string())?
            {
                core.emit(CoreEvent::InstallProgress(InstallProgress {
                    stage: "wanxiang-grammar-deploy".into(),
                    percent: 95,
                    message: format!("正在部署 {pkg}…"),
                }));
                host.deploy_package(&pkg).await?;
            }
        } else {
            let known_model = read_model_manifest(&root, spec)?;
            let shared_path_before = model_path(&root, spec.id);
            let affected = crate::runtime_materialize::installed_packages_using_grammar_model(
                core, &root, spec.id,
            )
            .map_err(|error| error.to_string())?;
            for pkg in &affected {
                remove_package_linked_model(
                    &root,
                    pkg,
                    spec,
                    shared_path_before.as_deref(),
                    known_model.as_ref(),
                );
                crate::runtime_materialize::update_package_grammar_model(&root, pkg, None)
                    .map_err(|error| error.to_string())?;
            }
            if target_path.is_file() {
                let removable = known_model.as_ref().is_some_and(|model| {
                    wanxiang::fingerprint(&target_path)
                        .ok()
                        .is_some_and(|(sha, size)| sha == model.sha256 && size == model.size)
                });
                if !removable {
                    return Err("语法模型已修改或没有所有权记录，已保留原文件".into());
                }
                fs::remove_file(&target_path)
                    .map_err(|error| format!("删除语法模型失败：{error}"))?;
            }
            write_model_manifest(&root, spec, None)?;
            for pkg in affected {
                core.emit(CoreEvent::InstallProgress(InstallProgress {
                    stage: "wanxiang-grammar-deploy".into(),
                    percent: 95,
                    message: format!("正在重新部署 {pkg}…"),
                }));
                host.deploy_package(&pkg).await?;
            }
        }

        core.emit(CoreEvent::InstallProgress(InstallProgress {
            stage: "done".into(),
            percent: 100,
            message: if installed {
                format!("{}已安装", spec.short_name)
            } else {
                format!("{}已删除", spec.short_name)
            },
        }));
        status(core, &root).map_err(|error| error.to_string())
    }
    .await;
    result.map_err(CoreError::Other)
}

pub async fn set_package_model(
    core: &Core,
    key: &str,
    model_id: Option<&str>,
    host: &impl SchemeHost,
) -> Result<GrammarModelsOverview, CoreError> {
    let result: Result<_, String> = async {
        use crate::runtime_materialize::{
            package_grammar_model, package_installed, update_package_grammar_model,
        };
        let root = host.user_root()?;
        crate::deploy::ensure_not_using_libjd(core, &root)
            .map_err(|error| error.to_string())?;
        crate::package_migration::ensure_before_write(core, &root)
            .await
            .map_err(|error| error.to_string())?;
        if key == wanxiang::SCHEMA_ID || key == crate::addon::EASY_EN_ADDON_ID {
            return Err("该方案不支持单独切换语法模型".into());
        }
        if !package_installed(core, &root, key).map_err(|error| error.to_string())? {
            return Err("方案未安装".into());
        }
        if let Some(id) = model_id {
            let spec = find_spec(id).ok_or_else(|| format!("不支持的语法模型：{id}"))?;
            if !wanxiang::grammar_supported() {
                return Err("当前平台未加载语法模型插件".into());
            }
            if model_path(&root, spec.id).is_none() {
                return Err(format!("请先在「附加功能 → 模型」中下载{}", spec.name));
            }
        }
        let previous = package_grammar_model(&root, key).map_err(|error| error.to_string())?;
        if previous.as_deref() == model_id {
            return status(core, &root).map_err(|error| error.to_string());
        }
        let prev_spec = previous.as_deref().and_then(find_spec);
        let prev_path = prev_spec.and_then(|s| model_path(&root, s.id));
        let prev_manifest = prev_spec.and_then(|s| read_model_manifest(&root, s).ok().flatten());

        update_package_grammar_model(&root, key, model_id).map_err(|error| error.to_string())?;
        if let Some(spec) = prev_spec {
            remove_package_linked_model(
                &root,
                key,
                spec,
                prev_path.as_deref(),
                prev_manifest.as_ref(),
            );
        }
        if let Err(error) = host.deploy_package(key).await {
            update_package_grammar_model(&root, key, previous.as_deref())
                .map_err(|restore| format!("{error}；恢复语法模型设置失败：{restore}"))?;
            host.deploy_package(key)
                .await
                .map_err(|restore| format!("{error}；恢复方案包部署失败：{restore}"))?;
            return Err(error);
        }
        status(core, &root).map_err(|error| error.to_string())
    }
    .await;
    result.map_err(CoreError::Other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_contains_four_models_with_distinct_ids_and_patches() {
        assert_eq!(CATALOG.len(), 4);
        for spec in CATALOG {
            assert!(find_spec(spec.id).is_some());
            let patch = managed_patch_for(spec.id).unwrap();
            assert!(patch.contains(&format!("grammar/language: {}", spec.id)));
            assert!(!spec.description.is_empty());
        }
        assert!(
            managed_patch_for(WANXIANG_HANS_ID)
                .unwrap()
                .contains("collocation_penalty: -14")
        );
        assert!(
            !managed_patch_for(OCTAGRAM_HANS_ID)
                .unwrap()
                .contains("collocation_penalty:")
        );
    }

    #[tokio::test]
    #[cfg(not(target_os = "ios"))]
    async fn switching_between_multiple_grammar_models_updates_runtime_cleanly() {
        use crate::scheme_host::tests::AddonHost;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let host = AddonHost::new(root);
        host.write_bundled_keytao();
        let runtime = RuntimeDirs::isolated_user_data_dir(root, "builtin/keytao").unwrap();
        fs::create_dir_all(&runtime).unwrap();

        for id in [WANXIANG_HANS_ID, OCTAGRAM_HANS_ID] {
            let spec = find_spec(id).unwrap();
            let path = root.join(spec.file_name);
            fs::write(&path, format!("fixture-{id}")).unwrap();
            let (sha256, size) = wanxiang::fingerprint(&path).unwrap();
            write_model_manifest(
                root,
                spec,
                Some(&GrammarModel {
                    version: spec.default_version.into(),
                    size,
                    sha256,
                }),
            )
            .unwrap();
        }

        let overview = set_package_model(&host.core, "builtin/keytao", Some(OCTAGRAM_HANS_ID), &host)
            .await
            .unwrap();
        assert_eq!(
            overview.package_models.get("builtin/keytao").map(String::as_str),
            Some(OCTAGRAM_HANS_ID)
        );
        assert!(runtime.join("zh-hans-t-essay-bgw.gram").is_file());
        assert!(!runtime.join("wanxiang-lts-zh-hans.gram").exists());

        // Switching to Wanxiang Hans removes the old linked .gram and links the new one.
        let overview = set_package_model(&host.core, "builtin/keytao", Some(WANXIANG_HANS_ID), &host)
            .await
            .unwrap();
        assert_eq!(
            overview.package_models.get("builtin/keytao").map(String::as_str),
            Some(WANXIANG_HANS_ID)
        );
        assert!(runtime.join("wanxiang-lts-zh-hans.gram").is_file());
        assert!(!runtime.join("zh-hans-t-essay-bgw.gram").exists());

        // Disabling removes the linked .gram cleanly.
        let overview = set_package_model(&host.core, "builtin/keytao", None, &host)
            .await
            .unwrap();
        assert!(!overview.package_models.contains_key("builtin/keytao"));
        assert!(!runtime.join("wanxiang-lts-zh-hans.gram").exists());
    }
}
