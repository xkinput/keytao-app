//! Downloadable Frost Pinyin releases, discovered without GitHub's API.

use crate::{
    Core, CoreError, CoreEvent,
    events::InstallProgress,
    pinyin_addons::FROST,
    scheme_host::SchemeHost,
    wanxiang::{InstallReceipt, RemotePackage, Status},
};
use std::{path::Path, sync::Arc, time::Duration};

pub const SCHEMA_ID: &str = "rime_frost";

#[cfg(test)]
pub(crate) fn package_from_location(location: &str) -> Result<RemotePackage, String> {
    release_package_from_location(location, "gaboolic/rime-frost", "rime-frost-schemas.zip")
}

pub(crate) fn release_package_from_location(
    location: &str,
    repo: &str,
    asset: &str,
) -> Result<RemotePackage, String> {
    let latest = format!("https://github.com/{repo}/releases/latest");
    let prefix = format!("/{repo}/releases/tag/");
    let url = reqwest::Url::parse(&latest)
        .and_then(|base| base.join(location))
        .map_err(|e| format!("方案发布地址无效：{e}"))?;
    let tag = url
        .path()
        .strip_prefix(&prefix)
        .filter(|tag| {
            !tag.is_empty()
                && tag
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        })
        .ok_or("方案发布地址缺少有效版本号")?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("方案发布地址无效".into());
    }
    Ok(RemotePackage {
        version: tag.into(),
        source: url.as_str().into(),
        download_url: format!("https://github.com/{repo}/releases/download/{tag}/{asset}"),
        download_bytes: None,
    })
}

async fn latest_package() -> Result<RemotePackage, String> {
    latest_release("gaboolic/rime-frost", "rime-frost-schemas.zip").await
}

pub(crate) async fn latest_release(repo: &str, asset: &str) -> Result<RemotePackage, String> {
    let response = crate::wanxiang::addon_client(Duration::from_secs(30), false)?
        .get(format!("https://github.com/{repo}/releases/latest"))
        .send()
        .await
        .map_err(|e| format!("方案更新查询失败：{e}"))?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    if !response.status().is_redirection() {
        return Err("方案发布地址未返回重定向".into());
    }
    release_package_from_location(
        response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or("方案发布地址缺少重定向位置")?,
        repo,
        asset,
    )
}

pub async fn latest_version() -> Result<String, String> {
    Ok(latest_package().await?.version)
}

pub fn status_at(root: &Path) -> Status {
    FROST.status_at(root)
}

pub(crate) async fn reinstall_store_only(core: &Core, root: &Path) -> Result<(), String> {
    FROST.reinstall_store_only(core, root, latest_package()).await
}

pub fn rime_frost_status(core: &Core, root: &Path) -> Result<Status, CoreError> {
    FROST.status(core, root)
}

pub async fn install(
    root: &Path,
    progress: impl Fn(u8, &str) + Send + Sync + 'static,
) -> Result<InstallReceipt, String> {
    FROST.install(root, latest_package(), progress).await
}

pub fn uninstall(root: &Path) -> Result<InstallReceipt, String> {
    FROST.uninstall(root)
}

pub async fn manage_rime_frost(
    core: &Arc<Core>,
    installed: bool,
    host: &impl SchemeHost,
) -> Result<Status, CoreError> {
    FROST
        .manage_interactive(core, installed, host, |receipt| async move {
            let core = core.clone();
            FROST
                .install_receipt(receipt, latest_package(), move |percent, message| {
                    core.emit(CoreEvent::InstallProgress(InstallProgress {
                        stage: "rime-frost-download".into(),
                        percent: u32::from(percent),
                        message: message.into(),
                    }))
                })
                .await
        })
        .await
}
