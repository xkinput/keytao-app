use crate::{api::types::*, frb_generated::StreamSink, runtime};
use keytao_app_core::scheme_host::SchemeHost;
#[cfg(not(any(target_os = "android", target_os = "windows")))]
use keytao_app_core::{events::InstallProgress, CoreEvent};

#[flutter_rust_bridge::frb(init)]
pub fn init_app() {
    flutter_rust_bridge::setup_default_user_utils();
}

/// Creates the process-wide Core. Every subsequent call returns an error,
/// including calls with identical config. Restart the process to change roots.
pub fn init_core(config: BridgeConfig) -> Result<BridgeInfo, String> {
    runtime::initialize(config)
}

/// One active subscriber; registering again replaces/closes the previous stream.
/// Wait for Ready before triggering an action whose events must be observed.
pub fn core_events(sink: StreamSink<BridgeEvent>) -> Result<(), String> {
    runtime::state()?.events.register(sink)
}

pub fn check_local_schema() -> Result<LocalSchemaDto, String> {
    let state = runtime::state()?;
    let local =
        keytao_app_core::scheme::check_local_schema(&state.core, Some(state.host.user_root()?))
            .map_err(|error| error.to_string())?;
    Ok(LocalSchemaDto {
        installed: local.installed,
        deployed: local.deployed,
        version: local.version,
        schemas: local.schemas,
    })
}

pub fn onboarding() -> Result<OnboardingDto, String> {
    Ok(OnboardingDto {
        completed: runtime::state()?.core.onboarding().completed,
    })
}

pub fn complete_onboarding() -> Result<(), String> {
    runtime::state()?
        .core
        .complete_onboarding()
        .map_err(|error| error.to_string())
}

pub async fn fetch_scheme_release(scheme: String) -> Result<SchemeReleaseDto, String> {
    let release = keytao_app_core::scheme::fetch_scheme_release(&runtime::state()?.core, scheme)
        .await
        .map_err(|error| error.to_string())?;
    Ok(SchemeReleaseDto {
        scheme: release.scheme,
        source_type: release.source_type,
        label: release.label,
        version: release.version,
        name: release.name,
        published_at: release.published_at,
        download_url: release.download_url,
        asset_name: release.asset_name,
    })
}

/// Fetches the latest release for the same scheme key accepted by Core,
/// downloads it, then installs into the host root. Does not deploy the IME.
pub async fn install_latest_scheme(scheme: String) -> Result<InstallResultDto, String> {
    #[cfg(any(target_os = "android", target_os = "windows"))]
    {
        let _ = scheme;
        return Err("Use the platform installation flow on Android and Windows".into());
    }
    #[cfg(not(any(target_os = "android", target_os = "windows")))]
    {
        let state = runtime::state()?;
        state.core.emit(CoreEvent::InstallProgress(InstallProgress {
            stage: "fetching".into(),
            percent: 0,
            message: "正在获取方案版本...".into(),
        }));
        let release = fetch_scheme_release(scheme).await?;
        install_scheme_from_url(release.download_url).await
    }
}

pub async fn fetch_latest_release() -> Result<ReleaseInfoDto, String> {
    runtime::dto(
        keytao_app_core::scheme::fetch_latest_release(&runtime::state()?.core)
            .await
            .map_err(|error| error.to_string())?,
    )
}

/// Installs a selected GitHub/Gitee or scheme release URL without deploying.
pub async fn install_scheme_from_url(url: String) -> Result<InstallResultDto, String> {
    #[cfg(any(target_os = "android", target_os = "windows"))]
    {
        let _ = url;
        Err("Use the platform installation flow on Android and Windows".into())
    }
    #[cfg(not(any(target_os = "android", target_os = "windows")))]
    {
        let state = runtime::state()?;
        // Core's downloader uses a fixed cache filename; serialize installations.
        let _guard = state.install_lock.lock().await;
        let root = state.host.user_root()?;
        std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        let archive = keytao_app_core::download::download_to_temp(&state.core, url)
            .await
            .map_err(|error| error.to_string())?;
        // Mirror rime_install_to_default while preserving an explicit host override.
        let result = keytao_app_core::install::smart_install(
            &state.core,
            archive.clone(),
            root.to_string_lossy().into_owned(),
        )
        .await;
        // Core removes the archive on success; also remove failed downloads here.
        let _ = std::fs::remove_file(archive);
        Ok(install_result_dto(
            result.map_err(|error| error.to_string())?,
        ))
    }
}

#[flutter_rust_bridge::frb(ignore)]
fn install_result_dto(result: keytao_app_core::scheme_types::InstallResult) -> InstallResultDto {
    InstallResultDto {
        merged_schemas: result.merged_schemas,
        logs: result.logs,
        verify: result
            .verify
            .into_iter()
            .map(|entry| VerifyEntryDto {
                path: entry.path,
                ok: entry.ok,
                note: entry.note,
            })
            .collect(),
    }
}

/// Call only after the Android channel confirms storage permission. Dart must
/// serialize prepare -> smartExtractZipToPrivate -> finish as one installation.
pub async fn prepare_android_install(url: String) -> Result<String, String> {
    #[cfg(target_os = "android")]
    {
        let state = runtime::state()?;
        let _guard = state.install_lock.lock().await;
        keytao_app_core::install::prepare_android_install(
            &state.core,
            &state.host.user_root()?,
            url,
        )
        .await
        .map_err(|error| error.to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = url;
        Err("Android installation is only available on Android".into())
    }
}

/// Pass jsonEncode of the Android channel's smart-extract result unchanged.
pub fn finish_android_install(result_json: String) -> Result<InstallResultDto, String> {
    #[cfg(target_os = "android")]
    {
        let result: serde_json::Value =
            serde_json::from_str(&result_json).map_err(|error| error.to_string())?;
        let result =
            keytao_app_core::install::finish_android_install(&runtime::state()?.core, &result)
                .map_err(|error| error.to_string())?;
        Ok(install_result_dto(result))
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = result_json;
        Err("Android installation is only available on Android".into())
    }
}

/// Android deployment belongs to the deployImeData platform channel.
pub async fn deploy_default() -> Result<DeployResultDto, String> {
    #[cfg(target_os = "macos")]
    {
        let state = runtime::state()?;
        let _guard = state.install_lock.lock().await;
        if state.host.uses_default_root() {
            return runtime::dto(
                keytao_app_core::deploy::rime_deploy_default(&state.core, state.host.clone())
                    .await
                    .map_err(|error| error.to_string())?,
            );
        }
        state.host.deploy().await?;
        Ok(DeployResultDto {
            success: true,
            message: "部署成功".into(),
        })
    }
    #[cfg(not(target_os = "macos"))]
    Err("Bridge deployment is macOS only; use deployImeData on Android".into())
}

pub async fn get_android_ime_input_settings() -> Result<AndroidImeInputSettingsDto, String> {
    runtime::dto(
        keytao_app_core::ime_settings::get_android_ime_input_settings(
            runtime::state()?.host.clone(),
        )
        .await
        .map_err(|error| error.to_string())?,
    )
}

/// Saves all 23 writable fields; returned path/message fields are read-only.
pub async fn set_android_ime_input_settings(
    settings: AndroidImeInputSettingsDto,
) -> Result<AndroidImeInputSettingsDto, String> {
    runtime::dto(
        keytao_app_core::ime_settings::set_android_ime_input_settings(
            runtime::state()?.host.clone(),
            settings.haptics_enabled,
            settings.haptic_intensity,
            settings.enter_key_behavior,
            settings.key_preview_enabled,
            settings.long_press_delay_ms,
            settings.keyboard_height_scale,
            settings.delete_speed,
            settings.english_mode,
            settings.backspace_gesture_mode,
            settings.keyboard_height_dp,
            settings.candidate_bar_height_dp,
            settings.swipe_threshold_dp,
            settings.key_sound_enabled,
            settings.key_sound_volume,
            settings.key_hint_visible,
            settings.flick_keys_enabled,
            settings.number_row_enabled,
            settings.candidate_font_scale,
            settings.double_space_period_enabled,
            settings.floating_portrait_enabled,
            settings.floating_portrait_scale,
            settings.floating_landscape_enabled,
            settings.floating_landscape_scale,
        )
        .await
        .map_err(|error| error.to_string())?,
    )
}

pub fn get_ime_ui_settings() -> Result<ImeUiSettingsDto, String> {
    let state = runtime::state()?;
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    state.host.require_default_theme_root()?;
    runtime::dto(
        keytao_app_core::ime_settings::get_ime_ui_settings(
            #[cfg(any(target_os = "android", target_os = "ios"))]
            state.host.clone(),
        )
        .map_err(|error| error.to_string())?,
    )
}

pub fn set_ime_ui_settings(
    color_scheme: UiColorSchemeDto,
    orientation: PanelOrientationDto,
    accent_color: String,
    font_size: f32,
) -> Result<ImeUiSettingsDto, String> {
    let state = runtime::state()?;
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    state.host.require_default_theme_root()?;
    runtime::dto(
        keytao_app_core::ime_settings::set_ime_ui_settings(
            #[cfg(any(target_os = "android", target_os = "ios"))]
            state.host.clone(),
            runtime::dto(color_scheme)?,
            runtime::dto(orientation)?,
            accent_color,
            font_size,
        )
        .map_err(|error| error.to_string())?,
    )
}

pub fn set_ime_embedded_composition(embedded: bool) -> Result<ImeUiSettingsDto, String> {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        runtime::state()?.host.require_default_theme_root()?;
        runtime::dto(
            keytao_app_core::ime_settings::set_ime_embedded_composition(embedded)
                .map_err(|error| error.to_string())?,
        )
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let _ = embedded;
        Err("Embedded composition settings are desktop only".into())
    }
}

pub fn get_desktop_english_mode() -> Result<EnglishModeDto, String> {
    runtime::dto(
        keytao_app_core::ime_settings::get_desktop_english_mode(runtime::state()?.host.clone())
            .map_err(|error| error.to_string())?,
    )
}

pub async fn set_desktop_english_mode(mode: EnglishModeDto) -> Result<EnglishModeDto, String> {
    runtime::dto(
        keytao_app_core::ime_settings::set_desktop_english_mode(
            runtime::state()?.host.clone(),
            runtime::dto(mode)?,
        )
        .await
        .map_err(|error| error.to_string())?,
    )
}

pub fn addon_schema_status(id: String) -> Result<AddonSchemaStatusDto, String> {
    let state = runtime::state()?;
    runtime::dto(
        keytao_app_core::addon::addon_schema_status(&state.core, || state.host.user_root(), id)
            .map_err(|error| error.to_string())?,
    )
}

pub fn macos_ime_status() -> Result<MacosImeStatusDto, String> {
    runtime::dto(keytao_app_core::ime_status::macos::macos_ime_status(
        &runtime::state()?.core,
    ))
}

pub fn get_component_versions() -> Result<ComponentVersionsDto, String> {
    let state = runtime::state()?;
    let mut versions: ComponentVersionsDto =
        runtime::dto(keytao_app_core::component_versions::get_component_versions(
            &state.core,
            &state.host,
            "",
            option_env!("RIME_VERSION"),
            option_env!("OPENCC_VERSION"),
        ))?;
    versions.data_dir = Some(state.host.user_root()?.to_string_lossy().into_owned());
    Ok(versions)
}

pub async fn get_runtime_log_settings() -> Result<RuntimeLogSettingsDto, String> {
    let state = runtime::state()?;
    runtime::dto(
        keytao_app_core::logs::get_runtime_log_settings(&state.core, &state.host)
            .await
            .map_err(|error| error.to_string())?,
    )
}

pub async fn set_runtime_log_settings(
    enabled: bool,
    level: RuntimeLogLevelDto,
) -> Result<(), String> {
    let state = runtime::state()?;
    keytao_app_core::logs::set_runtime_log_settings(
        &state.core,
        &state.host,
        enabled,
        runtime::dto(level)?,
    )
    .await
    .map_err(|error| error.to_string())
}

pub async fn read_runtime_log(max_lines: Option<u32>) -> Result<DebugLogFileDto, String> {
    let state = runtime::state()?;
    runtime::dto(
        keytao_app_core::logs::read_runtime_log(
            &state.core,
            &state.host,
            max_lines.map(|value| value as usize),
        )
        .await
        .map_err(|error| error.to_string())?,
    )
}

pub async fn clear_runtime_log() -> Result<(), String> {
    let state = runtime::state()?;
    keytao_app_core::logs::clear_runtime_log(&state.core, &state.host)
        .await
        .map_err(|error| error.to_string())
}

#[cfg(all(test, any(target_os = "android", target_os = "ios")))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn android_settings_change_one_field_preserves_the_other_22() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("user");
        init_core(BridgeConfig {
            data_dir: temp.path().join("state").to_string_lossy().into_owned(),
            cache_dir: temp.path().join("cache").to_string_lossy().into_owned(),
            resource_dir: temp.path().join("resources").to_string_lossy().into_owned(),
            app_version: "test".into(),
            platform: if cfg!(target_os = "android") {
                BridgePlatform::Android
            } else {
                BridgePlatform::Ios
            },
            user_root_override: Some(root.to_string_lossy().into_owned()),
        })
        .unwrap();
        let before = get_android_ime_input_settings().await.unwrap();
        let mut edited = before.clone();
        edited.haptics_enabled = !edited.haptics_enabled;
        set_android_ime_input_settings(edited.clone())
            .await
            .unwrap();
        let after = get_android_ime_input_settings().await.unwrap();
        assert_eq!(after, edited);
    }
}
