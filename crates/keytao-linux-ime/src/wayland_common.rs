//! Helpers shared by the two Wayland input-method protocols.

use std::{
    fs::File,
    os::fd::FromRawFd,
    time::{Duration, Instant},
};

use xkbcommon::xkb;

/// Wayland preedit indices are byte offsets; `ImeState::cursor` counts scalars.
pub fn preedit_cursor_bytes(preedit: &str, cursor: usize) -> i32 {
    preedit
        .char_indices()
        .nth(cursor)
        .map(|(offset, _)| offset)
        .unwrap_or(preedit.len()) as i32
}

/// A zero rate disables repeating; negative protocol values are invalid.
pub fn repeat_timings(rate: i32, delay: i32) -> Option<(Duration, Duration)> {
    if rate <= 0 || delay < 0 {
        return None;
    }
    Some((
        Duration::from_millis(delay as u64),
        Duration::from_micros(1_000_000 / rate as u64),
    ))
}

struct RepeatingKey {
    key: u32,
    next: Instant,
    interval: Duration,
}

/// Keys consumed by the IME must repeat here rather than in the client.
#[derive(Default)]
pub struct KeyRepeat {
    timings: Option<(Duration, Duration)>,
    current: Option<RepeatingKey>,
}

impl KeyRepeat {
    pub fn set_info(&mut self, rate: i32, delay: i32) {
        self.timings = repeat_timings(rate, delay);
        if self.timings.is_none() {
            self.cancel();
        }
    }

    pub fn arm(&mut self, key: u32, repeats: bool) {
        self.cancel();
        if let Some((delay, interval)) = self.timings.filter(|_| repeats) {
            self.current = Some(RepeatingKey {
                key,
                next: Instant::now() + delay,
                interval,
            });
        }
    }

    pub fn cancel(&mut self) {
        self.current = None;
    }

    pub fn cancel_for(&mut self, key: u32) {
        if self.key() == Some(key) {
            self.cancel();
        }
    }

    pub fn key(&self) -> Option<u32> {
        self.current.as_ref().map(|repeat| repeat.key)
    }

    pub fn due(&self) -> bool {
        self.next_deadline()
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.current.as_ref().map(|repeat| repeat.next)
    }

    pub fn advance(&mut self, key: u32) {
        // The key handler re-arms the initial delay; subsequent presses use
        // the interval unless that handler cancelled or replaced the key.
        if let Some(repeat) = self.current.as_mut().filter(|repeat| repeat.key == key) {
            repeat.next = Instant::now() + repeat.interval;
        }
    }
}

pub fn key_sym(state: Option<&xkb::State>, evdev_keycode: u32) -> u32 {
    state
        .map(|state| state.key_get_one_sym(xkb::Keycode::from(evdev_keycode + 8)))
        .unwrap_or(xkb::Keysym::from(xkb::keysyms::KEY_NoSymbol))
        .into()
}

/// Modifiers and keys marked `repeat=no` must not repeat.
pub fn key_repeats(keymap: Option<&xkb::Keymap>, evdev_keycode: u32) -> bool {
    keymap.is_some_and(|keymap| keymap.key_repeats(xkb::Keycode::from(evdev_keycode + 8)))
}

pub fn mode_hint_deadline(duration: Option<Duration>) -> Instant {
    Instant::now() + duration.unwrap_or_else(|| Duration::from_millis(750))
}

pub fn deadline_active(deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|deadline| Instant::now() < deadline)
}

pub fn take_expired_deadline(deadline: &mut Option<Instant>) -> bool {
    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        *deadline = None;
        true
    } else {
        false
    }
}

/// Milliseconds until the next timer fires, or -1 when nothing is pending.
pub fn poll_timeout_ms(deadlines: impl IntoIterator<Item = Option<Instant>>) -> i32 {
    let Some(next) = deadlines.into_iter().flatten().min() else {
        return -1;
    };
    let now = Instant::now();
    if next <= now {
        return 0;
    }
    (next - now).as_millis().min(i32::MAX as u128) as i32
}

pub fn tempfile(flags: libc::c_uint) -> std::io::Result<File> {
    let fd = unsafe { libc::memfd_create(c"keytao-shm".as_ptr(), flags) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(test)]
mod tests {
    use super::{preedit_cursor_bytes, repeat_timings};
    use std::time::Duration;

    #[test]
    fn repeat_info_maps_to_a_delay_and_an_interval() {
        assert_eq!(
            repeat_timings(25, 600),
            Some((Duration::from_millis(600), Duration::from_millis(40)))
        );
        // A rate of zero disables repeating, as does an illegal negative one.
        assert_eq!(repeat_timings(0, 600), None);
        assert_eq!(repeat_timings(-1, 600), None);
        assert_eq!(repeat_timings(25, -1), None);
    }

    #[test]
    fn preedit_cursor_is_a_byte_offset() {
        assert_eq!(preedit_cursor_bytes("nihao", 2), 2);
        // Unicode scalar 2 of "你好ni" starts at byte 6.
        assert_eq!(preedit_cursor_bytes("你好ni", 2), 6);
        // Past the end clamps to the byte length rather than panicking.
        assert_eq!(preedit_cursor_bytes("你好", 9), 6);
        assert_eq!(preedit_cursor_bytes("", 0), 0);
    }
}
