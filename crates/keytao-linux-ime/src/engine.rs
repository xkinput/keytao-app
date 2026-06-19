//! Linux-facing aliases for the shared IME runtime in keytao-core.
//!
//! Linux protocol backends should not manage librime sessions directly.  They
//! create sessions through `CoreEngine` and send normalized X11 keysym events.

pub use keytao_core::{
    rime_modifier_mask, ImeRuntime as CoreEngine, ImeRuntimeSession as ImeSession,
};

#[cfg(test)]
mod tests {
    use super::rime_modifier_mask;

    #[test]
    fn rime_modifier_mask_strips_lock_and_pointer_modifiers() {
        assert_eq!(rime_modifier_mask(0x10), 0);
        assert_eq!(rime_modifier_mask(0x10 | 0x0001 | 0x0004), 0x0001 | 0x0004);
        assert_eq!(rime_modifier_mask((1 << 30) | 0x10), 1 << 30);
    }
}
