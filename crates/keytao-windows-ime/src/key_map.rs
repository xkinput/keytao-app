//! Windows Virtual Key → X11 keysym + modifier mask conversion.
//!
//! librime uses X11 keysyms internally (same convention as the Linux backends).
//! The Wayland backend converts hardware keycodes via xkbcommon; here we convert
//! Windows VK codes directly.

pub use keytao_core::{
    key_policy, RIME_MOD_ALT, RIME_MOD_CONTROL, RIME_MOD_SHIFT, RIME_RELEASE_MASK,
};

/// Read the current state of Shift, Control, Alt and return an X11 modifier mask.
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
    }
    mask
}

/// Convert a Windows Virtual Key code to an X11 keysym.
///
/// Returns `None` for keys the IME should never eat (e.g. media keys).
pub fn vk_to_keysym(vk: u16, mods: u32) -> Option<u32> {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;

    let sym = match VIRTUAL_KEY(vk) {
        // --- Printable ASCII letters ---
        // Match Linux/macOS semantics: the keysym reflects the produced
        // printable character while the modifier mask still carries Shift.
        key if is_letter_vk(key) => {
            let base = if mods & RIME_MOD_SHIFT != 0 {
                b'A'
            } else {
                b'a'
            };
            base as u32 + (key.0 - VK_A.0) as u32
        }

        // --- Digits (unshifted) ---
        VK_0 => 0x0030,
        VK_1 => 0x0031,
        VK_2 => 0x0032,
        VK_3 => 0x0033,
        VK_4 => 0x0034,
        VK_5 => 0x0035,
        VK_6 => 0x0036,
        VK_7 => 0x0037,
        VK_8 => 0x0038,
        VK_9 => 0x0039,

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

        // --- Control keys ---
        VK_BACK => 0xFF08,   // XK_BackSpace
        VK_TAB => 0xFF09,    // XK_Tab
        VK_RETURN => 0xFF0D, // XK_Return
        VK_ESCAPE => 0xFF1B, // XK_Escape
        VK_SPACE => 0x0020,  // XK_space
        VK_DELETE => 0xFFFF, // XK_Delete

        // --- Navigation ---
        VK_LEFT => 0xFF51,  // XK_Left
        VK_UP => 0xFF52,    // XK_Up
        VK_RIGHT => 0xFF53, // XK_Right
        VK_DOWN => 0xFF54,  // XK_Down
        VK_HOME => 0xFF50,  // XK_Home
        VK_END => 0xFF57,   // XK_End
        VK_PRIOR => 0xFF55, // XK_Prior (Page Up)
        VK_NEXT => 0xFF56,  // XK_Next  (Page Down)
        VK_F4 => 0xFFC1,    // XK_F4 opens the Rime menu

        // --- OEM punctuation (US layout) ---
        VK_OEM_MINUS => 0x002D,  // '-'
        VK_OEM_PLUS => 0x003D,   // '='
        VK_OEM_COMMA => 0x002C,  // ','
        VK_OEM_PERIOD => 0x002E, // '.'
        VK_OEM_1 => 0x003B,      // ';'
        VK_OEM_2 => 0x002F,      // '/'
        VK_OEM_3 => 0x0060,      // '`'
        VK_OEM_4 => 0x005B,      // '['
        VK_OEM_5 => 0x005C,      // '\'
        VK_OEM_6 => 0x005D,      // ']'
        VK_OEM_7 => 0x0027,      // '\''

        // Anything else (other function keys, media keys, etc.) — don't intercept
        _ => return None,
    };
    Some(sym)
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

pub fn is_enter_vk(vk: u16) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;

    matches!(VIRTUAL_KEY(vk), VK_RETURN)
}

pub fn candidate_index_for_select_key(vk: u16, state: &keytao_core::ImeState) -> Option<usize> {
    if is_space_vk(vk) {
        return key_policy::highlighted_candidate_index(state);
    }

    let ch = ascii_char_for_vk(vk)?;
    key_policy::candidate_index_for_char(ch, state)
}

pub fn should_bypass_empty_composition(vk: u16, mods: u32, state: &keytao_core::ImeState) -> bool {
    key_policy::should_bypass_empty_composition_key(is_nonstarter_vk(vk), mods, state)
}

/// Returns true for keys the IME should intercept.
/// Used in OnTestKeyDown to tell TSF we own this keystroke.
pub fn should_eat_key(vk: u16, has_composition: bool, mods: u32) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;

    // Never eat keys with Ctrl/Alt (let application handle shortcuts)
    if mods & (RIME_MOD_CONTROL | RIME_MOD_ALT) != 0 {
        return false;
    }

    if is_shift_vk(vk) {
        return false;
    }

    let vk = VIRTUAL_KEY(vk);
    if vk == VK_F4 {
        return true;
    }

    if has_composition {
        is_nonstarter_vk(vk.0) || is_letter_vk(vk) || ascii_char_for_vk(vk.0).is_some()
    } else {
        is_letter_vk(vk)
    }
}

fn is_letter_vk(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_A, VK_Z};

    (VK_A.0..=VK_Z.0).contains(&vk.0)
}

fn is_space_vk(vk: u16) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VIRTUAL_KEY, VK_SPACE};

    VIRTUAL_KEY(vk) == VK_SPACE
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

fn ascii_char_for_vk(vk: u16) -> Option<char> {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;

    let ch = match VIRTUAL_KEY(vk) {
        VK_0 | VK_NUMPAD0 => '0',
        VK_1 | VK_NUMPAD1 => '1',
        VK_2 | VK_NUMPAD2 => '2',
        VK_3 | VK_NUMPAD3 => '3',
        VK_4 | VK_NUMPAD4 => '4',
        VK_5 | VK_NUMPAD5 => '5',
        VK_6 | VK_NUMPAD6 => '6',
        VK_7 | VK_NUMPAD7 => '7',
        VK_8 | VK_NUMPAD8 => '8',
        VK_9 | VK_NUMPAD9 => '9',
        VK_OEM_MINUS => '-',
        VK_OEM_PLUS => '=',
        VK_OEM_COMMA => ',',
        VK_OEM_PERIOD => '.',
        VK_OEM_1 => ';',
        VK_OEM_2 => '/',
        VK_OEM_3 => '`',
        VK_OEM_4 => '[',
        VK_OEM_5 => '\\',
        VK_OEM_6 => ']',
        VK_OEM_7 => '\'',
        _ => return None,
    };
    Some(ch)
}
