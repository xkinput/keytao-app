use crate::{
    Core, CoreError,
    download::{DOWNLOAD_CANCELLED, download_to_temp_with, emit_progress},
};
use keytao_core::libjd::{self, LibjdManifest, LibjdPlatform};
use reqwest::{Url, redirect::Policy};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufReader, Read, Seek},
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const RELEASE_LATEST: &str = "https://github.com/hronro/ime-jd/releases/latest";
const RELEASE_TAG_PREFIX: &str = "/hronro/ime-jd/releases/tag/";
const RELEASE_DOWNLOAD: &str = "https://github.com/hronro/ime-jd/releases/download/";
const RELEASE_CDN_PREFIX: &str = "/github-production-release-asset/612575122/";
const MAX_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;
const XZ_MEMORY_LIMIT_KIB: u32 = 72 * 1024; // Upstream xz -e9 uses a 64 MiB dictionary.

pub struct LibjdStatus {
    pub available: bool,
    pub installed_version: Option<String>,
}

fn supported_platform() -> Option<LibjdPlatform> {
    #[cfg(windows)]
    return crate::ime_status::windows::libjd_server_platform();
    #[cfg(target_os = "android")]
    if !keytao_core::android_libjd_enabled() {
        return None;
    }
    #[cfg(not(windows))]
    libjd::platform()
}

pub fn libjd_status(core: &Core) -> LibjdStatus {
    let platform = supported_platform();
    LibjdStatus {
        available: platform.is_some(),
        installed_version: platform
            .and_then(|platform| platform.installed_tag(&core.env().data_dir)),
    }
}

pub struct LibjdRelease {
    platform: LibjdPlatform,
    tag: String,
    asset: Asset,
}

impl LibjdRelease {
    pub fn version(&self) -> &str {
        &self.tag
    }

    pub fn size(&self) -> u32 {
        self.asset.size as u32
    }

    pub fn source_url(&self) -> &str {
        &self.asset.browser_download_url
    }
}

struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
}

fn allowed_asset_redirect(from: &Url, to: &Url) -> bool {
    from.as_str().starts_with(RELEASE_DOWNLOAD)
        && to.scheme() == "https"
        && to.host_str() == Some("release-assets.githubusercontent.com")
        && to.port().is_none()
        && to.username().is_empty()
        && to.password().is_none()
        && to.path().starts_with(RELEASE_CDN_PREFIX)
}

fn format_release_request_error(error: reqwest::Error) -> CoreError {
    let reason = if error.is_timeout() {
        "连接 GitHub 超时".to_owned()
    } else if error.is_connect() {
        "无法连接 GitHub，请检查网络".to_owned()
    } else if error.is_redirect() {
        "GitHub 重定向异常".to_owned()
    } else {
        error.without_url().to_string()
    };
    CoreError::Other(format!("查询 ime-jd 版本失败：{reason}"))
}

fn release_from_location(
    location: &str,
    platform: LibjdPlatform,
) -> Result<LibjdRelease, CoreError> {
    let url = Url::parse(RELEASE_LATEST)
        .and_then(|base| base.join(location))
        .map_err(|error| CoreError::Other(format!("查询 ime-jd 版本失败：发布地址无效（{error}）")))?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(CoreError::Other("查询 ime-jd 版本失败：发布地址无效".into()));
    }
    let tag = url
        .path()
        .strip_prefix(RELEASE_TAG_PREFIX)
        .filter(|tag| libjd::valid_tag(tag))
        .ok_or_else(|| CoreError::Other("ime-jd 版本号无效".into()))?
        .to_owned();
    let name = platform.asset_name(&tag);
    let browser_download_url = format!("{RELEASE_DOWNLOAD}{tag}/{name}");
    Ok(LibjdRelease {
        platform,
        tag,
        asset: Asset {
            name,
            browser_download_url,
            size: 0,
        },
    })
}

async fn latest_release_at(
    client: &reqwest::Client,
    url: &str,
    platform: LibjdPlatform,
) -> Result<LibjdRelease, CoreError> {
    let response = client
        .get(url)
        .header(reqwest::header::CACHE_CONTROL, "no-cache")
        .send()
        .await
        .map_err(format_release_request_error)?;
    let status = response.status();
    if status.is_redirection() {
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| CoreError::Other("查询 ime-jd 版本失败：缺少重定向地址".into()))?;
        return release_from_location(location, platform);
    }
    let detail = if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        format!("GitHub 请求过于频繁（HTTP {}），请稍后再试", status.as_u16())
    } else if status == reqwest::StatusCode::NOT_FOUND {
        "未找到已发布的版本（HTTP 404）".to_owned()
    } else if status.is_server_error() {
        format!("GitHub 服务异常（HTTP {}）", status.as_u16())
    } else {
        format!("发布地址未返回重定向（HTTP {status}）")
    };
    Err(CoreError::Other(format!("查询 ime-jd 版本失败：{detail}")))
}

fn compute_digest(file: &mut File) -> Result<String, CoreError> {
    let mut hash = Sha256::new();
    io::copy(file, &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}

fn write_library(input: impl Read, size: u64, target: &Path) -> Result<(), CoreError> {
    if size == 0 || size > MAX_ARCHIVE_BYTES {
        return Err(CoreError::Other("ime-jd 动态库大小无效".into()));
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)?;
    if io::copy(&mut input.take(size + 1), &mut output)? != size {
        return Err(CoreError::Other("ime-jd 动态库大小不符".into()));
    }
    output.sync_all()?;
    Ok(())
}

fn extract_library(file: File, target: &Path, platform: LibjdPlatform) -> Result<(), CoreError> {
    if platform.archive_suffix == "zip" {
        let mut archive = zip::ZipArchive::new(BufReader::new(file))
            .map_err(|error| CoreError::Other(format!("解压 ime-jd 失败: {error}")))?;
        let entry = archive
            .by_name(platform.archive_entry)
            .map_err(|error| CoreError::Other(format!("读取 ime-jd 动态库失败: {error}")))?;
        if entry.is_file() {
            let size = entry.size();
            return write_library(entry, size, target);
        }
    } else {
        let reader =
            lzma_rust2::XzReader::new_mem_limit(BufReader::new(file), false, XZ_MEMORY_LIMIT_KIB);
        let mut archive = tar::Archive::new(reader.take(2 * MAX_ARCHIVE_BYTES));
        for entry in archive.entries()? {
            let entry = entry?;
            if entry.path()?.as_ref() == Path::new(platform.archive_entry)
                && entry.header().entry_type().is_file()
            {
                let size = entry.size();
                return write_library(entry, size, target);
            }
        }
    }
    Err(CoreError::Other("安装包缺少 ime-jd 动态库".into()))
}

fn latest_redirect_client(core: &Core) -> Result<reqwest::Client, CoreError> {
    reqwest::Client::builder()
        .user_agent(format!("keytao-app/{}", core.env().app_version))
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(30))
        .https_only(true)
        .redirect(Policy::none())
        .build()
        .map_err(|error| CoreError::Network(error.without_url().to_string()))
}

fn release_client(core: &Core) -> Result<reqwest::Client, CoreError> {
    reqwest::Client::builder()
        .user_agent(format!("keytao-app/{}", core.env().app_version))
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(90))
        .https_only(true)
        .redirect(Policy::custom(|attempt| {
            if attempt.previous().len() == 1
                && allowed_asset_redirect(&attempt.previous()[0], attempt.url())
            {
                attempt.follow()
            } else {
                attempt.error("ime-jd release redirected outside its GitHub repository")
            }
        }))
        .build()
        .map_err(|error| CoreError::Network(error.without_url().to_string()))
}

pub async fn libjd_release(core: &Core) -> Result<LibjdRelease, CoreError> {
    let platform = supported_platform()
        .ok_or_else(|| CoreError::Unsupported("当前平台不支持 ime-jd".into()))?;
    let mut cancel = core.download_cancel.subscribe();
    cancel.borrow_and_update();
    let client = latest_redirect_client(core)?;
    tokio::select! {
        biased;
        _ = cancel.changed() => Err(CoreError::Other(DOWNLOAD_CANCELLED.into())),
        release = latest_release_at(&client, RELEASE_LATEST, platform) => release,
    }
}

pub async fn install_or_update_libjd(
    core: &Core,
    release: LibjdRelease,
) -> Result<LibjdStatus, CoreError> {
    let LibjdRelease {
        platform,
        tag,
        asset,
    } = release;
    if supported_platform() != Some(platform) {
        return Err(CoreError::Unsupported("当前平台不支持 ime-jd".into()));
    }
    let data = &core.env().data_dir;
    let root = libjd::engine_root(data).ok_or_else(|| CoreError::Other("引擎目录无效".into()))?;
    fs::create_dir_all(&root)?;
    let lock_path = root.join(format!(".install-{}.lock", platform.target));
    if fs::symlink_metadata(&lock_path).is_ok_and(|metadata| !metadata.is_file()) {
        return Err(CoreError::Other("ime-jd 安装锁无效".into()));
    }
    let install_lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    install_lock
        .try_lock()
        .map_err(|error| CoreError::Other(format!("无法取得 ime-jd 安装锁: {error}")))?;

    let mut cancel = core.download_cancel.subscribe();
    let client = release_client(core)?;
    let download = download_to_temp_with(
        core,
        client.get(asset.browser_download_url),
        &mut cancel,
        (asset.size > 0).then_some(asset.size),
    )
    .await
    .map_err(CoreError::Other)?;
    emit_progress(core, "extracting", 60, "正在校验 ime-jd");

    let destination = platform
        .library_path(data, &tag)
        .ok_or_else(|| CoreError::Other("ime-jd 安装目录无效".into()))?;
    let directory = destination.parent().unwrap();
    fs::create_dir_all(directory.parent().unwrap())?;
    let stage = tempfile::Builder::new()
        .prefix(".libjd-")
        .tempdir_in(directory.parent().unwrap())?;
    let stage_tag = tag.clone();
    let stage = tokio::task::spawn_blocking(move || -> Result<_, CoreError> {
        let mut archive = download.reopen()?;
        let archive_len = archive.metadata()?.len();
        if archive_len == 0 || archive_len > MAX_ARCHIVE_BYTES {
            return Err(CoreError::Other("ime-jd 安装包大小无效".into()));
        }
        let sha256 = compute_digest(&mut archive)?;
        archive.rewind()?;
        let library = stage.path().join(platform.library);
        extract_library(archive, &library, platform)?;
        let abi_layout = crate::validate_libjd_library(&library).map_err(|error| {
            tracing::warn!(%error, "ime-jd library validation failed");
            CoreError::Other("ime-jd 动态库加载或 ABI 校验失败".into())
        })?;
        let manifest = LibjdManifest {
            tag: stage_tag,
            asset: asset.name,
            sha256,
            abi_layout,
            installed_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };
        crate::scheme_files::write_file_atomic(
            &stage.path().join("manifest.json"),
            &serde_json::to_vec(&manifest)?,
        )?;
        Ok(stage)
    })
    .await
    .map_err(|error| CoreError::Other(error.to_string()))??;
    if cancel.has_changed().unwrap_or(true) {
        return Err(CoreError::Other(DOWNLOAD_CANCELLED.into()));
    }
    publish(&root, &tag, &destination, stage.path(), platform)?;
    emit_progress(core, "done", 100, "ime-jd 已安装");
    Ok(libjd_status(core))
}

fn publish(
    root: &Path,
    tag: &str,
    library: &Path,
    stage: &Path,
    platform: LibjdPlatform,
) -> Result<(), CoreError> {
    let directory = library.parent().unwrap();
    let old = directory.with_file_name(format!(".old-{}", platform.target));
    if fs::symlink_metadata(&old).is_ok_and(|metadata| !metadata.is_dir()) {
        return Err(CoreError::Other("ime-jd 旧版本目录无效".into()));
    }
    if old.try_exists()? {
        if directory.try_exists()? {
            crate::package_fs::remove_dir_all(&old)?;
        } else {
            crate::package_fs::rename(&old, directory)?;
        }
    }
    let staged_library = stage.join(platform.library);
    let same_install = platform
        .manifest_at(library, tag)
        .zip(platform.manifest_at(&staged_library, tag))
        .is_some_and(|(current, staged)| {
            current.sha256 == staged.sha256
                && current.abi_layout == staged.abi_layout
                && fs::metadata(library)
                    .ok()
                    .zip(fs::metadata(&staged_library).ok())
                    .is_some_and(|(current, staged)| current.len() == staged.len())
                && fs::read(library)
                    .ok()
                    .zip(fs::read(staged_library).ok())
                    .is_some_and(|(current, staged)| current == staged)
        });
    if !same_install {
        crate::package_store::publish_directory(directory, stage, crate::package_fs::rename)?;
    }
    crate::scheme_files::write_file_atomic(&platform.current_path(root), tag.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn release_from_location_parses_latest_redirect_without_api() {
        let platform = libjd::platform_for("windows", "aarch64").unwrap();
        let tag = "v0.13.0";
        let name = platform.asset_name(tag);
        let url = format!("{RELEASE_DOWNLOAD}{tag}/{name}");
        let offer = release_from_location("/hronro/ime-jd/releases/tag/v0.13.0", platform).unwrap();
        assert_eq!(
            (offer.version(), offer.size(), offer.source_url()),
            (tag, 0, url.as_str())
        );
        let full = release_from_location(
            "https://github.com/hronro/ime-jd/releases/tag/v0.13.0",
            platform,
        )
        .unwrap();
        assert_eq!((full.version(), full.source_url()), (tag, url.as_str()));

        for bad in [
            "https://evil.invalid/hronro/ime-jd/releases/tag/v0.13.0",
            "/elsewhere/ime-jd/releases/tag/v0.13.0",
            "/hronro/ime-jd/releases/tag/v0.13.0?foo=bar",
            "/hronro/ime-jd/releases/tag/not-a-tag!",
        ] {
            assert!(release_from_location(bad, platform).is_err(), "{bad}");
        }
    }

    #[test]
    fn compute_digest_matches_sha256_of_archive() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let original = b"release archive";
        let sha256 = format!("{:x}", Sha256::digest(original));
        file.write_all(original).unwrap();
        assert_eq!(compute_digest(&mut file.reopen().unwrap()).unwrap(), sha256);
    }

    #[test]
    fn only_the_original_repository_can_supply_asset_bytes() {
        let asset = Url::parse(&format!(
            "{RELEASE_DOWNLOAD}v0.13.0/libjd-v0.13.0-macos-arm64.dylib.tar.xz"
        ))
        .unwrap();
        let cdn = Url::parse(&format!(
            "https://release-assets.githubusercontent.com{RELEASE_CDN_PREFIX}example"
        ))
        .unwrap();
        assert!(allowed_asset_redirect(&asset, &cdn));
        let other_repo = Url::parse(
            "https://release-assets.githubusercontent.com/github-production-release-asset/999999/example",
        )
        .unwrap();
        assert!(!allowed_asset_redirect(&asset, &other_repo));
        assert!(!allowed_asset_redirect(
            &Url::parse(RELEASE_LATEST).unwrap(),
            &cdn
        ));
    }

    #[tokio::test]
    async fn release_query_uses_latest_redirect_and_reports_http_or_network_reasons() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        async fn serve_once(status: &str, headers: &str, body: &str) -> String {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
                body.len()
            );
            tokio::spawn(async move {
                if let Ok((mut socket, _)) = listener.accept().await {
                    let mut buf = [0u8; 1024];
                    let _ = socket.read(&mut buf).await;
                    let _ = socket.write_all(response.as_bytes()).await;
                }
            });
            format!("http://{addr}/releases/latest")
        }

        let platform = libjd::platform_for("macos", "aarch64").unwrap();
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .build()
            .unwrap();

        let redirect_url = serve_once(
            "302 Found",
            "Location: https://github.com/hronro/ime-jd/releases/tag/v0.14.0\r\n",
            "",
        )
        .await;
        let offer = latest_release_at(&client, &redirect_url, platform)
            .await
            .unwrap();
        assert_eq!(offer.version(), "v0.14.0");
        assert!(
            offer
                .source_url()
                .ends_with("/v0.14.0/libjd-v0.14.0-macos-arm64.dylib.tar.xz")
        );

        let missing_url = serve_once("404 Not Found", "", "").await;
        let missing_err = latest_release_at(&client, &missing_url, platform)
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(missing_err.contains("未找到已发布的版本"), "{missing_err}");

        let html_url = serve_once("200 OK", "Content-Type: text/html\r\n", "<html>portal</html>").await;
        let html_err = latest_release_at(&client, &html_url, platform)
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(html_err.contains("未返回重定向"), "{html_err}");

        let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closed_addr = closed.local_addr().unwrap();
        drop(closed);
        let conn_err = latest_release_at(
            &client,
            &format!("http://{closed_addr}/releases/latest"),
            platform,
        )
        .await
        .err()
        .unwrap()
        .to_string();
        assert!(conn_err.contains("无法连接"), "{conn_err}");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn latest_macos_archive_installs_and_loads() {
        use crate::{AppEnv, AppState, CoreEvent, EventSink, Platform};
        use std::sync::{Arc, Mutex};

        if std::env::var("LIBJD_TEST").as_deref() != Ok("1") {
            return;
        }
        struct Sink;
        impl EventSink for Sink {
            fn emit(&self, _: CoreEvent) {}
        }
        let directory = tempfile::tempdir().unwrap();
        let core = Core {
            env: AppEnv {
                data_dir: directory.path().to_owned(),
                cache_dir: directory.path().join("cache"),
                resource_dir: directory.path().join("resources"),
                app_version: "test".into(),
                platform: Platform::MacOs,
            },
            sink: Arc::new(Sink),
            state: Mutex::new(AppState::default()),
            download_cancel: tokio::sync::watch::channel(0).0,
            recovered_state_file: None,
            pending_package: Mutex::new(()),
            user_words: Mutex::new(crate::user_words::UserWordsState::default()),
        };
        assert!(libjd_status(&core).installed_version.is_none());
        let release = libjd_release(&core).await.unwrap();
        let status = install_or_update_libjd(&core, release).await.unwrap();
        let tag = status.installed_version.unwrap();
        let library = libjd::installed_library(directory.path()).unwrap();
        assert_eq!(
            libjd::library_path(directory.path(), &tag),
            Some(library.clone())
        );
        let manifest = libjd::manifest_at(&library, &tag).unwrap();
        assert_eq!(
            crate::validate_libjd_library(&library).unwrap(),
            manifest.abi_layout
        );
    }
}
