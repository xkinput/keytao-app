//! Runtime-downloaded Rime Ice sources, isolated in its own Rime directory.

#[cfg(test)]
use crate::addon::write_schema_list_transaction;
use crate::{
    Core, CoreError, CoreEvent,
    downloaded_addon::Addon,
    events::InstallProgress,
    generic_rime::{self, DEFAULT_MAX_EXPANDED_BYTES},
    scheme_host::SchemeHost,
    wanxiang::{self, InstallReceipt, InstalledFile, RemotePackage, Status},
};
use std::{path::Path, sync::Arc};

pub const SCHEMA_ID: &str = "rime_ice";
const RELEASES_ATOM: &str = "https://github.com/iDvel/rime-ice/releases.atom";
const RELEASES: &str = "https://api.github.com/repos/iDvel/rime-ice/releases?per_page=20";
const MANIFEST: &str = ".keytao-rime-ice.json";
const NOTICE: &str = "rime_ice-NOTICE.txt";
const REQUIRED: &[&str] = &["rime_ice.schema.yaml"];

static ADDON: Addon = Addon {
    id: SCHEMA_ID,
    dictionary: SCHEMA_ID,
    name: "雾凇拼音",
    manifest: MANIFEST,
    required: REQUIRED,
    extra_archives: &[],
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

pub(crate) fn installable(name: &str) -> bool {
    generic_rime::is_installable_path(name)
}

pub fn status_at(root: &Path) -> Status {
    ADDON.status_at(root)
}

fn extract(
    archive: &Path,
    stage: &Path,
    package: &RemotePackage,
) -> Result<Vec<InstalledFile>, String> {
    let mut files = generic_rime::extract_archive(
        archive,
        stage,
        Some(SCHEMA_ID),
        DEFAULT_MAX_EXPANDED_BYTES,
    )?;
    let version = &package.version;
    let notice = format!(
        "Rime Ice / 雾凇拼音 {version}\nCopyright: iDvel and upstream contributors\n\
         License: GPL-3.0-only\nSource: {}\n\
         License text: https://github.com/iDvel/rime-ice/blob/{version}/LICENSE\n\
         Downloaded at runtime into its own isolated Rime directory; not bundled with KeyTao.\n\
         User customizations and learned dictionaries are preserved during uninstall.\n",
        package.source,
    );
    generic_rime::append_notice(stage, &mut files, NOTICE, &notice)?;
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
    manage_rime_ice_with(core, installed, host, |receipt| async move {
        let core = core.clone();
        ADDON
            .install_receipt(receipt, latest_package(), move |percent, message| {
                core.emit(CoreEvent::InstallProgress(InstallProgress {
                    stage: "rime-ice-download".into(),
                    percent: u32::from(percent),
                    message: message.into(),
                }))
            })
            .await
    })
    .await
}

async fn manage_rime_ice_with<Install, InstallFuture>(
    core: &Arc<Core>,
    installed: bool,
    host: &impl SchemeHost,
    install_package: Install,
) -> Result<Status, CoreError>
where
    Install: FnOnce(InstallReceipt) -> InstallFuture,
    InstallFuture: std::future::Future<Output = Result<InstallReceipt, String>>,
{
    ADDON
        .manage_interactive(core, installed, host, install_package)
        .await
}

pub(crate) async fn reinstall_store_only(core: &Core, root: &Path) -> Result<(), String> {
    ADDON.reinstall_store_only(core, root, latest_package()).await
}

pub fn rime_ice_status(core: &Core, root: &Path) -> Result<Status, CoreError> {
    ADDON.status(core, root)
}

#[cfg(test)]
#[path = "rime_ice_tests.rs"]
mod tests;
