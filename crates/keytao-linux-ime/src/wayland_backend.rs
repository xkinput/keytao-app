//! Wayland backend: zwp_input_method_v2 + zwp_input_popup_surface_v2.
//!
//! Protocol stack:
//!   zwp_input_method_manager_v2      — get the input method handle
//!   zwp_input_method_v2              — activate/deactivate + commit/preedit
//!   zwp_input_method_keyboard_grab_v2 — exclusive keyboard grab
//!   zwp_input_popup_surface_v2       — auto-positioned candidate panel at cursor
//!   wl_shm                           — shared-memory pixel buffers

use std::{
    collections::HashSet,
    fs::File,
    io::{Seek, SeekFrom, Write},
    os::fd::{AsFd, AsRawFd, FromRawFd},
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
        wl_registry,
        wl_seat::WlSeat,
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
        wl_surface::WlSurface,
    },
    Connection, Dispatch, QueueHandle, WEnum,
};
use wayland_protocols_misc::zwp_input_method_v2::client::{
    zwp_input_method_keyboard_grab_v2::{self, ZwpInputMethodKeyboardGrabV2},
    zwp_input_method_manager_v2::ZwpInputMethodManagerV2,
    zwp_input_method_v2::{self, ZwpInputMethodV2},
    zwp_input_popup_surface_v2::{self, ZwpInputPopupSurfaceV2},
};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};
use xkbcommon::xkb;

use crate::{
    engine::{CoreEngine, ImeSession},
    panel::{load_font, PanelRenderer},
    reload_bus,
};

const WL_KEYMAP_FORMAT_XKB_V1: u32 = 1;
const WL_KEY_RELEASED: u32 = 0;
const WL_KEY_PRESSED: u32 = 1;

const DEACTIVATE_DEBOUNCE: Duration = Duration::from_millis(180);

/// `zwp_text_input_v3::content_purpose::password`.
const TEXT_INPUT_PURPOSE_PASSWORD: u32 = 8;
/// `zwp_text_input_v3::content_purpose::pin`.
const TEXT_INPUT_PURPOSE_PIN: u32 = 9;
/// `zwp_text_input_v3::content_hint`: `hidden_text | sensitive_data`.
const TEXT_INPUT_HINT_SECRET: u32 = 0x40 | 0x80;
/// Hint `none` and purpose `normal` — what `activate` resets the content type
/// to, per the protocol.
const DEFAULT_CONTENT_TYPE: (u32, u32) = (0, 0);

fn is_shift_key(sym: u32) -> bool {
    sym == xkb::keysyms::KEY_Shift_L || sym == xkb::keysyms::KEY_Shift_R
}

/// The session policy a `zwp_input_method_v2` content type maps to.
///
/// Only the conclusion is shared with the IBus path in `engine.rs`: the
/// `zwp_text_input_v3` hint bits and purpose values are a different numbering
/// than the IBus ones, so the constants must not be reused.
fn content_type_policy(hint: u32, purpose: u32) -> InputContextPolicy {
    let secret = matches!(
        purpose,
        TEXT_INPUT_PURPOSE_PASSWORD | TEXT_INPUT_PURPOSE_PIN
    ) || hint & TEXT_INPUT_HINT_SECRET != 0;
    if secret {
        InputContextPolicy::sensitive()
    } else {
        InputContextPolicy::default()
    }
}

/// `zwp_input_method_v2` preedit indices are byte offsets into the preedit
/// string, while `ImeState::cursor` counts Unicode scalars.
fn preedit_cursor_bytes(preedit: &str, cursor: usize) -> i32 {
    preedit
        .char_indices()
        .nth(cursor)
        .map(|(offset, _)| offset)
        .unwrap_or(preedit.len()) as i32
}

/// Turn a `repeat_info` event into (initial delay, interval between repeats).
///
/// `rate` is in keys per second and zero disables repeating entirely; negative
/// values are illegal per the protocol and are treated the same way.
fn repeat_timings(rate: i32, delay: i32) -> Option<(Duration, Duration)> {
    if rate <= 0 || delay < 0 {
        return None;
    }
    Some((
        Duration::from_millis(delay as u64),
        Duration::from_micros(1_000_000 / rate as u64),
    ))
}

/// A key the IME consumed and now has to repeat itself: with the keyboard
/// grabbed, the compositor no longer repeats anything for us.
struct KeyRepeat {
    key: u32,
    next: Instant,
    interval: Duration,
}

struct App {
    session: ImeSession,
    renderer: Option<PanelRenderer>,

    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    seat: Option<WlSeat>,
    ime_manager: Option<ZwpInputMethodManagerV2>,
    virtual_keyboard_manager: Option<ZwpVirtualKeyboardManagerV1>,
    wayland_unavailable: bool,

    input_method: Option<ZwpInputMethodV2>,
    keyboard_grab: Option<ZwpInputMethodKeyboardGrabV2>,
    virtual_keyboard: Option<ZwpVirtualKeyboardV1>,
    virtual_keymap: Option<File>,
    forwarded_keys: HashSet<u32>,
    last_key_time: u32,
    serial: u32,
    active: bool,
    pending_active: Option<bool>,
    deactivate_deadline: Option<Instant>,
    /// Double-buffered `content_type`, applied on the next `done`.
    pending_content_type: Option<(u32, u32)>,
    key_repeat: Option<KeyRepeat>,
    repeat_delay: Option<Duration>,
    repeat_interval: Option<Duration>,

    xkb_context: xkb::Context,
    xkb_keymap: Option<xkb::Keymap>,
    xkb_state: Option<xkb::State>,
    mods: u32,

    // Popup panel — compositor auto-positions it at the cursor
    panel_surface: Option<WlSurface>,
    panel_popup: Option<ZwpInputPopupSurfaceV2>,
    panel_buffer: Option<WlBuffer>,
    panel_visible: bool,

    ime_state: Option<ImeState>,
    ascii_mode: bool,
    mode_hint_until: Option<Instant>,
}

impl App {
    fn new(session: ImeSession) -> Self {
        let renderer = load_font().and_then(PanelRenderer::new);
        if renderer.is_none() {
            tracing::warn!("panel renderer unavailable: no font found");
        }
        Self {
            session,
            renderer,
            compositor: None,
            shm: None,
            seat: None,
            ime_manager: None,
            virtual_keyboard_manager: None,
            input_method: None,
            keyboard_grab: None,
            virtual_keyboard: None,
            virtual_keymap: None,
            forwarded_keys: HashSet::new(),
            last_key_time: 0,
            serial: 0,
            active: false,
            pending_active: None,
            deactivate_deadline: None,
            pending_content_type: None,
            key_repeat: None,
            repeat_delay: None,
            repeat_interval: None,
            xkb_context: xkb::Context::new(xkb::CONTEXT_NO_FLAGS),
            xkb_keymap: None,
            xkb_state: None,
            mods: 0,
            panel_surface: None,
            panel_popup: None,
            panel_buffer: None,
            panel_visible: false,
            ime_state: None,
            ascii_mode: false,
            mode_hint_until: None,
            wayland_unavailable: false,
        }
    }

    fn setup_ime(&mut self, qh: &QueueHandle<Self>) {
        let (Some(manager), Some(seat)) = (&self.ime_manager, &self.seat) else {
            return;
        };
        let im = manager.get_input_method(seat, qh, ());
        self.input_method = Some(im);
        tracing::debug!("input method registered");
    }

    fn setup_virtual_keyboard(&mut self, qh: &QueueHandle<Self>) {
        let (Some(manager), Some(seat)) = (&self.virtual_keyboard_manager, &self.seat) else {
            tracing::warn!("Wayland virtual-keyboard unavailable; shortcuts may not pass through");
            return;
        };
        self.virtual_keyboard = Some(manager.create_virtual_keyboard(seat, qh, ()));
        tracing::debug!("virtual keyboard registered for unhandled key forwarding");
    }

    fn install_virtual_keymap(&mut self, keymap: &[u8]) {
        let Some(vk) = &self.virtual_keyboard else {
            return;
        };

        let mut file = match tempfile() {
            Ok(file) => file,
            Err(e) => {
                tracing::warn!("failed to create virtual keyboard keymap fd: {e}");
                return;
            }
        };
        if file
            .set_len(keymap.len() as u64)
            .and_then(|_| file.write_all(keymap))
            .and_then(|_| file.seek(SeekFrom::Start(0)).map(|_| ()))
            .is_err()
        {
            tracing::warn!("failed to write virtual keyboard keymap");
            return;
        }

        vk.keymap(WL_KEYMAP_FORMAT_XKB_V1, file.as_fd(), keymap.len() as u32);
        self.virtual_keymap = Some(file);
        tracing::debug!("virtual keyboard keymap installed ({} bytes)", keymap.len());
    }

    fn create_panel_popup(&mut self, qh: &QueueHandle<Self>) {
        let (Some(compositor), Some(im), Some(shm)) =
            (&self.compositor, &self.input_method, &self.shm)
        else {
            return;
        };
        let surface = compositor.create_surface(qh, ());
        let popup = im.get_input_popup_surface(&surface, qh, ());

        // 1x1 transparent dummy buffer to make surface valid for KWin
        // KWin drops surfaces without buffers, so it won't send TextInputRectangle.
        let fd = unsafe {
            libc::memfd_create(
                b"keytao-shm\0".as_ptr() as *const libc::c_char,
                libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
            )
        };
        if fd < 0 {
            panic!("memfd_create failed");
        }
        unsafe {
            libc::ftruncate(fd, 4);
        }
        let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
        file.write_all(&[0u8; 4]).expect("write dummy buffer");
        let fd = std::os::fd::AsFd::as_fd(&file);
        let pool = shm.create_pool(fd, 4, qh, ());
        let buf = pool.create_buffer(0, 1, 1, 4, wl_shm::Format::Argb8888, qh, ());
        surface.attach(Some(&buf), 0, 0);
        surface.damage_buffer(0, 0, 1, 1);
        surface.commit();

        self.panel_buffer = Some(buf);
        self.panel_surface = Some(surface);
        self.panel_popup = Some(popup);
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

        let mut tmp = tempfile().expect("tempfile");
        tmp.write_all(&pixels).expect("write shm");
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
        // The application already renders preedit through set_preedit_string.
        // Keep the popup for actual candidate menus and short mode hints only.
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
        let duration = self
            .renderer
            .as_ref()
            .map(PanelRenderer::mode_hint_duration)
            .unwrap_or_else(|| Duration::from_millis(750));
        self.mode_hint_until = Some(Instant::now() + duration);
        self.show_panel(ImeState::empty(), qh);
        tracing::info!("IME mode changed: {}", if ascii_mode { "EN" } else { "CN" });
    }

    fn key_sym(&self, evdev_keycode: u32) -> u32 {
        let keycode = evdev_keycode + 8;
        self.xkb_state
            .as_ref()
            .map(|s| s.key_get_one_sym(xkb::Keycode::from(keycode)))
            .unwrap_or(xkb::Keysym::from(xkb::keysyms::KEY_NoSymbol))
            .into()
    }

    /// Publish one `ImeState` to the client: commit first, then the new
    /// preedit, then a single `commit(serial)` — the order
    /// `zwp_input_method_v2.commit` prescribes.  Every path that produces a
    /// state goes through here so none of them can forget the preedit.
    fn apply_state_to_input_method(&self, state: &ImeState) {
        let Some(im) = &self.input_method else { return };
        if let Some(committed) = &state.committed {
            if !committed.is_empty() {
                tracing::debug!("Wayland commit_string: {} chars", committed.chars().count());
                tracing::trace!("Wayland commit_string: {committed:?}");
                im.commit_string(committed.clone());
            }
        }
        let cursor = preedit_cursor_bytes(&state.preedit, state.cursor);
        im.set_preedit_string(state.preedit.clone(), cursor, cursor);
        im.commit(self.serial);
    }

    fn clear_client_preedit(&self) {
        let Some(im) = &self.input_method else { return };
        im.set_preedit_string(String::new(), 0, 0);
        im.commit(self.serial);
    }

    /// Apply a `content_type` that `done` made current.
    fn apply_content_type(&mut self, hint: u32, purpose: u32) {
        let policy = content_type_policy(hint, purpose);
        let previous = self.session.input_policy();
        self.session.set_input_policy(policy);
        if previous.composing && !policy.composing {
            tracing::debug!("Wayland: sensitive content type, passing keys through");
            self.cancel_key_repeat();
            self.clear_client_preedit();
            self.hide_panel_popup();
        }
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

    fn cancel_key_repeat(&mut self) {
        self.key_repeat = None;
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
        // `handle_key_event` re-arms with the initial delay; once repeating has
        // started the following presses are one interval apart.
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
        tracing::info!("Wayland IME: clearing composition after librime reload");
        self.cancel_key_repeat();
        self.session.clear_composition();
        self.clear_client_preedit();
        self.mode_hint_until = None;
        self.hide_panel_popup();
    }

    fn handle_key_event(
        &mut self,
        evdev_keycode: u32,
        key_state: wl_keyboard::KeyState,
        qh: &QueueHandle<Self>,
    ) {
        if key_state == wl_keyboard::KeyState::Released {
            self.handle_key_release(evdev_keycode, qh);
            return;
        }

        // A new press always supersedes whatever was repeating.
        self.cancel_key_repeat();

        // When inactive (e.g. during debounce window after deactivate), forward
        // all keys directly to the application instead of processing them.
        if !self.active {
            self.forward_unhandled_key(evdev_keycode, self.key_sym(evdev_keycode));
            return;
        }

        // Password and PIN fields: nothing reaches librime and nothing is drawn.
        if !self.session.input_policy().composing {
            self.forward_unhandled_key(evdev_keycode, self.key_sym(evdev_keycode));
            return;
        }

        let keycode = evdev_keycode + 8;
        tracing::trace!(
            "key press: keycode={keycode} xkb_state={}",
            if self.xkb_state.is_some() {
                "ok"
            } else {
                "NONE"
            }
        );
        let sym_raw = self.key_sym(evdev_keycode);
        tracing::trace!("key sym: {sym_raw:#x}");
        if sym_raw == xkb::keysyms::KEY_NoSymbol {
            tracing::warn!("key dropped: NoSymbol (xkb_state likely None)");
            return;
        }

        let effective_mods = if is_shift_key(sym_raw) {
            self.mods & !RIME_MOD_SHIFT
        } else {
            self.mods
        };

        let before_state = self.session.state();
        if key_policy::should_bypass_empty_composition(sym_raw, effective_mods, &before_state) {
            self.forward_unhandled_key(evdev_keycode, sym_raw);
            return;
        }
        // Return belongs to librime: only the schema's editor knows whether it
        // confirms the highlighted candidate or commits the raw code.  Space
        // and the digits are ordinary keys for the same reason.
        if key_policy::is_enter_key(sym_raw)
            && key_policy::enter_action(&before_state) == key_policy::EnterAction::ForwardToRime
        {
            let Some(result) = self.session.process_enter() else {
                self.forward_unhandled_key(evdev_keycode, sym_raw);
                return;
            };
            if !result.accepted {
                self.forward_unhandled_key(evdev_keycode, sym_raw);
                return;
            }
            self.update_ascii_mode(result.state.ascii_mode, qh);
            self.apply_state_to_input_method(&result.state);
            self.show_panel(result.state, qh);
            return;
        }

        let result = match self.session.process_key_result(sym_raw, effective_mods) {
            Some(r) => r,
            None => {
                // librime says it cannot process this key at all.
                self.forward_unhandled_key(evdev_keycode, sym_raw);
                return;
            }
        };

        let ime_state = result.state;

        let consumed = result.accepted;
        self.update_ascii_mode(ime_state.ascii_mode, qh);

        tracing::trace!(
            "ime state: consumed={} ascii_mode={} commit={:?} preedit={:?} candidates={}",
            consumed,
            ime_state.ascii_mode,
            ime_state.committed,
            ime_state.preedit,
            ime_state.candidates.len(),
        );

        if !consumed {
            // librime rejected the key, but it can still have flushed a commit
            // on the way out (the ascii_composer confirms the composition when
            // it switches mode).  Publishing the state before forwarding is
            // what keeps those characters from being dropped.
            self.apply_state_to_input_method(&ime_state);
            self.forward_unhandled_key(evdev_keycode, sym_raw);
            self.show_panel(ime_state, qh);
            return;
        }

        self.apply_state_to_input_method(&ime_state);

        // Forward consumed Ctrl shortcuts that the app should still
        // receive (e.g. Ctrl+` for terminal quake toggle). Match the
        // precise list from the original linux.rs implementation.
        if key_policy::should_forward_consumed_shortcut(sym_raw, effective_mods) {
            self.forward_unhandled_key(evdev_keycode, sym_raw);
        } else {
            // The compositor stopped repeating this key when the grab took it,
            // so held Backspace/arrows/select keys have to repeat from here.
            self.arm_key_repeat(evdev_keycode);
        }

        self.show_panel(ime_state, qh);
    }

    fn handle_key_release(&mut self, evdev_keycode: u32, qh: &QueueHandle<Self>) {
        self.cancel_key_repeat_for(evdev_keycode);
        let sym_raw = self.key_sym(evdev_keycode);
        if is_shift_key(sym_raw) && self.session.input_policy().composing {
            if let Some(result) = self.session.process_key_result(sym_raw, RIME_RELEASE_MASK) {
                self.update_ascii_mode(result.state.ascii_mode, qh);
            }
        }
        if self.forwarded_keys.remove(&evdev_keycode) {
            self.forward_physical_key(evdev_keycode, WL_KEY_RELEASED);
        }
    }

    fn forward_unhandled_key(&mut self, evdev_keycode: u32, sym: u32) {
        if self.virtual_keyboard.is_some() && self.virtual_keymap.is_some() {
            self.forwarded_keys.insert(evdev_keycode);
            self.forward_physical_key(evdev_keycode, WL_KEY_PRESSED);
        } else {
            self.forward_text_key(sym);
        }
    }

    fn forward_physical_key(&self, evdev_keycode: u32, state: u32) {
        let Some(vk) = &self.virtual_keyboard else {
            return;
        };
        vk.key(self.last_key_time, evdev_keycode, state);
    }

    // Forward a key to the application when IME does not consume it.
    // zwp_input_method_v2 has no explicit forward_key request; we use
    // commit_string for printable chars and delete_surrounding_text for
    // delete/backspace.  Arrow/cursor keys cannot be forwarded this way —
    // the keyboard grab must release them; wlroots-based compositors do
    // this automatically for keys the IME commits nothing for.
    fn forward_text_key(&self, sym: u32) {
        let Some(im) = &self.input_method else { return };
        match sym {
            // Space
            0x0020 => {
                im.commit_string(" ".into());
                im.commit(self.serial);
            }
            // Return / KP_Enter
            0xff0d | 0xff8d => {
                im.commit_string("\n".into());
                im.commit(self.serial);
            }
            // BackSpace: delete one char to the left
            0xff08 => {
                im.delete_surrounding_text(1, 0);
                im.commit(self.serial);
            }
            // Delete: delete one char to the right
            0xffff => {
                im.delete_surrounding_text(0, 1);
                im.commit(self.serial);
            }
            // Tab
            0xff09 => {
                im.commit_string("\t".into());
                im.commit(self.serial);
            }
            // Arrow keys and other navigation — cannot be forwarded via
            // commit_string; the compositor passes them through the grab
            // automatically when the IME does not call commit().
            _ => {}
        }
    }
}

// ── Dispatch implementations ──────────────────────────────────────────────────

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
            match interface.as_str() {
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, version.min(4), qh, ()));
                }
                "wl_shm" => {
                    state.shm = Some(registry.bind(name, version.min(1), qh, ()));
                }
                "wl_seat" => {
                    state.seat = Some(registry.bind(name, version.min(7), qh, ()));
                }
                "zwp_input_method_manager_v2" => {
                    state.ime_manager = Some(registry.bind(name, 1, qh, ()));
                }
                "zwp_virtual_keyboard_manager_v1" => {
                    state.virtual_keyboard_manager = Some(registry.bind(name, 1, qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<ZwpInputMethodV2, ()> for App {
    fn event(
        state: &mut Self,
        proxy: &ZwpInputMethodV2,
        event: zwp_input_method_v2::Event,
        _: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwp_input_method_v2::Event::Activate => {
                state.pending_active = Some(true);
                // activate resets the content type together with everything
                // else the previous text input had set.
                state.pending_content_type = Some(DEFAULT_CONTENT_TYPE);
                if state.keyboard_grab.is_none() {
                    state.keyboard_grab = Some(proxy.grab_keyboard(qh, ()));
                }
                tracing::debug!("IME activate pending until done");
            }
            zwp_input_method_v2::Event::Deactivate => {
                state.pending_active = Some(false);
                tracing::debug!("IME deactivate pending until done");
            }
            zwp_input_method_v2::Event::ContentType { hint, purpose } => {
                let hint = match hint {
                    WEnum::Value(hint) => hint.bits(),
                    WEnum::Unknown(raw) => raw,
                };
                let purpose = match purpose {
                    WEnum::Value(purpose) => purpose as u32,
                    WEnum::Unknown(raw) => raw,
                };
                tracing::debug!("IME content type: hint={hint:#x} purpose={purpose}");
                state.pending_content_type = Some((hint, purpose));
            }
            zwp_input_method_v2::Event::Done => {
                // `done` makes every double-buffered event since the last one
                // current, in the order the protocol lists them.
                state.serial = state.serial.wrapping_add(1);
                let next_active = state.pending_active.take();
                match next_active {
                    Some(true) => {
                        state.deactivate_deadline = None;
                        state.active = true;
                        tracing::debug!("IME activated");
                    }
                    Some(false) => {
                        state.deactivate_deadline = Some(Instant::now() + DEACTIVATE_DEBOUNCE);
                        tracing::debug!("IME deactivation deferred for debounce");
                    }
                    None => {}
                }
                if let Some((hint, purpose)) = state.pending_content_type.take() {
                    state.apply_content_type(hint, purpose);
                }
                if next_active == Some(true) {
                    // activate also dropped the preedit the compositor was
                    // holding, so a composition that survived the debounce has
                    // to be published again or the client and Rime silently
                    // disagree about what is being typed.  A sensitive content
                    // type has already emptied the session by now.
                    let current = state.session.state();
                    if !current.preedit.is_empty() || !current.candidates.is_empty() {
                        state.apply_state_to_input_method(&current);
                        state.show_panel(current, qh);
                    }
                }
            }
            zwp_input_method_v2::Event::Unavailable => {
                // KDE Plasma: this fires when another IME (Fcitx5, IBus, Maliit) already
                // holds the input-method slot, OR if "Virtual Keyboard" in System Settings
                // → Input Devices is not set to "None".  Signal the event loop to exit
                // gracefully so the IBus/XIM threads keep running for KDE apps.
                tracing::warn!(
                    "zwp_input_method_v2: Unavailable — Wayland backend shutting down. \
                     On KDE Plasma: disable the virtual keyboard under System Settings \
                     → Input Devices → Virtual Keyboard and make sure no other IME \
                     (fcitx5, ibus) is running. The IBus/XIM backends will continue \
                     serving apps via QT_IM_MODULE/XMODIFIERS."
                );
                state.wayland_unavailable = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwpInputMethodKeyboardGrabV2, ()> for App {
    fn event(
        state: &mut Self,
        _proxy: &ZwpInputMethodKeyboardGrabV2,
        event: zwp_input_method_keyboard_grab_v2::Event,
        _: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwp_input_method_keyboard_grab_v2::Event::Keymap { format, fd, size } => {
                tracing::debug!("keyboard grab: keymap received format={format:?} size={size}");
                if format != WEnum::Value(wl_keyboard::KeymapFormat::XkbV1) {
                    tracing::warn!("keyboard grab: unexpected keymap format {format:?}, skipping");
                    return;
                }
                let mmap = memmap2::MmapOptions::new()
                    .len(size as usize)
                    .map_raw_read_only(&fd);
                if let Ok(mmap) = mmap {
                    let keymap_bytes =
                        unsafe { std::slice::from_raw_parts(mmap.as_ptr(), size as usize) };
                    let keymap_text = keymap_bytes.strip_suffix(&[0]).unwrap_or(keymap_bytes);
                    let s = String::from_utf8_lossy(keymap_text).into_owned();
                    if let Some(km) = xkb::Keymap::new_from_string(
                        &state.xkb_context,
                        s.clone(),
                        xkb::KEYMAP_FORMAT_TEXT_V1,
                        xkb::KEYMAP_COMPILE_NO_FLAGS,
                    ) {
                        state.xkb_state = Some(xkb::State::new(&km));
                        state.xkb_keymap = Some(km);
                        state.install_virtual_keymap(keymap_bytes);
                    }
                }
            }
            zwp_input_method_keyboard_grab_v2::Event::Key {
                key,
                time,
                state: ks,
                ..
            } => {
                state.last_key_time = time;
                if let WEnum::Value(key_state) = ks {
                    state.handle_key_event(key, key_state, qh);
                }
            }
            zwp_input_method_keyboard_grab_v2::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
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
                if let (Some(vk), Some(_)) = (&state.virtual_keyboard, &state.virtual_keymap) {
                    vk.modifiers(mods_depressed, mods_latched, mods_locked, group);
                }
            }
            zwp_input_method_keyboard_grab_v2::Event::RepeatInfo { rate, delay } => {
                tracing::debug!("keyboard grab: repeat rate={rate} delay={delay}");
                state.set_repeat_info(rate, delay);
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwpInputPopupSurfaceV2, ()> for App {
    fn event(
        state: &mut Self,
        _proxy: &ZwpInputPopupSurfaceV2,
        event: zwp_input_popup_surface_v2::Event,
        _: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let zwp_input_popup_surface_v2::Event::TextInputRectangle {
            x,
            y,
            width,
            height,
        } = event
        {
            tracing::trace!("cursor hint: {x},{y} {width}x{height}");
            // KWin (and other Wayland compositors) often require a re-commit of the surface
            // after sending TextInputRectangle, otherwise the popup might not be mapped or updated.
            let has_content = state
                .ime_state
                .as_ref()
                .map_or(false, |s| !s.candidates.is_empty());
            let show_mode_hint = state.mode_hint_active();
            if state.panel_surface.is_some() && (has_content || show_mode_hint) {
                tracing::debug!("rerender_panel_after_rectangle");
                state.redraw_panel(qh);
            }
        }
    }
}

delegate_noop!(App: ignore WlCompositor);
delegate_noop!(App: ignore WlShm);
delegate_noop!(App: ignore WlShmPool);
delegate_noop!(App: ignore WlBuffer);
delegate_noop!(App: ignore WlSurface);
delegate_noop!(App: ignore WlSeat);
delegate_noop!(App: ignore WlKeyboard);
delegate_noop!(App: ignore ZwpInputMethodManagerV2);
delegate_noop!(App: ignore ZwpVirtualKeyboardManagerV1);
delegate_noop!(App: ignore ZwpVirtualKeyboardV1);

// ── Entry point ────────────────────────────────────────────────────────────────

/// Returns Ok(()) when the Wayland input-method slot is unavailable (e.g. KDE
/// virtual keyboard is active) so the caller can keep IBus/XIM threads alive.
pub fn run(engine: CoreEngine) -> Result<(), String> {
    let session = engine
        .create_session()
        .map_err(|e| format!("failed to create Wayland Rime session: {e}"))?;

    let conn = Connection::connect_to_env().map_err(|e| format!("Wayland connection: {e}"))?;
    let (globals, mut queue) =
        registry_queue_init::<App>(&conn).map_err(|e| format!("registry: {e}"))?;
    let qh = queue.handle();

    let compositor: WlCompositor = globals
        .bind(&qh, 1..=4, ())
        .map_err(|e| format!("wl_compositor not advertised: {e}"))?;
    let shm: WlShm = globals
        .bind(&qh, 1..=1, ())
        .map_err(|e| format!("wl_shm not advertised: {e}"))?;
    let seat: WlSeat = globals
        .bind(&qh, 1..=7, ())
        .map_err(|e| format!("wl_seat not advertised: {e}"))?;
    let ime_manager: ZwpInputMethodManagerV2 = globals.bind(&qh, 1..=1, ()).map_err(|_| {
        // KDE Plasma < 5.24 does not implement this protocol; Plasma 5.24+ does.
        "compositor does not advertise zwp_input_method_manager_v2 \
         (KDE Plasma < 5.24, GNOME Shell without the mutter fork, or a compositor \
         that does not implement the Wayland input-method-v2 protocol)"
            .to_string()
    })?;
    let virtual_keyboard_manager: Option<ZwpVirtualKeyboardManagerV1> = globals
        .bind(&qh, 1..=1, ())
        .map_err(|e| {
            tracing::warn!(
                "compositor does not advertise zwp_virtual_keyboard_manager_v1; \
                 shortcut forwarding will be limited: {e}"
            );
            e
        })
        .ok();

    let mut app = App::new(session);
    app.compositor = Some(compositor);
    app.shm = Some(shm);
    app.seat = Some(seat);
    app.ime_manager = Some(ime_manager);
    app.virtual_keyboard_manager = virtual_keyboard_manager;

    app.setup_ime(&qh);
    app.setup_virtual_keyboard(&qh);
    queue.roundtrip(&mut app).expect("initial roundtrip");

    let reload_signal = reload_bus::subscribe();

    tracing::info!("Wayland IME running (popup-surface positioning)");
    loop {
        if app.wayland_unavailable {
            tracing::info!("Wayland IME exiting gracefully (slot unavailable)");
            return Ok(());
        }

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
            app.deactivate_deadline = None;
            app.active = false;
            app.cancel_key_repeat();
            // `grab_keyboard` hands out an object whose `release` request is a
            // destructor; dropping the proxy alone leaves it alive in the
            // compositor, and the next activate takes a fresh grab.
            let had_grab = match app.keyboard_grab.take() {
                Some(grab) => {
                    grab.release();
                    true
                }
                None => false,
            };
            app.session.reset();
            app.mode_hint_until = None;
            app.hide_panel_popup();
            tracing::debug!("IME deactivated after debounce (had_grab={had_grab})");
        }

        if app.key_repeat_due() {
            app.fire_key_repeat(&qh);
        }

        if let Err(e) = queue.flush() {
            tracing::warn!("Wayland flush error: {e}");
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
                tracing::warn!("Wayland poll failed: {err}");
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
                tracing::warn!("Wayland dispatch error: {e}");
                return Err(format!("Wayland connection closed: {e}"));
            }
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

fn tempfile() -> std::io::Result<File> {
    use std::os::unix::io::FromRawFd;
    let name = c"keytao-shm";
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(test)]
mod tests {
    use super::{content_type_policy, preedit_cursor_bytes, repeat_timings};
    use std::time::Duration;

    #[test]
    fn repeat_info_maps_to_a_delay_and_an_interval() {
        assert_eq!(
            repeat_timings(25, 600),
            Some((Duration::from_millis(600), Duration::from_millis(40)))
        );
        // A rate of zero disables repeating, as does an illegal negative one.
        assert_eq!(repeat_timings(0, 600), None);
        assert_eq!(repeat_timings(-1, 600), None);
        assert_eq!(repeat_timings(25, -1), None);
    }

    #[test]
    fn password_and_pin_content_types_stop_composing() {
        // purpose password / pin
        assert!(!content_type_policy(0, 8).composing);
        assert!(!content_type_policy(0, 9).composing);
        // hint hidden_text / sensitive_data on an otherwise normal field
        assert!(!content_type_policy(0x40, 0).composing);
        assert!(!content_type_policy(0x80, 0).composing);
        assert!(!content_type_policy(0, 8).learning);
    }

    #[test]
    fn ordinary_content_types_keep_composing() {
        assert!(content_type_policy(0, 0).composing);
        // completion + spellcheck on a free-form field
        assert!(content_type_policy(0x3, 0).composing);
        // terminal is purpose 13 in text-input-v3, not a secret
        assert!(content_type_policy(0, 13).composing);
        // date is purpose 10 there, i.e. the IBus PIN value must not leak in
        assert!(content_type_policy(0, 10).composing);
    }

    #[test]
    fn preedit_cursor_is_a_byte_offset() {
        assert_eq!(preedit_cursor_bytes("nihao", 2), 2);
        // Unicode scalar 2 of "你好ni" starts at byte 6.
        assert_eq!(preedit_cursor_bytes("你好ni", 2), 6);
        // Past the end clamps to the byte length rather than panicking.
        assert_eq!(preedit_cursor_bytes("你好", 9), 6);
        assert_eq!(preedit_cursor_bytes("", 0), 0);
    }
}
