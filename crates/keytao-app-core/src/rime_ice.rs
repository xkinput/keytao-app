//! Runtime-downloaded Rime Ice sources, isolated from other installed schemes.

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

pub const SCHEMA_ID: &str = "rime_ice";
const RELEASES_ATOM: &str = "https://github.com/iDvel/rime-ice/releases.atom";
const RELEASES: &str = "https://api.github.com/repos/iDvel/rime-ice/releases?per_page=20";
const MANIFEST: &str = ".keytao-rime-ice.json";
const NOTICE: &str = "rime_ice-NOTICE.txt";
const LICENSE: &str = "rime_ice-LICENSE.txt";
const MAX_DOWNLOAD_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EXPANDED_BYTES: u64 = 128 * 1024 * 1024;
const REQUIRED: &[&str] = &[
    "rime_ice.schema.yaml",
    "rime_ice.dict.yaml",
    "melt_eng.schema.yaml",
    "melt_eng.dict.yaml",
    "radical_pinyin.schema.yaml",
    "radical_pinyin.dict.yaml",
    "symbols_v.yaml",
    "rime_ice_custom_phrase.txt",
    "opencc/rime_ice_emoji.json",
    "opencc/rime_ice_emoji.txt",
    "opencc/rime_ice_others.txt",
    "lua/rime_ice/lunar.db",
    "rime_ice-dicts/en_dicts/cn_en.txt",
    LICENSE,
];

#[derive(Serialize, Deserialize)]
struct Manifest {
    format: u32,
    version: String,
    source: String,
    archive_sha256: String,
    files: Vec<InstalledFile>,
}

fn valid_version(tag: &str) -> bool {
    let bytes = tag.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'.'
        && bytes[7] == b'.'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| matches!(i, 4 | 7) || b.is_ascii_digit())
        && tag[5..7]
            .parse::<u8>()
            .is_ok_and(|month| (1..=12).contains(&month))
        && tag[8..10]
            .parse::<u8>()
            .is_ok_and(|day| (1..=31).contains(&day))
}

fn package_from_version(version: &str) -> RemotePackage {
    RemotePackage {
        version: version.into(),
        source: format!("https://github.com/iDvel/rime-ice/tree/{version}"),
        download_url: format!(
            "https://github.com/iDvel/rime-ice/releases/download/{version}/full.zip"
        ),
        download_bytes: None,
    }
}

fn release_package_from_atom(feed: &str) -> Result<RemotePackage, String> {
    // GitHub release IDs carry the tag; titles and release-note links do not.
    let version = feed
        .split("<entry>")
        .skip(1)
        .filter_map(|entry| {
            let (entry, _) = entry.split_once("</entry>")?;
            let (_, id) = entry.split_once("<id>")?;
            id.split_once("</id>")?
                .0
                .trim()
                .strip_prefix("tag:github.com,2008:Repository/334110599/")
        })
        .filter(|tag| valid_version(tag))
        .max()
        .ok_or("雾凇拼音发布订阅中没有 YYYY.MM.DD 版本")?;
    Ok(package_from_version(version))
}

fn release_package(releases: &str) -> Result<RemotePackage, String> {
    let releases: serde_json::Value = serde_json::from_str(releases).map_err(|e| e.to_string())?;
    let version = releases
        .as_array()
        .ok_or("雾凇拼音发布信息无效")?
        .iter()
        .filter(|release| release["draft"] != true && release["prerelease"] != true)
        .filter(|release| {
            release["assets"]
                .as_array()
                .is_some_and(|assets| assets.iter().any(|asset| asset["name"] == "full.zip"))
        })
        .filter_map(|release| release["tag_name"].as_str())
        .filter(|tag| valid_version(tag))
        .max()
        .ok_or("雾凇拼音未发布带版本号的 full.zip")?;
    Ok(package_from_version(version))
}

async fn latest_package() -> Result<RemotePackage, String> {
    latest_package_with(wanxiang::github_text).await
}

async fn latest_package_with<F, Fut>(fetch: F) -> Result<RemotePackage, String>
where
    F: Fn(&'static str) -> Fut,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    let feed_error = match fetch(RELEASES_ATOM)
        .await
        .and_then(|feed| release_package_from_atom(&feed))
    {
        Ok(package) => return Ok(package),
        Err(error) => error,
    };
    fetch(RELEASES)
        .await
        .and_then(|releases| release_package(&releases))
        .map_err(|error| {
            format!("雾凇拼音更新查询失败：Atom 订阅：{feed_error}；API 回退：{error}")
        })
}

pub async fn latest_version() -> Result<String, String> {
    Ok(latest_package().await?.version)
}

fn dictionary_file(name: &str) -> bool {
    !name.contains('/')
        && name.ends_with(".dict.yaml")
        && !matches!(name, "41448.dict.yaml" | "tencent.dict.yaml")
}

fn package_destination(name: &str) -> Option<String> {
    if matches!(
        name,
        "rime_ice.schema.yaml"
            | "rime_ice.dict.yaml"
            | "melt_eng.schema.yaml"
            | "melt_eng.dict.yaml"
            | "radical_pinyin.schema.yaml"
            | "radical_pinyin.dict.yaml"
            | "symbols_v.yaml"
    ) {
        return Some(name.into());
    }
    match name {
        "LICENSE" => return Some(LICENSE.into()),
        "custom_phrase.txt" => return Some("rime_ice_custom_phrase.txt".into()),
        "opencc/emoji.json" => return Some("opencc/rime_ice_emoji.json".into()),
        "opencc/emoji.txt" => return Some("opencc/rime_ice_emoji.txt".into()),
        "opencc/others.txt" => return Some("opencc/rime_ice_others.txt".into()),
        _ => {}
    }
    if let Some(name) = name.strip_prefix("cn_dicts/") {
        return dictionary_file(name).then(|| format!("rime_ice-dicts/cn_dicts/{name}"));
    }
    if let Some(name) = name.strip_prefix("en_dicts/") {
        return (!name.contains('/') && (name.ends_with(".dict.yaml") || name.ends_with(".txt")))
            .then(|| format!("rime_ice-dicts/en_dicts/{name}"));
    }
    let name = name.strip_prefix("lua/")?;
    (name.ends_with(".lua") || name == "lunar.db").then(|| format!("lua/rime_ice/{name}"))
}

fn installable(name: &str) -> bool {
    REQUIRED.contains(&name)
        || name == NOTICE
        || name.strip_prefix("rime_ice-dicts/").is_some_and(|name| {
            package_destination(name)
                .is_some_and(|destination| destination == format!("rime_ice-dicts/{name}"))
        })
        || name
            .strip_prefix("lua/rime_ice/")
            .is_some_and(|name| package_destination(&format!("lua/{name}")).is_some())
}

fn read_manifest(root: &Path) -> Result<Option<Manifest>, String> {
    let path = wanxiang::safe_target(root, MANIFEST)?;
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 1024 * 1024 {
        return Err("雾凇拼音安装记录过大".into());
    }
    let manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|e| format!("雾凇拼音安装记录损坏：{e}"))?;
    let mut seen = BTreeSet::new();
    if manifest.format != 1
        || manifest.files.len() > 4096
        || manifest.archive_sha256.len() != 64
        || !manifest
            .archive_sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
    {
        return Err("雾凇拼音安装记录无效".into());
    }
    for file in &manifest.files {
        wanxiang::safe_target(root, &file.path)?;
        if !installable(&file.path)
            || !seen.insert(file.path.to_ascii_lowercase())
            || file.sha256.len() != 64
            || !file.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("雾凇拼音安装记录含无效文件".into());
        }
    }
    Ok(Some(manifest))
}

pub fn status_at(root: &Path) -> Status {
    let manifest = read_manifest(root).ok().flatten();
    let installed = manifest.as_ref().is_some_and(|manifest| {
        manifest
            .files
            .iter()
            .map(|file| file.path.as_str())
            .chain(REQUIRED.iter().copied())
            .all(|name| wanxiang::safe_target(root, name).is_ok_and(|path| path.is_file()))
    });
    Status {
        installed,
        deployed: installed
            && root.join("build/rime_ice.schema.yaml").is_file()
            && root.join("build/rime_ice.table.bin").is_file(),
        version: manifest.map_or_else(String::new, |manifest| manifest.version),
        variant: String::new(),
    }
}

fn adapt_source(name: &str, path: &Path) -> Result<(), String> {
    if !(name.ends_with(".schema.yaml")
        || name.ends_with(".lua")
        || name == "rime_ice.dict.yaml"
        || name == "melt_eng.dict.yaml"
        || name == "opencc/rime_ice_emoji.json")
    {
        return Ok(());
    }
    let mut content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    if name.ends_with(".schema.yaml") {
        let schema: serde_yaml::Value =
            serde_yaml::from_str(&content).map_err(|e| e.to_string())?;
        for group in ["processors", "translators", "filters"] {
            for component in schema
                .get("engine")
                .and_then(|engine| engine.get(group))
                .and_then(serde_yaml::Value::as_sequence)
                .into_iter()
                .flatten()
                .filter_map(serde_yaml::Value::as_str)
            {
                if let Some((kind, name)) = component.split_once("@*") {
                    if kind.starts_with("lua_") {
                        let (module, namespace) = name.split_once('@').unwrap_or((name, name));
                        // Keep the configuration namespace while relocating the module.
                        content = content
                            .replace(component, &format!("{kind}@*rime_ice/{module}@{namespace}"));
                    }
                }
            }
        }
        content = content
            .replace(
                "opencc_config: emoji.json",
                "opencc_config: rime_ice_emoji.json",
            )
            .replace(
                "user_dict: custom_phrase",
                "user_dict: rime_ice_custom_phrase",
            )
            .replace("en_dicts/", "rime_ice-dicts/en_dicts/");
    } else if name.ends_with(".dict.yaml") {
        // The optional Tencent table dominates deployment time on mobile.
        content = content
            .lines()
            .filter(|line| {
                let table = line
                    .trim()
                    .strip_prefix('-')
                    .map(str::trim_start)
                    .and_then(|line| line.split([' ', '\t', '#']).next());
                !matches!(table, Some("cn_dicts/tencent" | "cn_dicts/41448"))
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        content = content
            .replace("cn_dicts/", "rime_ice-dicts/cn_dicts/")
            .replace("en_dicts/", "rime_ice-dicts/en_dicts/");
    } else if name.ends_with(".lua") {
        content = content
            .replace(
                "\"convert_ar_num_to_zh\"",
                "\"rime_ice/convert_ar_num_to_zh\"",
            )
            .replace("'convert_ar_num_to_zh'", "'rime_ice/convert_ar_num_to_zh'")
            .replace("\"cold_word_drop.", "\"rime_ice/cold_word_drop/")
            .replace("'cold_word_drop.", "'rime_ice/cold_word_drop/")
            .replace("\"cold_word_drop/", "\"rime_ice/cold_word_drop/")
            .replace("'cold_word_drop/", "'rime_ice/cold_word_drop/")
            .replace("lua/cold_word_drop/", "lua/rime_ice/cold_word_drop/")
            .replace("lua/lunar.db", "lua/rime_ice/lunar.db");
    } else {
        content = content
            .replace("\"emoji.txt\"", "\"rime_ice_emoji.txt\"")
            .replace("\"others.txt\"", "\"rime_ice_others.txt\"");
    }
    fs::write(path, content).map_err(|e| e.to_string())
}

fn extract(
    archive: &Path,
    stage: &Path,
    package: &RemotePackage,
) -> Result<Vec<InstalledFile>, String> {
    let mut files = wanxiang::extract_package_with_limit(
        archive,
        stage,
        package_destination,
        adapt_source,
        MAX_EXPANDED_BYTES,
    )?;
    for name in REQUIRED {
        if !wanxiang::safe_target(stage, name)?.is_file() {
            return Err(format!("雾凇拼音发布包缺少必要文件：{name}"));
        }
    }
    for name in ["rime_ice.dict.yaml", "melt_eng.dict.yaml"] {
        let file = File::open(stage.join(name)).map_err(|e| e.to_string())?;
        let header = serde_yaml::Deserializer::from_reader(file)
            .next()
            .ok_or("词库为空")?;
        let header = serde_yaml::Value::deserialize(header).map_err(|e| e.to_string())?;
        for table in header
            .get("import_tables")
            .and_then(serde_yaml::Value::as_sequence)
            .into_iter()
            .flatten()
        {
            let table = table.as_str().ok_or("雾凇拼音词库引用无效")?;
            let path = wanxiang::safe_target(stage, &format!("{table}.dict.yaml"))?;
            if !path.is_file() {
                return Err(format!("雾凇拼音发布包缺少词库：{table}"));
            }
        }
    }
    let version = &package.version;
    let notice = format!(
        "Rime Ice / 雾凇拼音 {version}\nCopyright: iDvel and upstream contributors\n\
         License: GPL-3.0-only\nSource: {}\n\
         License text: https://github.com/iDvel/rime-ice/blob/{version}/LICENSE\n\
         KeyTao integration changes: Lua modules and their internal references,\n\
         dictionary tables, emoji data and custom phrases are namespaced.\n\
         Host settings, other input layouts, Tencent and 41448 tables are omitted.\n\
         Downloaded at runtime; not bundled with KeyTao.\n\
         User customizations and learned dictionaries are preserved during uninstall.\n",
        package.source,
    );
    fs::write(stage.join(NOTICE), notice).map_err(|e| e.to_string())?;
    let (sha256, size) = wanxiang::fingerprint(&stage.join(NOTICE))?;
    files.push(InstalledFile {
        path: NOTICE.into(),
        sha256,
        size,
    });
    Ok(files)
}

fn commit_files(
    receipt: &mut InstallReceipt,
    files: Vec<InstalledFile>,
    package: &RemotePackage,
    archive_sha256: String,
) -> Result<(), String> {
    #[cfg(windows)]
    {
        receipt.public_sources = files.iter().map(|file| file.path.clone()).collect();
    }
    let previous: BTreeMap<_, _> = read_manifest(&receipt.root)?
        .into_iter()
        .flat_map(|manifest| manifest.files)
        .map(|file| (file.path.clone(), file))
        .collect();
    let mut owned = Vec::new();
    let mut replacements = Vec::new();
    for file in files {
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
                    "已有文件 {} 与雾凇拼音冲突，已保留原文件；请先备份并移走该文件",
                    file.path
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
    let hash = wanxiang::fingerprint(&source)?.0;
    receipt.replace(MANIFEST, &source, hash)
}

pub async fn install(
    root: &Path,
    progress: impl Fn(u8, &str) + Send + Sync + 'static,
) -> Result<InstallReceipt, String> {
    let receipt = InstallReceipt::new(root)?;
    read_manifest(&receipt.root)?;
    let package = latest_package().await?;
    progress(5, "正在下载雾凇拼音…");
    let archive = receipt.work.join("download.zip");
    wanxiang::download_with_limit(&package, &archive, &progress, MAX_DOWNLOAD_BYTES).await?;
    progress(52, "正在校验并解压雾凇拼音…");
    tokio::task::spawn_blocking(move || {
        let mut receipt = receipt;
        let files = extract(&archive, &receipt.work.join("files"), &package)?;
        progress(60, "正在安装雾凇拼音…");
        commit_files(
            &mut receipt,
            files,
            &package,
            wanxiang::fingerprint(&archive)?.0,
        )?;
        Ok(receipt)
    })
    .await
    .map_err(|e| format!("雾凇拼音安装任务失败：{e}"))?
}

pub fn uninstall(root: &Path) -> Result<InstallReceipt, String> {
    let mut receipt = InstallReceipt::new(root)?;
    if let Some(manifest) = read_manifest(&receipt.root)? {
        for file in manifest.files {
            let path = wanxiang::safe_target(&receipt.root, &file.path)?;
            if path.is_file() && wanxiang::fingerprint(&path)?.0 == file.sha256 {
                receipt.move_original(&file.path)?;
            }
        }
        receipt.move_original(MANIFEST)?;
    }
    Ok(receipt)
}

#[cfg(target_os = "windows")]
#[derive(Default)]
struct PublicRollback(Option<crate::windows_public_schemas::PublicSchemaUpdate>);

#[cfg(target_os = "windows")]
impl Drop for PublicRollback {
    fn drop(&mut self) {
        if let Some(update) = self.0.take() {
            if let Err(error) = update.rollback() {
                tracing::error!(%error, "Rime Ice public source rollback failed");
            }
        }
    }
}

pub async fn manage_rime_ice(
    core: &Arc<Core>,
    installed: bool,
    host: &impl SchemeHost,
) -> Result<Status, CoreError> {
    manage_rime_ice_with(core, installed, host, |root| async move {
        let core = core.clone();
        install(&root, move |percent, message| core.emit(CoreEvent::InstallProgress(InstallProgress {
            stage: "rime-ice-download".into(), percent: u32::from(percent), message: message.into(),
        }))).await
    }).await
}

async fn manage_rime_ice_with<Install, InstallFuture>(
    core: &Arc<Core>,
    installed: bool,
    host: &impl SchemeHost,
    install_package: Install,
) -> Result<Status, CoreError>
where
    Install: FnOnce(std::path::PathBuf) -> InstallFuture,
    InstallFuture: std::future::Future<Output = Result<InstallReceipt, String>>,
{
    let result: Result<_, String> = async {
        let root = host.user_root()?;
        #[cfg(target_os = "windows")]
        let package_paths = read_manifest(&root)?.into_iter().flat_map(|manifest| manifest.files)
            .map(|file| std::path::PathBuf::from(file.path)).collect::<Vec<_>>();
        let mut receipt = if installed {
            install_package(root.clone()).await?
        } else { uninstall(&root)? };
        #[cfg(target_os = "windows")]
        let mut public_update = PublicRollback::default();
        let deployed = async {
            write_schema_list_transaction(
                &mut receipt, SCHEMA_ID, installed, &crate::addon::shared_data_dir(core, &root),
            )?;
            #[cfg(target_os = "windows")]
            let public_source_warning = {
                match crate::windows_public_schemas::update_public_schemas(|| {
                    if installed { host.publish_wanxiang_sources(&receipt.public_source_files()) }
                    else { host.remove_public_schema(SCHEMA_ID, &package_paths) }
                }) {
                    Ok(update) => {
                        public_update.0 = Some(update);
                        None
                    }
                    Err(error) => Some(crate::windows_public_schemas::public_warning(
                        "rime-ice-public-sources", &error,
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
                core.emit(CoreEvent::DeployProgress(format!("系统搜索框方案未更新：{warning}")));
            }
            if installed && !status_at(&root).deployed { return Err("雾凇拼音部署产物缺失".to_string()); }
            Ok(())
        }.await;
        if let Err(error) = deployed {
            let rollback = receipt.rollback();
            #[cfg(target_os = "windows")]
            let public_rollback = public_update.0.take().map_or(Ok(()), |update| update.rollback());
            rollback.map_err(|restore| format!("{error}；恢复词库失败：{restore}"))?;
            #[cfg(target_os = "windows")]
            public_rollback.map_err(|restore| format!("{error}；恢复公共方案失败：{restore}"))?;
            host.deploy().await.map_err(|restore| format!("{error}；已恢复文件，请重新部署：{restore}"))?;
            return Err(error);
        }
        #[cfg(target_os = "windows")]
        public_update.0.take();
        receipt.finish()?;
        if !installed {
            for extension in ["schema.yaml", "table.bin", "prism.bin", "reverse.bin"] {
                let path = root.join("build").join(format!("rime_ice.{extension}"));
                if let Err(error) = fs::remove_file(&path) {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        tracing::info!(%error, path = %path.display(), "unused Rime Ice build cache remains locked");
                    }
                }
            }
        }
        core.emit(CoreEvent::InstallProgress(InstallProgress { stage: "done".into(), percent: 100,
            message: if installed { "雾凇拼音已安装并部署" } else { "雾凇拼音已卸载，保留用户自定义和学习记录" }.into() }));
        Ok(status_at(&root))
    }.await;
    result.map_err(CoreError::Other)
}

pub fn rime_ice_status(_core: &Core, root: &Path) -> Result<Status, CoreError> {
    Ok(status_at(root))
}

#[cfg(test)]
#[path = "rime_ice_tests.rs"]
mod tests;
