use crate::{Core, CoreError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};

const PROFILE_LIMIT: usize = 256 * 1024;

#[derive(Serialize, Deserialize)]
struct ProfilePackage {
    scheme_key: String,
    version: Option<String>,
    source: String,
}

#[derive(Serialize, Deserialize)]
struct SchemeProfile {
    version: u8,
    active_scheme: Option<String>,
    packages: Vec<ProfilePackage>,
    settings: Value,
}

#[cfg(test)]
#[path = "scheme_sync_tests.rs"]
mod tests;

pub fn export_profile(core: &Core) -> Result<Value, CoreError> {
    let state = core.state();
    let packages = state
        .installed_packages
        .into_iter()
        .map(|(scheme_key, package)| {
            crate::scheme::validate_scheme_key(&scheme_key).map_err(CoreError::Other)?;
            let source = if let Some(id) = state.scheme_cloud_ids.get(&scheme_key) {
                format!("cloud:{id}")
            } else if let Some(meta) = crate::packages::stored_metadata(core, &scheme_key)?
                && meta.source == "cloud"
            {
                format!("cloud-sha256:{}", meta.zip_sha256)
            } else {
                "official".into()
            };
            Ok(ProfilePackage {
                scheme_key,
                version: package.version,
                source,
            })
        })
        .collect::<Result<Vec<_>, CoreError>>()?;
    Ok(serde_json::to_value(SchemeProfile {
        version: 1,
        active_scheme: state.active_scheme,
        packages,
        settings: sanitize_settings(&state.scheme_sync_settings),
    })?)
}

fn sanitize_settings(input: &Value) -> Value {
    let mut output = json!({});
    for (key, choices) in [
        ("download_source", &["github", "gitee"][..]),
        (
            "selected_scheme",
            &["keytao", "xmjd", "xmjd-feiding", "txjx", "keydo"][..],
        ),
        ("desktop_english_mode", &["ascii", "schema"][..]),
    ] {
        if input[key].as_str().is_some_and(|v| choices.contains(&v)) {
            output[key] = input[key].clone();
        }
    }
    for (group, booleans, numbers, choices) in [
        (
            "android",
            &[
                "haptics_enabled",
                "key_preview_enabled",
                "key_sound_enabled",
                "key_hint_visible",
                "flick_keys_enabled",
                "number_row_enabled",
                "double_space_period_enabled",
                "floating_portrait_enabled",
                "floating_landscape_enabled",
            ][..],
            &[
                ("haptic_intensity", 1.0, 100.0),
                ("long_press_delay_ms", 100.0, 700.0),
                ("keyboard_height_scale", 85.0, 130.0),
                ("keyboard_height_dp", 160.0, 420.0),
                ("candidate_bar_height_dp", 36.0, 96.0),
                ("swipe_threshold_dp", 12.0, 96.0),
                ("key_sound_volume", 0.0, 100.0),
                ("candidate_font_scale", 0.8, 1.4),
                ("floating_portrait_scale", 70.0, 100.0),
                ("floating_landscape_scale", 45.0, 100.0),
            ][..],
            &[
                ("enter_key_behavior", &["newline", "system"][..]),
                ("delete_speed", &["slow", "standard", "fast"][..]),
                ("english_mode", &["ascii", "schema"][..]),
                (
                    "backspace_gesture_mode",
                    &["selectThenDelete", "immediate"][..],
                ),
            ][..],
        ),
        (
            "ime_ui",
            &["embedded_composition"][..],
            &[("font_size", 10.0, 36.0)][..],
            &[
                ("color_scheme", &["auto", "light", "dark"][..]),
                ("orientation", &["horizontal", "vertical"][..]),
            ][..],
        ),
    ] {
        if !input[group].is_object() {
            continue;
        }
        let mut settings = json!({});
        for key in booleans {
            if input[group][key].is_boolean() {
                settings[key] = input[group][key].clone();
            }
        }
        for (key, min, max) in numbers {
            if input[group][key]
                .as_f64()
                .is_some_and(|v| v.is_finite() && (*min..=*max).contains(&v))
            {
                settings[key] = input[group][key].clone();
            }
        }
        for (key, allowed) in choices {
            if input[group][key]
                .as_str()
                .is_some_and(|v| allowed.contains(&v))
            {
                settings[key] = input[group][key].clone();
            }
        }
        if group == "ime_ui"
            && input[group]["accent_color"].as_str().is_some_and(|v| {
                v.len() == 7 && v.starts_with('#') && v[1..].bytes().all(|b| b.is_ascii_hexdigit())
            })
        {
            settings["accent_color"] = input[group]["accent_color"].clone();
        }
        output[group] = settings;
    }
    output
}

pub fn sync_state(core: &Core) -> Value {
    let state = core.state();
    json!({"settings":sanitize_settings(&state.scheme_sync_settings),
        "follow_account_sync":state.follow_account_sync,"synced_schemes":state.synced_schemes,
        "last_sync_at":state.last_scheme_sync_at})
}

pub fn set_preferences(
    core: &Core,
    follow: Option<bool>,
    scheme_key: Option<&str>,
    sync: Option<bool>,
    settings: Option<Value>,
) -> Result<Value, CoreError> {
    if let Some(key) = scheme_key {
        crate::scheme::validate_scheme_key(key).map_err(CoreError::Other)?;
    }
    core.update_state(|state| {
        if let Some(follow) = follow {
            state.follow_account_sync = follow;
        }
        if let (Some(key), Some(sync)) = (scheme_key, sync) {
            if sync {
                state.synced_schemes.insert(key.into());
            } else {
                state.synced_schemes.remove(key);
            }
        }
        if let Some(settings) = settings {
            state.scheme_sync_settings = sanitize_settings(&settings);
        }
    })?;
    Ok(sync_state(core))
}

fn checked_profile(profile: Value) -> Result<SchemeProfile, CoreError> {
    if serde_json::to_vec(&profile)?.len() > PROFILE_LIMIT {
        return Err(CoreError::Other("云端配置过大".into()));
    }
    let mut profile: SchemeProfile = serde_json::from_value(profile)?;
    if profile.version != 1 {
        return Err(CoreError::Other("不支持的配置版本".into()));
    }
    let mut keys = BTreeSet::new();
    for package in &profile.packages {
        crate::scheme::validate_scheme_key(&package.scheme_key).map_err(CoreError::Other)?;
        let cloud_id_valid = package.source.strip_prefix("cloud:").is_some_and(|id| {
            id.bytes().all(|b| b.is_ascii_digit()) && id.parse::<i32>().is_ok_and(|id| id > 0)
        });
        let cloud_sha_valid = package
            .source
            .strip_prefix("cloud-sha256:")
            .is_some_and(|sha| sha.len() == 64 && sha.bytes().all(|byte| byte.is_ascii_hexdigit()));
        if !keys.insert(&package.scheme_key)
            || (!cloud_id_valid
                && !cloud_sha_valid
                && !matches!(package.source.as_str(), "official" | "github" | "gitee"))
        {
            return Err(CoreError::Other("云端方案来源无效".into()));
        }
    }
    if let Some(key) = &profile.active_scheme {
        crate::scheme::validate_scheme_key(key).map_err(CoreError::Other)?;
        if !keys.contains(key) {
            return Err(CoreError::Other("云端启用方案不在配置中".into()));
        }
    }
    profile.settings = sanitize_settings(&profile.settings);
    Ok(profile)
}

pub async fn apply_profile_at(
    core: &Core,
    profile: Value,
    user_root: &Path,
    api_base: &str,
) -> Result<Value, CoreError> {
    let profile = checked_profile(profile)?;
    core.update_state(|state| state.scheme_sync_settings = profile.settings.clone())?;
    for package in profile.packages {
        if core
            .state()
            .installed_packages
            .contains_key(&package.scheme_key)
        {
            continue;
        }
        if package.source.starts_with("cloud:") || package.source.starts_with("cloud-sha256:") {
            let packages = crate::cloud_packages::list_at(core, api_base).await?;
            let cloud = packages
                .iter()
                .find(|cloud| {
                    cloud.scheme_key == package.scheme_key
                        && (package
                            .source
                            .strip_prefix("cloud:")
                            .is_some_and(|id| cloud.id == id)
                            || package
                                .source
                                .strip_prefix("cloud-sha256:")
                                .is_some_and(|sha| cloud.sha256.eq_ignore_ascii_case(sha)))
                })
                .ok_or_else(|| CoreError::Other("云端方案来源不匹配，请先上传方案包".into()))?;
            crate::cloud_packages::download_at(core, &cloud.id, user_root, api_base).await?;
        } else {
            let source = profile.settings["download_source"]
                .as_str()
                .unwrap_or("gitee");
            crate::packages::download_scheme_package(core, &package.scheme_key, source, user_root)
                .await?;
        }
    }
    Ok(json!({"active_scheme":profile.active_scheme,"settings":profile.settings}))
}

pub async fn upload_profile(core: &Core) -> Result<Value, CoreError> {
    upload_profile_at(core, crate::account::API_BASE).await
}

pub async fn fetch_profile(core: &Core) -> Result<Value, CoreError> {
    fetch_profile_at(core, crate::account::API_BASE).await
}

pub async fn apply_profile(
    core: &Core,
    profile: Value,
    user_root: &Path,
) -> Result<Value, CoreError> {
    apply_profile_at(core, profile, user_root, crate::account::API_BASE).await
}

pub(crate) async fn upload_profile_at(core: &Core, api_base: &str) -> Result<Value, CoreError> {
    let body = json!({"data":export_profile(core)?});
    if serde_json::to_vec(&body)?.len() > PROFILE_LIMIT {
        return Err(CoreError::Other("配置不能超过 256 KiB".into()));
    }
    let response = crate::account::ApiClient::new(core, api_base)
        .request(reqwest::Method::PUT, "/api/user/scheme-profile", Some(body))
        .await?;
    profile_response(core, response)
}

pub(crate) async fn fetch_profile_at(core: &Core, api_base: &str) -> Result<Value, CoreError> {
    let response = crate::account::ApiClient::new(core, api_base)
        .request(reqwest::Method::GET, "/api/user/scheme-profile", None)
        .await?;
    profile_response(core, response)
}

fn profile_response(core: &Core, response: Value) -> Result<Value, CoreError> {
    let profile = if response["profile"].is_null() {
        Value::Null
    } else {
        serde_json::to_value(checked_profile(response["profile"].clone())?)?
    };
    let updated_at = response["updateAt"].as_str().map(str::to_owned);
    if updated_at.is_some() {
        core.update_state(|state| state.last_scheme_sync_at = updated_at.clone())?;
    }
    Ok(json!({"profile":profile,"updateAt":updated_at}))
}
