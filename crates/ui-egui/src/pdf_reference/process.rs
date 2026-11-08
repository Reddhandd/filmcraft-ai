//! Blocking child supervisor. Own this only on the reference background worker, never the UI.
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

// Requested page PNGs fit the existing 256 KiB inline-image contract plus base64 framing.
const MAX_REPLY: u64 = 384 * 1024;
const MAX_CHILDREN: usize = 4;
static CHILDREN: AtomicUsize = AtomicUsize::new(0);
struct Permit;
impl Permit {
    fn acquire() -> Result<Self, String> {
        for _ in 0..32 {
            let current = CHILDREN.load(Ordering::Acquire);
            if current >= MAX_CHILDREN {
                return Err("PDF helper process budget is full; close a selected reference or restart after an unconfirmed termination".into());
            }
            if CHILDREN.compare_exchange_weak(current, current.saturating_add(1), Ordering::AcqRel, Ordering::Acquire).is_ok() {
                return Ok(Self);
            }
        }
        Err("PDF helper process admission is busy; retry shortly".into())
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        CHILDREN.fetch_sub(1, Ordering::AcqRel);
    }
}
struct OwnedChild(Child, Option<Permit>);
impl OwnedChild {
    fn terminate(&mut self) -> bool {
        let _ = self.0.kill();
        let started = Instant::now();
        loop {
            match self.0.try_wait() {
                Ok(Some(_)) => return true,
                Err(_) => return false,
                Ok(None) => {}
            }
            if started.elapsed() >= Duration::from_millis(250) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !self.terminate()
            && let Some(permit) = self.1.take()
        {
            // Retain the slot until application exit: an unconfirmed kill cannot spawn
            // an unlimited series of replacements. Never join a stuck native child.
            std::mem::forget(permit);
        }
    }
}
struct Message {
    header: Vec<u8>,
    body: Option<Arc<[u8]>>,
}
/// One selected snapshot's child. All blocking operations belong to the background worker.
pub struct Process {
    child: OwnedChild,
    input: SyncSender<Message>,
    output: Receiver<Result<Value, String>>,
    cancelled: Arc<AtomicBool>,
    failed: bool,
}
impl Process {
    pub fn new(executable: &Path, cancelled: Arc<AtomicBool>) -> Result<Self, String> {
        let mut command = Command::new(executable);
        command.arg("--reference-pdf-worker");
        Self::spawn(command, cancelled)
    }
    fn spawn(mut command: Command, cancelled: Arc<AtomicBool>) -> Result<Self, String> {
        if cancelled.load(Ordering::Acquire) {
            return Err("PDF reference request was cancelled".into());
        }
        let permit = Permit::acquire()?;
        let mut child =
            OwnedChild(command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|error| error.to_string())?, Some(permit));
        let mut stdin = child.0.stdin.take().ok_or("PDF helper has no input pipe")?;
        let stdout = child.0.stdout.take().ok_or("PDF helper has no output pipe")?;
        let (input, requests) = sync_channel::<Message>(1);
        let (replies, output) = sync_channel(1);
        let errors = replies.clone();
        std::thread::Builder::new()
            .name("filmcraft-pdf-replies".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut reader = BufReader::new(stdout);
                    loop {
                        let mut line = Vec::new();
                        let size = (&mut reader).take(MAX_REPLY + 1).read_until(b'\n', &mut line).map_err(|error| error.to_string())?;
                        if size == 0 || size as u64 > MAX_REPLY || line.last() != Some(&b'\n') {
                            return Err("PDF helper closed or exceeded its reply budget".to_string());
                        }
                        let value = serde_json::from_slice(&line).map_err(|_| "PDF helper returned malformed JSON".to_string())?;
                        if replies.send(Ok(value)).is_err() {
                            return Ok(());
                        }
                    }
                }));
                let error = match result {
                    Ok(Err(error)) => error,
                    Ok(Ok(())) => return,
                    Err(_) => "PDF reply worker failed safely".into(),
                };
                let _ = replies.send(Err(error));
            })
            .map_err(|error| format!("Cannot start PDF reply worker: {error}"))?;
        std::thread::Builder::new()
            .name("filmcraft-pdf-input".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    while let Ok(message) = requests.recv() {
                        stdin.write_all(&message.header).map_err(|error| error.to_string())?;
                        if let Some(body) = message.body {
                            stdin.write_all(&body).map_err(|error| error.to_string())?;
                        }
                        stdin.flush().map_err(|error| error.to_string())?;
                    }
                    Ok::<(), String>(())
                }));
                let error = match result {
                    Ok(Err(error)) => error,
                    Ok(Ok(())) => return,
                    Err(_) => "PDF input worker failed safely".into(),
                };
                let _ = errors.send(Err(error));
            })
            .map_err(|error| format!("Cannot start PDF input worker: {error}"))?;
        Ok(Self { child, input, output, cancelled, failed: false })
    }
    fn fail(&mut self, message: String) -> Result<Value, String> {
        self.failed = true;
        self.child.terminate();
        Err(message)
    }
    fn call(&mut self, request: Value, body: Option<Arc<[u8]>>, deadline: Duration) -> Result<Value, String> {
        if self.failed {
            return Err("PDF helper is unavailable; reload the selected snapshot".into());
        }
        let mut header = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
        if header.len() >= 4096 || deadline.is_zero() {
            return Err("Invalid PDF request size or deadline".into());
        }
        header.push(b'\n');
        if self.input.try_send(Message { header, body }).is_err() {
            return self.fail("PDF helper input is busy or unavailable".into());
        }
        let started = Instant::now();
        loop {
            if self.cancelled.load(Ordering::Acquire) {
                return self.fail("PDF reference request was cancelled".into());
            }
            if started.elapsed() >= deadline {
                return self.fail("PDF reference timed out; the editor remains available".into());
            }
            match self.output.recv_timeout(Duration::from_millis(10)) {
                Ok(Ok(value)) => {
                    if value.get("ok").and_then(Value::as_bool) == Some(true) {
                        let Some(result) = value.get("result") else {
                            return self.fail("PDF helper omitted its result".into());
                        };
                        return Ok(result.clone());
                    }
                    if value.get("ok").and_then(Value::as_bool) == Some(false) {
                        return Err(value.get("error").and_then(Value::as_str).unwrap_or("PDF helper rejected the request").chars().take(1024).collect());
                    }
                    return self.fail("PDF helper returned an invalid response envelope".into());
                }
                Ok(Err(error)) => return self.fail(error),
                Err(RecvTimeoutError::Disconnected) => return self.fail("PDF reply worker closed unexpectedly".into()),
                Err(RecvTimeoutError::Timeout) => {}
            }
            match self.child.0.try_wait() {
                Ok(Some(_)) => return self.fail("PDF helper exited before completing the request".into()),
                Ok(None) => {}
                Err(error) => return self.fail(format!("Cannot inspect PDF helper: {error}")),
            }
        }
    }
    pub fn load(&mut self, bytes: Arc<[u8]>, deadline: Duration) -> Result<Value, String> {
        if bytes.is_empty() || bytes.len() > super::MAX_LIBRARY_INPUT {
            return Err("PDF snapshot must contain 1 byte–64 MiB".into());
        }
        self.call(json!({"action":"load","bytes":bytes.len()}), Some(bytes), deadline)
    }
    pub fn load_images(&mut self, bytes: Arc<[u8]>, deadline: Duration) -> Result<Value, String> {
        if bytes.is_empty() || bytes.len() > super::MAX_LIBRARY_INPUT {
            return Err("Visual PDF snapshot exceeds 64 MiB".into());
        }
        self.call(json!({"action":"load","bytes":bytes.len(),"images":true}), Some(bytes), deadline)
    }
    /// Model payloads are pinned and validated before crossing the helper boundary.
    /// Loading never opens a path or contacts an AI provider.
    pub fn load_ocr(&mut self, detection: &[u8], recognition: &[u8], deadline: Duration) -> Result<Value, String> {
        super::ocr::verify_models(detection, recognition)?;
        let total = detection.len().checked_add(recognition.len()).ok_or("OCR model byte count overflow")?;
        let mut payload = Vec::new();
        payload.try_reserve_exact(total).map_err(|error| format!("Cannot allocate bounded OCR model payload: {error}"))?;
        payload.extend_from_slice(detection);
        payload.extend_from_slice(recognition);
        self.call(json!({"action":"ocrModels","detection":detection.len(),"recognition":recognition.len()}), Some(Arc::from(payload)), deadline)
    }
    pub fn ocr_page(&mut self, page: u32, deadline: Duration) -> Result<Value, String> {
        if !(1..=4096).contains(&page) {
            return Err("Choose an existing physical PDF page, 1–4096".into());
        }
        self.call(json!({"action":"ocrPage","page":page}), None, deadline)
    }
    pub fn render(&mut self, page: u32, max_side: u32, deadline: Duration) -> Result<Value, String> {
        if !(256..=1600).contains(&max_side) || page == 0 {
            return Err("Choose a positive page and 256–1600 pixels".into());
        }
        self.call(json!({"action":"render","page":page,"maxSide":max_side}), None, deadline)
    }
    pub fn read(&mut self, page: u32, offset: usize, limit: usize, deadline: Duration) -> Result<Value, String> {
        self.call(json!({"action":"read","page":page,"offset":offset,"limit":limit}), None, deadline)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    static TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn fake(script: &str, cancelled: Arc<AtomicBool>) -> Process {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        Process::spawn(command, cancelled).unwrap()
    }
    #[test]
    fn helper_budget_rejects_excess_children_and_releases_reaped_slots() {
        let _serial = TESTS.lock().unwrap();
        let children: Vec<_> = (0..MAX_CHILDREN).map(|_| fake("exec sleep 30", Arc::new(AtomicBool::new(false)))).collect();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exec sleep 30"]);
        assert!(Process::spawn(command, Arc::new(AtomicBool::new(false))).is_err());
        drop(children);
        let _next = fake("exec sleep 30", Arc::new(AtomicBool::new(false)));
    }
    #[test]
    fn blocked_child_is_killed_on_deadline_and_cannot_be_reused() {
        let _serial = TESTS.lock().unwrap();
        let mut process = fake("exec sleep 30", Arc::new(AtomicBool::new(false)));
        let started = Instant::now();
        assert!(process.read(1, 0, 4096, Duration::from_millis(30)).is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(process.child.0.try_wait().unwrap().is_some());
        assert!(process.read(1, 0, 4096, Duration::from_secs(1)).is_err());
    }
    #[test]
    fn cancelled_and_oversized_child_responses_fail_without_hanging() {
        let _serial = TESTS.lock().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut process = fake("exec sleep 30", cancel.clone());
        cancel.store(true, Ordering::Release);
        assert!(process.read(1, 0, 4096, Duration::from_secs(1)).is_err());
        assert!(process.child.0.try_wait().unwrap().is_some());
        let mut process = fake("printf '%040000d\\n' 0", Arc::new(AtomicBool::new(false)));
        assert!(process.read(1, 0, 4096, Duration::from_secs(1)).is_err());
        assert!(process.child.0.try_wait().unwrap().is_some());
    }
}
