//! Candidate panel renderer.
//!
//! Fixed dark theme (Catppuccin Mocha).  Renders to a raw BGRA pixel buffer
//! suitable for both X11 XCB image upload and Wayland wl_shm.
//!
//! Layout (single horizontal bar):
//!
//!   ┌──────────────────────────────────────────────────────┐
//!   │  [preedit]                                           │  ← 18px row
//!   │  1.候选  2.候选  3.候选  ...         ‹ page ›       │  ← 24px row
//!   └──────────────────────────────────────────────────────┘

use std::{collections::HashSet, path::Path as StdPath};

use freetype::{bitmap::PixelMode, face::LoadFlag, ffi, Face, Library};
use keytao_core::ImeState;
use tiny_skia::*;

// ── Catppuccin Mocha ──────────────────────────────────────────────────────────

const BG: [u8; 4] = [0x2e, 0x1e, 0x1e, 0xff]; // BGRA 0x1e1e2e
const FG: [u8; 4] = [0xfb, 0xf7, 0xf4, 0xff]; // 0xf4f7fb
const ACCENT: [u8; 4] = [0xd5, 0xe2, 0x94, 0xff]; // 0x94e2d5
const PREEDIT_COLOR: [u8; 4] = [0xeb, 0xdc, 0x89, 0xff]; // 0x89dceb
const DIM: [u8; 4] = [0xc8, 0xad, 0xa6, 0xff]; // 0xa6adc8
const COMMENT: [u8; 4] = [0xaf, 0xe2, 0xf9, 0xff]; // 0xf9e2af
const SEP: [u8; 4] = [0x50, 0x48, 0x45, 0xff]; // surface1 0x45475a → darker

const FONT_SIZE: f32 = 22.0;
const LABEL_SIZE: f32 = 15.0;
const COMMENT_SIZE: f32 = 15.0;
const PREEDIT_SIZE: f32 = 15.0;
const COLOR_GLYPH_HEIGHT_FACTOR: f32 = 1.05;
const COLOR_GLYPH_WIDTH_FACTOR: f32 = 1.35;
const PADDING: f32 = 12.0;
const CAND_GAP: f32 = 20.0;
const HEIGHT: u32 = 60;
const MIN_WIDTH: u32 = 260;
const NAV_WIDTH: f32 = 42.0;
const MAX_COLLECTION_FACES: isize = 32;
const HINT_HEIGHT: u32 = 36;
const HINT_MIN_WIDTH: u32 = 48;
const HINT_SIZE: f32 = 20.0;
const HINT_PAD_X: f32 = 14.0;
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
            .set_pixel_sizes(0, FONT_SIZE.ceil() as u32)
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

// ── Renderer ──────────────────────────────────────────────────────────────────

pub struct PanelRenderer {
    faces: Vec<Face>,
    _library: Library,
}

impl PanelRenderer {
    pub fn new(source: FontSource) -> Option<Self> {
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
        })
    }

    /// Render panel to a BGRA byte buffer.  Returns (bytes, width, height).
    pub fn render(&self, state: &ImeState) -> (Vec<u8>, u32, u32) {
        let cand_width: f32 = state
            .candidates
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let label = self.candidate_label(state, i);
                let comment = c.comment.as_deref().unwrap_or_default();
                self.text_width(&format!("{label}. "), LABEL_SIZE)
                    + self.text_width(&c.text, FONT_SIZE)
                    + if comment.is_empty() {
                        0.0
                    } else {
                        6.0 + self.text_width(comment, COMMENT_SIZE)
                    }
                    + CAND_GAP
            })
            .sum();
        let preedit_width = if state.preedit.is_empty() {
            0.0
        } else {
            self.text_width(&state.preedit, PREEDIT_SIZE) + PADDING * 2.0
        };
        let nav_width = if state.candidates.is_empty() {
            0.0
        } else {
            NAV_WIDTH
        };
        let width = (MIN_WIDTH as f32)
            .max(cand_width + PADDING * 2.0 + nav_width)
            .max(preedit_width) as u32;

        let mut pm = Pixmap::new(width, HEIGHT).expect("pixmap alloc");

        // Background
        pm.fill(Color::from_rgba8(BG[2], BG[1], BG[0], 255));

        // Preedit
        let cand_y = if state.preedit.is_empty() {
            HEIGHT as f32 / 2.0 + FONT_SIZE / 2.0 - 4.0
        } else {
            self.draw_text(
                &mut pm,
                &state.preedit,
                PADDING,
                14.0,
                PREEDIT_COLOR,
                PREEDIT_SIZE,
            );
            HEIGHT as f32 - 10.0
        };

        // Candidates
        let mut x = PADDING;
        let selected_index = state
            .highlighted_candidate_index
            .min(state.candidates.len().saturating_sub(1));
        for (i, cand) in state.candidates.iter().enumerate() {
            let label = format!("{}. ", self.candidate_label(state, i));
            let color = if i == selected_index { ACCENT } else { FG };
            self.draw_text(&mut pm, &label, x, cand_y, DIM, LABEL_SIZE);
            x += self.text_width(&label, LABEL_SIZE);
            self.draw_text(&mut pm, &cand.text, x, cand_y, color, FONT_SIZE);
            x += self.text_width(&cand.text, FONT_SIZE);
            if let Some(comment) = cand
                .comment
                .as_deref()
                .filter(|comment| !comment.is_empty())
            {
                x += 6.0;
                self.draw_text(&mut pm, comment, x, cand_y, COMMENT, COMMENT_SIZE);
                x += self.text_width(comment, COMMENT_SIZE);
            }
            x += CAND_GAP;
        }

        // Page arrows
        if !state.candidates.is_empty() {
            let ax = width as f32 - nav_width + 4.0;
            let prev_color = if state.page == 0 { DIM } else { FG };
            self.draw_text(&mut pm, "‹", ax, cand_y, prev_color, FONT_SIZE);
            let next_color = if state.is_last_page { DIM } else { FG };
            self.draw_text(&mut pm, "›", ax + 18.0, cand_y, next_color, FONT_SIZE);
        }

        // Bottom separator
        let mut paint = Paint::default();
        paint.set_color_rgba8(SEP[2], SEP[1], SEP[0], 255);
        pm.fill_rect(
            Rect::from_xywh(0.0, HEIGHT as f32 - 1.0, width as f32, 1.0).unwrap(),
            &paint,
            Transform::identity(),
            None,
        );

        // Convert RGBA (tiny-skia) → BGRA (platform native)
        let mut out: Vec<u8> = pm.data().to_vec();
        for px in out.chunks_exact_mut(4) {
            px.swap(0, 2); // R↔B
        }

        (out, width, HEIGHT)
    }

    fn candidate_label(&self, state: &ImeState, index: usize) -> String {
        state
            .select_keys
            .as_deref()
            .and_then(|keys| keys.chars().nth(index))
            .or_else(|| "1234567890".chars().nth(index))
            .map(|ch| ch.to_string())
            .unwrap_or_else(|| (index + 1).to_string())
    }

    pub fn render_mode_hint(&self, ascii_mode: bool) -> (Vec<u8>, u32, u32) {
        let label = if ascii_mode { "英" } else { "中" };
        let text_width = self.text_width(label, HINT_SIZE);
        let width = HINT_MIN_WIDTH.max((text_width + HINT_PAD_X * 2.0).ceil() as u32);
        let mut pm = Pixmap::new(width, HINT_HEIGHT).expect("pixmap alloc");
        pm.fill(Color::from_rgba8(0, 0, 0, 0));

        let bg = if ascii_mode {
            [0x1f, 0x44, 0x5f, 0xee]
        } else {
            [0x57, 0x52, 0x30, 0xee]
        };
        let fg = if ascii_mode {
            [0x8a, 0xc8, 0xff, 0xff]
        } else {
            [0xd7, 0xf3, 0x9c, 0xff]
        };
        let border = if ascii_mode {
            [0x35, 0x76, 0xa5, 0xb8]
        } else {
            [0x83, 0x7b, 0x48, 0xb8]
        };

        if let Some(path) =
            rounded_rect_path(0.5, 0.5, width as f32 - 1.0, HINT_HEIGHT as f32 - 1.0, 8.0)
        {
            let mut paint = Paint::default();
            paint.set_color(Color::from_rgba8(bg[2], bg[1], bg[0], bg[3]));
            paint.anti_alias = true;
            pm.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );

            paint.set_color(Color::from_rgba8(
                border[2], border[1], border[0], border[3],
            ));
            let mut stroke = Stroke::default();
            stroke.width = 1.0;
            pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }

        let x = (width as f32 - text_width) * 0.5;
        let baseline = (HINT_HEIGHT as f32 + HINT_SIZE) * 0.5 - 3.0;
        self.draw_text(&mut pm, label, x, baseline, fg, HINT_SIZE);

        let mut out = pm.data().to_vec();
        for px in out.chunks_exact_mut(4) {
            px.swap(0, 2);
        }

        (out, width, HINT_HEIGHT)
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
        let visible_text_pixels = pixels
            .chunks_exact(4)
            .filter(|pixel| *pixel != BG && *pixel != SEP)
            .count();

        assert!(
            visible_text_pixels > 0,
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
}
