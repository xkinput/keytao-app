//! IBus D-Bus backend for keytao-ime.
//!
//! Implements enough of the IBus D-Bus protocol so that Chromium/CEF apps
//! (e.g. WeChatAppEx) can use keytao as their IME without requiring a real
//! IBus daemon.

use crate::engine::{CoreEngine, ImeSession};
use crate::panel::{load_font, PanelRenderer};
use keytao_core::{key_policy, ImeState, RIME_RELEASE_MASK};
use keytao_theme::{
    CandidateOptionModel, CandidatePanelInput, CandidatePanelModel, ThemeCandidate, ThemeResolver,
    UiCapabilities,
};
use std::{
    fs,
    sync::{
        atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering},
        Arc,
    },
    time::Instant,
};
use x11rb::{
    connection::Connection as _,
    protocol::xproto::{
        ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, EventMask, ImageFormat,
        WindowClass,
    },
    rust_connection::RustConnection,
};
use zbus::{connection, interface, object_server::SignalContext, zvariant};

enum ImePanelMessage {
    Show { state: ImeState, x: i32, y: i32 },
    ModeHint { ascii_mode: bool, x: i32, y: i32 },
    Hide,
}

struct X11Panel {
    conn: RustConnection,
    panel_win: u32,
    gc: u32,
    depth: u8,
    visible: bool,
    renderer: PanelRenderer,
}

impl X11Panel {
    fn new() -> Option<Self> {
        let (conn, screen_num) = RustConnection::connect(None).ok()?;
        let setup = conn.setup();
        let screen = setup.roots.get(screen_num)?;
        let root = screen.root;
        let visual = screen.root_visual;
        let depth = screen.root_depth;

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
                .background_pixel(0x1e1e2e)
                .event_mask(EventMask::EXPOSURE),
        )
        .ok()?;

        let gc = conn.generate_id().ok()?;
        conn.create_gc(gc, panel_win, &Default::default()).ok()?;

        let renderer = load_font().and_then(PanelRenderer::new_x11)?;

        Some(Self {
            conn,
            panel_win,
            gc,
            depth,
            visible: false,
            renderer,
        })
    }

    fn show(&mut self, state: &ImeState, x: i32, y: i32) {
        let (pixels, w, h) = self.renderer.render(state);
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

    fn show_mode_hint(&mut self, ascii_mode: bool, x: i32, y: i32) -> Option<Instant> {
        let (pixels, w, h) = self.renderer.render_mode_hint(ascii_mode);
        let x = (x - w as i32 / 2).max(0);
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
        Some(Instant::now() + self.renderer.mode_hint_duration())
    }

    fn hide(&mut self) {
        if self.visible {
            self.conn.unmap_window(self.panel_win).ok();
            self.conn.flush().ok();
            self.visible = false;
        }
    }
}

impl Drop for X11Panel {
    fn drop(&mut self) {
        self.conn.destroy_window(self.panel_win).ok();
        self.conn.free_gc(self.gc).ok();
        self.conn.flush().ok();
    }
}

// ── IBus text helper ─────────────────────────────────────────────────────────

const IBUS_ORIENTATION_SYSTEM: i32 = 2;

fn is_shift_key(sym: u32) -> bool {
    matches!(sym, 0xffe1 | 0xffe2)
}

fn highlighted_candidate_index(state: &ImeState) -> Option<usize> {
    key_policy::highlighted_candidate_index(state)
}

/// Build an IBusText structure as a variant.
/// IBus D-Bus type: v containing (sa{sv}sv)
///   ("IBusText", {}, text_string, v:("IBusAttrList",{},[]))
fn ibus_text_variant(text: &str) -> zvariant::Value<'static> {
    use zvariant::{Array, Dict, Signature, StructureBuilder, Value};

    // Build IBusAttrList: ("IBusAttrList", {}, [])
    let sig_s = Signature::try_from("s").unwrap();
    let sig_v = Signature::try_from("v").unwrap();
    let empty_dict1 = Dict::new(sig_s.clone(), sig_v.clone());
    let empty_array = Array::new(sig_v.clone());
    let attr_list = StructureBuilder::new()
        .add_field("IBusAttrList".to_owned())
        .append_field(Value::Dict(empty_dict1))
        .append_field(Value::Array(empty_array))
        .build();

    // Wrap attr_list as variant
    let attr_list_variant = Value::Value(Box::new(Value::Structure(attr_list)));

    // Build IBusText: ("IBusText", {}, text, v:attr_list)
    let empty_dict2 = Dict::new(sig_s, sig_v);
    let ibus_text_struct = StructureBuilder::new()
        .add_field("IBusText".to_owned())
        .append_field(Value::Dict(empty_dict2))
        .add_field(text.to_owned())
        .append_field(attr_list_variant)
        .build();

    // Return the structure directly so callers used as `v` signal parameters get
    // single-wrapped (v(sa{sv}sv)).  For av array elements callers must wrap
    // explicitly with Value::Value(Box::new(ibus_text_variant(…))).
    Value::Structure(ibus_text_struct)
}

/// Wrap an IBusText structure inside a variant for use in `av` arrays.
fn ibus_text_as_variant(text: &str) -> zvariant::Value<'static> {
    zvariant::Value::Value(Box::new(ibus_text_variant(text)))
}

fn ibus_text_value(text: &str) -> zvariant::OwnedValue {
    zvariant::OwnedValue::try_from(ibus_text_variant(text)).expect("ibus_text_value")
}

/// Build an IBusEngineDesc value for the "keytao" engine.
/// Structure: (sa{sv} name longname description language license author icon layout rank hotkeys symbol setup layout_variant layout_option version textdomain)
fn ibus_engine_desc_value() -> zvariant::OwnedValue {
    use zvariant::{Dict, Signature, StructureBuilder, Value};

    let sig_s = Signature::try_from("s").unwrap();
    let sig_v = Signature::try_from("v").unwrap();
    let empty_dict = Dict::new(sig_s, sig_v);

    let engine = StructureBuilder::new()
        .add_field("IBusEngineDesc".to_owned())
        .append_field(Value::Dict(empty_dict))
        .add_field("keytao".to_owned()) // name
        .add_field("KeyTao".to_owned()) // longname
        .add_field("KeyTao Input Method".to_owned()) // description
        .add_field("zh".to_owned()) // language
        .add_field("".to_owned()) // license
        .add_field("".to_owned()) // author
        .add_field("".to_owned()) // icon
        .add_field("default".to_owned()) // layout
        .add_field(0u32) // rank
        .add_field("".to_owned()) // hotkeys
        .add_field("键".to_owned()) // symbol
        .add_field("".to_owned()) // setup
        .add_field("".to_owned()) // layout_variant
        .add_field("".to_owned()) // layout_option
        .add_field("".to_owned()) // version
        .add_field("".to_owned()) // textdomain
        .build();

    zvariant::OwnedValue::try_from(Value::Structure(engine)).expect("ibus_engine_desc_value")
}

fn candidate_display_text(candidate: &CandidateOptionModel) -> String {
    match candidate.comment.as_deref() {
        Some(comment) => format!("{} {}", candidate.text, comment),
        None => candidate.text.clone(),
    }
}

fn state_to_panel_model(state: &ImeState, theme_resolver: &ThemeResolver) -> CandidatePanelModel {
    let theme = theme_resolver.current();
    theme.candidate_panel_model(
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
        },
        &UiCapabilities::system_lookup_table(),
    )
}

/// Build an IBusLookupTable value.
/// Serialized shape: ("IBusLookupTable", a{sv}, u, u, b, b, i, av, av).
fn ibus_lookup_table_value(model: &CandidatePanelModel) -> zvariant::OwnedValue {
    use zvariant::{Array, Dict, Signature, StructureBuilder, Value};

    let sig_s = Signature::try_from("s").unwrap();
    let sig_v = Signature::try_from("v").unwrap();
    let empty_dict = Dict::new(sig_s, sig_v.clone());

    let mut candidates = Array::new(sig_v.clone());
    for candidate in &model.candidates {
        candidates
            .append(ibus_text_as_variant(&candidate_display_text(candidate)))
            .expect("append IBus lookup candidate");
    }

    let mut labels = Array::new(sig_v);
    for candidate in &model.candidates {
        labels
            .append(ibus_text_as_variant(&candidate.label))
            .expect("append IBus lookup label");
    }

    let page_size = model.candidates.len().clamp(1, 16) as u32;
    let cursor_pos = model
        .candidates
        .iter()
        .position(|candidate| candidate.selected)
        .unwrap_or(0) as u32;

    let table = StructureBuilder::new()
        .add_field("IBusLookupTable".to_owned())
        .append_field(Value::Dict(empty_dict))
        .add_field(page_size)
        .add_field(cursor_pos)
        .add_field(true)
        .add_field(false)
        .add_field(IBUS_ORIENTATION_SYSTEM)
        .append_field(Value::Array(candidates))
        .append_field(Value::Array(labels))
        .build();

    zvariant::OwnedValue::try_from(Value::Structure(table)).expect("ibus_lookup_table_value")
}

// ── InputContext D-Bus object ─────────────────────────────────────────────────

struct InputContext {
    session: ImeSession,
    kimpanel_ctxt: Option<SignalContext<'static>>,
    cursor_x: Arc<AtomicI32>,
    cursor_y: Arc<AtomicI32>,
    ascii_mode: Arc<AtomicBool>,
    x11_panel_tx: std::sync::mpsc::Sender<ImePanelMessage>,
    theme_resolver: Arc<ThemeResolver>,
}

impl InputContext {
    async fn clear_ui(&self, ctxt: &SignalContext<'_>) {
        let _ = Self::hide_preedit_text(ctxt).await;
        let _ = Self::hide_lookup_table(ctxt).await;
        if let Some(kc) = &self.kimpanel_ctxt {
            let _ = Kimpanel::show_preedit_text(kc, false).await;
            let _ = Kimpanel::show_lookup_table(kc, false).await;
        }
        let _ = self.x11_panel_tx.send(ImePanelMessage::Hide);
    }

    async fn update_mode_hint(&self, ascii_mode: bool) {
        let previous = self.ascii_mode.swap(ascii_mode, Ordering::Relaxed);
        if previous == ascii_mode {
            return;
        }
        let cx = self.cursor_x.load(Ordering::Relaxed);
        let cy = self.cursor_y.load(Ordering::Relaxed);
        let _ = self.x11_panel_tx.send(ImePanelMessage::ModeHint {
            ascii_mode,
            x: cx,
            y: cy + 24,
        });
    }

    async fn apply_ime_state(&self, ime_state: ImeState, ctxt: &SignalContext<'_>) {
        let ascii_mode = ime_state.ascii_mode;
        let has_candidates = !ime_state.candidates.is_empty();
        let mode_changed = self.ascii_mode.load(Ordering::Relaxed) != ascii_mode;
        if let Some(ref text) = ime_state.committed {
            if !text.is_empty() {
                tracing::info!("IBus CommitText: {text:?}");
                clear_preedit(ctxt, &self.kimpanel_ctxt).await;
                let ov = ibus_text_value(text);
                if let Ok(v) = zvariant::Value::try_from(&ov) {
                    let _ = Self::commit_text(ctxt, v).await;
                }
            }
        }

        if ime_state.preedit.is_empty() {
            let _ = Self::hide_preedit_text(ctxt).await;
            if let Some(kctxt) = &self.kimpanel_ctxt {
                let _ = Kimpanel::show_preedit_text(kctxt, false).await;
            }
        } else {
            clear_preedit(ctxt, &None).await;

            let cursor = ime_state.cursor as u32;
            let ov = ibus_text_value(&ime_state.preedit);
            if let Ok(v) = zvariant::Value::try_from(&ov) {
                let _ = Self::update_preedit_text(ctxt, v, cursor, true).await;
            }
            if let Some(kctxt) = &self.kimpanel_ctxt {
                let _ = Kimpanel::update_preedit_text(kctxt, &ime_state.preedit, "").await;
                let _ = Kimpanel::show_preedit_text(kctxt, true).await;
            }
        }

        if ime_state.candidates.is_empty() {
            let _ = Self::hide_lookup_table(ctxt).await;
            if let Some(kctxt) = &self.kimpanel_ctxt {
                let _ = Kimpanel::show_lookup_table(kctxt, false).await;
            }
            let _ = self.x11_panel_tx.send(ImePanelMessage::Hide);
        } else {
            let model = state_to_panel_model(&ime_state, &self.theme_resolver);
            let ov = ibus_lookup_table_value(&model);
            if let Ok(v) = zvariant::Value::try_from(&ov) {
                let _ = Self::update_lookup_table(ctxt, v, true).await;
            }
            if let Some(kctxt) = &self.kimpanel_ctxt {
                let labels: Vec<String> = model
                    .candidates
                    .iter()
                    .map(|candidate| candidate.label.clone())
                    .collect();
                let cands: Vec<String> = model
                    .candidates
                    .iter()
                    .map(candidate_display_text)
                    .collect();
                let labels_ref: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
                let cands_ref: Vec<&str> = cands.iter().map(|s| s.as_str()).collect();
                let attrs: Vec<&str> = vec![];
                let _ = Kimpanel::update_lookup_table(
                    kctxt,
                    &labels_ref,
                    &cands_ref,
                    &attrs,
                    model.navigation.can_go_previous,
                    model.navigation.can_go_next,
                )
                .await;
                let _ = Kimpanel::show_lookup_table(kctxt, true).await;
                let _ = Kimpanel::update_spot_location(
                    kctxt,
                    self.cursor_x.load(Ordering::Relaxed),
                    self.cursor_y.load(Ordering::Relaxed),
                )
                .await;
            }

            let cx = self.cursor_x.load(Ordering::Relaxed);
            let cy = self.cursor_y.load(Ordering::Relaxed);
            let _ = self.x11_panel_tx.send(ImePanelMessage::Show {
                state: ime_state,
                x: cx,
                y: cy + 24,
            });
        }

        if mode_changed && !has_candidates {
            self.update_mode_hint(ascii_mode).await;
        } else {
            self.ascii_mode.store(ascii_mode, Ordering::Relaxed);
        }
    }

    async fn select_candidate_at(&self, index: usize, ctxt: &SignalContext<'_>) -> bool {
        match self.session.select_candidate(index) {
            Some(ime_state) => {
                self.apply_ime_state(ime_state, ctxt).await;
                true
            }
            None => false,
        }
    }

    async fn change_page(&self, backward: bool, ctxt: &SignalContext<'_>) {
        if let Some(ime_state) = self.session.change_page(backward) {
            self.apply_ime_state(ime_state, ctxt).await;
        }
    }

    async fn process_navigation_key(&self, keyval: u32, ctxt: &SignalContext<'_>) {
        if let Some(result) = self.session.process_key_result(keyval, 0) {
            if result.accepted {
                self.apply_ime_state(result.state, ctxt).await;
            }
        }
    }
}

#[interface(name = "org.freedesktop.IBus.InputContext")]
impl InputContext {
    async fn focus_in(&self) {
        tracing::info!("IBus InputContext: FocusIn");
    }

    async fn focus_out(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        tracing::info!("IBus InputContext: FocusOut");
        self.session.reset();
        self.clear_ui(&ctxt).await;
    }

    async fn reset(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        tracing::info!("IBus InputContext: Reset");
        self.session.reset();
        self.clear_ui(&ctxt).await;
    }

    async fn set_cursor_location(&self, x: i32, y: i32, _w: i32, _h: i32) {
        self.cursor_x.store(x, Ordering::Relaxed);
        self.cursor_y.store(y, Ordering::Relaxed);
        if let Some(kctxt) = &self.kimpanel_ctxt {
            let _ = Kimpanel::update_spot_location(kctxt, x, y).await;
        }
    }
    async fn set_cursor_location_relative(&self, _x: i32, _y: i32, _w: i32, _h: i32) {}
    async fn set_capabilities(&self, _caps: u32) {}
    async fn set_surrounding_text(&self, _text: zvariant::Value<'_>, _cursor: u32, _anchor: u32) {}
    async fn set_content_type(&self, _purpose: u32, _hints: u32) {}

    async fn enable(&self) {
        tracing::info!("IBus InputContext: Enable");
    }

    async fn disable(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        tracing::info!("IBus InputContext: Disable");
        self.session.reset();
        self.clear_ui(&ctxt).await;
    }

    async fn page_up(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        self.change_page(true, &ctxt).await;
    }

    async fn page_down(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        self.change_page(false, &ctxt).await;
    }

    async fn cursor_up(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        self.process_navigation_key(0xff52, &ctxt).await;
    }

    async fn cursor_down(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        self.process_navigation_key(0xff54, &ctxt).await;
    }

    async fn candidate_clicked(
        &self,
        index: u32,
        _button: u32,
        _state: u32,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
    ) {
        let _ = self.select_candidate_at(index as usize, &ctxt).await;
    }

    async fn property_activate(&self, _name: &str, _state: u32) {}
    async fn property_show(&self, _name: &str) {}
    async fn property_hide(&self, _name: &str) {}

    async fn destroy(
        &self,
        #[zbus(object_server)] server: &zbus::ObjectServer,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
    ) -> zbus::fdo::Result<()> {
        tracing::info!("IBus InputContext: Destroy");
        self.session.reset();
        self.clear_ui(&ctxt).await;
        server
            .remove::<InputContext, _>(ctxt.path().to_owned())
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    /// Process a key event. Returns true if consumed by the IME.
    async fn process_key_event(
        &self,
        keyval: u32,
        _keycode: u32,
        state: u32,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
    ) -> bool {
        if state & RIME_RELEASE_MASK != 0 {
            if is_shift_key(keyval) {
                if let Some(result) = self.session.process_key_result(keyval, RIME_RELEASE_MASK) {
                    tracing::debug!(
                        "IBus mode after Shift release: ascii_mode={}",
                        result.state.ascii_mode
                    );
                    self.update_mode_hint(result.state.ascii_mode).await;
                    return result.accepted;
                }
            }
            return false;
        }

        tracing::info!("IBus ProcessKeyEvent keyval={keyval:#x} state={state:#x}");

        let before_state = self.session.state();
        if key_policy::should_bypass_empty_composition(keyval, state, &before_state) {
            self.clear_ui(&ctxt).await;
            return false;
        }
        if key_policy::is_enter_key(keyval) && !before_state.preedit.is_empty() {
            clear_preedit(&ctxt, &self.kimpanel_ctxt).await;
            let ov = ibus_text_value(&before_state.preedit);
            if let Ok(v) = zvariant::Value::try_from(&ov) {
                let _ = Self::commit_text(&ctxt, v).await;
            }
            self.session.reset();
            let _ = Self::hide_lookup_table(&ctxt).await;
            if let Some(kctxt) = &self.kimpanel_ctxt {
                let _ = Kimpanel::show_lookup_table(kctxt, false).await;
            }
            let _ = self.x11_panel_tx.send(ImePanelMessage::Hide);
            return true;
        }
        if let Some(index) =
            key_policy::candidate_index_for_space_or_select_key(keyval, &before_state)
        {
            if self.select_candidate_at(index, &ctxt).await {
                return true;
            }
        }

        let result = match self.session.process_key_result(keyval, state) {
            Some(r) => r,
            None => return false,
        };

        let ime_state = result.state;
        let consumed = result.accepted;

        self.apply_ime_state(ime_state, &ctxt).await;

        consumed
    }

    // ── Signals ──────────────────────────────────────────────────────────────

    #[zbus(signal)]
    async fn commit_text(ctxt: &SignalContext<'_>, text: zvariant::Value<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn update_preedit_text(
        ctxt: &SignalContext<'_>,
        text: zvariant::Value<'_>,
        cursor_pos: u32,
        visible: bool,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn hide_preedit_text(ctxt: &SignalContext<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn show_preedit_text(ctxt: &SignalContext<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn update_lookup_table(
        ctxt: &SignalContext<'_>,
        table: zvariant::Value<'_>,
        visible: bool,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn hide_lookup_table(ctxt: &SignalContext<'_>) -> zbus::Result<()>;
}

/// Send an empty UpdatePreeditText to tell the client the composition ended
/// before committing. This is the sequence Chromium/CEF requires so that it
/// can correctly place the committed text without conflating it with the
/// still-active preedit region.
async fn clear_preedit(ctxt: &SignalContext<'_>, kctxt: &Option<SignalContext<'static>>) {
    let ov = ibus_text_value("");
    if let Ok(v) = zvariant::Value::try_from(&ov) {
        let _ = InputContext::update_preedit_text(ctxt, v, 0, false).await;
    }
    if let Some(kc) = kctxt {
        let _ = Kimpanel::update_preedit_text(kc, "", "").await;
        let _ = Kimpanel::show_preedit_text(kc, false).await;
    }
}

// ── IBusBus D-Bus object ──────────────────────────────────────────────────────

struct Kimpanel;

#[interface(name = "org.kde.kimpanel.inputmethod")]
impl Kimpanel {
    #[zbus(signal)]
    async fn update_spot_location(ctxt: &SignalContext<'_>, x: i32, y: i32) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn update_lookup_table(
        ctxt: &SignalContext<'_>,
        labels: &[&str],
        candidates: &[&str],
        attrs: &[&str],
        has_prev: bool,
        has_next: bool,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn show_lookup_table(ctxt: &SignalContext<'_>, b: bool) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn update_preedit_text(
        ctxt: &SignalContext<'_>,
        text: &str,
        attr: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn show_preedit_text(ctxt: &SignalContext<'_>, b: bool) -> zbus::Result<()>;
}

struct IBusBus {
    engine: CoreEngine,
    ctx_counter: Arc<AtomicU32>,
    kimpanel_ctxt: Option<SignalContext<'static>>,
    x11_panel_tx: std::sync::mpsc::Sender<ImePanelMessage>,
    theme_resolver: Arc<ThemeResolver>,
}

#[interface(name = "org.freedesktop.IBus")]
impl IBusBus {
    /// CreateInputContext(client_name) → object_path
    async fn create_input_context(
        &self,
        client_name: &str,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> zbus::fdo::Result<zbus::zvariant::OwnedObjectPath> {
        let n = self.ctx_counter.fetch_add(1, Ordering::SeqCst);
        let path_str = format!("/org/freedesktop/IBus/InputContext_{n}");
        let path = zbus::zvariant::OwnedObjectPath::try_from(path_str.clone())
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;

        tracing::info!("IBus CreateInputContext client={client_name:?} -> {path_str}");

        let session = self
            .engine
            .create_session()
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        let ctx = InputContext {
            session,
            kimpanel_ctxt: self.kimpanel_ctxt.clone(),
            cursor_x: Arc::new(AtomicI32::new(0)),
            cursor_y: Arc::new(AtomicI32::new(0)),
            ascii_mode: Arc::new(AtomicBool::new(false)),
            x11_panel_tx: self.x11_panel_tx.clone(),
            theme_resolver: self.theme_resolver.clone(),
        };
        server
            .at(path.clone(), ctx)
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;

        Ok(path)
    }

    async fn is_global_engine(&self) -> bool {
        true
    }

    async fn get_engines(&self) -> Vec<zvariant::OwnedValue> {
        vec![ibus_engine_desc_value()]
    }

    async fn list_active_engines(&self) -> Vec<zvariant::OwnedValue> {
        vec![ibus_engine_desc_value()]
    }

    async fn get_global_engine(&self) -> zbus::fdo::Result<zvariant::OwnedValue> {
        Ok(ibus_engine_desc_value())
    }

    async fn set_global_engine(&self, name: &str) -> zbus::fdo::Result<()> {
        tracing::info!("IBus SetGlobalEngine: {name}");
        Ok(())
    }

    async fn register_component(&self, _component: zvariant::Value<'_>) -> zbus::fdo::Result<()> {
        tracing::info!("IBus RegisterComponent");
        Ok(())
    }

    async fn exit(&self, restart: bool) {
        tracing::info!("IBus Exit requested restart={restart}");
    }

    #[zbus(signal)]
    async fn global_engine_changed(ctxt: &SignalContext<'_>, name: &str) -> zbus::Result<()>;
}

// ── IBus address file management ──────────────────────────────────────────────

fn write_ibus_address_files(dbus_address: &str) {
    let machine_id = read_machine_id();
    let pid = std::process::id();

    let bus_dir = match dirs::config_dir() {
        Some(d) => d.join("ibus").join("bus"),
        None => {
            tracing::warn!("cannot determine config dir; skipping IBus address files");
            return;
        }
    };
    if let Err(e) = fs::create_dir_all(&bus_dir) {
        tracing::warn!("failed to create {}: {e}", bus_dir.display());
        return;
    }

    let content = format!(
        "# This file is created by keytao-ime (IBus compatible)\nIBUS_ADDRESS={dbus_address}\nIBUS_DAEMON_PID={pid}\n"
    );

    let display_num = display_number();
    let wayland_num = wayland_display_number();

    let mut names = vec![
        format!("{machine_id}-unix-{display_num}"),
        format!("{machine_id}-unix-wayland-0"),
        format!("{machine_id}-unix-wayland-1"),
    ];
    if let Some(wn) = wayland_num {
        names.push(format!("{machine_id}-unix-wayland-{wn}"));
    }
    names.sort();
    names.dedup();

    for name in names {
        let path = bus_dir.join(&name);
        if let Err(e) = fs::write(&path, &content) {
            tracing::warn!("failed to write {}: {e}", path.display());
        } else {
            tracing::debug!("wrote IBus address file: {}", path.display());
        }
    }
}

fn session_bus_address() -> String {
    let uid = unsafe { libc::geteuid() };
    session_bus_address_from(
        std::env::var("DBUS_SESSION_BUS_ADDRESS").ok(),
        std::env::var("XDG_RUNTIME_DIR").ok(),
        uid,
    )
}

fn session_bus_address_from(
    dbus_session_bus_address: Option<String>,
    xdg_runtime_dir: Option<String>,
    uid: u32,
) -> String {
    dbus_session_bus_address
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            xdg_runtime_dir
                .filter(|value| !value.trim().is_empty())
                .map(|runtime_dir| format!("unix:path={runtime_dir}/bus"))
        })
        .unwrap_or_else(|| format!("unix:path=/run/user/{uid}/bus"))
}

fn read_machine_id() -> String {
    for path in &["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        if let Ok(s) = fs::read_to_string(path) {
            let id = s.trim().to_owned();
            if !id.is_empty() {
                return id;
            }
        }
    }
    "unknown".to_owned()
}

fn display_number() -> u32 {
    std::env::var("DISPLAY")
        .ok()
        .and_then(|d| {
            d.rsplit(':')
                .next()
                .and_then(|s| s.split('.').next())
                .and_then(|s| s.parse().ok())
        })
        .unwrap_or(0)
}

fn wayland_display_number() -> Option<u32> {
    std::env::var("WAYLAND_DISPLAY")
        .ok()
        .and_then(|d| d.rsplit('-').next().and_then(|s| s.parse().ok()))
}

// ── Public entry point ────────────────────────────────────────────────────────

pub async fn run(engine: CoreEngine) {
    tracing::info!("IBus D-Bus backend starting");

    let builder = match connection::Builder::session() {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("IBus: failed to get session bus builder: {e}");
            return;
        }
    };
    let engine_clone = engine.clone();
    let theme_resolver = Arc::new(ThemeResolver::from_default_locations());
    let theme_resolver_clone = theme_resolver.clone();
    let builder = match builder.serve_at("/org/kde/kimpanel/inputmethod", Kimpanel) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("Failed to serve Kimpanel: {e}");
            return;
        }
    };

    let (tx, rx) = std::sync::mpsc::channel::<ImePanelMessage>();
    let tx_clone = tx.clone();
    std::thread::spawn(move || {
        let mut panel = X11Panel::new();
        let mut mode_hint_until: Option<Instant> = None;
        loop {
            let msg = match mode_hint_until {
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        if let Some(panel) = panel.as_mut() {
                            panel.hide();
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
                    ImePanelMessage::Show { state, x, y } => {
                        mode_hint_until = None;
                        panel.show(&state, x, y);
                    }
                    ImePanelMessage::ModeHint { ascii_mode, x, y } => {
                        mode_hint_until = panel.show_mode_hint(ascii_mode, x, y);
                    }
                    ImePanelMessage::Hide => {
                        mode_hint_until = None;
                        panel.hide();
                    }
                }
            }
        }
    });

    let builder = match builder.serve_at(
        "/org/freedesktop/IBus",
        IBusBus {
            engine,
            ctx_counter: Arc::new(AtomicU32::new(1)),
            kimpanel_ctxt: None, // Will fill after build
            x11_panel_tx: tx,
            theme_resolver,
        },
    ) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("IBus: failed to serve_at: {e}");
            return;
        }
    };

    let conn = match builder.build().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("IBus D-Bus backend failed to connect: {e}");
            return;
        }
    };

    if let Err(e) = conn.request_name("org.freedesktop.IBus").await {
        tracing::error!("IBus: failed to request IBus name: {e}");
        return;
    }

    if let Err(e) = conn.request_name("org.kde.kimpanel.inputmethod").await {
        tracing::warn!("Kimpanel: failed to request Kimpanel name (running as secondary?): {e}");
    }

    let dbus_address = session_bus_address();
    write_ibus_address_files(&dbus_address);

    let kimpanel_ctxt = SignalContext::new(&conn, "/org/kde/kimpanel/inputmethod").ok();

    // We need to update the IBusBus instance with the kimpanel_ctxt.
    // However, IBusBus is owned by the ObjectServer. Instead of mutating it, we just set
    // it properly before serving if possible, or use a shared state.
    // Actually, we can just create the SignalContext from `conn` and share it!

    // Let's re-register IBusBus with the valid kimpanel_ctxt.
    let _ = conn
        .object_server()
        .remove::<IBusBus, _>("/org/freedesktop/IBus")
        .await;
    let _ = conn
        .object_server()
        .at(
            "/org/freedesktop/IBus",
            IBusBus {
                engine: engine_clone,
                ctx_counter: Arc::new(AtomicU32::new(1)),
                kimpanel_ctxt,
                x11_panel_tx: tx_clone,
                theme_resolver: theme_resolver_clone,
            },
        )
        .await;

    // Notify any already-connected IBus clients that the keytao engine is active.
    // Chromium/CEF clients that connected before this signal can use GetGlobalEngine instead.
    if let Ok(signal_ctx) = SignalContext::new(&conn, "/org/freedesktop/IBus") {
        IBusBus::global_engine_changed(&signal_ctx, "keytao")
            .await
            .ok();
    }

    tracing::info!("IBus D-Bus backend ready ({})", dbus_address);
    let _conn = conn; // keep connection alive

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::{highlighted_candidate_index, session_bus_address_from};
    use keytao_core::{Candidate, ImeState};

    #[test]
    fn empty_composition_backspace_bypasses_to_client() {
        let state = ImeState::empty();
        assert!(key_policy::should_bypass_empty_composition(
            0xff08, 0x10, &state
        ));
    }

    #[test]
    fn highlighted_candidate_requires_candidates() {
        let state = ImeState::empty();
        assert_eq!(highlighted_candidate_index(&state), None);

        let mut state = ImeState::empty();
        state.candidates = vec![Candidate {
            text: "first".to_owned(),
            comment: None,
        }];
        state.highlighted_candidate_index = 9;
        assert_eq!(highlighted_candidate_index(&state), Some(0));
    }

    #[test]
    fn session_bus_address_uses_current_uid_fallback() {
        assert_eq!(
            session_bus_address_from(None, None, 501),
            "unix:path=/run/user/501/bus"
        );
        assert_eq!(
            session_bus_address_from(None, Some("/run/user/502".to_owned()), 501),
            "unix:path=/run/user/502/bus"
        );
        assert_eq!(
            session_bus_address_from(Some("unix:path=/tmp/bus".to_owned()), None, 501),
            "unix:path=/tmp/bus"
        );
    }
}
