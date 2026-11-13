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
    if bytes.len() > 1024 * 1024 {
        return Err("English 安装记录过大".into());
    }
    let manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|e| format!("English 安装记录损坏：{e}"))?;
    let mut seen = std::collections::BTreeSet::new();
    if manifest.format != 1
        || manifest.files.len() != 5
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
    Ok(Some(manifest))
}

fn is_package_file(path: &str) -> bool {
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
    let schemas = [
        "default.custom.yaml",
        "default-custom.yaml",
        "default.yaml",
        "build/default.yaml",
    ]
    .into_iter()
    .filter_map(|name| std::fs::read_to_string(root.join(name)).ok())
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
            let path = root.join("build").join(format!("{id}.schema.yaml"));
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
        && root
            .join("build")
            .join(format!("{id}.schema.yaml"))
            .is_file();
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
    let mut value = serde_yaml::from_str::<serde_yaml::Value>(&existing)
        .map_err(|error| format!("解析 default custom 配置失败: {error}"))?;
    let mapping = value
        .as_mapping_mut()
        .ok_or("default custom 配置根节点必须是 YAML mapping")?;
    let patch = yaml_child_mapping(mapping, "patch", "patch 必须是 YAML mapping")?;

    let mut schemas = parse_schema_list(&existing);
    schemas.retain(|schema| schema != id);
    if installed {
        schemas.push(id.to_string());
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
    let path = ["default.custom.yaml", "default-custom.yaml"]
        .into_iter()
        .map(|name| root.join(name))
        .find(|path| path.is_file())
        .ok_or("请先安装键道方案")?;
    let existing = std::fs::read_to_string(&path)
        .map_err(|error| format!("读取 {} 失败: {error}", path.display()))?;
    let content = update_addon_schema_list_content(&existing, id, installed)?;
    write_file_atomic(&path, content.as_bytes())
        .map_err(|error| format!("写入附加方案列表失败: {error}"))
}

pub fn addon_schema_status(
    _core: &Core,
    root: impl FnOnce() -> Result<PathBuf, String>,
    id: String,
) -> Result<AddonSchemaStatus, CoreError> {
    let result: Result<_, String> = (|| {
        let id = validate_addon_schema_id(&id)?;
        let root = root()?;
        Ok(addon_schema_status_at(&root, id))
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
    std::fs::write(&custom, CUSTOM).map_err(|e| e.to_string())?;
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
    let previous = read_manifest(&receipt.root)?;
    let legacy =
        previous.is_none() && addon_schema_status_at(&receipt.root, EASY_EN_ADDON_ID).installed;
    for file in &files {
        let destination = wanxiang::safe_target(&receipt.root, &file.path)?;
        if destination.exists() {
            if file.path == "easy_en.custom.yaml" && destination.is_file() {
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
        }
    }
    #[cfg(windows)]
    {
        receipt.public_sources = files.iter().map(|file| file.path.clone()).collect();
    }
    for file in &files {
        if file.path == "easy_en.custom.yaml"
            && wanxiang::safe_target(&receipt.root, &file.path)?.is_file()
        {
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
        files,
    };
    let path = receipt.work.join("manifest.json");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    receipt.replace(MANIFEST, &path, wanxiang::fingerprint(&path)?.0)
}

async fn install_package(
    root: &Path,
    progress: impl Fn(u8, &str) + Send + Sync,
) -> Result<InstallReceipt, String> {
    let receipt = InstallReceipt::new(root)?;
    read_manifest(&receipt.root)?;
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

#[cfg(any(windows, test))]
fn cached_manifest(cache: &Path) -> Result<(PathBuf, Manifest), String> {
    let generation =
        std::fs::read_to_string(cache.join("current.txt")).map_err(|e| e.to_string())?;
    if generation.len() != 64 || !generation.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("English 公共包缓存无效".into());
    }
    let root = wanxiang::safe_target(cache, &generation)?;
    let manifest = read_manifest(&root)?.ok_or("English 公共包缓存缺少安装记录")?;
    Ok((root, manifest))
}

#[cfg(any(windows, test))]
pub(crate) fn official_source_files(cache: &Path) -> Result<Vec<(PathBuf, Vec<u8>)>, String> {
    let (root, manifest) = cached_manifest(cache)?;
    let mut files = Vec::new();
    for file in manifest.files {
        let path = wanxiang::safe_target(&root, &file.path)?;
        if wanxiang::fingerprint(&path)? != (file.sha256, file.size) {
            return Err(format!(
                "English 文件 {} 已修改，请先更新附加方案",
                file.path
            ));
        }
        files.push((
            file.path.into(),
            std::fs::read(path).map_err(|e| e.to_string())?,
        ));
    }
    Ok(files)
}

#[cfg(any(windows, test))]
fn cache_public_package(cache: &Path, stage: &Path, manifest: &Manifest) -> Result<(), String> {
    let destination = cache.join(&manifest.archive_sha256);
    for file in &manifest.files {
        let path = wanxiang::safe_target(&destination, &file.path)?;
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        let source = stage.join(&file.path);
        if wanxiang::fingerprint(&source)? != (file.sha256.clone(), file.size) {
            return Err("English 官方暂存文件校验失败".into());
        }
        std::fs::copy(source, path).map_err(|e| e.to_string())?;
    }
    let content = serde_json::to_vec_pretty(manifest).map_err(|e| e.to_string())?;
    write_file_atomic(&destination.join(MANIFEST), &content).map_err(|e| e.to_string())
}

#[cfg(windows)]
pub(crate) async fn prepare_public_package(root: &Path, cache: &Path) -> Result<(), String> {
    let installed = read_manifest(root)?;
    if cached_manifest(cache).is_ok_and(|(_, cached)| {
        installed
            .as_ref()
            .is_none_or(|installed| installed.source == cached.source)
    }) && official_source_files(cache).is_ok()
    {
        return Ok(());
    }
    let package = if let Some(manifest) = installed {
        if let Some(sha) = manifest
            .source
            .strip_prefix("https://github.com/BlindingDark/rime-easy-en/commit/")
        {
            package_from_commit(sha)?
        } else if manifest.source == BRANCH_SOURCE {
            package_for_install(Err("English installed without version metadata".into()))
        } else {
            return Err("English 上游来源无效".into());
        }
    } else {
        package_for_install(latest_package().await)
    };
    let receipt = InstallReceipt::new(root)?;
    let archive = receipt.work.join("download.zip");
    wanxiang::download(&package, &archive, &|_, _| {}).await?;
    let files = extract_package(&archive, &receipt.work.join("files"))?;
    let manifest = Manifest {
        format: 1,
        version: package.version,
        source: package.source,
        archive_sha256: wanxiang::fingerprint(&archive)?.0,
        files,
    };
    cache_public_package(cache, &receipt.work.join("files"), &manifest)?;
    write_file_atomic(
        &cache.join("current.txt"),
        manifest.archive_sha256.as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    receipt.finish()
}

fn write_schema_list_transaction(
    receipt: &mut InstallReceipt,
    installed: bool,
) -> Result<(), String> {
    let relative = ["default.custom.yaml", "default-custom.yaml"]
        .into_iter()
        .find(|name| receipt.root.join(name).is_file())
        .ok_or("请先安装键道方案")?;
    let existing =
        std::fs::read_to_string(receipt.root.join(relative)).map_err(|e| e.to_string())?;
    let updated = update_addon_schema_list_content(&existing, EASY_EN_ADDON_ID, installed)?;
    let path = receipt.work.join("default.custom.yaml");
    std::fs::write(&path, updated).map_err(|e| e.to_string())?;
    receipt.replace(relative, &path, wanxiang::fingerprint(&path)?.0)
}

fn remove_path_if_present(path: &Path) -> Result<(), String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("读取 {} 失败: {error}", path.display())),
    };
    if metadata.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
    .map_err(|error| format!("删除 {} 失败: {error}", path.display()))
}

fn remove_addon_schema_files(root: &Path, id: &str) -> Result<(), String> {
    for dir in [root.to_path_buf(), root.join("build")] {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("读取 {} 失败: {error}", dir.display())),
        };
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let remove = if dir == root {
                name.starts_with(&format!("{id}.userdb"))
            } else {
                name.starts_with(&format!("{id}."))
            };
            if remove {
                remove_path_if_present(&entry.path())?;
            }
        }
    }
    Ok(())
}

fn uninstall_package(root: &Path) -> Result<InstallReceipt, String> {
    let mut receipt = InstallReceipt::new(root)?;
    read_manifest(&receipt.root)?;
    for relative in addon_schema_source_files(EASY_EN_ADDON_ID)
        .into_iter()
        .map(|(_, relative)| relative)
        .chain([LICENSE.into(), MANIFEST.into()])
    {
        let path = wanxiang::safe_target(&receipt.root, &relative)?;
        if path.exists() {
            receipt.move_original(&relative)?;
        }
    }
    Ok(receipt)
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
    let result: Result<_, String> = async {
        let id = validate_addon_schema_id(&id)?;
        let root = host.user_root()?;
        let base = keytao_core::schema_install_state(&root);
        if !base.installed {
            return Err("请先安装键道方案".into());
        }
        core.emit(CoreEvent::InstallProgress(InstallProgress {
            stage: "addon-download".into(),
            percent: 5,
            message: "正在下载附加方案 English...".into(),
        }));
        let mut receipt = install_package(&root, |percent, message| {
            core.emit(CoreEvent::InstallProgress(InstallProgress {
                stage: "addon-download".into(),
                percent: u32::from(percent),
                message: message.into(),
            }));
        }).await?;
        write_schema_list_transaction(&mut receipt, true)?;
        core.emit(CoreEvent::InstallProgress(InstallProgress {
            stage: "addon-deploy".into(),
            percent: 55,
            message: "正在部署键道方案与 English...".into(),
        }));
        let deployed = async {
            host.deploy().await?;
            if !addon_schema_status_at(&root, id).deployed {
                return Err("附加方案文件已安装，但部署产物缺失".to_string());
            }
            #[cfg(target_os = "windows")]
            {
                let manifest = read_manifest(&root)?.ok_or("English 安装记录缺失")?;
                let cache = core.env().data_dir.join("easy-en-public");
                cache_public_package(&cache, &receipt.work.join("files"), &manifest)?;
                host.publish_wanxiang_sources(&receipt.public_source_files())?;
                if let Err(error) = write_file_atomic(&cache.join("current.txt"), manifest.archive_sha256.as_bytes()) {
                    tracing::warn!(%error, "English public cache selection will be rebuilt when needed");
                }
            }
            Ok(())
        }.await;
        if let Err(error) = deployed {
            receipt.rollback().map_err(|rollback| format!("{error}；恢复 English 失败：{rollback}"))?;
            if let Err(restore) = host.deploy().await {
                return Err(format!("{error}；已恢复文件，请重新部署：{restore}"));
            }
            return Err(error);
        }
        receipt.finish()?;
        core.emit(CoreEvent::InstallProgress(InstallProgress {
            stage: "done".into(),
            percent: 100,
            message: "附加方案 English 已安装并部署".into(),
        }));
        Ok(addon_schema_status_at(&root, id))
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
        core.emit(CoreEvent::InstallProgress(InstallProgress {
            stage: "addon-uninstall".into(),
            percent: 20,
            message: "正在卸载附加方案 English...".into(),
        }));
        let mut receipt = uninstall_package(&root)?;
        write_schema_list_transaction(&mut receipt, false)?;
        let reset_english_mode = reset_english_mode_transaction(&mut receipt)?;
        let deployed = async {
            if keytao_core::schema_install_state(&root).installed {
                host.deploy().await?;
            } else {
                let _ = host.write_reload_stamp(&root)?;
            }
            #[cfg(target_os = "windows")]
            {
                let files = addon_schema_source_files(id)
                    .into_iter()
                    .map(|(_, relative)| PathBuf::from(relative))
                    .collect::<Vec<_>>();
                host.remove_public_schema(id, &files)?;
                host.publish_english_settings(false)?;
            }
            Ok::<_, String>(())
        }
        .await;
        if let Err(error) = deployed {
            receipt
                .rollback()
                .map_err(|rollback| format!("{error}；恢复 English 失败：{rollback}"))?;
            if let Err(restore) = host.deploy().await {
                return Err(format!("{error}；已恢复文件，请重新部署：{restore}"));
            }
            return Err(error);
        }
        receipt.finish()?;
        remove_addon_schema_files(&root, id)?;
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

    fn install_fixture(root: &Path, sha: char, content: &str) -> InstallReceipt {
        let mut receipt = InstallReceipt::new(root).unwrap();
        let archive = receipt.work.join("fixture.zip");
        fixture_archive(&archive, content, &[]);
        let sha = sha.to_string().repeat(40);
        let package = package_from_commit(&sha).unwrap();
        commit_package(&mut receipt, &archive, &package).unwrap();
        write_schema_list_transaction(&mut receipt, true).unwrap();
        receipt
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
            std::fs::read_to_string(root.path().join("easy_en.custom.yaml")).unwrap(),
            CUSTOM
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
        write_schema_list_transaction(&mut receipt, false).unwrap();
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
        write_schema_list_transaction(&mut receipt, false).unwrap();
        receipt.finish().unwrap();
        assert!(!addon_schema_status_at(root.path(), EASY_EN_ADDON_ID).installed);
        for (_, name) in addon_schema_source_files(EASY_EN_ADDON_ID) {
            assert!(!root.path().join(name).exists());
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
                std::fs::read_to_string(root.path().join(name)).unwrap(),
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
            std::fs::read_to_string(root.path().join("easy_en.custom.yaml")).unwrap(),
            custom
        );
        assert!(addon_schema_status_at(root.path(), EASY_EN_ADDON_ID).installed);
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
        assert_eq!(std::fs::read_to_string(&path).unwrap(), custom);
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
    fn public_cache_contains_only_staged_upstream_files_and_rejects_modified_cache() {
        let root = TestRoot::new();
        let archive = root.path().join("fixture.zip");
        fixture_archive(&archive, "official source", &[]);
        let stage = root.path().join("stage");
        let files = extract_package(&archive, &stage).unwrap();
        let manifest = Manifest {
            format: 1,
            version: "aaaaaaaaaaaa".into(),
            source: format!(
                "https://github.com/BlindingDark/rime-easy-en/commit/{}",
                "a".repeat(40)
            ),
            archive_sha256: wanxiang::fingerprint(&archive).unwrap().0,
            files,
        };
        let cache = root.path().join("public-cache");
        cache_public_package(&cache, &stage, &manifest).unwrap();
        assert!(!cache.join("current.txt").exists());
        write_file_atomic(
            &cache.join("current.txt"),
            manifest.archive_sha256.as_bytes(),
        )
        .unwrap();
        std::fs::write(root.path().join("easy_en.dict.yaml"), "private vocabulary").unwrap();
        let public = official_source_files(&cache).unwrap();
        assert_eq!(
            public
                .iter()
                .find(|(path, _)| path == Path::new("easy_en.dict.yaml"))
                .unwrap()
                .1,
            b"official source"
        );
        std::fs::write(
            cache
                .join(&manifest.archive_sha256)
                .join("easy_en.dict.yaml"),
            "modified cache",
        )
        .unwrap();
        assert!(official_source_files(&cache).is_err());
        assert_eq!(
            std::fs::read_to_string(root.path().join("easy_en.dict.yaml")).unwrap(),
            "private vocabulary"
        );
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
        write_schema_list_transaction(&mut receipt, false).unwrap();
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
        write_schema_list_transaction(&mut receipt, true).unwrap();
        receipt.finish().unwrap();
        let status = addon_schema_status_at(root.path(), EASY_EN_ADDON_ID);
        assert!(status.installed);
        assert!(status.version.is_empty());
    }
}
