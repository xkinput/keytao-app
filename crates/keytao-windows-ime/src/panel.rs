//! Candidate panel renderer — direct port of keytao-linux-ime/src/panel.rs.
//!
//! Renders to a raw BGRA pixel buffer.  On Windows this buffer is fed to
//! UpdateLayeredWindow; on Linux to wl_shm / XCB image upload.
//!
use keytao_core::ImeState;
use keytao_theme::{
    panel_hit::{layout, PanelClick, PanelGeometry, PanelHitAreas},
    preedit_caret_color, resolved_panel_shadow, scaled_candidate_panel, CandidatePanelInput,
    CandidatePanelModel, CommentPosition, PanelOrientation, ResolvedImeTheme, RgbaColor,
    ThemeCandidate, ThemeResolver, UiCapabilities,
};
use std::{
    cell::RefCell,
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};
use tiny_skia::*;

use crate::globals::dll_related_dirs;

// ── Font loader ───────────────────────────────────────────────────────────────

const FONT_PROBE_SIZE: f32 = 24.0;

pub struct FontSet {
    family: String,
    fonts: Vec<Arc<fontdue::Font>>,
}

#[cfg(test)]
fn font_with_samples(
    data: &[u8],
    collection_index: u32,
    samples: &[char],
) -> Option<fontdue::Font> {
    let font = fontdue::Font::from_bytes(
        data,
        fontdue::FontSettings {
            collection_index,
            ..Default::default()
        },
    )
    .ok()?;
    has_visible_sample(&font, samples).then_some(font)
}

fn has_visible_sample(font: &fontdue::Font, samples: &[char]) -> bool {
    samples.iter().any(|sample| {
        if !font.has_glyph(*sample) {
            return false;
        }
        let (metrics, bitmap) = font.rasterize(*sample, FONT_PROBE_SIZE);
        metrics.width > 0 && metrics.height > 0 && bitmap.iter().any(|alpha| *alpha != 0)
    })
}

fn load_font_with_samples(path: &str, samples: &[char]) -> Option<fontdue::Font> {
    let data = std::fs::read(path).ok()?;
    for collection_index in 0..16 {
        let Ok(font) = fontdue::Font::from_bytes(
            data.as_slice(),
            fontdue::FontSettings {
                collection_index,
                ..Default::default()
            },
        ) else {
            if collection_index == 0 {
                tracing::debug!("rejected unreadable font: {path}");
            }
            break;
        };
        if has_visible_sample(&font, samples) {
            tracing::info!("loaded font: {path} (collection index {collection_index})");
            return Some(font);
        }
    }
    None
}

fn font_database() -> &'static fontdb::Database {
    keytao_theme::font_resolution::system_font_database()
}

/// Share the active theme's faces and the default CJK/emoji fallbacks process-wide.
pub fn load_font(family: Option<&str>) -> Option<Arc<FontSet>> {
    static DEFAULT_FONTS: OnceLock<FontSet> = OnceLock::new();
    static FONTS: Mutex<Option<Arc<FontSet>>> = Mutex::new(None);
    let family = family.unwrap_or("").trim();
    let mut cached = FONTS.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(fonts) = cached.as_ref().filter(|fonts| fonts.family == family) {
        return Some(Arc::clone(fonts));
    }
    let mut fonts = Vec::new();
    for name in family
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        let database = font_database();
        let Some(id) = database.query(&fontdb::Query {
            families: &[fontdb::Family::Name(name)],
            ..Default::default()
        }) else {
            continue;
        };
        if let Some(font) = database
            .with_face_data(id, |data, index| {
                fontdue::Font::from_bytes(
                    data,
                    fontdue::FontSettings {
                        collection_index: index,
                        ..Default::default()
                    },
                )
                .ok()
            })
            .flatten()
        {
            fonts.push(Arc::new(font));
        }
    }
    if DEFAULT_FONTS.get().is_none() {
        if let Some(defaults) = load_font_set() {
            let _ = DEFAULT_FONTS.set(defaults);
        }
    }
    if let Some(defaults) = DEFAULT_FONTS.get() {
        fonts.extend(defaults.fonts.iter().cloned());
    }
    if fonts.is_empty() {
        return None;
    }
    let fonts = Arc::new(FontSet {
        family: family.to_owned(),
        fonts,
    });
    *cached = Some(Arc::clone(&fonts));
    Some(fonts)
}

fn load_font_set() -> Option<FontSet> {
    const TEXT_PATHS: &[&str] = &[
        r"C:\Windows\Fonts\msyh.ttc", // Microsoft YaHei
        r"C:\Windows\Fonts\msyhbd.ttc",
        r"C:\Windows\Fonts\simsun.ttc",   // SimSun
        r"C:\Windows\Fonts\simhei.ttf",   // SimHei
        r"C:\Windows\Fonts\STZHONGS.TTF", // STZhongSong
        r"C:\Windows\Fonts\NotoSansCJK-Regular.ttc",
    ];

    const FALLBACK_PATHS: &[&str] = &[
        r"C:\Windows\Fonts\seguiemj.ttf", // Segoe UI Emoji
        r"C:\Windows\Fonts\seguisym.ttf", // Segoe UI Symbol
        r"C:\Windows\Fonts\segoeui.ttf",  // Segoe UI
    ];

    let mut fonts = Vec::new();
    for path in TEXT_PATHS {
        if let Some(font) = load_font_with_samples(path, &['中', '候']) {
            fonts.push(Arc::new(font));
            break;
        }
    }
    if fonts.is_empty() {
        tracing::warn!("no CJK font found; candidate text may be blank");
        return None;
    }

    let fallback_samples = ['🚫', '😀', '⚠', '♥', '✓'];
    for path in FALLBACK_PATHS {
        if let Some(font) = load_font_with_samples(path, &fallback_samples) {
            fonts.push(Arc::new(font));
        }
    }
    if fonts.len() == 1 {
        tracing::warn!("no Windows emoji or symbol fallback font found");
    }

    Some(FontSet {
        family: String::new(),
        fonts,
    })
}

// ── Renderer ──────────────────────────────────────────────────────────────────

type RasterizedGlyph = (fontdue::Metrics, Arc<[u8]>);

pub struct PanelRenderer {
    fonts: Arc<FontSet>,
    glyph_cache: RefCell<VecDeque<((char, u32), Option<RasterizedGlyph>)>>,
}

pub struct RenderedPanel {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub shadow_inset: u32,
    pub hit_areas: PanelHitAreas,
}

impl RenderedPanel {
    fn empty() -> Self {
        Self {
            pixels: Vec::new(),
            width: 0,
            height: 0,
            shadow_inset: 0,
            hit_areas: PanelHitAreas::default(),
        }
    }
}

impl PanelRenderer {
    pub fn new(fonts: Arc<FontSet>) -> Self {
        Self {
            fonts,
            glyph_cache: RefCell::new(VecDeque::new()),
        }
    }

    pub fn font_family(&self) -> &str {
        &self.fonts.family
    }

    /// Render panel to a premultiplied BGRA byte buffer plus the hit areas the
    /// window needs for mouse selection.
    pub fn render(
        &self,
        model: &CandidatePanelModel,
        scale: f32,
        theme: ResolvedImeTheme,
        available_width: f32,
        preedit_caret: Option<usize>,
    ) -> RenderedPanel {
        self.render_with_pointer(
            model,
            scale,
            theme,
            available_width,
            preedit_caret,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_with_pointer(
        &self,
        model: &CandidatePanelModel,
        scale: f32,
        theme: ResolvedImeTheme,
        available_width: f32,
        preedit_caret: Option<usize>,
        hovered: Option<PanelClick>,
        pressed: Option<PanelClick>,
    ) -> RenderedPanel {
        let mut theme = scaled_candidate_panel(&theme);
        let scale = scale_candidate_ui_metrics(&mut theme, scale);
        let font_size = theme.font.size;
        let label_size = theme.font.label_size;
        let comment_size = theme.font.comment_size;
        let preedit_size = theme.font.preedit_size;
        let panel_pad_left = theme.panel.padding_left;
        let panel_pad_right = theme.panel.padding_right;
        let panel_pad_top = theme.panel.padding_top;
        let panel_pad_bottom = theme.panel.padding_bottom;
        let panel_gap = theme.panel.gap;
        let option_pad_x = theme.candidate.padding_x;
        let option_pad_y = theme.candidate.padding_y;
        let inline_gap = theme.candidate.inline_gap;
        let comment_below = theme.candidate.comment_position == CommentPosition::Below;
        let single_line_height = if comment_below {
            font_size.max(label_size)
        } else {
            font_size.max(label_size).max(comment_size)
        };
        let option_height = theme
            .candidate
            .min_height
            .max(single_line_height + option_pad_y * 2.0);
        let option_heights: Option<Vec<_>> = comment_below.then(|| {
            let annotated_height = theme
                .candidate
                .min_height
                .max(single_line_height + inline_gap * 0.5 + comment_size + option_pad_y * 2.0);
            model
                .candidates
                .iter()
                .map(|candidate| {
                    if candidate.comment.is_some() {
                        annotated_height
                    } else {
                        option_height
                    }
                })
                .collect()
        });
        let option_height = option_heights.as_ref().map_or(option_height, |heights| {
            heights.iter().copied().fold(option_height, f32::max)
        });
        let nav_button = theme.navigation.button_size;
        let preedit_height = model
            .preedit
            .as_ref()
            .map(|_| preedit_size + theme.panel.preedit_gap)
            .unwrap_or(0.0);
        let option_widths: Vec<f32> = model
            .candidates
            .iter()
            .map(|candidate| {
                let text_width = self.text_width(&candidate.text, font_size);
                let word_width = candidate.comment.as_ref().map_or(text_width, |comment| {
                    let comment_width = self.text_width(comment, comment_size);
                    if comment_below {
                        text_width.max(comment_width)
                    } else {
                        text_width + inline_gap + comment_width
                    }
                });
                option_pad_x * 2.0
                    + self.text_width(&candidate.label, label_size)
                    + inline_gap
                    + word_width
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
            .map(|preedit| {
                self.text_width(preedit, preedit_size) + panel_pad_left + panel_pad_right
            })
            .unwrap_or(0.0);
        let (content_width, content_height) = if model.orientation == PanelOrientation::Vertical {
            let max_option_width = option_widths.iter().copied().fold(0.0, f32::max);
            let candidate_height = option_heights
                .as_ref()
                .map_or(model.candidates.len() as f32 * option_height, |heights| {
                    heights.iter().sum()
                })
                + model.candidates.len().saturating_sub(1) as f32 * panel_gap;
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
                + model.candidates.len().saturating_sub(1) as f32 * panel_gap;
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
        let width = theme
            .panel
            .min_width
            .max(content_width + panel_pad_left + panel_pad_right)
            .max(preedit_width)
            .ceil()
            .min(
                (available_width
                    - resolved_panel_shadow(theme.panel.shadow)
                        .map_or(0.0, |shadow| shadow.margin * scale * 2.0))
                .floor()
                .max(1.0),
            ) as u32;
        let height = (content_height + panel_pad_top + panel_pad_bottom)
            .min(theme.panel.max_height)
            .ceil()
            .max(1.0) as u32;

        let Some(mut pm) = Pixmap::new(width, height) else {
            tracing::warn!("candidate panel: failed to allocate pixmap {width}x{height}");
            return RenderedPanel::empty();
        };
        pm.fill(Color::from_rgba8(0, 0, 0, 0));
        draw_rounded_rect(
            &mut pm,
            0.5,
            0.5,
            width as f32 - 1.0,
            height as f32 - 1.0,
            theme.panel.corner_radius,
            theme.panel.background,
            theme.panel.border_color,
            theme.panel.border_width,
        );

        let mut y = panel_pad_top;
        if let Some(preedit) = model.preedit.as_ref() {
            self.draw_text_with_caret(
                &mut pm,
                preedit,
                panel_pad_left,
                y + preedit_size,
                bgra(theme.candidate.foreground),
                preedit_size,
                preedit_caret
                    .map(|index| (index, bgra(preedit_caret_color(theme.candidate.foreground)))),
            );
            y += preedit_height;
        }

        let mut hit_areas = layout(&PanelGeometry {
            vertical: model.orientation == PanelOrientation::Vertical,
            pad_x: panel_pad_left,
            top: y,
            gap: panel_gap,
            option_widths: &option_widths,
            option_height,
            option_heights: option_heights.as_deref(),
            nav_button,
            can_go_previous: model.navigation.can_go_previous,
            can_go_next: model.navigation.can_go_next,
        });
        for (index, (candidate, rect)) in model
            .candidates
            .iter()
            .zip(hit_areas.candidates.iter())
            .enumerate()
        {
            self.draw_candidate_option(
                &mut pm,
                candidate,
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                label_size,
                font_size,
                comment_size,
                scale,
                &theme,
                hovered == Some(PanelClick::Candidate(index)),
                pressed == Some(PanelClick::Candidate(index)),
            );
            if theme.candidate.separator_visible && index + 1 < model.candidates.len() {
                let (x, y, width, height) = if model.orientation == PanelOrientation::Vertical {
                    (
                        rect.x,
                        rect.y + rect.height + panel_gap * 0.5,
                        rect.width,
                        scale,
                    )
                } else {
                    (
                        rect.x + rect.width + panel_gap * 0.5,
                        rect.y,
                        scale,
                        rect.height,
                    )
                };
                draw_rounded_rect(
                    &mut pm,
                    x,
                    y,
                    width,
                    height,
                    0.0,
                    theme.candidate.separator_color,
                    theme.candidate.separator_color,
                    0.0,
                );
            }
        }
        for (rect, glyph, target) in [
            (hit_areas.previous_page, "‹", PanelClick::PreviousPage),
            (hit_areas.next_page, "›", PanelClick::NextPage),
        ] {
            if let Some(rect) = rect {
                if hovered == Some(target) {
                    draw_rounded_rect(
                        &mut pm,
                        rect.x,
                        rect.y,
                        rect.width,
                        rect.height,
                        theme.navigation.corner_radius,
                        theme.navigation.hover_background,
                        theme.navigation.hover_background,
                        0.0,
                    );
                }
                self.draw_text(
                    &mut pm,
                    glyph,
                    rect.x + rect.width * 0.35,
                    rect.y + (rect.height + font_size) * 0.5 - 3.0 * scale,
                    bgra(theme.navigation.foreground),
                    font_size,
                );
            }
        }
        let (pm, inset) =
            with_panel_shadow(pm, theme.panel.corner_radius, theme.panel.shadow, scale);
        let (width, height) = (pm.width(), pm.height());
        // Preserve Windows' integer-pixel hit bounds after drawing at subpixel positions.
        for rect in hit_areas
            .candidates
            .iter_mut()
            .chain(hit_areas.previous_page.iter_mut())
            .chain(hit_areas.next_page.iter_mut())
        {
            rect.x += inset as f32;
            rect.y += inset as f32;
            rect.x = rect.x.floor() as i32 as f32;
            rect.y = rect.y.floor() as i32 as f32;
            rect.width = rect.width.ceil() as i32 as f32;
            rect.height = rect.height.ceil() as i32 as f32;
        }

        // Convert tiny-skia RGBA → BGRA (platform convention for both Windows and Linux)
        let mut out: Vec<u8> = pm.data().to_vec();
        for px in out.chunks_exact_mut(4) {
            px.swap(0, 2); // R ↔ B
        }

        RenderedPanel {
            pixels: out,
            width,
            height,
            shadow_inset: inset,
            hit_areas,
        }
    }

    pub fn render_mode_hint(
        &self,
        ascii_mode: bool,
        scale: f32,
        mut theme: ResolvedImeTheme,
    ) -> RenderedPanel {
        scale_candidate_ui_metrics(&mut theme, scale);
        let model = theme.mode_hint_model(ascii_mode);
        let font_size = theme.mode_hint.font_size;
        let padding = font_size / 3.0;
        let label = model.text;
        let (text_left, text_bottom, text_right, text_top) = self.text_bounds(&label, font_size);
        let text_width = text_right - text_left;
        let text_height = text_top - text_bottom;
        let width = theme
            .mode_hint
            .width
            .max(text_width + padding * 2.0)
            .ceil()
            .max(1.0) as u32;
        let height = theme
            .mode_hint
            .height
            .max(text_height + padding * 2.0)
            .ceil()
            .max(1.0) as u32;

        let Some(mut pm) = Pixmap::new(width, height) else {
            tracing::warn!("mode hint: failed to allocate pixmap {width}x{height}");
            return RenderedPanel::empty();
        };
        pm.fill(Color::from_rgba8(0, 0, 0, 0));
        draw_rounded_rect(
            &mut pm,
            0.5,
            0.5,
            width as f32 - 1.0,
            height as f32 - 1.0,
            theme.mode_hint.corner_radius.min(height as f32 / 2.0),
            theme.mode_hint.background,
            theme.mode_hint.border_color,
            theme.mode_hint.border_width,
        );

        let x = (width as f32 - text_width) * 0.5 - text_left;
        let baseline = (height as f32 - text_height) * 0.5 + text_top;
        self.draw_text(
            &mut pm,
            &label,
            x,
            baseline,
            bgra(theme.mode_hint.foreground),
            font_size,
        );

        let (pm, shadow_inset) = with_panel_shadow(
            pm,
            theme.mode_hint.corner_radius,
            theme.mode_hint.shadow,
            scale,
        );
        let (width, height) = (pm.width(), pm.height());
        let mut out: Vec<u8> = pm.data().to_vec();
        for px in out.chunks_exact_mut(4) {
            px.swap(0, 2);
        }

        RenderedPanel {
            pixels: out,
            width,
            height,
            shadow_inset,
            hit_areas: PanelHitAreas::default(),
        }
    }

    pub fn mode_hint_duration_ms(theme: &ResolvedImeTheme) -> u32 {
        (theme.mode_hint.duration * 1000.0)
            .round()
            .clamp(150.0, 4000.0) as u32
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
        scale: f32,
        theme: &keytao_theme::ResolvedImeTheme,
        hovered: bool,
        pressed: bool,
    ) {
        let option = &theme.candidate;
        let background = if pressed {
            option.pressed_background
        } else if candidate.selected {
            option.selected_background
        } else if hovered {
            option.hover_background
        } else {
            option.background
        };
        let border = if candidate.selected {
            option.selected_border_color
        } else {
            option.border_color
        };
        draw_rounded_rect(
            pm,
            x,
            y,
            width,
            height,
            option.corner_radius,
            background,
            border,
            option.border_width,
        );

        let label_x = x + option.padding_x;
        let text_x = label_x + self.text_width(&candidate.label, label_size) + option.inline_gap;
        let comment_below =
            option.comment_position == CommentPosition::Below && candidate.comment.is_some();
        let comment_gap = option.inline_gap * 0.5;
        let baseline = if comment_below {
            let word_row_height = font_size.max(label_size);
            let content_height = word_row_height + comment_gap + comment_size;
            y + (height - content_height) * 0.5 + word_row_height - 3.0 * scale
        } else {
            y + (height + font_size) * 0.5 - 3.0 * scale
        };
        let label_color = if candidate.selected {
            option.selected_label_color
        } else {
            option.label_color
        };
        let text_color = if pressed {
            option.pressed_foreground
        } else if candidate.selected {
            option.selected_foreground
        } else {
            option.foreground
        };
        let comment_color = if candidate.selected {
            option.selected_comment_color
        } else {
            option.comment_color
        };
        self.draw_text(
            pm,
            &candidate.label,
            label_x,
            baseline,
            bgra(label_color),
            label_size,
        );
        self.draw_text(
            pm,
            &candidate.text,
            text_x,
            baseline,
            bgra(text_color),
            font_size,
        );
        if let Some(comment) = candidate.comment.as_ref() {
            let (comment_x, comment_baseline) = if comment_below {
                (text_x, baseline + comment_gap + comment_size)
            } else {
                (
                    text_x + self.text_width(&candidate.text, font_size) + option.inline_gap,
                    baseline,
                )
            };
            self.draw_text(
                pm,
                comment,
                comment_x,
                comment_baseline,
                bgra(comment_color),
                comment_size,
            );
        }
    }

    fn draw_text(
        &self,
        pm: &mut Pixmap,
        text: &str,
        x: f32,
        baseline: f32,
        color: [u8; 4],
        size: f32,
    ) {
        self.draw_text_with_caret(pm, text, x, baseline, color, size, None);
    }

    fn draw_text_with_caret(
        &self,
        pm: &mut Pixmap,
        text: &str,
        mut x: f32,
        baseline: f32,
        color: [u8; 4],
        size: f32,
        caret: Option<(usize, [u8; 4])>,
    ) {
        for (index, ch) in text.chars().enumerate() {
            if is_zero_width_emoji_component(ch) {
                continue;
            }
            let Some((metrics, bitmap)) = self.rasterize_char(ch, size) else {
                x += size * 0.5;
                continue;
            };
            let gx = (x + metrics.xmin as f32) as i32;
            let gy = (baseline - metrics.height as f32 - metrics.ymin as f32) as i32;
            let color = match caret {
                Some((caret_index, caret_color)) if caret_index == index => caret_color,
                _ => color,
            };
            for row in 0..metrics.height {
                for col in 0..metrics.width {
                    let alpha = bitmap[row * metrics.width + col];
                    if alpha == 0 {
                        continue;
                    }
                    let px = gx + col as i32;
                    let py = gy + row as i32;
                    if px < 0 || py < 0 || px >= pm.width() as i32 || py >= pm.height() as i32 {
                        continue;
                    }
                    let a = alpha as f32 / 255.0 * color[3] as f32 / 255.0;
                    let off = (py as usize * pm.width() as usize + px as usize) * 4;
                    let d = pm.data_mut();
                    d[off] = lerp(d[off], color[2], a); // R
                    d[off + 1] = lerp(d[off + 1], color[1], a); // G
                    d[off + 2] = lerp(d[off + 2], color[0], a); // B
                    d[off + 3] = (255.0 * a + d[off + 3] as f32 * (1.0 - a)).round() as u8;
                }
            }
            x += metrics.advance_width;
        }
    }

    fn text_width(&self, text: &str, size: f32) -> f32 {
        text.chars()
            .map(|ch| {
                if is_zero_width_emoji_component(ch) {
                    0.0
                } else {
                    self.rasterize_char(ch, size)
                        .map(|(metrics, _)| metrics.advance_width)
                        .unwrap_or(size * 0.5)
                }
            })
            .sum()
    }

    fn text_bounds(&self, text: &str, size: f32) -> (f32, f32, f32, f32) {
        let mut advance = 0.0;
        let mut left = 0.0_f32;
        let mut right = 0.0_f32;
        let mut bottom = f32::INFINITY;
        let mut top = f32::NEG_INFINITY;
        for ch in text.chars() {
            if is_zero_width_emoji_component(ch) {
                continue;
            }
            if let Some((metrics, _)) = self.rasterize_char(ch, size) {
                if metrics.width > 0 && metrics.height > 0 {
                    left = left.min(advance + metrics.xmin as f32);
                    right = right.max(advance + metrics.xmin as f32 + metrics.width as f32);
                    bottom = bottom.min(metrics.ymin as f32);
                    top = top.max(metrics.ymin as f32 + metrics.height as f32);
                }
                advance += metrics.advance_width;
            } else {
                advance += size * 0.5;
            }
        }
        if !bottom.is_finite() {
            bottom = 0.0;
            top = size;
        }
        (left, bottom, right.max(advance), top)
    }

    fn rasterize_char(&self, ch: char, size: f32) -> Option<RasterizedGlyph> {
        let key = (ch, size.to_bits());
        let mut cache = self.glyph_cache.borrow_mut();
        if let Some(index) = cache.iter().position(|(cached_key, _)| *cached_key == key) {
            let entry = cache.remove(index).unwrap();
            let glyph = entry.1.clone();
            cache.push_back(entry);
            return glyph;
        }
        let glyph = self.fonts.fonts.iter().find_map(|font| {
            if !font.has_glyph(ch) {
                return None;
            }
            let (metrics, bitmap) = font.rasterize(ch, size);
            if ch.is_whitespace()
                || (metrics.width > 0
                    && metrics.height > 0
                    && bitmap.iter().any(|alpha| *alpha != 0))
            {
                return Some((metrics, Arc::from(bitmap)));
            }
            None
        });
        if cache.len() == 512 {
            cache.pop_front();
        }
        cache.push_back((key, glyph.clone()));
        glyph
    }
}

// fontdue does not shape emoji sequences; isolated modifiers render as missing-glyph boxes.
fn is_zero_width_emoji_component(ch: char) -> bool {
    matches!(
        ch,
        '\u{fe0e}' | '\u{fe0f}' | '\u{200d}' | '\u{1f3fb}'..='\u{1f3ff}'
    )
}

fn scale_candidate_ui_metrics(theme: &mut ResolvedImeTheme, scale: f32) -> f32 {
    let scale = scale.clamp(0.5, 3.0);

    theme.font.size *= scale;
    theme.font.label_size *= scale;
    theme.font.comment_size *= scale;
    theme.font.preedit_size *= scale;

    theme.panel.border_width *= scale;
    theme.panel.corner_radius *= scale;
    theme.panel.padding_x *= scale;
    theme.panel.padding_y *= scale;
    theme.panel.padding_top *= scale;
    theme.panel.padding_right *= scale;
    theme.panel.padding_bottom *= scale;
    theme.panel.padding_left *= scale;
    theme.panel.gap *= scale;
    theme.panel.preedit_gap *= scale;
    theme.panel.min_width *= scale;
    theme.panel.max_height *= scale;
    theme.panel.screen_margin *= scale;

    theme.candidate.border_width *= scale;
    theme.candidate.corner_radius *= scale;
    theme.candidate.padding_x *= scale;
    theme.candidate.padding_y *= scale;
    theme.candidate.inline_gap *= scale;
    theme.candidate.min_height *= scale;

    theme.navigation.button_size *= scale;
    theme.navigation.corner_radius *= scale;

    theme.mode_hint.border_width *= scale;
    theme.mode_hint.corner_radius *= scale;
    theme.mode_hint.font_size *= scale;
    theme.mode_hint.width *= scale;
    theme.mode_hint.height *= scale;

    scale
}

fn windows_bundled_default_theme_path() -> Option<PathBuf> {
    for base in dll_related_dirs() {
        for candidate in [
            base.join("default-theme.yaml"),
            base.join("theme.yaml"),
            base.join("resources").join("default-theme.yaml"),
            base.join("resources").join("theme.yaml"),
            base.join("share").join("keytao").join("default-theme.yaml"),
        ] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

pub(crate) fn windows_theme_resolver() -> ThemeResolver {
    ThemeResolver::new(
        windows_bundled_default_theme_path().or_else(keytao_theme::default_bundled_theme_path),
        keytao_theme::default_user_theme_path(),
    )
}

#[inline]
fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round() as u8
}

pub(crate) fn candidate_panel_model(
    state: &ImeState,
    embedded: bool,
    theme: &ResolvedImeTheme,
) -> CandidatePanelModel {
    theme.candidate_panel_model(
        state_to_panel_input(state, embedded),
        &UiCapabilities::full_custom(),
    )
}

fn state_to_panel_input(state: &ImeState, embedded: bool) -> CandidatePanelInput {
    CandidatePanelInput {
        preedit: if embedded {
            String::new()
        } else {
            preedit_with_caret(state)
        },
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

/// `ImeState::cursor` counts Unicode scalar values.
fn preedit_with_caret(state: &ImeState) -> String {
    let mut text = state.preedit.clone();
    let byte = state
        .preedit
        .char_indices()
        .nth(state.cursor)
        .map_or(text.len(), |(index, _)| index);
    text.insert(byte, '|');
    text
}

fn bgra(color: RgbaColor) -> [u8; 4] {
    [color.blue, color.green, color.red, color.alpha]
}

fn tiny_color(color: RgbaColor) -> Color {
    Color::from_rgba8(color.red, color.green, color.blue, color.alpha)
}

fn with_panel_shadow(body: Pixmap, radius: f32, enabled: bool, scale: f32) -> (Pixmap, u32) {
    let Some(shadow) = resolved_panel_shadow(enabled) else {
        return (body, 0);
    };
    let inset = (shadow.margin * scale).ceil() as u32;
    let Some(mut result) = Pixmap::new(body.width() + inset * 2, body.height() + inset * 2) else {
        return (body, 0);
    };
    let half_width = body.width() as f32 * 0.5;
    let half_height = body.height() as f32 * 0.5;
    let radius = radius.min(half_width).min(half_height);
    let sigma = shadow.blur_radius * scale * 0.5;
    let width = result.width() as usize;
    for (index, pixel) in result.data_mut().chunks_exact_mut(4).enumerate() {
        let dx =
            ((index % width) as f32 + 0.5 - inset as f32 - half_width).abs() - half_width + radius;
        let dy =
            ((index / width) as f32 + 0.5 - inset as f32 - half_height - shadow.offset_y * scale)
                .abs()
                - half_height
                + radius;
        let distance = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - radius;
        let coverage = if distance <= 0.0 {
            1.0
        } else {
            (-0.5 * (distance / sigma).powi(2)).exp()
        };
        let alpha = shadow.color.alpha as f32 * coverage / 255.0;
        pixel.copy_from_slice(&[
            (shadow.color.red as f32 * alpha).round() as u8,
            (shadow.color.green as f32 * alpha).round() as u8,
            (shadow.color.blue as f32 * alpha).round() as u8,
            (255.0 * alpha).round() as u8,
        ]);
    }
    result.draw_pixmap(
        inset as i32,
        inset as i32,
        body.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
    (result, inset)
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
    let stroke_width = if border_width > 0.0 && border.alpha > 0 {
        border_width
    } else {
        0.0
    };
    let stroke_inset = ((stroke_width - 1.0) * 0.5).max(0.0);
    let path_x = x + stroke_inset;
    let path_y = y + stroke_inset;
    let path_width = (width - stroke_inset * 2.0).max(1.0);
    let path_height = (height - stroke_inset * 2.0).max(1.0);
    let path_radius = (radius - stroke_inset).max(0.0);
    let Some(path) = rounded_rect_path(path_x, path_y, path_width, path_height, path_radius) else {
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
    if stroke_width > 0.0 {
        paint.set_color(tiny_color(border));
        let mut stroke = Stroke::default();
        stroke.width = stroke_width;
        pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{
        is_zero_width_emoji_component, load_font, state_to_panel_input, FontSet, PanelRenderer,
    };
    use keytao_core::ImeState;
    use keytao_theme::{
        CandidatePanelInput, CommentPosition, PanelOrientation, ResolvedImeTheme, ThemeCandidate,
        UiCapabilities,
    };

    #[test]
    fn below_comments_use_their_own_line_and_fit_the_candidate_hit_area() {
        let renderer = PanelRenderer::new(Arc::new(FontSet {
            family: String::new(),
            fonts: Vec::new(),
        }));
        for orientation in [PanelOrientation::Horizontal, PanelOrientation::Vertical] {
            let mut theme = ResolvedImeTheme::default();
            theme.panel.orientation = orientation;
            theme.panel.shadow = false;
            let mut model = theme.candidate_panel_model(
                CandidatePanelInput {
                    preedit: String::new(),
                    candidates: vec![
                        ThemeCandidate {
                            text: "word".into(),
                            comment: Some("reverse lookup".into()),
                        },
                        ThemeCandidate {
                            text: "word".into(),
                            comment: None,
                        },
                    ],
                    highlighted_candidate_index: 0,
                    page: 0,
                    is_last_page: true,
                    select_keys: None,
                },
                &UiCapabilities::full_custom(),
            );
            let inline = renderer.render(&model, 1.0, theme.clone(), 1920.0, None);
            theme.candidate.comment_position = CommentPosition::Below;
            let stacked = renderer.render(&model, 1.0, theme.clone(), 1920.0, None);
            let first = stacked.hit_areas.candidates[0];
            let second = stacked.hit_areas.candidates[1];
            assert!(first.width < inline.hit_areas.candidates[0].width);
            assert!(first.height > inline.hit_areas.candidates[0].height);
            assert_eq!(
                stacked.hit_areas.click_at(
                    (first.x + 1.0).into(),
                    (first.y + first.height - 1.0).into()
                ),
                Some(keytao_theme::panel_hit::PanelClick::Candidate(0))
            );
            if orientation == PanelOrientation::Vertical {
                assert_eq!(second.height, inline.hit_areas.candidates[1].height);
                assert!(second.y > inline.hit_areas.candidates[1].y);
            } else {
                assert_eq!(second.height, first.height);
            }
            model.candidates[0].comment = None;
            let unannotated = renderer.render(&model, 1.0, theme, 1920.0, None);
            assert_eq!(
                unannotated.hit_areas.candidates[0].height,
                inline.hit_areas.candidates[0].height
            );
        }
    }

    #[test]
    fn long_candidates_and_comments_are_limited_only_by_available_width() {
        let renderer = PanelRenderer::new(Arc::new(FontSet {
            family: String::new(),
            fonts: Vec::new(),
        }));
        for orientation in [PanelOrientation::Horizontal, PanelOrientation::Vertical] {
            let mut theme = ResolvedImeTheme::default();
            theme.panel.orientation = orientation;
            for candidate in [
                ThemeCandidate {
                    text: "long candidate ".repeat(6),
                    comment: None,
                },
                ThemeCandidate {
                    text: "word".into(),
                    comment: Some("reverse_lookup_code ".repeat(6)),
                },
            ] {
                let model = theme.candidate_panel_model(
                    CandidatePanelInput {
                        preedit: String::new(),
                        candidates: vec![candidate],
                        highlighted_candidate_index: 0,
                        page: 0,
                        is_last_page: true,
                        select_keys: None,
                    },
                    &UiCapabilities::full_custom(),
                );
                for scale in [1.0, 2.0] {
                    let panel = renderer.render(&model, scale, theme.clone(), 1920.0, None);
                    assert!(panel.width > 320);
                    assert_eq!(
                        panel.pixels.len(),
                        (panel.width * panel.height * 4) as usize
                    );
                    let offset = ((panel.height / 2 * panel.width + 321) * 4) as usize;
                    assert!(panel.pixels[offset + 3] > 0);
                    let constrained = renderer.render(&model, scale, theme.clone(), 480.0, None);
                    assert_eq!(constrained.width, 480);
                }
            }
        }
    }

    #[test]
    fn candidate_hit_areas_scale_with_font_size_and_dpi_in_both_orientations() {
        let renderer = PanelRenderer::new(Arc::new(FontSet {
            family: String::new(),
            fonts: Vec::new(),
        }));
        let default_theme = ResolvedImeTheme::default();
        for orientation in [PanelOrientation::Horizontal, PanelOrientation::Vertical] {
            let mut theme = default_theme.clone();
            theme.panel.shadow = false;
            theme.panel.orientation = orientation;
            let model = theme.candidate_panel_model(
                CandidatePanelInput {
                    preedit: "ni".into(),
                    candidates: vec![
                        ThemeCandidate {
                            text: "one".into(),
                            comment: Some("first".into()),
                        },
                        ThemeCandidate {
                            text: "two".into(),
                            comment: Some("second".into()),
                        },
                    ],
                    highlighted_candidate_index: 0,
                    page: 1,
                    is_last_page: false,
                    select_keys: None,
                },
                &UiCapabilities::full_custom(),
            );
            let baseline = renderer.render(&model, 1.0, theme.clone(), 1920.0, None);
            for font_size in [10.0, 20.0, 36.0] {
                for dpi_scale in [1.0, 2.0] {
                    theme.font.size = font_size;
                    let rendered = renderer.render(&model, dpi_scale, theme.clone(), 1920.0, None);
                    let ratio = font_size / default_theme.font.size * dpi_scale;
                    let original = baseline
                        .hit_areas
                        .candidates
                        .iter()
                        .chain(baseline.hit_areas.previous_page.iter())
                        .chain(baseline.hit_areas.next_page.iter());
                    let resized = rendered
                        .hit_areas
                        .candidates
                        .iter()
                        .chain(rendered.hit_areas.previous_page.iter())
                        .chain(rendered.hit_areas.next_page.iter());
                    for (original, resized) in original.zip(resized) {
                        for (expected, actual) in [
                            (original.x, resized.x),
                            (original.y, resized.y),
                            (original.width, resized.width),
                            (original.height, resized.height),
                        ] {
                            assert!((actual as f32 - expected as f32 * ratio).abs() <= 1.0);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn appearance_file_updates_reach_panel_pixels_without_engine_transport() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("theme.yaml");
        let resolver = keytao_theme::ThemeResolver::with_system_scheme(
            None,
            Some(path.clone()),
            Some(keytao_theme::EffectiveColorScheme::Light),
        );
        let renderer = PanelRenderer::new(Arc::new(FontSet {
            family: String::new(),
            fonts: Vec::new(),
        }));
        let render = || {
            let mut theme = resolver.current();
            theme.panel.shadow = false;
            let model = theme.candidate_panel_model(
                CandidatePanelInput {
                    preedit: String::new(),
                    candidates: Vec::new(),
                    highlighted_candidate_index: 0,
                    page: 0,
                    is_last_page: true,
                    select_keys: None,
                },
                &UiCapabilities::full_custom(),
            );
            renderer.render(&model, 1.0, theme, 1920.0, None)
        };
        let rounded = render();
        keytao_theme::write_ime_ui_settings_to_path(
            &path,
            keytao_theme::UiColorScheme::Light,
            PanelOrientation::Horizontal,
            Some("#4A8DF6".into()),
            20.0,
            Some(0.0),
            "#2468AC".into(),
        )
        .unwrap();
        let square = render();
        assert_eq!(
            (rounded.width, rounded.height),
            (square.width, square.height)
        );
        let pixel = |panel: &super::RenderedPanel, x: u32, y: u32| {
            let offset = ((y * panel.width + x) * 4) as usize;
            <[u8; 4]>::try_from(&panel.pixels[offset..offset + 4]).unwrap()
        };
        assert_eq!(pixel(&rounded, 2, 2)[3], 0);
        assert!(pixel(&square, 2, 2)[3] > 0);
        let background = pixel(&square, square.width / 2, 3);
        assert_eq!(background[3], 0xF2);
        for (actual, channel) in background[..3].iter().zip([0xACu16, 0x68, 0x24]) {
            let expected = (channel * 0xF2 + 127) / 255;
            assert!((*actual as i32 - expected as i32).abs() <= 1);
        }
    }

    #[test]
    fn mode_hint_uses_resolved_font_and_box_fields() {
        let renderer = PanelRenderer::new(Arc::new(FontSet {
            family: String::new(),
            fonts: Vec::new(),
        }));
        let mut theme = ResolvedImeTheme::default();
        theme.mode_hint.english_text = "English".into();
        theme.mode_hint.font_size = 26.0;
        theme.mode_hint.width = 96.0;
        theme.mode_hint.height = 60.0;
        theme.mode_hint.shadow = false;
        for font_size in [10.0, 20.0, 36.0] {
            for dpi_scale in [1.0, 2.0] {
                theme.font.size = font_size;
                for ascii_mode in [false, true] {
                    let rendered = renderer.render_mode_hint(ascii_mode, dpi_scale, theme.clone());
                    let (width, height) = (rendered.width, rendered.height);
                    let font_size = theme.mode_hint.font_size * dpi_scale;
                    let label = theme.mode_hint_model(ascii_mode).text;
                    let text_width = label.chars().count() as f32 * font_size * 0.5;
                    assert_eq!(
                        width,
                        (theme.mode_hint.width * dpi_scale)
                            .max(text_width + font_size / 3.0 * 2.0)
                            .ceil() as u32
                    );
                    assert_eq!(
                        height,
                        (theme.mode_hint.height * dpi_scale)
                            .max(font_size + font_size / 3.0 * 2.0)
                            .ceil() as u32
                    );
                }
            }
        }
    }

    #[test]
    fn candidate_panel_does_not_duplicate_inline_preedit() {
        let mut state = ImeState::empty();
        state.preedit = "f".to_owned();

        assert!(state_to_panel_input(&state, true).preedit.is_empty());
    }

    #[test]
    fn panel_shows_preedit_when_not_embedded() {
        let mut state = ImeState::empty();
        state.preedit = "ni".to_owned();
        state.cursor = 1;

        assert_eq!(state_to_panel_input(&state, false).preedit, "n|i");
    }

    #[test]
    fn preedit_caret_lands_at_end_when_cursor_past_len() {
        let mut state = ImeState::empty();
        state.preedit = "ni".to_owned();
        state.cursor = 10;

        assert_eq!(state_to_panel_input(&state, false).preedit, "ni|");
    }

    #[test]
    fn preedit_caret_mutes_only_its_glyph_without_moving_letters() {
        let renderer = PanelRenderer::new(load_font(None).expect("load Windows candidate fonts"));
        for (background, foreground) in [
            ([248, 250, 255, 255], [74, 141, 246, 255]),
            ([23, 26, 32, 255], [182, 227, 255, 255]),
        ] {
            for (text, caret_index) in [
                ("|ab", 0),
                ("a||b", 1),
                ("a||b", 2),
                ("ab|", 2),
                ("中|文", 1),
                ("a😀|b", 2),
            ] {
                let mut original = tiny_skia::Pixmap::new(200, 40).unwrap();
                original.fill(tiny_skia::Color::from_rgba8(
                    background[0],
                    background[1],
                    background[2],
                    background[3],
                ));
                let mut muted = original.clone();
                let mut mask = original.clone();
                let caret_color = keytao_theme::preedit_caret_color(keytao_theme::RgbaColor {
                    red: foreground[0],
                    green: foreground[1],
                    blue: foreground[2],
                    alpha: foreground[3],
                });
                renderer.draw_text(&mut original, text, 4.0, 30.0, foreground, 24.0);
                renderer.draw_text_with_caret(
                    &mut muted,
                    text,
                    4.0,
                    30.0,
                    foreground,
                    24.0,
                    Some((
                        caret_index,
                        [
                            caret_color.red,
                            caret_color.green,
                            caret_color.blue,
                            caret_color.alpha,
                        ],
                    )),
                );
                let prefix: String = text.chars().take(caret_index).collect();
                renderer.draw_text(
                    &mut mask,
                    "|",
                    4.0 + renderer.text_width(&prefix, 24.0),
                    30.0,
                    foreground,
                    24.0,
                );
                let mut changed = 0;
                for ((original, muted), mask) in original
                    .data()
                    .chunks_exact(4)
                    .zip(muted.data().chunks_exact(4))
                    .zip(mask.data().chunks_exact(4))
                {
                    assert_eq!(original[3], muted[3]);
                    if mask == background {
                        assert_eq!(original, muted, "letters moved or changed color: {text}");
                    } else {
                        changed += usize::from(original != muted);
                        for channel in 0..3 {
                            let expected = super::lerp(
                                background[channel],
                                original[channel],
                                caret_color.alpha as f32 / 255.0,
                            );
                            assert!((muted[channel] as i16 - expected as i16).abs() <= 1);
                        }
                    }
                }
                assert!(changed > 0, "caret was not muted: {text}");
            }
        }
    }

    #[test]
    fn windows_system_fonts_cover_common_emoji_and_symbols() {
        let fonts = load_font(None).expect("load Windows candidate fonts");
        let renderer = PanelRenderer::new(fonts);

        for ch in ['🚫', '😀', '⚠', '♥'] {
            let (metrics, bitmap) = renderer
                .rasterize_char(ch, 24.0)
                .unwrap_or_else(|| panic!("missing visible glyph for {ch}"));
            assert!(metrics.width > 0 && metrics.height > 0);
            assert!(bitmap.iter().any(|alpha| *alpha != 0));
        }
    }

    #[test]
    fn theme_family_selects_its_collection_face_and_shares_default_fallbacks() {
        let defaults = load_font(None).expect("load Windows candidate fonts");
        let database = super::font_database();
        let mut checked = 0;
        for family in ["SimSun", "宋体", "NSimSun", "新宋体", "SimHei", "黑体"] {
            let Some(id) = database.query(&fontdb::Query {
                families: &[fontdb::Family::Name(family)],
                ..Default::default()
            }) else {
                continue;
            };
            let expected = database
                .with_face_data(id, |data, index| {
                    super::font_with_samples(data, index, &['中', '候'])
                })
                .flatten()
                .expect("load selected Windows face");
            let requested = format!("Missing KeyTao Font, {family}");
            let selected = load_font(Some(&requested)).expect("load theme font family");
            assert_eq!(selected.family, requested);
            assert_eq!(selected.fonts.len(), defaults.fonts.len() + 1);
            assert_eq!(
                selected.fonts[0].rasterize('中', 24.0),
                expected.rasterize('中', 24.0)
            );
            for (selected, default) in selected.fonts[1..].iter().zip(&defaults.fonts) {
                assert!(Arc::ptr_eq(selected, default));
            }
            checked += 1;
        }
        assert!(checked > 0, "no Windows CJK test family installed");
        let missing = load_font(Some("Missing KeyTao Font")).expect("fallback to default fonts");
        assert_eq!(missing.fonts.len(), defaults.fonts.len());
        for (missing, default) in missing.fonts.iter().zip(&defaults.fonts) {
            assert!(Arc::ptr_eq(missing, default));
        }
    }

    #[test]
    fn unshaped_emoji_components_do_not_consume_layout_width() {
        for ch in [
            '\u{fe0e}',
            '\u{fe0f}',
            '\u{200d}',
            '\u{1f3fb}',
            '\u{1f3fc}',
            '\u{1f3fd}',
            '\u{1f3fe}',
            '\u{1f3ff}',
        ] {
            assert!(is_zero_width_emoji_component(ch));
        }
        assert!(!is_zero_width_emoji_component('👍'));
    }
}
