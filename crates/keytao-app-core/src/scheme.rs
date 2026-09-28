use crate::{Core, CoreError, scheme_files::parse_schema_list, scheme_types::*};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
const API_BASE: &str = "https://keytao.rea.ink";

#[derive(Serialize, Deserialize, Clone)]
struct ReleaseCache {
    etag: String,
    cached_at: u64,
    release: ReleaseInfo,
}

pub fn build_client(core: &Core) -> Result<reqwest::Client, String> {
    let version = core.env().app_version.clone();
    reqwest::Client::builder()
        .user_agent(format!("keytao-app/{version}"))
        .connect_timeout(std::time::Duration::from_secs(8))
        .build()
        .map_err(|e| e.to_string())
}

fn parse_download_urls(obj: &serde_json::Value) -> DownloadUrls {
    let urls = &obj["downloadUrls"];
    DownloadUrls {
        macos: urls["macos"].as_str().map(|s| s.to_string()),
        windows: urls["windows"].as_str().map(|s| s.to_string()),
        linux: urls["linux"].as_str().map(|s| s.to_string()),
        android: urls["android"].as_str().map(|s| s.to_string()),
        ios: urls["ios"].as_str().map(|s| s.to_string()),
    }
}

pub async fn fetch_latest_release(core: &Core) -> Result<ReleaseInfo, CoreError> {
    let result: Result<_, String> = async {
        let t0 = std::time::Instant::now();
        tracing::info!("[fetch_latest_release] start");
        let cache_path = Some(core.env().cache_dir.join("release_cache.json"));

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Check disk cache (5 min TTL, no ETag for our proxy API)
        let cached: Option<ReleaseCache> = cache_path.as_ref().and_then(|p| {
            std::fs::read_to_string(p)
                .ok()
                .and_then(|s| serde_json::from_str::<ReleaseCache>(&s).ok())
        });
        if let Some(ref c) = cached {
            if now.saturating_sub(c.cached_at) < 300 {
                tracing::info!(
                    "[fetch_latest_release] cache hit, {}ms",
                    t0.elapsed().as_millis()
                );
                return Ok(c.release.clone());
            }
        }

        let client = build_client(core)?;
        let url = format!("{API_BASE}/api/install/latest-release");

        let response = client
            .get(&url)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| format!("网络请求失败: {e}"))?;

        if !response.status().is_success() {
            tracing::warn!(
                "[fetch_latest_release] HTTP {} after {}ms",
                response.status(),
                t0.elapsed().as_millis()
            );
            if let Some(c) = cached {
                return Ok(c.release);
            }
            return Err(format!("获取版本信息失败，HTTP {}", response.status()));
        }

        let data: serde_json::Value = response.json().await.map_err(|e| {
            tracing::warn!(
                "[fetch_latest_release] body parse failed after {}ms: {e}",
                t0.elapsed().as_millis()
            );
            format!("解析响应失败: {e}")
        })?;

        let github_version = data["github"]["version"].as_str().unwrap_or("").to_string();
        let github = if !github_version.is_empty() {
            Some(PlatformRelease {
                version: github_version.clone(),
                download_urls: parse_download_urls(&data["github"]),
            })
        } else {
            None
        };

        let gitee_version = data["gitee"]["version"].as_str().unwrap_or("").to_string();
        let gitee = if !gitee_version.is_empty() {
            Some(PlatformRelease {
                version: gitee_version,
                download_urls: parse_download_urls(&data["gitee"]),
            })
        } else {
            None
        };

        let version = data["version"].as_str().unwrap_or("unknown").to_string();
        let name = data["name"].as_str().unwrap_or("").to_string();
        let published_at = data["publishedAt"].as_str().unwrap_or("").to_string();
        let body = data["body"].as_str().unwrap_or("").to_string();

        let info = ReleaseInfo {
            version,
            name,
            published_at,
            body,
            github,
            gitee,
        };

        if let Some(path) = cache_path {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).ok();
            }
            let cache = ReleaseCache {
                etag: String::new(),
                cached_at: now,
                release: info.clone(),
            };
            serde_json::to_string(&cache)
                .ok()
                .and_then(|s| std::fs::write(&path, s).ok());
        }

        tracing::info!(
            "[fetch_latest_release] done in {}ms",
            t0.elapsed().as_millis()
        );
        Ok(info)
    }
    .await;
    result.map_err(CoreError::Other)
}

pub async fn api_error_message(response: reqwest::Response, fallback: &str) -> String {
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
        if let Some(error) = value.get("error").and_then(|v| v.as_str()) {
            return error.to_string();
        }
        if let Some(message) = value.get("message").and_then(|v| v.as_str()) {
            return message.to_string();
        }
    }
    let trimmed = text.trim();
    if trimmed.is_empty() {
        format!("{fallback}，HTTP {status}")
    } else {
        format!("{fallback}，HTTP {status}: {trimmed}")
    }
}

fn validate_scheme_key(scheme: &str) -> Result<&'static str, String> {
    match scheme {
        "keytao" => Ok("keytao"),
        "xmjd" => Ok("xmjd"),
        "txjx" => Ok("txjx"),
        "keydo" => Ok("keydo"),
        _ => Err("不支持的方案".into()),
    }
}

pub async fn fetch_scheme_release(
    core: &Core,
    scheme: String,
) -> Result<SchemeReleaseInfo, CoreError> {
    let result: Result<_, String> = async {
        let scheme = validate_scheme_key(scheme.trim())?;
        let client = build_client(core)?;
        let url = format!("{API_BASE}/api/practice/scheme-release?scheme={scheme}");
        let response = client
            .get(&url)
            .timeout(std::time::Duration::from_secs(12))
            .send()
            .await
            .map_err(|e| format!("获取方案版本失败: {e}"))?;

        if !response.status().is_success() {
            return Err(api_error_message(response, "获取方案版本失败").await);
        }

        response
            .json::<SchemeReleaseInfo>()
            .await
            .map_err(|e| format!("解析方案版本失败: {e}"))
    }
    .await;
    result.map_err(CoreError::Other)
}

pub fn list_dir(_core: &Core, path: String) -> Result<Vec<FileItem>, CoreError> {
    let result: Result<_, String> = (|| {
        let entries = std::fs::read_dir(&path).map_err(|e| format!("读取目录失败: {e}"))?;
        let mut items: Vec<FileItem> = entries
            .filter_map(|e| e.ok())
            .map(|e| FileItem {
                name: e.file_name().to_string_lossy().into_owned(),
                is_dir: e.file_type().map(|t| t.is_dir()).unwrap_or(false),
            })
            .collect();
        items.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.cmp(&b.name),
        });
        Ok(items)
    })();
    result.map_err(CoreError::Other)
}

pub fn read_local_schemas(_core: &Core, path: String) -> Result<Vec<String>, CoreError> {
    let base = std::path::Path::new(&path);
    let content = std::fs::read_to_string(base.join("default.custom.yaml"))
        .or_else(|_| std::fs::read_to_string(base.join("default-custom.yaml")))
        .unwrap_or_default();
    Ok(parse_schema_list(&content))
}

pub fn check_local_schema(
    _core: &Core,
    dir: Option<PathBuf>,
) -> Result<LocalSchemaInfo, CoreError> {
    let Some(dir) = dir else {
        return Ok(LocalSchemaInfo {
            installed: false,
            deployed: false,
            version: None,
            schemas: vec![],
        });
    };

    let state = keytao_core::schema_install_state(&dir);
    let version = state
        .installed
        .then(|| {
            std::fs::read_to_string(dir.join("version.txt"))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .flatten();

    Ok(LocalSchemaInfo {
        installed: state.installed,
        deployed: state.deployed,
        version,
        schemas: state.schemas,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NullSink;
    impl crate::EventSink for NullSink {
        fn emit(&self, _: crate::CoreEvent) {}
    }

    #[tokio::test]
    async fn fresh_release_cache_preserves_all_source_payloads_without_network() {
        let dir = tempfile::tempdir().unwrap();
        let core = Core::new(
            crate::AppEnv {
                data_dir: dir.path().join("state"),
                cache_dir: dir.path().to_owned(),
                resource_dir: dir.path().join("resources"),
                app_version: "test".into(),
                platform: crate::Platform::MacOs,
            },
            std::sync::Arc::new(NullSink),
        )
        .unwrap();
        let release = serde_json::json!({
            "version": "v1", "name": "release", "published_at": "date", "body": "notes",
            "github": { "version": "g1", "download_urls": {
                "macos": "github-mac", "windows": "github-win", "linux": null, "android": null, "ios": null
            } },
            "gitee": { "version": "g2", "download_urls": {
                "macos": null, "windows": null, "linux": "gitee-linux", "android": "gitee-android", "ios": "gitee-ios"
            } }
        });
        let cache = serde_json::json!({
            "etag": "legacy-etag", "cached_at": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
            "release": release,
        });
        let path = dir.path().join("release_cache.json");
        let bytes = serde_json::to_vec(&cache).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let actual = fetch_latest_release(&core).await.unwrap();
        assert_eq!(serde_json::to_value(actual).unwrap(), release);
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}
