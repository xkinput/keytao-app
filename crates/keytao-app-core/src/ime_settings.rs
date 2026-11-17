#[cfg(any(target_os = "android", target_os = "ios"))]
use crate::addon::{android_ime_config_path, read_android_ime_config};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use crate::scheme_files::yaml_child_mapping;
use crate::{
    CoreError, addon::deployed_english_schema_available, ime_host::ImeHost, ime_paths::*,
    scheme_files::write_file_atomic,
};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::path::PathBuf;

pub fn read_candidate_page_size(root: &Path) -> Option<u8> {
    let config: serde_json::Value =
        std::fs::read(root.join(keytao_core::english_mode::SETTINGS_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())?;
    config["candidatePageSize"]
        .as_u64()
        .filter(|size| (1..=10).contains(size))
        .map(|size| size as u8)
}

pub fn set_candidate_page_size(root: &Path, size: Option<u8>) -> Result<Option<u8>, String> {
    if size.is_some_and(|size| !(1..=10).contains(&size)) {
        return Err("Candidate page size must be between 1 and 10".into());
    }
    let path = crate::packages::safe_path(root, keytao_core::english_mode::SETTINGS_FILE)?;
    let mut config: serde_json::Value = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| error.to_string())?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(error) => return Err(error.to_string()),
    };
    let object = config
        .as_object_mut()
        .ok_or("IME settings must be an object")?;
    if let Some(size) = size {
        object.insert("candidatePageSize".into(), size.into());
    } else {
        object.remove("candidatePageSize");
    }
    std::fs::create_dir_all(root).map_err(|error| error.to_string())?;
    write_file_atomic(
        &path,
        &serde_json::to_vec_pretty(&config).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    Ok(size)
}

pub fn prepare_candidate_page_size(root: &Path) -> Result<bool, String> {
    crate::candidate_page_size::apply_candidate_page_size(root, read_candidate_page_size(root))
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ImeUiSettings {
    pub color_scheme: keytao_theme::UiColorScheme,
    pub effective_color_scheme: keytao_theme::EffectiveColorScheme,
    pub orientation: keytao_theme::PanelOrientation,
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

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn ime_theme_path() -> Result<PathBuf, String> {
    keytao_theme::default_user_theme_path().ok_or("Cannot determine keytao data directory".into())
}

fn ime_ui_settings_from_paths(
    theme_path: PathBuf,
    reload_stamp_path: Option<PathBuf>,
    message: String,
) -> Result<ImeUiSettings, String> {
    let theme = keytao_theme::ThemeResolver::new(None, Some(theme_path.clone())).current();
    let (reload_stamp_path, reload_stamp_signature) = reload_stamp_path
        .map(|path| {
            let signature = file_signature(&path);
            (Some(path_string(path)), Some(signature))
        })
        .unwrap_or((None, None));
    let accent_color = theme
        .ui
        .accent_color
        .unwrap_or(theme.candidate.selected_label_color);
    Ok(ImeUiSettings {
        color_scheme: theme.ui.color_scheme,
        effective_color_scheme: theme.ui.effective_color_scheme,
        orientation: theme.panel.orientation,
        embedded_composition: theme.ui.embedded_composition,
        hide_candidate_panel: theme.ui.hide_candidate_panel,
        show_tray_mode_indicator: theme.ui.show_tray_mode_indicator,
        effective_embedded_composition: theme.ui.effective_embedded_composition(),
        accent_color: color_to_hex(accent_color),
        accent_color_override: theme.ui.accent_color.map(color_to_hex),
        font_size: theme.font.size,
        corner_radius: theme.ui.corner_radius,
        background_color: theme
            .ui
            .background_color
            .map(color_to_hex)
            .unwrap_or_default(),
        theme_exists: theme_path.is_file(),
        theme_path: Some(path_string(theme_path)),
        reload_stamp_path,
        reload_stamp_signature,
        message,
    })
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn ime_ui_settings_with_message(message: String) -> Result<ImeUiSettings, String> {
    let theme_path = ime_theme_path()?;
    let reload_stamp_path = reload_stamp_path();
    ime_ui_settings_from_paths(theme_path, reload_stamp_path, message)
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn write_ime_ui_settings(
    color_scheme: keytao_theme::UiColorScheme,
    orientation: keytao_theme::PanelOrientation,
    accent_color: Option<String>,
    font_size: f32,
    corner_radius: Option<f32>,
    background_color: String,
) -> Result<(), String> {
    let theme_path = ime_theme_path()?;
    keytao_theme::write_ime_ui_settings_to_path(
        &theme_path,
        color_scheme,
        orientation,
        accent_color,
        font_size,
        corner_radius,
        background_color,
    )
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn write_ime_embedded_composition(embedded: bool) -> Result<(), String> {
    let theme_path = ime_theme_path()?;
    write_ime_embedded_composition_to_path(theme_path, embedded)
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn write_ime_embedded_composition_to_path(
    theme_path: PathBuf,
    embedded: bool,
) -> Result<(), String> {
    write_ime_ui_flag_to_path(
        theme_path,
        "embeddedComposition",
        "embedded_composition",
        embedded,
    )
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn write_ime_hide_candidate_panel_to_path(theme_path: PathBuf, hide: bool) -> Result<(), String> {
    write_ime_ui_flag_to_path(
        theme_path,
        "hideCandidatePanel",
        "hide_candidate_panel",
        hide,
    )
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn write_ime_ui_flag_to_path(
    theme_path: PathBuf,
    key: &str,
    legacy_key: &str,
    value: bool,
) -> Result<(), String> {
    if let Some(parent) = theme_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建主题目录失败: {e}"))?;
    }
    let mut root = if theme_path.is_file() {
        let content = std::fs::read_to_string(&theme_path)
            .map_err(|e| format!("读取主题配置失败 {}: {e}", theme_path.display()))?;
        serde_yaml::from_str::<serde_yaml::Value>(&content)
            .map_err(|e| format!("主题配置无法解析 {}: {e}", theme_path.display()))?
    } else {
        serde_yaml::Value::Mapping(serde_yaml::Mapping::new())
    };
    if !matches!(root, serde_yaml::Value::Mapping(_)) {
        root = serde_yaml::Value::Mapping(serde_yaml::Mapping::new());
    }
    let mapping = root
        .as_mapping_mut()
        .ok_or("主题配置根节点必须是 YAML mapping")?;
    mapping
        .entry(serde_yaml::Value::String("version".into()))
        .or_insert_with(|| {
            serde_yaml::Value::Number(serde_yaml::Number::from(keytao_theme::THEME_SCHEMA_VERSION))
        });
    let ui_mapping = yaml_child_mapping(mapping, "ui", "主题 UI 配置必须是 YAML mapping")?;
    ui_mapping.remove(&serde_yaml::Value::String(legacy_key.into()));
    ui_mapping.insert(
        serde_yaml::Value::String(key.into()),
        serde_yaml::Value::Bool(value),
    );
    let content = serde_yaml::to_string(&root).map_err(|e| format!("序列化主题配置失败: {e}"))?;
    write_file_atomic(&theme_path, content.as_bytes())
        .map_err(|e| format!("写入主题配置失败 {}: {e}", theme_path.display()))
}

fn color_to_hex(color: keytao_theme::RgbaColor) -> String {
    format!("#{:02X}{:02X}{:02X}", color.red, color.green, color.blue)
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AndroidImeInputSettings {
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
    pub config_path: Option<String>,
    pub reload_stamp_path: Option<String>,
    pub message: String,
}

#[cfg(any(target_os = "android", target_os = "ios", test))]
fn mobile_keyboard_preferences(
    config: &serde_json::Map<String, serde_json::Value>,
    keyboard: &keytao_theme::KeyboardTheme,
) -> (bool, Vec<String>) {
    let symbol_auto_return = config
        .get("symbolAutoReturn")
        .and_then(|value| value.as_bool())
        .unwrap_or(keyboard.symbol_auto_return);
    let phrases = config
        .get("phrases")
        .and_then(|value| serde_json::from_value::<Vec<String>>(value.clone()).ok())
        .unwrap_or_else(|| keyboard.phrases.clone());
    (symbol_auto_return, phrases)
}

#[cfg(any(target_os = "android", target_os = "ios", test))]
fn write_mobile_keyboard_preferences(
    config: &mut serde_json::Map<String, serde_json::Value>,
    symbol_auto_return: bool,
    phrases: Vec<String>,
) {
    config.insert("symbolAutoReturn".into(), symbol_auto_return.into());
    config.insert("phrases".into(), phrases.into());
}

#[cfg(any(target_os = "android", target_os = "ios", test))]
#[derive(Debug, PartialEq)]
struct ClipboardPreferences {
    history_enabled: bool,
    history_limit: u16,
    suggestion_enabled: bool,
    suggestion_timeout_sec: u16,
    return_after_paste: bool,
    hide_sensitive: bool,
}

#[cfg(any(target_os = "android", target_os = "ios", test))]
fn clipboard_preferences(
    config: &serde_json::Map<String, serde_json::Value>,
) -> ClipboardPreferences {
    let flag = |key| {
        config
            .get(key)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true)
    };
    let integer = |key, default, min, max| {
        config
            .get(key)
            .and_then(serde_json::Value::as_f64)
            .filter(|value| value.is_finite() && value.fract() == 0.0)
            .unwrap_or(default)
            .clamp(min, max) as u16
    };
    ClipboardPreferences {
        history_enabled: flag("clipboardHistoryEnabled"),
        history_limit: integer("clipboardHistoryLimit", 50.0, 10.0, 200.0),
        suggestion_enabled: flag("clipboardSuggestionEnabled"),
        suggestion_timeout_sec: integer("clipboardSuggestionTimeoutSec", 30.0, 5.0, 120.0),
        return_after_paste: flag("clipboardReturnAfterPaste"),
        hide_sensitive: flag("clipboardHideSensitive"),
    }
}

#[cfg(any(target_os = "android", target_os = "ios", test))]
fn write_clipboard_preferences(
    config: &mut serde_json::Map<String, serde_json::Value>,
    clipboard: ClipboardPreferences,
) {
    for (key, value) in [
        ("clipboardHistoryEnabled", clipboard.history_enabled.into()),
        (
            "clipboardHistoryLimit",
            clipboard.history_limit.clamp(10, 200).into(),
        ),
        (
            "clipboardSuggestionEnabled",
            clipboard.suggestion_enabled.into(),
        ),
        (
            "clipboardSuggestionTimeoutSec",
            clipboard.suggestion_timeout_sec.clamp(5, 120).into(),
        ),
        (
            "clipboardReturnAfterPaste",
            clipboard.return_after_paste.into(),
        ),
        ("clipboardHideSensitive", clipboard.hide_sensitive.into()),
    ] {
        config.insert(key.into(), value);
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
fn normalize_enter_key_behavior(value: Option<&str>) -> String {
    match value
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "newline" | "linebreak" | "line_break" => "newline".into(),
        _ => "system".into(),
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
fn normalize_mobile_ime_delete_speed(value: Option<&str>) -> String {
    match value
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "slow" => "slow".into(),
        "fast" => "fast".into(),
        _ => "standard".into(),
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
fn normalize_mobile_ime_english_mode(value: Option<&str>) -> String {
    if value.is_some_and(|value| value.trim().eq_ignore_ascii_case("schema")) {
        "schema".into()
    } else {
        "ascii".into()
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
fn normalize_mobile_ime_backspace_gesture_mode(value: Option<&str>) -> String {
    if value.is_some_and(|value| value.eq_ignore_ascii_case("selectThenDelete")) {
        "selectThenDelete".into()
    } else {
        "immediate".into()
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
fn normalize_mobile_ime_keyboard_height_scale(value: Option<u64>) -> u8 {
    match value {
        Some(value) if (85..=130).contains(&value) && value % 5 == 0 => value as u8,
        _ => 100,
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
fn android_ime_haptics_settings_from_config(
    root: &Path,
    message: String,
) -> AndroidImeInputSettings {
    let path = android_ime_config_path(root);
    let config = read_android_ime_config(&path);
    let haptics = config.get("haptics").and_then(|value| value.as_object());
    let haptics_enabled = haptics
        .and_then(|value| value.get("enabled"))
        .and_then(|value| value.as_bool())
        .or_else(|| {
            config
                .get("hapticsEnabled")
                .and_then(|value| value.as_bool())
        })
        .unwrap_or(true);
    let haptic_intensity = haptics
        .and_then(|value| value.get("intensity"))
        .and_then(|value| value.as_u64())
        .or_else(|| {
            config
                .get("hapticIntensity")
                .and_then(|value| value.as_u64())
        })
        .unwrap_or(42)
        .clamp(1, 100) as u8;
    let enter_key_behavior = normalize_enter_key_behavior(
        config
            .get("enterKeyBehavior")
            .and_then(|value| value.as_str()),
    );
    let key_preview_enabled = config
        .get("keyPreviewEnabled")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    let long_press_delay_ms = config
        .get("longPressDelayMs")
        .and_then(|value| value.as_u64())
        .unwrap_or(300)
        .clamp(100, 700) as u16;
    let keyboard_height_scale = normalize_mobile_ime_keyboard_height_scale(
        config.get("keyboardHeightScale").and_then(|value| {
            value.as_u64().or_else(|| {
                value
                    .as_f64()
                    .filter(|value| value.fract() == 0.0)
                    .map(|value| value as u64)
            })
        }),
    );
    let delete_speed = normalize_mobile_ime_delete_speed(
        config.get("deleteSpeed").and_then(|value| value.as_str()),
    );
    let english_mode = normalize_mobile_ime_english_mode(
        config.get("englishMode").and_then(|value| value.as_str()),
    );
    let backspace_gesture_mode = normalize_mobile_ime_backspace_gesture_mode(
        config
            .get("backspaceGestureMode")
            .and_then(|value| value.as_str()),
    );
    let keyboard_path = root.join("keyboard.yaml");
    let keyboard = keytao_theme::resolve_keyboard_from_paths(
        None,
        keyboard_path.is_file().then_some(keyboard_path.as_path()),
    );
    let keyboard_height_dp = config
        .get("keyboardHeightDp")
        .and_then(|value| value.as_u64())
        .unwrap_or(keyboard.height.round() as u64)
        .clamp(160, 420) as u16;
    let candidate_bar_height_dp = config
        .get("candidateBarHeightDp")
        .and_then(|value| value.as_u64())
        .unwrap_or(keyboard.candidate_bar_height.round() as u64)
        .clamp(36, 96) as u8;
    let swipe_threshold_dp = config
        .get("swipeThresholdDp")
        .and_then(|value| value.as_f64())
        .unwrap_or(34.0)
        .clamp(12.0, 96.0) as f32;
    let key_sound_enabled = config
        .get("keySoundEnabled")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    let key_sound_volume = config
        .get("keySoundVolume")
        .and_then(|value| value.as_u64())
        .unwrap_or(100)
        .clamp(0, 100) as u8;
    let key_hint_visible = config
        .get("keyHintVisible")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    let flick_keys_enabled = config
        .get("flickKeysEnabled")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    let number_row_enabled = config
        .get("numberRowEnabled")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let candidate_font_scale = config
        .get("candidateFontScale")
        .and_then(|value| value.as_f64())
        .unwrap_or(1.0)
        .clamp(0.8, 1.4) as f32;
    let double_space_period_enabled = config
        .get("doubleSpacePeriodEnabled")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    let (symbol_auto_return, phrases) = mobile_keyboard_preferences(&config, &keyboard);
    let clipboard = clipboard_preferences(&config);
    let floating = config.get("floating").and_then(|value| value.as_object());
    let portrait = floating
        .and_then(|value| value.get("portrait"))
        .and_then(|value| value.as_object());
    let landscape = floating
        .and_then(|value| value.get("landscape"))
        .and_then(|value| value.as_object());
    let floating_portrait_enabled = portrait
        .and_then(|value| value.get("enabled"))
        .and_then(|value| value.as_bool())
        .unwrap_or(keyboard.floating.portrait.enabled);
    let floating_portrait_scale = floating_scale_percent(
        portrait
            .and_then(|value| value.get("scale"))
            .and_then(|value| value.as_f64()),
        keyboard.floating.portrait.scale,
        0.70,
    );
    let floating_landscape_enabled = landscape
        .and_then(|value| value.get("enabled"))
        .and_then(|value| value.as_bool())
        .unwrap_or(keyboard.floating.landscape.enabled);
    let floating_landscape_scale = floating_scale_percent(
        landscape
            .and_then(|value| value.get("scale"))
            .and_then(|value| value.as_f64()),
        keyboard.floating.landscape.scale,
        0.45,
    );

    AndroidImeInputSettings {
        haptics_enabled,
        haptic_intensity,
        enter_key_behavior,
        key_preview_enabled,
        long_press_delay_ms,
        keyboard_height_scale,
        delete_speed,
        english_mode,
        backspace_gesture_mode,
        keyboard_height_dp,
        candidate_bar_height_dp,
        swipe_threshold_dp,
        key_sound_enabled,
        key_sound_volume,
        key_hint_visible,
        flick_keys_enabled,
        number_row_enabled,
        candidate_font_scale,
        double_space_period_enabled,
        symbol_auto_return,
        phrases,
        clipboard_history_enabled: clipboard.history_enabled,
        clipboard_history_limit: clipboard.history_limit,
        clipboard_suggestion_enabled: clipboard.suggestion_enabled,
        clipboard_suggestion_timeout_sec: clipboard.suggestion_timeout_sec,
        clipboard_return_after_paste: clipboard.return_after_paste,
        clipboard_hide_sensitive: clipboard.hide_sensitive,
        floating_portrait_enabled,
        floating_portrait_scale,
        floating_landscape_enabled,
        floating_landscape_scale,
        config_path: Some(path_string(path)),
        reload_stamp_path: Some(path_string({
            #[cfg(target_os = "ios")]
            {
                ios_reload_stamp_path(root)
            }
            #[cfg(target_os = "android")]
            {
                android_reload_stamp_path(root)
            }
        })),
        message,
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
fn floating_scale_percent(value: Option<f64>, default_scale: f32, minimum_scale: f64) -> u8 {
    let value = value.unwrap_or(default_scale as f64);
    let ratio = if value > 1.5 { value / 100.0 } else { value };
    (ratio.clamp(minimum_scale, 1.0) * 100.0).round() as u8
}

pub async fn get_android_ime_input_settings(
    host: impl ImeHost,
) -> Result<AndroidImeInputSettings, CoreError> {
    let result: Result<_, String> = async {
        #[cfg(target_os = "android")]
        {
            let root = host.user_root()?;
            Ok(android_ime_haptics_settings_from_config(
                &root,
                "已读取 Android 输入反馈配置".into(),
            ))
        }
        #[cfg(target_os = "ios")]
        {
            let root = host.user_root()?;
            Ok(android_ime_haptics_settings_from_config(
                &root,
                "已读取 iOS 输入反馈配置".into(),
            ))
        }
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            let _ = host;
            Err("Mobile IME input settings are only available on Android and iOS".into())
        }
    }
    .await;
    result.map_err(CoreError::Other)
}

#[cfg_attr(
    not(any(target_os = "android", target_os = "ios")),
    allow(unused_variables)
)]
pub async fn set_android_ime_input_settings(
    host: impl ImeHost,
    haptics_enabled: bool,
    haptic_intensity: u8,
    enter_key_behavior: String,
    key_preview_enabled: bool,
    long_press_delay_ms: u16,
    keyboard_height_scale: u8,
    delete_speed: String,
    english_mode: String,
    backspace_gesture_mode: String,
    keyboard_height_dp: u16,
    candidate_bar_height_dp: u8,
    swipe_threshold_dp: f32,
    key_sound_enabled: bool,
    key_sound_volume: u8,
    key_hint_visible: bool,
    flick_keys_enabled: bool,
    number_row_enabled: bool,
    candidate_font_scale: f32,
    double_space_period_enabled: bool,
    symbol_auto_return: bool,
    phrases: Vec<String>,
    clipboard_history_enabled: bool,
    clipboard_history_limit: u16,
    clipboard_suggestion_enabled: bool,
    clipboard_suggestion_timeout_sec: u16,
    clipboard_return_after_paste: bool,
    clipboard_hide_sensitive: bool,
    floating_portrait_enabled: bool,
    floating_portrait_scale: u8,
    floating_landscape_enabled: bool,
    floating_landscape_scale: u8,
) -> Result<AndroidImeInputSettings, CoreError> {
    let result: Result<_, String> = async {
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            #[cfg(target_os = "android")]
            let root = host.user_root()?;
            #[cfg(target_os = "ios")]
            let root = host.user_root()?;
            let path = android_ime_config_path(&root);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("创建移动端输入法配置目录失败: {e}"))?;
            }
            let mut config = read_android_ime_config(&path);
            let mut haptics = config
                .get("haptics")
                .and_then(|value| value.as_object())
                .cloned()
                .unwrap_or_default();
            haptics.insert("enabled".into(), serde_json::Value::Bool(haptics_enabled));
            haptics.insert(
                "intensity".into(),
                serde_json::Value::from(haptic_intensity.clamp(1, 100)),
            );
            config.insert("haptics".into(), serde_json::Value::Object(haptics));
            config.insert(
                "enterKeyBehavior".into(),
                serde_json::Value::String(normalize_enter_key_behavior(Some(&enter_key_behavior))),
            );
            config.insert(
                "keyPreviewEnabled".into(),
                serde_json::Value::Bool(key_preview_enabled),
            );
            config.insert(
                "longPressDelayMs".into(),
                serde_json::Value::from(long_press_delay_ms.clamp(100, 700)),
            );
            config.insert(
                "keyboardHeightScale".into(),
                serde_json::Value::from(normalize_mobile_ime_keyboard_height_scale(Some(
                    keyboard_height_scale.into(),
                ))),
            );
            config.insert(
                "deleteSpeed".into(),
                serde_json::Value::String(normalize_mobile_ime_delete_speed(Some(&delete_speed))),
            );
            config.insert(
                "englishMode".into(),
                serde_json::Value::String(normalize_mobile_ime_english_mode(Some(&english_mode))),
            );
            config.insert(
                "backspaceGestureMode".into(),
                serde_json::Value::String(normalize_mobile_ime_backspace_gesture_mode(Some(
                    &backspace_gesture_mode,
                ))),
            );
            config.insert(
                "keyboardHeightDp".into(),
                serde_json::Value::from(keyboard_height_dp.clamp(160, 420)),
            );
            config.insert(
                "candidateBarHeightDp".into(),
                serde_json::Value::from(candidate_bar_height_dp.clamp(36, 96)),
            );
            config.insert(
                "swipeThresholdDp".into(),
                serde_json::Value::from(swipe_threshold_dp.clamp(12.0, 96.0)),
            );
            config.insert(
                "keySoundEnabled".into(),
                serde_json::Value::Bool(key_sound_enabled),
            );
            config.insert(
                "keySoundVolume".into(),
                serde_json::Value::from(key_sound_volume.clamp(0, 100)),
            );
            config.insert(
                "keyHintVisible".into(),
                serde_json::Value::Bool(key_hint_visible),
            );
            config.insert(
                "flickKeysEnabled".into(),
                serde_json::Value::Bool(flick_keys_enabled),
            );
            config.insert(
                "numberRowEnabled".into(),
                serde_json::Value::Bool(number_row_enabled),
            );
            config.insert(
                "candidateFontScale".into(),
                serde_json::Value::from(candidate_font_scale.clamp(0.8, 1.4)),
            );
            config.insert(
                "doubleSpacePeriodEnabled".into(),
                serde_json::Value::Bool(double_space_period_enabled),
            );
            write_mobile_keyboard_preferences(&mut config, symbol_auto_return, phrases);
            write_clipboard_preferences(
                &mut config,
                ClipboardPreferences {
                    history_enabled: clipboard_history_enabled,
                    history_limit: clipboard_history_limit,
                    suggestion_enabled: clipboard_suggestion_enabled,
                    suggestion_timeout_sec: clipboard_suggestion_timeout_sec,
                    return_after_paste: clipboard_return_after_paste,
                    hide_sensitive: clipboard_hide_sensitive,
                },
            );
            let mut floating = config
                .get("floating")
                .and_then(|value| value.as_object())
                .cloned()
                .unwrap_or_default();
            let mut portrait = floating
                .get("portrait")
                .and_then(|value| value.as_object())
                .cloned()
                .unwrap_or_default();
            portrait.insert(
                "enabled".into(),
                serde_json::Value::Bool(floating_portrait_enabled),
            );
            portrait.insert(
                "scale".into(),
                serde_json::Value::from(floating_portrait_scale.clamp(70, 100) as f64 / 100.0),
            );
            floating.insert("portrait".into(), serde_json::Value::Object(portrait));
            let mut landscape = floating
                .get("landscape")
                .and_then(|value| value.as_object())
                .cloned()
                .unwrap_or_default();
            landscape.insert(
                "enabled".into(),
                serde_json::Value::Bool(floating_landscape_enabled),
            );
            landscape.insert(
                "scale".into(),
                serde_json::Value::from(floating_landscape_scale.clamp(45, 100) as f64 / 100.0),
            );
            floating.insert("landscape".into(), serde_json::Value::Object(landscape));
            config.insert("floating".into(), serde_json::Value::Object(floating));

            let content = serde_json::to_string_pretty(&serde_json::Value::Object(config))
                .map_err(|e| format!("序列化移动端输入法配置失败: {e}"))?;
            write_file_atomic(&path, format!("{content}\n").as_bytes())
                .map_err(|e| format!("写入移动端输入法配置失败 {}: {e}", path.display()))?;

            #[cfg(target_os = "android")]
            let reload_result = write_android_reload_stamp(&root);
            #[cfg(target_os = "ios")]
            let reload_result = write_ios_reload_stamp(&root);
            let platform = {
                #[cfg(target_os = "android")]
                {
                    "Android"
                }
                #[cfg(target_os = "ios")]
                {
                    "iOS"
                }
            };
            let message = match reload_result {
                Ok(stamp) => format!(
                    "已保存 {platform} 输入反馈配置并通知输入法重载：{}",
                    stamp.display()
                ),
                Err(e) => format!("已保存 {platform} 输入反馈配置，但输入法重载通知失败：{e}"),
            };
            Ok(android_ime_haptics_settings_from_config(&root, message))
        }
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            let _ = host;
            let _ = haptics_enabled;
            let _ = haptic_intensity;
            let _ = enter_key_behavior;
            let _ = key_preview_enabled;
            let _ = long_press_delay_ms;
            let _ = keyboard_height_scale;
            let _ = delete_speed;
            let _ = english_mode;
            let _ = floating_portrait_enabled;
            let _ = floating_portrait_scale;
            let _ = floating_landscape_enabled;
            let _ = floating_landscape_scale;
            Err("Mobile IME input settings are only available on Android and iOS".into())
        }
    }
    .await;
    result.map_err(CoreError::Other)
}
pub fn get_desktop_english_mode(
    host: impl ImeHost,
) -> Result<keytao_core::english_mode::EnglishMode, CoreError> {
    let result: Result<_, String> = (|| {
        let root = host.user_root()?;
        Ok(keytao_core::english_mode::read_english_mode(&root))
    })();
    result.map_err(CoreError::Other)
}

pub fn get_schema_switches(
    host: impl ImeHost,
) -> Result<Vec<keytao_core::SchemaSwitchSetting>, CoreError> {
    let root = host.user_root().map_err(CoreError::Other)?;
    keytao_core::list_schema_switches(&root).map_err(CoreError::Other)
}

#[cfg(any(target_os = "windows", test))]
pub(crate) fn schema_switch_settings_file(
    root: &std::path::Path,
) -> Result<(PathBuf, Vec<u8>), String> {
    let request = keytao_core::read_schema_switch_request(root)?;
    Ok((
        keytao_core::SCHEMA_SWITCH_REQUEST_FILE.into(),
        serde_json::to_vec(&request).map_err(|error| error.to_string())?,
    ))
}

pub fn set_schema_switch(
    host: impl ImeHost,
    schema_id: &str,
    name: &str,
    value: usize,
) -> Result<Vec<keytao_core::SchemaSwitchSetting>, CoreError> {
    let root = host.user_root().map_err(CoreError::Other)?;
    let switches =
        keytao_core::set_schema_switch(&root, schema_id, name, value).map_err(CoreError::Other)?;
    #[cfg(target_os = "windows")]
    host.publish_schema_switches().map_err(CoreError::Other)?;
    host.write_reload_stamp(&root).map_err(CoreError::Other)?;
    Ok(switches)
}

pub async fn set_desktop_english_mode(
    host: impl ImeHost,
    mode: keytao_core::english_mode::EnglishMode,
) -> Result<keytao_core::english_mode::EnglishMode, CoreError> {
    let result: Result<_, String> = async {
        tokio::task::spawn_blocking(move || {
            use keytao_core::english_mode::{EnglishMode, write_english_mode};
            let root = host.user_root()?;
            if mode == EnglishMode::Schema && !deployed_english_schema_available(&root) {
                return Err("请先安装并部署附加方案 English".into());
            }
            #[cfg(target_os = "windows")]
            host.publish_english_settings(mode == EnglishMode::Schema)?;
            write_english_mode(&root, mode)?;
            host.write_reload_stamp(&root)?;
            Ok(mode)
        })
        .await
        .map_err(|e| format!("保存英文模式失败：{e}"))?
    }
    .await;
    result.map_err(CoreError::Other)
}
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn get_ime_ui_settings() -> Result<ImeUiSettings, CoreError> {
    let result: Result<_, String> =
        (|| ime_ui_settings_with_message("已读取输入法 UI 配置".into()))();
    result.map_err(CoreError::Other)
}

#[cfg(target_os = "android")]
pub fn get_ime_ui_settings(host: impl ImeHost) -> Result<ImeUiSettings, CoreError> {
    let result: Result<_, String> = (|| {
        let root = host.user_root()?;
        ime_ui_settings_from_paths(
            root.join("theme.yaml"),
            Some(android_reload_stamp_path(&root)),
            "已读取 Android 输入法 UI 配置".into(),
        )
    })();
    result.map_err(CoreError::Other)
}

#[cfg(target_os = "ios")]
pub fn get_ime_ui_settings(host: impl ImeHost) -> Result<ImeUiSettings, CoreError> {
    let result: Result<_, String> = (|| {
        let root = host.user_root()?;
        ime_ui_settings_from_paths(
            root.join("theme.yaml"),
            Some(ios_reload_stamp_path(&root)),
            "已读取 iOS 输入法 UI 配置".into(),
        )
    })();
    result.map_err(CoreError::Other)
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn set_ime_embedded_composition(embedded: bool) -> Result<ImeUiSettings, CoreError> {
    let result: Result<_, String> = (|| {
        write_ime_embedded_composition(embedded)?;
        let reload_message = match write_keytao_ime_reload_stamp() {
            Ok(()) => "已保存嵌入模式设置并通知系统输入法重载".to_string(),
            Err(e) => format!("已保存嵌入模式设置，但系统输入法重载通知失败：{e}"),
        };
        ime_ui_settings_with_message(reload_message)
    })();
    result.map_err(CoreError::Other)
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn set_ime_hide_candidate_panel(hide: bool) -> Result<ImeUiSettings, CoreError> {
    let result: Result<_, String> = (|| {
        write_ime_hide_candidate_panel_to_path(ime_theme_path()?, hide)?;
        let reload_message = match write_keytao_ime_reload_stamp() {
            Ok(()) => "已保存无候选框模式设置并通知系统输入法重载".to_string(),
            Err(e) => format!("已保存无候选框模式设置，但系统输入法重载通知失败：{e}"),
        };
        ime_ui_settings_with_message(reload_message)
    })();
    result.map_err(CoreError::Other)
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn set_ime_show_tray_mode_indicator(show: bool) -> Result<ImeUiSettings, CoreError> {
    let result: Result<_, String> = (|| {
        write_ime_ui_flag_to_path(
            ime_theme_path()?,
            "showTrayModeIndicator",
            "show_tray_mode_indicator",
            show,
        )?;
        let reload_message = match write_keytao_ime_reload_stamp() {
            Ok(()) => "已保存输入法 UI 配置并通知系统输入法重载".to_string(),
            Err(e) => format!("已保存输入法 UI 配置，但系统输入法重载通知失败：{e}"),
        };
        ime_ui_settings_with_message(reload_message)
    })();
    result.map_err(CoreError::Other)
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn set_ime_ui_settings(
    color_scheme: keytao_theme::UiColorScheme,
    orientation: keytao_theme::PanelOrientation,
    accent_color: Option<String>,
    font_size: f32,
    corner_radius: Option<f32>,
    background_color: String,
) -> Result<ImeUiSettings, CoreError> {
    let result: Result<_, String> = (|| {
        write_ime_ui_settings(
            color_scheme,
            orientation,
            accent_color,
            font_size,
            corner_radius,
            background_color,
        )?;
        let reload_message = match write_keytao_ime_reload_stamp() {
            Ok(()) => "已保存输入法 UI 配置并通知系统输入法重载".to_string(),
            Err(e) => format!("已保存输入法 UI 配置，但系统输入法重载通知失败：{e}"),
        };
        ime_ui_settings_with_message(reload_message)
    })();
    result.map_err(CoreError::Other)
}

#[cfg(target_os = "android")]
pub fn set_ime_ui_settings(
    host: impl ImeHost,
    color_scheme: keytao_theme::UiColorScheme,
    orientation: keytao_theme::PanelOrientation,
    accent_color: Option<String>,
    font_size: f32,
    corner_radius: Option<f32>,
    background_color: String,
) -> Result<ImeUiSettings, CoreError> {
    let result: Result<_, String> = (|| {
        let root = host.user_root()?;
        let theme_path = root.join("theme.yaml");
        keytao_theme::write_ime_ui_settings_to_path(
            &theme_path,
            color_scheme,
            orientation,
            accent_color,
            font_size,
            corner_radius,
            background_color,
        )?;
        let reload_message = match write_android_reload_stamp(&root) {
            Ok(path) => format!(
                "已保存 Android 输入法 UI 配置并通知输入法重载：{}",
                path.display()
            ),
            Err(e) => format!("已保存 Android 输入法 UI 配置，但输入法重载通知失败：{e}"),
        };
        ime_ui_settings_from_paths(
            theme_path,
            Some(android_reload_stamp_path(&root)),
            reload_message,
        )
    })();
    result.map_err(CoreError::Other)
}

#[cfg(target_os = "ios")]
pub fn set_ime_ui_settings(
    host: impl ImeHost,
    color_scheme: keytao_theme::UiColorScheme,
    orientation: keytao_theme::PanelOrientation,
    accent_color: Option<String>,
    font_size: f32,
    corner_radius: Option<f32>,
    background_color: String,
) -> Result<ImeUiSettings, CoreError> {
    let result: Result<_, String> = (|| {
        let root = host.user_root()?;
        let theme_path = root.join("theme.yaml");
        keytao_theme::write_ime_ui_settings_to_path(
            &theme_path,
            color_scheme,
            orientation,
            accent_color,
            font_size,
            corner_radius,
            background_color,
        )?;
        let reload_message = match write_ios_reload_stamp(&root) {
            Ok(path) => format!(
                "已保存 iOS 输入法 UI 配置并通知输入法重载：{}",
                path.display()
            ),
            Err(e) => format!("已保存 iOS 输入法 UI 配置，但输入法重载通知失败：{e}"),
        };
        ime_ui_settings_from_paths(
            theme_path,
            Some(ios_reload_stamp_path(&root)),
            reload_message,
        )
    })();
    result.map_err(CoreError::Other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_preferences_default_and_round_trip_without_overwriting_explicit_values() {
        let mut config = serde_json::json!({"unrelated": {"keep": true}})
            .as_object()
            .unwrap()
            .clone();
        assert_eq!(
            clipboard_preferences(&config),
            ClipboardPreferences {
                history_enabled: true,
                history_limit: 50,
                suggestion_enabled: true,
                suggestion_timeout_sec: 30,
                return_after_paste: true,
                hide_sensitive: true,
            }
        );
        for enabled in [false, true] {
            let preferences = ClipboardPreferences {
                history_enabled: enabled,
                history_limit: 75,
                suggestion_enabled: enabled,
                suggestion_timeout_sec: 45,
                return_after_paste: enabled,
                hide_sensitive: enabled,
            };
            write_clipboard_preferences(&mut config, preferences);
            let restored = serde_json::from_slice(&serde_json::to_vec(&config).unwrap()).unwrap();
            let actual = clipboard_preferences(&restored);
            assert_eq!(
                actual,
                ClipboardPreferences {
                    history_enabled: enabled,
                    history_limit: 75,
                    suggestion_enabled: enabled,
                    suggestion_timeout_sec: 45,
                    return_after_paste: enabled,
                    hide_sensitive: enabled,
                }
            );
            assert_eq!(restored["unrelated"]["keep"], true);
        }
    }

    #[test]
    fn clipboard_preferences_invalid_types_default_and_numbers_clamp() {
        for value in [
            serde_json::Value::Null,
            serde_json::json!("false"),
            serde_json::json!([]),
            serde_json::json!({}),
            serde_json::json!(1.5),
        ] {
            let config = serde_json::json!({
                "clipboardHistoryEnabled": value, "clipboardHistoryLimit": value,
                "clipboardSuggestionEnabled": value, "clipboardSuggestionTimeoutSec": value,
                "clipboardReturnAfterPaste": value, "clipboardHideSensitive": value,
            });
            assert_eq!(
                clipboard_preferences(config.as_object().unwrap()),
                clipboard_preferences(&serde_json::Map::new())
            );
        }
        for (value, limit, timeout) in [(-1000.0, 10, 5), (1000.0, 200, 120), (60.0, 60, 60)] {
            let mut config = serde_json::json!({
                "clipboardHistoryLimit": value, "clipboardSuggestionTimeoutSec": value,
            })
            .as_object()
            .unwrap()
            .clone();
            let mut preferences = clipboard_preferences(&config);
            assert_eq!(preferences.history_limit, limit);
            assert_eq!(preferences.suggestion_timeout_sec, timeout);
            preferences.history_limit = if value < 0.0 { 0 } else { u16::MAX };
            preferences.suggestion_timeout_sec = preferences.history_limit;
            write_clipboard_preferences(&mut config, preferences);
            assert_eq!(
                config["clipboardHistoryLimit"],
                if value < 0.0 { 10 } else { 200 }
            );
            assert_eq!(
                config["clipboardSuggestionTimeoutSec"],
                if value < 0.0 { 5 } else { 120 }
            );
        }
    }

    #[test]
    fn mobile_keyboard_preferences_round_trip_and_preserve_empty_phrases() {
        let keyboard = keytao_theme::resolve_keyboard_from_paths(None, None);
        let mut config = serde_json::json!({"unrelated": {"keep": true}})
            .as_object()
            .unwrap()
            .clone();
        assert_eq!(
            mobile_keyboard_preferences(&config, &keyboard),
            (
                false,
                vec!["谢谢", "好的", "收到", "稍等", "我在路上"]
                    .into_iter()
                    .map(String::from)
                    .collect()
            )
        );
        config.insert("phrases".into(), serde_json::json!([42]));
        config.insert("symbolAutoReturn".into(), serde_json::json!("invalid"));
        assert_eq!(
            mobile_keyboard_preferences(&config, &keyboard),
            (keyboard.symbol_auto_return, keyboard.phrases.clone())
        );
        for (enabled, phrases) in [(true, vec!["Custom phrase".into()]), (false, vec![])] {
            write_mobile_keyboard_preferences(&mut config, enabled, phrases.clone());
            let serialized = serde_json::to_vec(&config).unwrap();
            let restored = serde_json::from_slice(&serialized).unwrap();
            assert_eq!(
                mobile_keyboard_preferences(&restored, &keyboard),
                (enabled, phrases)
            );
            assert_eq!(restored["unrelated"]["keep"], true);
        }
    }

    #[test]
    fn candidate_page_size_defaults_and_preserves_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(keytao_core::english_mode::SETTINGS_FILE);
        assert_eq!(read_candidate_page_size(dir.path()), None);
        for value in ["null", "0", "11", "-1", "6.5", "\"6\"", "\"unknown\""] {
            std::fs::write(&path, format!("{{\"candidatePageSize\":{value}}}")).unwrap();
            assert_eq!(read_candidate_page_size(dir.path()), None, "{value}");
        }
        std::fs::write(&path, r#"{"englishMode":"schema","extra":{"keep":true}}"#).unwrap();
        for size in [Some(1), Some(10), None] {
            assert_eq!(set_candidate_page_size(dir.path(), size).unwrap(), size);
            assert_eq!(read_candidate_page_size(dir.path()), size);
            let saved: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(saved["englishMode"], "schema");
            assert_eq!(saved["extra"]["keep"], true);
            assert_eq!(saved.get("candidatePageSize").is_some(), size.is_some());
        }
        for invalid in ["[", "[]"] {
            std::fs::write(&path, invalid).unwrap();
            assert!(set_candidate_page_size(dir.path(), Some(6)).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), invalid);
        }
        assert!(set_candidate_page_size(dir.path(), Some(0)).is_err());
        assert!(set_candidate_page_size(dir.path(), Some(11)).is_err());
    }

    #[test]
    fn published_switches_include_only_the_request_and_its_generation() {
        let dir = tempfile::tempdir().unwrap();
        let (name, bytes) = schema_switch_settings_file(dir.path()).unwrap();
        assert_eq!(name, PathBuf::from(keytao_core::SCHEMA_SWITCH_REQUEST_FILE));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
            serde_json::json!({"generation": 0, "options": {}})
        );
        let user = "installation_id: private\nvar:\n  previously_selected_schema: private_schema\n  option:\n    mars: true\n    punctuation_ascii: false\n    punctuation_cjk: true\n    private_text: secret\n    42: true\n";
        std::fs::write(dir.path().join("user.yaml"), user).unwrap();
        let request = br#"{"generation":42,"options":{"mars":false,"punctuation_ascii":true,"punctuation_cjk":false}}"#;
        std::fs::write(
            dir.path().join(keytao_core::SCHEMA_SWITCH_REQUEST_FILE),
            request,
        )
        .unwrap();
        let (_, bytes) = schema_switch_settings_file(dir.path()).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
            serde_json::from_slice::<serde_json::Value>(request).unwrap()
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("user.yaml")).unwrap(),
            user
        );
        std::fs::write(dir.path().join("user.yaml"), "var: [").unwrap();
        assert!(schema_switch_settings_file(dir.path()).is_ok());
        std::fs::write(
            dir.path().join(keytao_core::SCHEMA_SWITCH_REQUEST_FILE),
            "{",
        )
        .unwrap();
        assert!(schema_switch_settings_file(dir.path()).is_err());
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    #[test]
    fn embedded_composition_preserves_unrelated_settings_and_replaces_legacy_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("theme.yaml");
        std::fs::write(&path, "version: 7\nui:\n  embedded_composition: false\n  accentColor: '#123456'\ncandidate:\n  labelSuffix: ')'\n").unwrap();
        write_ime_embedded_composition_to_path(path.clone(), true).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        let root: serde_yaml::Value = serde_yaml::from_str(&content).unwrap();
        assert_eq!(root["version"].as_u64(), Some(7));
        assert_eq!(root["ui"]["embeddedComposition"].as_bool(), Some(true));
        assert!(
            root["ui"]
                .as_mapping()
                .unwrap()
                .get("embedded_composition")
                .is_none()
        );
        assert_eq!(root["ui"]["accentColor"].as_str(), Some("#123456"));
        assert_eq!(root["candidate"]["labelSuffix"].as_str(), Some(")"));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    #[test]
    fn embedded_composition_rejects_invalid_yaml_without_overwriting_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("theme.yaml");
        for original in ["ui: [", "ui:\n  embeddedComposition: [true"] {
            std::fs::write(&path, original).unwrap();
            assert!(write_ime_embedded_composition_to_path(path.clone(), true).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    #[test]
    fn embedded_composition_creates_missing_parent_and_theme() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/theme.yaml");
        write_ime_embedded_composition_to_path(path.clone(), false).unwrap();
        let root: serde_yaml::Value =
            serde_yaml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(
            root["version"].as_u64(),
            Some(keytao_theme::THEME_SCHEMA_VERSION.into())
        );
        assert_eq!(root["ui"]["embeddedComposition"].as_bool(), Some(false));
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    #[test]
    fn hide_candidate_panel_round_trip_preserves_embedded_and_unrelated_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("theme.yaml");
        std::fs::write(&path, "version: 7\nui:\n  embeddedComposition: false\n  hide_candidate_panel: false\n  accentColor: '#123456'\ncandidate:\n  labelSuffix: ')'\n").unwrap();

        for hide in [true, false] {
            write_ime_hide_candidate_panel_to_path(path.clone(), hide).unwrap();
            keytao_theme::write_ime_ui_settings_to_path(
                &path,
                keytao_theme::UiColorScheme::Dark,
                keytao_theme::PanelOrientation::Horizontal,
                Some("#123456".into()),
                24.0,
                None,
                String::new(),
            )
            .unwrap();
            let root: serde_yaml::Value =
                serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(root["version"].as_u64(), Some(7));
            assert_eq!(root["ui"]["hideCandidatePanel"].as_bool(), Some(hide));
            assert!(root["ui"].get("hide_candidate_panel").is_none());
            assert_eq!(root["ui"]["embeddedComposition"].as_bool(), Some(false));
            assert_eq!(root["ui"]["accentColor"].as_str(), Some("#123456"));
            assert_eq!(root["candidate"]["labelSuffix"].as_str(), Some(")"));
            let settings = ime_ui_settings_from_paths(path.clone(), None, String::new()).unwrap();
            assert_eq!(settings.hide_candidate_panel, hide);
            assert!(!settings.embedded_composition);
            assert_eq!(settings.effective_embedded_composition, hide);
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    #[test]
    fn hide_candidate_panel_rejects_invalid_yaml_without_overwriting_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("theme.yaml");
        for original in ["ui: [", "ui:\n  hideCandidatePanel: [true"] {
            std::fs::write(&path, original).unwrap();
            assert!(write_ime_hide_candidate_panel_to_path(path.clone(), true).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    #[test]
    fn hide_candidate_panel_defaults_off_and_creates_missing_parent_and_theme() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/theme.yaml");
        let settings = ime_ui_settings_from_paths(path.clone(), None, String::new()).unwrap();
        assert!(!settings.hide_candidate_panel);
        assert!(!settings.embedded_composition);
        assert!(!settings.effective_embedded_composition);

        write_ime_hide_candidate_panel_to_path(path.clone(), true).unwrap();
        let root: serde_yaml::Value =
            serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            root["version"].as_u64(),
            Some(keytao_theme::THEME_SCHEMA_VERSION.into())
        );
        assert_eq!(root["ui"]["hideCandidatePanel"].as_bool(), Some(true));
        let settings = ime_ui_settings_from_paths(path, None, String::new()).unwrap();
        assert!(settings.hide_candidate_panel);
        assert!(!settings.embedded_composition);
        assert!(settings.effective_embedded_composition);
    }

    #[test]
    fn appearance_edits_preserve_unset_and_explicit_accents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("theme.yaml");
        for accent in [None, Some("#123456".to_string())] {
            keytao_theme::write_theme_ui_to_path(
                &path,
                keytao_theme::UiColorScheme::Auto,
                accent.clone(),
            )
            .unwrap();
            keytao_theme::write_ime_ui_settings_to_path(
                &path,
                keytao_theme::UiColorScheme::Auto,
                keytao_theme::PanelOrientation::Horizontal,
                None,
                20.0,
                Some(24.0),
                "#654321".into(),
            )
            .unwrap();
            let settings = ime_ui_settings_from_paths(path.clone(), None, String::new()).unwrap();
            assert_eq!(settings.accent_color_override, accent);
            assert_eq!(settings.corner_radius, Some(24.0));
            assert_eq!(settings.background_color, "#654321");
            let written: serde_yaml::Value =
                serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(written["ui"]["accentColor"].as_str(), accent.as_deref());
        }
    }

    #[test]
    fn test_write_ime_ui_settings_persists_candidate_font_size() {
        let dir = std::env::temp_dir().join(format!(
            "keytao-ime-ui-settings-test-{}",
            std::process::id()
        ));
        let theme_path = dir.join("theme.yaml");
        std::fs::create_dir_all(&dir).expect("create theme dir");
        std::fs::write(&theme_path, "candidate:\n  labelSuffix: ')'\n")
            .expect("write existing theme");

        keytao_theme::write_ime_ui_settings_to_path(
            &theme_path,
            keytao_theme::UiColorScheme::Dark,
            keytao_theme::PanelOrientation::Horizontal,
            Some("#123456".into()),
            27.0,
            Some(24.0),
            "#654321".into(),
        )
        .expect("write IME UI settings");

        let theme = keytao_theme::ThemeResolver::new(None, Some(theme_path.clone())).current();
        assert_eq!(theme.font.size, 27.0);
        assert_eq!(
            theme.panel.orientation,
            keytao_theme::PanelOrientation::Horizontal
        );
        assert_eq!(theme.candidate.label_suffix, ")");

        let settings = ime_ui_settings_from_paths(theme_path, None, String::new())
            .expect("read IME UI settings");
        assert_eq!(settings.font_size, 27.0);
        assert_eq!(settings.accent_color, "#123456");
        assert_eq!(settings.accent_color_override.as_deref(), Some("#123456"));
        assert_eq!(settings.corner_radius, Some(24.0));
        assert_eq!(settings.background_color, "#654321");

        std::fs::remove_dir_all(dir).ok();
    }
}
