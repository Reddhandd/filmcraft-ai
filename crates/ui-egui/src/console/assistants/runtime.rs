//! Project-scoped coordinator; one owner polls one checkpoint worker.
use crate::console::assistants::{Checkpoint, disk, tracker::Tracker, worker::Worker};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
pub struct Runtime {
    dir: PathBuf,
    snapshot: Checkpoint,
    tracker: Tracker,
    worker: Worker,
    pub recovered_previous: bool,
    pending_timestamp: Option<String>,
    compaction: Option<super::catalog_worker::Worker<Checkpoint, u64>>,
    waiting_compaction: Option<super::catalog_worker::Job<Checkpoint>>,
    pub(super) compaction_status: Option<String>,
    pub(super) autosave_state: Option<super::autosave::State>,
}
impl Runtime {
    pub(super) fn compaction_pending(&self) -> bool {
        self.waiting_compaction.is_some() || self.compaction.as_ref().is_some_and(|worker| worker.pending())
    }
    pub fn needs_polling(&self) -> bool {
        self.compaction_pending() || self.tracker.needs_polling() || self.autosave_state.as_ref().is_some_and(|state| state.needs_polling())
    }
    pub fn save_error(&self) -> Option<&str> {
        self.tracker.error.as_deref()
    }
    pub fn version(&self) -> u64 {
        self.tracker.version()
    }
    pub fn open(dir: PathBuf, initial: Checkpoint) -> Result<Self, String> {
        initial.validate(&initial.project_scope)?;
        let saved = disk::load(&dir, &initial.project_scope)?;
        let (snapshot, recovered_previous) = saved.unwrap_or((initial, false));
        let tracker = Tracker::restored(snapshot.generation);
        Ok(Self {
            dir,
            snapshot,
            tracker,
            worker: Worker::start()?,
            recovered_previous,
            pending_timestamp: None,
            autosave_state: None,
            compaction: None,
            waiting_compaction: None,
            compaction_status: None,
        })
    }
    /// Archive the exact source off-thread before adopting any condensed notes.
    pub(super) fn compact(&mut self, summary: &[u8]) -> Result<(), String> {
        if self.compaction_pending() {
            return Err("A memory compaction is already pending".into());
        }
        let next = super::compaction::context_candidate(&self.snapshot, summary)?;
        if self.compaction.is_none() {
            self.compaction = Some(super::catalog_worker::Worker::start(|job: &super::catalog_worker::Job<Checkpoint>| {
                let _lock = disk::ScopeLock::take(&job.dir)?;
                let original = super::compaction_archive::Original {
                    scope: job.expected.project_scope.clone(),
                    target: super::compaction_archive::Target::AssistantContext,
                    document_epoch: job.expected.document_epoch,
                    version: job.expected.context_version,
                    text: String::from_utf8(job.expected.encode()?).map_err(|error| error.to_string())?,
                };
                super::compaction_archive::retain(&job.dir, &original)
            })?);
        }
        self.waiting_compaction = Some(super::catalog_worker::Job {
            scope: self.snapshot.project_scope.clone(),
            name: "working notes".into(),
            dir: self.dir.clone(),
            expected: self.snapshot.clone(),
            next,
        });
        self.compaction_status = Some("Retaining original memory before compaction".into());
        Ok(())
    }
    fn poll_compaction(&mut self, now: Instant) -> Result<(), String> {
        let Some(reply) = self.compaction.as_mut().and_then(|worker| worker.poll()) else { return Ok(()) };
        self.adopt_compaction(reply, now)
    }
    fn adopt_compaction(&mut self, reply: super::catalog_worker::Reply<Checkpoint, u64>, now: Instant) -> Result<(), String> {
        let id = match reply.result {
            Ok(id) => id,
            Err(error) => {
                self.compaction_status = Some(format!("Compaction failed; original memory unchanged: {error}"));
                return Err(error);
            }
        };
        if !super::compaction::same_context(&self.snapshot, &reply.job.expected) {
            self.compaction_status = Some(format!("Memory changed while compacting; summary discarded. Original retained as {id}."));
            return Ok(());
        }
        // Persistence acknowledgements and session changes remain owned by the live runtime.
        let mut next = self.snapshot.clone();
        next.decisions = reply.job.next.decisions;
        next.commands = reply.job.next.commands;
        next.recent_context = reply.job.next.recent_context;
        next.context_version = next.context_version.checked_add(1).ok_or("Assistant context version exhausted")?;
        self.update(next, now)?;
        self.compaction_status = Some(format!("Working memory condensed; original retained as {id}."));
        Ok(())
    }
    pub(super) fn memory_originals(&self) -> Result<Vec<super::compaction_archive::Entry>, String> {
        // Create only the application's own project storage, using its normal lock protocol.
        let _lock = disk::ScopeLock::take(&self.dir)?;
        super::compaction_archive::list(&self.dir)
    }
    pub(super) fn restore_original(&mut self, id: u64, now: Instant) -> Result<(), String> {
        if self.compaction_pending() {
            return Err("Wait for pending compaction before restoring memory".into());
        }
        let _lock = disk::ScopeLock::take(&self.dir)?;
        let original = super::compaction_archive::load(&self.dir, id, &self.snapshot.project_scope)?;
        if original.target != super::compaction_archive::Target::AssistantContext {
            return Err("This original contains project notes, not assistant context".into());
        }
        let restored: Checkpoint = serde_json::from_str(&original.text).map_err(|error| format!("Invalid retained context: {error}"))?;
        restored.validate(&self.snapshot.project_scope)?;
        // Restoring old working notes must never undo subsequent teaching, reference selection,
        // session binding, pending tasks or native assistant-review decisions.
        let mut next = self.snapshot.clone();
        next.decisions = restored.decisions;
        next.commands = restored.commands;
        next.recent_context = restored.recent_context;
        next.context_version = next.context_version.checked_add(1).ok_or("Assistant context version exhausted")?;
        next.encode()?;
        super::compaction_archive::retain(
            &self.dir,
            &super::compaction_archive::Original {
                scope: self.snapshot.project_scope.clone(),
                target: super::compaction_archive::Target::AssistantContext,
                document_epoch: self.snapshot.document_epoch,
                version: self.snapshot.context_version,
                text: String::from_utf8(self.snapshot.encode()?).map_err(|error| error.to_string())?,
            },
        )?;
        self.update(next, now)?;
        self.compaction_status = Some("Original working notes restored; current teaching and unfinished tasks preserved".into());
        Ok(())
    }
    pub fn snapshot(&self) -> &Checkpoint {
        &self.snapshot
    }
    /// Keep persistence metadata owned by this runtime, not by model-supplied content.
    pub fn update(&mut self, mut next: Checkpoint, now: Instant) -> Result<bool, String> {
        next.validate(&self.snapshot.project_scope)?;
        next.generation = self.snapshot.generation;
        next.saved_at = self.snapshot.saved_at.clone();
        // Reject an unsavable combination before replacing the last valid in-memory snapshot.
        next.encode()?;
        if next == self.snapshot {
            return Ok(false);
        }
        self.tracker.changed(now)?;
        self.snapshot = next;
        Ok(true)
    }
    pub fn tick(&mut self, now: Instant, milestone_or_close: bool, saved_at: &str) -> Result<(), String> {
        if let Some(reply) = self.worker.poll() {
            self.complete(reply, now)?;
        }
        if !self.worker.pending()
            && let Some(job) = self.waiting_compaction.take()
        {
            self.compaction.as_mut().ok_or("Memory compaction worker unavailable")?.submit(job)?;
        }
        self.poll_compaction(now)?;
        if !self.compaction_pending() && self.tracker.due(now, milestone_or_close) {
            let mut next = self.snapshot.clone();
            next.generation = self.tracker.next_generation()?;
            next.saved_at = saved_at.to_owned();
            let ticket = self.worker.submit(self.dir.clone(), next)?;
            self.tracker.queued(ticket, self.tracker.next_generation()?)?;
            self.pending_timestamp = Some(saved_at.to_owned());
        }
        Ok(())
    }
    fn complete(&mut self, reply: crate::console::assistants::worker::Reply, now: Instant) -> Result<(), String> {
        self.tracker.completed(reply.ticket, now, reply.result)?;
        self.snapshot.generation = self.tracker.generation;
        if let Some(saved_at) = self.pending_timestamp.take() {
            self.snapshot.saved_at = saved_at;
        }
        self.recovered_previous = false;
        self.tracker.snapshot_changed()?;
        Ok(())
    }
    pub fn flush(&mut self, now: Instant, saved_at: &str) -> Result<(), String> {
        // Closing must not lose a newer dirty checkpoint behind an archive lock.
        // Unstarted summaries can be abandoned safely; an active archive gets a
        // bounded wait before the normal checkpoint is flushed.
        self.waiting_compaction = None;
        if let Some(worker) = self.compaction.as_mut()
            && worker.pending()
            && let Some(reply) = worker.finish(Duration::from_secs(2))?
        {
            self.adopt_compaction(reply, now)?;
        }
        let autosave_result = super::autosave::flush(self, now, saved_at);
        // At most one outgoing write plus the latest coalesced snapshot. Never wait unboundedly.
        let deadline = Instant::now();
        for _ in 0..2 {
            self.tick(now, true, saved_at)?;
            let left = Duration::from_secs(5).saturating_sub(deadline.elapsed());
            let Some(reply) = self.worker.finish(left)? else {
                return autosave_result;
            };
            self.complete(reply, now)?;
        }
        if self.tracker.due(now, true) {
            return Err("Assistant context changed during close; latest context remains unsaved".into());
        }
        autosave_result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn initial() -> Checkpoint {
        Checkpoint {
            schema: 1,
            project_scope: "A".into(),
            document_epoch: 1,
            revision: 0,
            generation: 0,
            context_version: 0,
            saved_at: String::new(),
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
    fn large_context() -> Checkpoint {
        let mut current = initial();
        current.assistant.role = "Band synchronization assistant".into();
        current.assistant.workflow = vec!["Preserve full song; synchronize selected performance clips".into()];
        current.pending = vec!["Review chorus with owner".into()];
        current.errors = vec!["One source is missing".into()];
        current.recent_context = (0..24).map(|n| ("Assistant".into(), format!("Completed inspection {n}. {}", "Historical detail. ".repeat(180)))).collect();
        current
    }
    fn finish_compaction(runtime: &mut Runtime) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(2);
        while runtime.compaction_pending() {
            runtime.tick(Instant::now(), false, "2026-11-05T12:00:00Z")?;
            if Instant::now() >= deadline {
                return Err("Test compaction exceeded its deadline".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    }
    #[test]
    fn checkpoint_write_and_compaction_archive_are_serialized_and_close_keeps_new_notes() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            mpsc,
        };
        let dir = tempfile::tempdir().unwrap();
        let mut runtime = Runtime::open(dir.path().to_owned(), large_context()).unwrap();
        let (go, wait) = mpsc::sync_channel(1);
        let first = AtomicBool::new(true);
        runtime.worker = Worker::start_with(move |dir, next| {
            if first.swap(false, Ordering::SeqCst) {
                wait.recv().map_err(|error| error.to_string())?;
            }
            disk::save(dir, next)
        })
        .unwrap();
        let mut source = runtime.snapshot().clone();
        source.pending.push("New note must survive clean close".into());
        runtime.update(source, Instant::now()).unwrap();
        runtime.tick(Instant::now(), true, "2026-11-05T12:00:00Z").unwrap();
        assert!(runtime.worker.pending());
        runtime.compact(br#"{"summary":"Completed inspections; chorus awaits review.","decisions":[]}"#).unwrap();
        runtime.tick(Instant::now(), true, "2026-11-05T12:00:00Z").unwrap();
        assert!(runtime.waiting_compaction.is_some());
        assert!(!runtime.compaction.as_ref().unwrap().pending());
        go.send(()).unwrap();
        finish_compaction(&mut runtime).unwrap();
        assert!(runtime.compaction_status.as_deref().unwrap().contains("condensed"));
        runtime.flush(Instant::now(), "2026-11-05T12:01:00Z").unwrap();
        let restored = Runtime::open(dir.path().to_owned(), large_context()).unwrap();
        assert!(restored.snapshot().pending.contains(&"New note must survive clean close".into()));
        assert!(restored.snapshot().recent_context.iter().any(|(role, _)| role == super::super::compaction::SUMMARY_ROLE));
    }
    #[test]
    fn close_abandons_unstarted_compaction_but_saves_the_exact_dirty_context() {
        let dir = tempfile::tempdir().unwrap();
        let mut runtime = Runtime::open(dir.path().to_owned(), large_context()).unwrap();
        let mut next = runtime.snapshot().clone();
        next.pending.push("Persist latest owner request".into());
        runtime.update(next.clone(), Instant::now()).unwrap();
        runtime.compact(br#"{"summary":"Completed inspections.","decisions":[]}"#).unwrap();
        runtime.flush(Instant::now(), "2026-11-05T12:00:00Z").unwrap();
        let restored = Runtime::open(dir.path().to_owned(), large_context()).unwrap();
        assert_eq!(restored.snapshot().pending, next.pending);
        assert_eq!(restored.snapshot().recent_context, next.recent_context);
    }
    #[test]
    fn compaction_retains_original_protects_teaching_and_restores_without_replaying_edits() {
        let dir = tempfile::tempdir().unwrap();
        let before = large_context();
        let mut runtime = Runtime::open(dir.path().to_owned(), before.clone()).unwrap();
        assert!(super::super::compaction::due(&before));
        runtime.compact(br#"{"summary":"Inspected the performance clips. Await chorus review.","decisions":[]}"#).unwrap();
        assert_eq!(runtime.snapshot(), &before);
        finish_compaction(&mut runtime).unwrap();
        assert_eq!(runtime.snapshot().assistant, before.assistant);
        assert_eq!(runtime.snapshot().pending, before.pending);
        assert_eq!(runtime.snapshot().errors, before.errors);
        assert!(runtime.snapshot().encode().unwrap().len() < before.encode().unwrap().len());
        let originals = runtime.memory_originals().unwrap();
        assert_eq!(originals.len(), 1);
        let mut newer = runtime.snapshot().clone();
        newer.assistant.preferences.push("Keep the audience cutaway".into());
        newer.pending.push("Check the outro".into());
        runtime.update(newer, Instant::now()).unwrap();
        runtime.restore_original(originals[0].id, Instant::now()).unwrap();
        assert_eq!(runtime.snapshot().recent_context, before.recent_context);
        assert_eq!(runtime.snapshot().assistant.preferences, ["Keep the audience cutaway"]);
        assert!(runtime.snapshot().pending.contains(&"Check the outro".into()));
        assert_eq!(runtime.memory_originals().unwrap().len(), 2);
    }
    #[test]
    fn concurrent_context_changes_and_archive_failure_never_adopt_a_summary() {
        let dir = tempfile::tempdir().unwrap();
        let mut runtime = Runtime::open(dir.path().to_owned(), large_context()).unwrap();
        let summary = br#"{"summary":"Completed inspections.","decisions":[]}"#;
        runtime.compact(summary).unwrap();
        let mut changed = runtime.snapshot().clone();
        changed.pending.push("New user request".into());
        runtime.update(changed.clone(), Instant::now()).unwrap();
        finish_compaction(&mut runtime).unwrap();
        assert_eq!(runtime.snapshot(), &changed);
        assert!(runtime.compaction_status.as_deref().unwrap().contains("discarded"));
        let failure_dir = tempfile::tempdir().unwrap();
        std::fs::write(failure_dir.path().join("MemoryHistory"), b"not a directory").unwrap();
        let mut failed = Runtime::open(failure_dir.path().to_owned(), large_context()).unwrap();
        let before = failed.snapshot().clone();
        failed.compact(summary).unwrap();
        assert!(finish_compaction(&mut failed).is_err());
        assert_eq!(failed.snapshot(), &before);
    }
    #[test]
    fn oversized_combined_context_is_rejected_before_replacing_the_last_valid_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let mut runtime = Runtime::open(dir.path().to_owned(), initial()).unwrap();
        let before = runtime.snapshot().clone();
        let mut oversized = before.clone();
        oversized.decisions = vec!["x".repeat(4096); 128];
        oversized.errors = vec!["y".repeat(4096); 128];
        assert!(oversized.validate("A").is_ok());
        assert!(runtime.update(oversized, Instant::now()).is_err());
        assert_eq!(runtime.snapshot(), &before);
    }
    #[test]
    fn new_context_during_async_save_survives_clean_close_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let now = Instant::now();
        let mut runtime = Runtime::open(dir.path().to_owned(), initial()).unwrap();
        assert!(!runtime.needs_polling());
        let mut next = initial();
        next.decisions.push("Use long takes".into());
        runtime.update(next.clone(), now).unwrap();
        assert!(runtime.needs_polling());
        runtime.tick(now, true, "2026-11-01T12:00:00Z").unwrap();
        next.pending.push("Inspect the chorus".into());
        runtime.update(next, now).unwrap();
        runtime.flush(now, "2026-11-01T12:01:00Z").unwrap();
        assert_eq!(runtime.snapshot().saved_at, "2026-11-01T12:01:00Z");
        assert!(!runtime.needs_polling());
        let reopened = Runtime::open(dir.path().to_owned(), initial()).unwrap();
        assert_eq!(reopened.snapshot().pending, ["Inspect the chorus"]);
        assert_eq!(reopened.snapshot().decisions, ["Use long takes"]);
        assert_eq!(reopened.snapshot().generation, 2);
    }
    #[test]
    fn failed_save_keeps_timestamp_unsaved_and_clean_close_can_retry() {
        let dir = tempfile::tempdir().unwrap();
        let now = Instant::now();
        let mut runtime = Runtime::open(dir.path().to_owned(), initial()).unwrap();
        let mut next = initial();
        next.decisions.push("Keep the first take".into());
        runtime.update(next, now).unwrap();
        let held = disk::ScopeLock::take(dir.path()).unwrap();
        assert!(runtime.flush(now, "2026-11-01T12:00:00Z").is_err());
        assert_eq!(runtime.snapshot().saved_at, "");
        assert_eq!(runtime.snapshot().generation, 0);
        drop(held);
        runtime.flush(now, "2026-11-01T12:01:00Z").unwrap();
        assert_eq!(runtime.snapshot().saved_at, "2026-11-01T12:01:00Z");
        assert_eq!(runtime.snapshot().generation, 1);
    }

    #[test]
    fn unchanged_polling_creates_no_files_and_another_scope_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let now = Instant::now();
        let mut runtime = Runtime::open(dir.path().to_owned(), initial()).unwrap();
        runtime.tick(now + crate::console::assistants::CHECKPOINT_INTERVAL, false, "2026-11-01T12:00:00Z").unwrap();
        assert!(!dir.path().join(crate::console::assistants::store::CURRENT).exists());
        let mut wrong = initial();
        wrong.project_scope = "B".into();
        assert!(runtime.update(wrong, now).is_err());
        assert!(disk::load(dir.path(), "A").unwrap().is_none());
    }
}
