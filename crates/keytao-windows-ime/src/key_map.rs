//! Windows Virtual Key → X11 keysym + modifier mask conversion.
//!
//! librime uses X11 keysyms internally (same convention as the Linux backends).
//! The Wayland backend converts hardware keycodes via xkbcommon; here we convert
//! Windows VK codes directly.

// X11 modifier bitmask — matches what Linux wayland_backend passes to librime.
pub const MOD_SHIFT: u32 = 0x0001;
pub const MOD_CONTROL: u32 = 0x0004;
pub const MOD_ALT: u32 = 0x0008; // Mod1 / Alt

/// Read the current state of Shift, Control, Alt and return an X11 modifier mask.
pub fn current_mod_mask() -> u32 {
    let mut mask = 0u32;
    unsafe {
        use windows::Win32::UI::Input::KeyboardAndMouse::*;
        if GetKeyState(VK_SHIFT.0 as i32) as u16 & 0x8000 != 0 {
            mask |= MOD_SHIFT;
        }
        if GetKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000 != 0 {
            mask |= MOD_CONTROL;
        }
        if GetKeyState(VK_MENU.0 as i32) as u16 & 0x8000 != 0 {
            mask |= MOD_ALT;
        }
    }
    mask
}

/// Convert a Windows Virtual Key code to an X11 keysym.
///
/// Returns `None` for keys the IME should never eat (e.g. function keys, media keys).
pub fn vk_to_keysym(vk: u16) -> Option<u32> {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;

    let sym = match VIRTUAL_KEY(vk) {
        // --- Printable ASCII letters: always pass lowercase; shift is in modifier mask ---
        VK_A => 0x0061, // 'a'
        VK_B => 0x0062,
        VK_C => 0x0063,
        VK_D => 0x0064,
        VK_E => 0x0065,
        VK_F => 0x0066,
        VK_G => 0x0067,
        VK_H => 0x0068,
        VK_I => 0x0069,
        VK_J => 0x006A,
        VK_K => 0x006B,
        VK_L => 0x006C,
        VK_M => 0x006D,
        VK_N => 0x006E,
        VK_O => 0x006F,
        VK_P => 0x0070,
        VK_Q => 0x0071,
        VK_R => 0x0072,
        VK_S => 0x0073,
        VK_T => 0x0074,
        VK_U => 0x0075,
        VK_V => 0x0076,
        VK_W => 0x0077,
        VK_X => 0x0078,
        VK_Y => 0x0079,
        VK_Z => 0x007A,

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
        VK_BACK   => 0xFF08, // XK_BackSpace
        VK_TAB    => 0xFF09, // XK_Tab
        VK_RETURN => 0xFF0D, // XK_Return
        VK_ESCAPE => 0xFF1B, // XK_Escape
        VK_SPACE  => 0x0020, // XK_space
        VK_DELETE => 0xFFFF, // XK_Delete

        // --- Navigation ---
        VK_LEFT  => 0xFF51, // XK_Left
        VK_UP    => 0xFF52, // XK_Up
        VK_RIGHT => 0xFF53, // XK_Right
        VK_DOWN  => 0xFF54, // XK_Down
        VK_HOME  => 0xFF50, // XK_Home
        VK_END   => 0xFF57, // XK_End
        VK_PRIOR => 0xFF55, // XK_Prior (Page Up)
        VK_NEXT  => 0xFF56, // XK_Next  (Page Down)

        // --- OEM punctuation (US layout) ---
        VK_OEM_MINUS  => 0x002D, // '-'
        VK_OEM_PLUS   => 0x003D, // '='
        VK_OEM_COMMA  => 0x002C, // ','
        VK_OEM_PERIOD => 0x002E, // '.'
        VK_OEM_1      => 0x003B, // ';'
        VK_OEM_2      => 0x002F, // '/'
        VK_OEM_3      => 0x0060, // '`'
        VK_OEM_4      => 0x005B, // '['
        VK_OEM_5      => 0x005C, // '\'
        VK_OEM_6      => 0x005D, // ']'
        VK_OEM_7      => 0x0027, // '\''

        // Anything else (function keys, media keys, etc.) — don't intercept
        _ => return None,
    };
    Some(sym)
}

/// Returns true for keys the IME should intercept regardless of preedit state.
/// Used in OnTestKeyDown to tell TSF we own this keystroke.
pub fn should_eat_key(vk: u16, has_preedit: bool, mods: u32) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;

    // Never eat keys with Ctrl/Alt (let application handle shortcuts)
    if mods & (MOD_CONTROL | MOD_ALT) != 0 {
        return false;
    }

    let vk = VIRTUAL_KEY(vk);

    if has_preedit {
        // With active preedit: eat navigation + selection keys too
        matches!(
            vk,
            VK_BACK | VK_ESCAPE | VK_RETURN | VK_SPACE
            | VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN
            | VK_PRIOR | VK_NEXT
            | VK_0 | VK_1 | VK_2 | VK_3 | VK_4
            | VK_5 | VK_6 | VK_7 | VK_8 | VK_9
            | VK_OEM_MINUS | VK_OEM_PLUS | VK_OEM_COMMA | VK_OEM_PERIOD
        ) || is_letter_vk(vk)
    } else {
        // Without preedit: only eat unshifted/shifted letters that start composition
        is_letter_vk(vk)
    }
}

fn is_letter_vk(vk: VIRTUAL_KEY) -> bool {
    (VK_A.0..=VK_Z.0).contains(&vk.0)
}
