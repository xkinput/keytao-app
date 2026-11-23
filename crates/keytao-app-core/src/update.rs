use crate::{Core, CoreError, scheme_types::AppUpdateInfo};
use reqwest::{Client, Url, header::LOCATION, redirect::Policy};
use semver::Version;
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
mod linux;
mod package;
#[cfg(windows)]
mod windows;
pub(crate) use package::VerifiedPackage;
pub use package::{download_app_update, open_app_update_package};

const RELEASES: &str = "https://github.com/xkinput/keytao-app/releases";
const RELEASE_TAG: &str = "https://github.com/xkinput/keytao-app/releases/tag/";

fn parse_version(tag: &str) -> Option<Version> {
    Version::parse(tag.trim_start_matches('v')).ok()
}

/// Picks the highest-semver release (tag, html_url) and whether it is newer than `current`.
/// Unparsable versions never report an update.
pub(crate) fn pick_update(current: &str, releases: &[(String, String)]) -> (String, bool, String) {
    let best = releases
        .iter()
        .filter_map(|(tag, url)| parse_version(tag).map(|v| (v, url)))
        .max_by(|a, b| a.0.cmp(&b.0));
    let Some((latest, url)) = best else {
        return (String::new(), false, String::new());
    };
    let has_update = parse_version(current).is_some_and(|cur| latest > cur);
    (latest.to_string(), has_update, url.clone())
}

async fn fetch_release_response(client: &Client, url: &str) -> Result<reqwest::Response, String> {
    client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| error.to_string())
}

async fn fetch_stable_release(client: &Client, releases: &str) -> Result<(String, String), String> {
    let response = fetch_release_response(client, &format!("{releases}/latest")).await?;
    let location = response
        .headers()
        .get(LOCATION)
        .filter(|_| response.status().is_redirection())
        .and_then(|value| value.to_str().ok())
        .ok_or("GitHub 未返回正式版发布地址")?;
    let url = Url::parse(&format!("{RELEASES}/latest"))
        .and_then(|base| base.join(location))
        .map_err(|_| "GitHub 正式版发布地址无效")?;
    let tag = url
        .as_str()
        .strip_prefix(RELEASE_TAG)
        .filter(|tag| parse_version(tag).is_some_and(|version| version.pre.is_empty()))
        .ok_or("GitHub 正式版发布地址无效")?;
    Ok((tag.to_owned(), url.into()))
}

async fn fetch_release_page(client: &Client, url: &str) -> Result<String, String> {
    let response = fetch_release_response(client, url).await?;
    if !response.status().is_success() {
        return Err(format!("GitHub 发布列表返回 HTTP {}", response.status()));
    }
    response.text().await.map_err(|error| error.to_string())
}

fn published_releases(page: &str) -> impl Iterator<Item = (String, String)> {
    // shortcut: the public cards exclude unpublished tags, until GitHub offers an equivalent feed.
    page.split("<section ").skip(1).filter_map(|section| {
        let attributes = section.split_once('>')?.0;
        let tag = attributes.split_ascii_whitespace().find_map(|attribute| {
            attribute
                .strip_prefix("data-release-anchor=\"release-")?
                .strip_suffix('"')
        })?;
        parse_version(tag)?;
        Some((tag.to_owned(), format!("{RELEASE_TAG}{tag}")))
    })
}

async fn check_app_update_at(
    client: &Client,
    releases: &str,
    current: &str,
) -> Result<AppUpdateInfo, String> {
    let entries = if parse_version(current).is_some_and(|v| !v.pre.is_empty()) {
        let older_page = format!("{releases}?page=2");
        let (first, second) = tokio::try_join!(
            fetch_release_page(client, releases),
            fetch_release_page(client, &older_page),
        )?;
        published_releases(&first)
            .chain(published_releases(&second))
            .collect()
    } else {
        vec![fetch_stable_release(client, releases).await?]
    };
    let (latest, has_update, release_url) = pick_update(current, &entries);
    if latest.is_empty() {
        return Err("GitHub 发布列表中没有有效版本".into());
    }
    Ok(AppUpdateInfo {
        current_version: current.to_owned(),
        latest_version: latest,
        has_update,
        release_url,
        install_mode: None,
    })
}

pub async fn check_app_update(core: &Core) -> Result<AppUpdateInfo, CoreError> {
    let t0 = Instant::now();
    let current = &core.env().app_version;
    let client = Client::builder()
        .user_agent(format!("keytao-app/{current}"))
        .timeout(Duration::from_secs(8))
        .https_only(true)
        .redirect(Policy::none())
        .build()
        .map_err(|error| CoreError::Other(error.to_string()))?;
    let result = check_app_update_at(&client, RELEASES, current).await;
    match &result {
        Ok(_) => tracing::info!("[check_app_update] done in {}ms", t0.elapsed().as_millis()),
        Err(error) => tracing::warn!(
            "[check_app_update] failed after {}ms: {error}",
            t0.elapsed().as_millis()
        ),
    }
    let mut info = result.map_err(CoreError::Other)?;
    if !info.has_update {
        return Ok(info);
    }
    tokio::task::spawn_blocking(move || {
        info.install_mode = Some(package::install_mode(&info).into());
        info
    })
    .await
    .map_err(|error| CoreError::Other(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{AppUpdateInfo, Client, Duration, Policy, check_app_update_at, pick_update};
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::TcpListener,
    };

    async fn mock_update(current: &str, routes: &[(&str, &str)]) -> Result<AppUpdateInfo, String> {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let client = Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let base = format!("http://{address}/releases");
        let serve = async {
            for _ in routes {
                let (stream, _) = listener.accept().await.unwrap();
                let mut stream = BufReader::new(stream);
                let mut line = String::new();
                stream.read_line(&mut line).await.unwrap();
                let path = line.split_whitespace().nth(1).unwrap();
                let response = routes.iter().find(|(route, _)| *route == path).unwrap().1;
                stream
                    .get_mut()
                    .write_all(response.as_bytes())
                    .await
                    .unwrap();
            }
        };
        let query = check_app_update_at(&client, &base, current);
        tokio::pin!(query);
        tokio::select! {
            result = &mut query => result,
            () = serve => query.await,
        }
    }

    #[tokio::test]
    async fn stable_checks_the_latest_release_redirect() {
        let update = mock_update("1.2.14", &[(
            "/releases/latest",
            "HTTP/1.1 302 Found\r\nLocation: tag/v1.2.15\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )]).await.unwrap();
        assert!(update.has_update);
        assert_eq!(update.latest_version, "1.2.15");
        assert_eq!(
            update.release_url,
            "https://github.com/xkinput/keytao-app/releases/tag/v1.2.15"
        );
    }

    #[tokio::test]
    async fn alpha_uses_published_releases_from_both_pages() {
        let update = mock_update("1.2.16-alpha.1", &[
            ("/releases", concat!(
                "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n",
                "<section data-release-anchor=\"release-v1.2.16-alpha.2\">",
                "<a href=\"/xkinput/keytao-app/releases/tag/v9.9.9\">Mentioned in notes</a></section>",
            )),
            ("/releases?page=2", concat!(
                "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n",
                "<section data-release-anchor=\"release-v1.2.16-alpha.3\"></section>",
            )),
        ]).await.unwrap();
        assert!(update.has_update);
        assert_eq!(update.latest_version, "1.2.16-alpha.3");
        assert_eq!(
            update.release_url,
            "https://github.com/xkinput/keytao-app/releases/tag/v1.2.16-alpha.3"
        );
    }

    #[tokio::test]
    async fn github_http_errors_are_not_reported_as_no_update() {
        let error = mock_update(
            "1.2.15",
            &[(
                "/releases/latest",
                "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )],
        )
        .await
        .err()
        .unwrap();
        assert!(error.contains("403"), "{error}");
    }

    fn rel(tags: &[&str]) -> Vec<(String, String)> {
        tags.iter()
            .map(|t| (t.to_string(), format!("https://example/{t}")))
            .collect()
    }

    #[test]
    fn alpha_does_not_downgrade_to_older_stable() {
        let (latest, has, _) = pick_update("1.2.1-alpha.92", &rel(&["v1.2.0"]));
        assert_eq!(latest, "1.2.0");
        assert!(!has);
    }

    #[test]
    fn alpha_updates_to_newer_alpha() {
        let (latest, has, url) =
            pick_update("1.2.1-alpha.92", &rel(&["v1.2.1-alpha.93", "v1.2.0"]));
        assert_eq!(latest, "1.2.1-alpha.93");
        assert!(has);
        assert_eq!(url, "https://example/v1.2.1-alpha.93");
    }

    #[test]
    fn alpha_updates_to_same_stable() {
        let (latest, has, _) = pick_update("1.2.1-alpha.92", &rel(&["v1.2.1"]));
        assert_eq!(latest, "1.2.1");
        assert!(has);
    }

    #[test]
    fn stable_equal_is_no_update() {
        assert!(!pick_update("1.2.0", &rel(&["v1.2.0"])).1);
    }

    #[test]
    fn unparsable_current_is_no_update() {
        assert!(!pick_update("dev", &rel(&["v9.9.9"])).1);
    }

    #[test]
    fn unparsable_tags_are_skipped() {
        let (latest, has, _) = pick_update("1.2.0", &rel(&["nightly", "garbage"]));
        assert_eq!(latest, "");
        assert!(!has);
    }

    #[test]
    fn numeric_prerelease_ordering() {
        let (latest, has, _) = pick_update(
            "1.2.1-alpha.92",
            &rel(&["v1.2.1-alpha.100", "v1.2.1-alpha.99"]),
        );
        assert_eq!(latest, "1.2.1-alpha.100");
        assert!(has);
    }
}
