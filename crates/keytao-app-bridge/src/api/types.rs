#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BridgePlatform {
    Windows,
    MacOs,
    Linux,
    Android,
    Ios,
}

pub struct BridgeConfig {
    pub data_dir: String,
    pub cache_dir: String,
    pub resource_dir: String,
    pub app_version: String,
    pub platform: BridgePlatform,
    /// An absolute path. When set, all bridge hosts use this root exclusively.
    pub user_root_override: Option<String>,
}

pub struct BridgeInfo {
    pub platform: BridgePlatform,
    pub app_version: String,
    pub user_root: String,
}

pub struct LocalSchemaDto {
    pub installed: bool,
    pub deployed: bool,
    pub version: Option<String>,
    pub schemas: Vec<String>,
}

pub struct OnboardingDto {
    pub completed: bool,
}

pub struct SchemeReleaseDto {
    pub scheme: String,
    pub source_type: Option<String>,
    pub label: String,
    pub version: String,
    pub name: String,
    pub published_at: Option<String>,
    pub download_url: String,
    pub asset_name: String,
}

pub struct VerifyEntryDto {
    pub path: String,
    pub ok: bool,
    pub note: String,
}

pub struct InstallResultDto {
    pub merged_schemas: Vec<String>,
    pub logs: Vec<String>,
    pub verify: Vec<VerifyEntryDto>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum BridgeEventKind {
    /// Subscription handshake; subsequent events come from Core's EventSink.
    Ready,
    InstallProgress,
    DeployProgress,
    WindowsImeStatus,
    WindowsImeAction,
}

pub struct InstallProgressDto {
    pub stage: String,
    pub percent: u32,
    pub message: String,
}

pub struct WindowsImeStatusDto {
    pub supported: bool,
    pub packaged: bool,
    pub registered: bool,
    pub registered_dll: bool,
    pub profile_enabled: bool,
    pub registration_busy: bool,
    pub registration_state: String,
    pub registration_error: Option<String>,
    pub runtime_dir: Option<String>,
    pub dll_path: Option<String>,
    pub registered_path: Option<String>,
    pub profile_status: String,
    pub user_data_dir: Option<String>,
    pub shared_data_dir: Option<String>,
    pub shared_data_source: String,
    pub reload_stamp_path: Option<String>,
    pub reload_stamp_signature: Option<String>,
    pub message: String,
}

/// Exactly one payload is present for progress/status events; action/ready have none.
/// Plain structs avoid requiring any Dart code-generation packages.
pub struct BridgeEvent {
    pub kind: BridgeEventKind,
    pub install_progress: Option<InstallProgressDto>,
    pub deploy_progress: Option<String>,
    pub windows_ime_status: Option<WindowsImeStatusDto>,
}

#[derive(serde::Deserialize)]
pub struct DeployResultDto {
    pub success: bool,
    pub message: String,
}

#[derive(serde::Deserialize)]
pub struct DownloadUrlsDto {
    pub macos: Option<String>,
    pub windows: Option<String>,
    pub linux: Option<String>,
    pub android: Option<String>,
    pub ios: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct PlatformReleaseDto {
    pub version: String,
    pub download_urls: DownloadUrlsDto,
}

#[derive(serde::Deserialize)]
pub struct ReleaseInfoDto {
    pub version: String,
    pub name: String,
    pub published_at: String,
    pub body: String,
    pub github: Option<PlatformReleaseDto>,
    pub gitee: Option<PlatformReleaseDto>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidImeInputSettingsDto {
    pub haptics_enabled: bool,
    pub haptic_intensity: u8,
    pub enter_key_behavior: String,
    pub key_preview_enabled: bool,
    pub long_press_delay_ms: u16,
    pub keyboard_height_scale: u8,
    pub delete_speed: String,
    pub english_mode: String,
    pub backspace_gesture_mode: String,
    pub keyboard_height_dp: u16,
    pub candidate_bar_height_dp: u8,
    pub swipe_threshold_dp: f32,
    pub key_sound_enabled: bool,
    pub key_sound_volume: u8,
    pub key_hint_visible: bool,
    pub flick_keys_enabled: bool,
    pub number_row_enabled: bool,
    pub candidate_font_scale: f32,
    pub double_space_period_enabled: bool,
    pub floating_portrait_enabled: bool,
    pub floating_portrait_scale: u8,
    pub floating_landscape_enabled: bool,
    pub floating_landscape_scale: u8,
    pub config_path: Option<String>,
    pub reload_stamp_path: Option<String>,
    pub message: String,
}

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UiColorSchemeDto {
    Auto,
    Light,
    Dark,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EffectiveColorSchemeDto {
    Light,
    Dark,
}

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PanelOrientationDto {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnglishModeDto {
    Ascii,
    Schema,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImeUiSettingsDto {
    pub color_scheme: UiColorSchemeDto,
    pub effective_color_scheme: EffectiveColorSchemeDto,
    pub orientation: PanelOrientationDto,
    pub embedded_composition: bool,
    pub accent_color: String,
    pub font_size: f32,
    pub theme_path: Option<String>,
    pub theme_exists: bool,
    pub reload_stamp_path: Option<String>,
    pub reload_stamp_signature: Option<String>,
    pub message: String,
}

#[derive(serde::Deserialize)]
pub struct AddonSchemaStatusDto {
    pub installed: bool,
    pub deployed: bool,
    pub version: String,
    pub english_available: bool,
}

#[derive(serde::Deserialize)]
pub struct MacosImeStatusDto {
    pub installed: bool,
    pub app_path: Option<String>,
    pub user_data_dir: Option<String>,
    pub shared_data_dir: Option<String>,
    pub shared_data_source: String,
    pub reload_stamp_path: Option<String>,
    pub reload_stamp_signature: Option<String>,
    pub log_dir: Option<String>,
    pub message: String,
}

#[derive(serde::Deserialize)]
pub struct ComponentVersionsDto {
    pub app_version: String,
    pub librime_version: Option<String>,
    pub opencc_version: Option<String>,
    pub data_dir: Option<String>,
}

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeLogLevelDto {
    Info,
    Verbose,
}

#[derive(serde::Deserialize)]
pub struct RuntimeLogFileInfoDto {
    pub name: String,
    pub size: u64,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeLogSettingsDto {
    pub enabled: bool,
    pub level: RuntimeLogLevelDto,
    pub log_dir: String,
    pub files: Vec<RuntimeLogFileInfoDto>,
    pub file_count: u32,
    pub total_bytes: u64,
}

#[derive(serde::Deserialize)]
pub struct DebugLogFileDto {
    pub lines: Vec<String>,
    pub truncated: bool,
}

#[cfg(all(test, not(any(target_os = "android", target_os = "ios"))))]
mod tests {
    use super::*;

    // Host builds cannot execute Core's mobile-only persistence APIs. Check the
    // full Core/bridge serialization contract here; the mobile test exercises
    // get -> edit -> set -> get against an isolated on-disk root.
    #[test]
    fn android_settings_dto_change_one_field_preserves_all_other_fields() {
        let core_settings = keytao_app_core::ime_settings::AndroidImeInputSettings {
            haptics_enabled: true,
            haptic_intensity: 57,
            enter_key_behavior: "newline".into(),
            key_preview_enabled: false,
            long_press_delay_ms: 450,
            keyboard_height_scale: 115,
            delete_speed: "fast".into(),
            english_mode: "schema".into(),
            backspace_gesture_mode: "selectThenDelete".into(),
            keyboard_height_dp: 310,
            candidate_bar_height_dp: 62,
            swipe_threshold_dp: 40.5,
            key_sound_enabled: false,
            key_sound_volume: 73,
            key_hint_visible: false,
            flick_keys_enabled: false,
            number_row_enabled: true,
            candidate_font_scale: 1.25,
            double_space_period_enabled: false,
            floating_portrait_enabled: true,
            floating_portrait_scale: 85,
            floating_landscape_enabled: false,
            floating_landscape_scale: 65,
            config_path: Some("/isolated/keytao/keytao_android_ime.json".into()),
            reload_stamp_path: Some("/isolated/keytao/.reload".into()),
            message: "settings fixture".into(),
        };
        let mut expected = serde_json::to_value(&core_settings).unwrap();
        let mut edited: AndroidImeInputSettingsDto = crate::runtime::dto(core_settings).unwrap();
        edited.haptics_enabled = false;
        expected["hapticsEnabled"] = serde_json::Value::Bool(false);
        let saved: keytao_app_core::ime_settings::AndroidImeInputSettings =
            crate::runtime::dto(edited).unwrap();
        let actual: AndroidImeInputSettingsDto = crate::runtime::dto(saved).unwrap();
        assert_eq!(serde_json::to_value(actual).unwrap(), expected);
        assert_eq!(expected.as_object().unwrap().len(), 26);
    }
}
