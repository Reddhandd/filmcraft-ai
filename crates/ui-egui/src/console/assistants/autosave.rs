//! Named instructions only; no transcript export, model calls or command replay.
use super::{
    Instructions, Runtime, catalog_store,
    catalog_worker::{Job, Reply, Worker},
    schedule::Schedule,
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
pub(super) struct State {
    worker: Worker<Instructions>,
    schedule: Schedule,
    dir: PathBuf,
    pub error: Option<String>,
}
impl State {
    fn new(dir: PathBuf) -> Result<Self, String> {
        Ok(Self {
            worker: Worker::start(|job: &Job<Instructions>| catalog_store::save(&job.dir, &job.name, job.next.clone(), Some(&job.expected)).map(|_| ()))?,
            schedule: Schedule::default(),
            dir,
            error: None,
        })
    }
    pub fn needs_polling(&self) -> bool {
        self.worker.pending() || (self.error.is_none() && self.schedule.has_changes())
    }
    pub fn pending(&self) -> bool {
        self.worker.pending()
    }
}
fn accept(runtime: &mut Runtime, state: &mut State, reply: Reply<Instructions>, now: Instant, saved_at: &str) -> Result<(), String> {
    state.schedule.attempted(now, reply.result.is_ok());
    if let Err(error) = reply.result {
        state.error = Some(format!("Assistant autosave failed: {error}. Review the saved assistant, then re-enable Autosave or update it manually."));
        return Ok(());
    }
    let mut snapshot = runtime.snapshot().clone();
    if snapshot.project_scope != reply.job.scope || snapshot.assistant_name.as_deref() != Some(reply.job.name.as_str()) {
        return Err("Assistant autosave acknowledgement belongs to another project or assistant".into());
    }
    let merged = super::preservation::merge(&reply.job.next, &snapshot.assistant, true)?;
    if snapshot.assistant != merged {
        snapshot.assistant = merged;
        snapshot.context_version = snapshot.context_version.checked_add(1).ok_or("Assistant context version exhausted")?;
    }
    if let Some(settings) = &mut snapshot.autosave {
        if settings.expected != reply.job.expected {
            return Err("Assistant autosave baseline changed during a pending save; review the catalog".into());
        }
        settings.expected = reply.job.next;
        settings.saved_at = saved_at.into();
        runtime.update(snapshot, now)?;
    }
    Ok(())
}
pub(super) fn tick(runtime: &mut Runtime, dir: PathBuf, now: Instant, force: bool, saved_at: &str) -> Result<(), String> {
    if runtime.snapshot().autosave.is_none() && runtime.autosave_state.is_none() {
        return Ok(());
    }
    let mut state = match runtime.autosave_state.take() {
        Some(state) => state,
        None => State::new(dir.clone())?,
    };
    let result = (|| {
        if state.dir != dir {
            return Err("Assistant catalog directory changed; finish its pending save before switching profiles".into());
        }
        if let Some(reply) = state.worker.poll() {
            accept(runtime, &mut state, reply, now, saved_at)?;
        }
        if state.worker.pending() {
            return Ok(());
        }
        let Some(settings) = runtime.snapshot().autosave.as_ref() else {
            return Ok(());
        };
        let candidate = super::preservation::merge(&settings.expected, &runtime.snapshot().assistant, true)?;
        if settings.expected != candidate {
            state.schedule.changed(now);
        } else {
            state.schedule = Schedule::default();
        }
        if state.error.is_none() && settings.expected != candidate && state.schedule.due(now, force) {
            let snapshot = runtime.snapshot();
            state.worker.submit(Job {
                scope: snapshot.project_scope.clone(),
                name: snapshot.assistant_name.clone().ok_or("Choose a named assistant before enabling autosave")?,
                dir: dir.clone(),
                expected: settings.expected.clone(),
                next: candidate,
            })?;
        }
        Ok(())
    })();
    runtime.autosave_state = Some(state);
    result
}
pub(super) fn flush(runtime: &mut Runtime, now: Instant, saved_at: &str) -> Result<(), String> {
    let Some(dir) = runtime.autosave_state.as_ref().map(|state| state.dir.clone()) else {
        return Ok(());
    };
    for _ in 0..2 {
        tick(runtime, dir.clone(), now, true, saved_at)?;
        let Some(mut state) = runtime.autosave_state.take() else {
            return Ok(());
        };
        let result = state.worker.finish(Duration::from_secs(5)).and_then(|reply| {
            if let Some(reply) = reply {
                accept(runtime, &mut state, reply, now, saved_at)?;
            }
            if let Some(error) = &state.error {
                return Err(error.clone());
            }
            Ok(())
        });
        runtime.autosave_state = Some(state);
        result?;
    }
    if let Some(settings) = runtime.snapshot().autosave.as_ref()
        && super::preservation::merge(&settings.expected, &runtime.snapshot().assistant, true)? != settings.expected
    {
        return Err("Named assistant instructions remain unsaved; update or disable Autosave before closing".into());
    }
    Ok(())
}
