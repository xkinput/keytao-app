#[cfg(any(target_os = "android", target_os = "ios"))]
use crate::addon::{android_ime_config_path, read_android_ime_config};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use crate::scheme_files::yaml_child_mapping;
use crate::{
    CoreError, addon::deployed_english_schema_available, ime_host::ImeHost, ime_paths::*,
    scheme_files::write_file_atomic,
};
use serde::{Deserialize, Serialize};
#[cfg(any(target_os = "android", target_os = "ios"))]
use std::path::Path;
use std::path::PathBuf;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ImeUiSettings {
    pub color_scheme: keytao_theme::UiColorScheme,
    pub effective_color_scheme: keytao_theme::EffectiveColorScheme,
    pub orientation: keytao_theme::PanelOrientation,
    pub embedded_composition: bool,
    pub accent_color: String,
    pub font_size: f32,
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
        accent_color: color_to_hex(accent_color),
        font_size: theme.font.size,
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
    accent_color: String,
    font_size: f32,
) -> Result<(), String> {
    let theme_path = ime_theme_path()?;
    keytao_theme::write_ime_ui_settings_to_path(
        &theme_path,
        color_scheme,
        orientation,
        accent_color,
        font_size,
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
    ui_mapping.remove(&serde_yaml::Value::String("embedded_composition".into()));
    ui_mapping.insert(
        serde_yaml::Value::String("embeddedComposition".into()),
        serde_yaml::Value::Bool(embedded),
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
    pub floating_portrait_enabled: bool,
    pub floating_portrait_scale: u8,
    pub floating_landscape_enabled: bool,
    pub floating_landscape_scale: u8,
    pub config_path: Option<String>,
    pub reload_stamp_path: Option<String>,
    pub message: String,
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
pub fn set_ime_ui_settings(
    color_scheme: keytao_theme::UiColorScheme,
    orientation: keytao_theme::PanelOrientation,
    accent_color: String,
    font_size: f32,
) -> Result<ImeUiSettings, CoreError> {
    let result: Result<_, String> = (|| {
        write_ime_ui_settings(color_scheme, orientation, accent_color, font_size)?;
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
    accent_color: String,
    font_size: f32,
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
    accent_color: String,
    font_size: f32,
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
            "#123456".into(),
            27.0,
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

        std::fs::remove_dir_all(dir).ok();
    }
}
