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

#[flutter_rust_bridge::frb]
pub struct BridgeInfo {
    pub platform: BridgePlatform,
    pub app_version: String,
    pub user_root: String,
    #[frb(default = false)]
    pub app_update_prereleases: bool,
    pub website_origin: Option<String>,
    #[frb(default = false)]
    pub website_hidden: bool,
}

pub struct SystemFontFamilyDto {
    pub family: String,
    pub display_name: String,
    pub weights: Vec<String>,
}

#[derive(serde::Deserialize)]
pub struct RimeSyncSettingsDto {
    pub installation_id: String,
    pub sync_dir: String,
    pub effective_sync_dir: String,
}

#[flutter_rust_bridge::frb]
pub struct LocalSchemaDto {
    pub installed: bool,
    pub deployed: bool,
    #[frb(default = false)]
    pub build_available: bool,
    pub version: Option<String>,
    pub schemas: Vec<String>,
    pub installed_scheme: Option<String>,
    pub installed_scheme_version: Option<String>,
    pub easy_english_enabled: Option<bool>,
    pub deploy_warning: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
pub struct GitSchemeConfigDto {
    pub repo_url: String,
    pub mode: String,
    pub branch: Option<String>,
    pub asset_name: Option<String>,
}

#[derive(serde::Deserialize)]
#[flutter_rust_bridge::frb]
pub struct InstalledPackageDto {
    pub scheme_key: String,
    pub version: Option<String>,
    pub active: bool,
    #[frb(default = true)]
    pub in_use: bool,
    pub has_package_zip: bool,
    #[frb(default = true)]
    pub supports_user_words: bool,
    pub source: Option<String>,
    pub display_name: Option<String>,
    pub logo_path: Option<String>,
    #[frb(default = "rime")]
    pub engine: String,
    #[serde(default)]
    pub git: Option<GitSchemeConfigDto>,
}

pub struct LibjdStatusDto {
    pub available: bool,
    pub installed_version: Option<String>,
}

#[derive(serde::Deserialize)]
#[flutter_rust_bridge::frb]
pub struct IsolatedPackageDto {
    pub package_key: String,
    pub display_name: String,
    pub active: bool,
    pub ready: bool,
    #[serde(default)]
    #[frb(default = "rime")]
    pub engine: String,
    #[serde(default)]
    #[frb(default = [])]
    pub schemas: Vec<SchemaDto>,
}

#[derive(serde::Deserialize)]
pub struct PackageSchemaDto {
    pub id: String,
    pub name: String,
    pub selected: bool,
}

#[derive(serde::Deserialize)]
pub struct PackageOrphanFileDto {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(serde::Deserialize)]
pub struct PackageKeptBackupDto {
    pub path: String,
    pub backup: String,
}

#[derive(serde::Deserialize)]
pub struct PackageOrphanReportDto {
    pub files: Vec<PackageOrphanFileDto>,
    pub kept_backups: Vec<PackageKeptBackupDto>,
}

#[derive(serde::Deserialize)]
pub struct SchemeUpdateDto {
    pub scheme_key: String,
    pub installed_version: String,
    pub latest_version: String,
    pub has_update: bool,
}

#[derive(serde::Deserialize)]
#[flutter_rust_bridge::frb]
pub struct UserWordDto {
    pub id: u32,
    pub word: String,
    pub code: String,
    pub weight: Option<String>,
    #[serde(default)]
    #[frb(default = false)]
    pub remove: bool,
}

#[derive(serde::Deserialize)]
pub struct UserWordComparisonDto {
    pub entry: UserWordDto,
    pub status: String,
    pub words: Vec<String>,
    pub codes: Vec<String>,
}

#[derive(serde::Deserialize)]
pub struct UserWordMutationDto {
    pub already_exists: bool,
}

#[derive(serde::Deserialize)]
pub struct UserWordsReportDto {
    pub removed: Vec<UserWordDto>,
    pub conflicts: Vec<UserWordComparisonDto>,
}

pub struct LocalSchemasDto {
    pub has_default_custom: bool,
    pub schemas: Vec<String>,
}

#[derive(serde::Deserialize)]
pub struct FileItemDto {
    pub name: String,
    pub is_dir: bool,
}

#[derive(serde::Deserialize)]
pub struct AppUpdateDto {
    pub has_update: bool,
    pub latest_version: String,
    pub current_version: String,
    pub release_url: String,
    pub install_mode: Option<String>,
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
    CloudTransferProgress,
}

pub struct InstallProgressDto {
    pub stage: String,
    pub percent: u32,
    pub message: String,
}

pub struct CloudTransferProgressDto {
    pub direction: String,
    pub scheme_key: String,
    pub bytes_transferred: u64,
    pub total_bytes: u64,
}

pub enum DeployStepDto {
    Materialize,
    Compile,
    Validate,
    Prune,
    Skip,
    Done,
    Failed,
}

pub struct DeployProgressDto {
    pub package_key: String,
    pub package_name: String,
    pub index: u32,
    pub total: u32,
    pub step: DeployStepDto,
    pub error: Option<String>,
    pub legacy_message: Option<String>,
    pub queued_package_keys: Vec<String>,
}

#[derive(serde::Deserialize)]
pub struct WindowsImeStatusDto {
    pub packaged: bool,
    pub registered: bool,
    pub registered_dll: bool,
    pub profile_enabled: bool,
    pub registration_busy: bool,
    pub registration_state: String,
    pub registration_error: Option<String>,
    pub runtime_dir: Option<String>,
    pub registered_path: Option<String>,
    pub profile_status: String,
    pub message: String,
}

/// Exactly one payload is present for progress/status events; action/ready have none.
/// Plain structs avoid requiring any Dart code-generation packages.
pub struct BridgeEvent {
    pub kind: BridgeEventKind,
    pub install_progress: Option<InstallProgressDto>,
    pub cloud_transfer_progress: Option<CloudTransferProgressDto>,
    pub deploy_progress: Option<DeployProgressDto>,
    pub windows_ime_status: Option<WindowsImeStatusDto>,
}

#[derive(serde::Deserialize)]
pub struct DeployResultDto {
    pub success: bool,
    pub message: String,
    pub warning: Option<String>,
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
pub struct SchemaDto {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SchemaListDto {
    pub current: Option<String>,
    pub schemas: Vec<SchemaDto>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaSwitchDto {
    pub schema_id: String,
    pub name: String,
    pub states: Vec<String>,
    pub options: Vec<String>,
    pub value: u32,
}

impl From<keytao_core::SchemaSwitchSetting> for SchemaSwitchDto {
    fn from(value: keytao_core::SchemaSwitchSetting) -> Self {
        Self {
            schema_id: value.schema_id,
            name: value.name,
            states: value.states,
            options: value.options,
            value: value.value as u32,
        }
    }
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
    pub symbol_auto_return: bool,
    pub phrases: Vec<String>,
    pub clipboard_history_enabled: bool,
    pub clipboard_history_limit: u16,
    pub clipboard_suggestion_enabled: bool,
    pub clipboard_suggestion_timeout_sec: u16,
    pub clipboard_return_after_paste: bool,
    pub clipboard_hide_sensitive: bool,
    pub floating_portrait_enabled: bool,
    pub floating_portrait_scale: u8,
    pub floating_landscape_enabled: bool,
    pub floating_landscape_scale: u8,
    pub keyboard_layout_json: Option<String>,
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
pub struct ThemePreviewColorsDto {
    pub background: String,
    pub foreground: String,
    pub accent: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeSummaryDto {
    pub id: String,
    pub name: String,
    pub built_in: bool,
    pub current: bool,
    pub light: ThemePreviewColorsDto,
    pub dark: ThemePreviewColorsDto,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeExportDto {
    pub id: String,
    pub file_name: String,
    pub json: String,
    pub share_string: String,
    pub use_file: bool,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeImportPreviewDto {
    pub id: String,
    pub name: String,
    pub can_overwrite: bool,
    pub conflict: bool,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeImportResultDto {
    pub id: String,
    pub name: String,
    pub disposition: String,
    pub renamed_from: Option<String>,
    pub dropped: Vec<String>,
    pub clamped: Vec<String>,
    pub unknown: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImeUiSettingsDto {
    pub color_scheme: UiColorSchemeDto,
    pub effective_color_scheme: EffectiveColorSchemeDto,
    pub orientation: PanelOrientationDto,
    pub embedded_composition: bool,
    pub hide_candidate_panel: bool,
    pub show_tray_mode_indicator: bool,
    pub effective_embedded_composition: bool,
    pub accent_color: String,
    pub accent_color_override: Option<String>,
    pub font_size: f32,
    pub corner_radius: Option<f32>,
    pub background_color: String,
    pub theme_path: Option<String>,
    pub theme_exists: bool,
    pub reload_stamp_path: Option<String>,
    pub reload_stamp_signature: Option<String>,
    pub message: String,
}

#[derive(serde::Deserialize)]
pub struct MacosImeStatusDto {
    pub installed: bool,
    pub app_path: Option<String>,
    pub message: String,
}

#[derive(serde::Deserialize)]
pub struct LinuxImeStatusDto {
    pub kde_session: bool,
    pub kde_configured: bool,
    pub running: bool,
    pub managed_pid: Option<u32>,
    pub command: String,
    pub kde_native_processes: u32,
    pub fallback_processes: u32,
    pub message: String,
}

#[derive(serde::Deserialize)]
pub struct ComponentVersionsDto {
    pub app_version: String,
    pub librime_version: Option<String>,
    pub librime_lua_version: Option<String>,
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

#[derive(serde::Deserialize)]
pub struct DebugLogsDto {
    pub ime: DebugLogFileDto,
    pub app: DebugLogFileDto,
    pub macos_ime: Option<DebugLogFileDto>,
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
            symbol_auto_return: true,
            phrases: vec!["Custom phrase".into()],
            clipboard_history_enabled: false,
            clipboard_history_limit: 75,
            clipboard_suggestion_enabled: false,
            clipboard_suggestion_timeout_sec: 45,
            clipboard_return_after_paste: false,
            clipboard_hide_sensitive: false,
            floating_portrait_enabled: true,
            floating_portrait_scale: 85,
            floating_landscape_enabled: false,
            floating_landscape_scale: 65,
            keyboard_layout_json: Some(r#"{"rows":[[{"label":"q"}]]}"#.into()),
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
        assert_eq!(expected.as_object().unwrap().len(), 35);
    }
}

#[derive(serde::Deserialize)]
pub struct AccountUserDto {
    pub id: i64,
    pub name: Option<String>,
    pub nickname: Option<String>,
    pub email: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct AccountStatsDto {
    pub batches_count: u32,
    pub pull_requests_count: u32,
    pub submitted_batches: u32,
    pub published_batches: u32,
}

pub struct UserDictionarySyncDto {
    pub file_name: String,
    pub count: u64,
    pub message: String,
    pub pending: bool,
    pub local_deletions: Vec<UserDictionaryDeletionDto>,
    pub server_deletions: Vec<UserDictionaryDeletionDto>,
}

#[flutter_rust_bridge::frb]
pub struct UserDictionaryDeletionDto {
    pub word: String,
    pub code: String,
    #[frb(default = false)]
    pub remove: bool,
}

pub struct UserDictionaryDeletionConfirmationDto {
    pub local_deletions: Vec<UserDictionaryDeletionDto>,
    pub server_deletions: Vec<UserDictionaryDeletionDto>,
}
