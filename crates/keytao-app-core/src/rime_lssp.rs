//! Downloadable LSSP Triple Pinyin with package-owned dictionary dependencies.

use crate::{
    Core, CoreError, CoreEvent,
    events::InstallProgress,
    pinyin_addons::LSSP,
    scheme_host::SchemeHost,
    wanxiang::{self, InstallReceipt, RemotePackage, Status},
};
use std::{path::Path, sync::Arc};

pub const SCHEMA_ID: &str = "triple_pinyin_lssp";
const REPO: &str = "arsenali/rime-triple-pinyin-lssp";

pub(crate) fn package_from_atom(feed: &str) -> Result<RemotePackage, String> {
    crate::rime_mint::branch_package_from_atom(feed, REPO)
}

async fn latest_package() -> Result<RemotePackage, String> {
    package_from_atom(
        &wanxiang::github_text(&format!("https://github.com/{REPO}/commits/main.atom")).await?,
    )
}

pub async fn latest_version() -> Result<String, String> {
    Ok(latest_package().await?.version)
}

pub fn status_at(root: &Path) -> Status {
    LSSP.status_at(root)
}

pub(crate) async fn reinstall_store_only(core: &Core, root: &Path) -> Result<(), String> {
    LSSP.reinstall_store_only(core, root, latest_package()).await
}

pub fn rime_lssp_status(core: &Core, root: &Path) -> Result<Status, CoreError> {
    LSSP.status(core, root)
}

pub async fn install(
    root: &Path,
    progress: impl Fn(u8, &str) + Send + Sync + 'static,
) -> Result<InstallReceipt, String> {
    LSSP.install(root, latest_package(), progress).await
}

pub fn uninstall(root: &Path) -> Result<InstallReceipt, String> {
    LSSP.uninstall(root)
}

pub async fn manage_rime_lssp(
    core: &Arc<Core>,
    installed: bool,
    host: &impl SchemeHost,
) -> Result<Status, CoreError> {
    LSSP.manage_interactive(core, installed, host, |receipt| async move {
        let core = core.clone();
        LSSP.install_receipt(receipt, latest_package(), move |percent, message| {
            core.emit(CoreEvent::InstallProgress(InstallProgress {
                stage: "rime-lssp-download".into(),
                percent: u32::from(percent),
                message: message.into(),
            }))
        })
        .await
    })
    .await
}
