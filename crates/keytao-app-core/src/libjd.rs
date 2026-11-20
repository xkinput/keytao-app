use crate::{
    Core, CoreError,
    download::{DOWNLOAD_CANCELLED, download_to_temp_with, emit_progress},
};
use keytao_core::libjd::{self, LibjdManifest, LibjdPlatform};
use reqwest::{Url, redirect::Policy};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufReader, Read, Seek},
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const RELEASE_API: &str = "https://api.github.com/repos/hronro/ime-jd/releases/latest";
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
    sha256: String,
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

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
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

async fn latest_release(client: &reqwest::Client) -> Result<Release, CoreError> {
    let response = client
        .get(RELEASE_API)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header(reqwest::header::CACHE_CONTROL, "no-cache")
        .send()
        .await
        .map_err(|error| CoreError::Network(error.without_url().to_string()))?;
    if !response.status().is_success() {
        return Err(CoreError::Other(format!(
            "查询 ime-jd 版本失败，HTTP {}",
            response.status()
        )));
    }
    response
        .json()
        .await
        .map_err(|error| CoreError::Network(error.without_url().to_string()))
}

fn select_asset(release: Release, platform: LibjdPlatform) -> Result<LibjdRelease, CoreError> {
    let tag = release.tag_name;
    if !libjd::valid_tag(&tag) {
        return Err(CoreError::Other("ime-jd 版本号无效".into()));
    }
    let name = platform.asset_name(&tag);
    let mut candidates = release
        .assets
        .into_iter()
        .filter(|asset| asset.name == name);
    let asset = candidates
        .next()
        .ok_or_else(|| CoreError::Other("当前平台没有 ime-jd 安装包".into()))?;
    if candidates.next().is_some()
        || asset.browser_download_url != format!("{RELEASE_DOWNLOAD}{tag}/{name}")
        || asset.size == 0
        || asset.size > MAX_ARCHIVE_BYTES
    {
        return Err(CoreError::Other("ime-jd 安装包信息无效".into()));
    }
    let sha256 = asset
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .filter(|digest| libjd::valid_sha256(digest))
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| CoreError::Other("ime-jd 安装包缺少 SHA-256".into()))?;
    Ok(LibjdRelease {
        platform,
        tag,
        asset,
        sha256,
    })
}

fn verify_digest(file: &mut File, expected: &str) -> Result<(), CoreError> {
    let mut hash = Sha256::new();
    io::copy(file, &mut hash)?;
    if format!("{:x}", hash.finalize()) != expected {
        return Err(CoreError::Other("ime-jd SHA-256 校验失败".into()));
    }
    Ok(())
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
    let client = release_client(core)?;
    let release = tokio::select! {
        biased;
        _ = cancel.changed() => return Err(CoreError::Other(DOWNLOAD_CANCELLED.into())),
        release = latest_release(&client) => release?,
    };
    select_asset(release, platform)
}

pub async fn install_or_update_libjd(
    core: &Core,
    release: LibjdRelease,
) -> Result<LibjdStatus, CoreError> {
    let LibjdRelease {
        platform,
        tag,
        asset,
        sha256,
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
        Some(asset.size),
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
        verify_digest(&mut archive, &sha256)?;
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
    fn release_offer_requires_the_exact_platform_asset_and_digest() {
        let platform = libjd::platform_for("windows", "aarch64").unwrap();
        let tag = "v0.13.0";
        let name = platform.asset_name(tag);
        let url = format!("{RELEASE_DOWNLOAD}{tag}/{name}");
        let release = |source: &str, digest: Option<String>| Release {
            tag_name: tag.to_owned(),
            assets: vec![Asset {
                name: name.clone(),
                browser_download_url: source.to_owned(),
                digest,
                size: 123,
            }],
        };
        let digest = format!("sha256:{}", "a".repeat(64));
        let offer = select_asset(release(&url, Some(digest.clone())), platform).unwrap();
        assert_eq!(
            (offer.version(), offer.size(), offer.source_url()),
            (tag, 123, url.as_str())
        );
        assert!(select_asset(release(&url, None), platform).is_err());
        assert!(
            select_asset(
                release(&url.replace("hronro", "elsewhere"), Some(digest.clone())),
                platform
            )
            .is_err()
        );
        let x64 = libjd::platform_for("windows", "x86_64").unwrap();
        assert!(select_asset(release(&url, Some(digest)), x64).is_err());
    }

    #[test]
    fn changed_archive_does_not_pass_the_release_digest() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let original = b"release archive";
        let sha256 = format!("{:x}", Sha256::digest(original));
        file.write_all(original).unwrap();
        assert!(verify_digest(&mut file.reopen().unwrap(), &sha256).is_ok());
        file.write_all(b"tampered").unwrap();
        let error = verify_digest(&mut file.reopen().unwrap(), &sha256).unwrap_err();
        assert!(error.to_string().contains("SHA-256"));
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
            &Url::parse(RELEASE_API).unwrap(),
            &cdn
        ));
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
            pending_package: Mutex::new(None),
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
