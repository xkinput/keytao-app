//! Shared KeyTao IME theme language.
//!
//! This crate owns the cross-platform theme schema, default values, merge rules,
//! and view models. Platform frontends render the resolved model with their own
//! native UI stack.

use serde::{Deserialize, Deserializer, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

pub const THEME_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_THEME_YAML: &str = include_str!("../default-theme.yaml");

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
    pub font: FontTheme,
    pub panel: PanelTheme,
    pub candidate: CandidateTheme,
    pub navigation: NavigationTheme,
    pub mode_hint: ModeHintTheme,
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
        Self {
            default_theme_path,
            user_theme_path,
            cache: Mutex::new(ThemeCache::default()),
        }
    }

    pub fn from_default_locations() -> Self {
        Self::new(None, default_user_theme_path())
    }

    pub fn current(&self) -> ResolvedImeTheme {
        let signature = self.signature();
        let Ok(mut cache) = self.cache.lock() else {
            return resolve_theme_from_paths(
                self.default_theme_path.as_deref(),
                self.user_theme_path.as_deref(),
            );
        };
        if cache.signature == signature {
            return cache.theme.clone();
        }
        let theme = resolve_theme_from_paths(
            self.default_theme_path.as_deref(),
            self.user_theme_path.as_deref(),
        );
        cache.signature = signature;
        cache.theme = theme.clone();
        theme
    }

    fn signature(&self) -> String {
        [
            self.default_theme_path.as_deref(),
            self.user_theme_path.as_deref(),
        ]
        .into_iter()
        .flatten()
        .map(path_signature)
        .collect::<Vec<_>>()
        .join("|")
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

pub fn resolve_theme_from_paths(
    default_theme_path: Option<&Path>,
    user_theme_path: Option<&Path>,
) -> ResolvedImeTheme {
    let mut theme = ResolvedImeTheme::default();
    if let Ok(partial) = serde_yaml::from_str::<PartialTheme>(DEFAULT_THEME_YAML) {
        theme.apply(partial);
    }
    for path in [default_theme_path, user_theme_path].into_iter().flatten() {
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        let Ok(partial) = serde_yaml::from_str::<PartialTheme>(&content) else {
            continue;
        };
        theme.apply(partial);
    }
    theme.sanitized()
}

pub fn resolved_theme_json(theme: &ResolvedImeTheme) -> Result<String, serde_json::Error> {
    serde_json::to_string(theme)
}

impl ResolvedImeTheme {
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

    fn sanitized(mut self) -> Self {
        self.version = THEME_SCHEMA_VERSION;
        self.font.size = clamp(self.font.size, 10.0, 36.0);
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
        Self {
            version: THEME_SCHEMA_VERSION,
            font: FontTheme {
                family: None,
                size: 18.0,
                label_size: 14.0,
                comment_size: 13.0,
                preedit_size: 15.0,
                weight: FontWeight::Medium,
            },
            panel: PanelTheme {
                orientation: PanelOrientation::Horizontal,
                background: rgba(0xF8, 0xFA, 0xFF, 0xF2),
                border_color: rgba(0xD8, 0xE2, 0xF1, 0xFF),
                border_width: 1.0,
                corner_radius: 14.0,
                padding_x: 8.0,
                padding_y: 7.0,
                gap: 6.0,
                min_width: 96.0,
                max_width: 820.0,
                max_height: 460.0,
                screen_margin: 8.0,
                shadow: true,
            },
            candidate: CandidateTheme {
                background: rgba(0, 0, 0, 0),
                hover_background: rgba(0xF1, 0xF6, 0xFF, 0xFF),
                selected_background: rgba(0xE6, 0xF0, 0xFF, 0xFF),
                foreground: rgba(0x1F, 0x29, 0x33, 0xFF),
                selected_foreground: rgba(0x14, 0x23, 0x3B, 0xFF),
                label_color: rgba(0x6B, 0x77, 0x85, 0xFF),
                selected_label_color: rgba(0x3B, 0x73, 0xD9, 0xFF),
                comment_color: rgba(0x7A, 0x87, 0x90, 0xFF),
                selected_comment_color: rgba(0x52, 0x6A, 0x91, 0xFF),
                border_color: rgba(0, 0, 0, 0),
                selected_border_color: rgba(0xA8, 0xC7, 0xFA, 0xFF),
                border_width: 0.0,
                corner_radius: 9.0,
                padding_x: 11.0,
                padding_y: 6.0,
                inline_gap: 5.0,
                min_height: 32.0,
                max_width: 210.0,
                separator_visible: false,
                separator_color: rgba(0xDC, 0xE7, 0xF7, 0xFF),
                label_suffix: ".".to_string(),
            },
            navigation: NavigationTheme {
                foreground: rgba(0x4A, 0x59, 0x66, 0xFF),
                disabled_foreground: rgba(0xA5, 0xB0, 0xB8, 0xFF),
                hover_background: rgba(0xF1, 0xF6, 0xFF, 0xFF),
                button_size: 28.0,
                corner_radius: 8.0,
            },
            mode_hint: ModeHintTheme {
                background: rgba(0xE6, 0xF0, 0xFF, 0xF2),
                foreground: rgba(0x2F, 0x5F, 0xB8, 0xFF),
                font_size: 24.0,
                width: 72.0,
                height: 48.0,
                corner_radius: 14.0,
                duration: 0.75,
                shadow: true,
                chinese_text: "中".to_string(),
                english_text: "英".to_string(),
            },
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartialTheme {
    version: Option<u32>,
    font: Option<PartialFontTheme>,
    panel: Option<PartialPanelTheme>,
    candidate: Option<PartialCandidateTheme>,
    navigation: Option<PartialNavigationTheme>,
    #[serde(alias = "mode_hint")]
    mode_hint: Option<PartialModeHintTheme>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartialFontTheme {
    family: Option<String>,
    size: Option<f32>,
    label_size: Option<f32>,
    comment_size: Option<f32>,
    preedit_size: Option<f32>,
    weight: Option<FontWeight>,
}

#[derive(Default, Deserialize)]
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

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartialCandidateTheme {
    #[serde(default, deserialize_with = "optional_color")]
    background: Option<RgbaColor>,
    #[serde(default, deserialize_with = "optional_color")]
    hover_background: Option<RgbaColor>,
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

#[derive(Default, Deserialize)]
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

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartialModeHintTheme {
    #[serde(default, deserialize_with = "optional_color")]
    background: Option<RgbaColor>,
    #[serde(default, deserialize_with = "optional_color")]
    foreground: Option<RgbaColor>,
    font_size: Option<f32>,
    width: Option<f32>,
    height: Option<f32>,
    corner_radius: Option<f32>,
    duration: Option<f32>,
    shadow: Option<bool>,
    chinese_text: Option<String>,
    english_text: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_theme_yaml_resolves() {
        let theme = resolve_theme_from_paths(None, None);
        assert_eq!(theme.version, THEME_SCHEMA_VERSION);
        assert_eq!(theme.panel.orientation, PanelOrientation::Horizontal);
        assert_eq!(theme.candidate.label_suffix, ".");
    }

    #[test]
    fn user_overlay_merges_and_clamps() {
        let mut theme = ResolvedImeTheme::default();
        let partial = serde_yaml::from_str::<PartialTheme>(
            "font:\n  size: 99\npanel:\n  orientation: vertical\ncandidate:\n  selectedBackground: '#11223344'\n",
        )
        .unwrap();
        theme.apply(partial);
        let theme = theme.sanitized();
        assert_eq!(theme.font.size, 36.0);
        assert_eq!(theme.panel.orientation, PanelOrientation::Vertical);
        assert_eq!(
            theme.candidate.selected_background,
            rgba(0x11, 0x22, 0x33, 0x44)
        );
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
