//! Downloadable Mint Pinyin from the upstream main-branch archive.

use crate::{
    Core, CoreError, CoreEvent,
    events::InstallProgress,
    pinyin_addons::MINT,
    scheme_host::SchemeHost,
    wanxiang::{self, InstallReceipt, RemotePackage, Status},
};
use std::{path::Path, sync::Arc};

pub const SCHEMA_ID: &str = "rime_mint";
const COMMITS_ATOM: &str = "https://github.com/Mintimate/oh-my-rime/commits/main.atom";

pub(crate) fn package_from_atom(feed: &str) -> Result<RemotePackage, String> {
    let entry = feed
        .split_once("<entry>")
        .and_then(|(_, entry)| entry.split_once("</entry>"))
        .map(|(entry, _)| entry)
        .ok_or("薄荷拼音提交订阅缺少最新提交")?;
    let sha = entry
        .split_once("<id>")
        .and_then(|(_, id)| id.split_once("</id>"))
        .and_then(|(id, _)| id.trim().strip_prefix("tag:github.com,2008:Grit::Commit/"))
        .filter(|sha| sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("薄荷拼音提交订阅中的提交号无效")?;
    let date = entry
        .split_once("<updated>")
        .and_then(|(_, date)| date.split_once("</updated>"))
        .and_then(|(date, _)| date.trim().get(..10))
        .filter(|date| {
            date.bytes().enumerate().all(|(i, b)| match i {
                4 | 7 => b == b'-',
                _ => b.is_ascii_digit(),
            }) && date[5..7]
                .parse::<u8>()
                .is_ok_and(|m| (1..=12).contains(&m))
                && date[8..10]
                    .parse::<u8>()
                    .is_ok_and(|d| (1..=31).contains(&d))
        })
        .ok_or("薄荷拼音提交订阅中的日期无效")?;
    Ok(RemotePackage {
        version: format!("{} ({date})", &sha[..12]),
        source: format!("https://github.com/Mintimate/oh-my-rime/commit/{sha}"),
        download_url: "https://codeload.github.com/Mintimate/oh-my-rime/zip/refs/heads/main".into(),
        download_bytes: None,
    })
}

async fn latest_package() -> Result<RemotePackage, String> {
    package_from_atom(&wanxiang::github_text(COMMITS_ATOM).await?)
}

pub async fn latest_version() -> Result<String, String> {
    Ok(latest_package().await?.version)
}

pub fn status_at(root: &Path) -> Status {
    MINT.status_at(root)
}

pub fn rime_mint_status(_core: &Core, root: &Path) -> Result<Status, CoreError> {
    Ok(status_at(root))
}

pub async fn install(
    root: &Path,
    progress: impl Fn(u8, &str) + Send + Sync + 'static,
) -> Result<InstallReceipt, String> {
    MINT.install(root, latest_package(), progress).await
}

pub fn uninstall(root: &Path) -> Result<InstallReceipt, String> {
    MINT.uninstall(root)
}

pub async fn manage_rime_mint(
    core: &Arc<Core>,
    installed: bool,
    host: &impl SchemeHost,
) -> Result<Status, CoreError> {
    MINT.manage(core, installed, host, |root| async move {
        let core = core.clone();
        install(&root, move |percent, message| {
            core.emit(CoreEvent::InstallProgress(InstallProgress {
                stage: "rime-mint-download".into(),
                percent: u32::from(percent),
                message: message.into(),
            }))
        })
        .await
    })
    .await
}
