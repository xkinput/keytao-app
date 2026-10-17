use crate::{Core, CoreError, scheme::build_client, scheme_types::AppUpdateInfo};
use semver::Version;

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

pub async fn check_app_update(core: &Core) -> Result<AppUpdateInfo, CoreError> {
    let result: Result<_, String> = async {
        let t0 = std::time::Instant::now();
        tracing::info!("[check_app_update] start");
        let current = core.env().app_version.clone();
        let prerelease = parse_version(&current).is_some_and(|v| !v.pre.is_empty());
        let url = if prerelease {
            "https://api.github.com/repos/xkinput/keytao-app/releases?per_page=20"
        } else {
            "https://api.github.com/repos/xkinput/keytao-app/releases/latest"
        };
        let client = build_client(core)?;
        let resp = client
            .get(url)
            .timeout(std::time::Duration::from_secs(8))
            .send()
            .await
            .map_err(|e| {
                tracing::warn!(
                    "[check_app_update] failed after {}ms: {e}",
                    t0.elapsed().as_millis()
                );
                e.to_string()
            })?;
        let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        tracing::info!("[check_app_update] done in {}ms", t0.elapsed().as_millis());
        let entries = match json.as_array() {
            Some(list) => list.iter().collect::<Vec<_>>(),
            None => vec![&json],
        };
        let releases: Vec<(String, String)> = entries
            .into_iter()
            .filter(|r| !r["draft"].as_bool().unwrap_or(false))
            .filter_map(|r| {
                Some((
                    r["tag_name"].as_str()?.to_string(),
                    r["html_url"].as_str().unwrap_or("").to_string(),
                ))
            })
            .collect();
        let (latest, has_update, release_url) = pick_update(&current, &releases);
        Ok(AppUpdateInfo {
            current_version: current,
            latest_version: latest,
            has_update,
            release_url,
        })
    }
    .await;
    result.map_err(CoreError::Other)
}

#[cfg(test)]
mod tests {
    use super::pick_update;

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
