use super::{
    PackageMeta,
    local_import::{LocalImportOptions, has_root_schema, import_local_scheme_with},
    validate_relative,
};
use crate::{
    Core, CoreError,
    download::{DOWNLOAD_CANCELLED, download_to_temp_with, emit_progress},
    package_store::PackageStore,
    scheme::build_client,
};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::Duration,
};

const MAX_ARCHIVE_FILES: usize = 10_000;
const MAX_EXPANDED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_UNWRAP_DEPTH: usize = 3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitSchemeMode {
    #[default]
    Release,
    Branch,
}

impl GitSchemeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::Branch => "branch",
        }
    }

    pub fn parse(value: &str) -> Result<Self, CoreError> {
        match value.trim() {
            "" | "release" => Ok(Self::Release),
            "branch" | "archive" => Ok(Self::Branch),
            other => Err(CoreError::Other(format!("不支持的下载方式：{other}"))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitSchemeConfig {
    pub repo_url: String,
    #[serde(default)]
    pub mode: GitSchemeMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_name: Option<String>,
}

#[derive(Clone, Debug)]
enum ParsedHost {
    GitHub {
        api_base: String,
        web_base: String,
        codeload_base: String,
        owner: String,
        repo: String,
    },
    Gitee {
        api_base: String,
        web_base: String,
        owner: String,
        repo: String,
    },
    Generic {
        url: String,
        slug: String,
    },
}

impl ParsedHost {
    fn slug(&self) -> &str {
        match self {
            Self::GitHub { repo, .. } | Self::Gitee { repo, .. } => repo,
            Self::Generic { slug, .. } => slug,
        }
    }
}

struct ResolvedDownload {
    download_url: String,
    version_hint: Option<String>,
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

fn validate_token(label: &str, value: &str) -> Result<(), CoreError> {
    if value.is_empty()
        || value.len() > 200
        || value.contains('\0')
        || value.contains('\\')
        || value.contains("..")
        || value.chars().any(char::is_control)
    {
        return Err(CoreError::Other(format!("{label}格式无效")));
    }
    Ok(())
}

fn valid_repo_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment.len() <= 100
        && segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn allowed_url_scheme(url: &Url) -> bool {
    if url.scheme() == "https" {
        return true;
    }
    #[cfg(test)]
    if url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "localhost")) {
        return true;
    }
    false
}

fn normalize_config(raw: GitSchemeConfig) -> Result<(GitSchemeConfig, ParsedHost), CoreError> {
    let raw_url = raw.repo_url.trim();
    if raw_url.is_empty() {
        return Err(CoreError::Other("请输入 Git 仓库链接".into()));
    }
    let expanded = if !raw_url.contains("://") && raw_url.matches('/').count() == 1 {
        let (owner, repo) = raw_url.split_once('/').unwrap();
        let repo = repo.strip_suffix(".git").unwrap_or(repo);
        if valid_repo_segment(owner) && valid_repo_segment(repo) {
            format!("https://github.com/{owner}/{repo}")
        } else {
            return Err(CoreError::Other("请输入有效的 Git 仓库链接".into()));
        }
    } else {
        raw_url.to_owned()
    };

    let url = Url::parse(&expanded).map_err(|_| CoreError::Other("请输入有效的 Git 仓库链接".into()))?;
    if !allowed_url_scheme(&url)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
    {
        return Err(CoreError::Other("Git 链接仅支持 HTTPS 地址".into()));
    }

    let mut mode = raw.mode;
    let mut branch = clean_optional(raw.branch);
    let mut asset_name = clean_optional(raw.asset_name);

    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let origin = format!(
        "{}://{}{}",
        url.scheme(),
        url.host_str().unwrap_or_default(),
        url.port().map_or_else(String::new, |port| format!(":{port}"))
    );
    let segments: Vec<&str> = url
        .path_segments()
        .map(|parts| parts.filter(|part| !part.is_empty()).collect())
        .unwrap_or_default();

    #[cfg(test)]
    let test_github = matches!(host.as_str(), "127.0.0.1" | "localhost")
        && segments.first().copied() == Some("github");
    #[cfg(not(test))]
    let test_github = false;

    if host == "github.com" || host == "gitee.com" || test_github {
        let offset = usize::from(test_github);
        if segments.len() < offset + 2 {
            return Err(CoreError::Other("请输入完整的仓库地址（包含 owner/repo）".into()));
        }
        let owner = segments[offset];
        let repo = segments[offset + 1]
            .strip_suffix(".git")
            .unwrap_or(segments[offset + 1]);
        if !valid_repo_segment(owner) || !valid_repo_segment(repo) {
            return Err(CoreError::Other("仓库地址格式无效".into()));
        }
        let rest = &segments[offset + 2..];
        if rest.len() >= 2 && rest[0] == "tree" {
            mode = GitSchemeMode::Branch;
            if branch.is_none() {
                branch = Some(rest[1..].join("/"));
            }
        } else if rest.len() == 4
            && rest[0] == "releases"
            && rest[1] == "latest"
            && rest[2] == "download"
        {
            mode = GitSchemeMode::Release;
            if asset_name.is_none() {
                asset_name = Some(rest[3].to_owned());
            }
        } else if rest.len() == 4 && rest[0] == "releases" && rest[1] == "download" {
            mode = GitSchemeMode::Release;
            if asset_name.is_none() {
                asset_name = Some(format!("{}/{}", rest[2], rest[3]));
            }
        }

        if let Some(ref b) = branch {
            validate_token("分支名称", b)?;
        }
        if let Some(ref a) = asset_name {
            validate_token("文件名", a)?;
        }

        let (repo_url, parsed) = if test_github {
            let base = format!("{origin}/github");
            (
                format!("{base}/{owner}/{repo}"),
                ParsedHost::GitHub {
                    api_base: format!("{base}/api"),
                    web_base: base.clone(),
                    codeload_base: format!("{base}/codeload"),
                    owner: owner.to_owned(),
                    repo: repo.to_owned(),
                },
            )
        } else if host == "github.com" {
            (
                format!("https://github.com/{owner}/{repo}"),
                ParsedHost::GitHub {
                    api_base: "https://api.github.com".into(),
                    web_base: "https://github.com".into(),
                    codeload_base: "https://codeload.github.com".into(),
                    owner: owner.to_owned(),
                    repo: repo.to_owned(),
                },
            )
        } else {
            (
                format!("https://gitee.com/{owner}/{repo}"),
                ParsedHost::Gitee {
                    api_base: "https://gitee.com/api/v5".into(),
                    web_base: "https://gitee.com".into(),
                    owner: owner.to_owned(),
                    repo: repo.to_owned(),
                },
            )
        };

        let normalized = GitSchemeConfig {
            repo_url,
            mode,
            branch: if mode == GitSchemeMode::Branch {
                branch
            } else {
                None
            },
            asset_name: if mode == GitSchemeMode::Release {
                asset_name
            } else {
                None
            },
        };
        return Ok((normalized, parsed));
    }

    if let Some(ref b) = branch {
        validate_token("分支名称", b)?;
    }
    if let Some(ref a) = asset_name {
        validate_token("文件名", a)?;
    }

    let clean_url = expanded
        .trim_end_matches('/')
        .strip_suffix(".git")
        .unwrap_or_else(|| expanded.trim_end_matches('/'))
        .to_owned();
    let raw_slug = segments
        .last()
        .copied()
        .unwrap_or("scheme")
        .strip_suffix(".git")
        .unwrap_or_else(|| segments.last().copied().unwrap_or("scheme"))
        .strip_suffix(".tar.gz")
        .or_else(|| segments.last().copied().unwrap_or("scheme").strip_suffix(".tar.xz"))
        .or_else(|| segments.last().copied().unwrap_or("scheme").strip_suffix(".tgz"))
        .or_else(|| segments.last().copied().unwrap_or("scheme").strip_suffix(".zip"))
        .unwrap_or_else(|| segments.last().copied().unwrap_or("scheme"))
        .to_owned();

    let normalized = GitSchemeConfig {
        repo_url: clean_url.clone(),
        mode,
        branch: if mode == GitSchemeMode::Branch {
            branch
        } else {
            None
        },
        asset_name: if mode == GitSchemeMode::Release {
            asset_name
        } else {
            None
        },
    };
    Ok((
        normalized,
        ParsedHost::Generic {
            url: clean_url,
            slug: raw_slug,
        },
    ))
}

fn is_archive_asset_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    (lower.ends_with(".zip")
        || lower.ends_with(".tar.gz")
        || lower.ends_with(".tgz")
        || lower.ends_with(".tar.xz"))
        && !lower.ends_with(".sha256")
        && !lower.ends_with(".sig")
        && !lower.ends_with(".asc")
}

fn matches_pattern(pattern: &str, candidate: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == candidate || pattern.eq_ignore_ascii_case(candidate);
    }
    let pattern = pattern.to_ascii_lowercase();
    let candidate = candidate.to_ascii_lowercase();
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == candidate;
    }
    let mut remainder = candidate.as_str();
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if index == 0 {
            let Some(rest) = remainder.strip_prefix(part) else {
                return false;
            };
            remainder = rest;
        } else if index + 1 == parts.len() {
            return remainder.ends_with(part);
        } else {
            let Some(pos) = remainder.find(part) else {
                return false;
            };
            remainder = &remainder[pos + part.len()..];
        }
    }
    true
}

fn pick_release_asset(
    release: &serde_json::Value,
    asset_name: Option<&str>,
    repo_name: &str,
    fallback_zipball: Option<String>,
) -> Option<(String, Option<String>)> {
    let tag = release["tag_name"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let assets = release["assets"].as_array();

    if let Some(pattern) = asset_name {
        let matched = assets.and_then(|list| {
            list.iter()
                .find(|asset| {
                    asset["name"]
                        .as_str()
                        .is_some_and(|name| name == pattern)
                })
                .or_else(|| {
                    list.iter().find(|asset| {
                        asset["name"]
                            .as_str()
                            .is_some_and(|name| matches_pattern(pattern, name))
                    })
                })
        })?;
        let url = matched["browser_download_url"]
            .as_str()
            .filter(|s| !s.is_empty())?
            .to_owned();
        return Some((url, tag));
    }

    if let Some(list) = assets {
        let mut archive_assets: Vec<(&str, &str)> = list
            .iter()
            .filter_map(|asset| {
                let name = asset["name"].as_str()?;
                let url = asset["browser_download_url"].as_str()?;
                if !url.is_empty() && is_archive_asset_name(name) {
                    Some((name, url))
                } else {
                    None
                }
            })
            .collect();
        if !archive_assets.is_empty() {
            let repo_lower = repo_name.to_ascii_lowercase();
            archive_assets.sort_by_key(|(name, _)| {
                let lower = name.to_ascii_lowercase();
                (
                    lower.contains("source"),
                    !lower.ends_with(".zip"),
                    !(lower.contains(&repo_lower)
                        || lower.contains("full")
                        || lower.contains("base")),
                    name.len(),
                )
            });
            return Some((archive_assets[0].1.to_owned(), tag));
        }
    }

    let zipball = release["zipball_url"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or(fallback_zipball)?;
    Some((zipball, tag))
}

enum ApiResult {
    Json(serde_json::Value),
    NotFound,
    RateLimited,
}

async fn fetch_api_json(client: &reqwest::Client, url: &str) -> Result<ApiResult, CoreError> {
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json, application/json")
        .timeout(Duration::from_secs(12))
        .send()
        .await
        .map_err(|error| CoreError::Other(format!("请求仓库接口失败：{}", error.without_url())))?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(ApiResult::NotFound);
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || (status == reqwest::StatusCode::FORBIDDEN
            && response
                .headers()
                .get("x-ratelimit-remaining")
                .and_then(|v| v.to_str().ok())
                == Some("0"))
    {
        return Ok(ApiResult::RateLimited);
    }
    if !status.is_success() {
        return Err(CoreError::Other(format!("请求仓库接口失败：HTTP {status}")));
    }
    let json = response
        .json::<serde_json::Value>()
        .await
        .map_err(|error| CoreError::Other(format!("解析仓库接口响应失败：{}", error.without_url())))?;
    Ok(ApiResult::Json(json))
}

async fn resolve_download(
    core: &Core,
    config: &GitSchemeConfig,
    host: &ParsedHost,
) -> Result<ResolvedDownload, CoreError> {
    let client = build_client(core).map_err(CoreError::Other)?;
    match host {
        ParsedHost::GitHub {
            api_base,
            web_base,
            codeload_base,
            owner,
            repo,
        } => match config.mode {
            GitSchemeMode::Release => {
                if let Some((tag, file)) = config
                    .asset_name
                    .as_deref()
                    .and_then(|spec| spec.split_once('/'))
                {
                    return Ok(ResolvedDownload {
                        download_url: format!("{web_base}/{owner}/{repo}/releases/download/{tag}/{file}"),
                        version_hint: Some(tag.to_owned()),
                    });
                }

                let latest_url = format!("{api_base}/repos/{owner}/{repo}/releases/latest");
                match fetch_api_json(&client, &latest_url).await? {
                    ApiResult::Json(release) => {
                        let fallback = release["tag_name"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .map(|tag| format!("{codeload_base}/{owner}/{repo}/zip/refs/tags/{tag}"));
                        if let Some((download_url, version_hint)) = pick_release_asset(
                            &release,
                            config.asset_name.as_deref(),
                            repo,
                            fallback,
                        ) {
                            return Ok(ResolvedDownload {
                                download_url,
                                version_hint,
                            });
                        }
                    }
                    ApiResult::RateLimited => {
                        if let Some(exact) = config
                            .asset_name
                            .as_deref()
                            .filter(|name| !name.contains('*'))
                        {
                            return Ok(ResolvedDownload {
                                download_url: format!(
                                    "{web_base}/{owner}/{repo}/releases/latest/download/{exact}"
                                ),
                                version_hint: None,
                            });
                        }
                        return Err(CoreError::Other(
                            "GitHub API 请求受限，可在「指定文件名」填写固定 Release 文件名后直接下载"
                                .into(),
                        ));
                    }
                    ApiResult::NotFound => {}
                }

                let list_url = format!("{api_base}/repos/{owner}/{repo}/releases?per_page=15");
                if let ApiResult::Json(releases) = fetch_api_json(&client, &list_url).await? {
                    if let Some(array) = releases.as_array() {
                        for release in array.iter().filter(|item| item["draft"] != true) {
                            let fallback = release["tag_name"]
                                .as_str()
                                .filter(|s| !s.is_empty())
                                .map(|tag| format!("{codeload_base}/{owner}/{repo}/zip/refs/tags/{tag}"));
                            if let Some((download_url, version_hint)) = pick_release_asset(
                                release,
                                config.asset_name.as_deref(),
                                repo,
                                fallback,
                            ) {
                                return Ok(ResolvedDownload {
                                    download_url,
                                    version_hint,
                                });
                            }
                        }
                    }
                }

                if let Some(asset) = &config.asset_name {
                    Err(CoreError::Other(format!(
                        "未在仓库 Release 中找到匹配文件：{asset}"
                    )))
                } else {
                    Err(CoreError::Other("该仓库暂无可用的 Release 发布包".into()))
                }
            }
            GitSchemeMode::Branch => {
                let mut branch = config.branch.clone();
                if branch.is_none() {
                    let repo_api = format!("{api_base}/repos/{owner}/{repo}");
                    if let Ok(ApiResult::Json(info)) = fetch_api_json(&client, &repo_api).await {
                        branch = info["default_branch"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned);
                    }
                }
                let mut version_hint = None;
                if let Some(ref b) = branch {
                    let commit_api = format!("{api_base}/repos/{owner}/{repo}/commits/{b}");
                    if let Ok(ApiResult::Json(commit)) = fetch_api_json(&client, &commit_api).await {
                        version_hint = commit["sha"]
                            .as_str()
                            .filter(|sha| sha.len() >= 7)
                            .map(|sha| sha[..sha.len().min(12)].to_owned());
                    }
                }
                let download_url = match branch {
                    Some(b) => format!("{codeload_base}/{owner}/{repo}/zip/refs/heads/{b}"),
                    None => format!("{web_base}/{owner}/{repo}/archive/HEAD.zip"),
                };
                Ok(ResolvedDownload {
                    download_url,
                    version_hint,
                })
            }
        },
        ParsedHost::Gitee {
            api_base,
            web_base,
            owner,
            repo,
        } => match config.mode {
            GitSchemeMode::Release => {
                let latest_url = format!("{api_base}/repos/{owner}/{repo}/releases/latest");
                if let ApiResult::Json(release) = fetch_api_json(&client, &latest_url).await? {
                    let fallback = release["tag_name"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(|tag| format!("{web_base}/{owner}/{repo}/repository/archive/{tag}.zip"));
                    if let Some((download_url, version_hint)) = pick_release_asset(
                        &release,
                        config.asset_name.as_deref(),
                        repo,
                        fallback,
                    ) {
                        return Ok(ResolvedDownload {
                            download_url,
                            version_hint,
                        });
                    }
                }
                let list_url = format!("{api_base}/repos/{owner}/{repo}/releases?page=1&per_page=10");
                if let ApiResult::Json(releases) = fetch_api_json(&client, &list_url).await? {
                    if let Some(array) = releases.as_array() {
                        for release in array {
                            let fallback = release["tag_name"]
                                .as_str()
                                .filter(|s| !s.is_empty())
                                .map(|tag| {
                                    format!("{web_base}/{owner}/{repo}/repository/archive/{tag}.zip")
                                });
                            if let Some((download_url, version_hint)) = pick_release_asset(
                                release,
                                config.asset_name.as_deref(),
                                repo,
                                fallback,
                            ) {
                                return Ok(ResolvedDownload {
                                    download_url,
                                    version_hint,
                                });
                            }
                        }
                    }
                }
                if let Some(asset) = &config.asset_name {
                    Err(CoreError::Other(format!(
                        "未在 Gitee Release 中找到匹配文件：{asset}"
                    )))
                } else {
                    Err(CoreError::Other("该 Gitee 仓库暂无可用的 Release 发布包".into()))
                }
            }
            GitSchemeMode::Branch => {
                let mut branch = config.branch.clone();
                if branch.is_none() {
                    let repo_api = format!("{api_base}/repos/{owner}/{repo}");
                    if let Ok(ApiResult::Json(info)) = fetch_api_json(&client, &repo_api).await {
                        branch = info["default_branch"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned);
                    }
                }
                let branch = branch.unwrap_or_else(|| "master".into());
                let commit_api = format!("{api_base}/repos/{owner}/{repo}/commits/{branch}");
                let version_hint = match fetch_api_json(&client, &commit_api).await {
                    Ok(ApiResult::Json(commit)) => commit["sha"]
                        .as_str()
                        .filter(|sha| sha.len() >= 7)
                        .map(|sha| sha[..sha.len().min(12)].to_owned()),
                    _ => None,
                };
                Ok(ResolvedDownload {
                    download_url: format!("{web_base}/{owner}/{repo}/repository/archive/{branch}.zip"),
                    version_hint,
                })
            }
        },
        ParsedHost::Generic { url, slug } => {
            if is_archive_asset_name(url) {
                return Ok(ResolvedDownload {
                    download_url: url.clone(),
                    version_hint: None,
                });
            }
            match config.mode {
                GitSchemeMode::Release => {
                    if let Some(asset) = &config.asset_name {
                        Ok(ResolvedDownload {
                            download_url: format!("{url}/releases/latest/download/{asset}"),
                            version_hint: None,
                        })
                    } else {
                        Err(CoreError::Other(
                            "自定义 Git 服务使用 Release 下载时请填写固定文件名，或改用分支归档".into(),
                        ))
                    }
                }
                GitSchemeMode::Branch => {
                    let branch = config.branch.as_deref().unwrap_or("main");
                    let download_url = if url.contains("gitlab") {
                        format!("{url}/-/archive/{branch}/{slug}-{branch}.zip")
                    } else {
                        format!("{url}/archive/{branch}.zip")
                    };
                    Ok(ResolvedDownload {
                        download_url,
                        version_hint: None,
                    })
                }
            }
        }
    }
}

fn sanitize_archive_entry(raw: &str) -> Result<Option<String>, CoreError> {
    let normalized = raw
        .trim_start_matches("./")
        .trim_start_matches('/')
        .trim_end_matches('/');
    if normalized.is_empty() {
        return Ok(None);
    }
    validate_relative(normalized).map_err(CoreError::Other)?;
    Ok(Some(normalized.to_owned()))
}

fn extract_archive_to_dir(archive_path: &Path, dest: &Path) -> Result<(), CoreError> {
    let mut file = File::open(archive_path)?;
    let mut magic = [0u8; 6];
    let read = file.read(&mut magic)?;
    file.seek(SeekFrom::Start(0))?;

    if read >= 4 && &magic[..2] == b"PK" && matches!(magic[2..4], [3, 4] | [5, 6]) {
        return extract_zip(file, dest);
    }
    if read >= 2 && magic[..2] == [0x1f, 0x8b] {
        let decoder = flate2::read::GzDecoder::new(BufReader::new(file));
        return extract_tar(decoder, dest);
    }
    if read >= 6 && magic == [0xfd, b'7', b'z', b'X', b'Z', 0x00] {
        let decoder = lzma_rust2::XzReader::new_mem_limit(BufReader::new(file), false, 72 * 1024);
        return extract_tar(decoder, dest);
    }
    Err(CoreError::Other(
        "不支持的压缩包格式（仅支持 .zip、.tar.gz、.tar.xz）".into(),
    ))
}

fn extract_zip(file: File, dest: &Path) -> Result<(), CoreError> {
    let mut archive = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|error| CoreError::Other(format!("无法读取压缩包：{error}")))?;
    if archive.len() > MAX_ARCHIVE_FILES {
        return Err(CoreError::Other("方案包文件过多".into()));
    }
    let mut total = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| CoreError::Other(format!("读取压缩包条目失败：{error}")))?;
        let Some(relative) = sanitize_archive_entry(entry.name())? else {
            continue;
        };
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            continue;
        }
        if entry.is_dir() {
            continue;
        }
        let target = super::safe_path(dest, &relative).map_err(CoreError::Other)?;
        let expected = entry.size();
        total = total.saturating_add(expected);
        if total > MAX_EXPANDED_BYTES {
            return Err(CoreError::Other("方案包解压后过大".into()));
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = File::create(&target)?;
        let written = std::io::copy(&mut entry.by_ref().take(expected.saturating_add(1)), &mut out)?;
        if written != expected {
            return Err(CoreError::Other("压缩包文件大小不符".into()));
        }
    }
    Ok(())
}

fn extract_tar(reader: impl Read, dest: &Path) -> Result<(), CoreError> {
    let mut archive = tar::Archive::new(reader.take(MAX_EXPANDED_BYTES.saturating_add(1024 * 1024)));
    let mut count = 0usize;
    let mut total = 0u64;
    for entry in archive
        .entries()
        .map_err(|error| CoreError::Other(format!("读取归档失败：{error}")))?
    {
        let mut entry = entry.map_err(|error| CoreError::Other(format!("读取归档条目失败：{error}")))?;
        count += 1;
        if count > MAX_ARCHIVE_FILES {
            return Err(CoreError::Other("方案包文件过多".into()));
        }
        let kind = entry.header().entry_type();
        if kind.is_dir() || kind.is_pax_global_extensions() || kind.is_pax_local_extensions() {
            continue;
        }
        if !kind.is_file() {
            continue;
        }
        let raw_path = entry
            .path()
            .map_err(|error| CoreError::Other(format!("归档路径无效：{error}")))?
            .to_string_lossy()
            .into_owned();
        let Some(relative) = sanitize_archive_entry(&raw_path)? else {
            continue;
        };
        let target = super::safe_path(dest, &relative).map_err(CoreError::Other)?;
        let expected = entry.size();
        total = total.saturating_add(expected);
        if total > MAX_EXPANDED_BYTES {
            return Err(CoreError::Other("方案包解压后过大".into()));
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = File::create(&target)?;
        let written = std::io::copy(&mut entry.by_ref().take(expected.saturating_add(1)), &mut out)?;
        if written != expected {
            return Err(CoreError::Other("归档文件大小不符".into()));
        }
    }
    Ok(())
}

fn locate_scheme_root(root: &Path) -> Result<PathBuf, CoreError> {
    let mut current = root.to_path_buf();
    for _ in 0..=MAX_UNWRAP_DEPTH {
        if has_root_schema(&current)? {
            return Ok(current);
        }
        let mut subdirs = Vec::new();
        for entry in fs::read_dir(&current)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name_str) = name.to_str() else {
                continue;
            };
            if name_str.starts_with('.') || name_str == "__MACOSX" {
                continue;
            }
            if entry.file_type()?.is_dir() {
                subdirs.push(entry.path());
            }
        }
        let mut with_schema = Vec::new();
        for subdir in &subdirs {
            if has_root_schema(subdir)? {
                with_schema.push(subdir.clone());
            }
        }
        if with_schema.len() == 1 {
            return Ok(with_schema.pop().unwrap());
        }
        if subdirs.len() == 1 {
            current = subdirs.pop().unwrap();
        } else {
            break;
        }
    }
    Err(CoreError::Other(
        "未在下载包中找到 .schema.yaml 方案文件，请检查分支或文件名配置".into(),
    ))
}

pub async fn import_git_scheme(
    core: &Core,
    user_root: &Path,
    config: GitSchemeConfig,
    display_name: &str,
    scheme_key: Option<&str>,
) -> Result<String, CoreError> {
    crate::package_migration::ensure_before_write(core, user_root).await?;

    let (normalized, host) = normalize_config(config)?;
    let mut cancel = core.download_cancel.subscribe();
    cancel.borrow_and_update();

    emit_progress(core, "fetching", 0, "正在解析仓库信息...");
    let resolved = tokio::select! {
        biased;
        _ = cancel.changed() => return Err(CoreError::Other(DOWNLOAD_CANCELLED.into())),
        resolved = resolve_download(core, &normalized, &host) => resolved?,
    };

    let client = build_client(core).map_err(CoreError::Other)?;
    let downloaded = download_to_temp_with(core, client.get(&resolved.download_url), &mut cancel, None, None)
        .await
        .map_err(CoreError::Other)?;

    emit_progress(core, "extracting", 65, "正在解压并校验方案文件...");
    let cache_dir = core.env().cache_dir.clone();
    fs::create_dir_all(&cache_dir)?;
    let stage = tempfile::Builder::new()
        .prefix(".git-scheme-")
        .tempdir_in(&cache_dir)?;
    let archive_path = downloaded.path().to_path_buf();
    let stage_path = stage.path().to_path_buf();
    let scheme_dir = tokio::task::spawn_blocking(move || -> Result<PathBuf, CoreError> {
        extract_archive_to_dir(&archive_path, &stage_path)?;
        locate_scheme_root(&stage_path)
    })
    .await
    .map_err(|error| CoreError::Other(format!("解压任务失败：{error}")))??;

    let origin = normalized.repo_url.clone();
    let slug_hint = host.slug().to_owned();
    let key = import_local_scheme_with(
        core,
        &scheme_dir,
        display_name,
        scheme_key,
        LocalImportOptions {
            slug_hint: Some(&slug_hint),
            version_override: resolved.version_hint.as_deref(),
            origin: Some(&origin),
            git: Some(normalized),
        },
    )?;

    emit_progress(core, "done", 100, "下载完成！");
    Ok(key)
}

pub async fn sync_git_scheme(
    core: &Core,
    user_root: &Path,
    scheme_key: &str,
) -> Result<String, CoreError> {
    if !crate::scheme::is_local_scheme_key(scheme_key) {
        return Err(CoreError::Other("仅支持同步 Git 本地方案".into()));
    }
    let meta: PackageMeta = PackageStore::new(core)
        .read_meta(scheme_key)?
        .ok_or_else(|| CoreError::Other("本地方案不存在".into()))?;
    let git = meta
        .git
        .ok_or_else(|| CoreError::Other("该本地方案未配置 Git 链接".into()))?;
    let display_name = meta.display_name.unwrap_or_default();
    import_git_scheme(core, user_root, git, &display_name, Some(scheme_key)).await
}

pub(super) async fn check_git_scheme_latest_version(
    core: &Core,
    config: &GitSchemeConfig,
) -> Result<Option<String>, CoreError> {
    let (normalized, host) = normalize_config(config.clone())?;
    let resolved = resolve_download(core, &normalized, &host).await?;
    Ok(resolved
        .version_hint
        .filter(|v| !matches!(v.as_str(), "latest" | "nightly")))
}

#[cfg(test)]
#[path = "git_import_tests.rs"]
mod tests;
