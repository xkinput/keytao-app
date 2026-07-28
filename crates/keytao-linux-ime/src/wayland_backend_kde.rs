//! KDE/KWin Wayland backend: zwp_input_method_unstable_v1.
//!
//! KWin 6 still exposes input methods through input-method-v1. Applications talk
//! text-input-v1/v2/v3 to KWin; the IME process talks input-method-v1 to KWin.

use std::{
    collections::HashSet,
    fs::File,
    io::Write,
    os::fd::{AsFd, AsRawFd},
    time::{Duration, Instant},
};

use keytao_core::{
    key_policy, ImeState, InputContextPolicy, RIME_MOD_ALT, RIME_MOD_CONTROL, RIME_MOD_SHIFT,
    RIME_RELEASE_MASK,
};
use wayland_client::{
    delegate_noop,
    globals::{registry_queue_init, GlobalListContents},
    protocol::{
        wl_buffer::WlBuffer,
        wl_compositor::WlCompositor,
        wl_keyboard::{self, WlKeyboard},
        wl_region::WlRegion,
        wl_registry,
        wl_seat::WlSeat,
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
        wl_surface::WlSurface,
    },
    Connection, Dispatch, Proxy, QueueHandle, WEnum,
};
use wayland_protocols::wp::input_method::zv1::client::{
    zwp_input_method_context_v1::{self, ZwpInputMethodContextV1},
    zwp_input_method_v1::{self, ZwpInputMethodV1},
    zwp_input_panel_surface_v1::ZwpInputPanelSurfaceV1,
    zwp_input_panel_v1::ZwpInputPanelV1,
};
use xkbcommon::xkb;

use crate::{
    engine::{CoreEngine, ImeSession},
    kimpanel::KimpanelHandle,
    panel::{load_font, PanelRenderer},
    reload_bus,
};

const DEACTIVATE_DEBOUNCE: Duration = Duration::from_millis(180);

/// `zwp_text_input_v1::content_purpose::password`.  Note that v1 has no `pin`
/// and numbers its later purposes one lower than `zwp_text_input_v3`, so the
/// v3/IBus tables must not be reused here.
const TEXT_INPUT_V1_PURPOSE_PASSWORD: u32 = 8;
/// `zwp_text_input_v1::content_hint`: `hidden_text | sensitive_data`, which is
/// also what the protocol's `password` shorthand (0xc0) expands to.
const TEXT_INPUT_V1_HINT_SECRET: u32 = 0x40 | 0x80;

/// `wl_keyboard` only gained `repeat_info` in version 4, and the keyboard KWin
/// hands out through `zwp_input_method_context_v1::grab_keyboard` is version 1.
/// Nothing will ever report the user's settings there, so keys the IME consumed
/// would simply never repeat.  These are the xkb/Plasma defaults.
const WL_KEYBOARD_REPEAT_INFO_SINCE: u32 = 4;
const FALLBACK_REPEAT_RATE: i32 = 25;
const FALLBACK_REPEAT_DELAY: i32 = 600;

fn is_shift_key(sym: u32) -> bool {
    sym == xkb::keysyms::KEY_Shift_L || sym == xkb::keysyms::KEY_Shift_R
}

/// The session policy a `zwp_input_method_context_v1` content type maps to.
fn content_type_policy(hint: u32, purpose: u32) -> InputContextPolicy {
    if purpose == TEXT_INPUT_V1_PURPOSE_PASSWORD || hint & TEXT_INPUT_V1_HINT_SECRET != 0 {
        InputContextPolicy::sensitive()
    } else {
        InputContextPolicy::default()
    }
}

/// `zwp_input_method_context_v1` preedit indices are byte offsets into the
/// preedit string, while `ImeState::cursor` counts Unicode scalars.
fn preedit_cursor_bytes(preedit: &str, cursor: usize) -> i32 {
    preedit
        .char_indices()
        .nth(cursor)
        .map(|(offset, _)| offset)
        .unwrap_or(preedit.len()) as i32
}

/// Turn a `repeat_info` event into (initial delay, interval between repeats).
///
/// `rate` is in keys per second and zero disables repeating entirely.  On KDE
/// this usually comes from the fallback above rather than from a `repeat_info`
/// event, because the grabbed keyboard predates that event.
fn repeat_timings(rate: i32, delay: i32) -> Option<(Duration, Duration)> {
    if rate <= 0 || delay < 0 {
        return None;
    }
    Some((
        Duration::from_millis(delay as u64),
        Duration::from_micros(1_000_000 / rate as u64),
    ))
}

/// A key the IME consumed and now has to repeat itself, since the grab took it
/// away from the compositor.
struct KeyRepeat {
    key: u32,
    next: Instant,
    interval: Duration,
}

struct App {
    session: ImeSession,
    seat: Option<WlSeat>,
    input_method: Option<ZwpInputMethodV1>,
    context: Option<ZwpInputMethodContextV1>,
    keyboard: Option<WlKeyboard>,
    serial: u32,
    active: bool,
    deactivate_deadline: Option<Instant>,
    mode_hint_until: Option<Instant>,
    xkb_context: xkb::Context,
    xkb_keymap: Option<xkb::Keymap>,
    xkb_state: Option<xkb::State>,
    mods: u32,
    last_key_time: u32,
    /// Serial of the last `wl_keyboard::key`; `context.key` has to echo that
    /// one back, not the `commit_state` serial.
    last_key_serial: u32,
    /// Keys whose press was handed to the client, so their release can be too.
    forwarded_keys: HashSet<u32>,
    key_repeat: Option<KeyRepeat>,
    repeat_delay: Option<Duration>,
    repeat_interval: Option<Duration>,
    ascii_mode: bool,
    kimpanel: Option<KimpanelHandle>,
    pending_kimpanel_state: Option<ImeState>,
    /// Conversion mode waiting to be pushed to the Kimpanel indicator; the
    /// Kimpanel calls are async, so the event loop drains this.
    pending_kimpanel_mode: Option<bool>,
    clear_kimpanel: bool,
    globals_seen: Vec<String>,

    renderer: Option<PanelRenderer>,
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    input_panel: Option<ZwpInputPanelV1>,
    panel_surface: Option<WlSurface>,
    panel_popup: Option<ZwpInputPanelSurfaceV1>,
    panel_buffer: Option<WlBuffer>,
    panel_visible: bool,
    ime_state: Option<ImeState>,
}

impl App {
    fn new(session: ImeSession, kimpanel: Option<KimpanelHandle>) -> Self {
        let renderer = load_font().and_then(PanelRenderer::new);
        Self {
            session,
            seat: None,
            input_method: None,
            context: None,
            keyboard: None,
            serial: 0,
            active: false,
            deactivate_deadline: None,
            mode_hint_until: None,
            xkb_context: xkb::Context::new(xkb::CONTEXT_NO_FLAGS),
            xkb_keymap: None,
            xkb_state: None,
            mods: 0,
            last_key_time: 0,
            last_key_serial: 0,
            forwarded_keys: HashSet::new(),
            key_repeat: None,
            repeat_delay: None,
            repeat_interval: None,
            ascii_mode: false,
            kimpanel,
            pending_kimpanel_state: None,
            pending_kimpanel_mode: None,
            clear_kimpanel: true,
            globals_seen: Vec::new(),

            renderer,
            compositor: None,
            shm: None,
            input_panel: None,
            panel_surface: None,
            panel_popup: None,
            panel_buffer: None,
            panel_visible: false,
            ime_state: None,
        }
    }

    fn key_sym(&self, evdev_keycode: u32) -> u32 {
        let keycode = evdev_keycode + 8;
        self.xkb_state
            .as_ref()
            .map(|s| s.key_get_one_sym(xkb::Keycode::from(keycode)))
            .unwrap_or(xkb::Keysym::from(xkb::keysyms::KEY_NoSymbol))
            .into()
    }

    /// Give the context and its keyboard grab back to KWin.
    ///
    /// input-method-v1 says a context keeps no state after deactivation and
    /// must be destroyed once that is handled; dropping the proxy is not
    /// enough, because wayland-client never sends a destructor on `Drop`.  A
    /// context left alive also keeps delivering `commit_state`/`reset` events
    /// that would clobber the state of whatever context replaced it.
    fn reset_context_state(&mut self) {
        self.active = false;
        self.deactivate_deadline = None;
        self.mode_hint_until = None;
        self.key_repeat = None;
        self.forwarded_keys.clear();
        if let Some(keyboard) = self.keyboard.take() {
            keyboard.release();
        }
        if let Some(context) = self.context.take() {
            context.destroy();
        }
        self.session.reset();
        self.pending_kimpanel_state = None;
        self.clear_kimpanel = true;
        self.hide_panel_popup();
    }

    fn set_repeat_info(&mut self, rate: i32, delay: i32) {
        match repeat_timings(rate, delay) {
            Some((delay, interval)) => {
                self.repeat_delay = Some(delay);
                self.repeat_interval = Some(interval);
            }
            None => {
                self.repeat_delay = None;
                self.repeat_interval = None;
                self.key_repeat = None;
            }
        }
    }

    /// Whether the keymap marks this key as repeating.  Modifiers and keys
    /// carrying `repeat=no` must not repeat, exactly as for an ordinary
    /// `wl_keyboard` client.
    fn key_repeats(&self, evdev_keycode: u32) -> bool {
        self.xkb_keymap
            .as_ref()
            .is_some_and(|keymap| keymap.key_repeats(xkb::Keycode::from(evdev_keycode + 8)))
    }

    /// Start repeating a key the IME consumed.  Forwarded keys are left alone:
    /// the client repeats those itself.
    fn arm_key_repeat(&mut self, evdev_keycode: u32) {
        let (Some(delay), Some(interval)) = (self.repeat_delay, self.repeat_interval) else {
            self.key_repeat = None;
            return;
        };
        if !self.key_repeats(evdev_keycode) {
            self.key_repeat = None;
            return;
        }
        self.key_repeat = Some(KeyRepeat {
            key: evdev_keycode,
            next: Instant::now() + delay,
            interval,
        });
    }

    fn cancel_key_repeat_for(&mut self, evdev_keycode: u32) {
        if self
            .key_repeat
            .as_ref()
            .is_some_and(|repeat| repeat.key == evdev_keycode)
        {
            self.key_repeat = None;
        }
    }

    fn key_repeat_due(&self) -> bool {
        self.key_repeat
            .as_ref()
            .is_some_and(|repeat| Instant::now() >= repeat.next)
    }

    fn fire_key_repeat(&mut self, qh: &QueueHandle<Self>) {
        let Some(key) = self.key_repeat.as_ref().map(|repeat| repeat.key) else {
            return;
        };
        self.handle_key_event(key, wl_keyboard::KeyState::Pressed, qh);
        if let Some(repeat) = self.key_repeat.as_mut() {
            if repeat.key == key {
                repeat.next = Instant::now() + repeat.interval;
            }
        }
    }

    /// Milliseconds until the next timer fires, or -1 when nothing is pending.
    fn poll_timeout_ms(&self) -> i32 {
        let deadlines = [
            self.mode_hint_until,
            self.deactivate_deadline,
            self.key_repeat.as_ref().map(|repeat| repeat.next),
        ];
        let Some(next) = deadlines.into_iter().flatten().min() else {
            return -1;
        };
        let now = Instant::now();
        if next <= now {
            return 0;
        }
        (next - now).as_millis().min(i32::MAX as u128) as i32
    }

    /// Drop the UI built from the dictionaries a reload just replaced.
    fn handle_reload(&mut self) {
        tracing::info!("KDE Wayland IME: clearing composition after librime reload");
        self.key_repeat = None;
        self.session.clear_composition();
        self.clear_context_preedit();
        self.clear_kimpanel();
        self.mode_hint_until = None;
        self.hide_panel_popup();
    }

    /// Apply a `content_type` event: password fields stop composing so that no
    /// secret reaches librime, the candidate window or Kimpanel.
    fn apply_content_type(&mut self, hint: u32, purpose: u32) {
        let policy = content_type_policy(hint, purpose);
        let previous = self.session.input_policy();
        self.session.set_input_policy(policy);
        if previous.composing && !policy.composing {
            tracing::debug!("KDE: sensitive content type, passing keys through");
            self.key_repeat = None;
            self.clear_context_preedit();
            self.clear_kimpanel();
            self.hide_panel_popup();
        }
    }

    fn create_panel_popup(&mut self, qh: &QueueHandle<Self>) {
        let (Some(compositor), Some(panel_manager), Some(shm)) =
            (&self.compositor, &self.input_panel, &self.shm)
        else {
            return;
        };
        let surface = compositor.create_surface(qh, ());
        let popup = panel_manager.get_input_panel_surface(&surface, qh, ());
        popup.set_overlay_panel();

        // Set input region to empty so clicks pass through the candidate window.
        let region = compositor.create_region(qh, ());
        surface.set_input_region(Some(&region));
        region.destroy();

        // 1x1 transparent dummy buffer to make surface valid for KWin
        let mut fd = match tempfile() {
            Ok(file) => file,
            Err(e) => {
                tracing::warn!("failed to create dummy SHM file: {e}");
                return;
            }
        };
        if fd.set_len(4).is_err() {
            tracing::warn!("failed to truncate dummy SHM file");
            return;
        }
        if fd.write_all(&[0u8; 4]).is_err() {
            tracing::warn!("failed to write dummy SHM buffer");
            return;
        }
        let pool = shm.create_pool(fd.as_fd(), 4, qh, ());
        let buf = pool.create_buffer(0, 1, 1, 4, wl_shm::Format::Argb8888, qh, ());
        surface.attach(Some(&buf), 0, 0);
        surface.damage_buffer(0, 0, 1, 1);
        surface.commit();

        self.panel_buffer = Some(buf);
        self.panel_surface = Some(surface);
        self.panel_popup = Some(popup);
        pool.destroy();
    }

    fn hide_panel_popup(&mut self) {
        if let Some(surface) = &self.panel_surface {
            surface.attach(None, 0, 0);
            surface.commit();
        }
        if let Some(buf) = self.panel_buffer.take() {
            buf.destroy();
        }
        self.panel_visible = false;
        self.ime_state = None;
    }

    fn redraw_panel(&mut self, qh: &QueueHandle<Self>) {
        let (Some(renderer), Some(shm), Some(surface)) = (
            self.renderer.as_ref(),
            self.shm.as_ref(),
            self.panel_surface.as_ref(),
        ) else {
            return;
        };

        let show_hint = self.mode_hint_active();
        let (pixels, w, h) = if let Some(state) = self
            .ime_state
            .as_ref()
            .filter(|state| !state.candidates.is_empty())
        {
            renderer.render(state)
        } else if show_hint {
            renderer.render_mode_hint(self.ascii_mode)
        } else {
            return;
        };
        if w == 0 || h == 0 {
            return;
        }
        let stride = w * 4;
        let pool_size = (stride * h) as usize;

        let mut tmp = match tempfile() {
            Ok(file) => file,
            Err(e) => {
                tracing::warn!("failed to create SHM tempfile: {e}");
                return;
            }
        };
        if tmp.set_len(pool_size as u64).is_err() {
            tracing::warn!("failed to truncate SHM tempfile");
            return;
        }
        if tmp.write_all(&pixels).is_err() {
            tracing::warn!("failed to write SHM buffer");
            return;
        }
        let pool = shm.create_pool(tmp.as_fd(), pool_size as i32, qh, ());
        let buffer = pool.create_buffer(
            0,
            w as i32,
            h as i32,
            stride as i32,
            wl_shm::Format::Argb8888,
            qh,
            (),
        );

        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, w as i32, h as i32);
        surface.commit();

        if let Some(old) = self.panel_buffer.replace(buffer) {
            old.destroy();
        }
        pool.destroy();
    }

    fn show_panel(&mut self, state: ImeState, qh: &QueueHandle<Self>) {
        let has_content = !state.candidates.is_empty();
        let show_hint = self.mode_hint_active();
        self.ime_state = Some(state);
        if has_content || show_hint {
            if self.panel_surface.is_none() {
                self.create_panel_popup(qh);
            }
            self.panel_visible = true;
            self.redraw_panel(qh);
        } else {
            self.hide_panel_popup();
        }
    }

    fn mode_hint_active(&self) -> bool {
        self.mode_hint_until
            .is_some_and(|deadline| Instant::now() < deadline)
    }

    fn update_ascii_mode(&mut self, ascii_mode: bool, qh: &QueueHandle<Self>) {
        if ascii_mode == self.ascii_mode {
            return;
        }
        self.ascii_mode = ascii_mode;
        // The Plasma indicator keeps the mode it was told about last, so it has
        // to be told again whenever Rime switches.
        self.pending_kimpanel_mode = Some(ascii_mode);
        let duration = self
            .renderer
            .as_ref()
            .map(PanelRenderer::mode_hint_duration)
            .unwrap_or_else(|| Duration::from_millis(750));
        self.mode_hint_until = Some(Instant::now() + duration);
        self.show_panel(ImeState::empty(), qh);
        tracing::info!("IME mode changed: {}", if ascii_mode { "EN" } else { "CN" });
    }

    fn commit_state_to_context(&self, state: &ImeState) {
        let Some(ctx) = &self.context else { return };
        if let Some(committed) = &state.committed {
            tracing::debug!("KDE commit_string: {} chars", committed.chars().count());
            tracing::trace!("KDE commit_string: {committed:?}");
            ctx.commit_string(self.serial, committed.clone());
        }
        let preedit = state.preedit.clone();
        ctx.preedit_cursor(preedit_cursor_bytes(&preedit, state.cursor));
        // The third argument is what the client commits if the text input is
        // reset while composing (typically on unfocus).  Passing an empty
        // string there throws away whatever the user had typed.
        ctx.preedit_string(self.serial, preedit.clone(), preedit);
    }

    fn clear_context_preedit(&self) {
        if let Some(ctx) = &self.context {
            ctx.preedit_cursor(0);
            ctx.preedit_string(self.serial, String::new(), String::new());
        }
    }

    fn update_kimpanel(&mut self, state: &ImeState) {
        self.pending_kimpanel_state = Some(state.clone());
        self.clear_kimpanel = false;
    }

    fn clear_kimpanel(&mut self) {
        self.pending_kimpanel_state = None;
        self.clear_kimpanel = true;
    }

    /// Hand a key back to the client.  The protocol wants the arguments of the
    /// originating `wl_keyboard::key` event, so this uses the key serial and
    /// not the `commit_state` serial that tracks the text-input state.
    fn forward_key(&self, evdev_keycode: u32, state: u32) {
        tracing::trace!("KDE forwarding key: key={evdev_keycode}, state={state}");
        if let Some(ctx) = &self.context {
            ctx.key(
                self.last_key_serial,
                self.last_key_time,
                evdev_keycode,
                state,
            );
        }
    }

    fn forward_key_press(&mut self, evdev_keycode: u32) {
        self.forwarded_keys.insert(evdev_keycode);
        self.forward_key(evdev_keycode, wl_keyboard::KeyState::Pressed as u32);
    }

    fn forward_modifiers(
        &self,
        serial: u32,
        mods_depressed: u32,
        mods_latched: u32,
        mods_locked: u32,
        group: u32,
    ) {
        if let Some(ctx) = &self.context {
            ctx.modifiers(serial, mods_depressed, mods_latched, mods_locked, group);
        }
    }

    fn handle_key_event(
        &mut self,
        evdev_keycode: u32,
        key_state: wl_keyboard::KeyState,
        qh: &QueueHandle<Self>,
    ) {
        tracing::trace!(
            "KDE handling key event: key={evdev_keycode}, state={key_state:?}, active={}",
            self.active
        );
        if key_state == wl_keyboard::KeyState::Released {
            self.handle_key_release(evdev_keycode, qh);
            return;
        }

        // A new press always supersedes whatever was repeating.
        self.key_repeat = None;

        if !self.active {
            self.forward_key_press(evdev_keycode);
            return;
        }

        // Password and PIN fields: nothing reaches librime and nothing is drawn.
        if !self.session.input_policy().composing {
            self.forward_key_press(evdev_keycode);
            return;
        }

        let sym_raw = self.key_sym(evdev_keycode);
        if sym_raw == xkb::keysyms::KEY_NoSymbol {
            tracing::warn!("KDE IME key dropped: NoSymbol");
            return;
        }

        let effective_mods = if is_shift_key(sym_raw) {
            self.mods & !RIME_MOD_SHIFT
        } else {
            self.mods
        };

        let before_state = self.session.state();
        if key_policy::should_bypass_empty_composition(sym_raw, effective_mods, &before_state) {
            self.forward_key_press(evdev_keycode);
            return;
        }

        // Return belongs to librime: only the schema's editor knows whether it
        // confirms the highlighted candidate or commits the raw code.  Space
        // and the digits are ordinary keys for the same reason.
        if key_policy::is_enter_key(sym_raw)
            && key_policy::enter_action(&before_state) == key_policy::EnterAction::ForwardToRime
        {
            let Some(result) = self.session.process_enter() else {
                self.forward_key_press(evdev_keycode);
                return;
            };
            if !result.accepted {
                self.forward_key_press(evdev_keycode);
                return;
            }
            self.update_ascii_mode(result.state.ascii_mode, qh);
            self.commit_state_to_context(&result.state);
            self.update_kimpanel(&result.state);
            self.show_panel(result.state, qh);
            return;
        }

        let Some(result) = self.session.process_key_result(sym_raw, effective_mods) else {
            self.forward_key_press(evdev_keycode);
            return;
        };
        let ime_state = result.state;

        self.update_ascii_mode(ime_state.ascii_mode, qh);

        if result.accepted {
            self.commit_state_to_context(&ime_state);
            self.update_kimpanel(&ime_state);
            self.show_panel(ime_state, qh);
            if key_policy::should_forward_consumed_shortcut(sym_raw, effective_mods) {
                self.forward_key_press(evdev_keycode);
            } else {
                // The grab took this key away from KWin, so a held
                // Backspace/arrow/select key has to repeat from here.
                self.arm_key_repeat(evdev_keycode);
            }
        } else {
            // librime rejected the key, but it can still have flushed a commit
            // on the way out (the ascii_composer confirms the composition when
            // it switches mode), so the commit goes out before the UI is torn
            // down or those characters are lost.
            self.commit_state_to_context(&ime_state);
            self.clear_context_preedit();
            self.clear_kimpanel();
            self.hide_panel_popup();
            self.forward_key_press(evdev_keycode);
        }
    }

    fn handle_key_release(&mut self, evdev_keycode: u32, qh: &QueueHandle<Self>) {
        self.cancel_key_repeat_for(evdev_keycode);
        let sym_raw = self.key_sym(evdev_keycode);
        if is_shift_key(sym_raw) && self.session.input_policy().composing {
            if let Some(result) = self.session.process_key_result(sym_raw, RIME_RELEASE_MASK) {
                self.update_ascii_mode(result.state.ascii_mode, qh);
            }
        }
        // Only keys whose press reached the client may deliver a release, or
        // the client sees a release with no matching press.
        if self.forwarded_keys.remove(&evdev_keycode) {
            self.forward_key(evdev_keycode, wl_keyboard::KeyState::Released as u32);
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for App {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            state
                .globals_seen
                .push(format!("{interface}@{version}#{name}"));
            match interface.as_str() {
                "wl_seat" => state.seat = Some(registry.bind(name, version.min(7), qh, ())),
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, version.min(6), qh, ()));
                }
                "wl_shm" => state.shm = Some(registry.bind(name, version.min(2), qh, ())),
                "zwp_input_method_v1" => {
                    state.input_method = Some(registry.bind(name, 1, qh, ()));
                }
                "zwp_input_panel_v1" => {
                    state.input_panel = Some(registry.bind(name, 1, qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<ZwpInputMethodV1, ()> for App {
    fn event(
        state: &mut Self,
        _proxy: &ZwpInputMethodV1,
        event: zwp_input_method_v1::Event,
        _: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwp_input_method_v1::Event::Activate { id } => {
                tracing::info!("KDE input-method-v1 context activated!");
                state.reset_context_state();
                // Every activation starts from the protocol defaults: hint
                // none, purpose normal.
                state
                    .session
                    .set_input_policy(InputContextPolicy::default());
                let keyboard = id.grab_keyboard(qh, ());
                if keyboard.version() < WL_KEYBOARD_REPEAT_INFO_SINCE {
                    state.set_repeat_info(FALLBACK_REPEAT_RATE, FALLBACK_REPEAT_DELAY);
                }
                state.keyboard = Some(keyboard);
                state.context = Some(id);
                state.active = true;
            }
            zwp_input_method_v1::Event::Deactivate { context } => {
                tracing::info!("KDE input-method-v1 context deactivated!");
                // The context named by the event is the one that has to be
                // destroyed; if the compositor already handed us a newer one,
                // destroy the stale proxy right away instead of the live one.
                if state.context.as_ref().is_some_and(|live| *live == context) {
                    state.deactivate_deadline = Some(Instant::now() + DEACTIVATE_DEBOUNCE);
                } else {
                    // A newer activate already replaced (and destroyed) it.
                    tracing::debug!("KDE: deactivate for an already replaced context");
                }
            }
            _ => {}
        }
    }

    wayland_client::event_created_child!(App, ZwpInputMethodV1, [
        0 => (ZwpInputMethodContextV1, ()),
    ]);
}

impl Dispatch<ZwpInputMethodContextV1, ()> for App {
    fn event(
        state: &mut Self,
        proxy: &ZwpInputMethodContextV1,
        event: zwp_input_method_context_v1::Event,
        _: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // A context that was deactivated but not yet destroyed can still emit
        // events; letting them through would reset the composition of the
        // context that replaced it.
        if !state.context.as_ref().is_some_and(|live| live == proxy) {
            return;
        }
        match event {
            zwp_input_method_context_v1::Event::CommitState { serial } => {
                state.serial = serial;
            }
            zwp_input_method_context_v1::Event::ContentType { hint, purpose } => {
                tracing::debug!("KDE content type: hint={hint:#x} purpose={purpose}");
                state.apply_content_type(hint, purpose);
            }
            zwp_input_method_context_v1::Event::Reset => {
                state.session.reset();
                state.clear_context_preedit();
                state.clear_kimpanel();
            }
            _ => {}
        }
    }
}

impl Dispatch<WlKeyboard, ()> for App {
    fn event(
        state: &mut Self,
        _proxy: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Keymap { format, fd, size } => {
                if format != WEnum::Value(wl_keyboard::KeymapFormat::XkbV1) {
                    tracing::warn!("KDE keyboard grab: unexpected keymap format {format:?}");
                    return;
                }
                let Ok(mmap) = memmap2::MmapOptions::new()
                    .len(size as usize)
                    .map_raw_read_only(&fd)
                else {
                    return;
                };
                let keymap_bytes =
                    unsafe { std::slice::from_raw_parts(mmap.as_ptr(), size as usize) };
                let keymap_text = keymap_bytes.strip_suffix(&[0]).unwrap_or(keymap_bytes);
                let keymap_string = String::from_utf8_lossy(keymap_text).into_owned();
                if let Some(km) = xkb::Keymap::new_from_string(
                    &state.xkb_context,
                    keymap_string,
                    xkb::KEYMAP_FORMAT_TEXT_V1,
                    xkb::KEYMAP_COMPILE_NO_FLAGS,
                ) {
                    state.xkb_state = Some(xkb::State::new(&km));
                    state.xkb_keymap = Some(km);
                }
            }
            wl_keyboard::Event::Key {
                serial,
                time,
                key,
                state: ks,
            } => {
                tracing::trace!("KDE keyboard Event::Key: key={key}, state={ks:?}");
                state.last_key_time = time;
                state.last_key_serial = serial;
                if let WEnum::Value(key_state) = ks {
                    state.handle_key_event(key, key_state, qh);
                }
            }
            wl_keyboard::Event::RepeatInfo { rate, delay } => {
                tracing::debug!("KDE keyboard grab: repeat rate={rate} delay={delay}");
                state.set_repeat_info(rate, delay);
            }
            wl_keyboard::Event::Modifiers {
                serial,
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
            } => {
                tracing::debug!("KDE keyboard Event::Modifiers");
                if let Some(xkb_state) = &mut state.xkb_state {
                    xkb_state.update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
                    let mut m = 0u32;
                    if xkb_state.mod_name_is_active(xkb::MOD_NAME_SHIFT, xkb::STATE_MODS_EFFECTIVE)
                    {
                        m |= RIME_MOD_SHIFT;
                    }
                    if xkb_state.mod_name_is_active(xkb::MOD_NAME_CTRL, xkb::STATE_MODS_EFFECTIVE) {
                        m |= RIME_MOD_CONTROL;
                    }
                    if xkb_state.mod_name_is_active(xkb::MOD_NAME_ALT, xkb::STATE_MODS_EFFECTIVE) {
                        m |= RIME_MOD_ALT;
                    }
                    state.mods = m;
                }
                state.forward_modifiers(serial, mods_depressed, mods_latched, mods_locked, group);
            }
            _ => {}
        }
    }
}

delegate_noop!(App: ignore WlSeat);
delegate_noop!(App: ignore WlCompositor);
delegate_noop!(App: ignore WlShm);
delegate_noop!(App: ignore WlShmPool);
delegate_noop!(App: ignore WlBuffer);
delegate_noop!(App: ignore WlSurface);
delegate_noop!(App: ignore ZwpInputPanelV1);
delegate_noop!(App: ignore ZwpInputPanelSurfaceV1);
delegate_noop!(App: ignore WlRegion);

pub fn run(engine: CoreEngine) -> Result<(), String> {
    let session = engine
        .create_session()
        .map_err(|e| format!("failed to create KDE Wayland Rime session: {e}"))?;
    let conn = Connection::connect_to_env().map_err(|e| format!("KDE Wayland connection: {e}"))?;
    let (globals, mut queue) =
        registry_queue_init::<App>(&conn).map_err(|e| format!("KDE Wayland registry: {e}"))?;
    let qh = queue.handle();

    let compositor: WlCompositor = globals
        .bind(&qh, 1..=6, ())
        .map_err(|e| format!("wl_compositor not advertised: {e}"))?;
    let shm: WlShm = globals
        .bind(&qh, 1..=2, ())
        .map_err(|e| format!("wl_shm not advertised: {e}"))?;
    let seat: WlSeat = globals
        .bind(&qh, 1..=7, ())
        .map_err(|e| format!("wl_seat not advertised: {e}"))?;
    let input_method: ZwpInputMethodV1 = globals
        .bind(&qh, 1..=1, ())
        .map_err(|e| format!("zwp_input_method_v1 not advertised: {e}"))?;
    let input_panel: ZwpInputPanelV1 = globals
        .bind(&qh, 1..=1, ())
        .map_err(|e| format!("zwp_input_panel_v1 not advertised: {e}"))?;

    let kimpanel_runtime =
        tokio::runtime::Runtime::new().map_err(|e| format!("Kimpanel runtime: {e}"))?;
    let kimpanel = kimpanel_runtime.block_on(KimpanelHandle::new());

    let mut app = App::new(session, kimpanel);
    app.compositor = Some(compositor);
    app.shm = Some(shm);
    app.seat = Some(seat);
    app.input_method = Some(input_method);
    app.input_panel = Some(input_panel);
    queue.roundtrip(&mut app).expect("initial KDE roundtrip");
    for g in globals.contents().clone_list() {
        tracing::info!("KDE Wayland global: {} v{}", g.interface, g.version);
    }

    let reload_signal = reload_bus::subscribe();

    tracing::info!("KDE Wayland IME running (input-method-v1)");
    loop {
        if app
            .mode_hint_until
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            app.mode_hint_until = None;
            app.show_panel(ImeState::empty(), &qh);
        }

        if app
            .deactivate_deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            app.reset_context_state();
            tracing::debug!("KDE input-method-v1 deactivated after debounce");
        }

        if app.key_repeat_due() {
            app.fire_key_repeat(&qh);
        }

        if let Err(e) = queue.flush() {
            tracing::warn!("KDE Wayland flush error: {e}");
        }
        if let Some(kimpanel) = &app.kimpanel {
            if let Some(ascii_mode) = app.pending_kimpanel_mode.take() {
                kimpanel_runtime.block_on(kimpanel.update_mode(ascii_mode));
            }
            if app.clear_kimpanel {
                kimpanel_runtime.block_on(kimpanel.clear());
                app.clear_kimpanel = false;
            } else if let Some(state) = app.pending_kimpanel_state.take() {
                kimpanel_runtime.block_on(kimpanel.update_state(&state));
            }
        }
        let timeout_ms = app.poll_timeout_ms();
        let mut pfds = vec![libc::pollfd {
            fd: conn.as_fd().as_raw_fd(),
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
        let ready =
            unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, timeout_ms) };
        if ready < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() != std::io::ErrorKind::Interrupted {
                tracing::warn!("KDE Wayland poll failed: {err}");
            }
            continue;
        }
        if let Some(signal) = &reload_signal {
            if signal.take() {
                app.handle_reload();
            }
        }
        if pfds[0].revents & libc::POLLIN != 0 {
            if let Err(e) = queue.blocking_dispatch(&mut app) {
                tracing::warn!("KDE Wayland dispatch error: {e}");
                return Err(format!("KDE Wayland connection closed: {e}"));
            }
        }
    }
}

fn tempfile() -> std::io::Result<File> {
    use std::os::unix::io::FromRawFd;
    let name = c"keytao-shm";
    let fd =
        unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(test)]
mod tests {
    use super::{
        content_type_policy, preedit_cursor_bytes, repeat_timings, FALLBACK_REPEAT_DELAY,
        FALLBACK_REPEAT_RATE,
    };
    use std::time::Duration;

    #[test]
    fn repeat_info_maps_to_a_delay_and_an_interval() {
        assert_eq!(
            repeat_timings(25, 600),
            Some((Duration::from_millis(600), Duration::from_millis(40)))
        );
        assert_eq!(repeat_timings(0, 600), None);
        assert_eq!(repeat_timings(-1, 600), None);
    }

    #[test]
    fn the_v1_fallback_actually_enables_repeating() {
        // KWin's grabbed keyboard is version 1, so nothing ever sends
        // repeat_info; the fallback is the only thing that makes a held
        // BackSpace repeat while composing.
        assert_eq!(
            repeat_timings(FALLBACK_REPEAT_RATE, FALLBACK_REPEAT_DELAY),
            Some((Duration::from_millis(600), Duration::from_millis(40)))
        );
    }

    #[test]
    fn password_content_types_stop_composing() {
        assert!(!content_type_policy(0, 8).composing);
        assert!(!content_type_policy(0, 8).learning);
        // hidden_text / sensitive_data, and the protocol's `password` shorthand
        assert!(!content_type_policy(0x40, 0).composing);
        assert!(!content_type_policy(0x80, 0).composing);
        assert!(!content_type_policy(0xc0, 0).composing);
    }

    #[test]
    fn ordinary_content_types_keep_composing() {
        assert!(content_type_policy(0, 0).composing);
        // text-input-v1 default hints (completion/correction/capitalization)
        assert!(content_type_policy(0x7, 0).composing);
        // purpose 9 is `date` in v1, not the v3/IBus `pin`
        assert!(content_type_policy(0, 9).composing);
        // purpose 12 is `terminal` in v1
        assert!(content_type_policy(0, 12).composing);
    }

    #[test]
    fn preedit_cursor_is_a_byte_offset() {
        assert_eq!(preedit_cursor_bytes("nihao", 2), 2);
        assert_eq!(preedit_cursor_bytes("你好ni", 2), 6);
        assert_eq!(preedit_cursor_bytes("你好", 9), 6);
        assert_eq!(preedit_cursor_bytes("", 0), 0);
    }
}
