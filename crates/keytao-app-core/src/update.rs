use crate::{Core, CoreError, scheme::build_client, scheme_types::AppUpdateInfo};

pub async fn check_app_update(core: &Core) -> Result<AppUpdateInfo, CoreError> {
    let result: Result<_, String> = async {
        let t0 = std::time::Instant::now();
        tracing::info!("[check_app_update] start");
        let current = core.env().app_version.clone();
        let client = build_client(core)?;
        let resp = client
            .get("https://api.github.com/repos/xkinput/keytao-app/releases/latest")
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
        let latest_tag = json["tag_name"].as_str().unwrap_or("").to_string();
        let latest = latest_tag.trim_start_matches('v').to_string();
        let release_url = json["html_url"].as_str().unwrap_or("").to_string();
        let has_update = !latest.is_empty() && latest != current;
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
