pub(crate) use crate::deploy::deploy_shared_data_dir as shared_data_dir;
use crate::wanxiang::{self, InstallReceipt, InstalledFile, RemotePackage};
use crate::{
    Core, CoreError, CoreEvent, events::InstallProgress, scheme_files::*, scheme_host::SchemeHost,
    scheme_types::AddonSchemaStatus,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const EASY_EN_ADDON_ID: &str = "easy_en";
const MANIFEST: &str = ".keytao-easy-en.json";
const LICENSE: &str = "easy_en-LICENSE";
const CUSTOM: &str = "# KeyTao add-on defaults: no external word-splitter on mobile/desktop.\npatch:\n  easy_en/split_sentence: false\n";

pub(crate) fn strip_legacy_custom(content: &str) -> String {
    let marker = CUSTOM.lines().next().unwrap();
    if content.trim() != "patch:\n  easy_en/split_sentence: false"
        && !content.lines().any(|line| line == marker)
    {
        return content.to_owned();
    }
    let updated = crate::managed_custom::strip_legacy_patch(
        content,
        "easy_en/split_sentence",
        "false",
        serde_yaml::Value::Bool(false),
    );
    if updated == content {
        return updated;
    }
    updated
        .split_inclusive('\n')
        .filter(|line| line.trim_end_matches(['\r', '\n']) != marker)
        .collect()
}
const COMMITS_ATOM: &str = "https://github.com/BlindingDark/rime-easy-en/commits/master.atom";
const BRANCH_SOURCE: &str = "https://github.com/BlindingDark/rime-easy-en/tree/master";
const BRANCH_ARCHIVE: &str =
    "https://codeload.github.com/BlindingDark/rime-easy-en/zip/refs/heads/master";
// Ownership fingerprints for the former bundled package, not download pins.
const LEGACY_FILES: &[(&str, &str)] = &[
    (
        "easy_en.schema.yaml",
        "34fad4619104f2f6ddd2571b650f6f9b4090cef17e60eb2ba957d9eb439f89e7",
    ),
    (
        "easy_en.dict.yaml",
        "4f039026b2746fa9b0d4d7a248cdf866b64609dca2317708f04e9e68ac7d868a",
    ),
    (
        "lua/easy_en.lua",
        "57b91eae86cce8a1e89bbc3cc212e08268b2426d75a9a2ef72b4893815c8485e",
    ),
];

#[derive(Serialize, Deserialize)]
struct Manifest {
    format: u32,
    version: String,
    source: String,
    archive_sha256: String,
    files: Vec<InstalledFile>,
}

fn read_manifest(root: &Path) -> Result<Option<Manifest>, String> {
    use std::io::Read;
    let path = wanxiang::safe_target(root, MANIFEST)?;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    parse_manifest(&bytes).map(Some)
}

fn parse_manifest(bytes: &[u8]) -> Result<Manifest, String> {
    if bytes.len() > 1024 * 1024 {
        return Err("English 安装记录过大".into());
    }
    let manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|e| format!("English 安装记录损坏：{e}"))?;
    let mut seen = std::collections::BTreeSet::new();
    if manifest.format != 1
        || manifest.files.len() > 5
        || manifest.files.iter().any(|file| {
            !is_package_file(&file.path)
                || !seen.insert(&file.path)
                || file.sha256.len() != 64
                || !file.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        })
    {
        return Err("English 安装记录含无效文件".into());
    }
    if manifest.archive_sha256.len() != 64
        || !manifest
            .archive_sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
    {
        return Err("English 安装记录的压缩包校验值无效".into());
    }
    Ok(manifest)
}

pub(crate) fn legacy_manifest(root: &Path) -> Result<Option<Vec<u8>>, String> {
    let mut files = Vec::new();
    for (name, expected) in LEGACY_FILES {
        let path = wanxiang::safe_target(root, name)?;
        if path.is_file() {
            let (sha256, size) = wanxiang::fingerprint(&path)?;
            if sha256 == *expected {
                files.push(InstalledFile {
                    path: (*name).into(),
                    sha256,
                    size,
                });
            }
        }
    }
    if files.is_empty() {
        return Ok(None);
    }
    serde_json::to_vec(&Manifest {
        format: 1,
        version: String::new(),
        source: BRANCH_SOURCE.into(),
        archive_sha256: "0".repeat(64),
        files,
    })
    .map(Some)
    .map_err(|error| error.to_string())
}

pub(crate) fn official_custom() -> Result<Vec<u8>, String> {
    crate::managed_custom::set_patch("", "english", Some("{easy_en/split_sentence: false}"))
        .map(String::into_bytes)
}

pub(crate) fn removed_sources<'a>(
    root: &Path,
    files: impl Iterator<Item = &'a InstalledFile>,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut sources = Vec::new();
    for file in files.filter(|file| file.path.ends_with(".schema.yaml")) {
        let path = wanxiang::safe_target(root, &file.path)?;
        match std::fs::read(path) {
            Ok(bytes) => sources.push((file.path.clone(), bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(sources)
}

pub(crate) fn cleanup_removed(root: &Path, sources: &[(String, Vec<u8>)]) -> Result<(), String> {
    let (_, content) = addon_default_custom(root)?;
    let remaining = parse_schema_list(&content).into_iter().collect();
    crate::package_artifacts::cleanup(root, sources, &remaining)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub(crate) fn is_package_file(path: &str) -> bool {
    path == LICENSE
        || addon_schema_source_files(EASY_EN_ADDON_ID)
            .iter()
            .any(|(_, relative)| relative == path)
}

fn package_from_commit(sha: &str) -> Result<RemotePackage, String> {
    if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("English 上游版本无效".into());
    }
    Ok(RemotePackage {
        version: sha[..12].into(),
        source: format!("https://github.com/BlindingDark/rime-easy-en/commit/{sha}"),
        download_url: format!("https://codeload.github.com/BlindingDark/rime-easy-en/zip/{sha}"),
        download_bytes: None,
    })
}

fn commit_sha_from_atom(feed: &str) -> Result<&str, String> {
    let entry = feed
        .split_once("<entry>")
        .and_then(|(_, rest)| rest.split_once("</entry>"))
        .map(|(entry, _)| entry)
        .ok_or("English 上游提交信息无效")?;
    let id = entry
        .split_once("<id>")
        .and_then(|(_, rest)| rest.split_once("</id>"))
        .and_then(|(id, _)| id.trim().split_once("Grit::Commit/"))
        .map(|(_, sha)| sha);
    let link = entry.split("<link").skip(1).find_map(|link| {
        let tag = link.split_once('>')?.0;
        let href = tag.split_once("href=")?.1;
        let quote = href.chars().next()?;
        if !matches!(quote, '\'' | '"') {
            return None;
        }
        href[1..]
            .split(quote)
            .next()?
            .split_once("/commit/")
            .map(|(_, sha)| sha)
    });
    [id, link]
        .into_iter()
        .flatten()
        .find(|sha| sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| "English 上游版本无效".into())
}

async fn latest_package() -> Result<RemotePackage, String> {
    let feed = wanxiang::github_text(COMMITS_ATOM).await?;
    package_from_commit(commit_sha_from_atom(&feed)?)
}

fn package_for_install(latest: Result<RemotePackage, String>) -> RemotePackage {
    let mut package = latest.unwrap_or_else(|error| {
        tracing::warn!(%error, "English version lookup failed; installing the default branch");
        RemotePackage {
            version: String::new(),
            source: BRANCH_SOURCE.into(),
            download_url: BRANCH_ARCHIVE.into(),
            download_bytes: None,
        }
    });
    package.download_url = BRANCH_ARCHIVE.into();
    package
}

pub async fn latest_version() -> Result<String, String> {
    Ok(latest_package().await?.version)
}

pub fn validate_addon_schema_id(id: &str) -> Result<&str, String> {
    match id.trim() {
        EASY_EN_ADDON_ID => Ok(EASY_EN_ADDON_ID),
        _ => Err(format!("不支持的附加方案：{id}")),
    }
}

pub fn addon_schema_source_files(id: &str) -> [(&'static str, String); 4] {
    [
        ("easy_en.schema.yaml", format!("{id}.schema.yaml")),
        ("easy_en.dict.yaml", format!("{id}.dict.yaml")),
        ("easy_en.custom.yaml", format!("{id}.custom.yaml")),
        ("lua/easy_en.lua", format!("lua/{id}.lua")),
    ]
}

pub fn deployed_english_schema_available(root: &Path) -> bool {
    let Ok(dirs) = keytao_core::RuntimeDirs::resolve(root, root) else {
        return false;
    };
    let schemas = [
        dirs.user_data_dir.join("default.custom.yaml"),
        dirs.user_data_dir.join("default-custom.yaml"),
        dirs.user_data_dir.join("default.yaml"),
        dirs.staging_dir.join("default.yaml"),
    ]
    .into_iter()
    .filter_map(|path| std::fs::read_to_string(path).ok())
    .map(|text| parse_schema_list(&text))
    .find(|schemas| !schemas.is_empty())
    .unwrap_or_default();
    schemas
        .iter()
        .filter(|id| {
            !id.is_empty()
                && id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
        })
        .any(|id| {
            let path = dirs.staging_dir.join(format!("{id}.schema.yaml"));
            let Some(schema) = std::fs::read(&path)
                .ok()
                .and_then(|bytes| serde_yaml::from_slice::<serde_yaml::Value>(&bytes).ok())
            else {
                return false;
            };
            let name = schema
                .get("schema")
                .and_then(|v| v.get("name"))
                .and_then(serde_yaml::Value::as_str)
                .unwrap_or("");
            keytao_core::english_mode::is_english_schema(id, name)
        })
}

pub fn addon_schema_status_at(root: &Path, id: &str) -> AddonSchemaStatus {
    let dirs = keytao_core::RuntimeDirs::resolve(root, root);
    let configured = ["default.custom.yaml", "default-custom.yaml"]
        .into_iter()
        .find_map(|name| std::fs::read_to_string(root.join(name)).ok())
        .is_some_and(|content| {
            parse_schema_list(&content)
                .iter()
                .any(|schema| schema == id)
        });
    let installed = configured
        && addon_schema_source_files(id)
            .iter()
            .all(|(_, relative)| root.join(relative).is_file());
    let deployed = installed
        && dirs.is_ok_and(|dirs| dirs.staging_dir.join(format!("{id}.schema.yaml")).is_file());
    AddonSchemaStatus {
        installed,
        deployed,
        version: read_manifest(root)
            .ok()
            .flatten()
            .map_or_else(String::new, |manifest| manifest.version),
        english_available: deployed_english_schema_available(root),
    }
}

pub fn update_addon_schema_list_content(
    existing: &str,
    id: &str,
    installed: bool,
) -> Result<String, String> {
    update_addon_schemas_content(existing, &[id.to_owned()], &if installed { vec![id.to_owned()] } else { Vec::new() })
}

pub(crate) fn update_addon_schemas_content(existing: &str, owned: &[String], selected: &[String]) -> Result<String, String> {
    let mut value = serde_yaml::from_str::<serde_yaml::Value>(&existing)
        .map_err(|error| format!("解析 default custom 配置失败: {error}"))?;
    let mapping = value
        .as_mapping_mut()
        .ok_or("default custom 配置根节点必须是 YAML mapping")?;
    let patch = yaml_child_mapping(mapping, "patch", "patch 必须是 YAML mapping")?;

    let mut schemas = parse_schema_list(&existing);
    schemas.retain(|schema| !owned.contains(schema));
    let selected: Vec<_> = selected.iter().filter(|id| !schemas.contains(id)).cloned().collect();
    if !selected.is_empty() {
        if schemas
            .iter()
            .all(|schema| keytao_core::is_addon_schema(schema))
        {
            schemas.splice(0..0, selected.iter().cloned());
        } else {
            schemas.extend_from_slice(&selected);
        }
    }
    let schema_list = serde_yaml::Value::Sequence(
        schemas
            .into_iter()
            .map(|schema| {
                let mut entry = serde_yaml::Mapping::new();
                entry.insert(
                    serde_yaml::Value::String("schema".into()),
                    serde_yaml::Value::String(schema),
                );
                serde_yaml::Value::Mapping(entry)
            })
            .collect(),
    );
    patch.insert(serde_yaml::Value::String("schema_list".into()), schema_list);

    let mut content = serde_yaml::to_string(&value)
        .map_err(|error| format!("序列化 default custom 配置失败: {error}"))?;
    if let Some(stripped) = content.strip_prefix("---\n") {
        content = stripped.to_string();
    }
    Ok(content)
}

pub fn write_addon_schema_list(root: &Path, id: &str, installed: bool) -> Result<(), String> {
    let (relative, existing) = addon_default_custom(root)?;
    let path = root.join(relative);
    let content = update_addon_schema_list_content(&existing, id, installed)?;
    write_file_atomic(&path, content.as_bytes())
        .map_err(|error| format!("写入附加方案列表失败: {error}"))
}

pub(crate) fn addon_default_custom(root: &Path) -> Result<(&'static str, String), String> {
    let relative = ["default.custom.yaml", "default-custom.yaml"]
        .into_iter()
        .find(|name| root.join(name).is_file())
        .unwrap_or("default.custom.yaml");
    let path = wanxiang::safe_target(root, relative)?;
    let existing = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "patch: {}\n".into(),
        Err(error) => return Err(format!("读取 {} 失败: {error}", path.display())),
    };
    Ok((relative, existing))
}

pub fn addon_schema_status(
    core: &Core,
    root: impl FnOnce() -> Result<PathBuf, String>,
    id: String,
) -> Result<AddonSchemaStatus, CoreError> {
    let result: Result<_, String> = (|| {
        let id = validate_addon_schema_id(&id)?;
        let root = root()?;
        let mut status = addon_schema_status_at(&root, id);
        let manifest = crate::addon_store::manifest(core, &root, id)?
            .map(|bytes| parse_manifest(&bytes))
            .transpose()?;
        if keytao_core::RuntimeDirs::isolation_enabled(&root) {
            status.installed = manifest.is_some() && crate::addon_store::isolated_files(core, id)?.is_dir();
            status.deployed = keytao_core::RuntimeDirs::resolve(
                &root,
                &shared_data_dir(core, &root),
            )
            .is_ok_and(|dirs| {
                status.installed
                    && dirs.active_package_key.as_deref().is_some_and(|key| {
                        crate::runtime_materialize::easy_english_enabled(&root, key).unwrap_or(false)
                    })
                    && dirs.staging_dir.join("easy_en.schema.yaml").is_file()
            });
        }
        status.version = manifest.map_or_else(String::new, |manifest| manifest.version);
        Ok(status)
    })();
    result.map_err(CoreError::Other)
}

fn extract_package(archive: &Path, stage: &Path) -> Result<Vec<InstalledFile>, String> {
    let mut files = wanxiang::extract_package(
        archive,
        stage,
        |name| {
            let (_, relative) = name.split_once('/')?;
            match relative {
                "easy_en.schema.yaml" | "easy_en.dict.yaml" | "lua/easy_en.lua" => {
                    Some(relative.into())
                }
                "LICENSE" => Some(LICENSE.into()),
                _ => None,
            }
        },
        |_, _| Ok(()),
    )?;
    for required in [
        "easy_en.schema.yaml",
        "easy_en.dict.yaml",
        "lua/easy_en.lua",
        LICENSE,
    ] {
        if !stage.join(required).is_file() {
            return Err(format!("English 上游压缩包缺少必要文件：{required}"));
        }
    }
    let custom = stage.join("easy_en.custom.yaml");
    std::fs::write(&custom, official_custom()?).map_err(|e| e.to_string())?;
    let (sha256, size) = wanxiang::fingerprint(&custom)?;
    files.push(InstalledFile {
        path: "easy_en.custom.yaml".into(),
        sha256,
        size,
    });
    Ok(files)
}

fn commit_package(
    receipt: &mut InstallReceipt,
    archive: &Path,
    package: &RemotePackage,
) -> Result<(), String> {
    let files = extract_package(archive, &receipt.work.join("files"))?;
    let previous = receipt
        .manifest_bytes(MANIFEST)?
        .map(|bytes| parse_manifest(&bytes))
        .transpose()?;
    let legacy =
        previous.is_none() && addon_schema_status_at(&receipt.root, EASY_EN_ADDON_ID).installed;
    let isolated = receipt.is_isolated();
    let mut owned = if isolated { files.clone() } else { Vec::new() };
    for file in files.iter().filter(|_| !isolated) {
        receipt.preflight_owner(&file.path)?;
        let destination = wanxiang::safe_target(&receipt.root, &file.path)?;
        if destination.exists() {
            if file.path == "easy_en.custom.yaml" && destination.is_file() {
                owned.push(file.clone());
                continue;
            }
            let (hash, _) = wanxiang::fingerprint(&destination)?;
            let legacy_owned = legacy
                && LEGACY_FILES
                    .iter()
                    .any(|(name, old_hash)| *name == file.path && *old_hash == hash);
            if hash != file.sha256
                && !legacy_owned
                && previous
                    .as_ref()
                    .and_then(|manifest| manifest.files.iter().find(|old| old.path == file.path))
                    .is_none_or(|old| old.sha256 != hash)
            {
                return Err(format!(
                    "已有文件 {} 与 English 冲突，已保留原文件",
                    file.path
                ));
            }
            if hash == file.sha256
                && !legacy_owned
                && previous
                    .as_ref()
                    .is_none_or(|manifest| !manifest.files.iter().any(|old| old.path == file.path))
            {
                continue;
            }
        }
        owned.push(file.clone());
    }
    #[cfg(windows)]
    {
        receipt.public_sources = files.iter().map(|file| file.path.clone()).collect();
    }
    for file in files.iter().filter(|_| !isolated) {
        if !owned.iter().any(|owned| owned.path == file.path) {
            continue;
        }
        if file.path == "easy_en.custom.yaml"
            && wanxiang::safe_target(&receipt.root, &file.path)?.is_file()
        {
            let destination = wanxiang::safe_target(&receipt.root, &file.path)?;
            let original =
                std::fs::read_to_string(destination).map_err(|error| error.to_string())?;
            let updated = crate::managed_custom::set_patch(
                &original,
                "english",
                Some("{easy_en/split_sentence: false}"),
            )?;
            if updated != original {
                let source = receipt.work.join("english-custom.yaml");
                std::fs::write(&source, updated).map_err(|error| error.to_string())?;
                receipt.replace(&file.path, &source, wanxiang::fingerprint(&source)?.0)?;
            }
            continue;
        }
        let source = receipt.work.join("files").join(&file.path);
        #[cfg(windows)]
        let source = {
            let copy = receipt.work.join("commit").join(&file.path);
            std::fs::create_dir_all(copy.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::copy(&source, &copy).map_err(|e| e.to_string())?;
            copy
        };
        receipt.replace(&file.path, &source, file.sha256.clone())?;
    }
    let manifest = Manifest {
        format: 1,
        version: package.version.clone(),
        source: package.source.clone(),
        archive_sha256: wanxiang::fingerprint(archive)?.0,
        files: owned,
    };
    let path = receipt.work.join("manifest.json");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    receipt.replace_manifest(MANIFEST, &path)
}

async fn install_package(
    receipt: InstallReceipt,
    progress: impl Fn(u8, &str) + Send + Sync,
) -> Result<InstallReceipt, String> {
    if let Some(bytes) = receipt.manifest_bytes(MANIFEST)? {
        parse_manifest(&bytes)?;
    }
    let package = package_for_install(latest_package().await);
    let archive = receipt.work.join("download.zip");
    wanxiang::download(&package, &archive, &progress).await?;
    tokio::task::spawn_blocking(move || {
        let mut receipt = receipt;
        commit_package(&mut receipt, &archive, &package)?;
        Ok(receipt)
    })
    .await
    .map_err(|e| format!("English 安装任务失败：{e}"))?
}

pub(crate) fn owned_addon_default(root: &Path) -> Result<Option<PathBuf>, String> {
    let path = wanxiang::safe_target(root, "default.yaml")?;
    match std::fs::read(&path) {
        Ok(content) => Ok((content == include_bytes!("addon-default.yaml")).then_some(path)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("读取 {} 失败: {error}", path.display())),
    }
}

pub(crate) fn write_schema_list_transaction(
    receipt: &mut InstallReceipt,
    id: &str,
    installed: bool,
    shared: &Path,
) -> Result<(), String> {
    let (relative, existing) = addon_default_custom(&receipt.root)?;
    let (owned, selected) = receipt.schema_selection(id)?;
    let updated = update_addon_schemas_content(&existing, &owned, if installed { &selected } else { &[] })?;
    write_schema_list_content_transaction(receipt, relative, &updated, shared, None)
}

pub(crate) fn write_schema_list_content_transaction(
    receipt: &mut InstallReceipt,
    relative: &str,
    updated: &str,
    shared: &Path,
    rime_lua: Option<&str>,
) -> Result<(), String> {
    let path = receipt.work.join("default.custom.yaml");
    std::fs::write(&path, updated).map_err(|e| e.to_string())?;
    receipt.replace(relative, &path, wanxiang::fingerprint(&path)?.0)?;
    let schemas = parse_schema_list(updated);
    let addon_only = schemas
        .iter()
        .all(|schema| keytao_core::is_addon_schema(schema));
    if schemas.is_empty() || !addon_only {
        if owned_addon_default(&receipt.root)?.is_some() {
            receipt.move_original("default.yaml")?;
        }
    } else if !wanxiang::safe_target(&receipt.root, "default.yaml")?.is_file()
        && !shared.join("default.yaml").is_file()
    {
        let path = receipt.work.join("default.yaml");
        std::fs::write(&path, include_str!("addon-default.yaml")).map_err(|e| e.to_string())?;
        receipt.replace("default.yaml", &path, wanxiang::fingerprint(&path)?.0)?;
    }
    if addon_only {
        let path = receipt.work.join("active.json");
        let manifest = serde_json::json!({
            "root": receipt.root,
            "scheme_key": schemas.first().map(String::as_str).unwrap_or(""),
            "manifest": {},
            "rime_lua": rime_lua,
        });
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        receipt.replace(
            keytao_core::ACTIVE_SCHEME_MANIFEST,
            &path,
            wanxiang::fingerprint(&path)?.0,
        )?;
        if rime_lua.is_none() {
            keytao_core::preserve_rime_lua_user_edits(&receipt.root, None)?;
            receipt.move_original("rime.lua")?;
        }
    }
    Ok(())
}

#[cfg(test)]

fn uninstall_package(root: &Path) -> Result<InstallReceipt, String> {
    let mut receipt = InstallReceipt::new(root)?;
    uninstall_receipt(&mut receipt)?;
    Ok(receipt)
}

fn uninstall_receipt(receipt: &mut InstallReceipt) -> Result<(), String> {
    let manifest = receipt
        .manifest_bytes(MANIFEST)?
        .or(legacy_manifest(&receipt.root)?)
        .map(|bytes| parse_manifest(&bytes))
        .transpose()?;
    for file in manifest.into_iter().flat_map(|manifest| manifest.files) {
        let path = wanxiang::safe_target(&receipt.root, &file.path)?;
        if path.is_file() {
            if file.path == "easy_en.custom.yaml" {
                let original = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
                let updated = crate::managed_custom::strip(&strip_legacy_custom(&original))?;
                if !updated.is_empty() {
                    if updated != original {
                        let source = receipt.work.join("english-custom.yaml");
                        std::fs::write(&source, updated).map_err(|error| error.to_string())?;
                        receipt.replace(&file.path, &source, wanxiang::fingerprint(&source)?.0)?;
                    }
                    continue;
                }
            }
            if wanxiang::fingerprint(&path)?.0 == file.sha256 {
                receipt.move_original(&file.path)?;
            }
        }
    }
    // The managed section is generated, so it may not appear in owned files.
    let path = wanxiang::safe_target(&receipt.root, "easy_en.custom.yaml")?;
    if path.is_file() {
        let original = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
        let updated = crate::managed_custom::strip(&strip_legacy_custom(&original))?;
        if updated != original {
            let source = receipt.work.join("english-custom.yaml");
            std::fs::write(&source, updated).map_err(|error| error.to_string())?;
            receipt.replace(
                "easy_en.custom.yaml",
                &source,
                wanxiang::fingerprint(&source)?.0,
            )?;
        }
    }
    if receipt.root.join(MANIFEST).is_file() {
        receipt.move_original(MANIFEST)?;
    }
    Ok(())
}

#[cfg(any(target_os = "android", target_os = "ios"))]
fn reset_mobile_english_mode(root: &Path) -> Result<bool, String> {
    let path = android_ime_config_path(root);
    let mut config = read_android_ime_config(&path);
    if config
        .get("englishMode")
        .and_then(serde_json::Value::as_str)
        != Some("schema")
    {
        return Ok(false);
    }
    config.insert(
        "englishMode".into(),
        serde_json::Value::String("ascii".into()),
    );
    let content = serde_json::to_vec_pretty(&config)
        .map_err(|error| format!("序列化移动端输入设置失败: {error}"))?;
    write_file_atomic(&path, &content)
        .map_err(|error| format!("写入 {} 失败: {error}", path.display()))?;
    Ok(true)
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn reset_mobile_english_mode(_root: &Path) -> Result<bool, String> {
    use keytao_core::english_mode::{EnglishMode, read_english_mode, write_english_mode};
    let changed = read_english_mode(_root) != EnglishMode::Ascii;
    if changed {
        write_english_mode(_root, EnglishMode::Ascii)?;
    }
    Ok(changed)
}

fn reset_english_mode_transaction(receipt: &mut InstallReceipt) -> Result<bool, String> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    let relative = android_ime_config_path(Path::new(""))
        .to_string_lossy()
        .into_owned();
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let relative = keytao_core::english_mode::SETTINGS_FILE.to_string();
    let source = wanxiang::safe_target(&receipt.root, &relative)?;
    let stage = receipt.work.join("settings");
    std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    if source.is_file() {
        std::fs::copy(&source, stage.join(&relative)).map_err(|e| e.to_string())?;
    }
    let changed = reset_mobile_english_mode(&stage)?;
    if changed {
        let path = stage.join(&relative);
        receipt.replace(&relative, &path, wanxiang::fingerprint(&path)?.0)?;
    }
    Ok(changed)
}

#[cfg(any(target_os = "android", target_os = "ios"))]
pub fn android_ime_config_path(root: &Path) -> PathBuf {
    #[cfg(target_os = "ios")]
    {
        root.join("ios_ime.json")
    }
    #[cfg(target_os = "android")]
    {
        root.join("android_ime.json")
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
pub fn read_android_ime_config(path: &Path) -> serde_json::Map<String, serde_json::Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

pub async fn addon_schema_install(
    core: &Core,
    id: String,
    host: &impl SchemeHost,
) -> Result<AddonSchemaStatus, CoreError> {
    addon_schema_install_with(core, id, host, |receipt| async move {
        install_package(receipt, |percent, message| {
            core.emit(CoreEvent::InstallProgress(InstallProgress {
                stage: "addon-download".into(),
                percent: u32::from(percent),
                message: message.into(),
            }));
        })
        .await
    })
    .await
}

async fn addon_schema_install_with<Install, InstallFuture>(
    core: &Core,
    id: String,
    host: &impl SchemeHost,
    install: Install,
) -> Result<AddonSchemaStatus, CoreError>
where
    Install: FnOnce(InstallReceipt) -> InstallFuture,
    InstallFuture: std::future::Future<Output = Result<InstallReceipt, String>>,
{
    let result: Result<_, String> = async {
        let id = validate_addon_schema_id(&id)?;
        let root = host.user_root()?;
        core.emit(CoreEvent::InstallProgress(InstallProgress {
            stage: "addon-download".into(),
            percent: 5,
            message: "正在下载附加方案 English...".into(),
        }));
        let mut receipt = InstallReceipt::new(&root)?;
        receipt.attach_store(core, id)?;
        let mut receipt = install(receipt).await?;
        if receipt.is_isolated() {
            receipt.commit_store(true)?;
            receipt.finish()?;
            return addon_schema_status(core, || Ok(root), id.into())
                .map_err(|error| error.to_string());
        }
        write_schema_list_transaction(&mut receipt, id, true, &shared_data_dir(core, &root))?;
        core.emit(CoreEvent::InstallProgress(InstallProgress {
            stage: "addon-deploy".into(),
            percent: 55,
            message: "正在部署键道方案与 English...".into(),
        }));
        #[cfg(target_os = "windows")]
        let mut public_update = None;
        let deployed = async {
            #[cfg(target_os = "windows")]
            let mut public_source_warning = None;
            #[cfg(target_os = "windows")]
            match crate::windows_public_schemas::update_public_schemas(|| {
                host.publish_wanxiang_sources(&receipt.public_source_files())
            }) {
                Ok(update) => public_update = Some(update),
                Err(error) => {
                    public_source_warning = Some(crate::windows_public_schemas::public_warning(
                        "english-public-prepare",
                        &error,
                    ));
                }
            }
            host.deploy().await?;
            #[cfg(target_os = "windows")]
            if let Some(warning) = public_source_warning {
                crate::deploy::emit_legacy_progress(
                    core,
                    keytao_core::DeployStep::Done,
                    format!("系统搜索框方案未更新：{warning}"),
                );
            }
            if !addon_schema_status_at(&root, id).deployed {
                return Err("附加方案文件已安装，但部署产物缺失".to_string());
            }
            receipt.commit_store(true)?;
            Ok(())
        }
        .await;
        if let Err(error) = deployed {
            let rollback = receipt.rollback();
            #[cfg(target_os = "windows")]
            let public_rollback = public_update.map_or(Ok(()), |update| update.rollback());
            #[cfg(target_os = "windows")]
            let error = match &public_rollback {
                Ok(()) => error,
                Err(restore) => format!("{error}；恢复公共方案失败：{restore}"),
            };
            rollback.map_err(|rollback| format!("{error}；恢复 English 失败：{rollback}"))?;
            #[cfg(target_os = "windows")]
            public_rollback.map_err(|_| error.clone())?;
            if let Err(restore) = host.deploy().await {
                return Err(format!("{error}；已恢复文件，请重新部署：{restore}"));
            }
            return Err(error);
        }
        receipt.finish()?;
        crate::packages::sync_generated(core, &root).map_err(|error| error.to_string())?;
        core.emit(CoreEvent::InstallProgress(InstallProgress {
            stage: "done".into(),
            percent: 100,
            message: "附加方案 English 已安装并部署".into(),
        }));
        addon_schema_status(core, || Ok(root), id.into()).map_err(|error| error.to_string())
    }
    .await;
    result.map_err(CoreError::Other)
}

pub async fn addon_schema_uninstall(
    core: &Core,
    id: String,
    host: &impl SchemeHost,
) -> Result<AddonSchemaStatus, CoreError> {
    let result: Result<_, String> = async {
        let id = validate_addon_schema_id(&id)?;
        let root = host.user_root()?;
        if keytao_core::RuntimeDirs::isolation_enabled(&root) {
            let key = keytao_core::RuntimeDirs::active_package_key(&root)
                .ok_or("没有使用中的方案包")?;
            set_package_easy_english(core, &key, false, host).await?;
            return addon_schema_status(core, || Ok(root), id.into())
                .map_err(|error| error.to_string());
        }
        core.emit(CoreEvent::InstallProgress(InstallProgress {
            stage: "addon-uninstall".into(),
            percent: 20,
            message: "正在卸载附加方案 English...".into(),
        }));
        let mut receipt = InstallReceipt::new(&root)?;
        receipt.attach_store(core, id)?;
        let previous = receipt
            .manifest_bytes(MANIFEST)?
            .map(|bytes| parse_manifest(&bytes))
            .transpose()?;
        let removed = removed_sources(
            &root,
            previous
                .as_ref()
                .into_iter()
                .flat_map(|manifest| &manifest.files),
        )?;
        uninstall_receipt(&mut receipt)?;
        write_schema_list_transaction(&mut receipt, id, false, &shared_data_dir(core, &root))?;
        let reset_english_mode = reset_english_mode_transaction(&mut receipt)?;
        #[cfg(target_os = "windows")]
        let mut public_update = None;
        let deployed = async {
            #[cfg(target_os = "windows")]
            let public_source_warning = {
                let files = addon_schema_source_files(id)
                    .into_iter()
                    .map(|(_, relative)| PathBuf::from(relative))
                    .collect::<Vec<_>>();
                match crate::windows_public_schemas::update_public_schemas(|| {
                    host.remove_public_schema(id, &files)?;
                    host.publish_english_settings(false)
                }) {
                    Ok(update) => {
                        public_update = Some(update);
                        None
                    }
                    Err(error) => Some(crate::windows_public_schemas::public_warning(
                        "english-public-remove",
                        &error,
                    )),
                }
            };
            if keytao_core::schema_install_state(&root).installed {
                host.deploy().await?;
            } else {
                let _ = host.write_reload_stamp(&root)?;
            }
            #[cfg(target_os = "windows")]
            if let Some(warning) = public_source_warning {
                crate::deploy::emit_legacy_progress(
                    core,
                    keytao_core::DeployStep::Done,
                    format!("系统搜索框方案未更新：{warning}"),
                );
            }
            cleanup_removed(&root, &removed)?;
            receipt.commit_store(false)?;
            Ok::<_, String>(())
        }
        .await;
        if let Err(error) = deployed {
            let rollback = receipt.rollback();
            #[cfg(target_os = "windows")]
            let public_rollback = public_update.map_or(Ok(()), |update| update.rollback());
            #[cfg(target_os = "windows")]
            let error = match &public_rollback {
                Ok(()) => error,
                Err(restore) => format!("{error}；恢复公共方案失败：{restore}"),
            };
            rollback.map_err(|rollback| format!("{error}；恢复 English 失败：{rollback}"))?;
            #[cfg(target_os = "windows")]
            public_rollback.map_err(|_| error.clone())?;
            if let Err(restore) = host.deploy().await {
                return Err(format!("{error}；已恢复文件，请重新部署：{restore}"));
            }
            return Err(error);
        }
        receipt.finish()?;
        crate::packages::sync_generated(core, &root).map_err(|error| error.to_string())?;
        if reset_english_mode {
            core.emit(CoreEvent::InstallProgress(InstallProgress {
                stage: "addon-reset-mode".into(),
                percent: 90,
                message: "英文模式已切回 ASCII 模式".into(),
            }));
        }
        core.emit(CoreEvent::InstallProgress(InstallProgress {
            stage: "done".into(),
            percent: 100,
            message: "附加方案 English 已卸载".into(),
        }));
        Ok(addon_schema_status_at(&root, id))
    }
    .await;
    result.map_err(CoreError::Other)
}

pub async fn set_package_easy_english(
    core: &Core,
    key: &str,
    enabled: bool,
    host: &impl SchemeHost,
) -> Result<(), String> {
    use crate::runtime_materialize::{easy_english_enabled, update_easy_english};
    use keytao_core::RuntimeDirs;
    let root = host.user_root()?;
    if !RuntimeDirs::isolation_enabled(&root)
        || key == EASY_EN_ADDON_ID
        || RuntimeDirs::active_package_key(&root).as_deref() != Some(key)
    {
        return Err("使用中的方案包已变更".into());
    }
    if easy_english_enabled(&root, key).map_err(|error| error.to_string())? == enabled {
        return Ok(());
    }
    if enabled && !crate::addon_store::isolated_files(core, EASY_EN_ADDON_ID)?.is_dir() {
        addon_schema_install(core, EASY_EN_ADDON_ID.into(), host)
            .await
            .map_err(|error| error.to_string())?;
    }
    if update_easy_english(&root, key, enabled).map_err(|error| error.to_string())?
        && let Err(error) = host.deploy_package(key).await
    {
        update_easy_english(&root, key, !enabled)
            .map_err(|restore| format!("{error}；恢复 English 设置失败：{restore}"))?;
        host.deploy_package(key)
            .await
            .map_err(|restore| format!("{error}；恢复方案包部署失败：{restore}"))?;
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../keytao-core/tests/support/smoke_fixture.rs"]
mod smoke_fixture;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    struct TestRoot {
        directory: tempfile::TempDir,
        _serial: std::sync::MutexGuard<'static, ()>,
    }

    impl TestRoot {
        fn new() -> Self {
            let serial = wanxiang::TEST_SERIAL
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let directory = tempfile::tempdir().unwrap();
            std::fs::write(
                directory.path().join("default.custom.yaml"),
                "patch:\n  schema_list:\n    - schema: keytao\n",
            )
            .unwrap();
            Self {
                directory,
                _serial: serial,
            }
        }

        fn path(&self) -> &Path {
            self.directory.path()
        }
    }

    fn fixture_archive(path: &Path, content: &str, extras: &[(&str, &str)]) {
        let mut archive = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, text) in [
            ("rime-easy-en-commit/easy_en.schema.yaml", content),
            ("rime-easy-en-commit/easy_en.dict.yaml", content),
            ("rime-easy-en-commit/lua/easy_en.lua", content),
            ("rime-easy-en-commit/LICENSE", "LGPL-3.0 fixture"),
            ("rime-easy-en-commit/README.md", "not installed"),
        ]
        .into_iter()
        .chain(extras.iter().copied())
        {
            archive
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(text.as_bytes()).unwrap();
        }
        archive.finish().unwrap();
    }

    fn commit_fixture_package(root: &Path, sha: char, content: &str) -> InstallReceipt {
        commit_fixture_receipt(InstallReceipt::new(root).unwrap(), sha, content)
    }

    fn commit_fixture_receipt(
        mut receipt: InstallReceipt,
        sha: char,
        content: &str,
    ) -> InstallReceipt {
        let archive = receipt.work.join("fixture.zip");
        fixture_archive(&archive, content, &[]);
        let sha = sha.to_string().repeat(40);
        let package = package_from_commit(&sha).unwrap();
        commit_package(&mut receipt, &archive, &package).unwrap();
        receipt
    }

    fn install_fixture(root: &Path, sha: char, content: &str) -> InstallReceipt {
        let mut receipt = commit_fixture_package(root, sha, content);
        write_schema_list_transaction(&mut receipt, EASY_EN_ADDON_ID, true, root).unwrap();
        receipt
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn easy_english_toggle_redeploys_only_the_active_package() {
        use crate::scheme_host::tests::AddonHost;
        use keytao_core::RuntimeDirs;
        use std::fs;

        let directory = TestRoot::new();
        let root = directory.path();
        let host = AddonHost::new(root);
        let bundled = host.core.env().resource_dir.join("rime-data");
        smoke_fixture::write(&bundled).unwrap();
        fs::write(root.join(RuntimeDirs::SETTINGS_FILE), br#"{"runtime_isolation":true}"#).unwrap();
        let active = keytao_core::package_store::active_manifest(root);
        fs::create_dir_all(active.parent().unwrap()).unwrap();
        let first = "builtin/keytao";
        let second = "builtin/second";
        host.core.lock_state().installed_packages.insert(
            second.into(),
            crate::state::InstalledPackage { version: None },
        );
        for key in [second, first] {
            fs::write(&active, serde_json::json!({"active_package": key}).to_string()).unwrap();
            host.deploy_package(key).await.unwrap();
        }
        let mut receipt = InstallReceipt::new(root).unwrap();
        receipt.attach_store(&host.core, EASY_EN_ADDON_ID).unwrap();
        let archive = receipt.work.join("fixture.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        for (suffix, content) in [
            ("schema.yaml", fs::read_to_string(bundled.join(format!("{}.schema.yaml", smoke_fixture::SCHEMA_ID))).unwrap()
                .replace(smoke_fixture::SCHEMA_ID, EASY_EN_ADDON_ID)),
            ("dict.yaml", fs::read_to_string(bundled.join(format!("{}.dict.yaml", smoke_fixture::SCHEMA_ID))).unwrap()
                .replace(smoke_fixture::SCHEMA_ID, EASY_EN_ADDON_ID)),
        ] {
            zip.start_file(format!("rime-easy-en-commit/easy_en.{suffix}"), zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        for (path, content) in [("lua/easy_en.lua", "return {}"), ("LICENSE", "fixture")] {
            zip.start_file(format!("rime-easy-en-commit/{path}"), zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        commit_package(&mut receipt, &archive, &package_from_commit(&"a".repeat(40)).unwrap()).unwrap();
        receipt.commit_store(true).unwrap();
        receipt.finish().unwrap();

        let first_dirs = RuntimeDirs::for_package(root, &bundled, first).unwrap();
        let second_dirs = RuntimeDirs::for_package(root, &bundled, second).unwrap();
        fs::remove_file(second_dirs.staging_dir.join("default.yaml")).unwrap();
        let second_before = crate::runtime_materialize::test_hashes(&second_dirs.user_data_dir);
        for enabled in [true, false] {
            set_package_easy_english(&host.core, first, enabled, &host).await.unwrap();
            assert_eq!(crate::runtime_materialize::easy_english_enabled(root, first).unwrap(), enabled);
            let built = fs::read_to_string(first_dirs.staging_dir.join("default.yaml")).unwrap();
            assert_eq!(keytao_core::parse_schema_list(&built).iter().any(|id| id == EASY_EN_ADDON_ID), enabled);
            assert_eq!(first_dirs.user_data_dir.join("lua/easy_en.lua").is_file(), enabled);
            assert_eq!(first_dirs.staging_dir.join("easy_en.schema.yaml").is_file(), enabled);
            assert_eq!(crate::runtime_materialize::test_hashes(&second_dirs.user_data_dir), second_before);
            assert_eq!(RuntimeDirs::active_package_key(root).as_deref(), Some(first));
        }
        assert!(!root.join("easy_en.schema.yaml").exists());
        assert!(set_package_easy_english(&host.core, second, true, &host).await.is_err());
    }

    #[tokio::test]
    async fn standalone_english_installs_deploys_and_uninstalls_without_a_main_scheme() {
        use crate::scheme_host::tests::AddonHost;
        use std::sync::atomic::Ordering;
        let root = TestRoot::new();
        std::fs::write(
            root.path().join("default.custom.yaml"),
            "patch:\n  menu/page_size: 9\n  key_binder/bindings: []\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("rime.lua"),
            "stale = require('old_main')\n",
        )
        .unwrap();
        let host = AddonHost::new(root.path());
        let shared_default_available = shared_data_dir(&host.core, root.path())
            .join("default.yaml")
            .is_file();
        let status = addon_schema_install_with(
            &host.core,
            EASY_EN_ADDON_ID.into(),
            &host,
            |receipt| async move { Ok(commit_fixture_receipt(receipt, 'a', "upstream")) },
        )
        .await
        .unwrap();
        assert!(status.installed && status.deployed);
        assert_eq!(host.deploys.load(Ordering::SeqCst), 1);
        let config = std::fs::read_to_string(root.path().join("default.custom.yaml")).unwrap();
        assert_eq!(parse_schema_list(&config), [EASY_EN_ADDON_ID]);
        let config: serde_yaml::Value = serde_yaml::from_str(&config).unwrap();
        assert_eq!(config["patch"]["menu/page_size"], 9);
        assert!(config["patch"]["key_binder/bindings"].is_sequence());
        assert_eq!(
            root.path().join("default.yaml").is_file(),
            !shared_default_available
        );
        assert!(!root.path().join("rime.lua").exists());
        let manifest: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.path().join(keytao_core::ACTIVE_SCHEME_MANIFEST)).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["scheme_key"], EASY_EN_ADDON_ID);
        assert!(manifest["rime_lua"].is_null());
        assert_eq!(
            std::fs::read(root.path().join(".keytao-rime-user-0.lua")).unwrap(),
            b"stale = require('old_main')\n"
        );
        let report = crate::package_orphans::report(&host.core, root.path()).unwrap();
        assert!(
            report
                .kept_backups
                .iter()
                .any(|backup| Path::new(&backup.backup)
                    == root.path().join(".keytao-rime-user-0.lua"))
        );
        std::fs::write(
            root.path().join("rime.lua"),
            "manual = require('new_user_hook')\n",
        )
        .unwrap();
        let status = addon_schema_uninstall(&host.core, EASY_EN_ADDON_ID.into(), &host)
            .await
            .unwrap();
        assert!(!status.installed && !status.deployed);
        assert_eq!(host.deploys.load(Ordering::SeqCst), 1);
        assert_eq!(host.reloads.load(Ordering::SeqCst), 1);
        assert!(
            parse_schema_list(
                &std::fs::read_to_string(root.path().join("default.custom.yaml")).unwrap()
            )
            .is_empty()
        );
        assert_eq!(keytao_core::deployed_schema_id_from_dir(root.path()), None);
        assert!(!root.path().join("default.yaml").exists());
        assert_eq!(
            std::fs::read(root.path().join(".keytao-rime-user-1.lua")).unwrap(),
            b"manual = require('new_user_hook')\n"
        );
        let report = crate::package_orphans::report(&host.core, root.path()).unwrap();
        assert!(
            report
                .kept_backups
                .iter()
                .any(|backup| Path::new(&backup.backup)
                    == root.path().join(".keytao-rime-user-1.lua"))
        );
    }

    #[test]
    fn standalone_addons_use_shared_defaults_without_shadowing_them() {
        for id in [
            EASY_EN_ADDON_ID,
            wanxiang::SCHEMA_ID,
            crate::rime_ice::SCHEMA_ID,
            "rime_mint",
            "rime_frost",
            "triple_pinyin_lssp",
        ] {
            let root = TestRoot::new();
            std::fs::write(root.path().join("default.custom.yaml"), "patch: {}\n").unwrap();
            let shared = root.path().join("rime-data");
            std::fs::create_dir_all(&shared).unwrap();
            std::fs::write(shared.join("default.yaml"), "config_version: shared\n").unwrap();
            let mut receipt = InstallReceipt::new(root.path()).unwrap();
            write_schema_list_transaction(&mut receipt, id, true, &shared).unwrap();
            receipt.finish().unwrap();
            assert!(!root.path().join("default.yaml").exists());
            assert_eq!(
                std::fs::read_to_string(shared.join("default.yaml")).unwrap(),
                "config_version: shared\n"
            );
        }
    }

    #[test]
    fn standalone_fallback_cleanup_preserves_edits_and_rolls_back() {
        for id in [
            EASY_EN_ADDON_ID,
            wanxiang::SCHEMA_ID,
            crate::rime_ice::SCHEMA_ID,
            "rime_mint",
            "rime_frost",
            "triple_pinyin_lssp",
        ] {
            for edited in [false, true] {
                let root = TestRoot::new();
                std::fs::write(root.path().join("default.custom.yaml"), "patch: {}\n").unwrap();
                let mut receipt = InstallReceipt::new(root.path()).unwrap();
                write_schema_list_transaction(&mut receipt, id, true, root.path()).unwrap();
                receipt.finish().unwrap();
                let path = root.path().join("default.yaml");
                let mut original = std::fs::read_to_string(&path).unwrap();
                if edited {
                    original.push_str("# User edit\n");
                    std::fs::write(&path, &original).unwrap();
                }
                let mut receipt = InstallReceipt::new(root.path()).unwrap();
                write_schema_list_transaction(&mut receipt, id, false, root.path()).unwrap();
                assert_eq!(path.exists(), edited);
                drop(receipt);
                assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
                let mut receipt = InstallReceipt::new(root.path()).unwrap();
                write_schema_list_transaction(&mut receipt, id, false, root.path()).unwrap();
                receipt.finish().unwrap();
                assert_eq!(path.exists(), edited);
            }
        }
    }

    #[tokio::test]
    async fn standalone_setup_rolls_back_on_failed_deploy() {
        use crate::scheme_host::tests::AddonHost;
        use std::sync::atomic::Ordering;
        let root = TestRoot::new();
        std::fs::remove_file(root.path().join("default.custom.yaml")).unwrap();
        std::fs::write(root.path().join("rime.lua"), "old lua").unwrap();
        let host = AddonHost::new(root.path());
        host.fail_deploys.store(1, Ordering::SeqCst);
        assert!(
            addon_schema_install_with(
                &host.core,
                EASY_EN_ADDON_ID.into(),
                &host,
                |receipt| async move { Ok(commit_fixture_receipt(receipt, 'a', "upstream")) }
            )
            .await
            .is_err()
        );
        for name in [
            "default.yaml",
            "default.custom.yaml",
            keytao_core::ACTIVE_SCHEME_MANIFEST,
            "easy_en.schema.yaml",
            MANIFEST,
        ] {
            assert!(!root.path().join(name).exists(), "{name}");
        }
        assert_eq!(
            std::fs::read_to_string(root.path().join("rime.lua")).unwrap(),
            "old lua"
        );
    }

    #[tokio::test]
    async fn main_scheme_keeps_priority_when_installing_english() {
        use crate::scheme_host::tests::AddonHost;
        let root = TestRoot::new();
        std::fs::write(
            root.path().join("keytao.schema.yaml"),
            "schema: {schema_id: keytao}\n",
        )
        .unwrap();
        std::fs::write(root.path().join("rime.lua"), "main lua").unwrap();
        let host = AddonHost::new(root.path());
        addon_schema_install_with(
            &host.core,
            EASY_EN_ADDON_ID.into(),
            &host,
            |receipt| async move { Ok(commit_fixture_receipt(receipt, 'a', "upstream")) },
        )
        .await
        .unwrap();
        assert_eq!(
            parse_schema_list(
                &std::fs::read_to_string(root.path().join("default.custom.yaml")).unwrap()
            ),
            ["keytao", EASY_EN_ADDON_ID]
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("rime.lua")).unwrap(),
            "main lua"
        );
        assert!(
            !root
                .path()
                .join(keytao_core::ACTIVE_SCHEME_MANIFEST)
                .exists()
        );
    }

    #[test]
    fn standalone_defaults_compile_and_input_with_librime() {
        for id in [
            EASY_EN_ADDON_ID,
            wanxiang::SCHEMA_ID,
            crate::rime_ice::SCHEMA_ID,
            "rime_mint",
            "rime_frost",
            "triple_pinyin_lssp",
        ] {
            let root = TestRoot::new();
            smoke_fixture::write(root.path()).unwrap();
            for suffix in ["schema.yaml", "dict.yaml"] {
                let source = root
                    .path()
                    .join(format!("{}.{suffix}", smoke_fixture::SCHEMA_ID));
                let content = std::fs::read_to_string(&source)
                    .unwrap()
                    .replace(smoke_fixture::SCHEMA_ID, id);
                let content = if suffix == "schema.yaml" {
                    format!(
                        "{content}\npunctuator:\n  import_preset: default\nkey_binder:\n  import_preset: default\nrecognizer:\n  import_preset: default\n"
                    )
                } else {
                    content
                };
                std::fs::write(root.path().join(format!("{id}.{suffix}")), content).unwrap();
                std::fs::remove_file(source).unwrap();
            }
            if id == wanxiang::SCHEMA_ID {
                std::fs::write(
                    root.path().join("wanxiang.schema.yaml"),
                    include_str!("wanxiang-basic.schema.yaml"),
                )
                .unwrap();
                std::fs::write(
                    root.path().join("wanxiang_algebra.yaml"),
                    "base: {全拼: {__append: []}}\n",
                )
                .unwrap();
                let mut dictionary = "---\nname: wanxiang\nversion: '1'\nsort: by_weight\nuse_preset_vocabulary: false\n...\n".to_string();
                for (index, word) in [
                    "你好", "你号", "拟好", "逆好", "腻好", "妮好", "匿好", "泥好",
                ]
                .iter()
                .enumerate()
                {
                    dictionary.push_str(&format!("{word}\tnihao\t{}\n", 100 - index));
                }
                std::fs::write(root.path().join("wanxiang.dict.yaml"), dictionary).unwrap();
            }
            std::fs::remove_file(root.path().join("default.yaml")).unwrap();
            std::fs::write(root.path().join("default.custom.yaml"), "patch: {}\n").unwrap();
            let mut receipt = InstallReceipt::new(root.path()).unwrap();
            write_schema_list_transaction(&mut receipt, id, true, root.path()).unwrap();
            let runtime = keytao_core::ImeRuntime::with_dirs(
                root.path(),
                root.path().to_string_lossy().into_owned(),
            );
            runtime.init().unwrap();
            assert!(keytao_core::schema_install_state(root.path()).deployed);
            let session = runtime.create_session().unwrap();
            session.select_schema(id).unwrap();
            let code = if id == wanxiang::SCHEMA_ID {
                "nihao"
            } else {
                smoke_fixture::CODE
            };
            for character in code.chars() {
                session.process_key_result(character as u32, 0).unwrap();
            }
            assert!(!session.state().candidates.is_empty());
            if id == wanxiang::SCHEMA_ID {
                assert_eq!(session.state().candidates.len(), 5);
                assert_eq!(
                    session
                        .process_key_result(b'=' as u32, 0)
                        .unwrap()
                        .state
                        .page,
                    1
                );
                assert_eq!(
                    session
                        .process_key_result(b'-' as u32, 0)
                        .unwrap()
                        .state
                        .page,
                    0
                );
                assert_eq!(
                    session
                        .process_key_result(b' ' as u32, 0)
                        .unwrap()
                        .state
                        .committed
                        .as_deref(),
                    Some("你好")
                );
                assert_eq!(
                    session
                        .process_key_result(b',' as u32, 0)
                        .unwrap()
                        .state
                        .committed
                        .as_deref(),
                    Some("，")
                );
            }
            receipt.finish().unwrap();
        }
    }

    #[test]
    fn installing_another_standalone_addon_preserves_the_first_and_uninstall_keeps_it() {
        let root = TestRoot::new();
        std::fs::write(root.path().join("default.custom.yaml"), "patch: {}\n").unwrap();
        install_fixture(root.path(), 'a', "upstream")
            .finish()
            .unwrap();
        let mut receipt = InstallReceipt::new(root.path()).unwrap();
        write_schema_list_transaction(&mut receipt, wanxiang::SCHEMA_ID, true, root.path())
            .unwrap();
        assert_eq!(
            parse_schema_list(
                &std::fs::read_to_string(root.path().join("default.custom.yaml")).unwrap()
            ),
            [wanxiang::SCHEMA_ID, EASY_EN_ADDON_ID]
        );
        receipt.finish().unwrap();
        let mut receipt = InstallReceipt::new(root.path()).unwrap();
        write_schema_list_transaction(&mut receipt, wanxiang::SCHEMA_ID, false, root.path())
            .unwrap();
        receipt.finish().unwrap();
        assert_eq!(
            parse_schema_list(
                &std::fs::read_to_string(root.path().join("default.custom.yaml")).unwrap()
            ),
            [EASY_EN_ADDON_ID]
        );
        assert!(addon_schema_status_at(root.path(), EASY_EN_ADDON_ID).installed);
    }

    #[test]
    fn fixture_install_writes_only_package_files_custom_defaults_and_version() {
        let root = TestRoot::new();
        install_fixture(root.path(), 'a', "first upstream")
            .finish()
            .unwrap();
        let status = addon_schema_status_at(root.path(), EASY_EN_ADDON_ID);
        assert!(status.installed);
        assert!(!status.deployed);
        assert_eq!(status.version, "aaaaaaaaaaaa");
        for name in [
            "easy_en.schema.yaml",
            "easy_en.dict.yaml",
            "lua/easy_en.lua",
        ] {
            assert_eq!(
                std::fs::read_to_string(root.path().join(name)).unwrap(),
                "first upstream"
            );
        }
        assert_eq!(
            crate::managed_custom::strip(
                &std::fs::read_to_string(root.path().join("easy_en.custom.yaml")).unwrap()
            )
            .unwrap(),
            ""
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join(LICENSE)).unwrap(),
            "LGPL-3.0 fixture"
        );
        assert!(!root.path().join("README.md").exists());
        assert!(!root.path().join("LICENSE").exists());
        let manifest = read_manifest(root.path()).unwrap().unwrap();
        assert_eq!(manifest.version, "aaaaaaaaaaaa");
        assert_eq!(manifest.files.len(), 5);
        assert!(manifest.source.ends_with(&"a".repeat(40)));
    }

    #[test]
    fn update_replaces_owned_sources_and_manifest_version() {
        let root = TestRoot::new();
        install_fixture(root.path(), 'a', "first upstream")
            .finish()
            .unwrap();
        install_fixture(root.path(), 'b', "updated upstream")
            .finish()
            .unwrap();
        assert_eq!(
            addon_schema_status_at(root.path(), EASY_EN_ADDON_ID).version,
            "bbbbbbbbbbbb"
        );
        for name in [
            "easy_en.schema.yaml",
            "easy_en.dict.yaml",
            "lua/easy_en.lua",
        ] {
            assert_eq!(
                std::fs::read_to_string(root.path().join(name)).unwrap(),
                "updated upstream"
            );
        }
        assert_eq!(
            parse_schema_list(
                &std::fs::read_to_string(root.path().join("default.custom.yaml")).unwrap()
            ),
            ["keytao", "easy_en"]
        );
    }

    #[test]
    fn legacy_install_without_receipt_is_detected_and_can_update_and_uninstall() {
        let root = TestRoot::new();
        for (_, name) in addon_schema_source_files(EASY_EN_ADDON_ID) {
            let path = root.path().join(&name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(
                path,
                if name == "easy_en.custom.yaml" {
                    CUSTOM
                } else {
                    "current upstream"
                },
            )
            .unwrap();
        }
        write_addon_schema_list(root.path(), EASY_EN_ADDON_ID, true).unwrap();
        let status = addon_schema_status_at(root.path(), EASY_EN_ADDON_ID);
        assert!(status.installed);
        assert!(status.version.is_empty());
        install_fixture(root.path(), 'c', "current upstream")
            .finish()
            .unwrap();
        assert_eq!(
            addon_schema_status_at(root.path(), EASY_EN_ADDON_ID).version,
            "cccccccccccc"
        );
        let mut receipt = uninstall_package(root.path()).unwrap();
        write_schema_list_transaction(&mut receipt, EASY_EN_ADDON_ID, false, root.path()).unwrap();
        receipt.finish().unwrap();
        assert!(!addon_schema_status_at(root.path(), EASY_EN_ADDON_ID).installed);
        assert!(!root.path().join(MANIFEST).exists());
        assert!(!root.path().join(LICENSE).exists());
    }

    #[test]
    fn legacy_install_can_uninstall_without_creating_a_receipt() {
        let root = TestRoot::new();
        for (_, name) in addon_schema_source_files(EASY_EN_ADDON_ID) {
            let path = root.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "legacy bundled source").unwrap();
        }
        write_addon_schema_list(root.path(), EASY_EN_ADDON_ID, true).unwrap();
        let mut receipt = uninstall_package(root.path()).unwrap();
        write_schema_list_transaction(&mut receipt, EASY_EN_ADDON_ID, false, root.path()).unwrap();
        receipt.finish().unwrap();
        assert!(!addon_schema_status_at(root.path(), EASY_EN_ADDON_ID).installed);
        for (_, name) in addon_schema_source_files(EASY_EN_ADDON_ID) {
            assert!(root.path().join(name).exists());
        }
    }

    #[test]
    fn modified_legacy_custom_is_preserved_on_update() {
        let root = TestRoot::new();
        for (_, name) in addon_schema_source_files(EASY_EN_ADDON_ID) {
            let path = root.path().join(&name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(
                path,
                if name == "easy_en.custom.yaml" {
                    "patch: {user_setting: true}"
                } else {
                    "current upstream"
                },
            )
            .unwrap();
        }
        write_addon_schema_list(root.path(), EASY_EN_ADDON_ID, true).unwrap();
        install_fixture(root.path(), 'a', "current upstream")
            .finish()
            .unwrap();
        for (_, name) in addon_schema_source_files(EASY_EN_ADDON_ID) {
            let expected = if name == "easy_en.custom.yaml" {
                "patch: {user_setting: true}"
            } else {
                "current upstream"
            };
            assert_eq!(
                if name == "easy_en.custom.yaml" {
                    crate::managed_custom::strip(
                        &std::fs::read_to_string(root.path().join(name)).unwrap(),
                    )
                    .unwrap()
                } else {
                    std::fs::read_to_string(root.path().join(name)).unwrap()
                },
                expected
            );
        }
        assert_eq!(
            addon_schema_status_at(root.path(), EASY_EN_ADDON_ID).version,
            "aaaaaaaaaaaa"
        );
    }

    #[test]
    fn existing_custom_is_preserved_on_first_install() {
        let root = TestRoot::new();
        let custom = "# User settings\npatch: {user_setting: true}\n";
        std::fs::write(root.path().join("easy_en.custom.yaml"), custom).unwrap();
        install_fixture(root.path(), 'a', "first upstream")
            .finish()
            .unwrap();
        assert_eq!(
            crate::managed_custom::strip(
                &std::fs::read_to_string(root.path().join("easy_en.custom.yaml")).unwrap()
            )
            .unwrap(),
            custom
        );
        assert!(addon_schema_status_at(root.path(), EASY_EN_ADDON_ID).installed);
        uninstall_package(root.path()).unwrap().finish().unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("easy_en.custom.yaml")).unwrap(),
            custom
        );
    }

    #[test]
    fn matching_unowned_sources_are_not_claimed_or_removed() {
        let root = TestRoot::new();
        for name in [
            "easy_en.schema.yaml",
            "easy_en.dict.yaml",
            "lua/easy_en.lua",
        ] {
            let path = root.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "same official bytes").unwrap();
        }
        install_fixture(root.path(), 'a', "same official bytes")
            .finish()
            .unwrap();
        uninstall_package(root.path()).unwrap().finish().unwrap();
        for name in [
            "easy_en.schema.yaml",
            "easy_en.dict.yaml",
            "lua/easy_en.lua",
        ] {
            assert_eq!(
                std::fs::read_to_string(root.path().join(name)).unwrap(),
                "same official bytes"
            );
        }
    }

    #[test]
    fn legacy_defaults_with_user_customizations_and_user_databases_are_preserved() {
        let original = format!("{CUSTOM}  user_setting: true\n");
        assert_eq!(
            strip_legacy_custom(&original),
            "patch:\n  user_setting: true\n"
        );
        let root = TestRoot::new();
        std::fs::create_dir(root.path().join("build")).unwrap();
        std::fs::write(root.path().join("easy_en.userdb"), "private vocabulary").unwrap();
        std::fs::write(root.path().join("build/easy_en.table.bin"), "compiled").unwrap();
        std::fs::write(
            root.path().join("keytao.schema.yaml"),
            "schema: {schema_id: keytao}\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("build/keytao.schema.yaml"),
            "schema: {schema_id: keytao}\n",
        )
        .unwrap();
        let sources = vec![(
            "easy_en.schema.yaml".into(),
            b"schema: {schema_id: easy_en}\ntranslator: {dictionary: easy_en}\n".to_vec(),
        )];
        cleanup_removed(root.path(), &sources).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("easy_en.userdb")).unwrap(),
            "private vocabulary"
        );
        assert!(!root.path().join("build/easy_en.table.bin").exists());
    }

    #[test]
    fn modified_receipt_custom_is_preserved_on_update_redeploy_and_rollback() {
        let root = TestRoot::new();
        install_fixture(root.path(), 'a', "first upstream")
            .finish()
            .unwrap();
        let custom = "# User settings\npatch: {user_setting: true}\n";
        let path = root.path().join("easy_en.custom.yaml");
        std::fs::write(&path, custom).unwrap();
        install_fixture(root.path(), 'b', "updated upstream")
            .finish()
            .unwrap();
        install_fixture(root.path(), 'b', "updated upstream")
            .finish()
            .unwrap();
        assert_eq!(
            crate::managed_custom::strip(&std::fs::read_to_string(&path).unwrap()).unwrap(),
            custom
        );
        assert_eq!(
            addon_schema_status_at(root.path(), EASY_EN_ADDON_ID).version,
            "bbbbbbbbbbbb"
        );
        let receipt = install_fixture(root.path(), 'c', "cancelled upstream");
        let edited = "patch: {edited_during_deploy: true}\n";
        std::fs::write(&path, edited).unwrap();
        receipt.rollback().unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), edited);
        assert_eq!(
            addon_schema_status_at(root.path(), EASY_EN_ADDON_ID).version,
            "bbbbbbbbbbbb"
        );
    }

    #[test]
    fn modified_owned_source_prevents_every_update_write() {
        let root = TestRoot::new();
        install_fixture(root.path(), 'a', "first upstream")
            .finish()
            .unwrap();
        std::fs::write(root.path().join("easy_en.dict.yaml"), "user vocabulary").unwrap();
        let manifest = std::fs::read(root.path().join(MANIFEST)).unwrap();
        let mut receipt = InstallReceipt::new(root.path()).unwrap();
        let archive = receipt.work.join("fixture.zip");
        fixture_archive(&archive, "updated upstream", &[]);
        let package = package_from_commit(&"b".repeat(40)).unwrap();
        assert!(
            commit_package(&mut receipt, &archive, &package)
                .unwrap_err()
                .contains("冲突")
        );
        receipt.rollback().unwrap();
        assert_eq!(std::fs::read(root.path().join(MANIFEST)).unwrap(), manifest);
        assert_eq!(
            std::fs::read_to_string(root.path().join("easy_en.schema.yaml")).unwrap(),
            "first upstream"
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("easy_en.dict.yaml")).unwrap(),
            "user vocabulary"
        );
    }

    #[test]
    fn stored_sources_stay_official_and_uninstall_preserves_modified_files() {
        use crate::scheme_host::tests::AddonHost;
        let root = TestRoot::new();
        let host = AddonHost::new(root.path());
        let mut receipt = InstallReceipt::new(root.path()).unwrap();
        receipt.attach_store(&host.core, EASY_EN_ADDON_ID).unwrap();
        let mut receipt = commit_fixture_receipt(receipt, 'a', "official source");
        receipt.commit_store(true).unwrap();
        receipt.finish().unwrap();
        assert!(!root.path().join(MANIFEST).exists());
        std::fs::write(root.path().join("easy_en.dict.yaml"), "private vocabulary").unwrap();
        let public = crate::addon_store::public_files(&host.core, EASY_EN_ADDON_ID).unwrap();
        assert_eq!(
            std::fs::read(
                &public
                    .iter()
                    .find(|(_, path)| path == Path::new("easy_en.dict.yaml"))
                    .unwrap()
                    .0
            )
            .unwrap(),
            b"official source"
        );
        let mut receipt = InstallReceipt::new(root.path()).unwrap();
        receipt.attach_store(&host.core, EASY_EN_ADDON_ID).unwrap();
        uninstall_receipt(&mut receipt).unwrap();
        receipt.commit_store(false).unwrap();
        receipt.finish().unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("easy_en.dict.yaml")).unwrap(),
            "private vocabulary"
        );
        assert!(!root.path().join("easy_en.schema.yaml").exists());
    }

    #[tokio::test]
    async fn stored_english_updates_and_failed_deploy_restores_the_previous_package() {
        use crate::scheme_host::tests::AddonHost;
        use std::sync::atomic::Ordering;
        let root = TestRoot::new();
        std::fs::write(root.path().join("default.custom.yaml"), "patch: {}\n").unwrap();
        let host = AddonHost::new(root.path());
        addon_schema_install_with(
            &host.core,
            EASY_EN_ADDON_ID.into(),
            &host,
            |receipt| async { Ok(commit_fixture_receipt(receipt, 'a', "first source")) },
        )
        .await
        .unwrap();
        let status = addon_schema_install_with(
            &host.core,
            EASY_EN_ADDON_ID.into(),
            &host,
            |receipt| async { Ok(commit_fixture_receipt(receipt, 'b', "second source")) },
        )
        .await
        .unwrap();
        assert_eq!(status.version, "bbbbbbbbbbbb");
        host.fail_deploys.store(1, Ordering::SeqCst);
        assert!(
            addon_schema_install_with(
                &host.core,
                EASY_EN_ADDON_ID.into(),
                &host,
                |receipt| async { Ok(commit_fixture_receipt(receipt, 'c', "third source")) }
            )
            .await
            .is_err()
        );
        let status = addon_schema_status(
            &host.core,
            || Ok(root.path().to_path_buf()),
            EASY_EN_ADDON_ID.into(),
        )
        .unwrap();
        assert_eq!(status.version, "bbbbbbbbbbbb");
        assert_eq!(
            std::fs::read_to_string(root.path().join("easy_en.schema.yaml")).unwrap(),
            "second source"
        );
        assert!(!root.path().join(MANIFEST).exists());
        addon_schema_uninstall(&host.core, EASY_EN_ADDON_ID.into(), &host)
            .await
            .unwrap();
        assert!(crate::addon_store::public_files(&host.core, EASY_EN_ADDON_ID).is_err());
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    #[test]
    fn uninstall_resets_english_before_reload_and_rollback_restores_the_preference() {
        use keytao_core::english_mode::{EnglishMode, SETTINGS_FILE, read_english_mode};
        let root = TestRoot::new();
        install_fixture(root.path(), 'a', "upstream")
            .finish()
            .unwrap();
        let original = b"{\"englishMode\":\"schema\",\"other\":7}";
        std::fs::write(root.path().join(SETTINGS_FILE), original).unwrap();
        let mut receipt = uninstall_package(root.path()).unwrap();
        write_schema_list_transaction(&mut receipt, EASY_EN_ADDON_ID, false, root.path()).unwrap();
        assert!(reset_english_mode_transaction(&mut receipt).unwrap());
        assert_eq!(read_english_mode(root.path()), EnglishMode::Ascii);
        drop(receipt);
        assert_eq!(
            std::fs::read(root.path().join(SETTINGS_FILE)).unwrap(),
            original
        );
        assert_eq!(read_english_mode(root.path()), EnglishMode::Schema);
        assert!(addon_schema_status_at(root.path(), EASY_EN_ADDON_ID).installed);
    }

    #[test]
    fn cancelled_install_and_update_restore_sources_manifest_and_schema_list() {
        let root = TestRoot::new();
        let initial_config = std::fs::read(root.path().join("default.custom.yaml")).unwrap();
        drop(install_fixture(root.path(), 'a', "first upstream"));
        assert!(!root.path().join(MANIFEST).exists());
        assert!(!root.path().join("easy_en.schema.yaml").exists());
        assert_eq!(
            std::fs::read(root.path().join("default.custom.yaml")).unwrap(),
            initial_config
        );
        install_fixture(root.path(), 'a', "first upstream")
            .finish()
            .unwrap();
        let manifest = std::fs::read(root.path().join(MANIFEST)).unwrap();
        let config = std::fs::read(root.path().join("default.custom.yaml")).unwrap();
        drop(install_fixture(root.path(), 'b', "updated upstream"));
        assert_eq!(std::fs::read(root.path().join(MANIFEST)).unwrap(), manifest);
        assert_eq!(
            std::fs::read(root.path().join("default.custom.yaml")).unwrap(),
            config
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("easy_en.schema.yaml")).unwrap(),
            "first upstream"
        );
    }

    #[test]
    fn archive_rejects_multiple_roots_that_target_the_same_file() {
        let root = TestRoot::new();
        let archive = root.path().join("fixture.zip");
        fixture_archive(
            &archive,
            "upstream",
            &[("other-root/easy_en.schema.yaml", "other")],
        );
        assert!(
            extract_package(&archive, &root.path().join("stage"))
                .unwrap_err()
                .contains("重复目标路径")
        );
        assert!(!root.path().join("easy_en.schema.yaml").exists());
    }

    #[test]
    fn metadata_requires_a_full_hex_commit_and_pins_the_archive_to_it() {
        for sha in ["", "short", "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"] {
            assert!(package_from_commit(sha).is_err());
        }
        let sha = "a".repeat(40);
        let package = package_from_commit(&sha).unwrap();
        assert!(package.download_url.ends_with(&sha));
        assert_eq!(package.version, sha[..12]);
    }

    #[test]
    fn atom_uses_the_first_entry_commit_id_or_link() {
        let first = "a".repeat(40);
        let second = "b".repeat(40);
        let feed = format!(
            "<feed><id>feed-id</id><entry><id>tag:github.com,2008:Grit::Commit/{first}</id></entry><entry><id>Grit::Commit/{second}</id></entry></feed>"
        );
        assert_eq!(commit_sha_from_atom(&feed).unwrap(), first);
        for quote in ['\'', '"'] {
            let feed = format!(
                "<feed><entry><link href={quote}https://github.com/BlindingDark/rime-easy-en/commit/{first}{quote}/></entry></feed>"
            );
            assert_eq!(commit_sha_from_atom(&feed).unwrap(), first);
        }
    }

    #[test]
    fn atom_rejects_missing_invalid_or_later_entry_commits() {
        for sha in ["", "short", &"g".repeat(40), &"a".repeat(41)] {
            let feed = format!("<feed><entry><id>Grit::Commit/{sha}</id></entry></feed>");
            assert!(commit_sha_from_atom(&feed).is_err());
        }
        assert!(commit_sha_from_atom("<feed/>").is_err());
        assert!(commit_sha_from_atom(&format!(
            "<feed><entry><id>invalid</id></entry><entry><id>Grit::Commit/{}</id></entry></feed>",
            "a".repeat(40)
        )).is_err());
    }

    #[test]
    fn install_uses_master_and_survives_a_failed_version_lookup() {
        let known = package_for_install(package_from_commit(&"a".repeat(40)));
        assert_eq!(known.download_url, BRANCH_ARCHIVE);
        assert_eq!(known.version, "aaaaaaaaaaaa");

        let package = package_for_install(Err("version lookup unavailable".into()));
        assert_eq!(package.download_url, BRANCH_ARCHIVE);
        assert_eq!(package.source, BRANCH_SOURCE);
        assert!(package.version.is_empty());
        let root = TestRoot::new();
        let mut receipt = InstallReceipt::new(root.path()).unwrap();
        let archive = receipt.work.join("fixture.zip");
        fixture_archive(&archive, "upstream", &[]);
        commit_package(&mut receipt, &archive, &package).unwrap();
        write_schema_list_transaction(&mut receipt, EASY_EN_ADDON_ID, true, root.path()).unwrap();
        receipt.finish().unwrap();
        let status = addon_schema_status_at(root.path(), EASY_EN_ADDON_ID);
        assert!(status.installed);
        assert!(status.version.is_empty());
    }
}
