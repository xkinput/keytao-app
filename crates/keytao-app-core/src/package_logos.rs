use super::{safe_path, validate_relative};
use crate::package_store::{PackageStore, StoredPackage};
use crate::{Core, CoreError};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_SOURCE_BYTES: u64 = 1024 * 1024;
pub const REMOTE_LOGO_TTL_SECS: u64 = 2 * 24 * 60 * 60;
const REMOTE_CACHE_NAMESPACE: &str = "remote-scheme-logos-v1";
const SCHEME_LOGO_CACHE: &str = "scheme-logos-v2";

pub fn package_logo_path(core: &Core, scheme_key: &str) -> Option<PathBuf> {
    let store = PackageStore::new(core);
    if let Ok(logo) = stored_package_logo(core, &store, scheme_key) {
        return logo;
    }
    if crate::addon_store::IDS.contains(&scheme_key) {
        let _guard = crate::addon_store::operation();
        let package = store.checked_addon_path(scheme_key).ok()?;
        let (stored, _) = crate::addon_store::file_inventory(&package, scheme_key).ok()?;
        if stored.files.len() > 10_000 {
            return None;
        }
        let digest = addon_logo_digest(&stored);
        return render_inventory_logo(core, &package, &stored, &digest, scheme_key)
            .ok()
            .flatten();
    }
    None
}

fn addon_logo_digest(stored: &StoredPackage) -> String {
    let mut hasher = Sha256::new();
    for entry in &stored.files {
        hasher.update(entry.path.as_bytes());
        hasher.update(b"\0");
        hasher.update(entry.sha256.as_bytes());
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}

fn live_logo_digests(core: &Core) -> Result<BTreeSet<String>, CoreError> {
    let store = PackageStore::new(core);
    let root = store.checked_root()?;
    let mut digests = BTreeSet::new();
    for namespace in ["cloud", "local", "addons"] {
        let directory = safe_path(&root, namespace).map_err(CoreError::Other)?;
        let entries = match fs::symlink_metadata(&directory) {
            Ok(meta) if meta.is_dir() && !crate::package_fs::is_link(&meta) => {
                fs::read_dir(&directory)?
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            _ => return Err(CoreError::Other("Invalid logo cache package source".into())),
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if crate::package_fs::is_link(&metadata) {
                return Err(CoreError::Other(
                    "Logo cache package source contains a link".into(),
                ));
            }
            if !metadata.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let key = name
                .to_str()
                .ok_or_else(|| CoreError::Other("Invalid logo cache package key".into()))?;
            if key.starts_with(".old-") {
                return Err(CoreError::Other("Package publication is pending".into()));
            }
            if namespace == "addons" {
                let (stored, _) =
                    crate::addon_store::file_inventory(&store.checked_addon_path(key)?, key)?;
                digests.insert(addon_logo_digest(&stored));
            } else {
                if store.checked_package_path(key)? != path {
                    return Err(CoreError::Other("Invalid logo cache package path".into()));
                }
                let metadata = store.checked_metadata_path(key)?;
                if !fs::symlink_metadata(&metadata)
                    .is_ok_and(|meta| meta.is_file() && !crate::package_fs::is_link(&meta))
                {
                    return Err(CoreError::Other(
                        "Missing logo cache package metadata".into(),
                    ));
                }
                let meta = PackageStore::read_meta_at(&metadata, key)?
                    .ok_or_else(|| CoreError::Other("Missing logo cache package metadata".into()))?;
                digests.insert(meta.zip_sha256.to_ascii_lowercase());
            }
        }
    }
    Ok(digests)
}

fn cache_file(path: &Path) -> Option<fs::Metadata> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_file()
        || crate::package_fs::is_link(&meta)
        || crate::package_activation::user_file(&name)
        || name.contains(".leveldb")
    {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() != 1 {
            return None;
        }
    }
    Some(meta)
}

fn owned_logo_cache_entry(path: &Path) -> Option<()> {
    let entries: Vec<_> = fs::read_dir(path).ok()?.collect::<Result<_, _>>().ok()?;
    let mut inputs = BTreeSet::new();
    let mut outputs = BTreeSet::from([".rendering".to_owned(), ".skipped".to_owned()]);
    for entry in &entries {
        let name = entry.file_name().into_string().ok()?;
        let Some(id) = name.strip_suffix(".schema.yaml") else {
            continue;
        };
        validate_relative(id).ok()?;
        let path = entry.path();
        if cache_file(&path)?.len() > MAX_SOURCE_BYTES {
            return None;
        }
        let mut bytes = Vec::new();
        fs::File::open(path)
            .ok()?
            .take(MAX_SOURCE_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() as u64 > MAX_SOURCE_BYTES {
            return None;
        }
        let schema: serde_yaml::Value = serde_yaml::from_slice(&bytes).ok()?;
        if schema["schema"]["schema_id"]
            .as_str()
            .is_some_and(|value| value != id)
        {
            return None;
        }
        inputs.extend(keytao_schema_logo::schema_logo_candidates(
            id,
            schema["schema"]["name"].as_str().unwrap_or(""),
            schema["schema"]["icon"].as_str(),
        ));
        outputs.insert(format!("{id}.png"));
        outputs.insert(format!("{id}.source"));
        inputs.insert(name);
    }
    if inputs.is_empty() {
        return None;
    }
    for entry in entries {
        let name = entry.file_name().into_string().ok()?;
        if name == ".keytao-logo" {
            let directory = entry.path();
            let meta = fs::symlink_metadata(&directory).ok()?;
            if !meta.is_dir() || crate::package_fs::is_link(&meta) {
                return None;
            }
            for output in fs::read_dir(directory).ok()? {
                let output = output.ok()?;
                let filename = output.file_name().into_string().ok()?;
                if !outputs.contains(&filename) {
                    return None;
                }
                cache_file(&output.path())?;
            }
        } else if inputs.contains(&name) {
            cache_file(&entry.path())?;
        } else {
            return None;
        }
    }
    Some(())
}

pub(crate) fn prune_package_logo_cache(core: &Core) {
    let cache_root = &core.env().cache_dir;
    let cache = cache_root.join(SCHEME_LOGO_CACHE);
    let _package_guard = core
        .pending_package
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _addon_guard = crate::addon_store::operation();
    if ![cache_root.as_path(), cache.as_path()].into_iter().all(|dir| {
        fs::symlink_metadata(dir)
            .is_ok_and(|meta| meta.is_dir() && !crate::package_fs::is_link(&meta))
    }) {
        return;
    }
    let Ok(_lease) = crate::package_fs::file_lease(&cache.join(".lease"), false) else {
        return;
    };
    let Ok(digests) = live_logo_digests(core) else {
        return;
    };
    let Ok(entries) = fs::read_dir(&cache) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(digest) = entry.file_name().into_string() else {
            continue;
        };
        let path = entry.path();
        if digest.len() != 64
            || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            || digests.contains(&digest.to_ascii_lowercase())
            || !fs::symlink_metadata(&path)
                .is_ok_and(|meta| meta.is_dir() && !crate::package_fs::is_link(&meta))
            || owned_logo_cache_entry(&path).is_none()
        {
            continue;
        }
        match fs::remove_dir_all(&path) {
            Ok(()) => tracing::info!(path = %path.display(), "Removed package logo cache"),
            Err(error) => tracing::warn!(path = %path.display(), %error, "Cannot remove package logo cache"),
        }
    }
}

fn stored_package_logo(
    core: &Core,
    store: &PackageStore<'_>,
    scheme_key: &str,
) -> Result<Option<PathBuf>, CoreError> {
    let _guard = core
        .pending_package
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let path = store.checked_package_path(scheme_key)?;
    let (stored, _) = PackageStore::file_inventory_at(&path, scheme_key, false)?;
    render_inventory_logo(core, &path, &stored, &stored.meta.zip_sha256, scheme_key)
}

fn render_inventory_logo(
    core: &Core,
    path: &Path,
    stored: &StoredPackage,
    archive_sha: &str,
    scheme_key: &str,
) -> Result<Option<PathBuf>, CoreError> {
    let files: BTreeMap<_, _> = stored
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect();
    let mut read_failure = None;
    let logo = render_logo(
        &core.env().cache_dir,
        files.keys().copied(),
        |name| {
            let Some(file) = files.get(name) else {
                return Ok(None);
            };
            if file.size > MAX_SOURCE_BYTES {
                return Ok(None);
            }
            safe_path(path, &format!("files/{name}"))
                .map_err(CoreError::Other)
                .and_then(|path| PackageStore::read_verified_file(&path, file))
                .map(Some)
                .map_err(|error| read_failure = Some(error))
        },
        archive_sha,
        scheme_key,
    );
    match read_failure {
        Some(error) => Err(error),
        None => Ok(logo),
    }
}

fn has_installed_real_files(core: &Core, scheme_key: &str) -> bool {
    let store = PackageStore::new(core);
    {
        let _guard = core
            .pending_package
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if store.checked_package_path(scheme_key).is_ok_and(|path| {
            PackageStore::file_inventory_at(&path, scheme_key, false).is_ok()
        }) {
            return true;
        }
    }
    if crate::addon_store::IDS.contains(&scheme_key) {
        return store
            .checked_addon_path(scheme_key)
            .ok()
            .is_some_and(|dir| crate::addon_store::file_inventory(&dir, scheme_key).is_ok());
    }
    false
}

#[derive(Serialize, Deserialize)]
struct RemoteLogoCacheMeta {
    repo_url: String,
    cached_at_unix: u64,
    has_ico: bool,
    #[serde(default)]
    ico_path: Option<String>,
}

enum RemoteCacheLookup {
    Fresh(Option<PathBuf>),
    StaleOrMissing,
}

fn prune_expired_remote_logos(cache_root: &Path, now_unix: u64) {
    let Ok(base) = safe_path(cache_root, REMOTE_CACHE_NAMESPACE) else {
        return;
    };
    let Ok(entries) = fs::read_dir(&base) else {
        return;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let expired = safe_path(&dir, "meta.json")
            .ok()
            .and_then(|meta| fs::read(meta).ok())
            .and_then(|bytes| serde_json::from_slice::<RemoteLogoCacheMeta>(&bytes).ok())
            .is_none_or(|meta| now_unix.saturating_sub(meta.cached_at_unix) >= REMOTE_LOGO_TTL_SECS);
        if expired {
            let _ = crate::package_fs::remove_dir_all(&dir);
        }
    }
}

fn read_remote_logo_cache(
    cache_root: &Path,
    scheme_key: &str,
    repo_url: &str,
    now_unix: u64,
) -> RemoteCacheLookup {
    let Ok(dir) = safe_path(cache_root, &format!("{REMOTE_CACHE_NAMESPACE}/{scheme_key}")) else {
        return RemoteCacheLookup::StaleOrMissing;
    };
    let Ok(meta_path) = safe_path(&dir, "meta.json") else {
        return RemoteCacheLookup::StaleOrMissing;
    };
    let Ok(bytes) = fs::read(&meta_path) else {
        return RemoteCacheLookup::StaleOrMissing;
    };
    let Ok(meta) = serde_json::from_slice::<RemoteLogoCacheMeta>(&bytes) else {
        let _ = crate::package_fs::remove_dir_all(&dir);
        return RemoteCacheLookup::StaleOrMissing;
    };
    if meta.repo_url != repo_url
        || now_unix.saturating_sub(meta.cached_at_unix) >= REMOTE_LOGO_TTL_SECS
    {
        let _ = crate::package_fs::remove_dir_all(&dir);
        return RemoteCacheLookup::StaleOrMissing;
    }
    if !meta.has_ico {
        return RemoteCacheLookup::Fresh(None);
    }
    match safe_path(&dir, "logo.png") {
        Ok(png) if png.is_file() => RemoteCacheLookup::Fresh(Some(png)),
        _ => {
            let _ = crate::package_fs::remove_dir_all(&dir);
            RemoteCacheLookup::StaleOrMissing
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ParsedLogoRepo {
    host: LogoRepoHost,
    owner: String,
    repo: String,
    branch: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LogoRepoHost {
    GitHub,
    Gitee,
}

fn parse_logo_repo_url(raw: &str, branch_override: Option<&str>) -> Result<ParsedLogoRepo, String> {
    let trimmed = raw.trim().trim_end_matches('/');
    let url = reqwest::Url::parse(trimmed).map_err(|e| format!("仓库地址无效: {e}"))?;
    if url.scheme() != "https" {
        return Err("仅支持 HTTPS 仓库地址".into());
    }
    let host = match url.host_str() {
        Some("github.com") => LogoRepoHost::GitHub,
        Some("gitee.com") => LogoRepoHost::Gitee,
        _ => return Err("仅支持 GitHub 或 Gitee 仓库读取预览图标".into()),
    };
    let segments: Vec<&str> = url
        .path_segments()
        .map(|it| it.filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    if segments.len() < 2 {
        return Err("仓库路径缺少 owner/repo".into());
    }
    let owner = segments[0];
    let repo = segments[1].strip_suffix(".git").unwrap_or(segments[1]);
    let valid_seg = |s: &str| {
        !s.is_empty()
            && s != "."
            && s != ".."
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    if !valid_seg(owner) || !valid_seg(repo) {
        return Err("仓库 owner/repo 名称无效".into());
    }
    let url_branch = if segments.len() >= 4 && segments[2] == "tree" {
        Some(segments[3..].join("/"))
    } else {
        None
    };
    let branch = branch_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or(url_branch);
    if let Some(ref br) = branch {
        validate_relative(br)?;
    }
    Ok(ParsedLogoRepo {
        host,
        owner: owner.to_owned(),
        repo: repo.to_owned(),
        branch,
    })
}

fn rank_ico_paths(scheme_key: &str, paths: Vec<String>) -> Vec<String> {
    let normalized_key = scheme_key
        .strip_prefix("local-")
        .unwrap_or(scheme_key)
        .replace('-', "_")
        .to_ascii_lowercase();
    let short_key = normalized_key
        .split('_')
        .next()
        .unwrap_or(normalized_key.as_str())
        .to_owned();
    let mut valid: Vec<String> = paths
        .into_iter()
        .filter(|path| {
            validate_relative(path).is_ok()
                && Path::new(path)
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("ico"))
        })
        .collect();
    valid.sort_by_key(|path| {
        let depth = path.matches('/').count();
        let file = path.rsplit('/').next().unwrap_or(path);
        let stem = file
            .get(..file.len().saturating_sub(4))
            .unwrap_or(file)
            .to_ascii_lowercase();
        let is_ascii_indicator = scheme_key != "easy_en"
            && matches!(
                stem.as_str(),
                "ascii" | "eng" | "en" | "english" | "half" | "full" | "punct"
            );
        let match_score = if stem == normalized_key {
            0usize
        } else if !short_key.is_empty() && stem.starts_with(&short_key) {
            1
        } else if stem == "logo" || stem == "icon" {
            2
        } else {
            3
        };
        (
            usize::from(is_ascii_indicator),
            depth,
            match_score,
            path.len(),
            path.clone(),
        )
    });
    valid.dedup();
    valid
}

#[derive(Clone, Debug)]
struct RepoIcoEntry {
    git_ref: String,
    path: String,
}

async fn download_small_bytes(
    client: &reqwest::Client,
    url: &str,
    max_bytes: u64,
) -> Result<Vec<u8>, String> {
    let response = client
        .get(url)
        .timeout(Duration::from_secs(12))
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    if response.content_length().is_some_and(|len| len > max_bytes) {
        return Err("file exceeds size limit".into());
    }
    let mut stream = response.bytes_stream();
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.without_url().to_string())?;
        if (out.len() + chunk.len()) as u64 > max_bytes {
            return Err("file exceeds size limit".into());
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

fn parse_github_html_ico_entries(html: &str, owner: &str, repo: &str) -> Vec<RepoIcoEntry> {
    let needle = format!("/{owner}/{repo}/blob/");
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(pos) = rest.find(&needle) {
        rest = &rest[pos + needle.len()..];
        let end = rest
            .find(['"', '\'', '?', '#', '<', '>', ' '])
            .unwrap_or(rest.len());
        let tail = &rest[..end];
        if let Some((git_ref, path)) = tail.split_once('/')
            && !git_ref.is_empty()
            && validate_relative(git_ref).is_ok()
            && validate_relative(path).is_ok()
            && path.to_ascii_lowercase().ends_with(".ico")
        {
            out.push(RepoIcoEntry {
                git_ref: git_ref.to_owned(),
                path: path.to_owned(),
            });
        }
    }
    out
}

async fn list_repo_icos_network(
    client: &reqwest::Client,
    parsed: &ParsedLogoRepo,
) -> Result<Vec<RepoIcoEntry>, String> {
    match parsed.host {
        LogoRepoHost::GitHub => {
            let git_ref = parsed.branch.as_deref().unwrap_or("HEAD");
            let api_url = format!(
                "https://api.github.com/repos/{}/{}/git/trees/{git_ref}?recursive=1",
                parsed.owner, parsed.repo
            );
            let resp = client
                .get(&api_url)
                .header(reqwest::header::ACCEPT, "application/vnd.github+json")
                .timeout(Duration::from_secs(10))
                .send()
                .await
                .map_err(|e| e.without_url().to_string())?;
            if resp.status().is_success() {
                let json: serde_json::Value =
                    resp.json().await.map_err(|e| e.without_url().to_string())?;
                let mut entries = Vec::new();
                if let Some(tree) = json["tree"].as_array() {
                    for item in tree {
                        if item["type"].as_str() != Some("blob") {
                            continue;
                        }
                        if item["size"].as_u64().is_some_and(|s| s > MAX_SOURCE_BYTES) {
                            continue;
                        }
                        if let Some(path) = item["path"].as_str()
                            && path.to_ascii_lowercase().ends_with(".ico")
                            && validate_relative(path).is_ok()
                        {
                            entries.push(RepoIcoEntry {
                                git_ref: git_ref.to_owned(),
                                path: path.to_owned(),
                            });
                        }
                    }
                }
                return Ok(entries);
            }
            if matches!(
                resp.status(),
                reqwest::StatusCode::FORBIDDEN | reqwest::StatusCode::TOO_MANY_REQUESTS
            ) {
                let page_url = match &parsed.branch {
                    Some(br) => {
                        format!("https://github.com/{}/{}/tree/{br}", parsed.owner, parsed.repo)
                    }
                    None => format!("https://github.com/{}/{}", parsed.owner, parsed.repo),
                };
                let bytes = download_small_bytes(client, &page_url, 2 * 1024 * 1024).await?;
                let html = String::from_utf8_lossy(&bytes);
                return Ok(parse_github_html_ico_entries(
                    &html,
                    &parsed.owner,
                    &parsed.repo,
                ));
            }
            Err(format!("GitHub HTTP {}", resp.status()))
        }
        LogoRepoHost::Gitee => {
            let refs: Vec<&str> = match parsed.branch.as_deref() {
                Some(br) => vec![br],
                None => vec!["master", "main"],
            };
            let mut last_err = String::from("Gitee tree lookup failed");
            for git_ref in refs {
                let api_url = format!(
                    "https://gitee.com/api/v5/repos/{}/{}/git/trees/{git_ref}?recursive=1",
                    parsed.owner, parsed.repo
                );
                let resp = match client
                    .get(&api_url)
                    .timeout(Duration::from_secs(10))
                    .send()
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        last_err = e.without_url().to_string();
                        continue;
                    }
                };
                if !resp.status().is_success() {
                    last_err = format!("Gitee HTTP {}", resp.status());
                    continue;
                }
                let json: serde_json::Value =
                    resp.json().await.map_err(|e| e.without_url().to_string())?;
                let mut entries = Vec::new();
                if let Some(tree) = json["tree"].as_array() {
                    for item in tree {
                        if item["type"].as_str() != Some("blob") {
                            continue;
                        }
                        if let Some(path) = item["path"].as_str()
                            && path.to_ascii_lowercase().ends_with(".ico")
                            && validate_relative(path).is_ok()
                        {
                            entries.push(RepoIcoEntry {
                                git_ref: git_ref.to_owned(),
                                path: path.to_owned(),
                            });
                        }
                    }
                }
                return Ok(entries);
            }
            Err(last_err)
        }
    }
}

fn raw_ico_download_url(parsed: &ParsedLogoRepo, entry: &RepoIcoEntry) -> String {
    match parsed.host {
        LogoRepoHost::GitHub => format!(
            "https://raw.githubusercontent.com/{}/{}/{}/{}",
            parsed.owner, parsed.repo, entry.git_ref, entry.path
        ),
        LogoRepoHost::Gitee => format!(
            "https://gitee.com/{}/{}/raw/{}/{}",
            parsed.owner, parsed.repo, entry.git_ref, entry.path
        ),
    }
}

pub async fn remote_scheme_logo(
    core: &Core,
    scheme_key: &str,
    repo_url: &str,
    branch: Option<&str>,
) -> Result<Option<String>, CoreError> {
    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let has_real = has_installed_real_files(core, scheme_key);
    let real_logo = if has_real {
        package_logo_path(core, scheme_key)
    } else {
        None
    };
    let parsed = parse_logo_repo_url(repo_url, branch).map_err(CoreError::Other)?;
    let client = crate::scheme::build_client(core).map_err(CoreError::Other)?;
    let parsed_for_list = parsed.clone();
    let client_for_list = client.clone();
    remote_scheme_logo_with(
        &core.env().cache_dir,
        now_unix,
        has_real,
        real_logo,
        scheme_key,
        repo_url.trim(),
        || async move { list_repo_icos_network(&client_for_list, &parsed_for_list).await },
        |entry| {
            let client = client.clone();
            let url = raw_ico_download_url(&parsed, &entry);
            async move { download_small_bytes(&client, &url, MAX_SOURCE_BYTES).await }
        },
    )
    .await
    .map(|opt| opt.map(|p| p.to_string_lossy().into_owned()))
    .map_err(CoreError::Other)
}

async fn remote_scheme_logo_with<ListFn, ListFut, DlFn, DlFut>(
    cache_root: &Path,
    now_unix: u64,
    has_real_files: bool,
    real_logo: Option<PathBuf>,
    scheme_key: &str,
    repo_url: &str,
    list_icos: ListFn,
    mut download_ico: DlFn,
) -> Result<Option<PathBuf>, String>
where
    ListFn: FnOnce() -> ListFut,
    ListFut: std::future::Future<Output = Result<Vec<RepoIcoEntry>, String>>,
    DlFn: FnMut(RepoIcoEntry) -> DlFut,
    DlFut: std::future::Future<Output = Result<Vec<u8>, String>>,
{
    validate_relative(scheme_key)?;
    if scheme_key.contains('/') {
        return Err("Invalid scheme key".into());
    }
    prune_expired_remote_logos(cache_root, now_unix);

    let cache_dir = safe_path(cache_root, &format!("{REMOTE_CACHE_NAMESPACE}/{scheme_key}"))?;
    if has_real_files {
        if cache_dir.exists() {
            let _ = crate::package_fs::remove_dir_all(&cache_dir);
        }
        return Ok(real_logo);
    }

    if let RemoteCacheLookup::Fresh(cached) =
        read_remote_logo_cache(cache_root, scheme_key, repo_url, now_unix)
    {
        return Ok(cached);
    }

    let entries = list_icos().await?;
    let mut by_path: BTreeMap<String, RepoIcoEntry> = BTreeMap::new();
    for entry in entries {
        by_path.entry(entry.path.clone()).or_insert(entry);
    }
    let ranked = rank_ico_paths(scheme_key, by_path.keys().cloned().collect());

    fs::create_dir_all(&cache_dir).map_err(|e| e.to_string())?;
    for path in ranked {
        let Some(entry) = by_path.remove(&path) else {
            continue;
        };
        let Ok(ico_bytes) = download_ico(entry).await else {
            continue;
        };
        let Some(png_bytes) = keytao_schema_logo::render_ico_bytes(&ico_bytes, 192) else {
            continue;
        };
        let ico_file = safe_path(&cache_dir, "icon.ico")?;
        let png_file = safe_path(&cache_dir, "logo.png")?;
        let meta_file = safe_path(&cache_dir, "meta.json")?;
        crate::scheme_files::write_file_atomic(&ico_file, &ico_bytes)
            .map_err(|e| e.to_string())?;
        crate::scheme_files::write_file_atomic(&png_file, &png_bytes)
            .map_err(|e| e.to_string())?;
        let meta = RemoteLogoCacheMeta {
            repo_url: repo_url.to_owned(),
            cached_at_unix: now_unix,
            has_ico: true,
            ico_path: Some(path),
        };
        crate::scheme_files::write_file_atomic(
            &meta_file,
            &serde_json::to_vec(&meta).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        return Ok(Some(png_file));
    }

    let meta_file = safe_path(&cache_dir, "meta.json")?;
    let meta = RemoteLogoCacheMeta {
        repo_url: repo_url.to_owned(),
        cached_at_unix: now_unix,
        has_ico: false,
        ico_path: None,
    };
    crate::scheme_files::write_file_atomic(
        &meta_file,
        &serde_json::to_vec(&meta).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(None)
}

#[cfg(test)]
fn read_entry<'a>(files: &'a BTreeMap<String, Vec<u8>>, name: &str) -> Option<&'a [u8]> {
    validate_relative(name).ok()?;
    let bytes = files.get(name)?;
    (bytes.len() as u64 <= MAX_SOURCE_BYTES).then_some(bytes)
}

#[cfg(test)]
fn render_files_logo(
    cache_root: &Path,
    files: Vec<(String, Vec<u8>)>,
    archive_sha: &str,
    scheme_key: &str,
) -> Option<PathBuf> {
    if files.len() > 10_000 {
        return None;
    }
    let files: BTreeMap<_, _> = files.into_iter().collect();
    render_logo(
        cache_root,
        files.keys().map(String::as_str),
        |name| Ok(read_entry(&files, name).map(<[u8]>::to_vec)),
        archive_sha,
        scheme_key,
    )
}

fn render_logo<'a>(
    cache_root: &Path,
    paths: impl Iterator<Item = &'a str>,
    mut read: impl FnMut(&str) -> Result<Option<Vec<u8>>, ()>,
    archive_sha: &str,
    scheme_key: &str,
) -> Option<PathBuf> {
    let mut selected = None;
    for config in ["default.custom.yaml", "default-custom.yaml", "default.yaml"] {
        let Some(content) = read(config).ok()? else {
            continue;
        };
        let Ok(content) = std::str::from_utf8(&content) else {
            continue;
        };
        selected = crate::scheme_files::parse_schema_list(content)
            .into_iter()
            .next()
            .map(|id| format!("{id}.schema.yaml"));
        if selected.is_some() {
            break;
        }
    }
    let selected = selected.or_else(|| {
        let matching = format!("{scheme_key}.schema.yaml");
        let schemas: Vec<_> = paths.filter(|name| name.ends_with(".schema.yaml")).collect();
        if schemas.contains(&matching.as_str()) {
            return Some(matching);
        }
        (schemas.len() == 1).then(|| schemas[0].to_owned())
    })?;
    let schema_bytes = read(&selected).ok()??;
    let schema: serde_yaml::Value = serde_yaml::from_slice(&schema_bytes).ok()?;
    let id = schema["schema"]["schema_id"]
        .as_str()
        .unwrap_or(selected.strip_suffix(".schema.yaml")?);
    validate_relative(id).ok()?;
    if id.contains('/') {
        return None;
    }
    let name = schema["schema"]["name"].as_str().unwrap_or("");
    let icon = schema["schema"]["icon"].as_str();
    let base = safe_path(cache_root, SCHEME_LOGO_CACHE).ok()?;
    fs::create_dir_all(&base).ok()?;
    let _lease = crate::package_fs::file_lease(&base.join(".lease"), true).ok()?;
    let cache = safe_path(&base, archive_sha).ok()?;
    fs::create_dir_all(&cache).ok()?;
    let output = safe_path(&cache, &format!(".keytao-logo/{id}.png")).ok()?;
    let parent = Path::new(&selected).parent()?;
    let mut icons = Vec::new();
    for candidate in keytao_schema_logo::schema_logo_candidates(id, name, icon) {
        let source = parent.join(&candidate);
        let Some(source) = source.to_str() else {
            continue;
        };
        if let Some(content) = read(source).ok()? {
            icons.push((candidate, content));
        }
    }
    if output.is_file() {
        return Some(output);
    }
    let schema_path = safe_path(&cache, &format!("{id}.schema.yaml")).ok()?;
    crate::scheme_files::write_file_atomic(&schema_path, &schema_bytes).ok()?;
    for (candidate, content) in icons {
        let path = safe_path(&cache, &candidate).ok()?;
        crate::scheme_files::write_file_atomic(&path, &content).ok()?;
    }
    keytao_schema_logo::render_schema_logos(&cache, &[(id.into(), name.into())], 192);
    output.is_file().then_some(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};

    fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, bytes) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn render(cache: &Path, bytes: &[u8]) -> Option<PathBuf> {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
        let mut files = Vec::new();
        for index in 0..archive.len() {
            let file = archive.by_index(index).ok()?;
            let name = file.name().to_owned();
            if file.is_dir()
                || file.size() > MAX_SOURCE_BYTES
                || file
                    .unix_mode()
                    .is_some_and(|mode| mode & 0o170000 == 0o120000)
            {
                continue;
            }
            let mut content = Vec::new();
            file.take(MAX_SOURCE_BYTES + 1)
                .read_to_end(&mut content)
                .ok()?;
            files.push((name, content));
        }
        render_files_logo(
            cache,
            files,
            &format!("{:x}", Sha256::digest(bytes)),
            "package",
        )
    }

    const RED: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32"><rect width="32" height="32" fill="red"/></svg>"#;
    const BLUE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32"><rect width="32" height="32" fill="blue"/></svg>"#;

    fn test_core(root: &Path) -> std::sync::Arc<Core> {
        struct Sink;
        impl crate::EventSink for Sink {
            fn emit(&self, _: crate::CoreEvent) {}
        }
        Core::new(
            crate::AppEnv {
                data_dir: root.join("data"),
                cache_dir: root.join("cache"),
                resource_dir: root.join("resources"),
                app_version: "test".into(),
                platform: crate::Platform::Android,
            },
            std::sync::Arc::new(Sink),
        )
        .unwrap()
    }

    #[test]
    fn stored_directory_logo_is_verified_before_cached_output_is_reused() {
        let dir = tempfile::tempdir().unwrap();
        let core = test_core(dir.path());
        let bytes = archive(&[
            ("keytao.schema.yaml", b"schema:\n  schema_id: keytao\n"),
            ("keytao.svg", RED),
            ("unused.dict.yaml", b"original"),
        ]);
        crate::packages::store_cloud_archive(&core, "keytao", Some("v1"), "official", None, &bytes, dir.path()).unwrap();
        let store = PackageStore::new(&core);
        assert!(store.package_path("keytao").join("files/keytao.svg").is_file());
        let path = package_logo_path(&core, "keytao").unwrap();
        assert!(fs::read(&path).unwrap().starts_with(b"\x89PNG\r\n\x1a\n"));
        fs::write(
            store.package_path("keytao").join("files/unused.dict.yaml"),
            b"modified",
        )
        .unwrap();
        assert_eq!(package_logo_path(&core, "keytao"), Some(path));
        fs::write(store.package_path("keytao").join("files/keytao.svg"), BLUE).unwrap();
        assert!(package_logo_path(&core, "keytao").is_none());
    }

    #[test]
    fn prunes_unreferenced_logo_digests_and_retains_live_and_protected_entries() {
        let dir = tempfile::tempdir().unwrap();
        let core = test_core(dir.path());
        let schema = b"schema:\n  schema_id: keytao\n";
        let cache = &core.env().cache_dir;
        let old = archive(&[("keytao.schema.yaml", schema), ("keytao.svg", RED)]);
        let current = archive(&[("keytao.schema.yaml", schema), ("keytao.svg", BLUE)]);
        crate::packages::store_cloud_archive(&core, "keytao", Some("v1"), "official", None, &old, dir.path()).unwrap();
        let old_logo = package_logo_path(&core, "keytao").unwrap();
        crate::packages::store_cloud_archive(&core, "keytao", Some("v2"), "official", None, &current, dir.path()).unwrap();
        let current_logo = package_logo_path(&core, "keytao").unwrap();

        let foreign = archive(&[("other.schema.yaml", b"schema:\n  schema_id: other\n"), ("other.svg", BLUE)]);
        let foreign_logo = render(cache, &foreign).unwrap();
        let protected = foreign_logo.parent().unwrap().parent().unwrap().join("personal.userdb");
        fs::write(&protected, b"dictionary").unwrap();
        #[cfg(unix)]
        let linked = {
            let linked = cache.join(SCHEME_LOGO_CACHE).join("f".repeat(64));
            std::os::unix::fs::symlink(foreign_logo.parent().unwrap().parent().unwrap(), &linked).unwrap();
            linked
        };

        prune_package_logo_cache(&core);
        assert!(!old_logo.parent().unwrap().parent().unwrap().exists());
        assert!(current_logo.is_file());
        assert!(foreign_logo.is_file());
        assert_eq!(fs::read(&protected).unwrap(), b"dictionary");
        #[cfg(unix)]
        assert!(fs::symlink_metadata(linked).unwrap().file_type().is_symlink());
    }

    #[test]
    fn extracts_primary_schema_icon_with_shared_lookup_and_archive_sha_cache() {
        let dir = tempfile::tempdir().unwrap();
        let schema = b"schema:\n  schema_id: id\n  name: Name\n  icon: own.svg\n";
        let bytes = archive(&[
            (
                "default.custom.yaml",
                b"patch:\n  schema_list:\n    - schema: id\n",
            ),
            ("aux.schema.yaml", b"schema:\n  schema_id: aux\n"),
            ("id.schema.yaml", schema),
            ("own.svg", RED),
            ("id.svg", BLUE),
        ]);
        let path = render(dir.path(), &bytes).unwrap();
        assert!(
            path.to_string_lossy()
                .contains(&format!("{:x}", Sha256::digest(&bytes)))
        );
        let first = fs::read(&path).unwrap();
        assert!(first.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(render(dir.path(), &bytes), Some(path.clone()));
        let changed = archive(&[("id.schema.yaml", schema), ("own.svg", BLUE)]);
        let changed_path = render(dir.path(), &changed).unwrap();
        assert_ne!(path, changed_path);
        assert_ne!(first, fs::read(changed_path).unwrap());
        let fallback = archive(&[("id.schema.yaml", schema), ("id.svg", RED)]);
        assert_eq!(
            first,
            fs::read(render(dir.path(), &fallback).unwrap()).unwrap()
        );
    }

    #[test]
    fn missing_unsafe_oversized_and_invalid_archive_icons_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let oversized = vec![0u8; MAX_SOURCE_BYTES as usize + 1];
        for (schema, icon_name, icon_bytes) in [
            (
                b"schema:\n  schema_id: id\n".as_slice(),
                "unrelated.svg",
                RED,
            ),
            (
                b"schema:\n  schema_id: id\n  icon: ../escape.svg\n",
                "../escape.svg",
                RED,
            ),
            (
                b"schema:\n  schema_id: id\n  icon: own.svg\n",
                "own.svg",
                oversized.as_slice(),
            ),
            (
                b"schema:\n  schema_id: id\n  icon: own.svg\n",
                "own.svg",
                b"invalid",
            ),
            (
                b"schema:\n  schema_id: id\n  icon: own.png\n",
                "own.png",
                RED,
            ),
        ] {
            let bytes = archive(&[("id.schema.yaml", schema), (icon_name, icon_bytes)]);
            assert!(render(dir.path(), &bytes).is_none(), "{icon_name}");
        }
        assert!(!dir.path().join("escape.svg").exists());
        let missing_primary = archive(&[
            (
                "default.custom.yaml",
                b"patch:\n  schema_list:\n    - schema: missing\n    - schema: english\n",
            ),
            ("english.schema.yaml", b"schema:\n  schema_id: english\n"),
            ("english.svg", RED),
        ]);
        assert!(render(dir.path(), &missing_primary).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_icon_and_cache_escape_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file("id.schema.yaml", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"schema:\n  schema_id: id\n  icon: own.svg\n")
            .unwrap();
        zip.add_symlink(
            "own.svg",
            "outside.svg",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        assert!(render(dir.path(), &bytes).is_none());
        let bytes = archive(&[
            ("id.schema.yaml", b"schema:\n  schema_id: id\n"),
            ("id.svg", RED),
        ]);
        let cache = dir.path().join("scheme-logos-v2");
        fs::create_dir_all(&cache).unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(
            outside.path(),
            cache.join(format!("{:x}", Sha256::digest(&bytes))),
        )
        .unwrap();
        assert!(render(dir.path(), &bytes).is_none());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    fn png_wrapped_ico(svg_bytes: &[u8]) -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        let bytes = archive(&[
            ("tmp.schema.yaml", b"schema:\n  schema_id: tmp\n"),
            ("tmp.svg", svg_bytes),
        ]);
        let png_path = render(dir.path(), &bytes).unwrap();
        let png = fs::read(png_path).unwrap();
        let mut ico = Vec::with_capacity(22 + png.len());
        ico.extend_from_slice(&[0, 0, 1, 0, 1, 0]);
        ico.extend_from_slice(&[192, 192, 0, 0, 1, 0, 32, 0]);
        ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
        ico.extend_from_slice(&22u32.to_le_bytes());
        ico.extend_from_slice(&png);
        ico
    }

    #[tokio::test]
    async fn remote_ico_downloads_only_ico_caches_for_two_days_and_yields_to_real_package() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let red_ico = png_wrapped_ico(RED);
        let blue_ico = png_wrapped_ico(BLUE);
        let repo = "https://github.com/hugh7007/xmjd6-rere";

        let list_calls = Arc::new(AtomicUsize::new(0));
        let dl_paths = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));

        // 1. Uninstalled: lists repo files, ignores ascii.ico/yaml, downloads ONLY xmjd6.ico.
        let t0 = 1_700_000_000u64;
        let first = {
            let list_calls = list_calls.clone();
            let dl_paths = dl_paths.clone();
            let ico = red_ico.clone();
            remote_scheme_logo_with(
                &cache,
                t0,
                false,
                None,
                "xmjd",
                repo,
                move || {
                    list_calls.fetch_add(1, Ordering::SeqCst);
                    async move {
                        Ok(vec![
                            RepoIcoEntry {
                                git_ref: "HEAD".into(),
                                path: "ascii.ico".into(),
                            },
                            RepoIcoEntry {
                                git_ref: "HEAD".into(),
                                path: "xmjd6.ico".into(),
                            },
                        ])
                    }
                },
                move |entry| {
                    dl_paths.lock().unwrap().push(entry.path.clone());
                    let ico = ico.clone();
                    async move { Ok(ico) }
                },
            )
            .await
            .unwrap()
            .unwrap()
        };
        assert_eq!(list_calls.load(Ordering::SeqCst), 1);
        assert_eq!(*dl_paths.lock().unwrap(), ["xmjd6.ico"]);
        let first_bytes = fs::read(&first).unwrap();
        assert!(first_bytes.starts_with(b"\x89PNG\r\n\x1a\n"));

        // 2. Within 2 days (t0 + 2d - 1s): hits cache without calling list or download!
        let cached = remote_scheme_logo_with(
            &cache,
            t0 + REMOTE_LOGO_TTL_SECS - 1,
            false,
            None,
            "xmjd",
            repo,
            || async { panic!("must not list while cache is fresh") },
            |_| async { panic!("must not download while cache is fresh") },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(cached, first);

        // 3. At 2 days (t0 + 2d): cache automatically expires and re-fetches updated .ico!
        let refreshed = {
            let list_calls = list_calls.clone();
            let ico = blue_ico.clone();
            remote_scheme_logo_with(
                &cache,
                t0 + REMOTE_LOGO_TTL_SECS,
                false,
                None,
                "xmjd",
                repo,
                move || {
                    list_calls.fetch_add(1, Ordering::SeqCst);
                    async move {
                        Ok(vec![RepoIcoEntry {
                            git_ref: "HEAD".into(),
                            path: "xmjd6.ico".into(),
                        }])
                    }
                },
                move |_| {
                    let ico = ico.clone();
                    async move { Ok(ico) }
                },
            )
            .await
            .unwrap()
            .unwrap()
        };
        assert_eq!(list_calls.load(Ordering::SeqCst), 2);
        assert_ne!(fs::read(&refreshed).unwrap(), first_bytes);

        // 4. Once real package files are installed: remote cache dir is removed and real logo wins!
        let real_path = dir.path().join("real-logo.png");
        fs::write(&real_path, b"real").unwrap();
        let resolved = remote_scheme_logo_with(
            &cache,
            t0 + REMOTE_LOGO_TTL_SECS + 10,
            true,
            Some(real_path.clone()),
            "xmjd",
            repo,
            || async { panic!("must not query network when real package exists") },
            |_| async { panic!("must not download when real package exists") },
        )
        .await
        .unwrap();
        assert_eq!(resolved, Some(real_path));
        assert!(!cache.join(REMOTE_CACHE_NAMESPACE).join("xmjd").exists());
    }

    #[test]
    fn github_html_fallback_extracts_only_safe_ico_blob_links() {
        let html = r#"
          <a href="/wzxmer/rime-txjx/blob/main/txjx.ico">txjx.ico</a>
          <a href="/wzxmer/rime-txjx/blob/main/txjx.schema.yaml">schema</a>
          <a href="/wzxmer/rime-txjx/blob/main/../evil.ico">bad</a>
        "#;
        let entries = parse_github_html_ico_entries(html, "wzxmer", "rime-txjx");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].git_ref, "main");
        assert_eq!(entries[0].path, "txjx.ico");
    }
}
