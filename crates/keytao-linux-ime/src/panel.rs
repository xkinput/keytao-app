//! Candidate panel renderer.
//!
//! Renders the shared keytao-theme model to a raw BGRA pixel buffer suitable
//! for both X11 XCB image upload and Wayland wl_shm.

use std::{collections::HashSet, path::Path as StdPath, process::Command, time::Duration};

use freetype::{bitmap::PixelMode, face::LoadFlag, ffi, Face, Library};
use keytao_core::ImeState;
use keytao_theme::{
    CandidatePanelInput, PanelOrientation, RgbaColor, ThemeCandidate, ThemeResolver, UiCapabilities,
};
use tiny_skia::*;

const FONT_PROBE_SIZE: f32 = 22.0;
const COLOR_GLYPH_HEIGHT_FACTOR: f32 = 1.05;
const COLOR_GLYPH_WIDTH_FACTOR: f32 = 1.35;
const MAX_COLLECTION_FACES: isize = 32;
const TEXT_FONT_ENV: &str = "KEYTAO_IME_FONT";
const SYMBOL_FONT_ENV: &str = "KEYTAO_IME_SYMBOL_FONT";
const TEXT_FALLBACK_PATTERNS: &[&str] = &[
    "sans-serif:lang=zh:weight=medium",
    "system-ui:lang=zh:weight=medium",
    "ui-sans-serif:lang=zh:weight=medium",
    "Sarasa Gothic SC:lang=zh:weight=medium",
    "Source Han Sans SC:lang=zh:weight=medium",
    "Noto Sans CJK SC:lang=zh:weight=medium",
    "LXGW WenKai:lang=zh",
];
const SYMBOL_FALLBACK_PATTERNS: &[&str] =
    &["Noto Sans Symbols 2", "symbol", "emoji", "Noto Color Emoji"];

#[derive(Clone, Debug)]
pub struct FontSource {
    path: String,
    index: isize,
}

fn font_is_usable(face: &Face) -> bool {
    for sample in ['中', '候'] {
        if face.get_char_index(sample as usize).unwrap_or_default() == 0 {
            continue;
        }
        if face
            .set_pixel_sizes(0, FONT_PROBE_SIZE.ceil() as u32)
            .and_then(|_| {
                face.load_char(sample as usize, LoadFlag::RENDER | LoadFlag::TARGET_NORMAL)
            })
            .is_err()
        {
            continue;
        }

        let bitmap = face.glyph().bitmap();
        if bitmap.width() > 0 && bitmap.rows() > 0 && bitmap.buffer().iter().any(|px| *px != 0) {
            return true;
        }
    }
    false
}

fn font_has_any(face: &Face, samples: &[char]) -> bool {
    samples
        .iter()
        .any(|sample| face.get_char_index(*sample as usize).unwrap_or_default() != 0)
}

fn load_font_source_with_samples(
    path: &str,
    preferred_index: Option<isize>,
    samples: &[char],
) -> Option<FontSource> {
    let library = Library::init().ok()?;
    let mut indices = Vec::new();
    if let Some(index) = preferred_index {
        indices.push(index);
    }
    indices.extend(0..MAX_COLLECTION_FACES);
    indices.sort_unstable();
    indices.dedup();

    for index in indices {
        let Ok(face) = library.new_face(path, index) else {
            continue;
        };
        if font_has_any(&face, samples) {
            tracing::debug!("loaded font: {path} (collection index {index})");
            return Some(FontSource {
                path: path.to_string(),
                index,
            });
        }
    }

    tracing::debug!("rejected font without requested glyphs: {path}");
    None
}

fn load_font_source(path: &str, preferred_index: Option<isize>) -> Option<FontSource> {
    let source = load_font_source_with_samples(path, preferred_index, &['中', '候'])?;
    let library = Library::init().ok()?;
    let face = library.new_face(&source.path, source.index).ok()?;
    if font_is_usable(&face) {
        Some(source)
    } else {
        tracing::debug!("rejected font with empty glyph rasters: {path}");
        None
    }
}

fn fc_match(pattern: &str) -> Option<(String, Option<isize>)> {
    let out = std::process::Command::new("fc-match")
        .args(["--format=%{file}\n%{index}", pattern])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let output = String::from_utf8(out.stdout).ok()?;
    let mut lines = output.lines();
    let path = lines.next().unwrap_or_default().trim();
    if path.is_empty() {
        return None;
    }
    let index = lines
        .next()
        .and_then(|line| line.trim().parse::<isize>().ok());
    Some((path.to_string(), index))
}

fn fontconfig_pattern(value: &str, lang: Option<&str>) -> String {
    if value.contains(':') || lang.is_none() {
        value.to_string()
    } else {
        format!("{value}:lang={}", lang.unwrap())
    }
}

fn load_text_font_from_value(value: &str) -> Option<FontSource> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if StdPath::new(value).exists() {
        return load_font_source(value, None);
    }
    let pattern = fontconfig_pattern(value, Some("zh"));
    let (path, index) = fc_match(&pattern)?;
    load_font_source(&path, index)
}

fn load_symbol_font_from_value(value: &str, samples: &[char]) -> Option<FontSource> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if StdPath::new(value).exists() {
        return load_font_source_with_samples(value, None, samples);
    }
    let pattern = fontconfig_pattern(value, None);
    let (path, index) = fc_match(&pattern)?;
    load_font_source_with_samples(&path, index, samples)
}

fn font_values(value: &str) -> impl Iterator<Item = &str> {
    value
        .split([',', ';'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

// ── Font loader ───────────────────────────────────────────────────────────────

/// Load the first available CJK-capable font, trying common paths then fc-match.
pub fn load_font() -> Option<FontSource> {
    if let Ok(value) = std::env::var(TEXT_FONT_ENV) {
        for value in font_values(&value) {
            if let Some(font) = load_text_font_from_value(value) {
                return Some(font);
            }
        }
        tracing::warn!("{TEXT_FONT_ENV} did not resolve to any usable CJK font");
    }

    for pattern in TEXT_FALLBACK_PATTERNS {
        let Some((path, index)) = fc_match(pattern) else {
            continue;
        };
        if let Some(font) = load_font_source(&path, index) {
            return Some(font);
        }
    }

    const PATHS: &[&str] = &[
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJKsc-Regular.otf",
        "/usr/share/fonts/wqy-zenhei/wqy-zenhei.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
    ];
    for p in PATHS {
        if let Some(font) = load_font_source(p, None) {
            return Some(font);
        }
    }
    tracing::warn!("no CJK font found; candidate text may be incomplete");
    None
}

fn load_symbol_fonts() -> Vec<FontSource> {
    let mut sources = Vec::new();
    let mut seen = HashSet::new();
    let samples = ['🚫', '⚠', '✓', '✕', '〔', '〕'];

    if let Ok(value) = std::env::var(SYMBOL_FONT_ENV) {
        for value in font_values(&value) {
            let Some(source) = load_symbol_font_from_value(value, &samples) else {
                continue;
            };
            if seen.insert((source.path.clone(), source.index)) {
                sources.push(source);
            }
        }
        if sources.is_empty() {
            tracing::warn!("{SYMBOL_FONT_ENV} did not resolve to any usable symbol font");
        }
    }

    const PATHS: &[&str] = &[
        "/usr/share/fonts/noto/NotoSansSymbols2-Regular.otf",
        "/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.otf",
        "/usr/share/fonts/noto/NotoSansSymbols2-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf",
        "/usr/share/fonts/noto/NotoColorEmoji.ttf",
        "/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf",
        "/usr/share/fonts/noto/NotoEmoji-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoEmoji-Regular.ttf",
    ];

    for path in PATHS {
        if let Some(source) = load_font_source_with_samples(path, None, &samples) {
            let key = (source.path.clone(), source.index);
            if seen.insert(key) {
                sources.push(source);
            }
        }
    }

    for pattern in SYMBOL_FALLBACK_PATTERNS {
        let Some((path, preferred_index)) = fc_match(pattern) else {
            continue;
        };
        if let Some(source) = load_font_source_with_samples(&path, preferred_index, &samples) {
            let key = (source.path.clone(), source.index);
            if seen.insert(key) {
                sources.push(source);
            }
        }
    }

    sources
}

fn clamp_panel_scale(scale: f32) -> f32 {
    if scale.is_finite() {
        scale.clamp(0.75, 4.0)
    } else {
        1.0
    }
}

fn parse_scale_value(value: &str) -> Option<f32> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    value.parse::<f32>().ok().map(clamp_panel_scale)
}

fn xft_dpi_scale() -> Option<f32> {
    let out = Command::new("xrdb").arg("-query").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8(out.stdout).ok()?;
    for line in stdout.lines() {
        let Some(value) = line.strip_prefix("Xft.dpi:") else {
            continue;
        };
        let dpi = value.trim().parse::<f32>().ok()?;
        if dpi > 0.0 {
            return Some(clamp_panel_scale(dpi / 96.0));
        }
    }
    None
}

fn detect_explicit_panel_scale() -> Option<f32> {
    for key in [
        "KEYTAO_IME_PANEL_SCALE",
        "GDK_SCALE",
        "QT_SCALE_FACTOR",
        "QT_SCREEN_SCALE_FACTORS",
    ] {
        if let Ok(value) = std::env::var(key) {
            let first = value.split([';', ':']).next().unwrap_or_default();
            let scale_text = first.rsplit('=').next().unwrap_or(first);
            if let Some(scale) = parse_scale_value(scale_text) {
                return Some(scale);
            }
        }
    }
    None
}

fn detect_panel_scale() -> f32 {
    detect_explicit_panel_scale().unwrap_or(1.0)
}

fn detect_x11_panel_scale() -> f32 {
    detect_explicit_panel_scale()
        .or_else(xft_dpi_scale)
        .unwrap_or(1.0)
}

// ── Renderer ──────────────────────────────────────────────────────────────────

pub struct PanelRenderer {
    faces: Vec<Face>,
    _library: Library,
    scale: f32,
    theme_resolver: ThemeResolver,
}

impl PanelRenderer {
    pub fn new(source: FontSource) -> Option<Self> {
        Self::with_scale(source, detect_panel_scale())
    }

    pub fn new_x11(source: FontSource) -> Option<Self> {
        Self::with_scale(source, detect_x11_panel_scale())
    }

    fn with_scale(source: FontSource, scale: f32) -> Option<Self> {
        let library = Library::init().ok()?;
        let face = library.new_face(&source.path, source.index).ok()?;
        let mut faces = vec![face];
        for fallback in load_symbol_fonts() {
            if let Ok(face) = library.new_face(&fallback.path, fallback.index) {
                faces.push(face);
            }
        }
        Some(Self {
            faces,
            _library: library,
            scale: clamp_panel_scale(scale),
            theme_resolver: ThemeResolver::from_default_locations(),
        })
    }

    /// Render panel to a BGRA byte buffer.  Returns (bytes, width, height).
    pub fn render(&self, state: &ImeState) -> (Vec<u8>, u32, u32) {
        let theme = self.theme_resolver.current();
        let model = theme
            .candidate_panel_model(state_to_panel_input(state), &UiCapabilities::full_custom());
        let font_size = self.s(theme.font.size);
        let label_size = self.s(theme.font.label_size);
        let comment_size = self.s(theme.font.comment_size);
        let preedit_size = self.s(theme.font.preedit_size);
        let panel_pad_x = self.s(theme.panel.padding_x);
        let panel_pad_y = self.s(theme.panel.padding_y);
        let panel_gap = self.s(theme.panel.gap);
        let option_pad_x = self.s(theme.candidate.padding_x);
        let option_pad_y = self.s(theme.candidate.padding_y);
        let inline_gap = self.s(theme.candidate.inline_gap);
        let option_gap = panel_gap;
        let option_height = self
            .s(theme.candidate.min_height)
            .max(font_size.max(label_size).max(comment_size) + option_pad_y * 2.0);
        let nav_button = self.s(theme.navigation.button_size);
        let preedit_height = model
            .preedit
            .as_ref()
            .map(|_| preedit_size + panel_gap)
            .unwrap_or(0.0);
        let option_widths: Vec<f32> = model
            .candidates
            .iter()
            .map(|candidate| {
                option_pad_x * 2.0
                    + self.text_width(&candidate.label, label_size)
                    + inline_gap
                    + self.text_width(&candidate.text, font_size)
                    + candidate.comment.as_ref().map_or(0.0, |comment| {
                        inline_gap + self.text_width(comment, comment_size)
                    })
            })
            .collect();
        let nav_count = usize::from(model.navigation.can_go_previous)
            + usize::from(model.navigation.can_go_next);
        let nav_width = if nav_count == 0 {
            0.0
        } else {
            nav_count as f32 * nav_button + nav_count.saturating_sub(1) as f32 * panel_gap
        };
        let preedit_width = model
            .preedit
            .as_ref()
            .map(|preedit| self.text_width(preedit, preedit_size) + panel_pad_x * 2.0)
            .unwrap_or(0.0);

        let (content_width, content_height) = if model.orientation == PanelOrientation::Vertical {
            let max_option_width = option_widths.iter().copied().fold(0.0, f32::max);
            let candidate_height = model.candidates.len() as f32 * option_height
                + model.candidates.len().saturating_sub(1) as f32 * option_gap;
            (
                max_option_width.max(nav_width).max(preedit_width),
                preedit_height
                    + candidate_height
                    + if nav_count > 0 {
                        panel_gap + nav_button
                    } else {
                        0.0
                    },
            )
        } else {
            let candidate_width = option_widths.iter().sum::<f32>()
                + model.candidates.len().saturating_sub(1) as f32 * option_gap;
            let nav_extra = if nav_count > 0 {
                panel_gap + nav_width
            } else {
                0.0
            };
            (
                (candidate_width + nav_extra).max(preedit_width),
                preedit_height + option_height,
            )
        };
        let width = self
            .s(theme.panel.min_width)
            .max(content_width + panel_pad_x * 2.0)
            .max(preedit_width)
            .min(self.s(theme.panel.max_width))
            .ceil() as u32;
        let height = (content_height + panel_pad_y * 2.0)
            .min(self.s(theme.panel.max_height))
            .ceil()
            .max(1.0) as u32;

        let mut pm = Pixmap::new(width, height).expect("pixmap alloc");
        pm.fill(Color::from_rgba8(0, 0, 0, 0));
        draw_rounded_rect(
            &mut pm,
            0.5,
            0.5,
            width as f32 - 1.0,
            height as f32 - 1.0,
            self.s(theme.panel.corner_radius),
            theme.panel.background,
            theme.panel.border_color,
            self.s(theme.panel.border_width),
        );

        let mut y = panel_pad_y;
        if let Some(preedit) = model.preedit.as_ref() {
            let baseline = y + preedit_size;
            self.draw_text(
                &mut pm,
                preedit,
                panel_pad_x,
                baseline,
                bgra(theme.candidate.selected_label_color),
                preedit_size,
            );
            y += preedit_height;
        }

        if model.orientation == PanelOrientation::Vertical {
            let mut row_y = y;
            for (candidate, option_width) in model.candidates.iter().zip(option_widths.iter()) {
                self.draw_candidate_option(
                    &mut pm,
                    candidate,
                    panel_pad_x,
                    row_y,
                    *option_width,
                    option_height,
                    label_size,
                    font_size,
                    comment_size,
                    &theme,
                );
                row_y += option_height + option_gap;
            }
            if nav_count > 0 {
                self.draw_navigation_row(
                    &mut pm,
                    panel_pad_x,
                    row_y,
                    nav_button,
                    font_size,
                    &model.navigation,
                    &theme,
                );
            }
        } else {
            let mut x = panel_pad_x;
            for (candidate, option_width) in model.candidates.iter().zip(option_widths.iter()) {
                self.draw_candidate_option(
                    &mut pm,
                    candidate,
                    x,
                    y,
                    *option_width,
                    option_height,
                    label_size,
                    font_size,
                    comment_size,
                    &theme,
                );
                x += option_width + option_gap;
            }
            if nav_count > 0 {
                x += panel_gap;
                self.draw_navigation_row(
                    &mut pm,
                    x,
                    y,
                    nav_button,
                    font_size,
                    &model.navigation,
                    &theme,
                );
            }
        }

        // Convert RGBA (tiny-skia) → BGRA (platform native)
        let mut out: Vec<u8> = pm.data().to_vec();
        for px in out.chunks_exact_mut(4) {
            px.swap(0, 2); // R↔B
        }

        (out, width, height)
    }

    pub fn render_mode_hint(&self, ascii_mode: bool) -> (Vec<u8>, u32, u32) {
        let theme = self.theme_resolver.current();
        let model = theme.mode_hint_model(ascii_mode);
        let hint_size = self.s(theme.mode_hint.font_size);
        let hint_height = self.s(theme.mode_hint.height).ceil() as u32;
        let hint_min_width = self.s(theme.mode_hint.width).ceil() as u32;
        let label = model.text;
        let text_width = self.text_width(&label, hint_size);
        let width = hint_min_width.max((text_width + self.s(20.0) * 2.0).ceil() as u32);
        let mut pm = Pixmap::new(width, hint_height).expect("pixmap alloc");
        pm.fill(Color::from_rgba8(0, 0, 0, 0));

        if let Some(path) = rounded_rect_path(
            0.5,
            0.5,
            width as f32 - 1.0,
            hint_height as f32 - 1.0,
            self.s(theme.mode_hint.corner_radius),
        ) {
            let mut paint = Paint::default();
            paint.set_color(tiny_color(theme.mode_hint.background));
            paint.anti_alias = true;
            pm.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );

            if theme.panel.border_width > 0.0 {
                paint.set_color(tiny_color(theme.panel.border_color));
                let mut stroke = Stroke::default();
                stroke.width = self.s(1.0).max(1.0);
                pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
        }

        let x = (width as f32 - text_width) * 0.5;
        let baseline = (hint_height as f32 + hint_size) * 0.5 - self.s(3.0);
        self.draw_text(
            &mut pm,
            &label,
            x,
            baseline,
            bgra(theme.mode_hint.foreground),
            hint_size,
        );

        let mut out = pm.data().to_vec();
        for px in out.chunks_exact_mut(4) {
            px.swap(0, 2);
        }

        (out, width, hint_height)
    }

    pub fn mode_hint_duration(&self) -> Duration {
        let theme = self.theme_resolver.current();
        Duration::from_secs_f32(theme.mode_hint.duration)
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_candidate_option(
        &self,
        pm: &mut Pixmap,
        candidate: &keytao_theme::CandidateOptionModel,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        label_size: f32,
        font_size: f32,
        comment_size: f32,
        theme: &keytao_theme::ResolvedImeTheme,
    ) {
        let candidate_theme = &theme.candidate;
        let background = if candidate.selected {
            candidate_theme.selected_background
        } else {
            candidate_theme.background
        };
        let border = if candidate.selected {
            candidate_theme.selected_border_color
        } else {
            candidate_theme.border_color
        };
        let border_width = if candidate.selected {
            self.s(candidate_theme.border_width.max(1.0))
        } else {
            self.s(candidate_theme.border_width)
        };
        draw_rounded_rect(
            pm,
            x,
            y,
            width,
            height,
            self.s(candidate_theme.corner_radius),
            background,
            border,
            border_width,
        );

        let option_pad_x = self.s(candidate_theme.padding_x);
        let inline_gap = self.s(candidate_theme.inline_gap);
        let baseline = y + (height + font_size) * 0.5 - self.s(4.0);
        let mut text_x = x + option_pad_x;
        let label_color = if candidate.selected {
            candidate_theme.selected_label_color
        } else {
            candidate_theme.label_color
        };
        let text_color = if candidate.selected {
            candidate_theme.selected_foreground
        } else {
            candidate_theme.foreground
        };
        let comment_color = if candidate.selected {
            candidate_theme.selected_comment_color
        } else {
            candidate_theme.comment_color
        };
        self.draw_text(
            pm,
            &candidate.label,
            text_x,
            baseline,
            bgra(label_color),
            label_size,
        );
        text_x += self.text_width(&candidate.label, label_size) + inline_gap;
        self.draw_text(
            pm,
            &candidate.text,
            text_x,
            baseline,
            bgra(text_color),
            font_size,
        );
        text_x += self.text_width(&candidate.text, font_size);
        if let Some(comment) = candidate.comment.as_ref() {
            text_x += inline_gap;
            self.draw_text(
                pm,
                comment,
                text_x,
                baseline,
                bgra(comment_color),
                comment_size,
            );
        }
    }

    fn draw_navigation_row(
        &self,
        pm: &mut Pixmap,
        x: f32,
        y: f32,
        button_size: f32,
        font_size: f32,
        navigation: &keytao_theme::PageNavigationModel,
        theme: &keytao_theme::ResolvedImeTheme,
    ) {
        let mut nav_x = x;
        let baseline = y + (button_size + font_size) * 0.5 - self.s(4.0);
        if navigation.can_go_previous {
            self.draw_text(
                pm,
                "‹",
                nav_x + button_size * 0.35,
                baseline,
                bgra(theme.navigation.foreground),
                font_size,
            );
            nav_x += button_size + self.s(theme.panel.gap);
        }
        if navigation.can_go_next {
            self.draw_text(
                pm,
                "›",
                nav_x + button_size * 0.35,
                baseline,
                bgra(theme.navigation.foreground),
                font_size,
            );
        }
    }

    fn s(&self, value: f32) -> f32 {
        value * self.scale
    }

    fn draw_text(
        &self,
        pm: &mut Pixmap,
        text: &str,
        mut x: f32,
        baseline: f32,
        color: [u8; 4],
        size: f32,
    ) {
        for ch in text.chars() {
            if is_zero_width_selector(ch) {
                continue;
            }
            let Some(face) = self.face_for_char(ch) else {
                x += size * 0.5;
                continue;
            };
            let color_face = face.has_color();
            let load_flags = if color_face {
                LoadFlag::RENDER | LoadFlag::COLOR
            } else {
                LoadFlag::TARGET_NORMAL
            };
            if face.set_pixel_sizes(0, size.ceil() as u32).is_err() {
                let _ = face.select_size(0);
            }
            if face.load_char(ch as usize, load_flags).is_err() {
                x += size * 0.5;
                continue;
            }

            if !color_face {
                let slot = face.glyph().raw() as *const _ as ffi::FT_GlyphSlot;
                unsafe {
                    ffi::FT_GlyphSlot_Embolden(slot);
                    if ffi::FT_Render_Glyph(slot, ffi::FT_RENDER_MODE_NORMAL) != 0 {
                        x += size * 0.5;
                        continue;
                    }
                }
            }

            let glyph = face.glyph();
            let bitmap = glyph.bitmap();
            let width = bitmap.width().max(0) as usize;
            let rows = bitmap.rows().max(0) as usize;
            let pitch = bitmap.pitch().unsigned_abs() as usize;
            let buffer = bitmap.buffer();
            let pixel_mode = bitmap.pixel_mode().ok();
            if width == 0 || rows == 0 || buffer.is_empty() {
                x += (glyph.advance().x as f32 / 64.0).max(size * 0.35);
                continue;
            }
            let color_glyph = pixel_mode == Some(PixelMode::Bgra);
            let scale = if color_glyph && width > 0 && rows > 0 {
                let max_height = size * COLOR_GLYPH_HEIGHT_FACTOR;
                let max_width = size * COLOR_GLYPH_WIDTH_FACTOR;
                (max_height / rows as f32)
                    .min(max_width / width as f32)
                    .min(1.0)
            } else {
                1.0
            };
            let draw_width = ((width as f32 * scale).ceil() as usize).max(1);
            let draw_rows = ((rows as f32 * scale).ceil() as usize).max(1);
            let gx = (x + glyph.bitmap_left() as f32 * scale) as i32;
            let gy = (baseline - glyph.bitmap_top() as f32 * scale) as i32;

            for row in 0..draw_rows {
                let scaled_row = (row as f32 / scale).floor() as usize;
                let source_row = if bitmap.pitch() >= 0 {
                    scaled_row.min(rows - 1)
                } else {
                    rows - scaled_row.min(rows - 1) - 1
                };
                for col in 0..draw_width {
                    let source_col = (col as f32 / scale).floor() as usize;
                    let source_col = source_col.min(width - 1);
                    let (r, g, b, alpha) = match pixel_mode {
                        Some(PixelMode::Bgra) => {
                            let offset = source_row * pitch + source_col * 4;
                            if offset + 3 >= buffer.len() {
                                continue;
                            }
                            (
                                buffer[offset + 2],
                                buffer[offset + 1],
                                buffer[offset],
                                buffer[offset + 3],
                            )
                        }
                        _ => {
                            let offset = source_row * pitch + source_col;
                            if offset >= buffer.len() {
                                continue;
                            }
                            let alpha = buffer[offset];
                            (color[2], color[1], color[0], alpha)
                        }
                    };
                    if alpha == 0 {
                        continue;
                    }
                    let px = gx + col as i32;
                    let py = gy + row as i32;
                    if px < 0 || py < 0 || px >= pm.width() as i32 || py >= pm.height() as i32 {
                        continue;
                    }
                    blend_pixel(pm, px, py, r, g, b, alpha as f32 / 255.0);
                }
            }
            x += if color_glyph {
                draw_width as f32 + size * 0.1
            } else {
                (glyph.advance().x as f32 / 64.0).max(size * 0.35)
            };
        }
    }

    fn text_width(&self, text: &str, size: f32) -> f32 {
        text.chars()
            .map(|c| {
                if is_zero_width_selector(c) {
                    return 0.0;
                }
                let Some(face) = self.face_for_char(c) else {
                    return size * 0.5;
                };
                if face.set_pixel_sizes(0, size.ceil() as u32).is_err() {
                    let _ = face.select_size(0);
                }
                let load_flags = if face.has_color() {
                    LoadFlag::RENDER | LoadFlag::COLOR
                } else {
                    LoadFlag::DEFAULT
                };
                if face.load_char(c as usize, load_flags).is_ok() {
                    let glyph = face.glyph();
                    let bitmap = glyph.bitmap();
                    if bitmap.pixel_mode().ok() == Some(PixelMode::Bgra)
                        && bitmap.width() > 0
                        && bitmap.rows() > 0
                    {
                        let width = bitmap.width() as f32;
                        let rows = bitmap.rows() as f32;
                        let scale = (size * COLOR_GLYPH_HEIGHT_FACTOR / rows)
                            .min(size * COLOR_GLYPH_WIDTH_FACTOR / width)
                            .min(1.0);
                        (width * scale).ceil() + size * 0.1
                    } else {
                        (glyph.advance().x as f32 / 64.0).max(size * 0.35)
                    }
                } else {
                    size * 0.5
                }
            })
            .sum()
    }

    fn face_for_char(&self, ch: char) -> Option<&Face> {
        self.faces
            .iter()
            .find(|face| face.get_char_index(ch as usize).unwrap_or_default() != 0)
    }
}

fn is_zero_width_selector(ch: char) -> bool {
    matches!(ch, '\u{fe0e}' | '\u{fe0f}' | '\u{200d}')
}

fn state_to_panel_input(state: &ImeState) -> CandidatePanelInput {
    CandidatePanelInput {
        preedit: state.preedit.clone(),
        candidates: state
            .candidates
            .iter()
            .map(|candidate| ThemeCandidate {
                text: candidate.text.clone(),
                comment: candidate.comment.clone(),
            })
            .collect(),
        highlighted_candidate_index: state.highlighted_candidate_index,
        page: state.page,
        is_last_page: state.is_last_page,
        select_keys: state.select_keys.clone(),
    }
}

fn bgra(color: RgbaColor) -> [u8; 4] {
    [color.blue, color.green, color.red, color.alpha]
}

fn tiny_color(color: RgbaColor) -> Color {
    Color::from_rgba8(color.red, color.green, color.blue, color.alpha)
}

fn draw_rounded_rect(
    pm: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    radius: f32,
    fill: RgbaColor,
    border: RgbaColor,
    border_width: f32,
) {
    let Some(path) = rounded_rect_path(x, y, width.max(1.0), height.max(1.0), radius) else {
        return;
    };
    let mut paint = Paint::default();
    paint.anti_alias = true;
    if fill.alpha > 0 {
        paint.set_color(tiny_color(fill));
        pm.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    if border_width > 0.0 && border.alpha > 0 {
        paint.set_color(tiny_color(border));
        let mut stroke = Stroke::default();
        stroke.width = border_width.max(1.0);
        pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

fn blend_pixel(pm: &mut Pixmap, px: i32, py: i32, r: u8, g: u8, b: u8, alpha: f32) {
    if alpha <= 0.0 || px < 0 || py < 0 || px >= pm.width() as i32 || py >= pm.height() as i32 {
        return;
    }
    let off = (py as usize * pm.width() as usize + px as usize) * 4;
    let d = pm.data_mut();
    let a = alpha.min(1.0);
    d[off] = lerp(d[off], r, a);
    d[off + 1] = lerp(d[off + 1], g, a);
    d[off + 2] = lerp(d[off + 2], b, a);
    d[off + 3] = 255;
}

fn rounded_rect_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let r = r.min(w * 0.5).min(h * 0.5);
    let k = r * 0.5523;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

#[inline]
fn lerp(bg: u8, fg: u8, a: f32) -> u8 {
    (bg as f32 * (1.0 - a) + fg as f32 * a) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use keytao_core::Candidate;

    #[test]
    fn candidate_text_renders_visible_pixels() {
        let Some(source) = load_font() else {
            eprintln!("skipping panel render test: no CJK font found");
            return;
        };
        let Some(renderer) = PanelRenderer::new(source) else {
            eprintln!("skipping panel render test: font source could not be reopened");
            return;
        };

        let mut state = ImeState::empty();
        state.candidates = vec![Candidate {
            text: "候选".to_string(),
            comment: None,
        }];

        let (pixels, _, _) = renderer.render(&state);
        let visible_colors = pixels
            .chunks_exact(4)
            .filter(|pixel| pixel[3] > 0)
            .copied()
            .collect::<HashSet<_>>()
            .len();

        assert!(
            visible_colors > 2,
            "candidate panel text rendered no visible pixels"
        );
    }

    #[test]
    fn candidate_comment_expands_panel_width() {
        let Some(source) = load_font() else {
            eprintln!("skipping comment render test: no CJK font found");
            return;
        };
        let Some(renderer) = PanelRenderer::new(source) else {
            eprintln!("skipping comment render test: font source could not be reopened");
            return;
        };

        let mut base = ImeState::empty();
        base.candidates = vec![Candidate {
            text: "这".to_string(),
            comment: None,
        }];

        let mut with_comment = base.clone();
        with_comment.candidates[0].comment = Some("abcdefghijklmnopqrstuvwxyz".to_string());

        let (_, base_width, _) = renderer.render(&base);
        let (_, comment_width, _) = renderer.render(&with_comment);

        assert!(
            comment_width > base_width,
            "candidate comment did not affect rendered panel width"
        );
    }

    #[test]
    fn emoji_hint_has_fallback_font() {
        let Some(source) = load_font() else {
            eprintln!("skipping emoji fallback test: no CJK font found");
            return;
        };
        let Some(renderer) = PanelRenderer::new(source) else {
            eprintln!("skipping emoji fallback test: font source could not be reopened");
            return;
        };

        assert!(
            renderer.face_for_char('🚫').is_some(),
            "emoji hint has no fallback font"
        );
    }

    #[test]
    fn mode_hint_renders_visible_pixels() {
        let Some(source) = load_font() else {
            eprintln!("skipping mode hint render test: no CJK font found");
            return;
        };
        let Some(renderer) = PanelRenderer::new(source) else {
            eprintln!("skipping mode hint render test: font source could not be reopened");
            return;
        };

        let (pixels, _, _) = renderer.render_mode_hint(false);
        let visible_text_pixels = pixels.chunks_exact(4).filter(|pixel| pixel[3] != 0).count();

        assert!(
            visible_text_pixels > 0,
            "mode hint rendered no visible pixels"
        );
    }

    #[test]
    fn explicit_panel_scale_expands_candidate_panel() {
        let Some(source) = load_font() else {
            eprintln!("skipping panel scale test: no CJK font found");
            return;
        };
        let Some(normal) = PanelRenderer::with_scale(source.clone(), 1.0) else {
            eprintln!("skipping panel scale test: font source could not be reopened");
            return;
        };
        let Some(scaled) = PanelRenderer::with_scale(source, 2.0) else {
            eprintln!("skipping panel scale test: font source could not be reopened");
            return;
        };

        let mut state = ImeState::empty();
        state.preedit = "fan".to_string();
        state.candidates = vec![Candidate {
            text: "这".to_string(),
            comment: Some("~a".to_string()),
        }];

        let (_, normal_width, normal_height) = normal.render(&state);
        let (_, scaled_width, scaled_height) = scaled.render(&state);

        assert!(scaled_width > normal_width);
        assert!(scaled_height > normal_height);
    }
}
