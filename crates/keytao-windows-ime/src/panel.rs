//! Candidate panel renderer — direct port of keytao-linux-ime/src/panel.rs.
//!
//! Renders to a raw BGRA pixel buffer.  On Windows this buffer is fed to
//! UpdateLayeredWindow; on Linux to wl_shm / XCB image upload.
//!
use keytao_core::ImeState;
use keytao_theme::{
    CandidatePanelInput, PanelOrientation, RgbaColor, ThemeCandidate, ThemeResolver, UiCapabilities,
};
use tiny_skia::*;

// ── Font loader ───────────────────────────────────────────────────────────────

/// Load the first available CJK-capable font from common Windows paths.
pub fn load_font() -> Option<fontdue::Font> {
    const PATHS: &[&str] = &[
        r"C:\Windows\Fonts\msyh.ttc", // Microsoft YaHei
        r"C:\Windows\Fonts\msyhbd.ttc",
        r"C:\Windows\Fonts\simsun.ttc",   // SimSun
        r"C:\Windows\Fonts\simhei.ttf",   // SimHei
        r"C:\Windows\Fonts\STZHONGS.TTF", // STZhongSong
        r"C:\Windows\Fonts\NotoSansCJK-Regular.ttc",
    ];
    for p in PATHS {
        if let Ok(data) = std::fs::read(p) {
            if let Ok(f) =
                fontdue::Font::from_bytes(data.as_slice(), fontdue::FontSettings::default())
            {
                tracing::info!("loaded font: {p}");
                return Some(f);
            }
        }
    }
    tracing::warn!("no CJK font found; candidate text may be blank");
    None
}

// ── Renderer ──────────────────────────────────────────────────────────────────

pub struct PanelRenderer {
    font: fontdue::Font,
    theme_resolver: ThemeResolver,
}

impl PanelRenderer {
    pub fn new(font: fontdue::Font) -> Self {
        Self {
            font,
            theme_resolver: ThemeResolver::from_default_locations(),
        }
    }

    /// Render panel to a premultiplied BGRA byte buffer. Returns (bytes, width, height).
    pub fn render(&self, state: &ImeState) -> (Vec<u8>, u32, u32) {
        let theme = self.theme_resolver.current();
        let model = theme
            .candidate_panel_model(state_to_panel_input(state), &UiCapabilities::full_custom());
        let font_size = theme.font.size;
        let label_size = theme.font.label_size;
        let comment_size = theme.font.comment_size;
        let preedit_size = theme.font.preedit_size;
        let panel_pad_x = theme.panel.padding_x;
        let panel_pad_y = theme.panel.padding_y;
        let panel_gap = theme.panel.gap;
        let option_pad_x = theme.candidate.padding_x;
        let option_pad_y = theme.candidate.padding_y;
        let inline_gap = theme.candidate.inline_gap;
        let option_height = theme
            .candidate
            .min_height
            .max(font_size.max(label_size).max(comment_size) + option_pad_y * 2.0);
        let nav_button = theme.navigation.button_size;
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
            .max(content_width + panel_pad_x * 2.0)
            .max(preedit_width)
            .min(theme.panel.max_width)
            .ceil() as u32;
        let height = (content_height + panel_pad_y * 2.0)
            .min(theme.panel.max_height)
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
            theme.panel.corner_radius,
            theme.panel.background,
            theme.panel.border_color,
            theme.panel.border_width,
        );

        let mut y = panel_pad_y;
        if let Some(preedit) = model.preedit.as_ref() {
            self.draw_text(
                &mut pm,
                preedit,
                panel_pad_x,
                y + preedit_size,
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
                row_y += option_height + panel_gap;
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
                x += option_width + panel_gap;
            }
            if nav_count > 0 {
                self.draw_navigation_row(
                    &mut pm,
                    x + panel_gap,
                    y,
                    nav_button,
                    font_size,
                    &model.navigation,
                    &theme,
                );
            }
        }

        // Convert tiny-skia RGBA → BGRA (platform convention for both Windows and Linux)
        let mut out: Vec<u8> = pm.data().to_vec();
        for px in out.chunks_exact_mut(4) {
            px.swap(0, 2); // R ↔ B
        }

        (out, width, height)
    }

    pub fn render_mode_hint(&self, ascii_mode: bool) -> (Vec<u8>, u32, u32) {
        let theme = self.theme_resolver.current();
        let model = theme.mode_hint_model(ascii_mode);
        let font_size = theme.mode_hint.font_size;
        let height = theme.mode_hint.height.ceil().max(1.0) as u32;
        let min_width = theme.mode_hint.width.ceil().max(1.0) as u32;
        let label = model.text;
        let text_width = self.text_width(&label, font_size);
        let width = min_width.max((text_width + 40.0).ceil() as u32);

        let mut pm = Pixmap::new(width, height).expect("pixmap alloc");
        pm.fill(Color::from_rgba8(0, 0, 0, 0));
        draw_rounded_rect(
            &mut pm,
            0.5,
            0.5,
            width as f32 - 1.0,
            height as f32 - 1.0,
            theme.mode_hint.corner_radius,
            theme.candidate.selected_background,
            theme.candidate.selected_border_color,
            theme.candidate.border_width.max(1.0),
        );

        let x = (width as f32 - text_width) * 0.5;
        let baseline = (height as f32 + font_size) * 0.5 - 3.0;
        self.draw_text(
            &mut pm,
            &label,
            x,
            baseline,
            bgra(RgbaColor {
                red: 0xff,
                green: 0xff,
                blue: 0xff,
                alpha: 0xff,
            }),
            font_size,
        );

        let mut out: Vec<u8> = pm.data().to_vec();
        for px in out.chunks_exact_mut(4) {
            px.swap(0, 2);
        }

        (out, width, height)
    }

    pub fn mode_hint_duration_ms(&self) -> u32 {
        let theme = self.theme_resolver.current();
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
        theme: &keytao_theme::ResolvedImeTheme,
    ) {
        let option = &theme.candidate;
        let background = if candidate.selected {
            option.selected_background
        } else {
            option.background
        };
        let border = if candidate.selected {
            option.selected_border_color
        } else {
            option.border_color
        };
        let border_width = if candidate.selected {
            option.border_width.max(1.0)
        } else {
            option.border_width
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
            border_width,
        );

        let mut text_x = x + option.padding_x;
        let baseline = y + (height + font_size) * 0.5 - 3.0;
        let label_color = if candidate.selected {
            option.selected_label_color
        } else {
            option.label_color
        };
        let text_color = if candidate.selected {
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
            text_x,
            baseline,
            bgra(label_color),
            label_size,
        );
        text_x += self.text_width(&candidate.label, label_size) + option.inline_gap;
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
            text_x += option.inline_gap;
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
        let baseline = y + (button_size + font_size) * 0.5 - 3.0;
        if navigation.can_go_previous {
            self.draw_text(
                pm,
                "‹",
                nav_x + button_size * 0.35,
                baseline,
                bgra(theme.navigation.foreground),
                font_size,
            );
            nav_x += button_size + theme.panel.gap;
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
            let (metrics, bitmap) = self.font.rasterize(ch, size);
            let gx = (x + metrics.xmin as f32) as i32;
            let gy = (baseline - metrics.height as f32 - metrics.ymin as f32) as i32;
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
                    let a = alpha as f32 / 255.0;
                    let off = (py as usize * pm.width() as usize + px as usize) * 4;
                    let d = pm.data_mut();
                    d[off] = lerp(d[off], color[0], a); // R
                    d[off + 1] = lerp(d[off + 1], color[1], a); // G
                    d[off + 2] = lerp(d[off + 2], color[2], a); // B
                }
            }
            x += metrics.advance_width;
        }
    }

    fn text_width(&self, text: &str, size: f32) -> f32 {
        text.chars()
            .map(|c| self.font.rasterize(c, size).0.advance_width)
            .sum()
    }
}

#[inline]
fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round() as u8
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
