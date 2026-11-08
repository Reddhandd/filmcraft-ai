//! Idle maintenance uses the existing signed-in Codex connection, never a new paid service.
use super::*;
use std::time::{Duration, Instant};
const INTERVAL: Duration = Duration::from_secs(15 * 60);
const TIMEOUT: Duration = Duration::from_secs(120);
#[derive(Default)]
pub(super) struct State {
    last_attempt: Option<Instant>,
    active: Option<Active>,
}
struct Active {
    scope: String,
    epoch: u64,
    started: Instant,
    cancel_requested: bool,
}
impl State {
    pub(super) fn active(&self) -> bool {
        self.active.is_some()
    }
    pub(super) fn eligible(&self, now: Instant) -> bool {
        self.active.is_none() && self.last_attempt.is_none_or(|last| now.saturating_duration_since(last) >= INTERVAL)
    }
}
/// Read-only readiness flags for native acceptance and connection diagnostics.
/// Never expose draft contents, credentials or provider message payloads.
pub(super) fn diagnostics(app: &FilmcraftApp, now: Instant) -> serde_json::Value {
    serde_json::json!({
        "active":app.assistants.maintenance.active(),
        "cooldownSeconds":app.assistants.maintenance.last_attempt.map(|last| INTERVAL.saturating_sub(now.saturating_duration_since(last)).as_secs()).unwrap_or(0),
        "chatBusy":app.ui.console.chat.busy,
        "voiceOn":app.ui.console.chat.voice.on(),
        "draftPresent":!app.ui.console.chat.prompt.trim().is_empty(),
        "pendingEdit":app.ui.console.chat.pending.is_some(),
        "connectionReady":app.ui.console.connection==super::super::ConnectionStatus::Ready,
        "contextRefreshPending":super::super::sessions::context_pending(app),
        "unsavedMemoryDraft":super::super::sessions::has_unsaved_memory(app),
        "conversationPresent":app.ui.console.sessions.active.is_some(),
        "supportedProvider":app.hooks.codex_connection.as_ref().is_some_and(|connection|connection.provider_id()==super::super::provider::ProviderId::Codex),
        "compactionPending":app.assistants.current.as_ref().is_some_and(|runtime|runtime.compaction_pending()),
        "workingMemoryDue":app.assistants.current.as_ref().is_some_and(|runtime|super::compaction::due(runtime.snapshot()))
    })
}
pub(super) fn active(app: &FilmcraftApp) -> bool {
    app.assistants.maintenance.active()
}
/// The editor's own controls remain available; maintenance cannot run project commands.
pub(super) fn guard(app: &FilmcraftApp) -> Result<(), String> {
    if active(app) {
        return Err("Working-memory maintenance cannot edit the project. Finish or cancel maintenance before requesting edits".into());
    }
    Ok(())
}
pub(super) fn poll(app: &mut FilmcraftApp, now: Instant) -> Result<(), String> {
    if let Some(active) = &mut app.assistants.maintenance.active {
        let same = app
            .assistants
            .current
            .as_ref()
            .is_some_and(|runtime| runtime.snapshot().project_scope == active.scope && runtime.snapshot().document_epoch == active.epoch);
        let pending = app.assistants.current.as_ref().is_some_and(|runtime| runtime.compaction_pending());
        if !app.ui.console.chat.busy && !pending {
            if let Some(runtime) = app.assistants.current.as_mut()
                && runtime.compaction_status.is_none()
                && super::compaction::due(runtime.snapshot())
            {
                runtime.compaction_status = Some("Automatic compaction finished without a usable summary; original memory is unchanged. A changed snapshot can be retried after the maintenance cooldown.".into());
            }
            app.assistants.maintenance.active = None;
            changed(app);
            return Ok(());
        }
        if (!same || now.saturating_duration_since(active.started) >= TIMEOUT) && !active.cancel_requested {
            // Keep the edit guard until the provider acknowledges termination. A delayed
            // response must never gain project-write permission after cancellation.
            active.cancel_requested = true;
            app.ui.status = "Memory maintenance timed out or its project changed; cancelling. Original memory is preserved.".into();
            if let Some(connection) = app.hooks.codex_connection.as_mut()
                && let Err(error) = connection.cancel()
            {
                app.ui.status = format!("Memory maintenance could not cancel: {error}. Use Cancel or disconnect; original memory remains available.");
            }
        }
        return Ok(());
    }
    if !app.assistants.maintenance.eligible(now)
        || app.ui.console.chat.busy
        || app.ui.console.chat.voice.on()
        || !app.ui.console.chat.prompt.trim().is_empty()
        || app.ui.console.chat.pending.is_some()
        || app.ui.console.connection != super::super::ConnectionStatus::Ready
        || super::super::sessions::context_pending(app)
        || super::super::sessions::has_unsaved_memory(app)
        || app.ui.console.sessions.active.is_none()
    {
        return Ok(());
    }
    let Some(connection) = app.hooks.codex_connection.as_ref() else { return Ok(()) };
    // Desktop is externally driven. Its next tool read receives compactionRecommended;
    // FilmCraft must not commandeer that app's conversation or start an API connection.
    if connection.provider_id() != super::super::provider::ProviderId::Codex {
        return Ok(());
    }
    let Some(runtime) = app.assistants.current.as_ref() else { return Ok(()) };
    if runtime.compaction_pending() || !super::compaction::due(runtime.snapshot()) {
        return Ok(());
    }
    let current = runtime.snapshot();
    let scope = current.project_scope.clone();
    let epoch = current.document_epoch;
    let prompt = serde_json::json!({
        "filmcraftMaintenance":"Condense historical working notes only. Do not edit or inspect project media, use attachments, change teaching, or change unfinished tasks. Raw project notes and standing teaching are protected and must not be compressed.",
        "scope":current.project_scope,"document_epoch":current.document_epoch,"initial_context_generation":current.context_version,
        "procedure":"Call console_memory assistant_history at offset 0 WITHOUT memory_generation. Its memoryGeneration guards history pagination only; use that returned value for following pages. Its contextGeneration guards semantic updates. After reading all needed pages, call assistant_compact with memory_generation=contextGeneration and history_generation=memoryGeneration from that same history snapshot, plus scope and document_epoch. Supply JSON text containing only summary (at most8192 bytes) and decisions (at most16 strings). Preserve useful facts, decisions and uncertainties. Read assistant_read for compactionStatus, then finish. Do not replay historical commands. If any guard changes, or compaction cannot reduce memory safely, finish without changing anything."
    }).to_string();
    app.assistants.maintenance.last_attempt = Some(now);
    let sent = app
        .hooks
        .codex_connection
        .as_mut()
        .ok_or("Codex unavailable for memory maintenance")?
        .send_prompt_with_model(prompt, app.ui.console.chat.selected_model.clone());
    if let Err(error) = sent {
        if let Some(runtime) = app.assistants.current.as_mut() {
            runtime.compaction_status = Some(format!("Automatic compaction deferred: {error}. Original memory is unchanged."));
        }
        return Ok(());
    }
    app.assistants.maintenance.active = Some(Active { scope, epoch, started: now, cancel_requested: false });
    app.ui.console.chat.busy = true;
    app.ui.status = "Automatically condensing working notes with connected Codex. Teaching and project notes are protected; Cancel stops maintenance.".into();
    Ok(())
}
/// Keep the existing summary and append only newly displayed completed dialogue.
/// The display is a bounded window; longest suffix/prefix overlap avoids reintroducing
/// the old conversation immediately after compaction. Historical dialogue is always partial.
pub(super) fn capture(current: &[(String, String)], before: &[(String, String)], after: &[(String, String)]) -> Vec<(String, String)> {
    let Some(summary) = current.iter().find(|(role, _)| role == compaction::SUMMARY_ROLE).cloned() else { return after.to_vec() };
    let overlap = (0..=before.len().min(after.len())).rev().find(|count| before.get(before.len().saturating_sub(*count)..) == after.get(..*count)).unwrap_or(0);
    let mut recent: Vec<_> = current.iter().filter(|(role, _)| role != compaction::SUMMARY_ROLE).cloned().collect();
    recent.extend(after.iter().skip(overlap).cloned());
    std::iter::once(summary).chain(recent.into_iter().rev().take(31).collect::<Vec<_>>().into_iter().rev()).collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    fn entry(text: &str) -> (String, String) {
        ("User".into(), text.into())
    }
    #[test]
    fn cooldown_and_inflight_work_prevent_maintenance_loops() {
        let now = Instant::now();
        let mut state = State::default();
        assert!(state.eligible(now));
        state.last_attempt = Some(now);
        assert!(!state.eligible(now + Duration::from_secs(5)));
        assert!(state.eligible(now + INTERVAL));
        state.active = Some(Active { scope: "A".into(), epoch: 1, started: now, cancel_requested: false });
        assert!(!state.eligible(now + INTERVAL));
    }
    #[test]
    fn compacted_dialogue_is_not_reexpanded_from_the_display_window() {
        let before = vec![entry("old one"), entry("old two"), entry("recent")];
        let current = vec![(compaction::SUMMARY_ROLE.into(), "Earlier discussion condensed".into()), entry("recent")];
        assert_eq!(capture(&current, &before, &before), current);
        let after = vec![entry("old two"), entry("recent"), entry("new request")];
        let result = capture(&current, &before, &after);
        assert_eq!(result.len(), 3);
        assert_eq!(result.last(), Some(&entry("new request")));
        assert!(!result.contains(&entry("old one")));
        assert!(!result.contains(&entry("old two")));
    }
}
