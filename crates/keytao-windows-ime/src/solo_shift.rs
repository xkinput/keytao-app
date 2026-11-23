use std::time::Instant;

pub(crate) fn solo_shift_keysym(vk: u16, lparam: isize) -> u32 {
    if vk == 0xa1 || (lparam >> 16) & 0xff == 0x36 {
        0xffe2
    } else {
        0xffe1
    }
}

pub(crate) fn record_modifier_tap(
    previous: Option<(u32, Instant)>,
    keysym: u32,
    lparam: isize,
    now: Instant,
) -> Option<(u32, Instant)> {
    if previous.is_some_and(|(key, _)| key != keysym) {
        None
    } else if lparam & (1 << 30) == 0 {
        Some((keysym, now))
    } else {
        previous
    }
}

pub(crate) fn modifier_tap_matches(
    press: Option<(u32, Instant)>,
    keysym: u32,
    now: Instant,
) -> bool {
    press.is_some_and(|(key, at)| {
        key == keysym && now.saturating_duration_since(at).as_millis() < 500
    })
}

pub(crate) fn modifier_tap_should_apply(accepted: bool, before_ascii: bool, after_ascii: bool) -> bool {
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
    fn modifier_tap_requires_the_same_side_within_librimes_half_second_window() {
        let at = Instant::now();
        let press = Some((0xffe3, at));
        let after = |ms| at + std::time::Duration::from_millis(ms);
        assert!(modifier_tap_matches(press, 0xffe3, after(499)));
        assert!(!modifier_tap_matches(press, 0xffe4, after(499)));
        assert!(!modifier_tap_matches(press, 0xffe3, after(500)));
        assert_eq!(record_modifier_tap(press, 0xffe4, 0, after(1)), None);
    }

    #[test]
    fn modifier_tap_applies_only_librimes_acceptance_or_mode_change() {
        for before in [false, true] {
            assert!(modifier_tap_should_apply(true, before, before));
            assert!(modifier_tap_should_apply(false, before, !before));
            assert!(!modifier_tap_should_apply(false, before, before));
        }
    }

    #[test]
    fn modifier_repeats_preserve_time_and_a_new_press_restarts_it() {
        let first = Instant::now();
        let later = first + std::time::Duration::from_millis(499);
        let press = Some((0xffe1, first));
        assert_eq!(record_modifier_tap(press, 0xffe1, 1 << 30, later), press);
        assert_eq!(
            record_modifier_tap(press, 0xffe1, 0, later),
            Some((0xffe1, later))
        );
        assert_eq!(record_modifier_tap(None, 0xffe1, 1 << 30, later), None);
    }
}
