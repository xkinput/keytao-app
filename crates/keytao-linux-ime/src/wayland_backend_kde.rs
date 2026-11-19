//! KDE/KWin Wayland backend: zwp_input_method_unstable_v1.
//!
//! KWin 6 still exposes input methods through input-method-v1. Applications talk
//! text-input-v1/v2/v3 to KWin; the IME process talks input-method-v1 to KWin.

use std::{
    collections::HashSet,
    io::Write,
    os::fd::{AsFd, AsRawFd},
    time::Instant,
};

use keytao_core::{
    key_policy, runtime_log, ImeState, InputContextPolicy, RIME_MOD_ALT, RIME_MOD_CONTROL,
    RIME_MOD_LOCK, RIME_MOD_SHIFT, RIME_MOD_SUPER, RIME_RELEASE_MASK,
};
use keytao_theme::ThemeResolver;
use wayland_client::{
    delegate_noop,
    globals::{registry_queue_init, GlobalListContents},
    protocol::{
        wl_buffer::WlBuffer,
        wl_compositor::WlCompositor,
        wl_keyboard::{self, WlKeyboard},
        wl_output::{self, WlOutput},
        wl_pointer::{self, WlPointer},
        wl_region::WlRegion,
        wl_registry,
        wl_seat::{self, WlSeat},
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
        wl_surface::{self, WlSurface},
    },
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
};
use wayland_protocols::wp::input_method::zv1::client::{
    zwp_input_method_context_v1::{self, ZwpInputMethodContextV1},
    zwp_input_method_v1::{self, ZwpInputMethodV1},
    zwp_input_panel_surface_v1::ZwpInputPanelSurfaceV1,
    zwp_input_panel_v1::ZwpInputPanelV1,
};
use wayland_protocols::wp::{
    fractional_scale::v1::client::{
        wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        wp_fractional_scale_v1::{self, WpFractionalScaleV1},
    },
    viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
};
use wayland_protocols::xdg::xdg_output::zv1::client::{
    zxdg_output_manager_v1::ZxdgOutputManagerV1,
    zxdg_output_v1::{self, ZxdgOutputV1},
};
use xkbcommon::xkb;

use crate::{
    engine::{candidate_panel_layout, inline_preedit, shift_key_modifiers, CoreEngine, ImeSession},
    ibus_shared::is_shift_key,
    kimpanel::KimpanelHandle,
    panel::{load_font, PanelRenderer, WaylandPanel},
    panel_hit::{PanelClick, PanelHitAreas},
    reload_bus,
    wayland_common::{
        deadline_active, key_repeats, key_sym, mode_hint_deadline, poll_timeout_ms,
        preedit_cursor_bytes, take_expired_deadline, tempfile, KeyRepeat, WaylandPanelScale,
    },
};

/// `BTN_LEFT` from linux/input-event-codes.h.
const BTN_LEFT: u32 = 0x110;

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

/// The session policy a `zwp_input_method_context_v1` content type maps to.
fn content_type_policy(hint: u32, purpose: u32) -> InputContextPolicy {
    if purpose == TEXT_INPUT_V1_PURPOSE_PASSWORD || hint & TEXT_INPUT_V1_HINT_SECRET != 0 {
        InputContextPolicy::sensitive()
    } else {
        InputContextPolicy::default()
    }
}

/// KWin deletes the context and its keyboard resource on deactivate. Sending
/// context.destroy or keyboard.release afterwards hits an invalid object (and
/// release is not even a request on the v1 keyboard returned by grab_keyboard).
/// Retire the client-side proxy too, so already queued events are ignored.
fn retire_proxy(proxy: impl Proxy) {
    if let Some(backend) = proxy.backend().upgrade() {
        let _ = backend.destroy_object(&proxy.id());
    }
}

struct App {
    session: ImeSession,
    deployment: crate::engine::DeploymentHint,
    deployment_text: Option<&'static str>,
    theme_resolver: ThemeResolver,
    seat: Option<WlSeat>,
    input_method: Option<ZwpInputMethodV1>,
    context: Option<ZwpInputMethodContextV1>,
    keyboard: Option<WlKeyboard>,
    serial: u32,
    active: bool,
    mode_hint_until: Option<Instant>,
    xkb_context: xkb::Context,
    xkb_keymap: Option<xkb::Keymap>,
    xkb_state: Option<xkb::State>,
    xkb_base_state: Option<xkb::State>,
    mods: u32,
    last_key_time: u32,
    /// Serial of the last `wl_keyboard::key`; `context.key` has to echo that
    /// one back, not the `commit_state` serial.
    last_key_serial: u32,
    /// Keys whose press was handed to the client, so their release can be too.
    forwarded_keys: HashSet<u32>,
    key_repeat: KeyRepeat,
    ascii_mode: bool,
    kimpanel: Option<KimpanelHandle>,
    pending_kimpanel_state: Option<ImeState>,
    /// Conversion mode waiting to be pushed to the Kimpanel indicator; the
    /// Kimpanel calls are async, so the event loop drains this.
    pending_kimpanel_mode: Option<bool>,
    clear_kimpanel: bool,
    globals_seen: Vec<String>,

    renderer: Option<PanelRenderer>,
    panel_scale: WaylandPanelScale,
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    input_panel: Option<ZwpInputPanelV1>,
    panel_surface: Option<WlSurface>,
    panel_popup: Option<ZwpInputPanelSurfaceV1>,
    panel_buffer: Option<WlBuffer>,
    panel_visible: bool,
    /// Where the candidates on screen were drawn; empty unless candidates are.
    panel_hit_areas: PanelHitAreas,
    pointer: Option<WlPointer>,
    /// Pointer position over the panel surface, `None` when it is elsewhere.
    pointer_pos: Option<(f64, f64)>,
    pressed_click: Option<PanelClick>,
    ime_state: Option<ImeState>,
}

impl App {
    fn new(session: ImeSession, kimpanel: Option<KimpanelHandle>) -> Self {
        let renderer = load_font().and_then(PanelRenderer::new);
        Self {
            session,
            deployment: crate::engine::DeploymentHint::default(),
            deployment_text: None,
            theme_resolver: ThemeResolver::from_default_locations(),
            seat: None,
            input_method: None,
            context: None,
            keyboard: None,
            serial: 0,
            active: false,
            mode_hint_until: None,
            xkb_context: xkb::Context::new(xkb::CONTEXT_NO_FLAGS),
            xkb_keymap: None,
            xkb_state: None,
            xkb_base_state: None,
            mods: 0,
            last_key_time: 0,
            last_key_serial: 0,
            forwarded_keys: HashSet::new(),
            key_repeat: KeyRepeat::default(),
            ascii_mode: false,
            kimpanel,
            pending_kimpanel_state: None,
            pending_kimpanel_mode: None,
            clear_kimpanel: true,
            globals_seen: Vec::new(),

            renderer,
            panel_scale: WaylandPanelScale::default(),
            compositor: None,
            shm: None,
            input_panel: None,
            panel_surface: None,
            panel_popup: None,
            panel_buffer: None,
            panel_visible: false,
            panel_hit_areas: PanelHitAreas::default(),
            pointer: None,
            pointer_pos: None,
            pressed_click: None,
            ime_state: None,
        }
    }

    /// Stop using the context as soon as KWin deactivates or replaces it.
    fn reset_context_state(&mut self) {
        self.deployment.reset();
        self.deployment_text = None;
        if let Some(kimpanel) = &self.kimpanel {
            kimpanel.show_deployment_hint(None);
        }
        self.active = false;
        self.mode_hint_until = None;
        self.key_repeat.cancel();
        self.forwarded_keys.clear();
        if let Some(keyboard) = self.keyboard.take() {
            retire_proxy(keyboard);
        }
        if let Some(context) = self.context.take() {
            retire_proxy(context);
        }
        self.serial = 0;
        self.last_key_serial = 0;
        self.last_key_time = 0;
        self.mods = 0;
        self.xkb_keymap = None;
        self.xkb_state = None;
        self.xkb_base_state = None;
        self.key_repeat = KeyRepeat::default();
        self.session.reset();
        self.pending_kimpanel_state = None;
        self.clear_kimpanel = true;
        self.hide_panel_popup();
    }

    fn fire_key_repeat(&mut self, qh: &QueueHandle<Self>) {
        let Some(key) = self.key_repeat.key() else {
            return;
        };
        self.handle_key_event(key, wl_keyboard::KeyState::Pressed, qh);
        self.key_repeat.advance(key);
    }

    /// Milliseconds until the next timer fires, or -1 when nothing is pending.
    fn poll_timeout_ms(&self) -> i32 {
        let timeout = poll_timeout_ms([self.mode_hint_until, self.key_repeat.next_deadline()]);
        if self.deployment_text.is_some() && (timeout < 0 || timeout > 250) {
            250
        } else {
            timeout
        }
    }

    fn refresh_deployment_hint(&mut self, qh: &QueueHandle<Self>) {
        let hint = self.deployment.hint();
        if hint != self.deployment_text {
            self.deployment_text = hint;
            self.show_panel(self.ime_state.clone().unwrap_or_else(ImeState::empty), qh);
        }
    }

    fn sync_deployment_aux(&self) {
        if self.renderer.is_none() || self.panel_surface.is_none() {
            if let Some(kimpanel) = &self.kimpanel {
                let hint = keytao_core::DeploymentState::visible_hint(
                    self.deployment_text,
                    self.ime_state
                        .as_ref()
                        .is_some_and(|state| !state.candidates.is_empty()),
                );
                kimpanel.show_deployment_hint(hint);
            }
        }
    }

    /// Drop the UI built from the dictionaries a reload just replaced.
    fn handle_reload(&mut self) {
        tracing::info!("KDE Wayland IME: clearing composition after librime reload");
        self.key_repeat.cancel();
        self.session.clear_composition();
        self.clear_context_preedit();
        self.clear_kimpanel();
        self.mode_hint_until = None;
        self.hide_panel_popup();
        self.deployment_text = None;
    }

    /// Apply a `content_type` event: password fields stop composing so that no
    /// secret reaches librime, the candidate window or Kimpanel.
    fn apply_content_type(&mut self, hint: u32, purpose: u32) {
        let policy = content_type_policy(hint, purpose);
        if !policy.composing {
            self.deployment.reset();
            self.deployment_text = None;
            if let Some(kimpanel) = &self.kimpanel {
                kimpanel.show_deployment_hint(None);
            }
        }
        let previous = self.session.input_policy();
        self.session.set_input_policy(policy);
        if previous.composing && !policy.composing {
            tracing::debug!("KDE: sensitive content type, passing keys through");
            self.key_repeat.cancel();
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
        // 1x1 transparent dummy buffer to make surface valid for KWin
        let mut fd = match tempfile(libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING) {
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
        let surface = compositor.create_surface(qh, ());
        self.panel_scale.attach_surface(&surface, qh);
        let popup = panel_manager.get_input_panel_surface(&surface, qh, ());
        popup.set_overlay_panel();
        let pool = shm.create_pool(fd.as_fd(), 4, qh, ());
        let buf = pool.create_buffer(0, 1, 1, 4, wl_shm::Format::Argb8888, qh, ());
        surface.attach(Some(&buf), 0, 0);
        if surface.version() >= 4 {
            surface.damage_buffer(0, 0, 1, 1);
        } else {
            surface.damage(0, 0, 1, 1);
        }
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
        self.panel_hit_areas = PanelHitAreas::default();
        self.pressed_click = None;
        self.ime_state = None;
    }

    fn redraw_panel(&mut self, qh: &QueueHandle<Self>) {
        let hide_candidates = self
            .renderer
            .as_ref()
            .is_some_and(PanelRenderer::hide_candidate_panel);
        if hide_candidates
            && !deadline_active(self.mode_hint_until)
            && self.deployment_text.is_none()
        {
            self.hide_panel_popup();
            return;
        }
        let (Some(renderer), Some(shm), Some(surface)) = (
            self.renderer.as_mut(),
            self.shm.as_ref(),
            self.panel_surface.as_ref(),
        ) else {
            return;
        };

        self.panel_scale.attach_surface(surface, qh);
        let show_hint = deadline_active(self.mode_hint_until);
        let state = self
            .ime_state
            .as_ref()
            .filter(|state| !hide_candidates && !state.candidates.is_empty());
        let deployment_hint =
            keytao_core::DeploymentState::visible_hint(self.deployment_text, state.is_some());
        if state.is_none() && !show_hint && deployment_hint.is_none() {
            return;
        }
        let WaylandPanel {
            pixels,
            width: w,
            height: h,
            logical_width,
            logical_height,
            hit_areas,
        } = renderer.render_wayland(
            state,
            self.ascii_mode,
            self.panel_scale.scale(surface),
            self.panel_scale.screen_width(),
            deployment_hint,
        );
        if w == 0 || h == 0 {
            return;
        }
        // Candidates take clicks; the transient mode hint lets them through.
        if let Some(compositor) = self.compositor.as_ref() {
            if hit_areas.candidates.is_empty() {
                let region = compositor.create_region(qh, ());
                surface.set_input_region(Some(&region));
                region.destroy();
            } else {
                surface.set_input_region(None);
            }
        }
        let stride = w * 4;
        let pool_size = (stride * h) as usize;

        let mut tmp = match tempfile(libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING) {
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

        self.panel_scale
            .configure_surface(surface, logical_width, logical_height);
        surface.attach(Some(&buffer), 0, 0);
        if surface.version() >= 4 {
            surface.damage_buffer(0, 0, w as i32, h as i32);
        } else {
            surface.damage(0, 0, logical_width as i32, logical_height as i32);
        }
        surface.commit();

        self.panel_hit_areas = hit_areas;
        if let Some(old) = self.panel_buffer.replace(buffer) {
            old.destroy();
        }
        pool.destroy();
    }

    fn show_panel(&mut self, state: ImeState, qh: &QueueHandle<Self>) {
        // The content changes, so a press on what was drawn before must not
        // select what is drawn now.
        self.pressed_click = None;
        let has_content = !state.candidates.is_empty()
            && self
                .renderer
                .as_ref()
                .is_some_and(|renderer| !renderer.hide_candidate_panel());
        if !state.candidates.is_empty() {
            // A hint's timeout must never hide candidates typed after it.
            self.mode_hint_until = None;
        }
        let show_hint = deadline_active(self.mode_hint_until);
        self.ime_state = Some(state);
        if has_content || show_hint || self.deployment_text.is_some() {
            if self.panel_surface.is_none() {
                self.create_panel_popup(qh);
            }
            self.panel_visible = true;
            self.redraw_panel(qh);
        } else {
            self.hide_panel_popup();
        }
        self.sync_deployment_aux();
    }

    fn update_ascii_mode(&mut self, ascii_mode: bool, qh: &QueueHandle<Self>) {
        if ascii_mode == self.ascii_mode {
            return;
        }
        self.ascii_mode = ascii_mode;
        // The Plasma indicator keeps the mode it was told about last, so it has
        // to be told again whenever Rime switches.
        self.pending_kimpanel_mode = Some(ascii_mode);
        self.mode_hint_until = Some(mode_hint_deadline(
            self.renderer
                .as_ref()
                .map(PanelRenderer::mode_hint_duration),
        ));
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
        let hidden = self.theme_resolver.current().ui.hide_candidate_panel;
        let (preedit, caret) = inline_preedit(state, hidden);
        if hidden {
            // text-input-v1 highlight style; the span is measured in UTF-8 bytes.
            ctx.preedit_styling(0, preedit.len() as u32, 4);
        }
        ctx.preedit_cursor(preedit_cursor_bytes(preedit, caret));
        // The third argument is what the client commits if the text input is
        // reset while composing (typically on unfocus).  Passing an empty
        // string there throws away whatever the user had typed.
        ctx.preedit_string(self.serial, preedit.to_owned(), state.preedit.clone());
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
        let started = Instant::now();
        tracing::trace!(
            "KDE handling key event: key={evdev_keycode}, state={key_state:?}, active={}",
            self.active
        );
        if key_state == wl_keyboard::KeyState::Released {
            self.handle_key_release(evdev_keycode, qh, started);
            return;
        }

        // A new press always supersedes whatever was repeating.
        self.key_repeat.cancel();

        if !self.active {
            self.forward_key_press(evdev_keycode);
            return;
        }

        // Password and PIN fields: nothing reaches librime and nothing is drawn.
        if !self.session.input_policy().composing {
            self.forward_key_press(evdev_keycode);
            return;
        }

        let sym_raw = key_sym(self.xkb_state.as_ref(), evdev_keycode);
        if sym_raw == xkb::keysyms::KEY_NoSymbol {
            tracing::warn!("KDE IME unmapped key: forwarding without XKB translation");
            self.forward_key_press(evdev_keycode);
            return;
        }

        let base_sym = key_sym(self.xkb_base_state.as_ref(), evdev_keycode);
        let shift_press = is_shift_key(base_sym);
        let effective_mods = if shift_press {
            shift_key_modifiers(self.mods)
        } else {
            self.mods
        };

        self.deployment_text = self.deployment.key_pressed();
        if self.deployment_text.is_some() {
            self.show_panel(self.ime_state.clone().unwrap_or_else(ImeState::empty), qh);
            if let Some(backend) = self
                .panel_surface
                .as_ref()
                .and_then(|surface| surface.backend().upgrade())
            {
                backend.flush().ok();
            }
        }
        let before_state = self.session.state();
        self.refresh_deployment_hint(qh);
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
            self.update_ascii_mode(result.state.is_english_mode(), qh);
            self.commit_state_to_context(&result.state);
            self.update_kimpanel(&result.state);
            self.show_panel(result.state, qh);
            runtime_log::record_keystroke(started.elapsed().as_secs_f64() * 1000.0);
            if !result.accepted {
                self.forward_key_press(evdev_keycode);
            }
            return;
        }

        let keysym = key_policy::candidate_arrow_key(
            if shift_press { base_sym } else { sym_raw },
            effective_mods,
            &before_state,
            candidate_panel_layout(&self.theme_resolver.current()),
        );
        let Some(result) = self.session.process_key_result(keysym, effective_mods) else {
            self.forward_key_press(evdev_keycode);
            return;
        };
        let ime_state = result.state;

        self.update_ascii_mode(ime_state.is_english_mode(), qh);

        if result.accepted {
            self.commit_state_to_context(&ime_state);
            self.update_kimpanel(&ime_state);
            self.show_panel(ime_state, qh);
            if key_policy::should_forward_consumed_shortcut(sym_raw, effective_mods) {
                self.forward_key_press(evdev_keycode);
            } else {
                // The grab took this key away from KWin, so a held
                // Backspace/arrow/select key has to repeat from here.
                self.key_repeat.arm(
                    evdev_keycode,
                    key_repeats(self.xkb_keymap.as_ref(), evdev_keycode),
                );
            }
        } else {
            // librime rejected the key, but it can still have flushed a commit
            // on the way out (the ascii_composer confirms the composition when
            // it switches mode). Publish the actual composition as well; a
            // rejected key does not mean Rime stopped composing.
            self.commit_state_to_context(&ime_state);
            self.update_kimpanel(&ime_state);
            self.show_panel(ime_state, qh);
            self.forward_key_press(evdev_keycode);
        }
        runtime_log::record_keystroke(started.elapsed().as_secs_f64() * 1000.0);
    }

    /// A primary click on the panel: the same session calls and client updates
    /// as a number-key selection or a page key.
    fn handle_panel_click(&mut self, click: PanelClick, qh: &QueueHandle<Self>) {
        if !self.active || !self.session.input_policy().composing {
            return;
        }
        let state = match click {
            PanelClick::Candidate(index) => self.session.select_candidate_on_page(index),
            PanelClick::PreviousPage => self.session.change_page(true),
            PanelClick::NextPage => self.session.change_page(false),
        };
        let Some(state) = state else { return };
        self.update_ascii_mode(state.is_english_mode(), qh);
        self.commit_state_to_context(&state);
        self.update_kimpanel(&state);
        self.show_panel(state, qh);
    }

    fn handle_key_release(&mut self, evdev_keycode: u32, qh: &QueueHandle<Self>, started: Instant) {
        self.key_repeat.cancel_for(evdev_keycode);
        let sym_raw = key_sym(self.xkb_base_state.as_ref(), evdev_keycode);
        if is_shift_key(sym_raw) && self.session.input_policy().composing {
            if let Some(result) = self
                .session
                .process_key_result(sym_raw, shift_key_modifiers(self.mods | RIME_RELEASE_MASK))
            {
                self.update_ascii_mode(result.state.is_english_mode(), qh);
                self.commit_state_to_context(&result.state);
                self.update_kimpanel(&result.state);
                self.show_panel(result.state, qh);
                runtime_log::record_keystroke(started.elapsed().as_secs_f64() * 1000.0);
            }
        } else if self.active && self.session.input_policy().composing {
            let sym = key_sym(self.xkb_state.as_ref(), evdev_keycode);
            if key_policy::should_forward_chord_release(sym, self.mods, &self.session.state()) {
                if let Some(result) = self
                    .session
                    .process_key_result(sym, self.mods | RIME_RELEASE_MASK)
                {
                    self.update_ascii_mode(result.state.is_english_mode(), qh);
                    self.commit_state_to_context(&result.state);
                    self.update_kimpanel(&result.state);
                    self.show_panel(result.state, qh);
                    runtime_log::record_keystroke(started.elapsed().as_secs_f64() * 1000.0);
                    if result.accepted {
                        self.forwarded_keys.remove(&evdev_keycode);
                        return;
                    }
                }
                // Rime may reject the release after consuming the chord press.
                self.forwarded_keys.remove(&evdev_keycode);
                self.forward_key(evdev_keycode, wl_keyboard::KeyState::Released as u32);
                return;
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
        if let wl_registry::Event::GlobalRemove { name } = &event {
            state.panel_scale.remove_global(*name);
            if state.panel_visible {
                state.pressed_click = None;
                state.redraw_panel(qh);
            }
            return;
        }
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            if state
                .panel_scale
                .bind_global(registry, name, &interface, version, qh)
            {
                if state.panel_visible {
                    state.pressed_click = None;
                    state.redraw_panel(qh);
                }
                return;
            }
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
                    state
                        .key_repeat
                        .set_info(FALLBACK_REPEAT_RATE, FALLBACK_REPEAT_DELAY);
                }
                state.keyboard = Some(keyboard);
                state.context = Some(id);
                state.active = true;
            }
            zwp_input_method_v1::Event::Deactivate { context } => {
                tracing::info!("KDE input-method-v1 context deactivated!");
                // KWin has already deleted this context and its keyboard.
                // Do not debounce protocol cleanup: queued keys, reloads and
                // repeat timers must never send requests through these proxies.
                if state.context.as_ref().is_some_and(|live| *live == context) {
                    state.reset_context_state();
                    runtime_log::drain_key_stats();
                } else {
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
        // Old context events must not reset the replacement's composition.
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
                state.deployment.reset();
                state.deployment_text = None;
                if let Some(kimpanel) = &state.kimpanel {
                    kimpanel.show_deployment_hint(None);
                }
                state.key_repeat.cancel();
                state.session.reset();
                state.clear_context_preedit();
                state.clear_kimpanel();
                state.hide_panel_popup();
            }
            _ => {}
        }
    }
}

impl Dispatch<WlKeyboard, ()> for App {
    fn event(
        state: &mut Self,
        proxy: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if !state.active || state.keyboard.as_ref() != Some(proxy) {
            return;
        }
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
                    state.xkb_base_state = Some(xkb::State::new(&km));
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
                state.key_repeat.set_info(rate, delay);
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
                    if xkb_state.mod_name_is_active(xkb::MOD_NAME_CAPS, xkb::STATE_MODS_EFFECTIVE) {
                        m |= RIME_MOD_LOCK;
                    }
                    if xkb_state.mod_name_is_active(xkb::MOD_NAME_LOGO, xkb::STATE_MODS_EFFECTIVE) {
                        m |= RIME_MOD_SUPER;
                    }
                    state.mods = m;
                }
                state.forward_modifiers(serial, mods_depressed, mods_latched, mods_locked, group);
            }
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for App {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            let has_pointer = caps.contains(wl_seat::Capability::Pointer);
            if has_pointer && state.pointer.is_none() {
                state.pointer = Some(seat.get_pointer(qh, ()));
            } else if !has_pointer {
                if let Some(pointer) = state.pointer.take() {
                    if pointer.version() >= 3 {
                        pointer.release();
                    }
                }
                state.pointer_pos = None;
                state.pressed_click = None;
            }
        }
    }
}

/// Pointer input on the candidate panel.  The overlay panel never takes
/// keyboard focus (KWin keeps it on the text-input client), so clicking it
/// leaves the client focused.
impl Dispatch<WlPointer, ()> for App {
    fn event(
        state: &mut Self,
        _pointer: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter {
                surface,
                surface_x,
                surface_y,
                ..
            } => {
                state.pointer_pos = (state.panel_surface.as_ref() == Some(&surface))
                    .then_some((surface_x, surface_y));
            }
            wl_pointer::Event::Leave { .. } => {
                state.pointer_pos = None;
                state.pressed_click = None;
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                if state.pointer_pos.is_some() {
                    state.pointer_pos = Some((surface_x, surface_y));
                }
            }
            wl_pointer::Event::Button {
                button,
                state: button_state,
                ..
            } if button == BTN_LEFT => {
                let Some((x, y)) = state.pointer_pos else {
                    return;
                };
                let click = state.panel_hit_areas.click_at(x, y);
                match button_state {
                    WEnum::Value(wl_pointer::ButtonState::Pressed) => state.pressed_click = click,
                    WEnum::Value(wl_pointer::ButtonState::Released) => {
                        if let Some(click) =
                            state.pressed_click.take().filter(|p| click == Some(*p))
                        {
                            state.handle_panel_click(click, qh);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}
delegate_noop!(App: ignore WlCompositor);
delegate_noop!(App: ignore WlShm);
delegate_noop!(App: ignore WlShmPool);
delegate_noop!(App: ignore WlBuffer);
delegate_noop!(App: ignore WpFractionalScaleManagerV1);
delegate_noop!(App: ignore WpViewporter);
delegate_noop!(App: ignore WpViewport);
delegate_noop!(App: ignore ZxdgOutputManagerV1);

impl Dispatch<ZxdgOutputV1, u32> for App {
    fn event(
        state: &mut Self,
        _: &ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        name: &u32,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let zxdg_output_v1::Event::LogicalSize { width, .. } = event {
            state.panel_scale.output_logical_width(*name, width);
            if state.panel_visible {
                state.pressed_click = None;
                state.redraw_panel(qh);
            }
        }
    }
}

impl Dispatch<WlSurface, ()> for App {
    fn event(
        state: &mut Self,
        surface: &WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if state.panel_surface.as_ref() != Some(surface) {
            return;
        }
        state.panel_scale.surface_event(&event);
        if state.panel_visible {
            state.pressed_click = None;
            state.redraw_panel(qh);
        }
    }
}

impl Dispatch<WlOutput, u32> for App {
    fn event(
        state: &mut Self,
        output: &WlOutput,
        event: wl_output::Event,
        _: &u32,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        state.panel_scale.output_event(output, &event);
        if state.panel_visible
            && matches!(
                event,
                wl_output::Event::Scale { .. } | wl_output::Event::Done
            )
        {
            state.pressed_click = None;
            state.redraw_panel(qh);
        }
    }
}

impl Dispatch<WpFractionalScaleV1, ()> for App {
    fn event(
        state: &mut Self,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        state.panel_scale.fractional_event(&event);
        if state.panel_visible {
            state.pressed_click = None;
            state.redraw_panel(qh);
        }
    }
}
delegate_noop!(App: ignore ZwpInputPanelV1);
delegate_noop!(App: ignore ZwpInputPanelSurfaceV1);
delegate_noop!(App: ignore WlRegion);

pub fn run(engine: CoreEngine, frontend: crate::frontend::Frontend) -> Result<(), String> {
    let session = crate::engine::create_session(&engine)
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
    app.panel_scale.bind_initial(&globals, &qh);
    app.compositor = Some(compositor);
    app.shm = Some(shm);
    app.seat = Some(seat);
    app.input_method = Some(input_method);
    app.input_panel = Some(input_panel);
    queue
        .roundtrip(&mut app)
        .map_err(|e| format!("initial KDE roundtrip: {e}"))?;
    for g in globals.contents().clone_list() {
        tracing::info!("KDE Wayland global: {} v{}", g.interface, g.version);
    }

    tracing::info!("KDE Wayland IME running (input-method-v1)");
    frontend.connected();
    run_event_loop(&conn, &mut queue, &mut app, &kimpanel_runtime)
}

fn run_event_loop(
    conn: &Connection,
    queue: &mut EventQueue<App>,
    app: &mut App,
    kimpanel_runtime: &tokio::runtime::Runtime,
) -> Result<(), String> {
    let qh = queue.handle();
    let reload_signal = reload_bus::subscribe();
    let mut activity = Some(crate::hot_update::activity());
    loop {
        activity.get_or_insert_with(crate::hot_update::activity);
        queue
            .dispatch_pending(app)
            .map_err(|e| format!("KDE Wayland dispatch: {e}"))?;
        app.refresh_deployment_hint(&qh);
        if take_expired_deadline(&mut app.mode_hint_until) {
            app.show_panel(ImeState::empty(), &qh);
        }

        if app.key_repeat.due() {
            app.fire_key_repeat(&qh);
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
        // Prepare the read before polling, as required by wayland-client. A
        // pending event must be dispatched even if the socket is no longer
        // readable; blocking_dispatch after poll can otherwise miss it.
        let Some(read_guard) = queue.prepare_read() else {
            continue;
        };
        let write_blocked = match queue.flush() {
            Ok(()) => {
                drop(activity.take());
                false
            }
            Err(wayland_client::backend::WaylandError::Io(e))
                if e.kind() == std::io::ErrorKind::WouldBlock =>
            {
                true
            }
            Err(e) => return Err(format!("KDE Wayland flush: {e}")),
        };
        let timeout_ms = app.poll_timeout_ms();
        let mut pfds = vec![libc::pollfd {
            fd: conn.as_fd().as_raw_fd(),
            events: libc::POLLIN | if write_blocked { libc::POLLOUT } else { 0 },
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
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("KDE Wayland poll: {err}"));
        }
        if pfds[0].revents & libc::POLLIN != 0 {
            match read_guard.read() {
                Ok(_) => {}
                Err(wayland_client::backend::WaylandError::Io(e))
                    if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(format!("KDE Wayland connection closed: {e}")),
            }
        } else {
            drop(read_guard);
        }
        if pfds[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
            return Err(format!(
                "KDE Wayland socket disconnected: {:#x}",
                pfds[0].revents
            ));
        }
        // Handle deactivate before a simultaneous reload can clear preedit
        // through a context KWin has just deleted.
        activity.get_or_insert_with(crate::hot_update::activity);
        queue
            .dispatch_pending(app)
            .map_err(|e| format!("KDE Wayland dispatch: {e}"))?;
        if let Some(signal) = &reload_signal {
            if signal.take() {
                app.handle_reload();
            }
        }
    }
}

#[cfg(test)]
#[path = "wayland_backend_kde_protocol_tests.rs"]
mod protocol_tests;

#[cfg(test)]
mod tests {
    use super::{content_type_policy, FALLBACK_REPEAT_DELAY, FALLBACK_REPEAT_RATE};
    use crate::wayland_common::repeat_timings;
    use std::time::Duration;

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
}
