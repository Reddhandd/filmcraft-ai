//! One input capture worker across the process; cancellation never waits for the driver.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, TryLockError, mpsc};
use std::time::Duration;
static INPUT_SLOT: AtomicBool = AtomicBool::new(false);
struct Slot(&'static AtomicBool);
impl Drop for Slot {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
pub struct Capture<F: Send + 'static> {
    active: Arc<AtomicBool>,
    stop: Option<mpsc::SyncSender<()>>,
    worker: Option<std::thread::JoinHandle<()>>,
    slot: &'static AtomicBool,
    error: Arc<Mutex<Option<String>>>,
    marker: std::marker::PhantomData<F>,
}
impl<F: Send + 'static> Capture<F> {
    pub fn new() -> Self {
        Self::with_slot(&INPUT_SLOT)
    }
    fn with_slot(slot: &'static AtomicBool) -> Self {
        Self { active: Arc::new(AtomicBool::new(false)), stop: None, worker: None, slot, error: Arc::default(), marker: std::marker::PhantomData }
    }
    fn reap(&mut self) {
        if self.worker.as_ref().is_some_and(|w| w.is_finished())
            && let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            *self.error.lock().unwrap_or_else(PoisonError::into_inner) = Some("Input capture worker panicked".into());
        }
    }
    pub fn start<R: 'static>(
        &mut self,
        deadline: Duration,
        open: impl FnOnce(Arc<AtomicBool>) -> Result<(R, F), String> + Send + 'static,
    ) -> Result<F, String> {
        self.stop();
        if self.worker.is_some() || self.slot.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
            return Err("Input hardware is still busy; retry shortly or restart FilmCraft when convenient".into());
        }
        let slot = Slot(self.slot);
        *self.error.lock().unwrap_or_else(PoisonError::into_inner) = None;
        let active = Arc::new(AtomicBool::new(true));
        self.active = active.clone();
        let errors = self.error.clone();
        let (format_send, format_recv) = mpsc::sync_channel(1);
        let (stop_send, stop_recv) = mpsc::sync_channel(1);
        let worker = std::thread::Builder::new()
            .name("filmcraft-input-capture".into())
            .spawn(move || {
                let _slot = slot;
                let task = || match open(active.clone()) {
                    Ok((resource, format)) => {
                        if active.load(Ordering::Acquire) && format_send.try_send(Ok(format)).is_ok() && active.load(Ordering::Acquire) {
                            let _ = stop_recv.recv();
                        }
                        active.store(false, Ordering::Release);
                        drop(resource); // Driver teardown is also confined to this worker.
                    }
                    Err(e) => {
                        active.store(false, Ordering::Release);
                        *errors.lock().unwrap_or_else(PoisonError::into_inner) = Some(e.clone());
                        let _ = format_send.try_send(Err(e));
                    }
                };
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(task)).is_err() {
                    active.store(false, Ordering::Release);
                    let message = "Input capture worker panicked; recording unavailable".to_string();
                    *errors.lock().unwrap_or_else(PoisonError::into_inner) = Some(message.clone());
                    let _ = format_send.try_send(Err(message));
                }
            })
            .map_err(|e| {
                self.active.store(false, Ordering::Release);
                format!("Cannot start input capture worker: {e}")
            })?;
        self.worker = Some(worker);
        self.stop = Some(stop_send);
        let result = format_recv
            .recv_timeout(deadline)
            .map_err(|e| match e {
                mpsc::RecvTimeoutError::Timeout => {
                    "Input hardware did not respond before the startup deadline; recording was cancelled. Retry shortly or restart FilmCraft when convenient"
                        .into()
                }
                mpsc::RecvTimeoutError::Disconnected => "Input capture worker stopped before returning a format".into(),
            })
            .and_then(|r| r);
        if let Err(e) = &result {
            *self.error.lock().unwrap_or_else(PoisonError::into_inner) = Some(e.clone());
            self.stop();
        }
        result
    }
    pub fn stop(&mut self) {
        self.active.store(false, Ordering::Release);
        if let Some(stop) = self.stop.take() {
            let _ = stop.try_send(());
        }
        self.reap();
    }
    pub fn error(&self) -> Option<String> {
        match self.error.try_lock() {
            Ok(e) => e.clone(),
            Err(TryLockError::Poisoned(e)) => e.into_inner().clone(),
            Err(TryLockError::WouldBlock) => None,
        }
    }
}
impl<F: Send + 'static> Drop for Capture<F> {
    fn drop(&mut self) {
        self.stop();
    }
}
impl<F: Send + 'static> Default for Capture<F> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    fn wait(mut done: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(2);
        while !done() {
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn cancelled_late_open_does_not_activate_and_blocks_extra_workers() {
        static SLOT: AtomicBool = AtomicBool::new(false);
        let (release, hold) = mpsc::channel();
        let activated = Arc::new(AtomicBool::new(false));
        let observed = activated.clone();
        let mut input = Capture::<u32>::with_slot(&SLOT);
        let now = Instant::now();
        assert!(
            input
                .start(Duration::from_millis(10), move |active| {
                    hold.recv().unwrap();
                    if active.load(Ordering::Acquire) {
                        observed.store(true, Ordering::Release);
                    }
                    Ok(((), 48000))
                })
                .is_err()
        );
        assert!(now.elapsed() < Duration::from_secs(1));
        assert!(!input.active.load(Ordering::Acquire));
        let mut other = Capture::<u32>::with_slot(&SLOT);
        assert!(other.start::<()>(Duration::from_millis(10), |_| panic!("must not start another worker")).is_err());
        let now = Instant::now();
        drop(input);
        assert!(now.elapsed() < Duration::from_millis(100));
        assert!(SLOT.load(Ordering::Acquire));
        release.send(()).unwrap();
        wait(|| !SLOT.load(Ordering::Acquire));
        assert!(!activated.load(Ordering::Acquire));
        assert_eq!(other.start(Duration::from_secs(1), |_| Ok(((), 44100))).unwrap(), 44100);
        other.stop();
        wait(|| !SLOT.load(Ordering::Acquire));
    }
    struct BlockDrop(mpsc::Receiver<()>, mpsc::Sender<()>);
    impl Drop for BlockDrop {
        fn drop(&mut self) {
            self.1.send(()).unwrap();
            self.0.recv().unwrap();
        }
    }
    #[test]
    fn blocked_teardown_never_blocks_stop_or_drop() {
        static SLOT: AtomicBool = AtomicBool::new(false);
        let (release, hold) = mpsc::channel();
        let (entered, ready) = mpsc::channel();
        let mut input = Capture::<u32>::with_slot(&SLOT);
        assert_eq!(input.start(Duration::from_secs(1), move |_| Ok((BlockDrop(hold, entered), 48000))).unwrap(), 48000);
        let now = Instant::now();
        input.stop();
        assert!(now.elapsed() < Duration::from_millis(100));
        ready.recv_timeout(Duration::from_secs(1)).unwrap();
        let now = Instant::now();
        drop(input);
        assert!(now.elapsed() < Duration::from_millis(100));
        assert!(SLOT.load(Ordering::Acquire));
        release.send(()).unwrap();
        wait(|| !SLOT.load(Ordering::Acquire));
    }
    #[test]
    fn driver_panic_is_reported_and_releases_capture_slot() {
        static SLOT: AtomicBool = AtomicBool::new(false);
        let mut input = Capture::<u32>::with_slot(&SLOT);
        let result = input.start::<()>(Duration::from_secs(1), |_| panic!("synthetic input driver panic"));
        assert!(result.unwrap_err().contains("panicked"));
        wait(|| !SLOT.load(Ordering::Acquire));
        assert!(input.error().unwrap().contains("panicked"));
        wait(|| input.worker.as_ref().is_none_or(|w| w.is_finished()));
        input.stop();
        assert_eq!(input.start(Duration::from_secs(1), |_| Ok(((), 48000))).unwrap(), 48000);
        input.stop();
    }
}
