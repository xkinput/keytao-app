use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

const KEY_LIMIT: usize = 64;
const RECOVERY_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug)]
pub(crate) enum KeyEventSource {
    OnKeyDown,
    OnKeyUp,
    DeferredKey,
}

impl KeyEventSource {
    pub(crate) fn async_dontcare(self) -> bool {
        matches!(self, Self::DeferredKey)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DeferredKey {
    pub keysym: u32,
    pub mods: u32,
    pub enter: bool,
    pub modifier_tap: bool,
}

#[derive(Default)]
pub(crate) struct DeferredKeys {
    keys: VecDeque<DeferredKey>,
    started: Option<Instant>,
    passthrough: bool,
}

impl DeferredKeys {
    pub fn can_queue(&mut self, now: Instant) -> bool {
        self.expire(now);
        if self.keys.len() >= KEY_LIMIT {
            self.keys.clear();
            self.passthrough = true;
        }
        !self.passthrough
    }

    pub fn expire(&mut self, now: Instant) {
        if self
            .started
            .is_some_and(|started| now.saturating_duration_since(started) >= RECOVERY_TIMEOUT)
        {
            self.keys.clear();
            self.passthrough = true;
        }
    }

    pub fn defer_failed(&mut self, key: DeferredKey, already_written: bool, now: Instant) -> bool {
        // A written timeout may have committed remotely; never send that key twice.
        if already_written {
            return true;
        }
        if !self.can_queue(now) {
            return false;
        }
        self.started.get_or_insert(now);
        self.keys.push_back(key);
        true
    }

    pub fn front(&self) -> Option<DeferredKey> {
        self.keys.front().copied()
    }

    pub fn pop(&mut self) {
        self.keys.pop_front();
        if self.keys.is_empty() {
            self.reset();
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(ch: char) -> DeferredKey {
        DeferredKey {
            keysym: ch as u32,
            mods: 0,
            enter: false,
            modifier_tap: false,
        }
    }

    #[test]
    fn timer_reload_clear_allows_async_writes_outside_tsf_key_callbacks() {
        assert!(KeyEventSource::DeferredKey.async_dontcare());
        assert!(!KeyEventSource::OnKeyDown.async_dontcare());
        assert!(!KeyEventSource::OnKeyUp.async_dontcare());
    }

    #[test]
    fn reload_keys_replay_in_order_when_ready() {
        let now = Instant::now();
        let mut queue = DeferredKeys::default();
        for ch in ['y', 'u', 'n'] {
            assert!(queue.defer_failed(key(ch), false, now));
        }
        let mut replayed = Vec::new();
        while let Some(key) = queue.front() {
            replayed.push(key.keysym);
            queue.pop();
        }
        assert_eq!(replayed, vec!['y' as u32, 'u' as u32, 'n' as u32]);
    }

    #[test]
    fn modifier_tap_waits_for_prior_keys_and_written_timeout_never_replays() {
        let now = Instant::now();
        let mut queue = DeferredKeys::default();
        let toggle = DeferredKey {
            keysym: 0xffe1,
            modifier_tap: true,
            ..key(' ')
        };
        assert!(queue.defer_failed(key('y'), false, now));
        assert!(queue.defer_failed(toggle, false, now));
        assert_eq!(queue.front(), Some(key('y')));
        queue.pop();
        assert_eq!(queue.front(), Some(toggle));
        queue.pop();
        assert!(queue.defer_failed(toggle, true, now));
        assert_eq!(queue.front(), None);
    }

    #[test]
    fn busy_and_lock_contention_consume_and_queue_claimed_keys() {
        for _failure in ["Busy", "try_lock"] {
            let mut queue = DeferredKeys::default();
            assert!(queue.defer_failed(key('y'), false, Instant::now()));
            assert_eq!(queue.front(), Some(key('y')));
        }
    }

    #[test]
    fn deadline_drops_queue_and_keeps_passthrough_until_recovery() {
        let now = Instant::now();
        let mut queue = DeferredKeys::default();
        assert!(queue.defer_failed(key('y'), false, now));
        assert!(!queue.can_queue(now + RECOVERY_TIMEOUT));
        assert_eq!(queue.front(), None);
        assert!(!queue.defer_failed(key('u'), false, now + RECOVERY_TIMEOUT));
        queue.reset();
        assert!(queue.defer_failed(key('n'), false, now + RECOVERY_TIMEOUT));
    }

    #[test]
    fn overflow_is_bounded_and_written_timeout_is_never_replayed() {
        let now = Instant::now();
        let mut queue = DeferredKeys::default();
        assert!(queue.defer_failed(key('y'), true, now));
        assert_eq!(queue.front(), None);
        for _ in 0..KEY_LIMIT {
            assert!(queue.defer_failed(key('y'), false, now));
        }
        assert!(!queue.defer_failed(key('y'), false, now));
        assert_eq!(queue.front(), None);
    }

    #[test]
    fn full_queue_can_replay_without_overflowing() {
        let now = Instant::now();
        let mut queue = DeferredKeys::default();
        for _ in 0..KEY_LIMIT {
            assert!(queue.defer_failed(key('y'), false, now));
        }
        queue.expire(now);
        for _ in 0..KEY_LIMIT {
            assert_eq!(queue.front(), Some(key('y')));
            queue.pop();
        }
        assert_eq!(queue.front(), None);
    }
}
