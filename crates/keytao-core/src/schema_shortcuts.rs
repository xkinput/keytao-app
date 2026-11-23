use crate::{
    ImeState, RIME_MOD_ALT, RIME_MOD_CONTROL, RIME_MOD_LOCK, RIME_MOD_SHIFT, key_policy,
    rime_modifier_mask,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SchemaShortcut {
    pub key: u32,
    pub modifiers: u32,
    condition: Condition,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
enum Condition {
    Always,
    Switcher,
    Composing,
    HasMenu,
}

impl SchemaShortcut {
    pub(crate) fn parse(accept: &str, when: &str) -> Option<Self> {
        let condition = match when {
            "always" => Condition::Always,
            "composing" => Condition::Composing,
            "has_menu" => Condition::HasMenu,
            // The snapshot has no segment tags for paging or predicting.
            _ => return None,
        };
        let mut keys = accept.rsplit('+');
        let key = parse_keysym(keys.next()?)?;
        let mut modifiers = 0;
        for name in keys {
            modifiers |= match name {
                "Control" => RIME_MOD_CONTROL,
                "Shift" => RIME_MOD_SHIFT,
                "Alt" | "Mod1" => RIME_MOD_ALT,
                "Lock" => RIME_MOD_LOCK,
                _ => return None,
            };
        }
        Some(Self {
            key,
            modifiers,
            condition,
        })
    }

    pub fn applies(self, state: &ImeState) -> bool {
        match self.condition {
            Condition::Always | Condition::Switcher => true,
            Condition::Composing => !state.preedit.is_empty(),
            Condition::HasMenu => !state.ascii_mode && !state.candidates.is_empty(),
        }
    }

    pub(crate) fn parse_switcher(accept: &str) -> Option<Self> {
        let mut shortcut = Self::parse(accept, "always")?;
        shortcut.condition = Condition::Switcher;
        Some(shortcut)
    }

    pub fn is_switcher(self) -> bool {
        matches!(self.condition, Condition::Switcher)
    }

    pub(crate) fn matches_switcher(self, key: u32, mask: u32) -> bool {
        self.is_switcher() && self.matches_key(key, mask)
    }

    pub(crate) fn matches_key(self, key: u32, mask: u32) -> bool {
        self.modifiers == (rime_modifier_mask(mask) | (mask & self.modifiers & RIME_MOD_LOCK))
            && (self.key == key
                || key_policy::normalize_key_for_modifiers(self.key, self.modifiers).0 == key
                // Shift+grave can arrive as either the physical key or its shifted symbol.
                || (self.key == 0x60 && key == 0x7e && self.modifiers & RIME_MOD_SHIFT != 0))
    }
}

fn parse_keysym(name: &str) -> Option<u32> {
    if name.len() == 1 && name.as_bytes()[0].is_ascii_graphic() {
        return Some(name.as_bytes()[0].into());
    }
    if let Some(hex) = name.strip_prefix("0x") {
        return u32::from_str_radix(hex, 16).ok();
    }
    if let Some(number) = name.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
        return (1..=35).contains(&number).then(|| 0xffbd + number);
    }
    Some(match name {
        "space" => 0x20,
        "exclam" => 0x21,
        "quotedbl" => 0x22,
        "numbersign" => 0x23,
        "dollar" => 0x24,
        "percent" => 0x25,
        "ampersand" => 0x26,
        "apostrophe" | "quoteright" => 0x27,
        "parenleft" => 0x28,
        "parenright" => 0x29,
        "asterisk" => 0x2a,
        "plus" => 0x2b,
        "comma" => 0x2c,
        "minus" => 0x2d,
        "period" => 0x2e,
        "slash" => 0x2f,
        "colon" => 0x3a,
        "semicolon" => 0x3b,
        "less" => 0x3c,
        "equal" => 0x3d,
        "greater" => 0x3e,
        "question" => 0x3f,
        "at" => 0x40,
        "bracketleft" => 0x5b,
        "backslash" => 0x5c,
        "bracketright" => 0x5d,
        "asciicircum" => 0x5e,
        "underscore" => 0x5f,
        "grave" | "quoteleft" => 0x60,
        "braceleft" => 0x7b,
        "bar" => 0x7c,
        "braceright" => 0x7d,
        "asciitilde" => 0x7e,
        "BackSpace" => 0xff08,
        "Tab" => 0xff09,
        "Return" => 0xff0d,
        "Escape" => 0xff1b,
        "Home" => 0xff50,
        "Left" => 0xff51,
        "Up" => 0xff52,
        "Right" => 0xff53,
        "Down" => 0xff54,
        "Prior" | "Page_Up" => 0xff55,
        "Next" | "Page_Down" => 0xff56,
        "End" => 0xff57,
        "Insert" => 0xff63,
        "Delete" => 0xffff,
        _ => return None,
    })
}
