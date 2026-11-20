use std::time::{Duration, Instant};

const ACTIVITY_WINDOW: Duration = Duration::from_secs(3);
const PENDING_IO_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Default)]
pub(super) struct GenerationActivity {
    last_key: Option<Instant>,
}

impl GenerationActivity {
    pub(super) fn record_key(&mut self, now: Instant) -> bool {
        let was_idle = self.remaining(now).is_none();
        self.last_key = Some(now);
        was_idle
    }

    pub(super) fn remaining(&self, now: Instant) -> Option<Duration> {
        let elapsed = now.saturating_duration_since(self.last_key?);
        (elapsed < ACTIVITY_WINDOW).then(|| ACTIVITY_WINDOW - elapsed)
    }
}

pub(super) fn reaper_wait(
    activity_remaining: Option<Duration>,
    until_poll: Duration,
    has_pending_io: bool,
) -> Option<Duration> {
    let poll_wait = activity_remaining.map(|remaining| remaining.min(until_poll));
    if has_pending_io {
        Some(
            poll_wait
                .unwrap_or(PENDING_IO_INTERVAL)
                .min(PENDING_IO_INTERVAL),
        )
    } else {
        poll_wait
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_key_after_idle_wakes_background_generation_check() {
        let mut activity = GenerationActivity::default();
        let start = Instant::now();
        assert_eq!(activity.remaining(start), None);
        assert!(activity.record_key(start));
        assert_eq!(activity.remaining(start), Some(ACTIVITY_WINDOW));
        let active = start + Duration::from_secs(2);
        assert!(!activity.record_key(active));
        assert_eq!(activity.remaining(active), Some(ACTIVITY_WINDOW));
        assert!(activity
            .remaining(active + ACTIVITY_WINDOW - Duration::from_nanos(1))
            .is_some());
        assert_eq!(activity.remaining(active + ACTIVITY_WINDOW), None);
        assert!(activity.record_key(active + ACTIVITY_WINDOW));
        assert!(activity.record_key(active + ACTIVITY_WINDOW * 3));
    }

    #[test]
    fn idle_reaper_blocks_unless_io_is_pending() {
        let until_poll = Duration::from_millis(250);
        assert_eq!(reaper_wait(None, until_poll, false), None);
        assert_eq!(
            reaper_wait(None, until_poll, true),
            Some(PENDING_IO_INTERVAL)
        );
        let active = Some(ACTIVITY_WINDOW);
        assert_eq!(reaper_wait(active, until_poll, false), Some(until_poll));
        assert_eq!(
            reaper_wait(active, until_poll, true),
            Some(PENDING_IO_INTERVAL)
        );
        let until_idle = Duration::from_millis(10);
        assert_eq!(
            reaper_wait(Some(until_idle), until_poll, false),
            Some(until_idle)
        );
    }
}
