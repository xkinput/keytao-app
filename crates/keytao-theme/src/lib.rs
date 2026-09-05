//! Shared KeyTao IME theme language.
//!
//! This crate owns the cross-platform theme schema, default values, merge rules,
//! and view models. Platform frontends render the resolved model with their own
//! native UI stack.
//!
//! The shared theme covers candidate panel and mode hint semantics only. The
//! mobile soft keyboard layout is an Android/iOS adapter concern and lives in
//! [`mobile_layout`], resolved from its own `keyboard.yaml` document.

pub mod mobile_layout;

use serde::{Deserialize, Deserializer, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use mobile_layout::PartialMobileLayout;

/// Compatibility aliases for call sites that still use the pre-split names.
/// New code should use the [`mobile_layout`] module directly.
pub use mobile_layout::{
    default_mobile_layout_yaml as default_keyboard_yaml,
    mobile_layout_json as resolved_keyboard_json,
    resolve_mobile_layout_from_paths as resolve_keyboard_from_paths,
    MobileCommand as KeyboardCommandTheme, MobileFloatingLayout as KeyboardFloatingTheme,
    MobileFloatingProfile as KeyboardFloatingProfileTheme, MobileKey as KeyboardKeyTheme,
    MobileKeyStackItem as KeyboardKeyStackItemTheme, MobileLayout as KeyboardTheme,
    DEFAULT_MOBILE_LAYOUT_YAML as DEFAULT_KEYBOARD_YAML,
};

pub const THEME_SCHEMA_VERSION: u32 = 2;
pub const DEFAULT_THEME_YAML: &str = include_str!("../default-theme.yaml");
pub const MIN_CANDIDATE_FONT_SIZE: f32 = 10.0;
pub const MAX_CANDIDATE_FONT_SIZE: f32 = 36.0;

/// Atomically update the complete set of App-owned IME UI settings while
/// preserving every unrelated key in the user's theme document.
pub fn write_ime_ui_settings_to_path(
    theme_path: &Path,
    color_scheme: UiColorScheme,
    orientation: PanelOrientation,
    accent_color: String,
    font_size: f32,
) -> Result<(), String> {
    let accent_color = normalize_hex_color(&accent_color)?;
    let font_size = normalize_candidate_font_size(font_size)?;
    let mut root = read_theme_yaml_mapping(theme_path)?;
    let mapping = root
        .as_mapping_mut()
        .ok_or("主题配置根节点必须是 YAML mapping")?;
    ensure_theme_version(mapping);
    write_theme_ui_mapping(mapping, color_scheme, Some(accent_color))?;

    let panel_mapping = yaml_child_mapping(mapping, "panel", "主题面板配置必须是 YAML mapping")?;
    panel_mapping.insert(
        serde_yaml::Value::String("orientation".into()),
        serde_yaml::Value::String(
            match orientation {
                PanelOrientation::Horizontal => "horizontal",
                PanelOrientation::Vertical => "vertical",
            }
            .into(),
        ),
    );

    let font_mapping = yaml_child_mapping(mapping, "font", "主题字体配置必须是 YAML mapping")?;
    font_mapping.insert(
        serde_yaml::Value::String("size".into()),
        serde_yaml::to_value(font_size).map_err(|e| format!("序列化候选字号失败: {e}"))?,
    );
    write_theme_yaml_atomic(theme_path, &root)
}

/// Atomically update the mobile keyboard's color scheme and, when supplied,
/// its accent. `None` preserves `ui.accentColor`; an empty string removes it.
pub fn write_theme_ui_to_path(
    theme_path: &Path,
    color_scheme: UiColorScheme,
    accent_color: Option<String>,
) -> Result<(), String> {
    let accent_color = match accent_color {
        Some(value) if value.trim().is_empty() => Some(String::new()),
        Some(value) => Some(normalize_hex_color(&value)?),
        None => None,
    };
    let mut root = read_theme_yaml_mapping(theme_path)?;
    let mapping = root
        .as_mapping_mut()
        .ok_or("主题配置根节点必须是 YAML mapping")?;
    ensure_theme_version(mapping);
    write_theme_ui_mapping(mapping, color_scheme, accent_color)?;
    write_theme_yaml_atomic(theme_path, &root)
}

fn read_theme_yaml_mapping(theme_path: &Path) -> Result<serde_yaml::Value, String> {
    if let Some(parent) = theme_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建主题目录失败: {e}"))?;
    }
    let mut root = if theme_path.is_file() {
        let content = fs::read_to_string(theme_path)
            .map_err(|e| format!("读取主题配置失败 {}: {e}", theme_path.display()))?;
        serde_yaml::from_str::<serde_yaml::Value>(&content)
            .map_err(|e| format!("主题配置无法解析 {}: {e}", theme_path.display()))?
    } else {
        serde_yaml::Value::Mapping(serde_yaml::Mapping::new())
    };
    if !matches!(root, serde_yaml::Value::Mapping(_)) {
        root = serde_yaml::Value::Mapping(serde_yaml::Mapping::new());
    }
    Ok(root)
}

fn ensure_theme_version(mapping: &mut serde_yaml::Mapping) {
    mapping
        .entry(serde_yaml::Value::String("version".into()))
        .or_insert_with(|| {
            serde_yaml::Value::Number(serde_yaml::Number::from(THEME_SCHEMA_VERSION))
        });
}

fn write_theme_ui_mapping(
    mapping: &mut serde_yaml::Mapping,
    color_scheme: UiColorScheme,
    accent_color: Option<String>,
) -> Result<(), String> {
    let ui_mapping = yaml_child_mapping(mapping, "ui", "主题 UI 配置必须是 YAML mapping")?;
    ui_mapping.insert(
        serde_yaml::Value::String("colorScheme".into()),
        serde_yaml::Value::String(
            match color_scheme {
                UiColorScheme::Auto => "auto",
                UiColorScheme::Light => "light",
                UiColorScheme::Dark => "dark",
            }
            .into(),
        ),
    );
    if let Some(accent_color) = accent_color {
        let accent_key = serde_yaml::Value::String("accentColor".into());
        if accent_color.is_empty() {
            ui_mapping.remove(&accent_key);
        } else {
            ui_mapping.insert(accent_key, serde_yaml::Value::String(accent_color));
        }
    }
    Ok(())
}

fn yaml_child_mapping<'a>(
    mapping: &'a mut serde_yaml::Mapping,
    key: &str,
    error: &str,
) -> Result<&'a mut serde_yaml::Mapping, String> {
    let value = mapping
        .entry(serde_yaml::Value::String(key.into()))
        .or_insert_with(|| serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));
    if !matches!(value, serde_yaml::Value::Mapping(_)) {
        *value = serde_yaml::Value::Mapping(serde_yaml::Mapping::new());
    }
    value.as_mapping_mut().ok_or_else(|| error.to_string())
}

fn normalize_hex_color(value: &str) -> Result<String, String> {
    let hex = value.trim().strip_prefix('#').unwrap_or(value.trim());
    if hex.len() != 6 || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err("主题色必须是 #RRGGBB 格式".into());
    }
    Ok(format!("#{}", hex.to_ascii_uppercase()))
}

fn normalize_candidate_font_size(value: f32) -> Result<f32, String> {
    if !value.is_finite() {
        return Err("候选字号必须是有效数字".into());
    }
    Ok(value.clamp(MIN_CANDIDATE_FONT_SIZE, MAX_CANDIDATE_FONT_SIZE))
}

fn write_theme_yaml_atomic(theme_path: &Path, root: &serde_yaml::Value) -> Result<(), String> {
    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);
    let content = serde_yaml::to_string(root).map_err(|e| format!("序列化主题配置失败: {e}"))?;
    let dir = theme_path.parent().unwrap_or_else(|| Path::new("."));
    let name = theme_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("theme.yaml");
    let temporary = dir.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let outcome = fs::File::create(&temporary)
        .and_then(|mut file| {
            file.write_all(content.as_bytes())?;
            file.sync_all()
        })
        .and_then(|()| fs::rename(&temporary, theme_path));
    if outcome.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    outcome.map_err(|e| format!("写入主题配置失败 {}: {e}", theme_path.display()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PanelOrientation {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FontWeight {
    UltraLight,
    Thin,
    Light,
    Regular,
    Medium,
    SemiBold,
    Bold,
    Heavy,
    Black,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UiColorScheme {
    Auto,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EffectiveColorScheme {
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RgbaColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: u8,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedImeTheme {
    pub version: u32,
    pub ui: UiTheme,
    pub font: FontTheme,
    pub panel: PanelTheme,
    pub candidate: CandidateTheme,
    pub navigation: NavigationTheme,
    pub mode_hint: ModeHintTheme,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiTheme {
    pub color_scheme: UiColorScheme,
    pub effective_color_scheme: EffectiveColorScheme,
    pub accent_color: Option<RgbaColor>,
    pub embedded_composition: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FontTheme {
    pub family: Option<String>,
    pub size: f32,
    pub label_size: f32,
    pub comment_size: f32,
    pub preedit_size: f32,
    pub weight: FontWeight,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelTheme {
    pub orientation: PanelOrientation,
    pub background: RgbaColor,
    pub border_color: RgbaColor,
    pub border_width: f32,
    pub corner_radius: f32,
    pub padding_x: f32,
    pub padding_y: f32,
    pub gap: f32,
    pub min_width: f32,
    pub max_width: f32,
    pub max_height: f32,
    pub screen_margin: f32,
    pub shadow: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateTheme {
    pub background: RgbaColor,
    pub hover_background: RgbaColor,
    pub pressed_background: RgbaColor,
    pub pressed_foreground: RgbaColor,
    pub selected_background: RgbaColor,
    pub foreground: RgbaColor,
    pub selected_foreground: RgbaColor,
    pub label_color: RgbaColor,
    pub selected_label_color: RgbaColor,
    pub comment_color: RgbaColor,
    pub selected_comment_color: RgbaColor,
    pub border_color: RgbaColor,
    pub selected_border_color: RgbaColor,
    pub border_width: f32,
    pub corner_radius: f32,
    pub padding_x: f32,
    pub padding_y: f32,
    pub inline_gap: f32,
    pub min_height: f32,
    pub max_width: f32,
    pub separator_visible: bool,
    pub separator_color: RgbaColor,
    pub label_suffix: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NavigationTheme {
    pub foreground: RgbaColor,
    pub disabled_foreground: RgbaColor,
    pub hover_background: RgbaColor,
    pub button_size: f32,
    pub corner_radius: f32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeHintTheme {
    pub background: RgbaColor,
    pub foreground: RgbaColor,
    pub border_color: RgbaColor,
    pub border_width: f32,
    pub font_size: f32,
    pub width: f32,
    pub height: f32,
    pub corner_radius: f32,
    pub duration: f32,
    pub shadow: bool,
    pub chinese_text: String,
    pub english_text: String,
}

#[derive(Clone, Debug, Default)]
pub struct UiCapabilities {
    pub supports_custom_colors: bool,
    pub supports_vertical: bool,
    pub supports_hover: bool,
    pub supports_shadow: bool,
    pub supports_separator: bool,
    pub system_lookup_table_only: bool,
}

impl UiCapabilities {
    pub fn full_custom() -> Self {
        Self {
            supports_custom_colors: true,
            supports_vertical: true,
            supports_hover: true,
            supports_shadow: true,
            supports_separator: true,
            system_lookup_table_only: false,
        }
    }

    pub fn system_lookup_table() -> Self {
        Self {
            system_lookup_table_only: true,
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug)]
pub struct CandidatePanelInput {
    pub preedit: String,
    pub candidates: Vec<ThemeCandidate>,
    pub highlighted_candidate_index: usize,
    pub page: usize,
    pub is_last_page: bool,
    pub select_keys: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ThemeCandidate {
    pub text: String,
    pub comment: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidatePanelModel {
    pub preedit: Option<String>,
    pub orientation: PanelOrientation,
    pub candidates: Vec<CandidateOptionModel>,
    pub navigation: PageNavigationModel,
    pub capabilities: ResolvedCapabilities,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateOptionModel {
    pub index: usize,
    pub label: String,
    pub text: String,
    pub comment: Option<String>,
    pub selected: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageNavigationModel {
    pub can_go_previous: bool,
    pub can_go_next: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedCapabilities {
    pub custom_colors: bool,
    pub vertical: bool,
    pub hover: bool,
    pub shadow: bool,
    pub separator: bool,
    pub system_lookup_table_only: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeHintModel {
    pub ascii_mode: bool,
    pub text: String,
}

#[derive(Default)]
pub struct ThemeResolver {
    default_theme_path: Option<PathBuf>,
    user_theme_path: Option<PathBuf>,
    system_scheme: Option<EffectiveColorScheme>,
    cache: Mutex<ThemeCache>,
}

#[derive(Clone, Debug)]
struct ThemeCache {
    signature: String,
    theme: ResolvedImeTheme,
}

impl Default for ThemeCache {
    fn default() -> Self {
        Self {
            signature: String::new(),
            theme: ResolvedImeTheme::default(),
        }
    }
}

impl ThemeResolver {
    pub fn new(default_theme_path: Option<PathBuf>, user_theme_path: Option<PathBuf>) -> Self {
        Self::with_system_scheme(default_theme_path, user_theme_path, None)
    }

    pub fn with_system_scheme(
        default_theme_path: Option<PathBuf>,
        user_theme_path: Option<PathBuf>,
        system_scheme: Option<EffectiveColorScheme>,
    ) -> Self {
        Self {
            default_theme_path,
            user_theme_path,
            system_scheme,
            cache: Mutex::new(ThemeCache::default()),
        }
    }

    pub fn from_default_locations() -> Self {
        Self::new(default_bundled_theme_path(), default_user_theme_path())
    }

    pub fn current(&self) -> ResolvedImeTheme {
        let signature = self.signature();
        let system_scheme = self
            .system_scheme
            .unwrap_or_else(cached_system_effective_color_scheme);
        let Ok(mut cache) = self.cache.lock() else {
            return resolve_theme_from_paths_with_system_scheme(
                self.default_theme_path.as_deref(),
                self.user_theme_path.as_deref(),
                system_scheme,
            );
        };
        if cache.signature == signature {
            return cache.theme.clone();
        }
        let theme = resolve_theme_from_paths_with_system_scheme(
            self.default_theme_path.as_deref(),
            self.user_theme_path.as_deref(),
            system_scheme,
        );
        cache.signature = signature;
        cache.theme = theme.clone();
        theme
    }

    fn signature(&self) -> String {
        let mut parts = [
            self.default_theme_path.as_deref(),
            self.user_theme_path.as_deref(),
        ]
        .into_iter()
        .flatten()
        .map(path_signature)
        .collect::<Vec<_>>();
        parts.push(format!(
            "system:{:?}",
            self.system_scheme
                .unwrap_or_else(cached_system_effective_color_scheme)
        ));
        parts.join("|")
    }
}

pub fn default_user_theme_path() -> Option<PathBuf> {
    if let Ok(value) = std::env::var("KEYTAO_IME_THEME_PATH") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }

    #[cfg(target_os = "macos")]
    {
        return dirs::home_dir().map(|home| home.join("Library/keytao/theme.yaml"));
    }
    #[cfg(target_os = "windows")]
    {
        return dirs::config_dir().map(|dir| dir.join("keytao/theme.yaml"));
    }
    #[cfg(target_os = "linux")]
    {
        return dirs::data_local_dir().map(|dir| dir.join("keytao/theme.yaml"));
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        None
    }
}

pub fn default_bundled_theme_path() -> Option<PathBuf> {
    if let Ok(value) = std::env::var("KEYTAO_IME_DEFAULT_THEME_PATH") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))?;
    let candidates = [
        exe_dir.join("default-theme.yaml"),
        exe_dir.join("theme.yaml"),
        exe_dir.join("resources").join("default-theme.yaml"),
        exe_dir.join("resources").join("theme.yaml"),
        exe_dir.join("runtime").join("default-theme.yaml"),
        exe_dir
            .join("resources")
            .join("runtime")
            .join("default-theme.yaml"),
        exe_dir
            .join("..")
            .join("runtime")
            .join("default-theme.yaml"),
        exe_dir
            .join("..")
            .join("lib")
            .join("keytao-app")
            .join("runtime")
            .join("default-theme.yaml"),
        exe_dir
            .join("..")
            .join("lib")
            .join("keytao-app")
            .join("resources")
            .join("runtime")
            .join("default-theme.yaml"),
    ];
    candidates.into_iter().find(|path| path.is_file())
}

pub fn resolve_theme_from_paths(
    default_theme_path: Option<&Path>,
    user_theme_path: Option<&Path>,
) -> ResolvedImeTheme {
    resolve_theme_from_paths_with_system(
        default_theme_path,
        user_theme_path,
        cached_system_effective_color_scheme(),
    )
}

pub fn resolve_theme_from_paths_with_system_scheme(
    default_theme_path: Option<&Path>,
    user_theme_path: Option<&Path>,
    system_scheme: EffectiveColorScheme,
) -> ResolvedImeTheme {
    let mut partials = Vec::new();
    if let Ok(partial) = serde_yaml::from_str::<PartialTheme>(DEFAULT_THEME_YAML) {
        partials.push(partial);
    }
    for path in [default_theme_path, user_theme_path].into_iter().flatten() {
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        let Ok(partial) = serde_yaml::from_str::<PartialTheme>(&content) else {
            continue;
        };
        partials.push(partial);
    }

    let mut ui = UiTheme::default();
    for partial in &partials {
        if let Some(partial_ui) = partial.ui.clone() {
            ui.apply(partial_ui);
        }
    }
    ui.effective_color_scheme = match ui.color_scheme {
        UiColorScheme::Auto => system_scheme,
        UiColorScheme::Light => EffectiveColorScheme::Light,
        UiColorScheme::Dark => EffectiveColorScheme::Dark,
    };

    let mut theme = ResolvedImeTheme::schema_base(ui.clone());
    for partial in partials {
        theme.apply(partial.clone());
        if ui.effective_color_scheme == EffectiveColorScheme::Light {
            if let Some(light) = partial.light {
                theme.apply_variant(light);
            }
        } else if let Some(dark) = partial.dark {
            theme.apply_variant(dark);
        }
    }
    theme.ui = ui.clone();
    if let Some(accent_color) = ui.accent_color {
        theme.apply_accent_color(accent_color);
    }

    theme.sanitized()
}

fn resolve_theme_from_paths_with_system(
    default_theme_path: Option<&Path>,
    user_theme_path: Option<&Path>,
    system_scheme: EffectiveColorScheme,
) -> ResolvedImeTheme {
    resolve_theme_from_paths_with_system_scheme(default_theme_path, user_theme_path, system_scheme)
}

fn builtin_default_theme(system_scheme: EffectiveColorScheme) -> ResolvedImeTheme {
    let mut partials = Vec::new();
    if let Ok(partial) = serde_yaml::from_str::<PartialTheme>(DEFAULT_THEME_YAML) {
        partials.push(partial);
    }

    let mut ui = UiTheme::default();
    for partial in &partials {
        if let Some(partial_ui) = partial.ui.clone() {
            ui.apply(partial_ui);
        }
    }
    ui.effective_color_scheme = match ui.color_scheme {
        UiColorScheme::Auto => system_scheme,
        UiColorScheme::Light => EffectiveColorScheme::Light,
        UiColorScheme::Dark => EffectiveColorScheme::Dark,
    };

    let mut theme = ResolvedImeTheme::schema_base(ui.clone());
    for partial in partials {
        theme.apply(partial.clone());
        if ui.effective_color_scheme == EffectiveColorScheme::Light {
            if let Some(light) = partial.light {
                theme.apply_variant(light);
            }
        } else if let Some(dark) = partial.dark {
            theme.apply_variant(dark);
        }
    }
    theme.ui = ui.clone();
    if let Some(accent_color) = ui.accent_color {
        theme.apply_accent_color(accent_color);
    }
    theme.sanitized()
}

pub fn resolved_theme_json(theme: &ResolvedImeTheme) -> Result<String, serde_json::Error> {
    serde_json::to_string(theme)
}

impl ResolvedImeTheme {
    fn schema_base(ui: UiTheme) -> Self {
        Self {
            version: THEME_SCHEMA_VERSION,
            ui,
            font: FontTheme {
                family: None,
                size: 20.0,
                label_size: 15.0,
                comment_size: 16.0,
                preedit_size: 15.0,
                weight: FontWeight::SemiBold,
            },
            panel: PanelTheme {
                orientation: PanelOrientation::Vertical,
                background: rgba(0xF8, 0xFA, 0xFF, 0xF2),
                border_color: rgba(0xB8, 0xC3, 0xD0, 0xFF),
                border_width: 1.0,
                corner_radius: 16.0,
                padding_x: 10.0,
                padding_y: 10.0,
                gap: 4.0,
                min_width: 128.0,
                max_width: 320.0,
                max_height: 460.0,
                screen_margin: 8.0,
                shadow: true,
            },
            candidate: CandidateTheme {
                background: rgba(0, 0, 0, 0),
                hover_background: rgba(0xF1, 0xF6, 0xFF, 0xFF),
                pressed_background: rgba(0xD4, 0xE7, 0xFF, 0xFF),
                pressed_foreground: rgba(0x14, 0x23, 0x3B, 0xFF),
                selected_background: rgba(0xE6, 0xF0, 0xFF, 0xFF),
                foreground: rgba(0x26, 0x34, 0x42, 0xFF),
                selected_foreground: rgba(0x24, 0x32, 0x41, 0xFF),
                label_color: rgba(0x7F, 0x8D, 0x9C, 0xFF),
                selected_label_color: rgba(0x4A, 0x8D, 0xF6, 0xFF),
                comment_color: rgba(0x84, 0x92, 0x9E, 0xFF),
                selected_comment_color: rgba(0x61, 0x72, 0x86, 0xFF),
                border_color: rgba(0, 0, 0, 0),
                selected_border_color: rgba(0x5D, 0xA7, 0xD7, 0xFF),
                border_width: 1.0,
                corner_radius: 11.0,
                padding_x: 10.0,
                padding_y: 5.0,
                inline_gap: 4.0,
                min_height: 34.0,
                max_width: 190.0,
                separator_visible: false,
                separator_color: rgba(0xDC, 0xE7, 0xF7, 0xFF),
                label_suffix: ".".to_string(),
            },
            navigation: NavigationTheme {
                foreground: rgba(0x68, 0x76, 0x84, 0xFF),
                disabled_foreground: rgba(0xA5, 0xB0, 0xB8, 0xFF),
                hover_background: rgba(0xF1, 0xF6, 0xFF, 0xFF),
                button_size: 28.0,
                corner_radius: 10.0,
            },
            mode_hint: ModeHintTheme {
                background: rgba(0x2D, 0x4B, 0x63, 0xFF),
                foreground: rgba(0xFF, 0xFF, 0xFF, 0xFF),
                border_color: rgba(0x5D, 0xA7, 0xD7, 0xFF),
                border_width: 1.0,
                font_size: 24.0,
                width: 72.0,
                height: 44.0,
                corner_radius: 14.0,
                duration: 0.75,
                shadow: true,
                chinese_text: "中".to_string(),
                english_text: "英".to_string(),
            },
        }
    }

    pub fn candidate_panel_model(
        &self,
        input: CandidatePanelInput,
        capabilities: &UiCapabilities,
    ) -> CandidatePanelModel {
        let orientation = if self.panel.orientation == PanelOrientation::Vertical
            && capabilities.supports_vertical
        {
            PanelOrientation::Vertical
        } else {
            PanelOrientation::Horizontal
        };
        let selected = input
            .highlighted_candidate_index
            .min(input.candidates.len().saturating_sub(1));
        let select_keys = input
            .select_keys
            .unwrap_or_else(|| "1234567890".to_string());
        let candidates = input
            .candidates
            .into_iter()
            .enumerate()
            .map(|(index, candidate)| {
                let key = select_keys
                    .chars()
                    .nth(index)
                    .map(|ch| ch.to_string())
                    .unwrap_or_else(|| (index + 1).to_string());
                CandidateOptionModel {
                    index,
                    label: format!("{key}{}", self.candidate.label_suffix),
                    text: candidate.text,
                    comment: candidate.comment.filter(|comment| !comment.is_empty()),
                    selected: index == selected,
                }
            })
            .collect();

        CandidatePanelModel {
            preedit: (!input.preedit.is_empty()).then_some(input.preedit),
            orientation,
            candidates,
            navigation: PageNavigationModel {
                can_go_previous: input.page > 0,
                can_go_next: !input.is_last_page,
            },
            capabilities: ResolvedCapabilities {
                custom_colors: capabilities.supports_custom_colors,
                vertical: capabilities.supports_vertical,
                hover: capabilities.supports_hover,
                shadow: capabilities.supports_shadow,
                separator: capabilities.supports_separator,
                system_lookup_table_only: capabilities.system_lookup_table_only,
            },
        }
    }

    pub fn mode_hint_model(&self, ascii_mode: bool) -> ModeHintModel {
        ModeHintModel {
            ascii_mode,
            text: if ascii_mode {
                self.mode_hint.english_text.clone()
            } else {
                self.mode_hint.chinese_text.clone()
            },
        }
    }

    fn apply(&mut self, partial: PartialTheme) {
        if let Some(version) = partial.version {
            self.version = version;
        }
        if let Some(ui) = partial.ui {
            self.ui.apply(ui);
        }
        self.apply_variant(PartialThemeVariant {
            font: partial.font,
            panel: partial.panel,
            candidate: partial.candidate,
            navigation: partial.navigation,
            mode_hint: partial.mode_hint,
        });
    }

    fn apply_variant(&mut self, partial: PartialThemeVariant) {
        if let Some(font) = partial.font {
            self.font.apply(font);
        }
        if let Some(panel) = partial.panel {
            self.panel.apply(panel);
        }
        if let Some(candidate) = partial.candidate {
            self.candidate.apply(candidate);
        }
        if let Some(navigation) = partial.navigation {
            self.navigation.apply(navigation);
        }
        if let Some(mode_hint) = partial.mode_hint {
            self.mode_hint.apply(mode_hint);
        }
    }

    fn apply_accent_color(&mut self, accent: RgbaColor) {
        let panel_background = self.panel.background;
        let is_dark = self.ui.effective_color_scheme == EffectiveColorScheme::Dark;
        let selected_weight = if is_dark { 0.42 } else { 0.18 };
        let pressed_weight = if is_dark { 0.54 } else { 0.28 };
        let hover_weight = if is_dark { 0.22 } else { 0.09 };

        self.candidate.selected_label_color = opaque(accent);
        self.candidate.selected_border_color = opaque(accent);
        self.candidate.selected_background =
            with_alpha(mix_color(panel_background, accent, selected_weight), 0xff);
        self.candidate.pressed_background =
            with_alpha(mix_color(panel_background, accent, pressed_weight), 0xff);
        self.candidate.hover_background =
            with_alpha(mix_color(panel_background, accent, hover_weight), 0xff);
        self.mode_hint.border_color = opaque(accent);
        self.mode_hint.foreground = rgba(0xff, 0xff, 0xff, 0xff);
    }

    fn sanitized(mut self) -> Self {
        self.version = THEME_SCHEMA_VERSION;
        self.font.size = clamp(
            self.font.size,
            MIN_CANDIDATE_FONT_SIZE,
            MAX_CANDIDATE_FONT_SIZE,
        );
        self.font.label_size = clamp(self.font.label_size, 9.0, 28.0);
        self.font.comment_size = clamp(self.font.comment_size, 9.0, 28.0);
        self.font.preedit_size = clamp(self.font.preedit_size, 9.0, 28.0);
        self.panel.border_width = clamp(self.panel.border_width, 0.0, 4.0);
        self.panel.corner_radius = clamp(self.panel.corner_radius, 0.0, 28.0);
        self.panel.padding_x = clamp(self.panel.padding_x, 0.0, 32.0);
        self.panel.padding_y = clamp(self.panel.padding_y, 0.0, 28.0);
        self.panel.gap = clamp(self.panel.gap, 0.0, 24.0);
        self.panel.min_width = clamp(self.panel.min_width, 48.0, 480.0);
        self.panel.max_width = clamp(self.panel.max_width, 160.0, 2400.0);
        self.panel.max_height = clamp(self.panel.max_height, 80.0, 1600.0);
        self.panel.screen_margin = clamp(self.panel.screen_margin, 0.0, 40.0);
        self.candidate.border_width = clamp(self.candidate.border_width, 0.0, 3.0);
        self.candidate.corner_radius = clamp(self.candidate.corner_radius, 0.0, 24.0);
        self.candidate.padding_x = clamp(self.candidate.padding_x, 0.0, 28.0);
        self.candidate.padding_y = clamp(self.candidate.padding_y, 0.0, 24.0);
        self.candidate.inline_gap = clamp(self.candidate.inline_gap, 0.0, 18.0);
        self.candidate.min_height = clamp(self.candidate.min_height, 20.0, 72.0);
        self.candidate.max_width = clamp(self.candidate.max_width, 72.0, 640.0);
        self.navigation.button_size = clamp(self.navigation.button_size, 18.0, 56.0);
        self.navigation.corner_radius = clamp(self.navigation.corner_radius, 0.0, 20.0);
        self.mode_hint.border_width = clamp(self.mode_hint.border_width, 0.0, 4.0);
        self.mode_hint.font_size = clamp(self.mode_hint.font_size, 12.0, 42.0);
        self.mode_hint.width = clamp(self.mode_hint.width, 36.0, 180.0);
        self.mode_hint.height = clamp(self.mode_hint.height, 28.0, 140.0);
        self.mode_hint.corner_radius = clamp(self.mode_hint.corner_radius, 0.0, 32.0);
        self.mode_hint.duration = clamp(self.mode_hint.duration, 0.15, 4.0);
        self
    }
}

impl Default for ResolvedImeTheme {
    fn default() -> Self {
        builtin_default_theme(EffectiveColorScheme::Light)
    }
}

impl Default for UiTheme {
    fn default() -> Self {
        Self {
            color_scheme: UiColorScheme::Auto,
            effective_color_scheme: EffectiveColorScheme::Light,
            accent_color: None,
            embedded_composition: false,
        }
    }
}

#[derive(Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartialTheme {
    version: Option<u32>,
    ui: Option<PartialUiTheme>,
    font: Option<PartialFontTheme>,
    panel: Option<PartialPanelTheme>,
    candidate: Option<PartialCandidateTheme>,
    navigation: Option<PartialNavigationTheme>,
    #[serde(alias = "mode_hint")]
    mode_hint: Option<PartialModeHintTheme>,
    /// Legacy `theme.yaml` documents may still carry the mobile keyboard
    /// layout here. It never reaches [`ResolvedImeTheme`]; it is only read back
    /// by [`mobile_layout::resolve_mobile_layout_from_paths`].
    keyboard: Option<PartialMobileLayout>,
    light: Option<PartialThemeVariant>,
    #[serde(alias = "night")]
    dark: Option<PartialThemeVariant>,
}

#[derive(Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartialThemeVariant {
    font: Option<PartialFontTheme>,
    panel: Option<PartialPanelTheme>,
    candidate: Option<PartialCandidateTheme>,
    navigation: Option<PartialNavigationTheme>,
    #[serde(alias = "mode_hint")]
    mode_hint: Option<PartialModeHintTheme>,
}

#[derive(Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartialUiTheme {
    #[serde(alias = "color_scheme")]
    color_scheme: Option<UiColorScheme>,
    #[serde(alias = "embedded_composition")]
    embedded_composition: Option<bool>,
    #[serde(alias = "night_mode")]
    night_mode: Option<bool>,
    #[serde(default, deserialize_with = "optional_color")]
    accent_color: Option<RgbaColor>,
}

#[derive(Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartialFontTheme {
    family: Option<String>,
    size: Option<f32>,
    label_size: Option<f32>,
    comment_size: Option<f32>,
    preedit_size: Option<f32>,
    weight: Option<FontWeight>,
}

#[derive(Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartialPanelTheme {
    orientation: Option<PanelOrientation>,
    #[serde(default, deserialize_with = "optional_color")]
    background: Option<RgbaColor>,
    #[serde(default, deserialize_with = "optional_color")]
    border_color: Option<RgbaColor>,
    border_width: Option<f32>,
    corner_radius: Option<f32>,
    padding_x: Option<f32>,
    padding_y: Option<f32>,
    gap: Option<f32>,
    min_width: Option<f32>,
    max_width: Option<f32>,
    max_height: Option<f32>,
    screen_margin: Option<f32>,
    shadow: Option<bool>,
}

#[derive(Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartialCandidateTheme {
    #[serde(default, deserialize_with = "optional_color")]
    background: Option<RgbaColor>,
    #[serde(default, deserialize_with = "optional_color")]
    hover_background: Option<RgbaColor>,
    #[serde(default, deserialize_with = "optional_color")]
    pressed_background: Option<RgbaColor>,
    #[serde(default, deserialize_with = "optional_color")]
    pressed_foreground: Option<RgbaColor>,
    #[serde(default, deserialize_with = "optional_color")]
    selected_background: Option<RgbaColor>,
    #[serde(default, deserialize_with = "optional_color")]
    foreground: Option<RgbaColor>,
    #[serde(default, deserialize_with = "optional_color")]
    selected_foreground: Option<RgbaColor>,
    #[serde(default, deserialize_with = "optional_color")]
    label_color: Option<RgbaColor>,
    #[serde(default, deserialize_with = "optional_color")]
    selected_label_color: Option<RgbaColor>,
    #[serde(default, deserialize_with = "optional_color")]
    comment_color: Option<RgbaColor>,
    #[serde(default, deserialize_with = "optional_color")]
    selected_comment_color: Option<RgbaColor>,
    #[serde(default, deserialize_with = "optional_color")]
    border_color: Option<RgbaColor>,
    #[serde(default, deserialize_with = "optional_color")]
    selected_border_color: Option<RgbaColor>,
    border_width: Option<f32>,
    corner_radius: Option<f32>,
    padding_x: Option<f32>,
    padding_y: Option<f32>,
    inline_gap: Option<f32>,
    min_height: Option<f32>,
    max_width: Option<f32>,
    separator_visible: Option<bool>,
    #[serde(default, deserialize_with = "optional_color")]
    separator_color: Option<RgbaColor>,
    label_suffix: Option<String>,
}

#[derive(Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartialNavigationTheme {
    #[serde(default, deserialize_with = "optional_color")]
    foreground: Option<RgbaColor>,
    #[serde(default, deserialize_with = "optional_color")]
    disabled_foreground: Option<RgbaColor>,
    #[serde(default, deserialize_with = "optional_color")]
    hover_background: Option<RgbaColor>,
    button_size: Option<f32>,
    corner_radius: Option<f32>,
}

#[derive(Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartialModeHintTheme {
    #[serde(default, deserialize_with = "optional_color")]
    background: Option<RgbaColor>,
    #[serde(default, deserialize_with = "optional_color")]
    foreground: Option<RgbaColor>,
    #[serde(default, deserialize_with = "optional_color")]
    border_color: Option<RgbaColor>,
    border_width: Option<f32>,
    font_size: Option<f32>,
    width: Option<f32>,
    height: Option<f32>,
    corner_radius: Option<f32>,
    duration: Option<f32>,
    shadow: Option<bool>,
    chinese_text: Option<String>,
    english_text: Option<String>,
}

impl UiTheme {
    fn apply(&mut self, partial: PartialUiTheme) {
        if let Some(color_scheme) = partial.color_scheme {
            self.color_scheme = color_scheme;
        } else if let Some(night_mode) = partial.night_mode {
            self.color_scheme = if night_mode {
                UiColorScheme::Dark
            } else {
                UiColorScheme::Light
            };
        }
        if let Some(accent_color) = partial.accent_color {
            self.accent_color = Some(accent_color);
        }
        if let Some(embedded_composition) = partial.embedded_composition {
            self.embedded_composition = embedded_composition;
        }
    }
}

impl FontTheme {
    fn apply(&mut self, partial: PartialFontTheme) {
        if let Some(family) = partial.family {
            self.family = (!family.trim().is_empty()).then_some(family);
        }
        assign(&mut self.size, partial.size);
        assign(&mut self.label_size, partial.label_size);
        assign(&mut self.comment_size, partial.comment_size);
        assign(&mut self.preedit_size, partial.preedit_size);
        assign(&mut self.weight, partial.weight);
    }
}

impl PanelTheme {
    fn apply(&mut self, partial: PartialPanelTheme) {
        assign(&mut self.orientation, partial.orientation);
        assign(&mut self.background, partial.background);
        assign(&mut self.border_color, partial.border_color);
        assign(&mut self.border_width, partial.border_width);
        assign(&mut self.corner_radius, partial.corner_radius);
        assign(&mut self.padding_x, partial.padding_x);
        assign(&mut self.padding_y, partial.padding_y);
        assign(&mut self.gap, partial.gap);
        assign(&mut self.min_width, partial.min_width);
        assign(&mut self.max_width, partial.max_width);
        assign(&mut self.max_height, partial.max_height);
        assign(&mut self.screen_margin, partial.screen_margin);
        assign(&mut self.shadow, partial.shadow);
    }
}

impl CandidateTheme {
    fn apply(&mut self, partial: PartialCandidateTheme) {
        assign(&mut self.background, partial.background);
        assign(&mut self.hover_background, partial.hover_background);
        assign(&mut self.pressed_background, partial.pressed_background);
        assign(&mut self.pressed_foreground, partial.pressed_foreground);
        assign(&mut self.selected_background, partial.selected_background);
        assign(&mut self.foreground, partial.foreground);
        assign(&mut self.selected_foreground, partial.selected_foreground);
        assign(&mut self.label_color, partial.label_color);
        assign(&mut self.selected_label_color, partial.selected_label_color);
        assign(&mut self.comment_color, partial.comment_color);
        assign(
            &mut self.selected_comment_color,
            partial.selected_comment_color,
        );
        assign(&mut self.border_color, partial.border_color);
        assign(
            &mut self.selected_border_color,
            partial.selected_border_color,
        );
        assign(&mut self.border_width, partial.border_width);
        assign(&mut self.corner_radius, partial.corner_radius);
        assign(&mut self.padding_x, partial.padding_x);
        assign(&mut self.padding_y, partial.padding_y);
        assign(&mut self.inline_gap, partial.inline_gap);
        assign(&mut self.min_height, partial.min_height);
        assign(&mut self.max_width, partial.max_width);
        assign(&mut self.separator_visible, partial.separator_visible);
        assign(&mut self.separator_color, partial.separator_color);
        if let Some(label_suffix) = partial.label_suffix {
            self.label_suffix = label_suffix;
        }
    }
}

impl NavigationTheme {
    fn apply(&mut self, partial: PartialNavigationTheme) {
        assign(&mut self.foreground, partial.foreground);
        assign(&mut self.disabled_foreground, partial.disabled_foreground);
        assign(&mut self.hover_background, partial.hover_background);
        assign(&mut self.button_size, partial.button_size);
        assign(&mut self.corner_radius, partial.corner_radius);
    }
}

impl ModeHintTheme {
    fn apply(&mut self, partial: PartialModeHintTheme) {
        assign(&mut self.background, partial.background);
        assign(&mut self.foreground, partial.foreground);
        assign(&mut self.border_color, partial.border_color);
        assign(&mut self.border_width, partial.border_width);
        assign(&mut self.font_size, partial.font_size);
        assign(&mut self.width, partial.width);
        assign(&mut self.height, partial.height);
        assign(&mut self.corner_radius, partial.corner_radius);
        assign(&mut self.duration, partial.duration);
        assign(&mut self.shadow, partial.shadow);
        if let Some(chinese_text) = partial.chinese_text {
            self.chinese_text = chinese_text;
        }
        if let Some(english_text) = partial.english_text {
            self.english_text = english_text;
        }
    }
}

impl<'de> Deserialize<'de> for RgbaColor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        parse_color(&value).ok_or_else(|| serde::de::Error::custom("invalid color"))
    }
}

fn optional_color<'de, D>(deserializer: D) -> Result<Option<RgbaColor>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    value
        .map(|value| parse_color(&value).ok_or_else(|| serde::de::Error::custom("invalid color")))
        .transpose()
}

fn parse_color(value: &str) -> Option<RgbaColor> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("transparent") || value.eq_ignore_ascii_case("clear") {
        return Some(rgba(0, 0, 0, 0));
    }
    if value.eq_ignore_ascii_case("black") {
        return Some(rgba(0, 0, 0, 255));
    }
    if value.eq_ignore_ascii_case("white") {
        return Some(rgba(255, 255, 255, 255));
    }
    let hex = value.strip_prefix('#')?;
    let raw = u32::from_str_radix(hex, 16).ok()?;
    match hex.len() {
        6 => Some(rgba(
            ((raw >> 16) & 0xff) as u8,
            ((raw >> 8) & 0xff) as u8,
            (raw & 0xff) as u8,
            255,
        )),
        8 => Some(rgba(
            ((raw >> 24) & 0xff) as u8,
            ((raw >> 16) & 0xff) as u8,
            ((raw >> 8) & 0xff) as u8,
            (raw & 0xff) as u8,
        )),
        _ => None,
    }
}

const fn rgba(red: u8, green: u8, blue: u8, alpha: u8) -> RgbaColor {
    RgbaColor {
        red,
        green,
        blue,
        alpha,
    }
}

fn assign<T>(slot: &mut T, value: Option<T>) {
    if let Some(value) = value {
        *slot = value;
    }
}

fn clamp(value: f32, min: f32, max: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        min
    }
}

fn path_signature(path: &Path) -> String {
    let Ok(meta) = fs::metadata(path) else {
        return format!("{}:missing", path.display());
    };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!("{}:{mtime}:{}", path.display(), meta.len())
}

#[derive(Clone, Copy, Debug)]
struct SystemSchemeCache {
    checked_at: Instant,
    scheme: EffectiveColorScheme,
}

fn cached_system_effective_color_scheme() -> EffectiveColorScheme {
    static CACHE: OnceLock<Mutex<Option<SystemSchemeCache>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    let Ok(mut cache) = cache.lock() else {
        return detect_system_effective_color_scheme();
    };
    let now = Instant::now();
    if let Some(entry) = *cache {
        if now.duration_since(entry.checked_at) < Duration::from_secs(1) {
            return entry.scheme;
        }
    }
    let scheme = detect_system_effective_color_scheme();
    *cache = Some(SystemSchemeCache {
        checked_at: now,
        scheme,
    });
    scheme
}

fn detect_system_effective_color_scheme() -> EffectiveColorScheme {
    if let Ok(value) = std::env::var("KEYTAO_IME_SYSTEM_COLOR_SCHEME") {
        if let Some(scheme) = parse_effective_color_scheme(&value) {
            return scheme;
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(scheme) = command_output_scheme(
            "defaults",
            &["read", "-g", "AppleInterfaceStyle"],
            |output| {
                if output.to_ascii_lowercase().contains("dark") {
                    Some(EffectiveColorScheme::Dark)
                } else {
                    None
                }
            },
        ) {
            return scheme;
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(scheme) = windows_effective_color_scheme() {
            return scheme;
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(value) = std::env::var("GTK_THEME") {
            if value.to_ascii_lowercase().contains("dark") {
                return EffectiveColorScheme::Dark;
            }
        }
        if let Some(scheme) = command_output_scheme(
            "gsettings",
            &["get", "org.gnome.desktop.interface", "color-scheme"],
            |output| {
                let lower = output.to_ascii_lowercase();
                if lower.contains("prefer-dark") {
                    Some(EffectiveColorScheme::Dark)
                } else if lower.contains("prefer-light") || lower.contains("default") {
                    Some(EffectiveColorScheme::Light)
                } else {
                    None
                }
            },
        ) {
            return scheme;
        }
    }

    EffectiveColorScheme::Light
}

fn parse_effective_color_scheme(value: &str) -> Option<EffectiveColorScheme> {
    match value.trim().to_ascii_lowercase().as_str() {
        "dark" | "night" => Some(EffectiveColorScheme::Dark),
        "light" | "day" => Some(EffectiveColorScheme::Light),
        _ => None,
    }
}

#[cfg(target_os = "windows")]
fn windows_effective_color_scheme() -> Option<EffectiveColorScheme> {
    use windows::{
        core::w,
        Win32::{
            Foundation::ERROR_SUCCESS,
            System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD},
        },
    };

    let mut value = 1u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS || size != std::mem::size_of::<u32>() as u32 {
        return None;
    }
    Some(if value == 0 {
        EffectiveColorScheme::Dark
    } else {
        EffectiveColorScheme::Light
    })
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn command_output_scheme(
    command: &str,
    args: &[&str],
    parse: impl FnOnce(&str) -> Option<EffectiveColorScheme>,
) -> Option<EffectiveColorScheme> {
    let output = std::process::Command::new(command)
        .args(args)
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    parse(&format!("{stdout}\n{stderr}"))
}

fn mix_color(base: RgbaColor, accent: RgbaColor, accent_weight: f32) -> RgbaColor {
    let weight = accent_weight.clamp(0.0, 1.0);
    rgba(
        mix_channel(base.red, accent.red, weight),
        mix_channel(base.green, accent.green, weight),
        mix_channel(base.blue, accent.blue, weight),
        0xff,
    )
}

fn mix_channel(base: u8, accent: u8, accent_weight: f32) -> u8 {
    (base as f32 * (1.0 - accent_weight) + accent as f32 * accent_weight)
        .round()
        .clamp(0.0, 255.0) as u8
}

fn opaque(color: RgbaColor) -> RgbaColor {
    with_alpha(color, 0xff)
}

fn with_alpha(color: RgbaColor, alpha: u8) -> RgbaColor {
    rgba(color.red, color.green, color.blue, alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mobile_theme_writer_updates_ui_and_preserves_other_settings() {
        let path = std::env::temp_dir().join(format!(
            "keytao-theme-mobile-writer-{}-{}.yaml",
            std::process::id(),
            line!()
        ));
        fs::write(
            &path,
            "panel:\n  orientation: vertical\nfont:\n  size: 27\ncandidate:\n  labelSuffix: ')'\n",
        )
        .unwrap();

        write_theme_ui_to_path(&path, UiColorScheme::Dark, Some("#123456".into())).unwrap();

        let theme = resolve_theme_from_paths(None, Some(&path));
        fs::remove_file(path).ok();
        assert_eq!(theme.ui.color_scheme, UiColorScheme::Dark);
        assert_eq!(theme.ui.accent_color, Some(rgba(0x12, 0x34, 0x56, 0xff)));
        assert_eq!(theme.panel.orientation, PanelOrientation::Vertical);
        assert_eq!(theme.font.size, 27.0);
        assert_eq!(theme.candidate.label_suffix, ")");
    }

    #[test]
    fn mobile_theme_writer_preserves_accent_when_it_is_omitted() {
        let path = std::env::temp_dir().join(format!(
            "keytao-theme-mobile-preserve-accent-{}-{}.yaml",
            std::process::id(),
            line!()
        ));
        fs::write(
            &path,
            "ui:\n  colorScheme: light\n  accentColor: '#123456'\n",
        )
        .unwrap();

        write_theme_ui_to_path(&path, UiColorScheme::Dark, None).unwrap();

        let written: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        fs::remove_file(path).ok();
        assert_eq!(written["ui"]["colorScheme"].as_str(), Some("dark"));
        assert_eq!(written["ui"]["accentColor"].as_str(), Some("#123456"));
    }

    #[test]
    fn mobile_theme_writer_removes_accent_when_it_is_explicitly_cleared() {
        let path = std::env::temp_dir().join(format!(
            "keytao-theme-mobile-clear-accent-{}-{}.yaml",
            std::process::id(),
            line!()
        ));
        fs::write(
            &path,
            "ui:\n  colorScheme: dark\n  accentColor: '#123456'\n",
        )
        .unwrap();

        write_theme_ui_to_path(&path, UiColorScheme::Auto, Some(String::new())).unwrap();

        let written: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        fs::remove_file(path).ok();
        assert_eq!(written["ui"]["colorScheme"].as_str(), Some("auto"));
        assert!(written["ui"].get("accentColor").is_none());
    }

    #[test]
    fn full_theme_writer_clamps_font_and_rejects_non_finite_values() {
        let path = std::env::temp_dir().join(format!(
            "keytao-theme-full-writer-{}-{}.yaml",
            std::process::id(),
            line!()
        ));

        write_ime_ui_settings_to_path(
            &path,
            UiColorScheme::Auto,
            PanelOrientation::Horizontal,
            "#3B73D9".into(),
            99.0,
        )
        .unwrap();
        let theme = resolve_theme_from_paths(None, Some(&path));
        assert_eq!(theme.font.size, MAX_CANDIDATE_FONT_SIZE);
        assert!(write_ime_ui_settings_to_path(
            &path,
            UiColorScheme::Auto,
            PanelOrientation::Horizontal,
            "#3B73D9".into(),
            f32::NAN,
        )
        .is_err());
        fs::remove_file(path).ok();
    }

    #[test]
    fn default_theme_yaml_resolves() {
        let theme = resolve_theme_from_paths(None, None);
        assert_eq!(theme.version, THEME_SCHEMA_VERSION);
        assert_eq!(theme.ui.color_scheme, UiColorScheme::Auto);
        assert!(!theme.ui.embedded_composition);
        assert_eq!(theme.panel.orientation, PanelOrientation::Vertical);
        assert_eq!(theme.panel.min_width, 128.0);
        assert_eq!(theme.font.size, 20.0);
        assert_eq!(theme.candidate.label_suffix, ".");
        assert_eq!(
            theme.candidate.selected_border_color,
            rgba(0x5d, 0xa7, 0xd7, 0xff)
        );
        assert_eq!(theme.mode_hint.width, 72.0);
        assert_eq!(theme.mode_hint.height, 44.0);
    }

    #[test]
    fn resolved_default_uses_builtin_theme_yaml() {
        let theme = ResolvedImeTheme::default();
        assert_eq!(theme.panel.orientation, PanelOrientation::Vertical);
        assert_eq!(theme.candidate.min_height, 34.0);
        assert_eq!(theme.mode_hint.background, rgba(0x2d, 0x4b, 0x63, 0xff));
        assert_eq!(theme.mode_hint.foreground, rgba(0xff, 0xff, 0xff, 0xff));
    }

    #[test]
    fn resolved_theme_json_carries_no_mobile_layout() {
        let theme = resolve_theme_from_paths(None, None);
        let json = resolved_theme_json(&theme).unwrap();
        assert!(!json.contains("\"keyboard\""));
        assert!(!json.contains("\"numberRows\""));
        assert!(!json.contains("\"symbolRows\""));
        assert!(json.contains("\"panel\""));
        assert!(json.contains("\"modeHint\""));
    }

    #[test]
    fn legacy_theme_yaml_with_keyboard_section_still_resolves() {
        let path = std::env::temp_dir().join(format!(
            "keytao-theme-legacy-keyboard-{}-{}.yaml",
            std::process::id(),
            line!()
        ));
        fs::write(
            &path,
            "panel:\n  minWidth: 200\nkeyboard:\n  height: 300\n  rows:\n    - [ { label: \"z\", value: \"z\" } ]\ndark:\n  keyboard:\n    height: 320\n",
        )
        .unwrap();

        let theme = resolve_theme_from_paths(None, Some(&path));
        let json = resolved_theme_json(&theme).unwrap();
        fs::remove_file(path).ok();

        assert_eq!(theme.panel.min_width, 200.0);
        assert!(!json.contains("\"keyboard\""));
    }

    #[test]
    fn user_overlay_merges_and_clamps() {
        let mut theme = ResolvedImeTheme::default();
        let partial = serde_yaml::from_str::<PartialTheme>(
            "font:\n  size: 99\npanel:\n  orientation: vertical\ncandidate:\n  pressedBackground: '#10203040'\n  pressedForeground: '#50607080'\n  selectedBackground: '#11223344'\n",
        )
        .unwrap();
        theme.apply(partial);
        let theme = theme.sanitized();
        assert_eq!(theme.font.size, 36.0);
        assert_eq!(theme.panel.orientation, PanelOrientation::Vertical);
        assert_eq!(
            theme.candidate.pressed_background,
            rgba(0x10, 0x20, 0x30, 0x40)
        );
        assert_eq!(
            theme.candidate.pressed_foreground,
            rgba(0x50, 0x60, 0x70, 0x80)
        );
        assert_eq!(
            theme.candidate.selected_background,
            rgba(0x11, 0x22, 0x33, 0x44)
        );
    }

    #[test]
    fn dark_ui_scheme_applies_dark_variant() {
        let path = std::env::temp_dir().join(format!(
            "keytao-theme-dark-{}-{}.yaml",
            std::process::id(),
            line!()
        ));
        fs::write(
            &path,
            "ui:\n  colorScheme: dark\ndark:\n  candidate:\n    foreground: '#010203'\n",
        )
        .unwrap();

        let theme = resolve_theme_from_paths(None, Some(&path));
        fs::remove_file(path).ok();

        assert_eq!(theme.ui.color_scheme, UiColorScheme::Dark);
        assert_eq!(theme.ui.effective_color_scheme, EffectiveColorScheme::Dark);
        assert_eq!(theme.candidate.foreground, rgba(0x01, 0x02, 0x03, 0xff));
    }

    #[test]
    fn auto_ui_scheme_uses_system_variant() {
        let path = std::env::temp_dir().join(format!(
            "keytao-theme-auto-{}-{}.yaml",
            std::process::id(),
            line!()
        ));
        fs::write(
            &path,
            "ui:\n  colorScheme: auto\ndark:\n  candidate:\n    foreground: '#0A0B0C'\n",
        )
        .unwrap();

        let theme =
            resolve_theme_from_paths_with_system(None, Some(&path), EffectiveColorScheme::Dark);
        fs::remove_file(path).ok();

        assert_eq!(theme.ui.color_scheme, UiColorScheme::Auto);
        assert_eq!(theme.ui.effective_color_scheme, EffectiveColorScheme::Dark);
        assert_eq!(theme.candidate.foreground, rgba(0x0a, 0x0b, 0x0c, 0xff));
    }

    #[test]
    fn night_mode_alias_selects_dark_scheme() {
        let mut theme = ResolvedImeTheme::default();
        let partial = serde_yaml::from_str::<PartialTheme>("ui:\n  nightMode: true\n").unwrap();
        theme.apply(partial);

        assert_eq!(theme.ui.color_scheme, UiColorScheme::Dark);
    }

    #[test]
    fn accent_color_derives_highlight_colors() {
        let theme = resolve_theme_from_paths_with_system(None, None, EffectiveColorScheme::Light);
        let mut theme = theme;
        theme.ui.accent_color = Some(rgba(0x12, 0x34, 0x56, 0xff));
        theme.apply_accent_color(rgba(0x12, 0x34, 0x56, 0xff));

        assert_eq!(
            theme.candidate.selected_label_color,
            rgba(0x12, 0x34, 0x56, 0xff)
        );
        assert_eq!(theme.mode_hint.background, rgba(0x2d, 0x4b, 0x63, 0xff));
        assert_eq!(theme.mode_hint.foreground, rgba(0xff, 0xff, 0xff, 0xff));
        assert_eq!(theme.mode_hint.border_color, rgba(0x12, 0x34, 0x56, 0xff));
    }

    #[test]
    fn candidate_model_uses_select_keys_and_capabilities() {
        let theme = ResolvedImeTheme::default();
        let model = theme.candidate_panel_model(
            CandidatePanelInput {
                preedit: "abc".to_string(),
                candidates: vec![ThemeCandidate {
                    text: "这".to_string(),
                    comment: Some("~a".to_string()),
                }],
                highlighted_candidate_index: 0,
                page: 1,
                is_last_page: false,
                select_keys: Some("asdf".to_string()),
            },
            &UiCapabilities::full_custom(),
        );
        assert_eq!(model.preedit.as_deref(), Some("abc"));
        assert_eq!(model.candidates[0].label, "a.");
        assert!(model.candidates[0].selected);
        assert!(model.navigation.can_go_previous);
        assert!(model.navigation.can_go_next);
    }
}
