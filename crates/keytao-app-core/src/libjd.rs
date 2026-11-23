use crate::{
    Core, CoreError,
    download::{DOWNLOAD_CANCELLED, download_to_temp_with, emit_progress, send_get_with_retry},
};
use keytao_core::libjd::{self, LibjdManifest, LibjdPlatform};
use reqwest::{Url, redirect::Policy};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufReader, Read, Seek},
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const RELEASE_LATEST: &str = "https://github.com/hronro/ime-jd/releases/latest";
const RELEASE_TAG_PREFIX: &str = "/hronro/ime-jd/releases/tag/";
const RELEASE_DOWNLOAD: &str = "https://github.com/hronro/ime-jd/releases/download/";
const RELEASE_ASSETS: &str = "https://github.com/hronro/ime-jd/releases/expanded_assets/";
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

pub fn imported_package_ready(core: &Core, key: &str) -> bool {
    let data = &core.env().data_dir;
    supported_platform()
        .is_some_and(|platform| platform.engine_only().installed_library(data).is_some())
        && libjd::imported_dictionary(data, key).is_some()
}

pub async fn import_package(core: &Arc<Core>, root: &Path, key: &str) -> Result<usize, CoreError> {
    if !crate::deploy::active_uses_libjd(core, root) {
        return Err(CoreError::Other("请先切换到 ime-jd 引擎".into()));
    }
    let platform = supported_platform()
        .ok_or_else(|| CoreError::Unsupported("当前平台不支持 ime-jd".into()))?
        .engine_only();
    let data = &core.env().data_dir;
    let destination = libjd::dictionary_directory(data, key)
        .ok_or_else(|| CoreError::Other("ime-jd 方案目录无效".into()))?;
    let shared = crate::deploy::deploy_shared_data_dir(core, root);
    if keytao_core::RuntimeDirs::for_package_in_store(root, &shared, data, key).is_none() {
        return Err(CoreError::Other("请先下载或导入此 Rime 方案".into()));
    }
    if platform.installed_library(data).is_none() {
        let client = latest_redirect_client(core)?;
        let release = latest_release_at(&client, RELEASE_LATEST, platform).await?;
        install_or_update_libjd(core, release).await?;
    }
    let library = platform
        .installed_library(data)
        .ok_or_else(|| CoreError::Other("ime-jd 转换引擎未安装".into()))?;
    let job_core = Arc::clone(core);
    let job_root = root.to_path_buf();
    let job_key = key.to_owned();
    let skipped = tokio::task::spawn_blocking(move || {
        emit_progress(&job_core, "converting", 0, "正在准备转换词库…");
        let result =
            crate::libjd_dictionary::convert(&job_core, &job_root, &job_key, |index, total| {
                let percent = (index.saturating_sub(1) * 90 / total.max(1)) as u32;
                emit_progress(
                    &job_core,
                    "converting",
                    percent,
                    &format!("正在转换词库 {index}/{total}"),
                );
            })?;
        let parent = destination.parent().unwrap();
        let stage = tempfile::Builder::new()
            .prefix(".jd-dictionary-")
            .tempdir_in(parent)?;
        emit_progress(&job_core, "converting", 92, "正在编译并校验 ime-jd 词库…");
        build_imported_dictionary(&library, &result.text, &stage.path().join("dictionary.bin"))?;
        let schema = keytao_core::SchemaInfo {
            id: result.schema_id,
            name: result.schema_name,
        };
        crate::scheme_files::write_file_atomic(
            &stage.path().join("schema.json"),
            &serde_json::to_vec(&schema)?,
        )?;
        let old = destination.with_file_name(".old-libjd-dictionary");
        match fs::symlink_metadata(&old) {
            Ok(metadata) if !metadata.is_dir() => {
                return Err(CoreError::Other("ime-jd 旧词库目录无效".into()));
            }
            Ok(_) if destination.try_exists()? => crate::package_fs::remove_dir_all(&old)?,
            Ok(_) => crate::package_fs::rename(&old, &destination)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        crate::package_store::publish_directory(
            &destination,
            stage.path(),
            crate::package_fs::rename,
        )?;
        emit_progress(&job_core, "converting", 98, "正在切换输入方案…");
        crate::packages::set_package_engine(&job_core, &job_root, &job_key, "libjd_engine")?;
        Ok::<_, CoreError>(result.skipped)
    })
    .await
    .map_err(|error| CoreError::Other(error.to_string()))??;
    emit_progress(core, "done", 100, "已切换到导入的方案");
    Ok(skipped)
}

fn build_imported_dictionary(
    library: &Path,
    text: &[u8],
    destination: &Path,
) -> Result<(), CoreError> {
    #[cfg(windows)]
    {
        let version = library
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| CoreError::Other("ime-jd 安装目录无效".into()))?;
        let stage = tempfile::Builder::new()
            .prefix(".libjd-")
            .tempdir_in(version)?;
        let staged_library = stage.path().join("libjd.dll");
        fs::copy(library, &staged_library)?;
        fs::write(stage.path().join("dictionary.txt"), text)?;
        crate::ime_status::windows::build_libjd_dictionary(&staged_library)
            .map_err(CoreError::Other)?;
        crate::package_fs::rename(&stage.path().join("dictionary.bin"), destination)?;
        Ok(())
    }
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "android"))]
    {
        // Library resolution requires a verified installation manifest.
        unsafe { keytao_core::jd_backend::JdBackend::build_dictionary(library, text, destination) }
            .map_err(CoreError::Other)
    }
    #[cfg(not(any(
        windows,
        target_os = "macos",
        target_os = "linux",
        target_os = "android"
    )))]
    {
        let _ = (library, text, destination);
        Err(CoreError::Unsupported("当前平台不支持 ime-jd".into()))
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
        .map_err(|error| {
            CoreError::Other(format!("查询 ime-jd 版本失败：发布地址无效（{error}）"))
        })?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(CoreError::Other(
            "查询 ime-jd 版本失败：发布地址无效".into(),
        ));
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
        format!(
            "GitHub 请求过于频繁（HTTP {}），请稍后再试",
            status.as_u16()
        )
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

fn asset_digest(html: &str, asset: &str) -> Option<String> {
    let label = format!(r#"aria-label="Copy to clipboard digest for {asset}""#);
    html.lines().find_map(|line| {
        if !line.trim_start().starts_with("<clipboard-copy ") || !line.contains(&label) {
            return None;
        }
        let digest = line.split_once(" value=\"sha256:")?.1.split_once('"')?.0;
        libjd::valid_sha256(digest).then(|| digest.to_ascii_lowercase())
    })
}

async fn published_digest(
    client: &reqwest::Client,
    tag: &str,
    asset: &str,
) -> Result<String, CoreError> {
    let request = client
        .get(format!("{RELEASE_ASSETS}{tag}"))
        .header(reqwest::header::ACCEPT_LANGUAGE, "en");
    let mut response = send_get_with_retry(request)
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(format_release_request_error)?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(format_release_request_error)?
    {
        if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
            return Err(CoreError::Other("ime-jd 发布信息过大".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
    asset_digest(&String::from_utf8_lossy(&bytes), asset)
        .ok_or_else(|| CoreError::Other("找不到 ime-jd 安装包的发布校验值".into()))
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
    let supported = supported_platform().map(|base| {
        if platform.is_engine_only() {
            base.engine_only()
        } else {
            base
        }
    });
    if supported != Some(platform) {
        return Err(CoreError::Unsupported("当前平台不支持 ime-jd".into()));
    }
    let data = &core.env().data_dir;
    let root = libjd::engine_root(data).ok_or_else(|| CoreError::Other("引擎目录无效".into()))?;
    fs::create_dir_all(&root)?;
    let lock_path = root.join(format!(".install-{}.lock", platform.install_target()));
    if fs::symlink_metadata(&lock_path).is_ok_and(|metadata| !metadata.is_file()) {
        return Err(CoreError::Other("ime-jd 安装锁无效".into()));
    }
    let install_lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    keytao_core::lock_file(&install_lock, false)
        .map_err(|error| CoreError::Other(format!("无法取得 ime-jd 安装锁: {error}")))?;

    let mut cancel = core.download_cancel.subscribe();
    let client = release_client(core)?;
    let expected_sha256 = tokio::select! {
        biased;
        _ = cancel.changed() => return Err(CoreError::Other(DOWNLOAD_CANCELLED.into())),
        digest = published_digest(&client, &tag, &asset.name) => digest?,
    };
    let download = download_to_temp_with(
        core,
        client.get(asset.browser_download_url),
        &mut cancel,
        (asset.size > 0).then_some(asset.size),
        Some(MAX_ARCHIVE_BYTES),
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
        if sha256 != expected_sha256 {
            return Err(CoreError::Other("ime-jd 安装包的 SHA-256 校验失败".into()));
        }
        archive.rewind()?;
        let library = stage.path().join(platform.library);
        extract_library(archive, &library, platform)?;
        #[cfg(target_os = "android")]
        fix_android_libjd_dependency(&library, &sha256)?;
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

#[cfg(any(target_os = "android", test))]
fn fix_android_libjd_dependency(library: &Path, archive_sha256: &str) -> io::Result<()> {
    // shortcut: pinned v0.13.0 and v0.14.0 Android ELF files omit libc.so; remove when fixed upstream.
    let (tag_offset, name_offset) = match archive_sha256 {
        "321b471349472c66302d731b66d8205aa985507f314d9b861e5e8de274b842fa" => (0x5292f8, 0x6ad),
        "4357521d54b02c6cc025af1c94be4b1ee97dab6314b6607f7770328d8f8bfa19" => (0x52b420, 0x622),
        "29fd7e249df8fcd3e9108899c3bdd058eb0022060b39878e2bdbcb0c863f34aa" => (0x532d38, 0x782),
        "2c7173d140b0d0993d3b3443ad24c170b173f2091786ccedc037a38caf505c5a" => (0x5391f0, 0x6f7),
        "796b4e618213a11adc364aad88afe51043298d24c3218383990c5cb42d32e619" => (0x12f58, 0x782),
        "0bdd44d2b1389d07f268a9fe310e67b988a1836cd2f6d41f70b59880c322e62c" => (0x193c0, 0x6f7),
        _ => return Ok(()),
    };
    let mut image = fs::read(library)?;
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "unexpected Android libjd ELF");
    let tag = image
        .get_mut(tag_offset..tag_offset + 8)
        .filter(|bytes| *bytes == 14u64.to_le_bytes())
        .ok_or_else(invalid)?;
    // Loaded by absolute path, libjd can use its SONAME slot for DT_NEEDED.
    tag.copy_from_slice(&1u64.to_le_bytes());
    let name = image
        .get_mut(name_offset..name_offset + 9)
        .filter(|bytes| *bytes == b"libjd.so\0")
        .ok_or_else(invalid)?;
    name.copy_from_slice(b"libc.so\0\0");
    let mut output = OpenOptions::new().write(true).open(library)?;
    std::io::Write::write_all(&mut output, &image)?;
    output.sync_all()
}

fn publish(
    root: &Path,
    tag: &str,
    library: &Path,
    stage: &Path,
    platform: LibjdPlatform,
) -> Result<(), CoreError> {
    let directory = library.parent().unwrap();
    let old = directory.with_file_name(format!(".old-{}", platform.install_target()));
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
    fn published_digest_must_match_the_exact_asset() {
        let name = "libjd-engine-v0.14.0-macos-arm64.dylib.tar.xz";
        let hash = "a".repeat(64);
        let page = format!(
            r#"<clipboard-copy aria-label="Copy to clipboard digest for {name}" value="sha256:{hash}">"#
        );
        assert_eq!(asset_digest(&page, name), Some(hash));
        assert!(asset_digest(&page, "libjd-v0.14.0-macos-arm64.dylib.tar.xz").is_none());
        assert!(asset_digest(&page.replace("sha256:", "sha512:"), name).is_none());
    }

    #[test]
    fn only_known_android_libjd_archives_get_the_libc_dependency() {
        let dir = tempfile::tempdir().unwrap();
        let library = dir.path().join("libjd.so");
        let mut image = vec![0; 0x5292f8 + 8];
        image[0x5292f8..].copy_from_slice(&14u64.to_le_bytes());
        image[0x6ad..0x6ad + 9].copy_from_slice(b"libjd.so\0");
        fs::write(&library, &image).unwrap();
        fix_android_libjd_dependency(&library, "unknown").unwrap();
        assert_eq!(fs::read(&library).unwrap(), image);

        let digest = "321b471349472c66302d731b66d8205aa985507f314d9b861e5e8de274b842fa";
        fix_android_libjd_dependency(&library, digest).unwrap();
        image[0x5292f8..].copy_from_slice(&1u64.to_le_bytes());
        image[0x6ad..0x6ad + 9].copy_from_slice(b"libc.so\0\0");
        assert_eq!(fs::read(&library).unwrap(), image);
        assert!(fix_android_libjd_dependency(&library, digest).is_err());
        assert_eq!(fs::read(&library).unwrap(), image);
    }

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

        let html_url = serve_once(
            "200 OK",
            "Content-Type: text/html\r\n",
            "<html>portal</html>",
        )
        .await;
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
        let platform = supported_platform().unwrap().engine_only();
        let client = latest_redirect_client(&core).unwrap();
        let release = latest_release_at(&client, RELEASE_LATEST, platform).await.unwrap();
        let status = install_or_update_libjd(&core, release).await.unwrap();
        assert_eq!(status.installed_version.as_deref(), Some(tag.as_str()));
        let engine = platform.installed_library(directory.path()).unwrap();
        assert_ne!(engine, library);
        assert!(unsafe { keytao_core::jd_backend::JdBackend::load(&engine) }.is_err());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn converted_rime_package_becomes_active_and_types() {
        use crate::{AppEnv, CoreEvent, EventSink, Platform};
        use std::io::Cursor;

        let (Some(bundled), Some(engine)) = (
            std::env::var_os("LIBJD_TEST_LIBRARY"),
            std::env::var_os("LIBJD_TEST_ENGINE_LIBRARY"),
        ) else {
            return;
        };
        #[derive(Default)]
        struct Sink(std::sync::Mutex<Vec<String>>);
        impl EventSink for Sink {
            fn emit(&self, event: CoreEvent) {
                if let CoreEvent::InstallProgress(progress) = event {
                    self.0.lock().unwrap().push(progress.message);
                }
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let sink = Arc::new(Sink::default());
        let core = Core::new(
            AppEnv {
                data_dir: root.into(),
                cache_dir: root.join("cache"),
                resource_dir: root.join("resources"),
                app_version: "test".into(),
                platform: Platform::MacOs,
            },
            sink.clone(),
        )
        .unwrap();
        let platform = supported_platform().unwrap();
        let tag = "v0.14.0";
        for (platform, source) in [(platform, bundled), (platform.engine_only(), engine)] {
            let library = platform.library_path(root, tag).unwrap();
            fs::create_dir_all(library.parent().unwrap()).unwrap();
            fs::copy(source, &library).unwrap();
            let manifest = LibjdManifest {
                tag: tag.into(),
                asset: platform.asset_name(tag),
                sha256: "1".repeat(64),
                abi_layout: crate::validate_libjd_library(&library).unwrap(),
                installed_at: 0,
            };
            fs::write(
                library.with_file_name("manifest.json"),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
            fs::write(
                platform.current_path(&libjd::engine_root(root).unwrap()),
                tag,
            )
            .unwrap();
        }
        fs::create_dir_all(root.join("runtime")).unwrap();
        fs::write(
            root.join(keytao_core::RuntimeDirs::MIGRATION_MARKER),
            br#"{"version":1,"runtime_package":"keytao"}"#,
        )
        .unwrap();
        fs::write(
            root.join(keytao_core::english_mode::SETTINGS_FILE),
            br#"{"engines":{"keytao":"libjd"}}"#,
        )
        .unwrap();
        core.lock_state().installed_packages.insert(
            "keytao".into(),
            crate::state::InstalledPackage { version: None },
        );

        let key = "local-jd-test";
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (filename, data) in [
            (
                "test.schema.yaml",
                "schema: {schema_id: test, name: 导入测试}\ntranslator: {dictionary: test}\n",
            ),
            (
                "test.dict.yaml",
                "---\nname: test\nversion: '1'\ncolumns: [text, code, weight]\n...\n转换实测\ta\t1\n",
            ),
        ] {
            zip.start_file(filename, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(data.as_bytes()).unwrap();
        }
        let archive = zip.finish().unwrap().into_inner();
        let meta = crate::packages::PackageMeta {
            scheme_key: key.into(),
            engine: Default::default(),
            version: None,
            installed_at_unix: 1,
            source: "local".into(),
            display_name: None,
            selection_base_version: None,
            git: None,
            manifest: Default::default(),
            zip_sha256: crate::packages::sha256_reader(archive.as_slice()).unwrap(),
        };
        crate::package_store::PackageStore::new(&core)
            .write_archive(Cursor::new(archive), &meta, None)
            .unwrap();
        let ime = keytao_core::ImeRuntime::with_dirs(root, root.to_string_lossy()).with_ime_host();
        ime.init_without_deploy().unwrap();
        let session = ime.create_session().unwrap();
        assert_eq!(session.state().schema_id, "keytao");
        assert_eq!(import_package(&core, root, key).await.unwrap(), 0);
        assert_eq!(
            keytao_core::RuntimeDirs::active_package_key(root).as_deref(),
            Some(key)
        );
        assert_eq!(
            keytao_core::engine_choice::read(root, Some(key)),
            keytao_core::engine_choice::PackageEngine::LibjdEngine
        );
        assert!(
            sink.0
                .lock()
                .unwrap()
                .iter()
                .any(|message| message == "正在转换词库 1/1")
        );
        ime.reload_without_deploy().unwrap();
        ime.prewarm(Some(&session)).unwrap();
        let typed = session.process_key_result(b'a'.into(), 0).unwrap();
        assert_eq!(typed.state.candidates[0].text, "转换实测");
        drop(session);
        ime.shutdown().unwrap();
        crate::package_store::PackageStore::new(&core).remove(key).unwrap();
        assert!(!imported_package_ready(&core, key));
    }
}
