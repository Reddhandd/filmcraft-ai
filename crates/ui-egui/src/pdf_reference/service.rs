//! One lazily created reference worker. UI callers only submit, cancel and poll bounded queues.
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

static WORKER_ACTIVE: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
pub(crate) fn test_worker_active() -> bool {
    WORKER_ACTIVE.load(Ordering::Acquire)
}
struct Permit;
impl Drop for Permit {
    fn drop(&mut self) {
        WORKER_ACTIVE.store(false, Ordering::Release);
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Key {
    pub epoch: u64,
    pub snapshot: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Load,
    Read,
}
#[derive(Debug)]
pub struct Reply {
    pub ticket: u64,
    pub key: Key,
    pub kind: Kind,
    pub result: Result<Value, String>,
}
enum Operation {
    Load(PathBuf),
    Read { page: u32, offset: usize, limit: usize },
}
struct Request {
    ticket: u64,
    key: Key,
    kind: Kind,
    operation: Operation,
    cancelled: Arc<AtomicBool>,
}
struct Pending {
    key: Key,
    kind: Kind,
    started: Instant,
}
#[derive(Default)]
struct State {
    next: u64,
    live: HashMap<Key, Arc<AtomicBool>>,
    pending: HashMap<u64, Pending>,
}
struct Inner {
    state: RefCell<State>,
    requests: SyncSender<Request>,
    replies: RefCell<Receiver<Reply>>,
    closed: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    timeout: Duration,
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
        for cancelled in self.state.get_mut().live.values() {
            cancelled.store(true, Ordering::Release);
        }
        // Dropping receivers releases a reply sender; never join a blocked file/device operation.
    }
}
#[derive(Clone)]
pub struct Service(Rc<Inner>);
impl std::fmt::Debug for Service {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PdfReferenceService")
    }
}
pub(crate) trait Backend {
    fn load(&mut self, key: Key, path: &Path, cancelled: Arc<AtomicBool>) -> Result<Value, String>;
    fn read(&mut self, key: Key, page: u32, offset: usize, limit: usize) -> Result<Value, String>;
    fn prune(&mut self);
}
impl Service {
    pub fn new(notify: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        Self::with_backend(notify, Duration::from_secs(30), || {
            Ok(Native { executable: std::env::current_exe().map_err(|error| error.to_string())?, documents: HashMap::new() })
        })
    }
    pub(crate) fn with_backend<B: Backend + Send + 'static>(
        notify: Arc<dyn Fn() + Send + Sync>,
        timeout: Duration,
        factory: impl FnOnce() -> Result<B, String> + Send + 'static,
    ) -> Result<Self, String> {
        if WORKER_ACTIVE.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
            return Err("The PDF reference worker is still active or stopping; retry when it finishes".into());
        }
        let permit = Permit;
        let (requests, receive) = sync_channel::<Request>(4);
        let (send, replies) = sync_channel(8);
        let closed = Arc::new(AtomicBool::new(false));
        let failed = Arc::new(AtomicBool::new(false));
        let shutdown = closed.clone();
        let failure = failed.clone();
        std::thread::Builder::new()
            .name("filmcraft-pdf-references".into())
            .spawn(move || {
                let _permit = permit;
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut backend = factory()?;
                    while !shutdown.load(Ordering::Acquire) {
                        backend.prune();
                        let request = match receive.recv_timeout(Duration::from_millis(20)) {
                            Ok(request) => request,
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok::<(), String>(()),
                        };
                        if request.cancelled.load(Ordering::Acquire) {
                            continue;
                        }
                        let result = match request.operation {
                            Operation::Load(path) => backend.load(request.key, &path, request.cancelled.clone()),
                            Operation::Read { page, offset, limit } => backend.read(request.key, page, offset, limit),
                        }
                        .map_err(|error| error.chars().take(1024).collect());
                        if send.send(Reply { ticket: request.ticket, key: request.key, kind: request.kind, result }).is_err() {
                            return Ok(());
                        }
                        notify();
                    }
                    Ok(())
                }));
                if !matches!(outcome, Ok(Ok(()))) {
                    failure.store(true, Ordering::Release);
                    notify();
                }
            })
            .map_err(|error| format!("Cannot start PDF reference worker: {error}"))?;
        Ok(Self(Rc::new(Inner { state: RefCell::new(State::default()), requests, replies: RefCell::new(replies), closed, failed, timeout })))
    }
    fn submit(&self, key: Key, operation: Operation) -> Result<u64, String> {
        if self.0.closed.load(Ordering::Acquire) || self.0.failed.load(Ordering::Acquire) {
            return Err("PDF reference worker unavailable; restart FilmCraft when convenient".into());
        }
        let mut state = self.0.state.try_borrow_mut().map_err(|_| "PDF reference state is busy")?;
        if state.pending.len() >= 8 {
            return Err("PDF reference request budget is full; wait for existing work".into());
        }
        let kind = if matches!(&operation, Operation::Load(_)) { Kind::Load } else { Kind::Read };
        let is_new = kind == Kind::Load;
        let cancelled = if is_new {
            if state.live.len() >= 4 || state.live.contains_key(&key) {
                return Err("Maximum four PDF snapshots; remove a reference before adding another".into());
            }
            Arc::new(AtomicBool::new(false))
        } else {
            state.live.get(&key).filter(|flag| !flag.load(Ordering::Acquire)).cloned().ok_or("PDF snapshot is no longer selected")?
        };
        let ticket = state.next.checked_add(1).ok_or("PDF request counter exhausted; restart FilmCraft when convenient")?;
        self.0
            .requests
            .try_send(Request { ticket, key, kind, operation, cancelled: cancelled.clone() })
            .map_err(|_| "PDF worker queue is full or unavailable; retry shortly")?;
        if is_new {
            state.live.insert(key, cancelled);
        }
        state.next = ticket;
        state.pending.insert(ticket, Pending { key, kind, started: Instant::now() });
        Ok(ticket)
    }
    pub fn load(&self, key: Key, path: PathBuf) -> Result<u64, String> {
        if path.as_os_str().len() > 4096 {
            return Err("PDF reference path exceeds its input budget".into());
        }
        self.submit(key, Operation::Load(path))
    }
    pub fn read(&self, key: Key, page: u32, offset: usize, limit: usize) -> Result<u64, String> {
        if page == 0 || limit == 0 || limit > 4096 || offset > 64 * 1024 {
            return Err("Invalid PDF page, offset or text limit".into());
        }
        self.submit(key, Operation::Read { page, offset, limit })
    }
    pub fn cancel(&self, key: Key) -> Result<(), String> {
        let mut state = self.0.state.try_borrow_mut().map_err(|_| "PDF reference state is busy")?;
        if let Some(flag) = state.live.remove(&key) {
            flag.store(true, Ordering::Release);
        }
        state.pending.retain(|_, pending| pending.key != key);
        Ok(())
    }
    pub fn poll(&self) -> Result<Vec<Reply>, String> {
        let mut state = self.0.state.try_borrow_mut().map_err(|_| "PDF reference state is busy")?;
        let replies = self.0.replies.try_borrow_mut().map_err(|_| "PDF reply state is busy")?;
        let mut ready = Vec::new();
        for _ in 0..8 {
            match replies.try_recv() {
                Ok(reply) => {
                    if let Some(pending) = state.pending.remove(&reply.ticket)
                        && pending.key == reply.key
                        && pending.kind == reply.kind
                        && state.live.get(&reply.key).is_some_and(|flag| !flag.load(Ordering::Acquire))
                    {
                        if reply.kind == Kind::Load
                            && reply.result.is_err()
                            && let Some(flag) = state.live.remove(&reply.key)
                        {
                            flag.store(true, Ordering::Release);
                        }
                        ready.push(reply);
                    }
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        let expired: Vec<_> = state
            .pending
            .iter()
            .filter(|(_, pending)| self.0.failed.load(Ordering::Acquire) || pending.started.elapsed() >= self.0.timeout)
            .map(|(ticket, _)| *ticket)
            .collect();
        for ticket in expired {
            if let Some(pending) = state.pending.remove(&ticket) {
                if let Some(flag) = state.live.remove(&pending.key) {
                    flag.store(true, Ordering::Release);
                }
                ready.push(Reply {
                    ticket,
                    key: pending.key,
                    kind: pending.kind,
                    result: Err("PDF reference request failed or timed out; the editor remains available".into()),
                });
            }
        }
        Ok(ready)
    }
    pub fn pending(&self) -> bool {
        self.0.state.try_borrow().is_ok_and(|state| !state.pending.is_empty())
    }
}
struct Document {
    process: super::process::Process,
    cancelled: Arc<AtomicBool>,
    path: PathBuf,
}
struct Native {
    executable: PathBuf,
    documents: HashMap<Key, Document>,
}
impl Backend for Native {
    fn load(&mut self, key: Key, path: &Path, cancelled: Arc<AtomicBool>) -> Result<Value, String> {
        if self.documents.len() >= 4 {
            return Err("PDF snapshot budget is full".into());
        }
        let path = path.canonicalize().map_err(|error| format!("Cannot locate selected PDF: {error}"))?;
        if self.documents.values().any(|document| document.path == path) {
            return Err("This PDF is already selected".into());
        }
        if !path.metadata().map_err(|error| error.to_string())?.is_file() {
            return Err("Choose a regular PDF file".into());
        }
        let file = std::fs::File::open(&path).map_err(|error| error.to_string())?;
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() || metadata.len() > super::MAX_INPUT as u64 {
            return Err("Choose a regular PDF of at most 32 MiB".into());
        }
        let mut bytes = Vec::new();
        file.take(super::MAX_INPUT as u64 + 1).read_to_end(&mut bytes).map_err(|error| error.to_string())?;
        if bytes.is_empty() || bytes.len() > super::MAX_INPUT {
            return Err("PDF snapshot must contain 1 byte–32 MiB".into());
        }
        if cancelled.load(Ordering::Acquire) {
            return Err("PDF selection was cancelled".into());
        }
        let size = bytes.len();
        let mut process = super::process::Process::new(&self.executable, cancelled.clone())?;
        let mut result = process.load(Arc::from(bytes), Duration::from_secs(10))?;
        if !result.get("pageCount").and_then(Value::as_u64).is_some_and(|count| (1..=4096).contains(&count))
            || result.get("inputBytes").and_then(Value::as_u64) != Some(size as u64)
        {
            return Err("PDF helper returned invalid snapshot metadata".into());
        }
        let preview = process.read(1, 0, 4096, Duration::from_secs(2));
        result["path"] = json!(path.to_string_lossy());
        match preview {
            Ok(page) => result["firstPage"] = page,
            Err(error) => result["firstPageError"] = json!(error),
        }
        self.documents.insert(key, Document { process, cancelled, path });
        Ok(result)
    }
    fn read(&mut self, key: Key, page: u32, offset: usize, limit: usize) -> Result<Value, String> {
        let document = self.documents.get_mut(&key).ok_or("PDF snapshot is unavailable; select the file again")?;
        document.process.read(page, offset, limit, Duration::from_secs(2))
    }
    fn prune(&mut self) {
        self.documents.retain(|_, document| !document.cancelled.load(Ordering::Acquire));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    struct Fake {
        entered: SyncSender<Key>,
        release: Receiver<()>,
    }
    impl Backend for Fake {
        fn load(&mut self, key: Key, path: &Path, _cancelled: Arc<AtomicBool>) -> Result<Value, String> {
            if path == Path::new("panic.pdf") {
                panic!("synthetic PDF backend failure");
            }
            self.entered.send(key).unwrap();
            self.release.recv().unwrap();
            Ok(json!({"pageCount":2}))
        }
        fn read(&mut self, _key: Key, page: u32, offset: usize, limit: usize) -> Result<Value, String> {
            Ok(json!({"page":page,"offset":offset,"limit":limit}))
        }
        fn prune(&mut self) {}
    }
    fn wait(mut predicate: impl FnMut() -> bool) {
        let started = Instant::now();
        while !predicate() {
            assert!(started.elapsed() < Duration::from_secs(2));
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    #[test]
    fn blocked_load_keeps_admission_polling_and_cancellation_available_and_discards_late_replies() {
        let _serial = TEST_LOCK.lock().unwrap();
        let (entered, receive) = sync_channel(4);
        let (release, resume) = sync_channel(4);
        let starts = Arc::new(AtomicUsize::new(0));
        let count = starts.clone();
        let notices = Arc::new(AtomicUsize::new(0));
        let notice = notices.clone();
        let service = Service::with_backend(
            Arc::new(move || {
                notice.fetch_add(1, Ordering::AcqRel);
            }),
            Duration::from_secs(30),
            move || {
                count.fetch_add(1, Ordering::AcqRel);
                Ok(Fake { entered, release: resume })
            },
        )
        .unwrap();
        let first = Key { epoch: 1, snapshot: 1 };
        service.load(first, PathBuf::from("first.pdf")).unwrap();
        assert_eq!(receive.recv_timeout(Duration::from_secs(1)).unwrap(), first);
        let started = Instant::now();
        assert!(service.poll().unwrap().is_empty());
        for snapshot in 2..=4 {
            service.load(Key { epoch: 1, snapshot }, PathBuf::from("next.pdf")).unwrap();
        }
        assert!(service.load(Key { epoch: 1, snapshot: 5 }, PathBuf::from("extra.pdf")).is_err());
        for snapshot in 1..=4 {
            service.cancel(Key { epoch: 1, snapshot }).unwrap();
        }
        assert!(!service.pending());
        assert!(started.elapsed() < Duration::from_secs(1), "UI-side calls must complete before the fake backend is released");
        release.send(()).unwrap();
        wait(|| notices.load(Ordering::Acquire) > 0);
        assert!(service.poll().unwrap().is_empty(), "a cancelled load cannot restore a reference");
        assert_eq!(starts.load(Ordering::Acquire), 1);
        drop(service);
        wait(|| !WORKER_ACTIVE.load(Ordering::Acquire));
    }
    #[test]
    fn expired_or_dropped_requests_cannot_accumulate_replacement_workers() {
        let _serial = TEST_LOCK.lock().unwrap();
        let (entered, receive) = sync_channel(1);
        let (release, resume) = sync_channel(1);
        let service = Service::with_backend(Arc::new(|| {}), Duration::from_millis(20), move || Ok(Fake { entered, release: resume })).unwrap();
        let key = Key { epoch: 1, snapshot: 1 };
        let ticket = service.load(key, PathBuf::from("slow.pdf")).unwrap();
        receive.recv_timeout(Duration::from_secs(1)).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        let replies = service.poll().unwrap();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].ticket, ticket);
        assert!(replies[0].result.is_err());
        assert!(Service::new(Arc::new(|| {})).is_err());
        let started = Instant::now();
        drop(service);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(Service::new(Arc::new(|| {})).is_err(), "a blocked worker retains its sole process-wide admission slot");
        release.send(()).unwrap();
        wait(|| !WORKER_ACTIVE.load(Ordering::Acquire));
    }
    #[test]
    fn panicking_backend_becomes_a_failed_request_without_a_stuck_ticket() {
        let _serial = TEST_LOCK.lock().unwrap();
        let (entered, _receive) = sync_channel(1);
        let (_release, resume) = sync_channel(1);
        let service = Service::with_backend(Arc::new(|| {}), Duration::from_secs(30), move || Ok(Fake { entered, release: resume })).unwrap();
        service.load(Key { epoch: 1, snapshot: 1 }, PathBuf::from("panic.pdf")).unwrap();
        wait(|| service.0.failed.load(Ordering::Acquire));
        let replies = service.poll().unwrap();
        assert_eq!(replies.len(), 1);
        assert!(replies[0].result.is_err());
        assert!(!service.pending());
        drop(service);
        wait(|| !WORKER_ACTIVE.load(Ordering::Acquire));
    }
}
