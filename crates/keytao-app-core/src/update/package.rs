use super::{AppUpdateInfo, RELEASES, includes_prereleases, parse_version};
use crate::{
    Core,
    download::{download_with_progress, emit_progress},
};
use reqwest::{Client, redirect::Policy};
use semver::Version;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek},
    path::{Path, PathBuf},
    time::Duration,
};

const MAX_PACKAGE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ASSET_PAGE_BYTES: usize = 512 * 1024;

pub(crate) struct VerifiedPackage {
    version: Version,
    path: PathBuf,
    digest: [u8; 32],
}

pub(super) fn install_mode(_info: &AppUpdateInfo) -> &'static str {
    match package_suffix() {
        None => "link",
        #[cfg(windows)]
        Some(_) if super::windows::can_restart() => "restart",
        #[cfg(target_os = "linux")]
        Some(suffix)
            if linux_can_upgrade(&_info.current_version, &_info.latest_version)
                && super::linux::can_install(suffix) =>
        {
            "restart"
        }
        Some(_) => "package",
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_can_upgrade(current: &str, latest: &str) -> bool {
    let (Some(current), Some(latest)) = (parse_version(current), parse_version(latest)) else {
        return false;
    };
    // The existing DEB and RPM headers sort same-base prereleases above stable.
    current.pre.is_empty()
        || !latest.pre.is_empty()
        || (current.major, current.minor, current.patch)
            != (latest.major, latest.minor, latest.patch)
}

fn package_suffix() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64" | "x86_64") => Some("macos.dmg"),
        ("windows", "aarch64" | "x86_64") => Some("windows-x64-setup.exe"),
        ("android", "aarch64") => Some("android-arm64-release.apk"),
        ("android", "arm") => Some("android-arm-release.apk"),
        ("android", "x86_64") => Some("android-x86_64-release.apk"),
        #[cfg(target_os = "linux")]
        ("linux", arch @ ("x86_64" | "aarch64")) => super::linux::package_suffix(arch),
        _ => None,
    }
}

fn validate_version(
    current: &str,
    requested: &str,
    include_prereleases: bool,
) -> Result<Version, String> {
    let current = parse_version(current).ok_or("无法识别当前 KeyTao 版本")?;
    let requested = Version::parse(requested).map_err(|_| "升级版本号无效")?;
    if requested <= current || (!include_prereleases && !requested.pre.is_empty()) {
        return Err("升级版本不适用于当前 KeyTao".into());
    }
    Ok(requested)
}

fn asset_digest(page: &str, tag: &str, filename: &str) -> Option<[u8; 32]> {
    // shortcut: read GitHub's asset markup until releases provide a checksum file.
    let href = format!("href=\"/xkinput/keytao-app/releases/download/{tag}/{filename}\"");
    let label = format!("aria-label=\"Copy to clipboard digest for {filename}\"");
    let mut items = page.split("<li ").skip(1).filter_map(|item| {
        let item = item.split_once("</li>")?.0;
        item.contains(&href).then_some(item)
    });
    let item = items.next()?;
    if items.next().is_some() {
        return None;
    }
    let digest = item.split("<clipboard-copy ").skip(1).find_map(|copy| {
        let attributes = copy.split_once('>')?.0;
        if !attributes.contains(&label) {
            return None;
        }
        attributes
            .split_once(" value=\"sha256:")?
            .1
            .split_once('"')
            .map(|(value, _)| value)
    })?;
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut bytes = [0; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&digest[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(bytes)
}

fn client() -> Result<Client, String> {
    Client::builder()
        .user_agent("keytao-app-updater")
        .connect_timeout(Duration::from_secs(8))
        .read_timeout(Duration::from_secs(30))
        .https_only(true)
        .redirect(Policy::custom(|attempt| {
            let url = attempt.url();
            if attempt.previous().len() < 5
                && url.scheme() == "https"
                && url.port_or_known_default() == Some(443)
                && matches!(
                    url.host_str(),
                    Some(
                        "github.com"
                            | "release-assets.githubusercontent.com"
                            | "objects.githubusercontent.com"
                    )
                )
            {
                attempt.follow()
            } else {
                attempt.error("Unexpected KeyTao update redirect")
            }
        }))
        .build()
        .map_err(|error| error.to_string())
}

async fn fetch_digest(
    client: &Client,
    tag: &str,
    filename: &str,
) -> Result<Option<[u8; 32]>, String> {
    let mut response = client
        .get(format!("{RELEASES}/expanded_assets/{tag}"))
        .timeout(Duration::from_secs(12))
        .send()
        .await
        .map_err(|error| error.without_url().to_string())?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    response = response
        .error_for_status()
        .map_err(|error| error.without_url().to_string())?;
    let mut page = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| error.without_url().to_string())?
    {
        if page.len() + chunk.len() > MAX_ASSET_PAGE_BYTES {
            return Err("GitHub 发布资产列表超过大小限制".into());
        }
        page.extend_from_slice(&chunk);
    }
    Ok(asset_digest(&String::from_utf8_lossy(&page), tag, filename))
}

fn verified_file(path: &Path, digest: [u8; 32]) -> Result<File, String> {
    if !fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file()) {
        return Err("升级包不存在或文件类型不正确".into());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Keep the same verified file in place through ShellExecute.
        options.share_mode(1);
    }
    let mut file = options.open(path).map_err(|error| error.to_string())?;
    verify_contents(&mut file, digest)?;
    Ok(file)
}

fn verify_contents(file: &mut File, digest: [u8; 32]) -> Result<(), String> {
    file.rewind().map_err(|error| error.to_string())?;
    let mut hash = Sha256::new();
    let size = std::io::copy(&mut file.take(MAX_PACKAGE_BYTES + 1), &mut hash)
        .map_err(|error| error.to_string())?;
    if size == 0 || size > MAX_PACKAGE_BYTES || <[u8; 32]>::from(hash.finalize()) != digest {
        return Err("升级包 SHA-256 校验失败，请重新下载".into());
    }
    Ok(())
}

fn persist_verified_package(
    mut temp: tempfile::NamedTempFile,
    target: &Path,
    digest: [u8; 32],
) -> Result<(), String> {
    verify_contents(temp.as_file_mut(), digest)?;
    temp.as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    temp.persist(target).map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn download_app_update(core: &Core, requested: &str) -> Result<bool, String> {
    let mut package = core.app_update_package.lock().await;
    let version = validate_version(&core.env().app_version, requested, includes_prereleases(core))?;
    let Some(suffix) = tokio::task::spawn_blocking(package_suffix)
        .await
        .map_err(|error| error.to_string())?
    else {
        return Ok(false);
    };
    let client = client()?;
    let tag = format!("v{version}");
    let filename = format!("keytao-app-{version}-{suffix}");
    let Some(digest) = fetch_digest(&client, &tag, &filename).await? else {
        return Ok(false);
    };
    let cache = core.env().cache_dir.join("app-updates");
    fs::create_dir_all(&cache).map_err(|error| error.to_string())?;
    if !fs::symlink_metadata(&cache).is_ok_and(|meta| meta.is_dir()) {
        return Err("升级包缓存目录无效".into());
    }
    let path = cache.join(&filename);
    let cached = {
        let path = path.clone();
        tokio::task::spawn_blocking(move || verified_file(&path, digest).is_ok())
            .await
            .map_err(|error| error.to_string())?
    };
    if !cached {
        let (_cancel_sender, mut cancel) = tokio::sync::watch::channel(0);
        let url = format!("{RELEASES}/download/{tag}/{filename}");
        let mut last_percent = 0;
        emit_progress(core, "app_update_downloading", 0, "正在下载升级包");
        let temp = download_with_progress(
            core,
            client.get(url),
            &mut cancel,
            None,
            Some(MAX_PACKAGE_BYTES),
            |received, total| {
                if total != 0 {
                    let percent = (received.saturating_mul(100) / total).min(100) as u32;
                    if percent != last_percent {
                        last_percent = percent;
                        emit_progress(core, "app_update_downloading", percent, "正在下载升级包");
                    }
                }
            },
        )
        .await?;
        emit_progress(core, "app_update_verifying", 100, "正在校验升级包");
        let target = path.clone();
        tokio::task::spawn_blocking(move || persist_verified_package(temp, &target, digest))
            .await
            .map_err(|error| error.to_string())??;
    }
    *package = Some(VerifiedPackage {
        version,
        path,
        digest,
    });
    Ok(true)
}

pub async fn open_app_update_package(
    core: &Core,
    requested: &str,
    restart: bool,
) -> Result<String, String> {
    let package = core.app_update_package.lock().await;
    let version = validate_version(&core.env().app_version, requested, includes_prereleases(core))?;
    #[cfg(target_os = "linux")]
    if restart && !linux_can_upgrade(&core.env().app_version, requested) {
        return Err("系统包版本排序需要手动安装此升级包".into());
    }
    let package = package
        .as_ref()
        .filter(|package| package.version == version)
        .ok_or("请先下载并校验此版本的升级包")?;
    let path = package.path.clone();
    let digest = package.digest;
    tokio::task::spawn_blocking(move || {
        let _file = verified_file(&path, digest)?;
        #[cfg(windows)]
        super::windows::open_installer(&path, restart)?;
        #[cfg(target_os = "linux")]
        if restart {
            let suffix = package_suffix().ok_or("当前 Linux 安装来源已变化，请重新检查更新")?;
            super::linux::install_update(&path, suffix, digest, _file)?;
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        if restart {
            return Err("当前平台需要在系统安装程序中升级".into());
        }
        Ok(path.to_string_lossy().into_owned())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_must_belong_to_the_exact_published_asset() {
        let tag = "v1.2.16";
        let filename = "keytao-app-1.2.16-macos.dmg";
        let checksum = "ab".repeat(32);
        let item = format!(
            "<li class=\"asset\"><a href=\"/xkinput/keytao-app/releases/download/{tag}/{filename}\"></a><clipboard-copy aria-label=\"Copy to clipboard digest for {filename}\" value=\"sha256:{checksum}\"></clipboard-copy></li>"
        );
        assert_eq!(asset_digest(&item, tag, filename), Some([0xab; 32]));
        assert_eq!(
            asset_digest(
                &item.replace("digest for keytao-app", "digest for another"),
                tag,
                filename
            ),
            None
        );
        assert_eq!(
            asset_digest(
                &item.replace(checksum.as_str(), "not-a-digest"),
                tag,
                filename
            ),
            None
        );
        assert_eq!(asset_digest(&item.repeat(2), tag, filename), None);
    }

    #[test]
    fn verifies_downloaded_bytes_and_rechecks_before_opening() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("installer");
        fs::write(&path, b"previous").unwrap();
        let digest = Sha256::digest(b"installer").into();
        let mut temp = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
        temp.write_all(b"bad download").unwrap();
        assert!(persist_verified_package(temp, &path, digest).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"previous");

        let mut temp = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
        temp.write_all(b"installer").unwrap();
        persist_verified_package(temp, &path, digest).unwrap();
        assert!(verified_file(&path, digest).is_ok());
        fs::write(&path, b"tampered!").unwrap();
        assert!(verified_file(&path, digest).is_err());
    }

    #[test]
    fn permits_only_newer_versions_on_the_selected_channel() {
        assert!(validate_version("1.2.16-alpha.5", "1.2.16", false).is_ok());
        assert!(validate_version("1.2.16", "1.2.17-alpha.1", true).is_ok());
        assert!(validate_version("1.2.16", "1.2.17-alpha.1", false).is_err());
        assert!(validate_version("1.2.16-alpha.5", "1.2.17-alpha.1", false).is_err());
        assert!(validate_version("1.2.16", "1.2.16-alpha.5", true).is_err());
        assert!(validate_version("1.2.16", "1.2.16", true).is_err());
        assert!(validate_version("1.2.16", "../../file", true).is_err());
    }

    #[test]
    fn linux_same_base_prerelease_to_stable_uses_manual_install() {
        assert!(!linux_can_upgrade("1.2.16-alpha.5", "1.2.16"));
        assert!(linux_can_upgrade("1.2.16-alpha.5", "1.2.17"));
        assert!(linux_can_upgrade("1.2.16-alpha.5", "1.2.16-alpha.6"));
        assert!(linux_can_upgrade("1.2.16", "1.2.17"));
    }
}
