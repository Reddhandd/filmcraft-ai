//! One outstanding bounded checkpoint write; UI keeps changes made during that write dirty.
use crate::console::assistants::{Checkpoint, disk};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, SyncSender, TryRecvError},
    time::Duration,
};
struct Request {
    ticket: u64,
    dir: PathBuf,
    checkpoint: Checkpoint,
}
#[derive(Debug)]
pub struct Reply {
    pub ticket: u64,
    pub result: Result<(), String>,
}
pub struct Worker {
    send: SyncSender<Request>,
    replies: Receiver<Reply>,
    pending: Option<u64>,
    next: u64,
}
impl Worker {
    pub fn start() -> Result<Self, String> {
        Self::start_with(disk::save)
    }
    pub(super) fn start_with(write: impl Fn(&std::path::Path, &Checkpoint) -> Result<(), String> + Send + 'static) -> Result<Self, String> {
        let (send, receive) = mpsc::sync_channel::<Request>(1);
        let (reply, replies) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("assistant-checkpoint".into())
            .spawn(move || {
                let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    while let Ok(request) = receive.recv() {
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| write(&request.dir, &request.checkpoint)))
                            .unwrap_or_else(|_| Err("Assistant checkpoint worker failed safely; memory remains unsaved".into()));
                        if reply.send(Reply { ticket: request.ticket, result }).is_err() {
                            break;
                        }
                    }
                }));
                if stopped.is_err() {
                    let _ = reply.try_send(Reply { ticket: 0, result: Err("Assistant checkpoint worker stopped unexpectedly".into()) });
                }
            })
            .map_err(|e| format!("Cannot start assistant checkpoint worker: {e}"))?;
        Ok(Self { send, replies, pending: None, next: 0 })
    }
    pub fn submit(&mut self, dir: PathBuf, checkpoint: Checkpoint) -> Result<u64, String> {
        if self.pending.is_some() {
            return Err("Assistant checkpoint save is already pending".into());
        }
        checkpoint.encode()?;
        let ticket = self.next.checked_add(1).ok_or("Assistant checkpoint ticket exhausted")?;
        self.send.try_send(Request { ticket, dir, checkpoint }).map_err(|e| format!("Cannot queue assistant checkpoint: {e}"))?;
        self.next = ticket;
        self.pending = Some(ticket);
        Ok(ticket)
    }
    pub(super) fn pending(&self) -> bool {
        self.pending.is_some()
    }
    fn accept(&mut self, reply: Reply) -> Reply {
        let expected = self.pending.take();
        if expected != Some(reply.ticket) {
            return Reply { ticket: expected.unwrap_or(0), result: Err("Assistant checkpoint reply identity changed; reload saved memory".into()) };
        }
        reply
    }
    pub fn poll(&mut self) -> Option<Reply> {
        match self.replies.try_recv() {
            Ok(reply) => Some(self.accept(reply)),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.pending.take().map(|ticket| Reply { ticket, result: Err("Assistant checkpoint worker disconnected; memory remains unsaved".into()) })
            }
        }
    }
    /// Bounded clean-close wait; timeout leaves the old durable checkpoint usable.
    pub fn finish(&mut self, timeout: Duration) -> Result<Option<Reply>, String> {
        if self.pending.is_none() {
            return Ok(None);
        }
        let reply = self
            .replies
            .recv_timeout(timeout.min(Duration::from_secs(5)))
            .map_err(|e| format!("Assistant checkpoint has not finished; previous saved memory retained: {e}"))?;
        Ok(Some(self.accept(reply)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn checkpoint() -> Checkpoint {
        Checkpoint {
            schema: 1,
            project_scope: "A".into(),
            document_epoch: 1,
            revision: 2,
            generation: 1,
            context_version: 0,
            saved_at: "2026-11-01T12:00:00Z".into(),
            conversation: None,
            assistant_name: None,
            autosave: None,
            reference_documents: None,
            assistant: Default::default(),
            decisions: vec![],
            pending: vec![],
            errors: vec![],
            commands: vec![],
            recent_context: vec![],
        }
    }
    #[test]
    fn worker_writes_real_checkpoint_and_finishes_without_native_devices() {
        let dir = tempfile::tempdir().unwrap();
        let mut worker = Worker::start().unwrap();
        let ticket = worker.submit(dir.path().to_owned(), checkpoint()).unwrap();
        let reply = worker.finish(Duration::from_secs(2)).unwrap().unwrap();
        assert_eq!(reply.ticket, ticket);
        reply.result.unwrap();
        assert!(disk::load(dir.path(), "A").unwrap().is_some());
        assert!(worker.poll().is_none());
    }
    #[test]
    fn one_pending_write_bounds_queue_and_close_timeout_does_not_fake_success() {
        let (entered, ready) = mpsc::sync_channel(1);
        let (release, wait) = mpsc::sync_channel(1);
        let mut worker = Worker::start_with(move |_, _| {
            entered.send(()).map_err(|e| e.to_string())?;
            wait.recv().map_err(|e| e.to_string())
        })
        .unwrap();
        worker.submit(PathBuf::new(), checkpoint()).unwrap();
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(worker.submit(PathBuf::new(), checkpoint()).is_err());
        assert!(worker.finish(Duration::ZERO).is_err());
        release.send(()).unwrap();
        worker.finish(Duration::from_secs(2)).unwrap().unwrap().result.unwrap();
    }
    #[test]
    fn worker_panic_becomes_error_and_next_request_is_accepted() {
        let mut worker = Worker::start_with(|_, _| panic!("synthetic failure")).unwrap();
        for _ in 0..2 {
            worker.submit(PathBuf::new(), checkpoint()).unwrap();
            assert!(worker.finish(Duration::from_secs(2)).unwrap().unwrap().result.is_err());
        }
    }
}
