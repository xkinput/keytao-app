//! Candidate panel renderer.
//!
//! Renders the shared keytao-theme model to a raw BGRA pixel buffer suitable
//! for both X11 XCB image upload and Wayland wl_shm.

use std::{
    borrow::Cow,
    collections::HashSet,
    path::Path as StdPath,
    process::Command,
    sync::mpsc::Receiver,
    thread,
    time::{Duration, Instant},
};

use freetype::{bitmap::PixelMode, face::LoadFlag, ffi, Face, Library};
use keytao_core::ImeState;
use keytao_theme::{
    resolved_panel_shadow, scaled_candidate_panel, CandidatePanelInput, CommentPosition,
    PanelOrientation, RgbaColor, ThemeCandidate, ThemeResolver, UiCapabilities,
};
use tiny_skia::*;

use crate::panel_hit::{fit_candidate_widths, layout, PanelGeometry, PanelHitAreas, PanelRect};
use x11rb::{
    connection::Connection as _,
    protocol::xproto::{
        ColormapAlloc, ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, EventMask,
        ImageFormat, Screen, VisualClass, WindowClass,
    },
    rust_connection::RustConnection,
};

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

fn detect_x11_panel_scale() -> f32 {
    detect_explicit_panel_scale()
        .or_else(xft_dpi_scale)
        .unwrap_or(1.0)
}

// ── Renderer ──────────────────────────────────────────────────────────────────

pub struct PanelRenderer {
    faces: Vec<Face>,
    theme_face: Option<Face>,
    font_family: String,
    _library: Library,
    scale: f32,
    theme_resolver: ThemeResolver,
    supports_alpha: bool,
}

pub struct WaylandPanel {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub logical_width: u32,
    pub logical_height: u32,
    pub hit_areas: PanelHitAreas,
}

struct GlyphLayout<'a> {
    glyph: &'a freetype::GlyphSlot,
    scale: f32,
    width: usize,
    rows: usize,
    advance: f32,
}

#[derive(Clone, Copy)]
struct StackedCandidate {
    word_x: f32,
    word_baseline: f32,
    comment_baseline: f32,
    height: f32,
}

pub enum OverlayPanelMessage {
    Show { state: ImeState, x: i32, y: i32 },
    ModeHint { ascii_mode: bool, x: i32, y: i32 },
    SwitchHint { text: String, x: i32, y: i32 },
    DeploymentHint { text: &'static str, x: i32, y: i32 },
    Hide,
}

pub struct X11OverlayPanel {
    conn: RustConnection,
    panel_win: u32,
    gc: u32,
    depth: u8,
    screen_width: i32,
    screen_height: i32,
    visible: bool,
    renderer: PanelRenderer,
}

pub(crate) fn x11_shadow_supported(depth: u8, compositor_owner: u32) -> bool {
    depth == 32 && compositor_owner != 0
}

pub(crate) fn x11_panel_position(
    anchor: (i32, i32),
    size: (u32, u32),
    inset: i32,
    screen: (i32, i32),
    margin: i32,
    caret_gap: i32,
) -> (i32, i32) {
    let max_x = (screen.0 - size.0 as i32 - margin).max(margin);
    let max_y = (screen.1 - size.1 as i32).max(0);
    let mut y = anchor.1 + caret_gap - inset;
    if y + size.1 as i32 > screen.1 {
        y = anchor.1 - caret_gap - size.1 as i32 + inset;
    }
    ((anchor.0 - inset).clamp(margin, max_x), y.clamp(0, max_y))
}

pub(crate) fn x11_panel_visual<C: x11rb::connection::Connection>(
    conn: &C,
    screen: &Screen,
    screen_num: usize,
) -> (u8, u32, u32, bool) {
    let fallback = (
        screen.root_depth,
        screen.root_visual,
        screen.default_colormap,
        false,
    );
    let compositor_owner = conn
        .intern_atom(true, format!("_NET_WM_CM_S{screen_num}").as_bytes())
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .filter(|reply| reply.atom != 0)
        .and_then(|reply| conn.get_selection_owner(reply.atom).ok())
        .and_then(|cookie| cookie.reply().ok())
        .map_or(0, |reply| reply.owner);
    if !x11_shadow_supported(32, compositor_owner) {
        return fallback;
    }
    let Some(visual) = screen
        .allowed_depths
        .iter()
        .filter(|depth| depth.depth == 32)
        .flat_map(|depth| &depth.visuals)
        .find(|visual| {
            visual.class == VisualClass::TRUE_COLOR
                && visual.red_mask == 0x00FF0000
                && visual.green_mask == 0x0000FF00
                && visual.blue_mask == 0x000000FF
        })
    else {
        return fallback;
    };
    let Ok(colormap) = conn.generate_id() else {
        return fallback;
    };
    let Ok(cookie) =
        conn.create_colormap(ColormapAlloc::NONE, colormap, screen.root, visual.visual_id)
    else {
        return fallback;
    };
    if cookie.check().is_err() {
        return fallback;
    }
    (32, visual.visual_id, colormap, true)
}

impl X11OverlayPanel {
    pub fn new() -> Option<Self> {
        let (conn, screen_num) = RustConnection::connect(None).ok()?;
        let setup = conn.setup();
        let screen = setup.roots.get(screen_num)?;
        let root = screen.root;
        let (depth, visual, colormap, supports_alpha) = x11_panel_visual(&conn, screen, screen_num);
        let screen_width = screen.width_in_pixels as i32;
        let screen_height = screen.height_in_pixels as i32;

        let panel_win = conn.generate_id().ok()?;
        conn.create_window(
            depth,
            panel_win,
            root,
            0,
            0,
            300,
            46,
            0,
            WindowClass::INPUT_OUTPUT,
            visual,
            &CreateWindowAux::new()
                .override_redirect(1)
                .background_pixel(0)
                .border_pixel(0)
                .colormap(colormap)
                .event_mask(EventMask::EXPOSURE),
        )
        .ok()?;

        let gc = conn.generate_id().ok()?;
        conn.create_gc(gc, panel_win, &Default::default()).ok()?;

        let renderer =
            load_font().and_then(|source| PanelRenderer::new_x11(source, supports_alpha))?;

        Some(Self {
            conn,
            panel_win,
            gc,
            depth,
            screen_width,
            screen_height,
            visible: false,
            renderer,
        })
    }

    pub fn show(&mut self, state: &ImeState, x: i32, y: i32) {
        let theme = self.renderer.theme_resolver.current();
        if !crate::engine::panel_has_composition(state, &theme.ui) {
            self.hide();
            return;
        }
        let (pixels, w, h, _) = self.renderer.render_themed(
            state,
            self.screen_width as f32,
            &theme,
            !theme.ui.effective_embedded_composition(),
        );
        let (x, y) = x11_panel_position(
            (x, y),
            (w, h),
            self.renderer.candidate_shadow_inset(),
            (self.screen_width, self.screen_height),
            self.renderer.screen_margin(),
            0,
        );
        self.conn
            .configure_window(
                self.panel_win,
                &ConfigureWindowAux::new().x(x).y(y).width(w).height(h),
            )
            .ok();

        if !self.visible {
            self.conn.map_window(self.panel_win).ok();
            self.visible = true;
        }

        self.conn
            .put_image(
                ImageFormat::Z_PIXMAP,
                self.panel_win,
                self.gc,
                w as u16,
                h as u16,
                0,
                0,
                0,
                self.depth,
                &pixels,
            )
            .ok();
        self.conn.flush().ok();
    }

    pub fn show_mode_hint(&mut self, ascii_mode: bool, x: i32, y: i32) -> Option<Instant> {
        let (pixels, w, h) = self.renderer.render_mode_hint(ascii_mode);
        self.show_hint_pixels(&pixels, w, h, x, y);
        Some(Instant::now() + self.renderer.mode_hint_duration())
    }

    pub fn show_deployment_hint(&mut self, text: &str, x: i32, y: i32) {
        let (pixels, w, h) = self.renderer.render_text_hint(text);
        self.show_hint_pixels(&pixels, w, h, x, y);
    }

    fn show_hint_pixels(&mut self, pixels: &[u8], w: u32, h: u32, x: i32, y: i32) {
        let inset = self.renderer.hint_shadow_inset();
        let (x, y) = x11_panel_position(
            (x - (w as i32 - inset * 2) / 2, y),
            (w, h),
            inset,
            (self.screen_width, self.screen_height),
            self.renderer.screen_margin(),
            0,
        );
        self.conn
            .configure_window(
                self.panel_win,
                &ConfigureWindowAux::new().x(x).y(y).width(w).height(h),
            )
            .ok();

        if !self.visible {
            self.conn.map_window(self.panel_win).ok();
            self.visible = true;
        }

        self.conn
            .put_image(
                ImageFormat::Z_PIXMAP,
                self.panel_win,
                self.gc,
                w as u16,
                h as u16,
                0,
                0,
                0,
                self.depth,
                pixels,
            )
            .ok();
        self.conn.flush().ok();
    }

    pub fn hide(&mut self) {
        if self.visible {
            self.conn.unmap_window(self.panel_win).ok();
            self.conn.flush().ok();
            self.visible = false;
        }
    }
}

impl Drop for X11OverlayPanel {
    fn drop(&mut self) {
        self.conn.destroy_window(self.panel_win).ok();
        self.conn.free_gc(self.gc).ok();
        self.conn.flush().ok();
    }
}

pub fn spawn_x11_overlay_panel(rx: Receiver<OverlayPanelMessage>) -> bool {
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut panel = X11OverlayPanel::new();
        let _ = ready_tx.send(panel.is_some());
        if panel.is_none() {
            tracing::warn!("X11 overlay panel unavailable; falling back to system lookup UI");
        }
        let mut mode_hint_until: Option<Instant> = None;
        let mut composition = None;
        loop {
            let msg = match mode_hint_until {
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        if let Some(panel) = panel.as_mut() {
                            if let Some((state, x, y)) = composition.take() {
                                panel.show(&state, x, y);
                            } else {
                                panel.hide();
                            }
                        }
                        mode_hint_until = None;
                        continue;
                    }
                    rx.recv_timeout(deadline.saturating_duration_since(now))
                        .ok()
                }
                None => rx.recv().ok(),
            };
            let Some(msg) = msg else {
                if mode_hint_until.is_some() {
                    continue;
                }
                break;
            };
            if let Some(panel) = panel.as_mut() {
                match msg {
                    OverlayPanelMessage::Show { state, x, y } => {
                        mode_hint_until = None;
                        panel.show(&state, x, y);
                        composition = Some((state, x, y));
                    }
                    OverlayPanelMessage::ModeHint { ascii_mode, x, y } => {
                        composition = None;
                        mode_hint_until = panel.show_mode_hint(ascii_mode, x, y);
                    }
                    OverlayPanelMessage::SwitchHint { text, x, y } => {
                        panel.show_deployment_hint(&text, x, y);
                        mode_hint_until = Some(Instant::now() + panel.renderer.mode_hint_duration());
                    }
                    OverlayPanelMessage::DeploymentHint { text, x, y } => {
                        mode_hint_until = None;
                        composition = None;
                        panel.show_deployment_hint(text, x, y);
                    }
                    OverlayPanelMessage::Hide => {
                        mode_hint_until = None;
                        composition = None;
                        panel.hide();
                    }
                }
            }
        }
    });
    ready_rx.recv().unwrap_or(false)
}

impl PanelRenderer {
    pub fn new(source: FontSource) -> Option<Self> {
        Self::with_scale(source, 1.0)
    }

    pub fn new_x11(source: FontSource, supports_alpha: bool) -> Option<Self> {
        let mut renderer = Self::with_scale(source, detect_x11_panel_scale())?;
        renderer.supports_alpha = supports_alpha;
        Some(renderer)
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
            theme_face: None,
            font_family: String::new(),
            _library: library,
            scale: clamp_panel_scale(scale),
            theme_resolver: ThemeResolver::from_default_locations(),
            supports_alpha: true,
        })
    }

    pub fn hide_candidate_panel(&self) -> bool {
        self.theme_resolver.current().ui.hide_candidate_panel
    }

    pub fn candidate_panel_layout(&self) -> keytao_core::key_policy::CandidatePanelLayout {
        crate::engine::candidate_panel_layout(&self.theme_resolver.current())
    }

    pub fn screen_margin(&self) -> i32 {
        self.s(self.theme_resolver.current().panel.screen_margin)
            .ceil() as i32
    }

    pub fn candidate_shadow_inset(&self) -> i32 {
        self.shadow_inset(self.theme_resolver.current().panel.shadow)
    }

    pub fn hint_shadow_inset(&self) -> i32 {
        self.shadow_inset(self.theme_resolver.current().mode_hint.shadow)
    }

    fn shadow_inset(&self, enabled: bool) -> i32 {
        resolved_panel_shadow(enabled && self.supports_alpha)
            .map_or(0, |shadow| self.s(shadow.margin).ceil() as i32)
    }

    pub fn render_wayland(
        &mut self,
        state: Option<&ImeState>,
        ascii_mode: bool,
        scale: f32,
        screen_width: f32,
        text_hint: Option<&str>,
    ) -> WaylandPanel {
        self.scale = scale;
        let (pixels, width, height, mut hit_areas) = if let Some(text) = text_hint {
            let (pixels, width, height) = self.render_text_hint(text);
            (pixels, width, height, PanelHitAreas::default())
        } else if let Some(state) = state {
            let theme = self.theme_resolver.current();
            self.render_themed(
                state,
                screen_width * scale,
                &theme,
                !theme.ui.effective_embedded_composition(),
            )
        } else {
            let (pixels, width, height) = self.render_mode_hint(ascii_mode);
            (pixels, width, height, PanelHitAreas::default())
        };
        let logical_width = (width as f64 / scale as f64).ceil().max(1.0) as u32;
        let logical_height = (height as f64 / scale as f64).ceil().max(1.0) as u32;
        let buffer_width = crate::panel_scale::logical_to_physical(logical_width, scale);
        let buffer_height = crate::panel_scale::logical_to_physical(logical_height, scale);
        // Pad rather than stretch the raster; integer buffer scales require divisible sizes.
        let pixels = if (buffer_width, buffer_height) == (width, height) {
            pixels
        } else {
            let mut padded = vec![0; buffer_width as usize * buffer_height as usize * 4];
            for (source, target) in pixels
                .chunks_exact(width as usize * 4)
                .zip(padded.chunks_exact_mut(buffer_width as usize * 4))
            {
                target[..source.len()].copy_from_slice(source);
            }
            padded
        };
        let x_scale = logical_width as f32 / buffer_width as f32;
        let y_scale = logical_height as f32 / buffer_height as f32;
        for rect in hit_areas
            .candidates
            .iter_mut()
            .chain(hit_areas.previous_page.iter_mut())
            .chain(hit_areas.next_page.iter_mut())
        {
            rect.x *= x_scale;
            rect.y *= y_scale;
            rect.width *= x_scale;
            rect.height *= y_scale;
        }
        WaylandPanel {
            pixels,
            width: buffer_width,
            height: buffer_height,
            logical_width,
            logical_height,
            hit_areas,
        }
    }

    /// Render panel to a BGRA byte buffer.  Returns (bytes, width, height).
    #[cfg(test)]
    pub fn render(&mut self, state: &ImeState, screen_width: f32) -> (Vec<u8>, u32, u32) {
        let (pixels, width, height, _) = self.render_with_hit_areas(state, screen_width);
        (pixels, width, height)
    }

    /// Like `render`, plus where each candidate and paging button was drawn.
    pub fn render_with_hit_areas(
        &mut self,
        state: &ImeState,
        screen_width: f32,
    ) -> (Vec<u8>, u32, u32, PanelHitAreas) {
        self.render_themed(state, screen_width, &self.theme_resolver.current(), true)
    }

    fn render_themed(
        &mut self,
        state: &ImeState,
        screen_width: f32,
        theme: &keytao_theme::ResolvedImeTheme,
        include_preedit: bool,
    ) -> (Vec<u8>, u32, u32, PanelHitAreas) {
        let theme = scaled_candidate_panel(theme);
        self.update_theme_font(theme.font.effective_family.as_deref());
        let mut input = state_to_panel_input(state);
        if !include_preedit {
            input.preedit.clear();
        }
        let model = theme.candidate_panel_model(
            input,
            &UiCapabilities {
                supports_hover: false,
                ..UiCapabilities::full_custom()
            },
        );
        let font_size = self.s(theme.font.size);
        let label_size = self.s(theme.font.label_size);
        let comment_size = self.s(theme.font.comment_size);
        let preedit_size = self.s(theme.font.preedit_size);
        let panel_pad_left = self.s(theme.panel.padding_left);
        let panel_pad_right = self.s(theme.panel.padding_right);
        let panel_pad_top = self.s(theme.panel.padding_top);
        let panel_pad_bottom = self.s(theme.panel.padding_bottom);
        let panel_gap = self.s(theme.panel.gap);
        let option_pad_x = self.s(theme.candidate.padding_x);
        let option_pad_y = self.s(theme.candidate.padding_y);
        let inline_gap = self.s(theme.candidate.inline_gap);
        let below = theme.candidate.comment_position == CommentPosition::Below;
        let align_stacked_row = below
            && model.orientation == PanelOrientation::Horizontal
            && model
                .candidates
                .iter()
                .any(|candidate| candidate.comment.is_some());
        let min_option_height = self.s(theme.candidate.min_height);
        let single_line_height = font_size.max(label_size);
        let single_line_height = if below {
            single_line_height
        } else {
            single_line_height.max(comment_size)
        };
        let single_option_height = min_option_height.max(single_line_height + option_pad_y * 2.0);
        let nav_button = self.s(theme.navigation.button_size);
        let preedit_height = model
            .preedit
            .as_ref()
            .map(|_| preedit_size + self.s(theme.panel.preedit_gap))
            .unwrap_or(0.0);
        let (natural_widths, mut stacked): (Vec<_>, Vec<_>) = model
            .candidates
            .iter()
            .map(|candidate| {
                let comment = candidate.comment.as_deref().filter(|_| below);
                if comment.is_some() || align_stacked_row {
                    let (_, label_top, label_right, label_bottom) =
                        self.text_bounds(&candidate.label, label_size);
                    let (word_left, word_top, word_right, word_bottom) =
                        self.text_bounds(&candidate.text, font_size);
                    let (comment_left, comment_top, comment_right, comment_bottom) = comment
                        .map(|text| self.text_bounds(text, comment_size))
                        .unwrap_or_default();
                    let word_x = label_right + inline_gap - word_left.min(comment_left);
                    let word_baseline = -label_top.min(word_top);
                    let word_bottom = word_baseline + label_bottom.max(word_bottom);
                    let comment_baseline = word_bottom + inline_gap * 0.5 - comment_top;
                    let height = if comment.is_some() {
                        comment_baseline + comment_bottom
                    } else {
                        word_bottom
                    };
                    let width = word_x + word_right.max(comment_right) + option_pad_x * 2.0;
                    return (
                        width,
                        Some(StackedCandidate {
                            word_x,
                            word_baseline,
                            comment_baseline,
                            height,
                        }),
                    );
                }
                let width = option_pad_x * 2.0
                    + self.text_width(&candidate.label, label_size)
                    + inline_gap
                    + self.text_width(&candidate.text, font_size)
                    + candidate.comment.as_ref().map_or(0.0, |comment| {
                        inline_gap + self.text_width(comment, comment_size)
                    });
                (width, None)
            })
            .unzip();
        if align_stacked_row {
            let row_baseline = stacked
                .iter()
                .flatten()
                .map(|option| option.word_baseline)
                .fold(0.0, f32::max);
            for option in stacked.iter_mut().flatten() {
                let offset = row_baseline - option.word_baseline;
                option.word_baseline = row_baseline;
                option.comment_baseline += offset;
                option.height += offset;
            }
        }
        let option_heights = below.then(|| {
            stacked
                .iter()
                .map(|option| {
                    option.map_or(single_option_height, |option| {
                        min_option_height.max(option.height + option_pad_y * 2.0)
                    })
                })
                .collect::<Vec<_>>()
        });
        let option_height = option_heights
            .as_deref()
            .unwrap_or_default()
            .iter()
            .copied()
            .fold(single_option_height, f32::max);
        let nav_count = usize::from(model.navigation.can_go_previous)
            + usize::from(model.navigation.can_go_next);
        let nav_width = if nav_count == 0 {
            0.0
        } else {
            nav_count as f32 * nav_button + nav_count.saturating_sub(1) as f32 * panel_gap
        };
        let max_width = (screen_width
            - self.s(theme.panel.screen_margin).ceil() * 2.0
            - self.shadow_inset(theme.panel.shadow) as f32 * 2.0)
            .floor()
            .max(1.0);
        let row_width = (max_width - panel_pad_left - panel_pad_right).max(0.0);
        let candidate_count = model.candidates.len();
        let option_widths = if model.orientation == PanelOrientation::Horizontal {
            let gaps = candidate_count.saturating_sub(1) + if nav_count > 0 { 2 } else { 0 };
            let available = (row_width - nav_width - gaps as f32 * panel_gap).max(0.0);
            if natural_widths.iter().sum::<f32>() <= available {
                natural_widths.clone()
            } else {
                let selected = state
                    .highlighted_candidate_index
                    .min(candidate_count.saturating_sub(1));
                let ellipsis_width = self.text_bounds(self.ellipsis(), font_size).2;
                let minimum_widths: Vec<_> = model
                    .candidates
                    .iter()
                    .enumerate()
                    .map(|(index, candidate)| {
                        let word_x = stacked[index].map_or_else(
                            || self.text_width(&candidate.label, label_size) + inline_gap,
                            |option| option.word_x,
                        );
                        let leading_width = if index.abs_diff(selected) <= 1 {
                            candidate.text.chars().next().map_or(0.0, |ch| {
                                self.layout_glyph(ch, font_size)
                                    .map_or(font_size * 0.5, |glyph| glyph.advance)
                            })
                        } else {
                            0.0
                        };
                        option_pad_x * 2.0 + word_x + leading_width + ellipsis_width
                    })
                    .collect();
                fit_candidate_widths(&natural_widths, &minimum_widths, selected, available)
            }
        } else {
            natural_widths
                .iter()
                .map(|width| width.min(row_width))
                .collect()
        };
        let visible_count = option_widths.iter().filter(|&&width| width > 0.0).count();
        let last_visible = option_widths.iter().rposition(|&width| width > 0.0);
        let preedit_width = model
            .preedit
            .as_ref()
            .map(|preedit| self.text_width(preedit, preedit_size))
            .unwrap_or(0.0);

        let (content_width, content_height) = if model.orientation == PanelOrientation::Vertical {
            let max_option_width = option_widths.iter().copied().fold(0.0, f32::max);
            let candidate_height = option_heights.as_deref().map_or_else(
                || model.candidates.len() as f32 * option_height,
                |heights| heights.iter().sum(),
            ) + model.candidates.len().saturating_sub(1) as f32 * panel_gap;
            (
                max_option_width.max(nav_width),
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
                + visible_count.saturating_sub(1) as f32 * panel_gap;
            let nav_extra = if nav_count > 0 {
                panel_gap * if visible_count == 0 { 1.0 } else { 2.0 } + nav_width
            } else {
                0.0
            };
            (candidate_width + nav_extra, preedit_height + option_height)
        };
        let width = self
            .s(theme.panel.min_width)
            .max(content_width.max(preedit_width) + panel_pad_left + panel_pad_right)
            .ceil()
            .max(1.0)
            .min(max_width) as u32;
        let height = (content_height + panel_pad_top + panel_pad_bottom)
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

        let mut y = panel_pad_top;
        if let Some(preedit) = model.preedit.as_ref() {
            let baseline = y + preedit_size;
            if theme.ui.hide_candidate_panel {
                draw_rounded_rect(
                    &mut pm,
                    panel_pad_left,
                    y,
                    self.text_width(preedit, preedit_size),
                    preedit_size,
                    0.0,
                    theme.candidate.selected_background,
                    theme.candidate.selected_background,
                    0.0,
                );
            }
            self.draw_text(
                &mut pm,
                preedit,
                panel_pad_left,
                baseline,
                bgra(if theme.ui.hide_candidate_panel {
                    theme.candidate.selected_foreground
                } else {
                    theme.candidate.foreground
                }),
                preedit_size,
            );
            y += preedit_height;
        }

        let mut areas = layout(&PanelGeometry {
            vertical: model.orientation == PanelOrientation::Vertical,
            pad_x: panel_pad_left,
            top: y,
            gap: panel_gap,
            option_widths: &option_widths,
            option_heights: option_heights.as_deref(),
            option_height,
            nav_button,
            can_go_previous: model.navigation.can_go_previous,
            can_go_next: model.navigation.can_go_next,
        });
        for (index, (candidate, rect)) in model
            .candidates
            .iter()
            .zip(areas.candidates.iter())
            .enumerate()
        {
            if rect.width <= 0.0 {
                continue;
            }
            self.draw_candidate_option(
                &mut pm,
                candidate,
                *rect,
                stacked[index],
                label_size,
                font_size,
                comment_size,
                &theme,
                align_stacked_row,
                rect.width < natural_widths[index],
            );
            if theme.candidate.separator_visible && last_visible.is_some_and(|last| index < last) {
                let (x, y, width, height) = if model.orientation == PanelOrientation::Vertical {
                    (
                        rect.x,
                        rect.y + rect.height + panel_gap * 0.5,
                        rect.width,
                        self.scale,
                    )
                } else {
                    (
                        rect.x + rect.width + panel_gap * 0.5,
                        rect.y,
                        self.scale,
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
        for (rect, glyph) in [(areas.previous_page, "‹"), (areas.next_page, "›")] {
            if let Some(rect) = rect {
                self.draw_navigation_button(&mut pm, rect, glyph, font_size, &theme);
            }
        }
        let (pm, inset) = with_panel_shadow(
            pm,
            self.s(theme.panel.corner_radius),
            theme.panel.shadow && self.supports_alpha,
            self.scale,
        );
        let (buffer_width, buffer_height) = (pm.width(), pm.height());
        for rect in areas
            .candidates
            .iter_mut()
            .chain(areas.previous_page.iter_mut())
            .chain(areas.next_page.iter_mut())
        {
            rect.width = rect.width.min((width as f32 - rect.x).max(0.0));
            rect.height = rect.height.min((height as f32 - rect.y).max(0.0));
            rect.x += inset;
            rect.y += inset;
        }

        // Convert RGBA (tiny-skia) → BGRA (platform native)
        let mut out: Vec<u8> = pm.data().to_vec();
        for px in out.chunks_exact_mut(4) {
            px.swap(0, 2); // R↔B
        }

        (out, buffer_width, buffer_height, areas)
    }

    pub fn render_mode_hint(&mut self, ascii_mode: bool) -> (Vec<u8>, u32, u32) {
        let theme = self.theme_resolver.current();
        let model = theme.mode_hint_model(ascii_mode);
        self.render_text_hint(&model.text)
    }

    pub fn render_text_hint(&mut self, label: &str) -> (Vec<u8>, u32, u32) {
        let theme = self.theme_resolver.current();
        self.update_theme_font(theme.font.effective_family.as_deref());
        let hint_size = self.s(theme.mode_hint.font_size);
        let padding = hint_size / 6.0;
        let (left, top, right, bottom) = self.text_bounds(&label, hint_size);
        let text_width = right - left;
        let text_height = bottom - top;
        let width = self
            .s(theme.mode_hint.width)
            .max(text_width + padding * 2.0)
            .ceil()
            .max(1.0) as u32;
        let hint_height = self
            .s(theme.mode_hint.height)
            .max(text_height + padding * 2.0)
            .ceil()
            .max(1.0) as u32;
        let mut pm = Pixmap::new(width, hint_height).expect("pixmap alloc");
        pm.fill(Color::from_rgba8(0, 0, 0, 0));

        draw_rounded_rect(
            &mut pm,
            0.5,
            0.5,
            width as f32 - 1.0,
            hint_height as f32 - 1.0,
            self.s(theme.mode_hint.corner_radius)
                .min(hint_height as f32 * 0.5),
            theme.mode_hint.background,
            theme.mode_hint.border_color,
            self.s(theme.mode_hint.border_width),
        );

        let x = (width as f32 - text_width) * 0.5 - left;
        let baseline = (hint_height as f32 - text_height) * 0.5 - top;
        self.draw_text(
            &mut pm,
            &label,
            x,
            baseline,
            bgra(theme.mode_hint.foreground),
            hint_size,
        );

        let (pm, _) = with_panel_shadow(
            pm,
            self.s(theme.mode_hint.corner_radius),
            theme.mode_hint.shadow && self.supports_alpha,
            self.scale,
        );
        let (width, hint_height) = (pm.width(), pm.height());
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
        rect: PanelRect,
        stacked: Option<StackedCandidate>,
        label_size: f32,
        font_size: f32,
        comment_size: f32,
        theme: &keytao_theme::ResolvedImeTheme,
        align_stacked_row: bool,
        constrained: bool,
    ) {
        let PanelRect {
            x,
            y,
            width,
            height,
        } = rect;
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
        draw_rounded_rect(
            pm,
            x,
            y,
            width,
            height,
            self.s(candidate_theme.corner_radius),
            background,
            border,
            self.s(candidate_theme.border_width),
        );

        let option_pad_x = self.s(candidate_theme.padding_x);
        let inline_gap = self.s(candidate_theme.inline_gap);
        let label_x = x + option_pad_x;
        let (text_x, baseline, comment_baseline) = if let Some(stacked) = stacked {
            let top = y + if align_stacked_row {
                self.s(candidate_theme.padding_y)
            } else {
                (height - stacked.height) * 0.5
            };
            (
                label_x + stacked.word_x,
                top + stacked.word_baseline,
                top + stacked.comment_baseline,
            )
        } else {
            let baseline = y + (height + font_size) * 0.5 - self.s(4.0);
            (
                label_x + self.text_width(&candidate.label, label_size) + inline_gap,
                baseline,
                baseline,
            )
        };
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
        let right = x + width - option_pad_x;
        let label = if constrained {
            self.fit_line(&candidate.label, label_size, right - label_x)
        } else {
            Cow::Borrowed(candidate.label.as_str())
        };
        let text = if constrained {
            self.fit_line(&candidate.text, font_size, right - text_x)
        } else {
            Cow::Borrowed(candidate.text.as_str())
        };
        self.draw_text(pm, &label, label_x, baseline, bgra(label_color), label_size);
        self.draw_text(pm, &text, text_x, baseline, bgra(text_color), font_size);
        if let Some(comment) = candidate.comment.as_ref() {
            let comment_x = if stacked.is_some() {
                text_x
            } else {
                if text != candidate.text {
                    return;
                }
                text_x + self.text_width(&text, font_size) + inline_gap
            };
            let comment = if constrained {
                self.fit_line(comment, comment_size, right - comment_x)
            } else {
                Cow::Borrowed(comment.as_str())
            };
            self.draw_text(
                pm,
                &comment,
                comment_x,
                comment_baseline,
                bgra(comment_color),
                comment_size,
            );
        }
    }

    fn draw_navigation_button(
        &self,
        pm: &mut Pixmap,
        rect: PanelRect,
        glyph: &str,
        font_size: f32,
        theme: &keytao_theme::ResolvedImeTheme,
    ) {
        let baseline = rect.y + (rect.height + font_size) * 0.5 - self.s(4.0);
        self.draw_text(
            pm,
            glyph,
            rect.x + rect.width * 0.35,
            baseline,
            bgra(theme.navigation.foreground),
            font_size,
        );
    }

    fn ellipsis(&self) -> &'static str {
        if self.face_for_char('…').is_some() {
            "…"
        } else {
            "..."
        }
    }

    fn fit_line<'a>(&self, text: &'a str, size: f32, width: f32) -> Cow<'a, str> {
        if width <= 0.0 {
            return Cow::Borrowed("");
        }
        let ellipsis = self.ellipsis();
        let ellipsis_width = self.text_bounds(ellipsis, size).2;
        let mut pen = 0.0;
        let mut prefix = 0;
        for (index, ch) in text.char_indices() {
            if !is_zero_width_selector(ch) {
                let (advance, right) =
                    self.layout_glyph(ch, size)
                        .map_or((size * 0.5, size * 0.5), |layout| {
                            let right = layout.glyph.bitmap_left() as f32 * layout.scale
                                + layout.width as f32;
                            (layout.advance, right.max(layout.advance))
                        });
                if pen + right > width {
                    if ellipsis_width > width {
                        return Cow::Borrowed("");
                    }
                    let mut fitted = String::with_capacity(prefix + ellipsis.len());
                    fitted.push_str(&text[..prefix]);
                    fitted.push_str(ellipsis);
                    return Cow::Owned(fitted);
                }
                pen += advance;
            }
            if pen + ellipsis_width <= width {
                prefix = index + ch.len_utf8();
            }
        }
        Cow::Borrowed(text)
    }

    fn s(&self, value: f32) -> f32 {
        value * self.scale
    }

    fn update_theme_font(&mut self, family: Option<&str>) {
        let family = family.unwrap_or("").trim();
        if self.font_family == family {
            return;
        }
        self.theme_face = if family.is_empty() {
            None
        } else {
            fc_match(&fontconfig_pattern(family, Some("zh")))
                .and_then(|(path, index)| self._library.new_face(path, index.unwrap_or(0)).ok())
        };
        self.font_family = family.to_owned();
    }

    fn layout_glyph(&self, ch: char, size: f32) -> Option<GlyphLayout<'_>> {
        let face = self.face_for_char(ch)?;
        let color_face = face.has_color();
        let load_flags = if color_face {
            LoadFlag::RENDER | LoadFlag::COLOR
        } else {
            LoadFlag::TARGET_NORMAL
        };
        if face.set_pixel_sizes(0, size.ceil() as u32).is_err() {
            let _ = face.select_size(0);
        }
        face.load_char(ch as usize, load_flags).ok()?;
        if !color_face {
            let slot = face.glyph().raw() as *const _ as ffi::FT_GlyphSlot;
            unsafe {
                ffi::FT_GlyphSlot_Embolden(slot);
                if ffi::FT_Render_Glyph(slot, ffi::FT_RENDER_MODE_NORMAL) != 0 {
                    return None;
                }
            }
        }

        let glyph = face.glyph();
        let bitmap = glyph.bitmap();
        let width = bitmap.width().max(0) as usize;
        let rows = bitmap.rows().max(0) as usize;
        let advance = (glyph.advance().x as f32 / 64.0).max(size * 0.35);
        if width == 0 || rows == 0 || bitmap.buffer().is_empty() {
            return Some(GlyphLayout {
                glyph,
                scale: 1.0,
                width: 0,
                rows: 0,
                advance,
            });
        }
        let color_glyph = bitmap.pixel_mode().ok() == Some(PixelMode::Bgra);
        let scale = if color_glyph {
            (size * COLOR_GLYPH_HEIGHT_FACTOR / rows as f32)
                .min(size * COLOR_GLYPH_WIDTH_FACTOR / width as f32)
                .min(1.0)
        } else {
            1.0
        };
        let width = ((width as f32 * scale).ceil() as usize).max(1);
        let rows = ((rows as f32 * scale).ceil() as usize).max(1);
        Some(GlyphLayout {
            glyph,
            scale,
            width,
            rows,
            advance: if color_glyph {
                width as f32 + size * 0.1
            } else {
                advance
            },
        })
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
        for ch in text.chars().filter(|ch| !is_zero_width_selector(*ch)) {
            let Some(layout) = self.layout_glyph(ch, size) else {
                x += size * 0.5;
                continue;
            };
            let bitmap = layout.glyph.bitmap();
            let width = bitmap.width().max(0) as usize;
            let rows = bitmap.rows().max(0) as usize;
            let pitch = bitmap.pitch().unsigned_abs() as usize;
            let buffer = bitmap.buffer();
            let pixel_mode = bitmap.pixel_mode().ok();
            let gx = (x + layout.glyph.bitmap_left() as f32 * layout.scale) as i32;
            let gy = (baseline - layout.glyph.bitmap_top() as f32 * layout.scale) as i32;

            for row in 0..layout.rows {
                let scaled_row = (row as f32 / layout.scale).floor() as usize;
                let source_row = if bitmap.pitch() >= 0 {
                    scaled_row.min(rows - 1)
                } else {
                    rows - scaled_row.min(rows - 1) - 1
                };
                for col in 0..layout.width {
                    let source_col = (col as f32 / layout.scale).floor() as usize;
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
                    blend_pixel(
                        pm,
                        px,
                        py,
                        r,
                        g,
                        b,
                        alpha as f32 / 255.0 * color[3] as f32 / 255.0,
                    );
                }
            }
            x += layout.advance;
        }
    }

    fn text_width(&self, text: &str, size: f32) -> f32 {
        text.chars()
            .filter(|ch| !is_zero_width_selector(*ch))
            .map(|ch| {
                self.layout_glyph(ch, size)
                    .map_or(size * 0.5, |layout| layout.advance)
            })
            .sum()
    }

    fn text_bounds(&self, text: &str, size: f32) -> (f32, f32, f32, f32) {
        let mut x: f32 = 0.0;
        let mut left: f32 = 0.0;
        let mut right: f32 = 0.0;
        let mut top = f32::INFINITY;
        let mut bottom = f32::NEG_INFINITY;
        for ch in text.chars().filter(|ch| !is_zero_width_selector(*ch)) {
            let Some(layout) = self.layout_glyph(ch, size) else {
                x += size * 0.5;
                continue;
            };
            if layout.width > 0 && layout.rows > 0 {
                let glyph_left = x + layout.glyph.bitmap_left() as f32 * layout.scale;
                let glyph_top = -layout.glyph.bitmap_top() as f32 * layout.scale;
                left = left.min(glyph_left);
                right = right.max(glyph_left + layout.width as f32);
                top = top.min(glyph_top);
                bottom = bottom.max(glyph_top + layout.rows as f32);
            }
            x += layout.advance;
        }
        if !top.is_finite() {
            top = -size;
            bottom = 0.0;
        }
        (left, top, right.max(x), bottom)
    }

    fn face_for_char(&self, ch: char) -> Option<&Face> {
        self.theme_face
            .iter()
            .chain(self.faces.iter())
            .find(|face| face.get_char_index(ch as usize).unwrap_or_default() != 0)
    }
}

fn is_zero_width_selector(ch: char) -> bool {
    matches!(ch, '\u{fe0e}' | '\u{fe0f}' | '\u{200d}')
}

pub(crate) fn state_to_panel_input(state: &ImeState) -> CandidatePanelInput {
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

fn with_panel_shadow(body: Pixmap, radius: f32, enabled: bool, scale: f32) -> (Pixmap, f32) {
    let Some(shadow) = resolved_panel_shadow(enabled) else {
        return (body, 0.0);
    };
    let inset = (shadow.margin * scale).ceil() as u32;
    let Some(mut result) = Pixmap::new(body.width() + inset * 2, body.height() + inset * 2) else {
        return (body, 0.0);
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
    (result, inset as f32)
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
    d[off + 3] = (255.0 * a + d[off + 3] as f32 * (1.0 - a)).round() as u8;
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
mod x11_shadow_tests {
    use super::{x11_panel_position, x11_shadow_supported};

    #[test]
    fn shadows_require_compositor_and_argb_visual() {
        assert!(x11_shadow_supported(32, 42));
        assert!(!x11_shadow_supported(32, 0));
        assert!(!x11_shadow_supported(24, 42));
        assert!(!x11_shadow_supported(24, 0));
    }

    #[test]
    fn shadow_inset_keeps_body_at_caret_at_every_scale() {
        for scale in [1.0_f32, 1.25, 2.0] {
            let inset = (24.0 * scale).ceil() as i32;
            let size = (200 + inset as u32 * 2, 100 + inset as u32 * 2);
            let origin = x11_panel_position((300, 250), size, inset, (1920, 1080), 8, 4);
            assert_eq!((origin.0 + inset, origin.1 + inset), (300, 254));
        }
    }

    #[test]
    fn shadow_panel_flips_body_above_caret_and_clamps_full_window() {
        let origin = x11_panel_position((300, 1040), (248, 148), 24, (1920, 1080), 8, 4);
        assert_eq!(origin, (276, 912));
        assert_eq!(origin.1 + 24 + 100, 1040 - 4);
        assert_eq!(
            x11_panel_position((1910, 1060), (248, 148), 24, (1920, 1080), 8, 4),
            (1664, 932),
        );
        assert_eq!(
            x11_panel_position((0, 0), (248, 148), 24, (1920, 1080), 8, 4),
            (8, 0),
        );
    }

    #[test]
    fn panels_without_shadow_keep_anchor_and_centered_hints() {
        assert_eq!(
            x11_panel_position((300, 250), (200, 100), 0, (1920, 1080), 8, 4),
            (300, 254),
        );
        let inset = 24;
        let width = 128;
        let body_left = 300 - (width - inset * 2) / 2;
        assert_eq!(
            x11_panel_position(
                (body_left, 250),
                (width as u32, 100),
                inset,
                (1920, 1080),
                8,
                0
            ),
            (300 - width / 2, 226),
        );
    }
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
        let Some(mut renderer) = PanelRenderer::new(source) else {
            eprintln!("skipping panel render test: font source could not be reopened");
            return;
        };

        let mut state = ImeState::empty();
        state.candidates = vec![Candidate {
            text: "候选".repeat(20),
            comment: None,
        }];

        let (pixels, width, _) = renderer.render(&state, 1920.0);
        assert!(
            width > 320,
            "long candidate was capped at the legacy theme width"
        );
        let (_, constrained_width, _) = renderer.render(&state, 480.0);
        assert_eq!(constrained_width, 480 - renderer.screen_margin() as u32 * 2);
        let visible_colors = pixels
            .chunks_exact(4)
            .filter(|pixel| pixel[3] > 0)
            .map(|pixel| [pixel[0], pixel[1], pixel[2], pixel[3]])
            .collect::<HashSet<_>>()
            .len();

        assert!(
            visible_colors > 2,
            "candidate panel text rendered no visible pixels"
        );
    }

    #[test]
    fn long_horizontal_candidates_keep_neighbors_and_paging_clickable() {
        let Some(mut renderer) = load_font().and_then(PanelRenderer::new) else {
            eprintln!("skipping candidate overflow test: no CJK font found");
            return;
        };
        let mut state = ImeState::empty();
        state.candidates = (0..5)
            .map(|_| Candidate {
                text: "超长候选文字".repeat(20),
                comment: None,
            })
            .collect();
        state.highlighted_candidate_index = 3;
        state.is_last_page = false;
        let mut theme = keytao_theme::ResolvedImeTheme::default();
        theme.panel.shadow = false;
        theme.panel.orientation = PanelOrientation::Horizontal;
        let (_, width, _, areas) = renderer.render_themed(&state, 480.0, &theme, true);
        for index in 2..=4 {
            let rect = areas.candidates[index];
            assert!(rect.width > areas.candidates[0].width);
            assert!(rect.x + rect.width <= width as f32);
            assert_eq!(
                areas.click_at(
                    f64::from(rect.x + rect.width * 0.5),
                    f64::from(rect.y + rect.height * 0.5),
                ),
                Some(crate::panel_hit::PanelClick::Candidate(index)),
            );
        }
        let next = areas.next_page.unwrap();
        assert!(next.width > 0.0 && next.x + next.width <= width as f32);
        assert_eq!(
            areas.click_at(f64::from(next.x + 1.0), f64::from(next.y + 1.0)),
            Some(crate::panel_hit::PanelClick::NextPage),
        );

        let text = renderer.fit_line(&state.candidates[3].text, theme.font.size, 100.0);
        assert!(text.ends_with(renderer.ellipsis()));
        assert!(renderer.text_bounds(&text, theme.font.size).2 <= 100.0);
    }

    #[test]
    fn candidate_comment_expands_panel_width() {
        let Some(source) = load_font() else {
            eprintln!("skipping comment render test: no CJK font found");
            return;
        };
        let Some(mut renderer) = PanelRenderer::new(source) else {
            eprintln!("skipping comment render test: font source could not be reopened");
            return;
        };

        let mut base = ImeState::empty();
        base.candidates = vec![Candidate {
            text: "这".to_string(),
            comment: None,
        }];

        let mut with_comment = base.clone();
        with_comment.candidates[0].comment = Some("abcdefghijklmnopqrstuvwxyz".repeat(3));

        let (_, base_width, _) = renderer.render(&base, 1920.0);
        let (_, comment_width, _) = renderer.render(&with_comment, 1920.0);

        assert!(
            comment_width > base_width,
            "candidate comment did not affect rendered panel width"
        );
        assert!(
            comment_width > 320,
            "long comment was capped at the legacy theme width"
        );
    }

    #[test]
    fn candidate_below_uses_glyph_bounds_and_clickable_rows() {
        let Some(mut renderer) = load_font().and_then(PanelRenderer::new) else {
            eprintln!("skipping stacked candidate render test: no CJK font found");
            return;
        };
        let mut state = ImeState::empty();
        state.candidates = vec![
            Candidate {
                text: "候选文字".into(),
                comment: Some("注释拼音 gpq".into()),
            },
            Candidate {
                text: "候选文字".into(),
                comment: None,
            },
        ];
        let color = |red, green, blue| RgbaColor {
            red,
            green,
            blue,
            alpha: 255,
        };
        let mut theme = keytao_theme::ResolvedImeTheme::default();
        theme.panel.shadow = false;
        theme.candidate.selected_label_color = color(255, 0, 0);
        theme.candidate.selected_foreground = color(0, 255, 0);
        theme.candidate.selected_comment_color = color(0, 0, 255);
        theme.candidate.foreground = color(255, 0, 255);
        for orientation in [PanelOrientation::Vertical, PanelOrientation::Horizontal] {
            theme.panel.orientation = orientation;
            theme.candidate.comment_position = CommentPosition::Inline;
            let (_, _, _, inline) = renderer.render_themed(&state, 1920.0, &theme, true);
            theme.candidate.comment_position = CommentPosition::Below;
            let (pixels, width, _, stacked) = renderer.render_themed(&state, 1920.0, &theme, true);
            let first = stacked.candidates[0];
            assert!(first.width < inline.candidates[0].width);
            assert!(first.height > inline.candidates[0].height);
            if orientation == PanelOrientation::Vertical {
                assert_eq!(stacked.candidates[1].height, inline.candidates[1].height);
            }

            let glyph_positions = |color| {
                pixels
                    .chunks_exact(4)
                    .enumerate()
                    .filter(|(_, pixel)| *pixel == bgra(color))
                    .map(|(i, _)| (i % width as usize, i / width as usize))
                    .collect::<Vec<_>>()
            };
            let label = glyph_positions(theme.candidate.selected_label_color);
            let word = glyph_positions(theme.candidate.selected_foreground);
            let comment = glyph_positions(theme.candidate.selected_comment_color);
            if orientation == PanelOrientation::Horizontal {
                let unannotated = glyph_positions(theme.candidate.foreground);
                assert!(!unannotated.is_empty());
                assert_eq!(
                    word.iter().map(|(_, y)| y).min(),
                    unannotated.iter().map(|(_, y)| y).min()
                );
            }
            assert!(
                label.iter().map(|(x, _)| x).max().unwrap()
                    < word.iter().map(|(x, _)| x).min().unwrap()
            );
            assert!(
                word.iter().map(|(_, y)| y).max().unwrap()
                    < comment.iter().map(|(_, y)| y).min().unwrap()
            );
            let (x, y) = comment[0];
            assert_eq!(
                stacked.click_at(x as f64, y as f64),
                Some(crate::panel_hit::PanelClick::Candidate(0))
            );
        }
        let (_, width, _, constrained) = renderer.render_themed(&state, 70.0, &theme, true);
        let first = constrained.candidates[0];
        assert!(first.x + first.width <= width as f32);
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
        let Some(mut renderer) = PanelRenderer::new(source) else {
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
    fn measured_text_bounds_contain_rendered_glyphs() {
        let Some(source) = load_font() else {
            eprintln!("skipping text bounds test: no CJK font found");
            return;
        };
        let Some(renderer) = PanelRenderer::with_scale(source, 1.0) else {
            eprintln!("skipping text bounds test: font source could not be reopened");
            return;
        };

        for size in [10.0, 20.0, 36.0] {
            let text = "j中英";
            let (left, top, right, bottom) = renderer.text_bounds(text, size);
            let mut pm = Pixmap::new(256, 128).unwrap();
            let x = 32.0;
            let baseline = 64.0;
            renderer.draw_text(&mut pm, text, x, baseline, [0, 0, 0, 255], size);
            let mut visible = false;
            for (index, pixel) in pm.data().chunks_exact(4).enumerate() {
                if pixel[3] == 0 {
                    continue;
                }
                visible = true;
                let px = (index % pm.width() as usize) as f32;
                let py = (index / pm.width() as usize) as f32;
                assert!(px >= x + left && px < x + right);
                assert!(py >= baseline + top && py < baseline + bottom);
            }
            assert!(visible, "text bounds check rendered no visible glyphs");
        }
    }

    #[test]
    fn explicit_panel_scale_expands_candidate_panel() {
        let Some(source) = load_font() else {
            eprintln!("skipping panel scale test: no CJK font found");
            return;
        };
        let Some(mut normal) = PanelRenderer::with_scale(source.clone(), 1.0) else {
            eprintln!("skipping panel scale test: font source could not be reopened");
            return;
        };
        let Some(mut scaled) = PanelRenderer::with_scale(source, 2.0) else {
            eprintln!("skipping panel scale test: font source could not be reopened");
            return;
        };

        let mut state = ImeState::empty();
        state.preedit = "fan".to_string();
        state.candidates = vec![Candidate {
            text: "这".to_string(),
            comment: Some("~a".to_string()),
        }];

        let (_, normal_width, normal_height) = normal.render(&state, 1920.0);
        let (_, scaled_width, scaled_height) = scaled.render(&state, 3840.0);

        assert!(scaled_width > normal_width);
        assert!(scaled_height > normal_height);
    }
}
