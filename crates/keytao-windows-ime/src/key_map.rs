//! Windows Virtual Key → X11 keysym + modifier mask conversion.
//!
//! librime uses X11 keysyms internally (same convention as the Linux backends).
//! The Wayland backend converts hardware keycodes via xkbcommon; here we convert
//! Windows VK codes directly.

pub use keytao_core::{
    key_policy, RIME_MOD_ALT, RIME_MOD_CONTROL, RIME_MOD_LOCK, RIME_MOD_SHIFT, RIME_MOD_SUPER,
    RIME_RELEASE_MASK,
};

const XK_PAGE_UP: u32 = 0xFF55;
const XK_PAGE_DOWN: u32 = 0xFF56;
const TOUCH_KEYBOARD_NEXT_PAGE: u32 = 0xF003;
const TOUCH_KEYBOARD_PREVIOUS_PAGE: u32 = 0xF004;

/// Read Shift, Control, Alt, the Windows key and the CapsLock toggle into an
/// X11 modifier mask.
///
/// CapsLock is reported as `RIME_MOD_LOCK`; `keytao_core` folds it into the
/// keysym and strips the bit before librime sees the key, so the engine
/// receives the character the layout actually produces.
pub fn current_mod_mask() -> u32 {
    let mut mask = 0u32;
    unsafe {
        use windows::Win32::UI::Input::KeyboardAndMouse::*;
        if GetKeyState(VK_SHIFT.0 as i32) as u16 & 0x8000 != 0 {
            mask |= RIME_MOD_SHIFT;
        }
        if GetKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000 != 0 {
            mask |= RIME_MOD_CONTROL;
        }
        if GetKeyState(VK_MENU.0 as i32) as u16 & 0x8000 != 0 {
            mask |= RIME_MOD_ALT;
        }
        if GetKeyState(VK_LWIN.0 as i32) as u16 & 0x8000 != 0
            || GetKeyState(VK_RWIN.0 as i32) as u16 & 0x8000 != 0
        {
            mask |= RIME_MOD_SUPER;
        }
        if GetKeyState(VK_CAPITAL.0 as i32) as u16 & 0x0001 != 0 {
            mask |= RIME_MOD_LOCK;
        }
    }
    mask
}

/// Whether CapsLock or Shift (but not both) is in effect for letter keys.
fn letters_are_uppercase(mods: u32) -> bool {
    (mods & RIME_MOD_LOCK != 0) != (mods & RIME_MOD_SHIFT != 0)
}

/// Convert a Windows Virtual Key code to an X11 keysym.
///
/// Returns `None` for keys the IME should never eat (e.g. media keys).
pub fn vk_to_keysym(vk: u16, lparam: isize, mods: u32) -> Option<u32> {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;

    let sym = match VIRTUAL_KEY(vk) {
        VK_PACKET => packet_keysym()?,

        // --- Printable ASCII letters ---
        // Match Linux/macOS semantics: the keysym reflects the produced
        // printable character while the modifier mask still carries Shift.
        // CapsLock and Shift cancel each other out, which is what the layout
        // produces and what librime's ascii_composer expects to see.
        key if is_letter_vk(key) => {
            let base = if letters_are_uppercase(mods) {
                b'A'
            } else {
                b'a'
            };
            base as u32 + (key.0 - VK_A.0) as u32
        }

        // Printable keys are resolved with the active keyboard layout. This
        // preserves non-US OEM punctuation and shifted number-row symbols.
        key if is_printable_vk(key) => printable_keysym(key, vk, lparam, mods)?,

        // --- Numpad ---
        VK_NUMPAD0 => 0x0030,
        VK_NUMPAD1 => 0x0031,
        VK_NUMPAD2 => 0x0032,
        VK_NUMPAD3 => 0x0033,
        VK_NUMPAD4 => 0x0034,
        VK_NUMPAD5 => 0x0035,
        VK_NUMPAD6 => 0x0036,
        VK_NUMPAD7 => 0x0037,
        VK_NUMPAD8 => 0x0038,
        VK_NUMPAD9 => 0x0039,
        VK_MULTIPLY => 0x002A,
        VK_ADD => 0x002B,
        VK_SUBTRACT => 0x002D,
        VK_DECIMAL => 0x002E,
        VK_DIVIDE => 0x002F,

        // --- Control keys ---
        VK_BACK => 0xFF08,   // XK_BackSpace
        VK_TAB => 0xFF09,    // XK_Tab
        VK_RETURN => 0xFF0D, // XK_Return
        VK_ESCAPE => 0xFF1B, // XK_Escape
        VK_SPACE => 0x0020,  // XK_space
        VK_DELETE => 0xFFFF, // XK_Delete

        // --- Navigation ---
        VK_LEFT => 0xFF51,       // XK_Left
        VK_UP => 0xFF52,         // XK_Up
        VK_RIGHT => 0xFF53,      // XK_Right
        VK_DOWN => 0xFF54,       // XK_Down
        VK_HOME => 0xFF50,       // XK_Home
        VK_END => 0xFF57,        // XK_End
        VK_PRIOR => XK_PAGE_UP,  // XK_Prior (Page Up)
        VK_NEXT => XK_PAGE_DOWN, // XK_Next  (Page Down)
        VK_F4 => 0xFFC1,         // XK_F4 opens the Rime menu

        // Anything else (other function keys, media keys, etc.) — don't intercept
        _ => return None,
    };
    Some(sym)
}

enum PrintableTranslation {
    Character(u32),
    DeadKey,
    Unavailable,
}

fn printable_keysym(
    key: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY,
    vk: u16,
    lparam: isize,
    mods: u32,
) -> Option<u32> {
    if mods & RIME_MOD_CONTROL != 0 {
        // ToUnicodeEx folds Control chords into control characters (Ctrl+[ →
        // U+001B), which would reach librime as an unrelated keysym. Rime wants
        // the unmodified character plus the Control mask.
        return fallback_printable_keysym(key, mods);
    }
    match translated_printable_keysym(vk, lparam) {
        PrintableTranslation::Character(keysym) => Some(keysym),
        PrintableTranslation::DeadKey => None,
        PrintableTranslation::Unavailable => fallback_printable_keysym(key, mods),
    }
}

fn translated_printable_keysym(vk: u16, lparam: isize) -> PrintableTranslation {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyboardLayout, GetKeyboardState, ToUnicodeEx,
    };

    let mut keyboard_state = [0u8; 256];
    unsafe {
        if GetKeyboardState(&mut keyboard_state).is_err() {
            return PrintableTranslation::Unavailable;
        }
        let scan_code = ((lparam as usize >> 16) & 0xff) as u32;
        let mut buffer = [0u16; 4];
        // Bit 2 asks Windows 10 1607+ not to mutate the keyboard dead-key state.
        let count = ToUnicodeEx(
            vk as u32,
            scan_code,
            &keyboard_state,
            &mut buffer,
            4,
            GetKeyboardLayout(0),
        );
        if count < 0 {
            return PrintableTranslation::DeadKey;
        }
        if count != 1 {
            return PrintableTranslation::Unavailable;
        }
        let Some(ch) = char::from_u32(buffer[0] as u32) else {
            return PrintableTranslation::Unavailable;
        };
        PrintableTranslation::Character(unicode_to_keysym(ch))
    }
}

fn unicode_to_keysym(ch: char) -> u32 {
    let value = ch as u32;
    if value <= 0xff {
        value
    } else {
        0x0100_0000 | value
    }
}

fn fallback_printable_keysym(
    vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY,
    mods: u32,
) -> Option<u32> {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;

    let shifted = mods & RIME_MOD_SHIFT != 0;
    let value = match vk {
        VK_0 => {
            if shifted {
                ')'
            } else {
                '0'
            }
        }
        VK_1 => {
            if shifted {
                '!'
            } else {
                '1'
            }
        }
        VK_2 => {
            if shifted {
                '@'
            } else {
                '2'
            }
        }
        VK_3 => {
            if shifted {
                '#'
            } else {
                '3'
            }
        }
        VK_4 => {
            if shifted {
                '$'
            } else {
                '4'
            }
        }
        VK_5 => {
            if shifted {
                '%'
            } else {
                '5'
            }
        }
        VK_6 => {
            if shifted {
                '^'
            } else {
                '6'
            }
        }
        VK_7 => {
            if shifted {
                '&'
            } else {
                '7'
            }
        }
        VK_8 => {
            if shifted {
                '*'
            } else {
                '8'
            }
        }
        VK_9 => {
            if shifted {
                '('
            } else {
                '9'
            }
        }
        VK_OEM_MINUS => {
            if shifted {
                '_'
            } else {
                '-'
            }
        }
        VK_OEM_PLUS => {
            if shifted {
                '+'
            } else {
                '='
            }
        }
        VK_OEM_COMMA => {
            if shifted {
                '<'
            } else {
                ','
            }
        }
        VK_OEM_PERIOD => {
            if shifted {
                '>'
            } else {
                '.'
            }
        }
        VK_OEM_1 => {
            if shifted {
                ':'
            } else {
                ';'
            }
        }
        VK_OEM_2 => {
            if shifted {
                '?'
            } else {
                '/'
            }
        }
        VK_OEM_3 => {
            if shifted {
                '~'
            } else {
                '`'
            }
        }
        VK_OEM_4 => {
            if shifted {
                '{'
            } else {
                '['
            }
        }
        VK_OEM_5 | VK_OEM_102 => {
            if shifted {
                '|'
            } else {
                '\\'
            }
        }
        VK_OEM_6 => {
            if shifted {
                '}'
            } else {
                ']'
            }
        }
        VK_OEM_7 => {
            if shifted {
                '"'
            } else {
                '\''
            }
        }
        _ => return None,
    };
    Some(value as u32)
}

fn is_printable_vk(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;

    matches!(
        vk,
        VK_0 | VK_1
            | VK_2
            | VK_3
            | VK_4
            | VK_5
            | VK_6
            | VK_7
            | VK_8
            | VK_9
            | VK_OEM_MINUS
            | VK_OEM_PLUS
            | VK_OEM_COMMA
            | VK_OEM_PERIOD
            | VK_OEM_1
            | VK_OEM_2
            | VK_OEM_3
            | VK_OEM_4
            | VK_OEM_5
            | VK_OEM_6
            | VK_OEM_7
            | VK_OEM_102
    )
}

pub fn shift_keysym_for_vk(vk: u16) -> Option<u32> {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;

    match VIRTUAL_KEY(vk) {
        VK_SHIFT | VK_LSHIFT => Some(0xffe1),
        VK_RSHIFT => Some(0xffe2),
        _ => None,
    }
}

pub fn is_shift_vk(vk: u16) -> bool {
    shift_keysym_for_vk(vk).is_some()
}

/// Solo-Shift tracking: Shift toggles Chinese/English only when no other key
/// was pressed while it was held.
///
/// This has to be fed from `OnTestKeyDown`, which TSF calls for every key —
/// `OnKeyDown` only runs for keys the test callback claimed, so a flag kept
/// there alone would never be cleared by a passed-through key and, worse,
/// would never be set at all because Shift itself is never claimed.
pub fn shift_pending_after_key_down(vk: u16) -> bool {
    is_shift_vk(vk)
}

pub fn is_enter_vk(vk: u16) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;

    matches!(VIRTUAL_KEY(vk), VK_RETURN)
}

pub fn should_bypass_empty_composition(vk: u16, mods: u32, state: &keytao_core::ImeState) -> bool {
    key_policy::should_bypass_empty_composition_key(is_nonstarter_vk(vk), mods, state)
}

/// Returns true for keys the IME may handle, i.e. the keys `OnTestKeyDown`
/// claims so that TSF calls `OnKeyDown` for them.
///
/// TSF never calls `OnKeyDown` for a key the test callback declined, so this is
/// deliberately an over-approximation: every key that can reach librime is
/// claimed here and released again in `OnKeyDown` when librime does not accept
/// it (`should_consume_processed_state`). The one rule both callbacks must
/// agree on is which keys never reach librime at all — that rule lives in
/// `key_policy::should_bypass_empty_composition_key` and is mirrored below.
pub fn should_eat_key(vk: u16, has_composition: bool, mods: u32) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;

    // Windows-key chords belong to the shell.
    if key_policy::is_system_reserved_modifier(mods) {
        return false;
    }

    if is_shift_vk(vk) {
        return false;
    }

    let key = VIRTUAL_KEY(vk);
    if key == VK_PACKET {
        return packet_keysym()
            .map(|sym| has_composition || is_touch_keyboard_page_key(sym))
            .unwrap_or(false);
    }

    if key == VK_F4 {
        // Alt+F4 closes the host window and is never a schema switcher hotkey.
        return mods & RIME_MOD_ALT == 0;
    }

    if has_composition {
        return is_nonstarter_vk(vk) || produces_text_vk(key);
    }

    // Without a composition the navigation and editing keys belong to the
    // application; everything printable — letters, digits and OEM punctuation —
    // is offered to librime so that the punctuator can emit full-width forms.
    !is_nonstarter_vk(vk) && produces_text_vk(key)
}

fn packet_keysym() -> Option<u32> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardState, ToUnicode, VK_PACKET};

    let mut state = [0u8; 256];
    unsafe {
        GetKeyboardState(&mut state).ok()?;
        let mut buf = [0u16; 2];
        if ToUnicode(VK_PACKET.0 as u32, 0, Some(&state), &mut buf, 0) != 1 {
            return None;
        }
        let raw = buf[0] as u32;
        Some(match raw {
            TOUCH_KEYBOARD_NEXT_PAGE => XK_PAGE_DOWN,
            TOUCH_KEYBOARD_PREVIOUS_PAGE => XK_PAGE_UP,
            _ => raw,
        })
    }
}

fn is_touch_keyboard_page_key(sym: u32) -> bool {
    matches!(sym, XK_PAGE_UP | XK_PAGE_DOWN)
}

fn is_letter_vk(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_A, VK_Z};

    (VK_A.0..=VK_Z.0).contains(&vk.0)
}

/// Every key `vk_to_keysym` turns into a printable character.
fn produces_text_vk(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool {
    is_letter_vk(vk) || is_printable_vk(vk) || is_numpad_text_vk(vk)
}

fn is_numpad_text_vk(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;

    (VK_NUMPAD0.0..=VK_NUMPAD9.0).contains(&vk.0)
        || matches!(
            vk,
            VK_MULTIPLY | VK_ADD | VK_SUBTRACT | VK_DECIMAL | VK_DIVIDE
        )
}

fn is_nonstarter_vk(vk: u16) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;

    matches!(
        VIRTUAL_KEY(vk),
        VK_SPACE
            | VK_BACK
            | VK_TAB
            | VK_DELETE
            | VK_ESCAPE
            | VK_RETURN
            | VK_LEFT
            | VK_RIGHT
            | VK_UP
            | VK_DOWN
            | VK_HOME
            | VK_END
            | VK_PRIOR
            | VK_NEXT
    )
}

#[cfg(test)]
mod tests {
    use keytao_core::{
        key_policy, Candidate, ImeState, RIME_MOD_ALT, RIME_MOD_CONTROL, RIME_MOD_LOCK,
        RIME_MOD_SHIFT, RIME_MOD_SUPER,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        VK_1, VK_A, VK_BACK, VK_F4, VK_OEM_1, VK_OEM_COMMA, VK_RETURN, VK_SHIFT, VK_SPACE,
    };

    use super::{
        fallback_printable_keysym, letters_are_uppercase, shift_pending_after_key_down,
        should_bypass_empty_composition, should_eat_key, unicode_to_keysym, vk_to_keysym,
    };

    fn composing_state() -> ImeState {
        let mut state = ImeState::empty();
        state.preedit = "ni".into();
        state.candidates.push(Candidate {
            text: "你".into(),
            comment: None,
        });
        state
    }

    #[test]
    fn fallback_printable_mapping_respects_shift() {
        assert_eq!(fallback_printable_keysym(VK_1, 0), Some('1' as u32));
        assert_eq!(
            fallback_printable_keysym(VK_1, RIME_MOD_SHIFT),
            Some('!' as u32)
        );
        assert_eq!(
            fallback_printable_keysym(VK_OEM_1, RIME_MOD_SHIFT),
            Some(':' as u32)
        );
    }

    #[test]
    fn unicode_keysym_uses_x11_unicode_encoding() {
        assert_eq!(unicode_to_keysym('a'), 0x61);
        assert_eq!(unicode_to_keysym('键'), 0x0100_0000 | '键' as u32);
    }

    #[test]
    fn caps_lock_and_shift_cancel_out_for_letters() {
        assert!(!letters_are_uppercase(0));
        assert!(letters_are_uppercase(RIME_MOD_SHIFT));
        assert!(letters_are_uppercase(RIME_MOD_LOCK));
        assert!(!letters_are_uppercase(RIME_MOD_LOCK | RIME_MOD_SHIFT));

        assert_eq!(vk_to_keysym(VK_A.0, 0, RIME_MOD_LOCK), Some('A' as u32));
        assert_eq!(
            vk_to_keysym(VK_A.0, 0, RIME_MOD_LOCK | RIME_MOD_SHIFT),
            Some('a' as u32)
        );
    }

    #[test]
    fn punctuation_reaches_rime_without_a_composition() {
        // Chinese punctuation only works if the punctuator sees the key, so the
        // test callback has to claim it (cross-3 / windows-2).
        assert!(should_eat_key(VK_OEM_COMMA.0, false, 0));
        assert!(should_eat_key(VK_1.0, false, 0));
        assert!(should_eat_key(VK_A.0, false, 0));
    }

    #[test]
    fn editing_keys_are_left_to_the_application_without_a_composition() {
        let empty = ImeState::empty();
        for vk in [VK_BACK, VK_RETURN, VK_SPACE] {
            assert!(!should_eat_key(vk.0, false, 0));
            assert!(should_bypass_empty_composition(vk.0, 0, &empty));
        }
        // With a composition the same keys belong to librime.
        let composing = composing_state();
        for vk in [VK_BACK, VK_RETURN, VK_SPACE] {
            assert!(should_eat_key(vk.0, true, 0));
            assert!(!should_bypass_empty_composition(vk.0, 0, &composing));
        }
    }

    #[test]
    fn control_and_alt_chords_are_offered_to_rime() {
        // Rime binds Control+grave and the emacs editing keys; OnKeyDown hands
        // them back to the host when librime declines (D6 / core-8).
        assert!(should_eat_key(VK_A.0, false, RIME_MOD_CONTROL));
        assert!(should_eat_key(VK_A.0, true, RIME_MOD_ALT));
        assert!(!should_bypass_empty_composition(
            VK_A.0,
            RIME_MOD_CONTROL,
            &ImeState::empty()
        ));
    }

    #[test]
    fn solo_shift_survives_keys_the_test_callback_declines() {
        // Shift arms the toggle; any other key down disarms it, including keys
        // that are passed through to the host and never reach OnKeyDown.
        assert!(shift_pending_after_key_down(VK_SHIFT.0));
        assert!(!shift_pending_after_key_down(VK_A.0));
        assert!(!shift_pending_after_key_down(VK_BACK.0));
        assert!(!should_eat_key(VK_BACK.0, false, 0));
    }

    #[test]
    fn shell_and_window_chords_are_never_claimed() {
        assert!(!should_eat_key(VK_A.0, true, RIME_MOD_SUPER));
        assert!(!should_eat_key(VK_F4.0, false, RIME_MOD_ALT));
        assert!(should_eat_key(VK_F4.0, false, 0));
        assert!(!should_eat_key(VK_SHIFT.0, true, 0));
        assert!(key_policy::is_system_reserved_modifier(RIME_MOD_SUPER));
    }
}
