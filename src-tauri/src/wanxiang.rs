//! Optional Wanxiang dictionary installation shared by desktop and mobile.
//!
//! Download and extraction finish before any live source file changes. The
//! caller updates the schema list and runs the existing Rime deploy pipeline,
//! then calls `finish`; dropping the receipt rolls the source files back.
//! The portable basic schema uses only built-in Rime components, retaining
//! upstream's dictionary and full-pinyin algebra without Lua or a grammar model.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    io::AsyncWriteExt,
    sync::{Mutex, MutexGuard},
};

pub const SCHEMA_ID: &str = "wanxiang";
pub const VERSION: &str = "18.0.5";
pub const DOWNLOAD_BYTES: u64 = 35_216_559;
pub const SOURCE_URL: &str = "https://github.com/amzxyz/rime-wanxiang/releases/tag/v18.0.5";
const DOWNLOAD_URL: &str =
    "https://github.com/amzxyz/rime-wanxiang/releases/download/v18.0.5/rime-wanxiang-base.zip";
const DOWNLOAD_SHA256: &str = "9d3a1d26213c3d2c59e6cf2325678f14dedd7182149644e18675f764ab7393c1";
const MANIFEST: &str = ".keytao-wanxiang.json";
const NOTICE: &str = "wanxiang-NOTICE.txt";
const MAX_FILES: usize = 4096;
const MAX_EXPANDED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 128 * 1024 * 1024;
static OPERATION: Mutex<()> = Mutex::const_new(());
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

const REQUIRED: &[&str] = &[
    "wanxiang.schema.yaml",
    "wanxiang.dict.yaml",
    "wanxiang_algebra.yaml",
];
const DICTIONARIES: &[&str] = &[
    "zi",
    "jichu",
    "lianxiang",
    "cuoyin",
    "duoyin",
    "shici",
    "diming",
    "yixue",
    "huaxue",
    "yaopin",
    "mingren",
    "yiren",
    "wuzhong",
    "renming",
    "taifeng",
    "fangyan",
];
const BASIC_SCHEMA: &str = include_str!("wanxiang-basic.schema.yaml");

fn required_files() -> Vec<String> {
    REQUIRED
        .iter()
        .map(|s| (*s).to_string())
        .chain(
            DICTIONARIES
                .iter()
                .map(|name| format!("wanxiang-dicts/{name}.dict.yaml")),
        )
        .collect()
}

#[derive(Debug, Serialize)]
pub struct Status {
    pub installed: bool,
    pub deployed: bool,
    pub version: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct InstalledFile {
    path: String,
    sha256: String,
    size: u64,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    format: u32,
    version: String,
    source: String,
    archive_sha256: String,
    includes_grammar: bool,
    files: Vec<InstalledFile>,
}

pub fn status_at(root: &Path) -> Status {
    let manifest = read_manifest(root).ok().flatten();
    let installed = manifest.is_some()
        && required_files()
            .iter()
            .all(|name| safe_target(root, name).is_ok_and(|p| p.is_file()));
    Status {
        installed,
        deployed: installed
            && root.join("build/wanxiang.schema.yaml").is_file()
            && root.join("build/wanxiang.table.bin").is_file(),
        version: manifest.map_or_else(|| VERSION.into(), |m| m.version),
    }
}

fn read_manifest(root: &Path) -> Result<Option<Manifest>, String> {
    let path = safe_target(root, MANIFEST)?;
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("读取万象安装记录失败：{e}")),
    };
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 1024 * 1024 {
        return Err("万象安装记录过大".into());
    }
    let manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|e| format!("万象安装记录损坏：{e}"))?;
    if manifest.format != 1 || manifest.files.len() > MAX_FILES {
        return Err("无法识别万象安装记录".into());
    }
    let mut seen = BTreeSet::new();
    for file in &manifest.files {
        validate_relative(&file.path)?;
        if !installable(&file.path)
            || !seen.insert(file.path.to_ascii_lowercase())
            || file.sha256.len() != 64
            || !file.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("万象安装记录含无效文件".into());
        }
    }
    Ok(Some(manifest))
}

// Reject Windows aliases/ADS and traversal on every OS so an archive accepted
// on Linux cannot become unsafe when the same package is installed on Windows.
fn validate_relative(name: &str) -> Result<(), String> {
    if name.is_empty() || name.contains('\\') || name.contains(':') || name.contains('\0') {
        return Err(format!("万象压缩包包含不安全路径：{name}"));
    }
    for part in name.split('/') {
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'));
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with(['.', ' '])
            || part.chars().any(char::is_control)
            || reserved
        {
            return Err(format!("万象压缩包包含不安全路径：{name}"));
        }
    }
    Ok(())
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn safe_target(root: &Path, relative: &str) -> Result<PathBuf, String> {
    validate_relative(relative)?;
    let mut path = root.to_path_buf();
    for component in relative.split('/') {
        path.push(component);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if is_link(&metadata) => {
                return Err(format!("万象安装路径不能经过符号链接：{}", path.display()));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("检查万象安装路径失败：{e}")),
        }
    }
    Ok(path)
}

fn installable(name: &str) -> bool {
    REQUIRED.contains(&name)
        || name == NOTICE
        || name
            .strip_prefix("wanxiang-dicts/")
            .and_then(|s| s.strip_suffix(".dict.yaml"))
            .is_some_and(|stem| DICTIONARIES.contains(&stem))
}

fn package_destination(name: &str) -> Option<String> {
    if REQUIRED.contains(&name) {
        return Some(name.into());
    }
    let stem = name.strip_prefix("dicts/")?.strip_suffix(".dict.yaml")?;
    DICTIONARIES
        .contains(&stem)
        .then(|| format!("wanxiang-dicts/{stem}.dict.yaml"))
}

fn fingerprint(path: &Path) -> Result<(String, u64), String> {
    let mut file = File::open(path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
    let mut hash = Sha256::new();
    let mut size = 0;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        size += count as u64;
    }
    Ok((format!("{:x}", hash.finalize()), size))
}

struct Change {
    path: String,
    had_original: bool,
    replacement_hash: Option<String>,
}

/// Must survive until deployment succeeds. An unfinished receipt restores the
/// old source files, including on async command cancellation/unwinding.
pub struct InstallReceipt {
    root: PathBuf,
    work: PathBuf,
    changes: Vec<Change>,
    pending: bool,
    #[cfg(windows)]
    public_sources: Vec<String>,
    _operation: MutexGuard<'static, ()>,
}

impl InstallReceipt {
    fn new(root: &Path) -> Result<Self, String> {
        let operation = OPERATION
            .try_lock()
            .map_err(|_| "万象安装或卸载正在进行中")?;
        fs::create_dir_all(root).map_err(|e| format!("创建用户目录失败：{e}"))?;
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        let work = loop {
            let path = root.join(format!(
                ".keytao-wanxiang-stage-{}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => break path,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("创建万象临时目录失败：{e}")),
            }
        };
        Ok(Self {
            root,
            work,
            changes: Vec::new(),
            pending: true,
            #[cfg(windows)]
            public_sources: Vec::new(),
            _operation: operation,
        })
    }

    fn move_original(&mut self, relative: &str) -> Result<(), String> {
        let destination = safe_target(&self.root, relative)?;
        let had_original = destination.exists();
        if had_original {
            if !destination.is_file() {
                return Err(format!("万象目标不是文件：{relative}"));
            }
            let backup = self.work.join("backup").join(relative);
            fs::create_dir_all(backup.parent().unwrap()).map_err(|e| e.to_string())?;
            fs::rename(&destination, backup).map_err(|e| format!("备份 {relative} 失败：{e}"))?;
        }
        self.changes.push(Change {
            path: relative.into(),
            had_original,
            replacement_hash: None,
        });
        Ok(())
    }

    fn replace(&mut self, relative: &str, source: &Path, hash: String) -> Result<(), String> {
        let destination = safe_target(&self.root, relative)?;
        fs::create_dir_all(destination.parent().unwrap()).map_err(|e| e.to_string())?;
        self.move_original(relative)?;
        fs::rename(source, &destination).map_err(|e| format!("安装 {relative} 失败：{e}"))?;
        self.changes.last_mut().unwrap().replacement_hash = Some(hash);
        Ok(())
    }

    fn clean_work(&self) -> Result<(), String> {
        // Only delete the exact temporary child this transaction created.
        if self.work.parent() != Some(self.root.as_path())
            || !self
                .work
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .starts_with(".keytao-wanxiang-stage-")
        {
            return Err("万象临时目录不在用户目录内".into());
        }
        if let Ok(metadata) = fs::symlink_metadata(&self.work) {
            if is_link(&metadata) {
                return Err("万象临时目录已被替换为链接".into());
            }
            fs::remove_dir_all(&self.work).map_err(|e| format!("清理万象临时文件失败：{e}"))?;
        }
        Ok(())
    }

    fn rollback_inner(&mut self) -> Result<(), String> {
        while let Some(change) = self.changes.last() {
            let destination = safe_target(&self.root, &change.path)?;
            if let Some(expected) = &change.replacement_hash {
                if destination.exists() {
                    if fingerprint(&destination)?.0 != *expected {
                        return Err(format!(
                            "{} 安装后已被修改，旧文件保留在 {}",
                            change.path,
                            self.work.display()
                        ));
                    }
                    fs::remove_file(&destination).map_err(|e| e.to_string())?;
                }
            } else if destination.exists() {
                return Err(format!(
                    "{} 已重新出现，旧文件保留在 {}",
                    change.path,
                    self.work.display()
                ));
            }
            if change.had_original {
                fs::rename(self.work.join("backup").join(&change.path), &destination)
                    .map_err(|e| format!("还原 {} 失败：{e}", change.path))?;
            }
            self.changes.pop();
        }
        self.clean_work()
    }

    pub fn rollback(mut self) -> Result<(), String> {
        self.pending = false;
        self.rollback_inner()
    }

    pub fn finish(mut self) -> Result<(), String> {
        self.pending = false;
        self.clean_work()
    }

    /// Official, adapted package sources only. Do not substitute files from
    /// `root`: those may contain private user customizations or vocabulary.
    #[cfg(windows)]
    pub fn public_source_files(&self) -> Vec<(PathBuf, PathBuf)> {
        self.public_sources
            .iter()
            .map(|relative| {
                (
                    PathBuf::from(relative),
                    self.work.join("files").join(relative),
                )
            })
            .collect()
    }
}

impl Drop for InstallReceipt {
    fn drop(&mut self) {
        if self.pending {
            if let Err(error) = self.rollback_inner() {
                tracing::error!(%error, "wanxiang source rollback failed; backups retained");
            }
        }
    }
}

async fn download(path: &Path, progress: &(impl Fn(u8, &str) + Send + Sync)) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .user_agent("KeyTao-Wanxiang-Installer")
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(15 * 60))
        .https_only(true)
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .get(DOWNLOAD_URL)
        .send()
        .await
        .map_err(|e| format!("下载万象词库失败：{e}"))?
        .error_for_status()
        .map_err(|e| format!("下载万象词库失败：{e}"))?;
    if response
        .content_length()
        .is_some_and(|size| size != DOWNLOAD_BYTES)
    {
        return Err("官方万象发布包大小已变化，请更新应用后重试".into());
    }
    let mut output = tokio::fs::File::create(path)
        .await
        .map_err(|e| e.to_string())?;
    let mut stream = response.bytes_stream();
    let mut downloaded = 0u64;
    let mut hash = Sha256::new();
    let mut last_percent = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("下载万象词库中断：{e}"))?;
        downloaded += chunk.len() as u64;
        if downloaded > DOWNLOAD_BYTES {
            return Err("万象下载超出预期大小".into());
        }
        output
            .write_all(&chunk)
            .await
            .map_err(|e| format!("保存万象下载失败：{e}"))?;
        hash.update(&chunk);
        let percent = (5 + downloaded * 45 / DOWNLOAD_BYTES) as u8;
        if percent != last_percent {
            progress(percent, "正在下载万象拼音基础词库…");
            last_percent = percent;
        }
    }
    output.flush().await.map_err(|e| e.to_string())?;
    if downloaded != DOWNLOAD_BYTES || format!("{:x}", hash.finalize()) != DOWNLOAD_SHA256 {
        return Err("万象词库完整性校验失败，未修改已安装方案".into());
    }
    Ok(())
}

fn extract(archive: &Path, stage: &Path) -> Result<Vec<InstalledFile>, String> {
    let mut archive = zip::ZipArchive::new(File::open(archive).map_err(|e| e.to_string())?)
        .map_err(|e| format!("无法读取万象压缩包：{e}"))?;
    if archive.len() > MAX_FILES {
        return Err("万象压缩包文件数量超出限制".into());
    }
    let mut seen = BTreeSet::new();
    let mut files = Vec::new();
    let mut expanded = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
        let name = entry.name().trim_end_matches('/').to_string();
        validate_relative(&name)?;
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(format!("万象压缩包包含符号链接：{name}"));
        }
        if entry.is_dir() {
            continue;
        }
        if !seen.insert(name.to_ascii_lowercase()) {
            return Err(format!("万象压缩包含重复路径：{name}"));
        }
        expanded = expanded
            .checked_add(entry.size())
            .ok_or("万象压缩包大小溢出")?;
        if entry.size() > MAX_FILE_BYTES || expanded > MAX_EXPANDED_BYTES {
            return Err("万象压缩包展开大小超出限制".into());
        }
        let Some(name) = package_destination(&name) else {
            continue;
        };
        let destination = safe_target(stage, &name)?;
        fs::create_dir_all(destination.parent().unwrap()).map_err(|e| e.to_string())?;
        let mut file = File::create(&destination).map_err(|e| e.to_string())?;
        let copied = std::io::copy(&mut entry.by_ref().take(MAX_FILE_BYTES + 1), &mut file)
            .map_err(|e| format!("解压 {name} 失败：{e}"))?;
        if copied != entry.size() {
            return Err(format!("万象文件大小不符：{name}"));
        }
        drop(file);
        adapt_source(&name, &destination)?;
        let (sha256, size) = fingerprint(&destination)?;
        files.push(InstalledFile {
            path: name,
            sha256,
            size,
        });
    }
    for required in required_files() {
        if !stage.join(&required).is_file() {
            return Err(format!("万象发布包缺少必要文件：{required}"));
        }
    }
    let notice = format!(
        "Wanxiang Pinyin / 万象拼音 {VERSION}\nAuthor: amzxyz and upstream contributors\n\
         Source: {SOURCE_URL}\nLicense: Creative Commons Attribution 4.0 International\n\
         https://creativecommons.org/licenses/by/4.0/\n\
         Full upstream license: https://github.com/amzxyz/rime-wanxiang/blob/v18.0.5/LICENSE\n\
         Provided AS IS, without warranties; see the linked license for terms.\n\
         KeyTao integration changes: a portable schema uses built-in Rime components\n\
         with upstream's full-pinyin algebra and dictionary; dictionary tables are\n\
         relocated from dicts/ to wanxiang-dicts/ to avoid other schemes' files.\n\
         The separate grammar model, Lua extensions, host appearance settings and\n\
         unrelated auxiliary schemas are not included.\n\
         User customizations and learned dictionaries are preserved during uninstall.\n"
    );
    fs::write(stage.join(NOTICE), notice).map_err(|e| e.to_string())?;
    let (sha256, size) = fingerprint(&stage.join(NOTICE))?;
    files.push(InstalledFile {
        path: NOTICE.into(),
        sha256,
        size,
    });
    Ok(files)
}

fn adapt_source(name: &str, path: &Path) -> Result<(), String> {
    if name == "wanxiang.schema.yaml" {
        fs::write(path, BASIC_SCHEMA).map_err(|e| e.to_string())?;
    } else if name == "wanxiang.dict.yaml" {
        let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
        fs::write(path, content.replace("dicts/", "wanxiang-dicts/")).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn commit_files(receipt: &mut InstallReceipt, files: Vec<InstalledFile>) -> Result<(), String> {
    #[cfg(windows)]
    {
        receipt.public_sources = files.iter().map(|file| file.path.clone()).collect();
    }
    let previous = read_manifest(&receipt.root)?;
    let previous: BTreeMap<_, _> = previous
        .into_iter()
        .flat_map(|m| m.files)
        .map(|file| (file.path.clone(), file))
        .collect();
    let mut owned = Vec::new();
    let mut to_replace = Vec::new();
    // Preflight every conflict before moving a single installed source file.
    for file in files {
        let destination = safe_target(&receipt.root, &file.path)?;
        if destination.exists() {
            let (hash, _) = fingerprint(&destination)?;
            if hash == file.sha256 {
                if previous.contains_key(&file.path) {
                    owned.push(file);
                }
                continue;
            }
            if previous
                .get(&file.path)
                .is_none_or(|old| old.sha256 != hash)
            {
                return Err(format!(
                    "已有文件 {} 与万象词库冲突，已保留原文件；请先备份并移走该文件",
                    file.path
                ));
            }
        }
        owned.push(file.clone());
        to_replace.push(file);
    }
    for file in to_replace {
        let source = receipt.work.join("files").join(&file.path);
        #[cfg(windows)]
        let source = {
            // Retain immutable official sources for AppContainer publication.
            let copy = receipt.work.join("commit").join(&file.path);
            fs::create_dir_all(copy.parent().unwrap()).map_err(|e| e.to_string())?;
            fs::copy(&source, &copy).map_err(|e| e.to_string())?;
            copy
        };
        receipt.replace(&file.path, &source, file.sha256)?;
    }
    let manifest = Manifest {
        format: 1,
        version: VERSION.into(),
        source: SOURCE_URL.into(),
        archive_sha256: DOWNLOAD_SHA256.into(),
        includes_grammar: false,
        files: owned,
    };
    let path = receipt.work.join("manifest.json");
    fs::write(
        &path,
        serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let hash = fingerprint(&path)?.0;
    receipt.replace(MANIFEST, &path, hash)
}

pub async fn install(
    root: &Path,
    progress: impl Fn(u8, &str) + Send + Sync + 'static,
) -> Result<InstallReceipt, String> {
    let receipt = InstallReceipt::new(root)?;
    // Detect an invalid ownership record before spending bandwidth.
    read_manifest(&receipt.root)?;
    progress(5, "正在下载万象拼音基础词库（不含语法模型）…");
    let archive = receipt.work.join("download.zip");
    download(&archive, &progress).await?;
    progress(52, "正在校验并解压万象拼音词库…");
    tokio::task::spawn_blocking(move || {
        let mut receipt = receipt;
        let files = extract(&archive, &receipt.work.join("files"))?;
        progress(60, "正在安装万象拼音词库…");
        commit_files(&mut receipt, files)?;
        Ok(receipt)
    })
    .await
    .map_err(|e| format!("万象安装任务失败：{e}"))?
}

pub fn uninstall(root: &Path) -> Result<InstallReceipt, String> {
    let mut receipt = InstallReceipt::new(root)?;
    if let Some(manifest) = read_manifest(&receipt.root)? {
        for file in manifest.files {
            let path = safe_target(&receipt.root, &file.path)?;
            // Uninstall owns only unmodified files it actually installed.
            if path.is_file() && fingerprint(&path)?.0 == file.sha256 {
                receipt.move_original(&file.path)?;
            }
        }
        receipt.move_original(MANIFEST)?;
    }
    Ok(receipt)
}

#[cfg(test)]
#[path = "wanxiang_tests.rs"]
mod tests;
