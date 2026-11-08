//! Monotonic checkpoint scheduling; no wall-clock arithmetic or automatic model calls.
use crate::console::assistants::CHECKPOINT_INTERVAL;
use std::time::{Duration, Instant};
const RETRY_DELAY: Duration = Duration::from_secs(30);

#[derive(Debug, Default)]
pub struct Schedule {
    dirty_since: Option<Instant>,
    last_attempt: Option<Instant>,
    last_success: Option<Instant>,
    failed: bool,
}
impl Schedule {
    pub fn has_changes(&self) -> bool {
        self.dirty_since.is_some()
    }
    pub fn changed(&mut self, now: Instant) {
        self.dirty_since.get_or_insert(now);
    }
    pub fn due(&self, now: Instant, milestone_or_close: bool) -> bool {
        let Some(dirty) = self.dirty_since else {
            return false;
        };
        if milestone_or_close {
            return true;
        }
        if self.failed {
            return self.last_attempt.is_none_or(|last| now.saturating_duration_since(last) >= RETRY_DELAY);
        }
        let anchor = self.last_success.unwrap_or(dirty);
        now.saturating_duration_since(anchor) >= CHECKPOINT_INTERVAL
    }
    /// Invoke only after the save attempt returns. A failed write retains dirty context.
    pub fn attempted(&mut self, now: Instant, success: bool) {
        self.last_attempt = Some(now);
        self.failed = !success;
        if success {
            self.last_success = Some(now);
            self.dirty_since = None;
        }
    }
    #[cfg(test)]
    pub fn saved_age(&self, now: Instant) -> Option<Duration> {
        self.last_success.map(|last| now.saturating_duration_since(last))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unchanged_context_never_saves_and_changed_context_reaches_fifteen_minutes() {
        let now = Instant::now();
        let mut schedule = Schedule::default();
        assert!(!schedule.due(now + CHECKPOINT_INTERVAL, false));
        schedule.changed(now);
        assert!(!schedule.due(now + Duration::from_secs(899), false));
        assert!(schedule.due(now + CHECKPOINT_INTERVAL, false));
        schedule.attempted(now + CHECKPOINT_INTERVAL, true);
        assert!(!schedule.due(now + CHECKPOINT_INTERVAL * 2, true));
        assert_eq!(schedule.saved_age(now + CHECKPOINT_INTERVAL + Duration::from_secs(5)), Some(Duration::from_secs(5)));
    }
    #[test]
    fn failure_remains_dirty_and_retries_without_waiting_another_fifteen_minutes() {
        let now = Instant::now();
        let mut schedule = Schedule::default();
        schedule.changed(now);
        schedule.attempted(now, false);
        assert!(!schedule.due(now + Duration::from_secs(1), false));
        assert!(schedule.due(now + RETRY_DELAY, false));
        assert!(schedule.due(now, true), "clean close can retry immediately");
        assert_eq!(schedule.saved_age(now), None);
        schedule.attempted(now + RETRY_DELAY, true);
        assert!(!schedule.due(now + CHECKPOINT_INTERVAL, false));
    }
    #[test]
    fn milestone_flushes_new_context_and_time_reversal_does_not_panic() {
        let now = Instant::now();
        let later = now + Duration::from_secs(2);
        let mut schedule = Schedule::default();
        schedule.changed(later);
        assert!(!schedule.due(now, false));
        assert!(schedule.due(now, true));
        schedule.attempted(later, true);
        assert_eq!(schedule.saved_age(now), Some(Duration::ZERO));
        schedule.changed(later);
        assert!(schedule.due(later, true));
    }
}
