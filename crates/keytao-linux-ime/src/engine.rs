//! Linux-facing aliases for the shared IME runtime in keytao-core.
//!
//! Linux protocol backends should not manage librime sessions directly.  They
//! create sessions through `CoreEngine` and send normalized X11 keysym events.

pub use keytao_core::ImeRuntime as CoreEngine;
use keytao_core::{
    key_policy, ImeState, InputContextPolicy, KeyProcessResult, RIME_MOD_ALT, RIME_MOD_CONTROL,
    RIME_MOD_SHIFT, RIME_RELEASE_MASK,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct DeploymentHint {
    root: Option<std::path::PathBuf>,
    state: Mutex<keytao_core::DeploymentState>,
}

impl Default for DeploymentHint {
    fn default() -> Self {
        let root = keytao_core::default_user_data_dir();
        let mut state = keytao_core::DeploymentState::default();
        if let Some(root) = &root {
            state.hint(root);
        }
        Self {
            root,
            state: Mutex::new(state),
        }
    }
}

impl DeploymentHint {
    pub fn key_pressed(&self) -> Option<&'static str> {
        if !crate::reload_bus::engine_enabled() {
            return None;
        }
        let root = self.root.as_ref()?;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.key_pressed(root);
        state.cached_hint()
    }

    pub fn hint(&self) -> Option<&'static str> {
        if !crate::reload_bus::engine_enabled() {
            return None;
        }
        let root = self.root.as_ref()?;
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .hint(root)
    }

    pub fn reset(&self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .reset_input();
    }
}

pub type ImeSession = Arc<keytao_core::ImeRuntimeSession>;

pub fn candidate_panel_layout(
    theme: &keytao_theme::ResolvedImeTheme,
) -> keytao_core::key_policy::CandidatePanelLayout {
    use keytao_core::key_policy::CandidatePanelLayout;
    use keytao_theme::PanelOrientation;

    if theme.ui.hide_candidate_panel {
        CandidatePanelLayout::Hidden
    } else {
        match theme.panel.orientation {
            PanelOrientation::Horizontal => CandidatePanelLayout::Horizontal,
            PanelOrientation::Vertical => CandidatePanelLayout::Vertical,
        }
    }
}

pub fn create_session(engine: &CoreEngine) -> Result<ImeSession, String> {
    let _activity = crate::hot_update::activity();
    let session = Arc::new(engine.create_session()?);
    crate::hot_update::register_session(&session);
    Ok(session)
}

/// Text fallback for a Wayland grab without a virtual keyboard. Shortcut
/// chords must not turn into literal characters in the document.
pub fn unhandled_key_text(text: &str, mods: u32) -> Option<char> {
    use keytao_core::{key_policy, RIME_MOD_LOCK, RIME_MOD_SHIFT};
    if mods & !(RIME_MOD_SHIFT | RIME_MOD_LOCK) != 0 {
        return None;
    }
    key_policy::keysym_for_text(text).and_then(key_policy::char_for_keysym)
}

/// A solo Shift must not carry its own modifier into ascii_composer. A Shift
/// chord must carry it: otherwise Ctrl+Shift looks like one modifier and Rime
/// arms a solo-Shift toggle. Preserve Release and every other modifier.
pub fn shift_key_modifiers(mods: u32) -> u32 {
    use keytao_core::{key_policy, RIME_MOD_ALT, RIME_MOD_CONTROL, RIME_MOD_SHIFT};
    if mods & (RIME_MOD_CONTROL | RIME_MOD_ALT) != 0
        || key_policy::is_system_reserved_modifier(mods)
    {
        mods | RIME_MOD_SHIFT
    } else {
        mods & !RIME_MOD_SHIFT
    }
}

#[derive(Default)]
pub(crate) struct ControlAltTap(Mutex<Option<(u32, Instant)>>);

impl ControlAltTap {
    const X11_MOD5: u32 = 1 << 7;

    fn mask(sym: u32) -> Option<u32> {
        match sym {
            0xffe3 | 0xffe4 => Some(RIME_MOD_CONTROL),
            0xffe9 | 0xffea => Some(RIME_MOD_ALT),
            _ => None,
        }
    }

    fn solo(mods: u32, own: u32) -> bool {
        const X11_MOD4: u32 = 1 << 6;
        let other = (RIME_MOD_CONTROL | RIME_MOD_ALT | RIME_MOD_SHIFT) & !own;
        mods & (other | X11_MOD4 | Self::X11_MOD5) == 0
            && !key_policy::is_system_reserved_modifier(mods)
    }

    pub(crate) fn xkb_modifiers(sym: u32, mods: u32, state: Option<&xkbcommon::xkb::State>) -> u32 {
        if Self::mask(sym).is_some()
            && state.is_some_and(|state| {
                state.mod_name_is_active("Mod5", xkbcommon::xkb::STATE_MODS_EFFECTIVE)
            })
        {
            return mods | Self::X11_MOD5;
        }
        mods
    }

    pub(crate) fn press(&self, sym: u32, mods: u32) -> bool {
        let mut key = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = key.take();
        let Some(own) = Self::mask(sym) else {
            return false;
        };
        if previous.is_none() && Self::solo(mods, own) {
            *key = Some((sym, Instant::now()));
        }
        true
    }

    fn take_tap(&self, sym: u32, mods: u32) -> bool {
        let Some(own) = Self::mask(sym) else {
            return false;
        };
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .is_some_and(|(key, started)| {
                key == sym && started.elapsed() < Duration::from_millis(500) && Self::solo(mods, own)
            })
    }

    pub(crate) fn release(&self, sym: u32, mods: u32, session: &ImeSession) -> Option<KeyProcessResult> {
        // Delay the Rime press to avoid arming a tap for keys already handled by the host.
        if self.take_tap(sym, mods)
            && session.input_policy().composing
            && key_policy::is_configured_modifier_tap(sym, &session.state())
        {
            let mut result = session.process_keys_result(&[(sym, 0), (sym, RIME_RELEASE_MASK)])?;
            result.accepted = false;
            return Some(result);
        }
        None
    }

    pub(crate) fn clear(&self) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

/// `IBusInputPurpose::PASSWORD`, mirroring `GtkInputPurpose` and
/// `zwp_text_input_v3::content_purpose`.
pub const IBUS_INPUT_PURPOSE_PASSWORD: u32 = 8;
/// `IBusInputPurpose::PIN`.
pub const IBUS_INPUT_PURPOSE_PIN: u32 = 9;
/// `IBusInputHints::PRIVATE`: the client asks the input method not to remember
/// what is typed.  Note that the Wayland `content_hint` bit for the same idea
/// (`sensitive_data`) has a different value and belongs to the Wayland backends.
pub const IBUS_INPUT_HINT_PRIVATE: u32 = 1 << 11;

/// Whether an IBus `content-type` describes a secret the engine must not touch.
pub fn is_sensitive_ibus_content_type(purpose: u32, hints: u32) -> bool {
    matches!(
        purpose,
        IBUS_INPUT_PURPOSE_PASSWORD | IBUS_INPUT_PURPOSE_PIN
    ) || hints & IBUS_INPUT_HINT_PRIVATE != 0
}

/// The session policy an IBus `content-type` maps to.
///
/// Password and PIN fields get a policy that keeps every key away from librime,
/// so no preedit, no candidate window and no user-dictionary learning can leak
/// what was typed.
pub fn ibus_content_type_policy(purpose: u32, hints: u32) -> InputContextPolicy {
    if is_sensitive_ibus_content_type(purpose, hints) {
        InputContextPolicy::sensitive()
    } else {
        InputContextPolicy::default()
    }
}

/// The caret offset an IBusText carries.
///
/// `ImeState::cursor` counts Unicode scalars and so does `IBusText`, but a
/// stale state could still point past the end of the preedit, which makes the
/// GTK client walk off the string.
pub fn ibus_cursor_pos(state: &ImeState) -> u32 {
    state.cursor.min(state.preedit.chars().count()) as u32
}

/// Inline text and caret in Unicode scalars; Wayland converts the caret to bytes.
pub fn inline_preedit(state: &ImeState, hide_candidate_panel: bool) -> (&str, usize) {
    if hide_candidate_panel {
        let text = if state.commit_preview.is_empty() {
            &state.preedit
        } else {
            &state.commit_preview
        };
        (text, text.chars().count())
    } else {
        (&state.preedit, ibus_cursor_pos(state) as usize)
    }
}

pub fn client_preedit<'a>(
    state: &'a ImeState,
    ui: &keytao_theme::UiTheme,
    has_floating_panel: bool,
) -> (&'a str, usize) {
    if ui.effective_embedded_composition() || !has_floating_panel {
        inline_preedit(
            state,
            ui.hide_candidate_panel && !state.is_package_switcher(),
        )
    } else {
        ("", 0)
    }
}

pub fn panel_has_composition(state: &ImeState, ui: &keytao_theme::UiTheme) -> bool {
    (!state.candidates.is_empty() && (!ui.hide_candidate_panel || state.is_package_switcher()))
        || (!ui.effective_embedded_composition() && !state.preedit.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{
        client_preedit, ibus_content_type_policy, ibus_cursor_pos, inline_preedit,
        panel_has_composition, shift_key_modifiers, unhandled_key_text, ControlAltTap, ImeState,
        IBUS_INPUT_HINT_PRIVATE, IBUS_INPUT_PURPOSE_PASSWORD, IBUS_INPUT_PURPOSE_PIN,
    };
    use keytao_core::rime_modifier_mask;

    #[test]
    fn shift_chords_keep_both_modifiers_on_press_and_release() {
        use keytao_core::{
            RIME_MOD_ALT, RIME_MOD_CONTROL, RIME_MOD_SHIFT, RIME_MOD_SUPER, RIME_RELEASE_MASK,
        };
        assert_eq!(shift_key_modifiers(RIME_MOD_SHIFT), 0);
        assert_eq!(
            shift_key_modifiers(RIME_MOD_SHIFT | RIME_RELEASE_MASK),
            RIME_RELEASE_MASK
        );
        for other in [RIME_MOD_CONTROL, RIME_MOD_ALT, RIME_MOD_SUPER] {
            for shift in [0, RIME_MOD_SHIFT] {
                assert_eq!(shift_key_modifiers(other | shift), other | RIME_MOD_SHIFT);
                assert_eq!(
                    shift_key_modifiers(other | shift | RIME_RELEASE_MASK),
                    other | RIME_MOD_SHIFT | RIME_RELEASE_MASK
                );
            }
        }
    }

    #[test]
    fn control_alt_taps_ignore_chords_and_altgr() {
        use keytao_core::{RIME_MOD_ALT, RIME_MOD_CONTROL, RIME_MOD_SUPER};

        let control = 0xffe3;
        let alt = 0xffea;
        let tap = ControlAltTap::default();
        assert!(tap.press(control, RIME_MOD_CONTROL));
        assert!(tap.take_tap(control, 0));
        assert!(tap.press(alt, RIME_MOD_ALT));
        assert!(tap.take_tap(alt, 0));

        assert!(tap.press(control, 0));
        assert!(!tap.press(b' ' as u32, RIME_MOD_CONTROL));
        assert!(!tap.take_tap(control, 0));
        assert!(tap.press(control, 0));
        tap.clear();
        assert!(!tap.take_tap(control, 0));
        for mods in [RIME_MOD_CONTROL, RIME_MOD_SUPER, ControlAltTap::X11_MOD5] {
            assert!(tap.press(alt, mods));
            assert!(!tap.take_tap(alt, 0));
        }
        assert!(!tap.press(0xfe03, 0)); // ISO_Level3_Shift (AltGr)
        assert!(!tap.take_tap(alt, 0));
    }

    #[test]
    fn unhandled_printable_keys_survive_without_a_virtual_keyboard() {
        use keytao_core::{RIME_MOD_LOCK, RIME_MOD_SHIFT};
        for ch in "0123456789abc!é键".chars() {
            for mods in [0, RIME_MOD_SHIFT, RIME_MOD_LOCK] {
                assert_eq!(unhandled_key_text(&ch.to_string(), mods), Some(ch));
            }
        }
    }

    #[test]
    fn text_fallback_never_inserts_shortcuts_or_function_keys() {
        use keytao_core::{RIME_MOD_ALT, RIME_MOD_CONTROL, RIME_MOD_SUPER};
        for mods in [RIME_MOD_CONTROL, RIME_MOD_ALT, RIME_MOD_SUPER] {
            assert_eq!(unhandled_key_text("1", mods), None);
        }
        for text in ["", "\0", "\n", "\t", "\u{8}", "ab"] {
            assert_eq!(unhandled_key_text(text, 0), None);
        }
    }

    #[test]
    fn rime_modifier_mask_strips_lock_and_pointer_modifiers() {
        assert_eq!(rime_modifier_mask(0x10), 0);
        assert_eq!(rime_modifier_mask(0x10 | 0x0001 | 0x0004), 0x0001 | 0x0004);
        assert_eq!(rime_modifier_mask((1 << 30) | 0x10), 1 << 30);
    }

    #[test]
    fn password_and_pin_content_types_stop_composing() {
        assert!(!ibus_content_type_policy(IBUS_INPUT_PURPOSE_PASSWORD, 0).composing);
        assert!(!ibus_content_type_policy(IBUS_INPUT_PURPOSE_PASSWORD, 0).learning);
        assert!(!ibus_content_type_policy(IBUS_INPUT_PURPOSE_PIN, 0).composing);
        assert!(!ibus_content_type_policy(0, IBUS_INPUT_HINT_PRIVATE).composing);
    }

    #[test]
    fn ordinary_content_types_keep_composing() {
        assert!(ibus_content_type_policy(0, 0).composing);
        // FREE_FORM with spellcheck hints stays a normal text field.
        assert!(ibus_content_type_policy(0, 0x1).composing);
        // TERMINAL is not a secret, only an unusual text field.
        assert!(ibus_content_type_policy(10, 0).composing);
    }

    #[test]
    fn cursor_pos_is_clamped_to_the_preedit_length() {
        let mut state = ImeState::empty();
        state.preedit = "你好ni".to_owned();
        state.cursor = 3;
        assert_eq!(ibus_cursor_pos(&state), 3);
        state.cursor = 99;
        assert_eq!(ibus_cursor_pos(&state), 4);
        state.preedit = String::new();
        assert_eq!(ibus_cursor_pos(&state), 0);
    }

    #[test]
    fn hidden_inline_uses_commit_preview_and_an_end_caret() {
        let mut state = ImeState::empty();
        state.preedit = "你好f".into();
        state.commit_preview = "你好这".into();
        state.cursor = 2;
        assert_eq!(inline_preedit(&state, true), ("你好这", 3));
        assert_eq!(inline_preedit(&state, false), ("你好f", 2));
        state.commit_preview.clear();
        assert_eq!(inline_preedit(&state, true), ("你好f", 3));
        state.preedit.clear();
        assert_eq!(inline_preedit(&state, true), ("", 0));
    }

    #[test]
    fn preedit_moves_between_client_and_panel_without_losing_raw_input() {
        let mut ui = keytao_theme::ResolvedImeTheme::default().ui;
        let mut state = ImeState::empty();
        state.preedit = "你好f".into();
        state.commit_preview = "你好这".into();
        state.cursor = 2;

        ui.embedded_composition = true;
        assert_eq!(client_preedit(&state, &ui, true), ("你好f", 2));
        assert!(!panel_has_composition(&state, &ui));

        ui.embedded_composition = false;
        assert_eq!(client_preedit(&state, &ui, true), ("", 0));
        assert!(panel_has_composition(&state, &ui));
        assert_eq!(client_preedit(&state, &ui, false), ("你好f", 2));

        ui.hide_candidate_panel = true;
        assert_eq!(client_preedit(&state, &ui, true), ("你好这", 3));
        assert!(!panel_has_composition(&state, &ui));

        let mut state = ImeState::empty();
        state.schema_id = keytao_core::switcher_model::SCHEMA_ID.into();
        state.candidates.push(keytao_core::Candidate {
            text: "Rime".into(),
            comment: None,
        });
        assert!(panel_has_composition(&state, &ui));
        assert_eq!(client_preedit(&state, &ui, true), ("", 0));
    }
}
