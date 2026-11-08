//! Keep context learned during an in-flight save dirty until a later checkpoint succeeds.
use crate::console::assistants::schedule::Schedule;
use std::time::Instant;
#[derive(Debug, Default)]
pub struct Tracker {
    schedule: Schedule,
    version: u64,
    pending: Option<(u64, u64, u64)>,
    pub generation: u64,
    pub error: Option<String>,
}
impl Tracker {
    pub fn needs_polling(&self) -> bool {
        self.pending.is_some() || self.schedule.has_changes()
    }
    pub fn version(&self) -> u64 {
        self.version
    }
    pub fn restored(generation: u64) -> Self {
        Self { generation, ..Default::default() }
    }
    pub fn snapshot_changed(&mut self) -> Result<(), String> {
        self.version = self.version.checked_add(1).ok_or("Assistant context version exhausted; save and reopen")?;
        Ok(())
    }
    pub fn changed(&mut self, now: Instant) -> Result<(), String> {
        self.snapshot_changed()?;
        self.schedule.changed(now);
        Ok(())
    }
    pub fn due(&self, now: Instant, milestone_or_close: bool) -> bool {
        self.pending.is_none() && self.schedule.due(now, milestone_or_close)
    }
    pub fn next_generation(&self) -> Result<u64, String> {
        self.generation.checked_add(1).ok_or_else(|| "Assistant checkpoint generation exhausted".into())
    }
    pub fn queued(&mut self, ticket: u64, generation: u64) -> Result<(), String> {
        if self.pending.is_some() || generation != self.next_generation()? {
            return Err("Assistant checkpoint save state changed".into());
        }
        self.pending = Some((ticket, self.version, generation));
        Ok(())
    }
    pub fn completed(&mut self, ticket: u64, now: Instant, result: Result<(), String>) -> Result<(), String> {
        let (expected, version, generation) = self.pending.ok_or("No assistant checkpoint save is pending")?;
        if expected != ticket {
            return Err("Assistant checkpoint response does not match pending save".into());
        }
        self.pending = None;
        match result {
            Ok(()) => {
                self.generation = generation;
                self.error = None;
                self.schedule.attempted(now, true);
                if version != self.version {
                    self.schedule.changed(now);
                }
                Ok(())
            }
            Err(error) => {
                self.schedule.attempted(now, false);
                self.error = Some(error.clone());
                Err(error)
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn new_context_during_save_remains_dirty_and_next_generation_retains_it() {
        let now = Instant::now();
        let mut tracker = Tracker::default();
        tracker.changed(now).unwrap();
        tracker.queued(1, 1).unwrap();
        tracker.changed(now).unwrap();
        assert!(!tracker.due(now, true));
        tracker.completed(1, now, Ok(())).unwrap();
        assert!(tracker.due(now, true));
        assert_eq!(tracker.next_generation().unwrap(), 2);
        tracker.queued(2, 2).unwrap();
        tracker.completed(2, now, Ok(())).unwrap();
        assert!(!tracker.due(now, true));
    }
    #[test]
    fn failed_or_unmatched_reply_never_claims_context_saved() {
        let now = Instant::now();
        let mut tracker = Tracker::default();
        tracker.changed(now).unwrap();
        tracker.queued(4, 1).unwrap();
        assert!(tracker.completed(3, now, Ok(())).is_err());
        assert_eq!(tracker.generation, 0);
        assert!(!tracker.due(now, true));
        assert!(tracker.completed(4, now, Err("disk full".into())).is_err());
        assert_eq!(tracker.generation, 0);
        assert!(tracker.due(now, true));
        assert_eq!(tracker.error.as_deref(), Some("disk full"));
    }
}
