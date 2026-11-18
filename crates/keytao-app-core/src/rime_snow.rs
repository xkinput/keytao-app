//! Downloadable Snow Jiandao, using upstream release redirects instead of the API.

use crate::{
    Core, CoreError, CoreEvent,
    events::InstallProgress,
    pinyin_addons::SNOW,
    scheme_host::SchemeHost,
    wanxiang::{InstallReceipt, RemotePackage, Status},
};
use std::{path::Path, sync::Arc};

pub const SCHEMA_ID: &str = "snow_jiandao";
const REPO: &str = "rimeinn/rime-snow-pinyin";

#[cfg(test)]
pub(crate) fn package_from_location(location: &str) -> Result<RemotePackage, String> {
    crate::rime_frost::release_package_from_location(location, REPO, "snow-pinyin.zip")
}

async fn latest_package() -> Result<RemotePackage, String> {
    crate::rime_frost::latest_release(REPO, "snow-pinyin.zip").await
}

pub async fn latest_version() -> Result<String, String> {
    Ok(latest_package().await?.version)
}

pub fn status_at(root: &Path) -> Status {
    SNOW.status_at(root)
}

pub fn rime_snow_status(core: &Core, root: &Path) -> Result<Status, CoreError> {
    SNOW.status(core, root)
}

pub async fn install(
    root: &Path,
    progress: impl Fn(u8, &str) + Send + Sync + 'static,
) -> Result<InstallReceipt, String> {
    SNOW.install(root, latest_package(), progress).await
}

pub fn uninstall(root: &Path) -> Result<InstallReceipt, String> {
    SNOW.uninstall(root)
}

pub async fn manage_rime_snow(
    core: &Arc<Core>,
    installed: bool,
    host: &impl SchemeHost,
) -> Result<Status, CoreError> {
    SNOW.manage(core, installed, host, |receipt| async move {
        let core = core.clone();
        SNOW.install_receipt(receipt, latest_package(), move |percent, message| {
            core.emit(CoreEvent::InstallProgress(InstallProgress {
                stage: "rime-snow-download".into(),
                percent: u32::from(percent),
                message: message.into(),
            }))
        })
        .await
    })
    .await
}
