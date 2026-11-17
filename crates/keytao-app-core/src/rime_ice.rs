//! Runtime-downloaded Rime Ice sources, isolated from other installed schemes.

use crate::{
    Core, CoreError, CoreEvent,
    downloaded_addon::Addon,
    events::InstallProgress,
    scheme_host::SchemeHost,
    wanxiang::{self, InstallReceipt, InstalledFile, RemotePackage, Status},
};
use serde::Deserialize;
#[cfg(test)]
use crate::addon::write_schema_list_transaction;
use std::{
    fs::{self, File},
    path::Path,
    sync::Arc,
};

pub const SCHEMA_ID: &str = "rime_ice";
const RELEASES_ATOM: &str = "https://github.com/iDvel/rime-ice/releases.atom";
const RELEASES: &str = "https://api.github.com/repos/iDvel/rime-ice/releases?per_page=20";
const MANIFEST: &str = ".keytao-rime-ice.json";
const NOTICE: &str = "rime_ice-NOTICE.txt";
const LICENSE: &str = "rime_ice-LICENSE.txt";
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

static ADDON: Addon = Addon {
    id: SCHEMA_ID,
    name: "雾凇拼音",
    manifest: MANIFEST,
    required: REQUIRED,
    installable,
    extract,
};

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

pub(crate) fn installable(name: &str) -> bool {
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

pub fn status_at(root: &Path) -> Status {
    ADDON.status_at(root)
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

#[cfg(test)]
fn commit_files(
    receipt: &mut InstallReceipt,
    files: Vec<InstalledFile>,
    package: &RemotePackage,
    archive_sha256: String,
) -> Result<(), String> {
    ADDON.commit_files(receipt, files, package, archive_sha256)
}

pub async fn install(
    root: &Path,
    progress: impl Fn(u8, &str) + Send + Sync + 'static,
) -> Result<InstallReceipt, String> {
    ADDON.install(root, latest_package(), progress).await
}

pub fn uninstall(root: &Path) -> Result<InstallReceipt, String> {
    ADDON.uninstall(root)
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
    ADDON.manage(core, installed, host, install_package).await
}

pub fn rime_ice_status(_core: &Core, root: &Path) -> Result<Status, CoreError> {
    Ok(status_at(root))
}

#[cfg(test)]
#[path = "rime_ice_tests.rs"]
mod tests;
