//! X11 backend: XIM server (@server=keytao) + XCB candidate overlay window.
//!
//! Set XMODIFIERS=@im=keytao in your session before launching apps.
//! The server registers under the name "keytao"; XIM clients that respect
//! XMODIFIERS will connect automatically.

use std::{os::fd::AsRawFd, sync::Arc, time::Instant};

use keytao_core::{key_policy, ImeState, RIME_MOD_SHIFT, RIME_RELEASE_MASK};
use x11rb::{
    connection::Connection as _,
    protocol::{
        xproto::{
            AtomEnum, ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, EventMask, Gcontext,
            ImageFormat, PropMode, Window, WindowClass,
        },
        Event,
    },
    wrapper::ConnectionExt as _,
    xcb_ffi::XCBConnection,
};
use xim::{
    x11rb::X11rbServer, InputContext, InputStyle, Server, ServerError, ServerHandler,
    UserInputContext, XimConnections,
};

use crate::{
    engine::{CoreEngine, ImeSession},
    panel::{load_font, PanelRenderer},
    reload_bus,
};

/// X11 core protocol event numbers.
const X_KEY_PRESS: u8 = 2;
const X_KEY_RELEASE: u8 = 3;
/// `XNFilterEvents` mask bits: `KeyPressMask | KeyReleaseMask`.  Rime's
/// `ascii_composer` needs the release to see a solo `Shift`.
const XIM_FILTER_KEY_EVENTS: u32 = 1 | 2;
/// Gap between the caret and the top of the candidate window, in pixels.
const PANEL_CARET_GAP: i32 = 4;

fn commit_text(server: &mut MyServer, ic: &InputContext, text: &str) -> Result<(), ServerError> {
    tracing::debug!("XIM commit text: {} chars", text.chars().count());
    tracing::trace!("XIM commit text: {text:?}");
    server.commit(ic, text)
}

fn is_shift_key(sym: u32) -> bool {
    sym == 0xffe1 || sym == 0xffe2
}

/// Pick a keysym out of a `GetKeyboardMapping` reply.
///
/// X11 pads the unused levels of a group with `NoSymbol`, and the core protocol
/// says such a group behaves as if the missing entry repeated the first one.
/// Modifier keys are the common case: `Shift_L` has no shifted level, so a
/// `Shift` release — which carries `ShiftMask` in `state` — would otherwise
/// resolve to `NoSymbol` and never reach librime's `ascii_composer`.
fn keysym_at_level(
    keysyms: &[u32],
    min_keycode: u8,
    keysyms_per_keycode: u8,
    keycode: u8,
    shift: bool,
) -> u32 {
    let kc = keycode as usize;
    let min = min_keycode as usize;
    let per = keysyms_per_keycode as usize;
    if kc < min || per == 0 {
        return 0;
    }
    let base = (kc - min) * per;
    let unshifted = keysyms.get(base).copied().unwrap_or(0);
    if !shift || per < 2 {
        return unshifted;
    }
    match keysyms.get(base + 1).copied().unwrap_or(0) {
        0 => unshifted,
        shifted => shifted,
    }
}

/// Whether the client draws the preedit itself.
///
/// Only on-the-spot (`PREEDIT_CALLBACKS`) clients do.  Over-the-spot
/// (`PREEDIT_POSITION`) clients only report the caret through
/// `XNSpotLocation`; the input method still draws the preedit in its own
/// window, so `preedit_draw` must not be sent to them.
fn client_supports_preedit(ic: &InputContext) -> bool {
    ic.input_style().contains(InputStyle::PREEDIT_CALLBACKS)
}

fn draw_client_preedit(
    server: &mut MyServer,
    ic: &mut InputContext,
    text: &str,
) -> Result<(), ServerError> {
    if client_supports_preedit(ic) {
        server.preedit_draw(ic, text)?;
    }
    Ok(())
}

// ── IC per-context data ───────────────────────────────────────────────────────

// Spot location is stored directly on InputContext by xim (via preedit_spot()).
struct IcData {
    session: ImeSession,
    /// Last state drawn for this context, so a spot update can move the panel
    /// without waiting for the next key.
    last_state: Option<ImeState>,
    /// Reload generation `last_state` was produced under.  A spot update must
    /// not repaint candidates that a librime reload has already invalidated.
    state_epoch: u64,
}

// ── Main handler type ─────────────────────────────────────────────────────────

type MyServer = X11rbServer<Arc<XCBConnection>>;

struct KeyTaoHandler {
    engine: CoreEngine,
    renderer: Option<PanelRenderer>,
    conn: Arc<XCBConnection>,
    root: Window,
    panel_win: Window,
    panel_depth: u8,
    gc: Gcontext,
    panel_visible: bool,
    // Keycode → keysym table fetched from the X11 server at init time.
    // Layout: flat array, each keycode has `keysyms_per_keycode` slots.
    // keysym index 0 = unshifted, 1 = shifted.
    keycode_map: Vec<u32>,
    min_keycode: u8,
    keysyms_per_keycode: u8,
    screen_width: i32,
    screen_height: i32,
    ascii_mode: bool,
    mode_hint_until: Option<Instant>,
    /// Root coordinates the panel was last anchored at, used to place the mode
    /// hint when there is no composition to derive a spot from.
    last_anchor: (i32, i32),
    /// Bumped on every librime reload, so per-context state drawn from the
    /// previous dictionaries can be recognised and dropped.
    reload_epoch: u64,
}

impl KeyTaoHandler {
    fn new(engine: CoreEngine, conn: Arc<XCBConnection>, screen_num: usize) -> Self {
        let renderer = load_font().and_then(PanelRenderer::new_x11);
        let setup = conn.setup();
        let screen = &setup.roots[screen_num];
        let root = screen.root;
        let visual = screen.root_visual;
        let depth = screen.root_depth;
        let screen_width = screen.width_in_pixels as i32;
        let screen_height = screen.height_in_pixels as i32;

        let panel_win = conn.generate_id().expect("gen panel window id");
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
        .expect("create panel window");

        let class = b"keytao-candidate\0keytao-candidate\0";
        conn.change_property8(
            PropMode::REPLACE,
            panel_win,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            class,
        )
        .ok();

        let gc = conn.generate_id().expect("gen gc");
        conn.create_gc(gc, panel_win, &Default::default()).ok();
        conn.flush().ok();

        // Fetch keycode→keysym mapping from the X11 server.
        // This lets us convert raw XIM keycodes to keysyms without needing XKB.
        let setup = conn.setup();
        let min_keycode = setup.min_keycode;
        let max_keycode = setup.max_keycode;
        let count = (max_keycode - min_keycode) as u8 + 1;

        let (keycode_map, keysyms_per_keycode) = conn
            .get_keyboard_mapping(min_keycode, count)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map(|reply| {
                let kpk = reply.keysyms_per_keycode;
                let syms: Vec<u32> = reply.keysyms.iter().map(|&s| s).collect();
                (syms, kpk)
            })
            .unwrap_or_else(|| {
                tracing::warn!("GetKeyboardMapping failed; keysym lookup disabled");
                (Vec::new(), 1)
            });

        Self {
            engine,
            renderer,
            conn,
            root,
            panel_win,
            panel_depth: depth,
            gc,
            panel_visible: false,
            keycode_map,
            min_keycode,
            keysyms_per_keycode,
            screen_width,
            screen_height,
            ascii_mode: false,
            mode_hint_until: None,
            last_anchor: (0, 0),
            reload_epoch: 0,
        }
    }

    /// Drop the candidate window a librime reload just invalidated.
    ///
    /// The per-context sessions rebuild themselves lazily through the runtime
    /// generation, but the pixels on screen and the `last_state` each context
    /// keeps for spot updates come from the dictionaries that were replaced.
    fn handle_reload(&mut self) {
        tracing::info!("XIM: hiding candidates after librime reload");
        self.reload_epoch = self.reload_epoch.wrapping_add(1);
        self.hide_panel();
    }

    /// Where the candidate window belongs, in root coordinates.
    ///
    /// Over-the-spot clients keep `XNSpotLocation` on the caret, so the spot is
    /// exact.  Root-window clients never set it and the spot stays at the
    /// window origin, which is the best anchor XIM offers for them.
    fn anchor_point(&self, user_ic: &UserInputContext<IcData>) -> (i32, i32) {
        let spot = user_ic.ic.preedit_spot();
        let anchor = user_ic
            .ic
            .app_focus_win()
            .or_else(|| user_ic.ic.app_win())
            .map(|window| window.get())
            .unwrap_or_else(|| user_ic.ic.client_win());
        self.conn
            .translate_coordinates(anchor, self.root, spot.x, spot.y)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map(|reply| (reply.dst_x as i32, reply.dst_y as i32))
            .unwrap_or((spot.x as i32, spot.y as i32))
    }

    /// Keep the whole panel on screen: a caret near the right or bottom edge
    /// would otherwise push half of it out of view.
    fn clamp_to_screen(&self, x: i32, y: i32, w: u32, h: u32) -> (i32, i32) {
        let max_x = (self.screen_width - w as i32).max(0);
        let max_y = (self.screen_height - h as i32).max(0);
        (x.clamp(0, max_x), y.clamp(0, max_y))
    }

    fn draw_panel(&mut self, pixels: &[u8], w: u32, h: u32, x: i32, y: i32) {
        if w == 0 || h == 0 {
            return;
        }
        let (x, y) = self.clamp_to_screen(x, y, w, h);
        self.conn
            .configure_window(
                self.panel_win,
                &ConfigureWindowAux::new().x(x).y(y).width(w).height(h),
            )
            .ok();

        if !self.panel_visible {
            self.conn.map_window(self.panel_win).ok();
            self.panel_visible = true;
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
                self.panel_depth,
                pixels,
            )
            .ok();
        self.conn.flush().ok();
    }

    /// Resolve an X11 keycode to a keysym at the requested Shift level.
    ///
    /// X11 pads unused levels of a group with `NoSymbol`, and the core protocol
    /// says such a group behaves as if the missing entry repeated the first
    /// one.  Modifier keys are the common case: `Shift_L` has no shifted level,
    /// so a `Shift` release — which carries `ShiftMask` in `state` — would
    /// otherwise resolve to `NoSymbol` and never reach `ascii_composer`.
    fn keysym_for(&self, keycode: u8, shift: bool) -> u32 {
        if self.keycode_map.is_empty() {
            // Fallback: broken, but better than crashing.
            return keycode as u32;
        }
        keysym_at_level(
            &self.keycode_map,
            self.min_keycode,
            self.keysyms_per_keycode,
            keycode,
            shift,
        )
    }

    fn mode_hint_active(&self) -> bool {
        self.mode_hint_until
            .is_some_and(|deadline| Instant::now() < deadline)
    }

    fn show_panel(&mut self, user_ic: &UserInputContext<IcData>, state: &ImeState) {
        // Under both offered styles the preedit lives in this window, so a
        // composition without candidates still has to be visible.
        if state.preedit.is_empty() && state.candidates.is_empty() {
            if !self.mode_hint_active() {
                self.hide_panel();
            }
            return;
        }
        let Some(renderer) = &self.renderer else {
            return;
        };
        let (pixels, w, h) = renderer.render(state);
        let (root_x, root_y) = self.anchor_point(user_ic);
        self.last_anchor = (root_x, root_y);
        // The panel now holds a composition, so the hint deadline must not be
        // allowed to unmap it later on.
        self.mode_hint_until = None;
        self.draw_panel(&pixels, w, h, root_x, root_y + PANEL_CARET_GAP);
    }

    /// Show the same 中/英 hint the other backends draw when Rime switches.
    fn show_mode_hint(&mut self, ascii_mode: bool) {
        let Some(renderer) = &self.renderer else {
            return;
        };
        let (pixels, w, h) = renderer.render_mode_hint(ascii_mode);
        let duration = renderer.mode_hint_duration();
        let (x, y) = self.last_anchor;
        self.draw_panel(&pixels, w, h, x - w as i32 / 2, y + PANEL_CARET_GAP);
        self.mode_hint_until = Some(Instant::now() + duration);
    }

    fn update_ascii_mode(&mut self, ascii_mode: bool) {
        if ascii_mode == self.ascii_mode {
            return;
        }
        self.ascii_mode = ascii_mode;
        tracing::debug!("XIM mode changed: {}", if ascii_mode { "EN" } else { "CN" });
        self.show_mode_hint(ascii_mode);
    }

    /// Milliseconds until the mode hint expires, or -1 when none is showing.
    fn poll_timeout_ms(&self) -> i32 {
        let Some(deadline) = self.mode_hint_until else {
            return -1;
        };
        let now = Instant::now();
        if deadline <= now {
            return 0;
        }
        (deadline - now).as_millis().min(i32::MAX as u128) as i32
    }

    fn expire_mode_hint(&mut self) {
        if self
            .mode_hint_until
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.mode_hint_until = None;
            self.hide_panel();
        }
    }

    fn hide_panel(&mut self) {
        self.mode_hint_until = None;
        if self.panel_visible {
            self.conn.unmap_window(self.panel_win).ok();
            self.conn.flush().ok();
            self.panel_visible = false;
        }
    }
}

// ── ServerHandler impl ────────────────────────────────────────────────────────

impl ServerHandler<MyServer> for KeyTaoHandler {
    type InputContextData = IcData;
    type InputStyleArray = [InputStyle; 3];

    fn new_ic_data(
        &mut self,
        _server: &mut MyServer,
        style: InputStyle,
    ) -> Result<IcData, ServerError> {
        tracing::info!("XIM NewICData style={style:?}");
        let session = self
            .engine
            .create_session()
            .map_err(ServerError::Internal)?;
        Ok(IcData {
            session,
            last_state: None,
            state_epoch: self.reload_epoch,
        })
    }

    fn input_styles(&self) -> Self::InputStyleArray {
        [
            // Over-the-spot: the client reports the caret through
            // XNSpotLocation and keytao draws the preedit in its own window,
            // which is what lets the candidate window follow the cursor.
            InputStyle::PREEDIT_POSITION | InputStyle::STATUS_NOTHING,
            // Root-window fallback for clients that do not track the spot.
            // On-the-spot (PREEDIT_CALLBACKS) is deliberately not offered:
            // Electron/Chromium X11 clients can get stuck when XIM drives an
            // in-client preedit region.
            InputStyle::PREEDIT_NOTHING | InputStyle::STATUS_NOTHING,
            InputStyle::PREEDIT_NONE | InputStyle::STATUS_NONE,
        ]
    }

    fn filter_events(&self) -> u32 {
        XIM_FILTER_KEY_EVENTS
    }

    fn handle_connect(&mut self, _server: &mut MyServer) -> Result<(), ServerError> {
        tracing::info!("XIM client connected");
        Ok(())
    }

    fn handle_create_ic(
        &mut self,
        server: &mut MyServer,
        user_ic: &mut UserInputContext<IcData>,
    ) -> Result<(), ServerError> {
        tracing::info!(
            "XIM CreateIC client_win={} style={:?}",
            user_ic.ic.client_win(),
            user_ic.ic.input_style()
        );
        server.set_event_mask(&user_ic.ic, XIM_FILTER_KEY_EVENTS, 0)
    }

    fn handle_destroy_ic(
        &mut self,
        _server: &mut MyServer,
        user_ic: UserInputContext<IcData>,
    ) -> Result<(), ServerError> {
        tracing::info!("XIM DestroyIC client_win={}", user_ic.ic.client_win());
        user_ic.user_data.session.clear_composition();
        self.hide_panel();
        Ok(())
    }

    fn handle_reset_ic(
        &mut self,
        _server: &mut MyServer,
        user_ic: &mut UserInputContext<IcData>,
    ) -> Result<String, ServerError> {
        tracing::info!("XIM ResetIC client_win={}", user_ic.ic.client_win());
        user_ic.user_data.session.clear_composition();
        user_ic.user_data.last_state = None;
        self.hide_panel();
        Ok(String::new())
    }

    fn handle_set_ic_values(
        &mut self,
        _server: &mut MyServer,
        user_ic: &mut UserInputContext<IcData>,
    ) -> Result<(), ServerError> {
        // xim stores SpotLocation internally; read via user_ic.ic.preedit_spot().
        // Over-the-spot clients move the caret while composing, so follow it
        // instead of waiting for the next key.
        if user_ic.user_data.state_epoch != self.reload_epoch {
            // A reload replaced the dictionaries this state came from; moving
            // the caret must not bring those candidates back.
            user_ic.user_data.last_state = None;
            return Ok(());
        }
        if let Some(state) = user_ic.user_data.last_state.take() {
            self.show_panel(user_ic, &state);
            user_ic.user_data.last_state = Some(state);
        }
        Ok(())
    }

    fn handle_set_focus(
        &mut self,
        _server: &mut MyServer,
        user_ic: &mut UserInputContext<IcData>,
    ) -> Result<(), ServerError> {
        tracing::info!("XIM SetFocus client_win={}", user_ic.ic.client_win());
        Ok(())
    }

    fn handle_unset_focus(
        &mut self,
        _server: &mut MyServer,
        user_ic: &mut UserInputContext<IcData>,
    ) -> Result<(), ServerError> {
        tracing::info!("XIM UnsetFocus client_win={}", user_ic.ic.client_win());
        // The composition never reached the client, so dropping it is the
        // "discard" half of the shared focus-loss contract.
        user_ic.user_data.session.clear_composition();
        user_ic.user_data.last_state = None;
        self.hide_panel();
        Ok(())
    }

    fn handle_forward_event(
        &mut self,
        server: &mut MyServer,
        user_ic: &mut UserInputContext<IcData>,
        xev: &x11rb::protocol::xproto::KeyPressEvent,
    ) -> Result<bool, ServerError> {
        // Convert the X11 hardware keycode to a keysym using the keyboard mapping
        // fetched at init time.  xev.detail is the raw X11 keycode and the Shift
        // bit (bit 0) of xev.state picks the keysym level.
        let shift = u32::from(xev.state) & 0x0001 != 0;
        let keysym = self.keysym_for(xev.detail, shift);

        if keysym == 0 {
            return Ok(false);
        }

        let mods = u32::from(xev.state);

        // KeyRelease is subscribed only so that librime's ascii_composer sees a
        // solo Shift and can toggle Chinese/English; the client keeps the event.
        match xev.response_type & 0x7f {
            X_KEY_PRESS => {}
            X_KEY_RELEASE => {
                if is_shift_key(keysym) {
                    if let Some(result) = user_ic
                        .user_data
                        .session
                        .process_key_result(keysym, RIME_RELEASE_MASK)
                    {
                        self.update_ascii_mode(result.state.ascii_mode);
                    }
                }
                return Ok(false);
            }
            other => {
                tracing::trace!("XIM ForwardEvent: ignoring event type {other}");
                return Ok(false);
            }
        }

        tracing::trace!(
            "XIM ForwardEvent client_win={} keycode={} keysym={keysym:#x} mods={mods:#x}",
            user_ic.ic.client_win(),
            xev.detail
        );

        // Shift held down while typing is a normal modifier; Shift on its own
        // must not carry its own bit into the press event.
        let effective_mods = if is_shift_key(keysym) {
            mods & !RIME_MOD_SHIFT
        } else {
            mods
        };

        let before_state = user_ic.user_data.session.state();
        if key_policy::should_bypass_empty_composition(keysym, effective_mods, &before_state) {
            user_ic.user_data.last_state = None;
            self.hide_panel();
            draw_client_preedit(server, &mut user_ic.ic, "")?;
            return Ok(false);
        }

        // Return belongs to librime: only the schema's editor knows whether it
        // confirms the highlighted candidate or commits the raw code.  Space
        // and the digits are ordinary keys for the same reason.
        let result = if key_policy::is_enter_key(keysym)
            && key_policy::enter_action(&before_state) == key_policy::EnterAction::ForwardToRime
        {
            user_ic.user_data.session.process_enter()
        } else {
            user_ic
                .user_data
                .session
                .process_key_result(keysym, effective_mods)
        };
        let Some(result) = result else {
            return Ok(false);
        };

        let ime_state = result.state;
        let consumed = result.accepted;

        self.update_ascii_mode(ime_state.ascii_mode);

        if let Some(text) = &ime_state.committed {
            draw_client_preedit(server, &mut user_ic.ic, "")?;
            commit_text(server, &user_ic.ic, text)?;
        }

        if !ime_state.preedit.is_empty() {
            draw_client_preedit(server, &mut user_ic.ic, &ime_state.preedit)?;
        } else if ime_state.committed.is_some() {
            draw_client_preedit(server, &mut user_ic.ic, "")?;
        }

        self.show_panel(user_ic, &ime_state);
        user_ic.user_data.last_state = Some(ime_state);
        user_ic.user_data.state_epoch = self.reload_epoch;

        Ok(consumed)
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn run(engine: CoreEngine) {
    // XWayland starts lazily in niri: DISPLAY may be set in the environment but
    // the actual server socket may not exist yet.  Retry until it is available.
    let (conn, screen_num) = loop {
        match XCBConnection::connect(None) {
            Ok(pair) => break pair,
            Err(e) => {
                tracing::debug!(
                    "X11 connect failed ({e}), retrying in 1 s (XWayland not ready yet?)"
                );
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
    };
    let conn = Arc::new(conn);

    let server = match X11rbServer::init(Arc::clone(&conn), screen_num, "keytao", xim::ALL_LOCALES)
    {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("XIM server init failed: {e} (another XIM server named 'keytao' may already be running)");
            return;
        }
    };

    let mut server = server;
    let mut connections = XimConnections::new();
    let mut handler = KeyTaoHandler::new(engine, Arc::clone(&conn), screen_num);

    let reload_signal = reload_bus::subscribe();

    tracing::info!("X11 XIM server running as @server=keytao");
    tracing::info!("Set XMODIFIERS=@im=keytao in your session to use this IME");

    loop {
        // Drain everything XCB already buffered before going back to sleep,
        // otherwise poll() would block on a socket that has nothing new.
        loop {
            match conn.poll_for_event() {
                Ok(Some(event)) => {
                    if let Err(e) = server.filter_event(&event, &mut connections, &mut handler) {
                        tracing::error!("XIM error: {e}");
                    }
                    if let Event::Expose(ev) = &event {
                        // Candidate window was covered and re-exposed.
                        // The next key event will re-render; nothing to do here.
                        let _ = ev;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::error!("X11 connection error: {e}");
                    return;
                }
            }
        }

        handler.expire_mode_hint();
        if let Err(e) = conn.flush() {
            tracing::warn!("X11 flush error: {e}");
        }

        let mut pfds = vec![libc::pollfd {
            fd: conn.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        }];
        if let Some(signal) = &reload_signal {
            pfds.push(libc::pollfd {
                fd: signal.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            });
        }
        let timeout_ms = handler.poll_timeout_ms();
        let ready =
            unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, timeout_ms) };
        if ready < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            tracing::error!("X11 poll failed: {err}");
            break;
        }
        if let Some(signal) = &reload_signal {
            if signal.take() {
                handler.handle_reload();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{keysym_at_level, XIM_FILTER_KEY_EVENTS};

    /// `min_keycode = 8`, two keysyms per keycode:
    ///   8 → `a` / `A`, 9 → `Shift_L` / NoSymbol, 10 → `1` / `exclam`.
    const MAP: &[u32] = &[0x061, 0x041, 0xffe1, 0x0000, 0x031, 0x021];

    #[test]
    fn a_missing_shift_level_falls_back_to_the_base_keysym() {
        // Shift_L's release carries ShiftMask, and its group has no shifted
        // level; resolving to NoSymbol there would hide solo Shift from Rime.
        assert_eq!(keysym_at_level(MAP, 8, 2, 9, true), 0xffe1);
        assert_eq!(keysym_at_level(MAP, 8, 2, 9, false), 0xffe1);
    }

    #[test]
    fn present_shift_levels_are_used() {
        assert_eq!(keysym_at_level(MAP, 8, 2, 8, false), 0x061);
        assert_eq!(keysym_at_level(MAP, 8, 2, 8, true), 0x041);
        assert_eq!(keysym_at_level(MAP, 8, 2, 10, true), 0x021);
    }

    #[test]
    fn out_of_range_keycodes_resolve_to_nosymbol() {
        assert_eq!(keysym_at_level(MAP, 8, 2, 7, false), 0);
        assert_eq!(keysym_at_level(MAP, 8, 2, 200, false), 0);
        assert_eq!(keysym_at_level(MAP, 8, 0, 8, false), 0);
    }

    #[test]
    fn key_release_is_subscribed_for_solo_shift() {
        // Bit 0 is KeyPress, bit 1 is KeyRelease; librime's ascii_composer only
        // sees a solo Shift if the release is filtered to the input method.
        assert_eq!(XIM_FILTER_KEY_EVENTS & 1, 1);
        assert_eq!(XIM_FILTER_KEY_EVENTS & 2, 2);
    }
}
