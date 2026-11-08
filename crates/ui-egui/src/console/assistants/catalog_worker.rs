//! One bounded background named-assistant catalog write; acknowledge submitted context only.
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, SyncSender, TryRecvError},
    time::Duration,
};
#[derive(Clone, Debug)]
pub struct Job<T> {
    pub scope: String,
    pub name: String,
    pub dir: PathBuf,
    pub expected: T,
    pub next: T,
}
#[derive(Debug)]
pub struct Reply<T, R = ()> {
    pub job: Job<T>,
    pub result: Result<R, String>,
}
pub struct Worker<T, R = ()> {
    send: SyncSender<Job<T>>,
    replies: Receiver<Reply<T, R>>,
    pending: Option<Job<T>>,
}
impl<T: Clone + Send + 'static, R: Send + 'static> Worker<T, R> {
    pub fn start(write: impl Fn(&Job<T>) -> Result<R, String> + Send + 'static) -> Result<Self, String> {
        let (send, receive) = mpsc::sync_channel::<Job<T>>(1);
        let (reply, replies) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("assistant-catalog-save".into())
            .spawn(move || {
                while let Ok(job) = receive.recv() {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| write(&job)))
                        .unwrap_or_else(|_| Err("Assistant catalog worker failed; inspect saved instructions before retrying".into()));
                    if reply.send(Reply { job, result }).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| format!("Cannot start assistant catalog worker: {e}"))?;
        Ok(Self { send, replies, pending: None })
    }
    pub fn submit(&mut self, job: Job<T>) -> Result<(), String> {
        if self.pending.is_some() {
            return Err("Assistant catalog save is already pending".into());
        }
        self.send.try_send(job.clone()).map_err(|_| "Assistant catalog worker unavailable".to_string())?;
        self.pending = Some(job);
        Ok(())
    }
    pub fn pending(&self) -> bool {
        self.pending.is_some()
    }
    fn disconnected(&mut self) -> Option<Reply<T, R>> {
        self.pending.take().map(|job| Reply { job, result: Err("Assistant catalog worker disconnected; save outcome is uncertain".into()) })
    }
    pub fn poll(&mut self) -> Option<Reply<T, R>> {
        match self.replies.try_recv() {
            Ok(reply) => {
                self.pending = None;
                Some(reply)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => self.disconnected(),
        }
    }
    pub fn finish(&mut self, timeout: Duration) -> Result<Option<Reply<T, R>>, String> {
        if self.pending.is_none() {
            return Ok(None);
        }
        match self.replies.recv_timeout(timeout.min(Duration::from_secs(5))) {
            Ok(reply) => {
                self.pending = None;
                Ok(Some(reply))
            }
            Err(mpsc::RecvTimeoutError::Timeout) => Err("Assistant catalog save is still pending; previous saved instructions remain available".into()),
            Err(mpsc::RecvTimeoutError::Disconnected) => Ok(self.disconnected()),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn job() -> Job<String> {
        Job { scope: "project-A".into(), name: "Editor".into(), dir: PathBuf::from("unused"), expected: "old".into(), next: "new".into() }
    }
    #[test]
    fn one_pending_request_and_timeout_do_not_fake_success() {
        let (go, wait) = mpsc::sync_channel(1);
        let mut worker = Worker::start(move |_: &Job<String>| {
            wait.recv().map_err(|e| e.to_string())?;
            Ok(())
        })
        .unwrap();
        worker.submit(job()).unwrap();
        assert!(worker.submit(job()).is_err());
        assert!(worker.finish(Duration::ZERO).is_err());
        assert!(worker.pending());
        go.send(()).unwrap();
        let result = worker.finish(Duration::from_secs(1)).unwrap().unwrap();
        assert!(result.result.is_ok());
        assert_eq!(result.job.next, "new");
        assert_eq!(result.job.scope, "project-A");
        assert!(!worker.pending());
    }
    #[test]
    fn storage_failure_keeps_expected_version_for_explicit_recovery() {
        let mut worker: Worker<String> = Worker::start(|_: &Job<String>| Err("disk full".into())).unwrap();
        worker.submit(job()).unwrap();
        let result = worker.finish(Duration::from_secs(1)).unwrap().unwrap();
        assert_eq!(result.result.unwrap_err(), "disk full");
        assert_eq!(result.job.expected, "old");
        assert_eq!(result.job.next, "new");
    }
    #[test]
    fn panic_is_reported_and_worker_can_accept_next_request() {
        let mut worker = Worker::start(|job: &Job<String>| {
            if job.next == "new" {
                panic!("synthetic failure");
            }
            Ok(())
        })
        .unwrap();
        worker.submit(job()).unwrap();
        assert!(worker.finish(Duration::from_secs(1)).unwrap().unwrap().result.is_err());
        let mut next = job();
        next.next = "retry".into();
        worker.submit(next).unwrap();
        assert!(worker.finish(Duration::from_secs(1)).unwrap().unwrap().result.is_ok());
    }
    #[test]
    fn disconnected_worker_reports_uncertain_outcome_once() {
        let (send, receive) = mpsc::sync_channel(1);
        let (reply, replies) = mpsc::sync_channel(1);
        drop(receive);
        drop(reply);
        let mut worker: Worker<String> = Worker { send, replies, pending: Some(job()) };
        let result = worker.poll().unwrap();
        assert!(result.result.unwrap_err().contains("uncertain"));
        assert_eq!(result.job.scope, "project-A");
        assert!(worker.poll().is_none());
        assert!(!worker.pending());
        assert!(worker.submit(job()).is_err());
        assert!(!worker.pending());
    }
    #[test]
    fn late_ack_keeps_submitted_context_instead_of_newer_project_context() {
        let (go, wait) = mpsc::sync_channel(1);
        let mut worker = Worker::start(move |_: &Job<String>| {
            wait.recv().map_err(|e| e.to_string())?;
            Ok(())
        })
        .unwrap();
        let submitted = job();
        worker.submit(submitted).unwrap();
        assert!(worker.finish(Duration::ZERO).is_err());
        let mut newer = job();
        newer.scope = "project-B".into();
        newer.next = "newer instructions".into();
        assert!(worker.submit(newer.clone()).is_err());
        go.send(()).unwrap();
        let ack = worker.finish(Duration::from_secs(1)).unwrap().unwrap();
        assert_ne!(ack.job.scope, newer.scope);
        assert_ne!(ack.job.next, newer.next);
        assert_eq!(ack.job.next, "new");
        assert_eq!(ack.job.expected, "old");
    }
}
