//! Shared KeyTao IME theme language.
//!
//! This crate owns the cross-platform theme schema, default values, merge rules,
//! and view models. Platform frontends render the resolved model with their own
//! native UI stack.
//!
//! The shared theme covers desktop candidates, mode hints and mobile appearance.
//! Mobile layout and geometry live in [`mobile_layout`], resolved separately
//! from `keyboard.yaml`; v3 theme sources compile to backward-compatible v2.

pub mod editor;
pub mod font_resolution;
pub mod mobile_layout;
pub mod mobile_theme;
pub mod panel_hit;
pub mod share;
pub mod source;
pub mod theme_store;
pub use source::THEME_SOURCE_VERSION;

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

/// Update the active v3 source, then atomically compile its v2 `theme.yaml`.
/// Local UI settings stay in the compiled document and are never shared.
/// An omitted accent preserves the existing override, including its absence.
pub fn write_ime_ui_settings_to_path(
    theme_path: &Path,
    color_scheme: UiColorScheme,
    orientation: PanelOrientation,
    accent_color: Option<String>,
    font_size: f32,
    corner_radius: Option<f32>,
    background_color: String,
) -> Result<(), String> {
    let accent_color = accent_color
        .map(|value| normalize_hex_color(&value))
        .transpose()?;
    let font_size = normalize_candidate_font_size(font_size)?;
    let corner_radius = corner_radius.map(normalize_corner_radius).transpose()?;
    let background_color = normalize_background_color(&background_color)?;
    theme_store::write_source_settings(
        theme_path,
        color_scheme,
        accent_color,
        Some(corner_radius),
        Some(background_color),
        Some(orientation),
        Some(font_size),
    )
}

/// Atomically update the mobile keyboard's color scheme and, when supplied,
/// its source accent. `None` preserves it; an empty string removes it.
pub fn write_theme_ui_to_path(
    theme_path: &Path,
    color_scheme: UiColorScheme,
    accent_color: Option<String>,
) -> Result<(), String> {
    write_theme_appearance_to_path(theme_path, color_scheme, accent_color, None, None)
}

/// Update appearance while preserving omitted values. `Some(None)` removes the
/// radius override; an empty color string removes that color override.
pub fn write_theme_appearance_to_path(
    theme_path: &Path,
    color_scheme: UiColorScheme,
    accent_color: Option<String>,
    corner_radius: Option<Option<f32>>,
    background_color: Option<String>,
) -> Result<(), String> {
    let accent_color = match accent_color {
        Some(value) if value.trim().is_empty() => Some(String::new()),
        Some(value) => Some(normalize_hex_color(&value)?),
        None => None,
    };
    let corner_radius = corner_radius
        .map(|value| value.map(normalize_corner_radius).transpose())
        .transpose()?;
    let background_color = background_color
        .map(|value| normalize_background_color(&value))
        .transpose()?;
    theme_store::write_source_settings(
        theme_path,
        color_scheme,
        accent_color,
        corner_radius,
        background_color,
        None,
        None,
    )
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

fn normalize_corner_radius(value: f32) -> Result<f32, String> {
    if !value.is_finite() {
        return Err("圆角必须是有效数字".into());
    }
    Ok(value.clamp(0.0, 28.0))
}

fn normalize_background_color(value: &str) -> Result<String, String> {
    if value.trim().is_empty() {
        Ok(String::new())
    } else {
        normalize_hex_color(value)
    }
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommentPosition {
    #[default]
    Inline,
    Below,
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

pub fn preedit_caret_color(text_color: RgbaColor) -> RgbaColor {
    with_alpha(text_color, (text_color.alpha as f32 * 0.35).round() as u8)
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelShadow {
    pub color: RgbaColor,
    pub offset_y: f32,
    pub blur_radius: f32,
    pub margin: f32,
}

pub fn resolved_panel_shadow(enabled: bool) -> Option<PanelShadow> {
    enabled.then_some(PanelShadow {
        color: rgba(0, 0, 0, 40),
        offset_y: 8.0,
        blur_radius: 16.0,
        margin: 24.0,
    })
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
    pub mobile: serde_json::Value,
}

#[derive(Clone, Debug)]
pub struct UiTheme {
    pub color_scheme: UiColorScheme,
    pub effective_color_scheme: EffectiveColorScheme,
    pub accent_color: Option<RgbaColor>,
    pub corner_radius: Option<f32>,
    pub background_color: Option<RgbaColor>,
    pub embedded_composition: bool,
    pub hide_candidate_panel: bool,
    pub show_tray_mode_indicator: bool,
}

impl Serialize for UiTheme {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;

        let mut state = serializer.serialize_struct("UiTheme", 9)?;
        state.serialize_field("colorScheme", &self.color_scheme)?;
        state.serialize_field("effectiveColorScheme", &self.effective_color_scheme)?;
        state.serialize_field("accentColor", &self.accent_color)?;
        state.serialize_field("cornerRadius", &self.corner_radius)?;
        state.serialize_field("backgroundColor", &self.background_color)?;
        state.serialize_field("embeddedComposition", &self.embedded_composition)?;
        state.serialize_field("hideCandidatePanel", &self.hide_candidate_panel)?;
        state.serialize_field("showTrayModeIndicator", &self.show_tray_mode_indicator)?;
        state.serialize_field(
            "effectiveEmbeddedComposition",
            &self.effective_embedded_composition(),
        )?;
        state.end()
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FontTheme {
    pub family: Option<String>,
    pub effective_family: Option<String>,
    #[cfg(target_os = "android")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_file: Option<String>,
    #[cfg(target_os = "android")]
    #[serde(skip_serializing_if = "is_zero_font_index")]
    pub effective_index: u32,
    pub size: f32,
    pub label_size: f32,
    pub comment_size: f32,
    pub preedit_size: f32,
    pub weight: FontWeight,
}

#[cfg(target_os = "android")]
fn is_zero_font_index(index: &u32) -> bool {
    *index == 0
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
    pub padding_top: f32,
    pub padding_right: f32,
    pub padding_bottom: f32,
    pub padding_left: f32,
    #[serde(skip)]
    padding_overrides: [Option<f32>; 4],
    pub gap: f32,
    pub preedit_gap: f32,
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
    pub comment_position: CommentPosition,
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
        let Ok(mut cache) = self.cache.lock() else {
            return resolve_theme_from_paths_with_system_scheme(
                self.default_theme_path.as_deref(),
                self.user_theme_path.as_deref(),
                self.system_scheme
                    .unwrap_or_else(cached_system_effective_color_scheme),
            );
        };
        let mut system_scheme = match cache.theme.ui.color_scheme {
            UiColorScheme::Auto => self
                .system_scheme
                .unwrap_or_else(cached_system_effective_color_scheme),
            UiColorScheme::Light => EffectiveColorScheme::Light,
            UiColorScheme::Dark => EffectiveColorScheme::Dark,
        };
        let mut signature = self.signature(system_scheme);
        if cache.signature == signature {
            return cache.theme.clone();
        }
        let mut theme = resolve_theme_from_paths_with_system_scheme(
            self.default_theme_path.as_deref(),
            self.user_theme_path.as_deref(),
            system_scheme,
        );
        if theme.ui.color_scheme == UiColorScheme::Auto
            && cache.theme.ui.color_scheme != UiColorScheme::Auto
        {
            system_scheme = self
                .system_scheme
                .unwrap_or_else(cached_system_effective_color_scheme);
            signature = self.signature(system_scheme);
            theme = resolve_theme_from_paths_with_system_scheme(
                self.default_theme_path.as_deref(),
                self.user_theme_path.as_deref(),
                system_scheme,
            );
        }
        cache.signature = signature;
        cache.theme = theme.clone();
        theme
    }

    fn signature(&self, system_scheme: EffectiveColorScheme) -> String {
        let mut parts = [
            self.default_theme_path.as_deref(),
            self.user_theme_path.as_deref(),
        ]
        .into_iter()
        .flatten()
        .map(path_signature)
        .collect::<Vec<_>>();
        parts.push(format!("system:{system_scheme:?}"));
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
    resolve_theme_from_paths_with_system_scheme(
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
    resolve_theme_with_pending_settings(default_theme_path, user_theme_path, system_scheme, None)
}

/// Scale candidate spacing and secondary fonts from the default candidate size.
pub fn scaled_candidate_panel(theme: &ResolvedImeTheme) -> ResolvedImeTheme {
    let scale = theme.font.size / ResolvedImeTheme::default().font.size;
    let mut theme = theme.clone();
    theme.font.label_size *= scale;
    theme.font.comment_size *= scale;
    theme.font.preedit_size *= scale;
    theme.panel.padding_x *= scale;
    theme.panel.padding_y *= scale;
    theme.panel.padding_top *= scale;
    theme.panel.padding_right *= scale;
    theme.panel.padding_bottom *= scale;
    theme.panel.padding_left *= scale;
    theme.panel.gap *= scale;
    theme.panel.preedit_gap *= scale;
    theme.candidate.padding_x *= scale;
    theme.candidate.padding_y *= scale;
    theme.candidate.inline_gap *= scale;
    theme.candidate.min_height *= scale;
    theme.navigation.button_size *= scale;
    theme
}

/// Resolve unsaved App settings without changing the user's theme document.
/// Candidate metrics match the native panels' default-size scaling.
pub fn resolve_candidate_panel_preview_from_paths(
    default_theme_path: Option<&Path>,
    user_theme_path: Option<&Path>,
    system_scheme: EffectiveColorScheme,
    color_scheme: UiColorScheme,
    accent_color: &str,
    font_size: f32,
    corner_radius: Option<f32>,
    background_color: &str,
) -> Result<ResolvedImeTheme, String> {
    let accent_color = normalize_hex_color(accent_color)?;
    let font_size = normalize_candidate_font_size(font_size)?;
    let corner_radius = corner_radius.map(normalize_corner_radius).transpose()?;
    let background_color = normalize_background_color(background_color)?;
    if let Some(source) = user_theme_path
        .map(theme_store::read_current_theme_source)
        .transpose()?
        .flatten()
    {
        let source = theme_store::edit_source_settings(
            source,
            Some(accent_color),
            Some(corner_radius),
            Some(background_color),
            None,
            Some(font_size),
        )?;
        let scheme = match color_scheme {
            UiColorScheme::Auto => system_scheme,
            UiColorScheme::Light => EffectiveColorScheme::Light,
            UiColorScheme::Dark => EffectiveColorScheme::Dark,
        };
        let mut theme = source::resolve_theme_source(&source, scheme);
        let local = resolve_theme_from_paths_with_system_scheme(
            default_theme_path,
            user_theme_path,
            system_scheme,
        )
        .ui;
        theme.ui.color_scheme = color_scheme;
        theme.ui.embedded_composition = local.embedded_composition;
        theme.ui.hide_candidate_panel = local.hide_candidate_panel;
        theme.ui.show_tray_mode_indicator = local.show_tray_mode_indicator;
        return Ok(scaled_candidate_panel(&theme));
    }
    let pending = PartialTheme {
        ui: Some(PartialUiTheme {
            color_scheme: Some(color_scheme),
            accent_color: parse_color(&accent_color),
            corner_radius: Some(corner_radius),
            background_color: Some(parse_color(&background_color)),
            ..Default::default()
        }),
        font: Some(PartialFontTheme {
            size: Some(font_size),
            ..Default::default()
        }),
        ..Default::default()
    };
    let theme = resolve_theme_with_pending_settings(
        default_theme_path,
        user_theme_path,
        system_scheme,
        Some(pending),
    );
    Ok(scaled_candidate_panel(&theme))
}

fn resolve_theme_with_pending_settings(
    default_theme_path: Option<&Path>,
    user_theme_path: Option<&Path>,
    system_scheme: EffectiveColorScheme,
    pending_settings: Option<PartialTheme>,
) -> ResolvedImeTheme {
    let mut partials = Vec::new();
    let mut compiled = None;
    if let Ok(partial) = serde_yaml::from_str::<PartialTheme>(DEFAULT_THEME_YAML) {
        partials.push(partial);
    }
    for path in [default_theme_path, user_theme_path].into_iter().flatten() {
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        let Ok((value, _)) = source::parse_compilation_base_yaml(&content) else {
            continue;
        };
        let Ok(partial) = serde_yaml::from_value::<PartialTheme>(value.clone()) else {
            continue;
        };
        if value.get("themeHash").is_some() {
            if let Ok(value) = serde_json::to_value(value) {
                compiled = Some(value);
            }
        }
        partials.push(partial);
    }
    let has_pending = pending_settings.is_some();
    partials.extend(pending_settings);
    let mut theme = resolve_legacy_partials(partials, system_scheme);
    if let Some(compiled) = compiled.filter(|_| !has_pending) {
        restore_compiled_theme(&mut theme, &compiled);
    }
    theme
}

fn restore_compiled_theme(theme: &mut ResolvedImeTheme, compiled: &serde_json::Value) {
    let variant = if theme.ui.effective_color_scheme == EffectiveColorScheme::Dark {
        "dark"
    } else {
        "light"
    };
    if let Some(metadata) = compiled.get("resolvedUi").and_then(|v| v.get(variant)) {
        theme.ui.accent_color = compiled["ui"]
            .get("accentColor")
            .or_else(|| metadata.get("accentColor"))
            .and_then(|v| v.as_str())
            .and_then(parse_color);
        theme.ui.corner_radius = compiled["ui"]
            .get("cornerRadius")
            .or_else(|| metadata.get("cornerRadius"))
            .and_then(|v| v.as_f64())
            .map(|v| v as f32);
        theme.ui.background_color = compiled["ui"]
            .get("backgroundColor")
            .or_else(|| metadata.get("backgroundColor"))
            .and_then(|v| v.as_str())
            .and_then(parse_color);
        if let Some(scheme) = metadata
            .get("effectiveColorScheme")
            .and_then(|v| v.as_str())
            .and_then(parse_effective_color_scheme)
        {
            theme.ui.effective_color_scheme = scheme;
        }
    }
    if let Some(mobile) = if variant == "dark" {
        compiled.pointer("/dark/mobile")
    } else {
        compiled.get("mobile")
    } {
        theme.mobile = mobile_theme::resolve_mobile_theme(
            theme,
            mobile,
            theme.ui.effective_color_scheme == EffectiveColorScheme::Dark,
        );
        // Keep compiled-only states intact while filling newly added fields.
        source::merge(&mut theme.mobile, mobile);
    }
}

pub(crate) fn resolve_compiled_theme_yaml(
    yaml: &str,
    scheme: EffectiveColorScheme,
) -> ResolvedImeTheme {
    let mut theme = resolve_legacy_theme_yaml(yaml, scheme);
    if let Ok(compiled) = serde_yaml::from_str::<serde_json::Value>(yaml) {
        if compiled.get("themeHash").is_some() {
            restore_compiled_theme(&mut theme, &compiled);
        }
    }
    theme
}

/// The frozen v2 resolver used to prove migration equivalence without disk I/O.
pub(crate) fn resolve_legacy_theme_yaml(
    yaml: &str,
    scheme: EffectiveColorScheme,
) -> ResolvedImeTheme {
    let mut partials = vec![serde_yaml::from_str(DEFAULT_THEME_YAML).expect("bundled theme")];
    if let Ok(partial) = serde_yaml::from_str(yaml) {
        partials.push(partial);
    }
    resolve_legacy_partials(partials, scheme)
}

fn resolve_legacy_partials(
    partials: Vec<PartialTheme>,
    system_scheme: EffectiveColorScheme,
) -> ResolvedImeTheme {
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

    let resolve_variant = |scheme| {
        let mut theme = ResolvedImeTheme::schema_base(ui.clone());
        for partial in &partials {
            theme.apply(partial.clone());
            if scheme == EffectiveColorScheme::Light {
                if let Some(light) = partial.light.clone() {
                    theme.apply_variant(light);
                }
            } else if let Some(dark) = partial.dark.clone() {
                theme.apply_variant(dark);
            }
        }
        theme
    };
    let mut theme = resolve_variant(ui.effective_color_scheme);
    theme.ui = ui.clone();
    if let Some(radius) = ui.corner_radius {
        theme.panel.corner_radius = radius;
        theme.candidate.corner_radius = radius * 11.0 / 16.0;
        theme.navigation.corner_radius = radius * 10.0 / 16.0;
        theme.mode_hint.corner_radius = radius * 14.0 / 16.0;
    }
    if let Some(background) = ui.background_color {
        theme.panel.background = with_alpha(background, theme.panel.background.alpha);
        let alternate_scheme = match ui.effective_color_scheme {
            EffectiveColorScheme::Light => EffectiveColorScheme::Dark,
            EffectiveColorScheme::Dark => EffectiveColorScheme::Light,
        };
        let alternate = resolve_variant(alternate_scheme);
        if contrast_ratio(background, alternate.candidate.foreground)
            > contrast_ratio(background, theme.candidate.foreground)
        {
            // Keep text and its interaction surfaces paired without changing layout or panel alpha.
            theme.panel.border_color = alternate.panel.border_color;
            theme.candidate.background = alternate.candidate.background;
            theme.candidate.hover_background = alternate.candidate.hover_background;
            theme.candidate.pressed_background = alternate.candidate.pressed_background;
            theme.candidate.pressed_foreground = alternate.candidate.pressed_foreground;
            theme.candidate.selected_background = alternate.candidate.selected_background;
            theme.candidate.foreground = alternate.candidate.foreground;
            theme.candidate.selected_foreground = alternate.candidate.selected_foreground;
            theme.candidate.label_color = alternate.candidate.label_color;
            theme.candidate.selected_label_color = alternate.candidate.selected_label_color;
            theme.candidate.comment_color = alternate.candidate.comment_color;
            theme.candidate.selected_comment_color = alternate.candidate.selected_comment_color;
            theme.candidate.border_color = alternate.candidate.border_color;
            theme.candidate.selected_border_color = alternate.candidate.selected_border_color;
            theme.candidate.separator_color = alternate.candidate.separator_color;
            theme.navigation.foreground = alternate.navigation.foreground;
            theme.navigation.disabled_foreground = alternate.navigation.disabled_foreground;
            theme.navigation.hover_background = alternate.navigation.hover_background;
            theme.mode_hint.background = alternate.mode_hint.background;
            theme.mode_hint.foreground = alternate.mode_hint.foreground;
            theme.mode_hint.border_color = alternate.mode_hint.border_color;
        }
    }
    if let Some(accent_color) = ui.accent_color {
        theme.apply_accent_color(accent_color);
    }

    let mut theme = theme.sanitized();
    theme.mobile = mobile_theme::resolve_mobile_theme(
        &theme,
        &serde_json::Value::Null,
        theme.ui.effective_color_scheme == EffectiveColorScheme::Dark,
    );
    theme
}

pub fn resolved_theme_json(theme: &ResolvedImeTheme) -> Result<String, serde_json::Error> {
    let mut resolved = serde_json::to_value(theme)?;
    resolved["font"]["family"] =
        serde_json::Value::String(theme.font.family.clone().unwrap_or_default());
    serde_json::to_string(&resolved)
}

/// Renderer metadata is separate from the frozen v2 resolved-theme contract.
pub fn resolved_renderer_theme_json(theme: &ResolvedImeTheme) -> Result<String, serde_json::Error> {
    let mut resolved: serde_json::Value = serde_json::from_str(&resolved_theme_json(theme)?)?;
    resolved["candidate"]["preeditCaretColor"] =
        serde_json::to_value(preedit_caret_color(theme.candidate.foreground))?;
    resolved["panel"]["shadowStyle"] =
        serde_json::to_value(resolved_panel_shadow(theme.panel.shadow))?;
    resolved["modeHint"]["shadowStyle"] =
        serde_json::to_value(resolved_panel_shadow(theme.mode_hint.shadow))?;
    serde_json::to_string(&resolved)
}

impl ResolvedImeTheme {
    fn schema_base(ui: UiTheme) -> Self {
        Self {
            version: THEME_SCHEMA_VERSION,
            ui,
            font: FontTheme {
                family: None,
                effective_family: None,
                #[cfg(target_os = "android")]
                effective_file: None,
                #[cfg(target_os = "android")]
                effective_index: 0,
                size: 20.0,
                label_size: 15.0,
                comment_size: 16.0,
                preedit_size: 15.0,
                weight: FontWeight::SemiBold,
            },
            panel: PanelTheme {
                orientation: PanelOrientation::Horizontal,
                background: rgba(0xF8, 0xFA, 0xFF, 0xF2),
                border_color: rgba(0xB8, 0xC3, 0xD0, 0xFF),
                border_width: 1.0,
                corner_radius: 16.0,
                padding_x: 10.0,
                padding_y: 10.0,
                padding_top: 10.0,
                padding_right: 10.0,
                padding_bottom: 10.0,
                padding_left: 10.0,
                padding_overrides: [None; 4],
                gap: 4.0,
                preedit_gap: 10.0,
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
                comment_position: CommentPosition::Inline,
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
            mobile: serde_json::Value::Null,
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
        // Follow the actual background, which a custom color may have flipped.
        let is_dark = relative_luminance(panel_background) < 0.18;
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
        self.panel.padding_top = clamp(
            self.panel.padding_overrides[0].unwrap_or(self.panel.padding_y),
            0.0,
            28.0,
        );
        self.panel.padding_right = clamp(
            self.panel.padding_overrides[1].unwrap_or(self.panel.padding_x),
            0.0,
            32.0,
        );
        self.panel.padding_bottom = clamp(
            self.panel.padding_overrides[2].unwrap_or(self.panel.padding_y),
            0.0,
            28.0,
        );
        self.panel.padding_left = clamp(
            self.panel.padding_overrides[3].unwrap_or(self.panel.padding_x),
            0.0,
            32.0,
        );
        self.panel.gap = clamp(self.panel.gap, 0.0, 24.0);
        self.panel.preedit_gap = clamp(self.panel.preedit_gap, 0.0, 48.0);
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
        self.font.resolve_family();
        self
    }
}

impl Default for ResolvedImeTheme {
    fn default() -> Self {
        resolve_theme_with_pending_settings(None, None, EffectiveColorScheme::Light, None)
    }
}

impl Default for UiTheme {
    fn default() -> Self {
        Self {
            color_scheme: UiColorScheme::Auto,
            effective_color_scheme: EffectiveColorScheme::Light,
            accent_color: None,
            corner_radius: None,
            background_color: None,
            embedded_composition: true,
            hide_candidate_panel: false,
            show_tray_mode_indicator: true,
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
    #[serde(alias = "hide_candidate_panel")]
    hide_candidate_panel: Option<bool>,
    #[serde(alias = "show_tray_mode_indicator")]
    show_tray_mode_indicator: Option<bool>,
    #[serde(alias = "night_mode")]
    night_mode: Option<bool>,
    #[serde(default, deserialize_with = "optional_color")]
    accent_color: Option<RgbaColor>,
    #[serde(default, deserialize_with = "optional_corner_radius")]
    corner_radius: Option<Option<f32>>,
    #[serde(default, deserialize_with = "optional_background_color")]
    background_color: Option<Option<RgbaColor>>,
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
    padding_top: Option<f32>,
    padding_right: Option<f32>,
    padding_bottom: Option<f32>,
    padding_left: Option<f32>,
    gap: Option<f32>,
    preedit_gap: Option<f32>,
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
    comment_position: Option<String>,
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
    pub fn effective_embedded_composition(&self) -> bool {
        self.hide_candidate_panel || self.embedded_composition
    }

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
        assign(&mut self.corner_radius, partial.corner_radius);
        assign(&mut self.background_color, partial.background_color);
        if let Some(embedded_composition) = partial.embedded_composition {
            self.embedded_composition = embedded_composition;
        }
        if let Some(hide_candidate_panel) = partial.hide_candidate_panel {
            self.hide_candidate_panel = hide_candidate_panel;
        }
        if let Some(show_tray_mode_indicator) = partial.show_tray_mode_indicator {
            self.show_tray_mode_indicator = show_tray_mode_indicator;
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
        for (value, side) in self.padding_overrides.iter_mut().zip([
            partial.padding_top,
            partial.padding_right,
            partial.padding_bottom,
            partial.padding_left,
        ]) {
            if side.is_some() {
                *value = side;
            }
        }
        assign(&mut self.gap, partial.gap);
        assign(&mut self.preedit_gap, partial.preedit_gap);
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
        match partial.comment_position.as_deref() {
            Some("inline") => self.comment_position = CommentPosition::Inline,
            Some("below") => self.comment_position = CommentPosition::Below,
            _ => (),
        }
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

fn optional_corner_radius<'de, D>(deserializer: D) -> Result<Option<Option<f32>>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_yaml::Value::deserialize(deserializer)?;
    Ok(Some(value.as_f64().and_then(|value| {
        normalize_corner_radius(value as f32).ok()
    })))
}

fn optional_background_color<'de, D>(deserializer: D) -> Result<Option<Option<RgbaColor>>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_yaml::Value::deserialize(deserializer)?;
    Ok(Some(value.as_str().and_then(|value| {
        let hex = value.trim().strip_prefix('#')?;
        (hex.len() == 6 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then(|| parse_color(value))
            .flatten()
    })))
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

fn relative_luminance(color: RgbaColor) -> f64 {
    let linear = |channel: u8| {
        let srgb = f64::from(channel) / 255.0;
        if srgb <= 0.04045 {
            srgb / 12.92
        } else {
            ((srgb + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color.red) + 0.7152 * linear(color.green) + 0.0722 * linear(color.blue)
}

fn contrast_ratio(background: RgbaColor, foreground: RgbaColor) -> f64 {
    let foreground = mix_color(background, foreground, f32::from(foreground.alpha) / 255.0);
    let background = relative_luminance(background);
    let foreground = relative_luminance(foreground);
    (background.max(foreground) + 0.05) / (background.min(foreground) + 0.05)
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
    fn preedit_caret_preserves_rgb_and_reduces_existing_alpha() {
        for scheme in [EffectiveColorScheme::Light, EffectiveColorScheme::Dark] {
            let theme = resolve_theme_from_paths_with_system_scheme(None, None, scheme);
            for text in [
                theme.candidate.foreground,
                theme.candidate.selected_label_color,
            ] {
                for alpha in [0, 128, 255] {
                    let text = with_alpha(text, alpha);
                    let caret = preedit_caret_color(text);
                    assert_eq!(opaque(caret), opaque(text));
                    assert_eq!(caret.alpha, (alpha as f32 * 0.35).round() as u8);
                }
            }
        }
    }

    #[test]
    fn appearance_defaults_match_existing_light_and_dark_themes() {
        for (scheme, background, foreground) in [
            (
                EffectiveColorScheme::Light,
                rgba(0xf8, 0xfa, 0xff, 0xf2),
                rgba(0x26, 0x34, 0x42, 0xff),
            ),
            (
                EffectiveColorScheme::Dark,
                rgba(0x17, 0x1a, 0x20, 0xf2),
                rgba(0xee, 0xf3, 0xf7, 0xff),
            ),
        ] {
            let theme = resolve_theme_from_paths_with_system_scheme(None, None, scheme);
            assert_eq!(theme.ui.corner_radius, None);
            assert_eq!(theme.ui.background_color, None);
            assert_eq!(theme.panel.background, background);
            assert_eq!(theme.candidate.foreground, foreground);
            assert_eq!(theme.panel.corner_radius, 16.0);
            assert_eq!(theme.candidate.corner_radius, 11.0);
            assert_eq!(theme.navigation.corner_radius, 10.0);
            assert_eq!(theme.mode_hint.corner_radius, 14.0);
        }
    }

    #[test]
    fn custom_background_uses_highest_contrast_variant_colors_without_changing_metrics() {
        let partial: PartialTheme = serde_yaml::from_str(
            "light:\n  font:\n    size: 25\n  panel:\n    background: '#FFFFFF80'\n  candidate:\n    foreground: '#102030'\n    paddingX: 7\n  modeHint:\n    foreground: '#112233'\ndark:\n  font:\n    size: 31\n  panel:\n    background: '#00000090'\n  candidate:\n    foreground: '#E0F0FF'\n    paddingX: 13\n  modeHint:\n    foreground: '#DDEEFF'\n",
        )
        .unwrap();
        for (scheme, background, text_scheme) in [
            (
                EffectiveColorScheme::Light,
                "#1C1D21",
                EffectiveColorScheme::Dark,
            ),
            (
                EffectiveColorScheme::Dark,
                "#F6F5F2",
                EffectiveColorScheme::Light,
            ),
        ] {
            let original =
                resolve_theme_with_pending_settings(None, None, scheme, Some(partial.clone()));
            let expected =
                resolve_theme_with_pending_settings(None, None, text_scheme, Some(partial.clone()));
            let mut custom = partial.clone();
            custom.ui = Some(PartialUiTheme {
                background_color: Some(parse_color(background)),
                ..Default::default()
            });
            let actual = resolve_theme_with_pending_settings(None, None, scheme, Some(custom));
            assert_eq!(actual.ui.effective_color_scheme, scheme);
            assert_eq!(
                actual.panel.background,
                with_alpha(
                    parse_color(background).unwrap(),
                    original.panel.background.alpha
                )
            );
            assert_eq!(actual.font.size, original.font.size);
            assert_eq!(actual.candidate.padding_x, original.candidate.padding_x);
            assert_eq!(actual.panel.border_color, expected.panel.border_color);
            let actual = serde_json::to_value(actual).unwrap();
            let expected = serde_json::to_value(expected).unwrap();
            for section in ["candidate", "navigation", "modeHint"] {
                for (key, color) in expected[section].as_object().unwrap() {
                    if color.get("red").is_some() {
                        assert_eq!(actual[section][key], *color, "{section}.{key}");
                    }
                }
            }
        }
    }

    #[test]
    fn custom_background_contrast_accounts_for_foreground_alpha() {
        let theme = resolve_theme_with_pending_settings(
            None,
            None,
            EffectiveColorScheme::Light,
            Some(serde_yaml::from_str(
                "ui:\n  backgroundColor: '#FFFFFF'\nlight:\n  candidate:\n    foreground: '#00000000'\ndark:\n  candidate:\n    foreground: '#555555'\n",
            ).unwrap()),
        );
        assert_eq!(theme.candidate.foreground, rgba(0x55, 0x55, 0x55, 255));
        assert_eq!(
            contrast_ratio(theme.panel.background, rgba(0, 0, 0, 0)),
            1.0
        );
    }

    #[test]
    fn preset_backgrounds_have_accessible_text_in_runtime_and_preview() {
        let options = include_str!("../../../flutter_app/lib/src/app/options.dart");
        let presets = options
            .split_once("const themeBackgrounds = {")
            .unwrap()
            .1
            .split_once("};")
            .unwrap()
            .0;
        let backgrounds: Vec<_> = presets
            .lines()
            .filter_map(|line| line.split('\'').nth(1))
            .collect();
        assert_eq!(backgrounds.len(), 8);
        assert_eq!(
            contrast_ratio(rgba(0, 0, 0, 255), rgba(255, 255, 255, 255)),
            21.0
        );
        for background in backgrounds {
            for scheme in [EffectiveColorScheme::Light, EffectiveColorScheme::Dark] {
                let theme = resolve_theme_with_pending_settings(
                    None,
                    None,
                    scheme,
                    Some(
                        serde_yaml::from_str(&format!(
                            "ui:\n  backgroundColor: '{background}'\n  accentColor: '#3B73D9'\n"
                        ))
                        .unwrap(),
                    ),
                );
                let preview = resolve_candidate_panel_preview_from_paths(
                    None,
                    None,
                    scheme,
                    UiColorScheme::Auto,
                    "#3B73D9",
                    20.0,
                    None,
                    background,
                )
                .unwrap();
                let contrast = contrast_ratio(theme.panel.background, theme.candidate.foreground);
                assert!(contrast >= 4.5, "{scheme:?} {background}: {contrast}");
                assert_eq!(
                    resolved_theme_json(&theme).unwrap(),
                    resolved_theme_json(&preview).unwrap()
                );
                assert_eq!(theme.ui.accent_color, Some(rgba(0x3b, 0x73, 0xd9, 255)));
                assert_eq!(
                    theme.candidate.selected_label_color,
                    rgba(0x3b, 0x73, 0xd9, 255)
                );
                let background = theme.panel.background;
                let mobile_dark = (u32::from(background.red) * 299
                    + u32::from(background.green) * 587
                    + u32::from(background.blue) * 114)
                    / 1000
                    < 128;
                let text_scheme = if mobile_dark {
                    EffectiveColorScheme::Dark
                } else {
                    EffectiveColorScheme::Light
                };
                let expected = resolve_theme_from_paths_with_system_scheme(None, None, text_scheme);
                assert_eq!(theme.candidate.foreground, expected.candidate.foreground);
            }
        }
    }

    #[test]
    fn appearance_overrides_follow_variants_and_preserve_background_alpha() {
        for scheme in [EffectiveColorScheme::Light, EffectiveColorScheme::Dark] {
            let theme = resolve_theme_with_pending_settings(
                None,
                None,
                scheme,
                Some(serde_yaml::from_str(
                    "ui:\n  cornerRadius: 24\n  backgroundColor: '#123456'\n  accentColor: '#ABCDEF'\nlight:\n  panel:\n    background: '#FFFFFF80'\n    cornerRadius: 7\ndark:\n  panel:\n    background: '#00000090'\n    cornerRadius: 8\n",
                ).unwrap()),
            );
            assert_eq!(theme.panel.corner_radius, 24.0);
            assert_eq!(theme.candidate.corner_radius, 16.5);
            assert_eq!(theme.navigation.corner_radius, 15.0);
            assert_eq!(theme.mode_hint.corner_radius, 21.0);
            let alpha = if scheme == EffectiveColorScheme::Light {
                0x80
            } else {
                0x90
            };
            assert_eq!(theme.panel.background, rgba(0x12, 0x34, 0x56, alpha));
            // #123456 is dark, so the dark blend applies in either scheme.
            let weight = 0.42;
            assert_eq!(
                theme.candidate.selected_background,
                mix_color(theme.panel.background, rgba(0xab, 0xcd, 0xef, 255), weight)
            );
            let json = serde_json::to_value(&theme).unwrap();
            assert_eq!(json["ui"]["cornerRadius"], 24.0);
            assert_eq!(json["ui"]["backgroundColor"]["alpha"], 255);
        }
    }

    #[test]
    fn appearance_parsing_clamps_numbers_and_defaults_invalid_values() {
        for (value, expected) in [
            ("-4", Some(0.0)),
            ("99", Some(28.0)),
            ("8.5", Some(8.5)),
            (".nan", None),
            (".inf", None),
            ("'bad'", None),
            ("null", None),
            ("[]", None),
        ] {
            let theme = resolve_theme_with_pending_settings(
                None,
                None,
                EffectiveColorScheme::Light,
                Some(serde_yaml::from_str(&format!("ui:\n  cornerRadius: {value}\n")).unwrap()),
            );
            assert_eq!(theme.ui.corner_radius, expected);
            assert_eq!(theme.panel.corner_radius, expected.unwrap_or(16.0));
        }
        for value in [
            "''",
            "'#12345678'",
            "'#+12345'",
            "'bad'",
            "123",
            "null",
            "[]",
        ] {
            let theme = resolve_theme_with_pending_settings(
                None,
                None,
                EffectiveColorScheme::Light,
                Some(serde_yaml::from_str(&format!("ui:\n  backgroundColor: {value}\n")).unwrap()),
            );
            assert_eq!(theme.ui.background_color, None);
            assert_eq!(theme.panel.background, rgba(0xf8, 0xfa, 0xff, 0xf2));
        }
    }

    #[test]
    fn appearance_writer_preserves_and_clears_overrides_and_unrelated_fields() {
        let path = writer_test_theme_path("appearance");
        fs::write(
            &path,
            "ui:\n  showTrayModeIndicator: false\ncustom: preserved\n",
        )
        .unwrap();
        write_theme_appearance_to_path(
            &path,
            UiColorScheme::Light,
            None,
            Some(Some(99.0)),
            Some("#aabbcc".into()),
        )
        .unwrap();
        write_theme_ui_to_path(&path, UiColorScheme::Dark, None).unwrap();
        let theme = resolve_theme_from_paths(None, Some(&path));
        assert_eq!(theme.ui.corner_radius, Some(28.0));
        assert_eq!(theme.ui.background_color, Some(rgba(0xaa, 0xbb, 0xcc, 255)));
        assert!(!theme.ui.show_tray_mode_indicator);
        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("custom: preserved"));
        assert!(write_theme_appearance_to_path(
            &path,
            UiColorScheme::Light,
            None,
            Some(Some(f32::NAN)),
            None
        )
        .is_err());
        assert!(write_theme_appearance_to_path(
            &path,
            UiColorScheme::Light,
            None,
            None,
            Some("#12345678".into())
        )
        .is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), written);
        write_ime_ui_settings_to_path(
            &path,
            UiColorScheme::Light,
            PanelOrientation::Horizontal,
            Some("#123456".into()),
            20.0,
            None,
            String::new(),
        )
        .unwrap();
        let theme = resolve_theme_from_paths(None, Some(&path));
        assert_eq!(theme.ui.corner_radius, None);
        assert_eq!(theme.ui.background_color, None);
        write_theme_appearance_to_path(
            &path,
            UiColorScheme::Light,
            None,
            Some(Some(8.0)),
            Some("#123456".into()),
        )
        .unwrap();
        write_theme_appearance_to_path(
            &path,
            UiColorScheme::Light,
            None,
            Some(None),
            Some(String::new()),
        )
        .unwrap();
        let written: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(written["ui"].get("cornerRadius").is_none());
        assert!(written["ui"].get("backgroundColor").is_none());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn preview_can_clear_saved_appearance_without_writing_it() {
        let path = std::env::temp_dir().join(format!(
            "keytao-theme-appearance-preview-{}.yaml",
            std::process::id()
        ));
        let saved = "ui:\n  cornerRadius: 28\n  backgroundColor: '#123456'\n";
        fs::write(&path, saved).unwrap();
        let theme = resolve_candidate_panel_preview_from_paths(
            None,
            Some(&path),
            EffectiveColorScheme::Light,
            UiColorScheme::Light,
            "#3B73D9",
            20.0,
            None,
            "",
        )
        .unwrap();
        assert_eq!(theme.panel.corner_radius, 16.0);
        assert_eq!(theme.panel.background, rgba(0xf8, 0xfa, 0xff, 0xf2));
        assert_eq!(theme.ui.corner_radius, None);
        assert_eq!(theme.ui.background_color, None);
        assert_eq!(fs::read_to_string(&path).unwrap(), saved);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn mobile_theme_writer_updates_ui_and_preserves_other_settings() {
        let path = writer_test_theme_path("mobile");
        fs::write(
            &path,
            "ui:\n  showTrayModeIndicator: false\npanel:\n  orientation: vertical\nfont:\n  size: 27\ncandidate:\n  labelSuffix: ')'\n",
        )
        .unwrap();

        write_theme_ui_to_path(&path, UiColorScheme::Dark, Some("#123456".into())).unwrap();

        let theme = resolve_theme_from_paths(None, Some(&path));
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
        assert_eq!(theme.ui.color_scheme, UiColorScheme::Dark);
        assert_eq!(theme.ui.accent_color, Some(rgba(0x12, 0x34, 0x56, 0xff)));
        assert!(!theme.ui.show_tray_mode_indicator);
        assert_eq!(theme.panel.orientation, PanelOrientation::Vertical);
        assert_eq!(theme.font.size, 27.0);
        assert_eq!(theme.candidate.label_suffix, ")");
    }

    #[test]
    fn mobile_theme_writer_preserves_accent_when_it_is_omitted() {
        let path = writer_test_theme_path("preserve-accent");
        fs::write(
            &path,
            "ui:\n  colorScheme: light\n  accentColor: '#123456'\n",
        )
        .unwrap();

        write_theme_ui_to_path(&path, UiColorScheme::Dark, None).unwrap();

        let written: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["ui"]["colorScheme"].as_str(), Some("dark"));
        assert!(written["ui"].get("accentColor").is_none());
        let source = theme_store::read_current_theme_source(&path)
            .unwrap()
            .unwrap();
        assert_eq!(
            source.value()["common"]["accentColor"].as_str(),
            Some("#123456FF")
        );
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn mobile_theme_writer_removes_accent_when_it_is_explicitly_cleared() {
        let path = writer_test_theme_path("clear-accent");
        fs::write(
            &path,
            "ui:\n  colorScheme: dark\n  accentColor: '#123456'\n",
        )
        .unwrap();

        write_theme_ui_to_path(&path, UiColorScheme::Auto, Some(String::new())).unwrap();

        let written: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["ui"]["colorScheme"].as_str(), Some("auto"));
        assert!(written["ui"].get("accentColor").is_none());
        let source = theme_store::read_current_theme_source(&path)
            .unwrap()
            .unwrap();
        assert!(source.value()["common"].get("accentColor").is_none());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn full_theme_writer_clamps_font_and_rejects_non_finite_values() {
        let path = writer_test_theme_path("full");
        fs::write(&path, "ui:\n  showTrayModeIndicator: false\n").unwrap();

        write_ime_ui_settings_to_path(
            &path,
            UiColorScheme::Auto,
            PanelOrientation::Horizontal,
            Some("#3B73D9".into()),
            99.0,
            None,
            String::new(),
        )
        .unwrap();
        let theme = resolve_theme_from_paths(None, Some(&path));
        assert_eq!(theme.font.size, MAX_CANDIDATE_FONT_SIZE);
        assert!(!theme.ui.show_tray_mode_indicator);
        assert!(write_ime_ui_settings_to_path(
            &path,
            UiColorScheme::Auto,
            PanelOrientation::Horizontal,
            Some("#3B73D9".into()),
            f32::NAN,
            None,
            String::new(),
        )
        .is_err());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    fn writer_test_theme_path(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "keytao-theme-writer-{label}-{}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir.join("theme.yaml")
    }

    #[test]
    fn default_theme_yaml_resolves() {
        let theme = resolve_theme_from_paths(None, None);
        assert_eq!(theme.version, THEME_SCHEMA_VERSION);
        assert_eq!(theme.ui.color_scheme, UiColorScheme::Auto);
        assert!(theme.ui.embedded_composition);
        assert!(!theme.ui.hide_candidate_panel);
        assert!(theme.ui.show_tray_mode_indicator);
        assert!(theme.ui.effective_embedded_composition());
        assert_eq!(theme.panel.orientation, PanelOrientation::Horizontal);
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
    fn tray_mode_indicator_defaults_to_visible_when_omitted() {
        let mut ui = UiTheme::default();
        assert!(ui.show_tray_mode_indicator);
        ui.apply(serde_yaml::from_str::<PartialUiTheme>("colorScheme: dark\n").unwrap());
        assert!(ui.show_tray_mode_indicator);
    }

    #[test]
    fn tray_mode_indicator_round_trips_and_preserves_explicit_false() {
        for key in ["showTrayModeIndicator", "show_tray_mode_indicator"] {
            for visible in [true, false] {
                let mut ui = UiTheme::default();
                ui.apply(
                    serde_yaml::from_str::<PartialUiTheme>(&format!("{key}: {visible}\n")).unwrap(),
                );
                ui.apply(PartialUiTheme::default());
                assert_eq!(ui.show_tray_mode_indicator, visible);
                let serialized = serde_json::to_value(&ui).unwrap();
                assert_eq!(serialized["showTrayModeIndicator"], visible);
                assert!(serialized.get("show_tray_mode_indicator").is_none());
                let mut restored = UiTheme::default();
                restored.apply(serde_json::from_value(serialized).unwrap());
                assert_eq!(restored.show_tray_mode_indicator, visible);
            }
        }
    }

    #[test]
    fn hide_candidate_panel_forces_effective_embedded_composition() {
        for hide in [false, true] {
            for embedded in [false, true] {
                let mut theme = ResolvedImeTheme::default();
                let partial = serde_yaml::from_str::<PartialTheme>(&format!(
                    "ui:\n  hideCandidatePanel: {hide}\n  embeddedComposition: {embedded}\n"
                ))
                .unwrap();
                theme.apply(partial);
                assert_eq!(theme.ui.hide_candidate_panel, hide);
                assert_eq!(theme.ui.embedded_composition, embedded);
                assert_eq!(theme.ui.effective_embedded_composition(), hide || embedded);
                let json = serde_json::to_value(&theme).unwrap();
                assert_eq!(json["ui"]["hideCandidatePanel"], hide);
                assert_eq!(json["ui"]["embeddedComposition"], embedded);
                assert_eq!(json["ui"]["effectiveEmbeddedComposition"], hide || embedded);
            }
        }
    }

    #[test]
    fn hide_candidate_panel_accepts_legacy_key_and_rejects_non_boolean() {
        let partial =
            serde_yaml::from_str::<PartialTheme>("ui:\n  hide_candidate_panel: true\n").unwrap();
        let mut theme = ResolvedImeTheme::default();
        theme.apply(partial);
        assert!(theme.ui.hide_candidate_panel);
        assert!(
            serde_yaml::from_str::<PartialTheme>("ui:\n  hideCandidatePanel: wrong\n").is_err()
        );
    }

    #[test]
    fn resolved_default_uses_builtin_theme_yaml() {
        let theme = ResolvedImeTheme::default();
        assert_eq!(theme.panel.orientation, PanelOrientation::Horizontal);
        assert_eq!(theme.candidate.min_height, 34.0);
        assert_eq!(theme.mode_hint.background, rgba(0xf8, 0xfa, 0xff, 0xf2));
        assert_eq!(theme.mode_hint.foreground, rgba(0x26, 0x34, 0x42, 0xff));
    }

    #[test]
    fn resolved_theme_json_carries_no_mobile_layout() {
        let theme = resolve_theme_from_paths(None, None);
        let json = resolved_theme_json(&theme).unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(&json)
            .unwrap()
            .get("keyboard")
            .is_none());
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
        assert!(serde_json::from_str::<serde_json::Value>(&json)
            .unwrap()
            .get("keyboard")
            .is_none());
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

        let theme = resolve_theme_from_paths_with_system_scheme(
            None,
            Some(&path),
            EffectiveColorScheme::Dark,
        );
        fs::remove_file(path).ok();

        assert_eq!(theme.ui.color_scheme, UiColorScheme::Auto);
        assert_eq!(theme.ui.effective_color_scheme, EffectiveColorScheme::Dark);
        assert_eq!(theme.candidate.foreground, rgba(0x0a, 0x0b, 0x0c, 0xff));
    }

    #[test]
    fn resolver_ignores_system_changes_until_auto_is_selected() {
        let path = std::env::temp_dir().join(format!(
            "keytao-theme-resolver-scheme-{}-{}.yaml",
            std::process::id(),
            line!()
        ));
        fs::write(&path, "ui:\n  colorScheme: light\n").unwrap();
        let mut resolver = ThemeResolver::with_system_scheme(
            None,
            Some(path.clone()),
            Some(EffectiveColorScheme::Light),
        );
        assert_eq!(
            resolver.current().ui.effective_color_scheme,
            EffectiveColorScheme::Light
        );
        let fixed_signature = resolver.cache.lock().unwrap().signature.clone();

        resolver.system_scheme = Some(EffectiveColorScheme::Dark);
        assert_eq!(
            resolver.current().ui.effective_color_scheme,
            EffectiveColorScheme::Light
        );
        assert_eq!(resolver.cache.lock().unwrap().signature, fixed_signature);

        fs::write(
            &path,
            "ui:\n  colorScheme: auto\ndark:\n  candidate:\n    foreground: '#0A0B0C'\n",
        )
        .unwrap();
        let theme = resolver.current();
        fs::remove_file(path).ok();
        assert_eq!(theme.ui.color_scheme, UiColorScheme::Auto);
        assert_eq!(theme.ui.effective_color_scheme, EffectiveColorScheme::Dark);
        assert_eq!(theme.candidate.foreground, rgba(0x0a, 0x0b, 0x0c, 0xff));
    }

    #[test]
    fn preview_pending_settings_override_saved_values_before_variant_resolution() {
        let path = std::env::temp_dir().join(format!(
            "keytao-theme-preview-{}-{}.yaml",
            std::process::id(),
            line!()
        ));
        let saved = "ui:\n  colorScheme: dark\n  accentColor: '#FFFFFF'\nfont:\n  size: 36\nlight:\n  font:\n    size: 30\n  panel:\n    background: '#102030'\n  candidate:\n    foreground: '#010203'\ndark:\n  candidate:\n    foreground: '#AABBCC'\n";
        fs::write(&path, saved).unwrap();

        let theme = resolve_candidate_panel_preview_from_paths(
            None,
            Some(&path),
            EffectiveColorScheme::Dark,
            UiColorScheme::Light,
            "123456",
            15.0,
            None,
            "",
        )
        .unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), saved);
        fs::remove_file(path).ok();
        assert_eq!(theme.ui.color_scheme, UiColorScheme::Light);
        assert_eq!(theme.ui.effective_color_scheme, EffectiveColorScheme::Light);
        assert_eq!(theme.ui.accent_color, Some(rgba(0x12, 0x34, 0x56, 0xff)));
        assert_eq!(theme.font.size, 15.0);
        assert_eq!(theme.candidate.foreground, rgba(0x01, 0x02, 0x03, 0xff));
        assert_eq!(
            theme.candidate.selected_label_color,
            rgba(0x12, 0x34, 0x56, 0xff)
        );
        // #102030 is dark, so the accent blends with the dark weights.
        assert_eq!(theme.candidate.selected_background, rgba(17, 40, 64, 255));
    }

    #[test]
    fn preview_auto_scheme_uses_supplied_brightness() {
        for system_scheme in [EffectiveColorScheme::Light, EffectiveColorScheme::Dark] {
            let theme = resolve_candidate_panel_preview_from_paths(
                None,
                None,
                system_scheme,
                UiColorScheme::Auto,
                "#3B73D9",
                20.0,
                None,
                "",
            )
            .unwrap();
            assert_eq!(theme.ui.color_scheme, UiColorScheme::Auto);
            assert_eq!(theme.ui.effective_color_scheme, system_scheme);
            let expected = resolve_theme_from_paths_with_system_scheme(None, None, system_scheme);
            assert_eq!(theme.panel.background, expected.panel.background);
        }
    }

    #[test]
    fn scaled_candidate_panel_scales_metrics_without_reclamping() {
        let base = ResolvedImeTheme::default();
        assert_eq!(
            resolved_theme_json(&scaled_candidate_panel(&base)).unwrap(),
            resolved_theme_json(&base).unwrap()
        );
        for font_size in [10.0, 20.0, 36.0] {
            let mut theme = base.clone();
            theme.font.size = font_size;
            let scaled = scaled_candidate_panel(&theme);
            let scale = font_size / base.font.size;
            assert_eq!(scaled.font.size, font_size);
            for (actual, original) in [
                (scaled.font.label_size, base.font.label_size),
                (scaled.font.comment_size, base.font.comment_size),
                (scaled.font.preedit_size, base.font.preedit_size),
                (scaled.panel.padding_x, base.panel.padding_x),
                (scaled.panel.padding_y, base.panel.padding_y),
                (scaled.panel.padding_top, base.panel.padding_top),
                (scaled.panel.padding_right, base.panel.padding_right),
                (scaled.panel.padding_bottom, base.panel.padding_bottom),
                (scaled.panel.padding_left, base.panel.padding_left),
                (scaled.panel.gap, base.panel.gap),
                (scaled.panel.preedit_gap, base.panel.preedit_gap),
                (scaled.candidate.padding_x, base.candidate.padding_x),
                (scaled.candidate.padding_y, base.candidate.padding_y),
                (scaled.candidate.inline_gap, base.candidate.inline_gap),
                (scaled.candidate.min_height, base.candidate.min_height),
                (scaled.navigation.button_size, base.navigation.button_size),
            ] {
                assert_eq!(actual, original * scale);
            }
            assert_eq!(theme.candidate.min_height, base.candidate.min_height);
        }
    }

    #[test]
    fn preview_scales_only_candidate_metrics_without_reclamping() {
        let base = resolve_candidate_panel_preview_from_paths(
            None,
            None,
            EffectiveColorScheme::Light,
            UiColorScheme::Light,
            "#3B73D9",
            20.0,
            None,
            "",
        )
        .unwrap();
        let small = resolve_candidate_panel_preview_from_paths(
            None,
            None,
            EffectiveColorScheme::Light,
            UiColorScheme::Light,
            "#3B73D9",
            10.0,
            None,
            "",
        )
        .unwrap();
        assert_eq!(small.font.size, 10.0);
        assert_eq!(small.font.label_size, base.font.label_size * 0.5);
        assert_eq!(small.font.comment_size, base.font.comment_size * 0.5);
        assert_eq!(small.font.preedit_size, base.font.preedit_size * 0.5);
        assert_eq!(small.panel.padding_x, base.panel.padding_x * 0.5);
        assert_eq!(small.panel.padding_y, base.panel.padding_y * 0.5);
        assert_eq!(small.panel.gap, base.panel.gap * 0.5);
        assert_eq!(small.panel.preedit_gap, base.panel.preedit_gap * 0.5);
        assert_eq!(small.candidate.padding_x, base.candidate.padding_x * 0.5);
        assert_eq!(small.candidate.padding_y, base.candidate.padding_y * 0.5);
        assert_eq!(small.candidate.inline_gap, base.candidate.inline_gap * 0.5);
        assert_eq!(small.candidate.min_height, 17.0);
        assert_eq!(small.navigation.button_size, 14.0);
        assert_eq!(small.panel.corner_radius, base.panel.corner_radius);
        assert_eq!(small.panel.min_width, base.panel.min_width);
        assert_eq!(small.panel.max_width, base.panel.max_width);
        assert_eq!(small.candidate.corner_radius, base.candidate.corner_radius);
        assert_eq!(small.candidate.max_width, base.candidate.max_width);
        assert_eq!(
            small.navigation.corner_radius,
            base.navigation.corner_radius
        );
        assert_eq!(small.mode_hint.font_size, base.mode_hint.font_size);
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
        for (scheme, background, foreground) in [
            (
                EffectiveColorScheme::Light,
                rgba(0xf8, 0xfa, 0xff, 0xf2),
                rgba(0x26, 0x34, 0x42, 0xff),
            ),
            (
                EffectiveColorScheme::Dark,
                rgba(0x2d, 0x4b, 0x63, 0xff),
                rgba(0xff, 0xff, 0xff, 0xff),
            ),
        ] {
            let mut theme = resolve_theme_from_paths_with_system_scheme(None, None, scheme);
            theme.ui.accent_color = Some(rgba(0x12, 0x34, 0x56, 0xff));
            theme.apply_accent_color(rgba(0x12, 0x34, 0x56, 0xff));

            assert_eq!(
                theme.candidate.selected_label_color,
                rgba(0x12, 0x34, 0x56, 0xff)
            );
            assert_eq!(theme.mode_hint.background, background);
            assert_eq!(theme.mode_hint.foreground, foreground);
            assert_eq!(theme.mode_hint.border_color, rgba(0x12, 0x34, 0x56, 0xff));
        }
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
