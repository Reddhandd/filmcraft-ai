//! Bounded asynchronous input-device metadata; no driver access or worker join on the caller.
use std::collections::VecDeque;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Mutex, PoisonError, TryLockError};
use std::time::{Duration, Instant};
#[derive(Clone, Default)]
pub struct Snapshot {
    pub devices: Vec<String>,
    pub channels: u16,
}
#[derive(Clone, PartialEq, Eq)]
struct Key {
    host: String,
    device: String,
}
struct Entry {
    key: Key,
    snapshot: Snapshot,
    error: Option<String>,
    at: Instant,
}
#[derive(Default)]
struct State {
    desired: Option<Key>,
    cache: VecDeque<Entry>,
    pending: VecDeque<Key>,
    busy: Option<(Key, Instant)>,
    error: Option<String>,
    closed: bool,
}
pub struct Discovery {
    state: Arc<Mutex<State>>,
    wake: SyncSender<()>,
}
const DEADLINE: Duration = Duration::from_secs(2);
const MAX_KEYS: usize = 16;
impl Discovery {
    pub fn new(query: impl Fn(&str, &str) -> Result<Snapshot, String> + Send + 'static) -> Self {
        let state = Arc::new(Mutex::new(State::default()));
        let (wake, recv) = sync_channel(1);
        let shared = state.clone();
        let failed = state.clone();
        if let Err(e) = std::thread::Builder::new().name("filmcraft-input-discovery".into()).spawn(move || {
            let run = || {
                while recv.recv().is_ok() {
                    loop {
                        let key = {
                            let mut s = shared.lock().unwrap_or_else(PoisonError::into_inner);
                            if s.closed {
                                return;
                            }
                            let Some(key) = s.pending.pop_front() else { break };
                            s.busy = Some((key.clone(), Instant::now()));
                            key
                        };
                        // Never hold the metadata mutex while accessing a driver.
                        let result = query(&key.host, &key.device);
                        let (mut snapshot, error) = match result {
                            Ok(snapshot) => (snapshot, None),
                            Err(mut e) => {
                                truncate(&mut e, 4096);
                                (Snapshot::default(), Some(e))
                            }
                        };
                        snapshot.channels = snapshot.channels.min(256);
                        snapshot.devices.truncate(128);
                        for name in &mut snapshot.devices {
                            truncate(name, 1024);
                        }
                        let mut s = shared.lock().unwrap_or_else(PoisonError::into_inner);
                        s.busy = None;
                        if s.closed {
                            return;
                        }
                        s.cache.retain(|entry| entry.key != key);
                        if s.cache.len() >= MAX_KEYS {
                            s.cache.pop_front();
                        }
                        s.cache.push_back(Entry { key, snapshot, error, at: Instant::now() });
                    }
                }
            };
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)).is_err() {
                let mut s = failed.lock().unwrap_or_else(PoisonError::into_inner);
                s.error = Some("Input-device discovery failed; restart FilmCraft when convenient".into());
                s.closed = true;
            }
        }) {
            let mut s = state.lock().unwrap_or_else(PoisonError::into_inner);
            s.error = Some(format!("Cannot start input-device discovery: {e}"));
            s.closed = true;
        }
        Self { state, wake }
    }
    fn state(&self) -> Option<std::sync::MutexGuard<'_, State>> {
        match self.state.try_lock() {
            Ok(s) => Some(s),
            Err(TryLockError::Poisoned(e)) => Some(e.into_inner()),
            Err(TryLockError::WouldBlock) => None,
        }
    }
    pub fn get(&self, host: &str, device: Option<&str>) -> Snapshot {
        let Some(mut s) = self.state() else { return Snapshot::default() };
        let device = device.unwrap_or_else(|| s.desired.as_ref().filter(|k| k.host == host).map_or("", |k| k.device.as_str()));
        if host.len() > 1024 || device.len() > 1024 {
            s.error = Some("Input-device names exceed 1024 bytes; choose a valid device".into());
            return Snapshot::default();
        }
        let key = Key { host: host.into(), device: device.into() };
        if s.closed {
            return Snapshot::default();
        }
        s.error = None;
        s.desired = Some(key.clone());
        let cached = s.cache.iter().find(|entry| entry.key == key);
        if let Some(entry) = cached
            && entry.at.elapsed() < Duration::from_secs(if entry.error.is_some() { 1 } else { 5 })
        {
            return entry.snapshot.clone();
        }
        let fallback = cached.map(|entry| entry.snapshot.clone()).unwrap_or_default();
        if !s.busy.as_ref().is_some_and(|(busy, _)| *busy == key) && !s.pending.contains(&key) {
            if s.pending.len() >= MAX_KEYS {
                s.pending.pop_front();
            }
            s.pending.push_back(key);
            let _ = self.wake.try_send(());
        }
        fallback
    }
    pub fn error(&self) -> Option<String> {
        let s = self.state()?;
        if let Some(e) = &s.error {
            return Some(e.clone());
        }
        if let Some(entry) = s.cache.iter().find(|entry| Some(&entry.key) == s.desired.as_ref()) {
            if let Some(error) = &entry.error {
                return Some(error.clone());
            }
            if entry.snapshot.channels == 0 {
                return Some("No microphone input channels are available; check Audio Hardware or connect an input device.".into());
            }
        }
        s.busy
            .as_ref()
            .filter(|(_, since)| since.elapsed() >= DEADLINE)
            .map(|_| "Input hardware is not responding; the editor remains available. Check Audio Hardware or restart FilmCraft when convenient.".into())
    }
}
impl Drop for Discovery {
    fn drop(&mut self) {
        self.state.lock().unwrap_or_else(PoisonError::into_inner).closed = true;
        let _ = self.wake.try_send(());
    }
}
fn truncate(text: &mut String, limit: usize) {
    if text.len() > limit {
        let mut boundary = limit;
        while !text.is_char_boundary(boundary) {
            boundary = boundary.saturating_sub(1);
        }
        text.truncate(boundary);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    fn wait(mut check: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(2);
        while !check() {
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn blocked_driver_does_not_block_reads_or_drop() {
        let (entered, ready) = mpsc::channel();
        let (release, hold) = mpsc::channel();
        let probe = Discovery::new(move |_, _| {
            entered.send(()).unwrap();
            hold.recv().unwrap();
            Ok(Snapshot::default())
        });
        assert_eq!(probe.get("", Some("")).channels, 0);
        ready.recv_timeout(Duration::from_secs(1)).unwrap();
        let before = Instant::now();
        for n in 0..10000 {
            probe.get("", Some(&format!("device{n}")));
        }
        assert!(before.elapsed() < Duration::from_secs(1));
        assert!(probe.state.lock().unwrap().pending.len() <= MAX_KEYS);
        let before = Instant::now();
        drop(probe);
        assert!(before.elapsed() < Duration::from_millis(100));
        release.send(()).unwrap();
    }
    #[test]
    fn stale_host_result_never_becomes_current_channels() {
        let (entered, ready) = mpsc::channel();
        let (release, hold) = mpsc::channel();
        let probe = Discovery::new(move |host, _| {
            if host == "old" {
                entered.send(()).unwrap();
                hold.recv().unwrap();
            }
            Ok(Snapshot { devices: vec![host.into()], channels: if host == "old" { 2 } else { 8 } })
        });
        probe.get("old", Some(""));
        ready.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(probe.get("new", Some("")).channels, 0);
        release.send(()).unwrap();
        wait(|| probe.get("new", Some("")).channels == 8);
        assert_eq!(probe.get("new", None).devices, ["new"]);
    }
    #[test]
    fn panic_is_reported_and_names_are_bounded() {
        let probe = Discovery::new(|_, _| panic!("synthetic driver panic"));
        probe.get("", Some(""));
        wait(|| probe.error().is_some());
        assert_eq!(probe.get("", Some("")).channels, 0);
        let probe = Discovery::new(|_, _| Ok(Snapshot { devices: vec!["音".repeat(2000); 200], channels: u16::MAX }));
        wait(|| probe.get("", Some("")).channels != 0);
        let snapshot = probe.get("", None);
        assert_eq!(snapshot.channels, 256);
        assert_eq!(snapshot.devices.len(), 128);
        assert!(snapshot.devices.iter().all(|n| n.len() <= 1024));
        assert_eq!(probe.get(&"x".repeat(1025), Some("")).channels, 0);
        assert!(probe.error().is_some());
    }
    #[test]
    fn hung_query_reports_a_bounded_actionable_timeout() {
        let (entered, ready) = mpsc::channel();
        let (release, hold) = mpsc::channel();
        let probe = Discovery::new(move |_, _| {
            entered.send(()).unwrap();
            hold.recv().unwrap();
            Ok(Snapshot::default())
        });
        probe.get("", Some(""));
        ready.recv_timeout(Duration::from_secs(1)).unwrap();
        {
            let mut state = probe.state.lock().unwrap();
            state.busy.as_mut().unwrap().1 = Instant::now() - Duration::from_secs(3);
        }
        assert!(probe.error().unwrap().contains("editor remains available"));
        drop(probe);
        release.send(()).unwrap();
    }
    #[test]
    fn cached_queries_refresh_and_errors_do_not_probe_each_frame() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let probe = Discovery::new(move |_, _| {
            let n = counted.fetch_add(1, Ordering::SeqCst);
            Ok(Snapshot { devices: vec![], channels: (n + 1) as u16 })
        });
        wait(|| probe.get("", Some("")).channels == 1);
        for _ in 0..10000 {
            assert_eq!(probe.get("", None).channels, 1);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        {
            let mut s = probe.state.lock().unwrap();
            s.cache.front_mut().unwrap().at = Instant::now() - Duration::from_secs(6);
        }
        wait(|| probe.get("", Some("")).channels == 2);
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let probe = Discovery::new(move |_, _| {
            counted.fetch_add(1, Ordering::SeqCst);
            Err("driver unavailable".into())
        });
        probe.get("", Some(""));
        wait(|| probe.error().is_some());
        for _ in 0..10000 {
            probe.get("", Some(""));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn concurrent_default_and_named_queries_keep_independent_bounded_results() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let probe = Discovery::new(move |_, name| {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(Snapshot { devices: vec!["default".into(), "named".into()], channels: if name.is_empty() { 2 } else { 8 } })
        });
        wait(|| probe.get("", Some("")).channels == 2 && probe.get("", Some("named")).channels == 8);
        for _ in 0..10000 {
            assert_eq!(probe.get("", Some("")).channels, 2);
            assert_eq!(probe.get("", Some("named")).channels, 8);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        for n in 0..20 {
            wait(|| probe.get("", Some(&format!("named{n}"))).channels == 8);
        }
        assert!(probe.state.lock().unwrap().cache.len() <= MAX_KEYS);
        assert!(probe.state.lock().unwrap().pending.len() <= MAX_KEYS);
    }
}
