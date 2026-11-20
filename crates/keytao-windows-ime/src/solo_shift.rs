use std::time::Instant;

pub(crate) fn solo_shift_keysym(vk: u16, lparam: isize) -> u32 {
    if vk == 0xa1 || (lparam >> 16) & 0xff == 0x36 {
        0xffe2
    } else {
        0xffe1
    }
}

pub(crate) fn solo_shift_should_send(elapsed_ms: u128) -> bool {
    elapsed_ms < 500
}

pub(crate) fn solo_shift_press_time(
    previous: Option<Instant>,
    lparam: isize,
    now: Instant,
) -> Option<Instant> {
    if lparam & (1 << 30) == 0 {
        Some(now)
    } else {
        previous
    }
}

pub(crate) fn solo_shift_should_apply(
    accepted: bool,
    before_ascii: bool,
    after_ascii: bool,
) -> bool {
    accepted || before_ascii != after_ascii
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solo_shift_distinguishes_sides_and_masks_extended_key_flags() {
        assert_eq!(solo_shift_keysym(0x10, 0x2a << 16), 0xffe1);
        assert_eq!(solo_shift_keysym(0x10, 0x36 << 16), 0xffe2);
        assert_eq!(solo_shift_keysym(0xa1, 0), 0xffe2);
        assert_eq!(solo_shift_keysym(0x10, (0x36 << 16) | (1 << 24)), 0xffe2);
        assert_eq!(solo_shift_keysym(0x10, (0x2a << 16) | (1 << 24)), 0xffe1);
    }

    #[test]
    fn solo_shift_uses_librimes_half_second_window() {
        assert!(solo_shift_should_send(499));
        assert!(!solo_shift_should_send(500));
        assert!(!solo_shift_should_send(501));
    }

    #[test]
    fn solo_shift_applies_only_librimes_acceptance_or_mode_change() {
        for before in [false, true] {
            assert!(solo_shift_should_apply(true, before, before));
            assert!(solo_shift_should_apply(false, before, !before));
            assert!(!solo_shift_should_apply(false, before, before));
        }
    }

    #[test]
    fn solo_shift_repeats_preserve_time_and_a_new_press_restarts_it() {
        let first = Instant::now();
        let later = first + std::time::Duration::from_millis(499);
        assert_eq!(
            solo_shift_press_time(Some(first), 1 << 30, later),
            Some(first)
        );
        assert_eq!(solo_shift_press_time(Some(first), 0, later), Some(later));
        assert_eq!(solo_shift_press_time(None, 1 << 30, later), None);
    }
}
