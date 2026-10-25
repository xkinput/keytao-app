//! Linux-facing aliases for the shared IME runtime in keytao-core.
//!
//! Linux protocol backends should not manage librime sessions directly.  They
//! create sessions through `CoreEngine` and send normalized X11 keysym events.

pub use keytao_core::{ImeRuntime as CoreEngine, ImeRuntimeSession as ImeSession};
use keytao_core::{ImeState, InputContextPolicy};

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

#[cfg(test)]
mod tests {
    use super::{
        ibus_content_type_policy, ibus_cursor_pos, shift_key_modifiers, unhandled_key_text,
        ImeState, IBUS_INPUT_HINT_PRIVATE, IBUS_INPUT_PURPOSE_PASSWORD, IBUS_INPUT_PURPOSE_PIN,
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
}
