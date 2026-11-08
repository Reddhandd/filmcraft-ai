//! Cooperative import cancellation with an explicit, non-cancellable publication boundary.
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};
#[derive(Clone, Debug)]
pub(super) struct Control {
    pub cancelled: Arc<AtomicBool>,
    phase: Arc<AtomicU8>,
    page: Arc<AtomicU32>,
    total: Arc<AtomicU32>,
    started: Instant,
}
impl Default for Control {
    fn default() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            phase: Arc::new(AtomicU8::new(0)),
            page: Arc::new(AtomicU32::new(0)),
            total: Arc::new(AtomicU32::new(0)),
            started: Instant::now(),
        }
    }
}
impl Control {
    pub(super) fn cancel(&self) -> bool {
        if self.phase.compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            self.cancelled.store(true, Ordering::Release);
            true
        } else {
            false
        }
    }
    pub(super) fn check(&self) -> Result<(), String> {
        if self.cancelled.load(Ordering::Acquire) || self.phase.load(Ordering::Acquire) == 2 {
            return Err("Book import cancelled; existing library is unchanged".into());
        }
        if self.started.elapsed() > Duration::from_secs(30 * 60) {
            return Err("Book import exceeded its 30-minute deadline; existing library is unchanged".into());
        }
        Ok(())
    }
    pub(super) fn progress(&self, page: u32, total: u32) {
        self.total.store(total, Ordering::Release);
        self.page.store(page, Ordering::Release);
    }
    pub(super) fn publishing(&self) -> bool {
        self.phase.load(Ordering::Acquire) == 1
    }
    pub(super) fn status(&self) -> serde_json::Value {
        serde_json::json!({"page":self.page.load(Ordering::Acquire),"totalPages":self.total.load(Ordering::Acquire),"publishing":self.phase.load(Ordering::Acquire)==1,"cancelRequested":self.phase.load(Ordering::Acquire)==2})
    }
    pub(super) fn publish(&self) -> Result<(), String> {
        self.check()?;
        self.phase.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).map_err(|_| "Import was cancelled or publication already started")?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn acknowledged_cancellation_cannot_cross_publication_boundary() {
        let control = Control::default();
        let worker = control.clone();
        assert!(control.cancel());
        assert!(worker.publish().is_err());
        assert!(worker.check().is_err());
        let control = Control::default();
        control.publish().unwrap();
        assert!(!control.cancel());
        assert!(!control.cancelled.load(Ordering::Acquire));
    }
}
