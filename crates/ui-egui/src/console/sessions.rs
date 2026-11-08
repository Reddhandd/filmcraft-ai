//! Project memory and a durable local conversation archive, including unsaved projects.
//! Only recent display messages are cached here; Codex owns the complete thread history.
use super::*;
use serde::{Deserialize, Serialize};
use std::{io::Read, path::PathBuf};

mod memory;
mod memory_text;
#[cfg(test)]
use memory::memory_reply;
use memory::save_memory_with;
pub(super) use memory::{inline_saved_memory, memory, memory_metadata};
pub const MAX_MEMORY: usize = memory_text::MAX_SAVED_BYTES;
pub const MAX_CONTEXT_MEMORY: usize = memory_text::MAX_CONTEXT_BYTES;
const MAX_INDEX: usize = 8 * 1024 * 1024;
const MAX_SESSIONS: usize = 32;
const TEMPLATE: &str = "# FilmCraft project memory\n\n## Editing preferences\n\n## Decisions\n\n## Current goal\n\n## Where we stopped\n";
#[derive(Clone, Debug, Default)]
pub struct SessionRequest {
    pub project: String,
    pub resume: Option<String>,
    pub memory: String,
    pub model: Option<String>,
    /// Recent user/assistant dialogue for voice startup, never new instructions to execute.
    pub recent_dialogue: Vec<(String, String)>,
    /// Compact selected-assistant identity; separate from a potentially large notes excerpt.
    pub assistant_summary: String,
    /// Only paths verified as the same open document by the host (Save/Save As).
    pub scope_aliases: Vec<String>,
}
impl SessionRequest {
    pub fn validate(&self) -> Result<(), String> {
        if self.project.is_empty()
            || self.project.len() > 4096
            || self.memory.len() > MAX_CONTEXT_MEMORY + 16 * 1024
            || self.recent_dialogue.len() > 32
            || self.assistant_summary.len() > 1536
            || self.recent_dialogue.iter().any(|(role, text)| !matches!(role.as_str(), "user" | "assistant") || text.len() > 64 * 1024 + 128)
            || self.model.as_ref().is_some_and(|m| m.is_empty() || m.len() > 256)
            || self.scope_aliases.len() > 16
            || self.scope_aliases.iter().any(|scope| scope.is_empty() || scope.len() > 4096)
            || self.resume.as_deref().is_some_and(|id| !valid_id(id))
        {
            return Err("Invalid project memory or conversation identifier".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedSession {
    pub thread_id: String,
    pub title: String,
    pub updated: String,
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scope_aliases: Vec<String>,
    pub messages: VecDeque<(String, String)>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Index {
    version: u32,
    project: String,
    sessions: Vec<SavedSession>,
}
#[derive(Clone, Debug, Default)]
pub struct Sessions {
    pub project: Option<String>,
    provider: super::provider::ProviderId,
    epoch: u64,
    document_path: Option<String>,
    history_scope: Option<String>,
    context_pending: bool,
    pending_scope_aliases: Vec<String>,
    pub memory: String,
    saved_memory: String,
    memory_generation: u64,
    index_disk: Option<String>,
    pub records: Vec<SavedSession>,
    pub active: Option<String>,
    pub error: Option<String>,
    pub memory_open: bool,
    pub picker_open: bool,
}
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
pub(super) fn folder(project: &str) -> PathBuf {
    let mut path = std::ffi::OsString::from(project);
    path.push(".console");
    PathBuf::from(path)
}
fn read(path: &std::path::Path, limit: usize) -> Result<Option<String>, String> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Err(format!("Console storage must be a regular file: {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("Cannot inspect console file {}: {e}", path.display())),
    }
    // A regular-file check can race an external replacement. Nonblocking open on
    // Unix ensures a replacement FIFO cannot wait forever for a writer on the UI.
    #[cfg(unix)]
    let file = std::fs::File::from(
        rustix::fs::open(path, rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::CLOEXEC, rustix::fs::Mode::empty())
            .map_err(|e| format!("Cannot open console file {}: {e}", path.display()))?,
    );
    #[cfg(not(unix))]
    let file = std::fs::File::open(path).map_err(|e| format!("Cannot open console file {}: {e}", path.display()))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err(format!("Console storage must be a regular file: {}", path.display()));
    }
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1) as u64).read_to_end(&mut bytes).map_err(|e| format!("Cannot read console file: {e}"))?;
    if bytes.len() > limit {
        return Err(format!("Console file exceeds its {limit}-byte limit: {}", path.display()));
    }
    String::from_utf8(bytes).map(Some).map_err(|_| "Console files must contain valid UTF-8".into())
}
struct Lock(PathBuf);
impl Lock {
    fn take(dir: &std::path::Path) -> Result<Self, String> {
        let path = dir.join("write.lock");
        std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| {
            format!(
                "Console files are locked or not writable ({}): {e}. If no other FilmCraft instance is using this project, remove its stale write.lock.",
                path.display()
            )
        })?;
        Ok(Self(path))
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
impl Sessions {
    #[cfg(test)]
    fn load(project: String) -> Result<Self, String> {
        Self::load_for(project, super::provider::ProviderId::Codex)
    }
    fn load_for(project: String, provider: super::provider::ProviderId) -> Result<Self, String> {
        if project.is_empty() || project.len() > 4096 {
            return Err("Save this project to a valid path before using project memory".into());
        }
        let dir = folder(&project);
        let memory = read(&dir.join("memory.md"), MAX_MEMORY)?.unwrap_or_else(|| TEMPLATE.into());
        let index_disk = read(&dir.join(index_name(provider)), MAX_INDEX)?;
        let records = if let Some(text) = &index_disk {
            let index: Index = serde_json::from_str(text).map_err(|e| format!("Cannot load saved console sessions: {e}"))?;
            if index.version != 1 || index.project != project || index.sessions.len() > MAX_SESSIONS {
                return Err("Saved console session index has an unsupported version, project or size".into());
            }
            let mut seen = std::collections::HashSet::new();
            for item in &index.sessions {
                if !valid_id(&item.thread_id)
                    || !seen.insert(&item.thread_id)
                    || item.title.len() > 256
                    || item.updated.len() > 64
                    || item.model.as_ref().is_some_and(|m| m.len() > 256)
                    || item.project.as_ref().is_some_and(|p| p.is_empty() || p.len() > 4096)
                    || item.scope_aliases.len() > 16
                    || item.scope_aliases.iter().any(|scope| scope.is_empty() || scope.len() > 4096)
                    || item.messages.len() > 32
                    || item.messages.iter().any(|(r, t)| r.len() > 64 || t.len() > 64 * 1024 + 128)
                {
                    return Err("Saved console session contains invalid or oversized history".into());
                }
            }
            index.sessions
        } else {
            Vec::new()
        };
        Ok(Self { provider, project: Some(project), saved_memory: memory.clone(), memory, index_disk, records, ..Default::default() })
    }
    fn write_index(&mut self) -> Result<(), String> {
        let Some(project) = self.history_scope.as_ref().or(self.project.as_ref()) else { return Ok(()) };
        let dir = folder(project);
        std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create console folder: {e}"))?;
        let _lock = Lock::take(&dir)?;
        if read(&dir.join(index_name(self.provider)), MAX_INDEX)? != self.index_disk {
            return Err("Saved conversations changed on disk; reload the project before continuing".into());
        }
        let data = serde_json::to_vec(&Index { version: 1, project: project.clone(), sessions: self.records.clone() }).map_err(|e| e.to_string())?;
        if data.len() > MAX_INDEX {
            return Err("Saved console histories exceed 8 MiB; move older entries out of sessions.json before continuing".into());
        }
        filmcraft_format::atomic::atomic_write(&dir.join(index_name(self.provider)), &data).map_err(|e| format!("Cannot save console history: {e}"))?;
        self.index_disk = Some(String::from_utf8(data).map_err(|e| e.to_string())?);
        Ok(())
    }
    fn request(&mut self, resume: Option<String>) -> Result<SessionRequest, String> {
        self.request_for_lock(resume, false)
    }
    fn request_for_lock(&mut self, resume: Option<String>, locked: bool) -> Result<SessionRequest, String> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let project = self.project.clone().ok_or("Local conversation storage is unavailable")?;
        if resume.is_none() && self.records.len() >= MAX_SESSIONS {
            return Err("The local conversation archive is full; preserve older entries before starting another conversation".into());
        }
        if self.memory != self.saved_memory {
            return Err("Save your memory changes before starting or resuming a conversation".into());
        }
        if !locked && resume.as_ref().is_some_and(|id| !self.records.iter().any(|r| &r.thread_id == id)) {
            return Err("Choose a conversation belonging to this project".into());
        }
        if resume.is_none() && self.records.len() >= MAX_SESSIONS {
            return Err("This project's 32 saved conversation slots are full; move older entries out of sessions.json first".into());
        }
        if resume.is_none() {
            self.pending_scope_aliases = vec![project.clone()];
        }
        let dir = folder(&project);
        std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create console folder: {e}"))?;
        let path = dir.join("memory.md");
        let memory = match read(&path, MAX_MEMORY)? {
            Some(memory) => memory,
            None => {
                filmcraft_format::atomic::atomic_write_new(&path, self.memory.as_bytes()).map_err(|e| format!("Cannot create project memory: {e}"))?;
                self.memory.clone()
            }
        };
        if memory != self.saved_memory {
            self.memory_generation = self.memory_generation.checked_add(1).ok_or("Memory generation exhausted; reopen the project")?;
        }
        self.memory = memory.clone();
        self.saved_memory = memory.clone();
        let model = resume.as_ref().and_then(|id| self.records.iter().find(|r| &r.thread_id == id)).and_then(|r| r.model.clone());
        let selected_style = super::references::selected_for_scope(&project)?;
        let memory = memory_text::context_excerpt(&memory)?;
        let memory = format!(
            "{memory}\n\nCurrent selected creative defaults (reference preferences only; explicit user requests and project decisions take precedence): {}",
            serde_json::to_string(&selected_style).map_err(|e| e.to_string())?
        );
        let scope_aliases = resume
            .as_ref()
            .and_then(|id| self.records.iter().find(|record| &record.thread_id == id))
            .filter(|record| record.scope_aliases.contains(&project))
            .map(|record| record.scope_aliases.clone())
            .unwrap_or_default();
        let recent_dialogue = resume
            .as_ref()
            .and_then(|id| self.records.iter().find(|record| &record.thread_id == id))
            .map(|record| {
                record
                    .messages
                    .iter()
                    .filter_map(|(role, text)| {
                        let role = if role == "You" || role == "You (voice)" || role == tl!("You") || role == tl!("You (voice)") {
                            "user"
                        } else if role == "Codex" || role == "Codex (voice)" || role == tl!("Codex (voice)") {
                            "assistant"
                        } else {
                            return None;
                        };
                        Some((role.to_owned(), text.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let request = SessionRequest { project, resume, memory, model, scope_aliases, recent_dialogue, assistant_summary: String::new() };
        request.validate()?;
        Ok(request)
    }
}
fn local_root(app: &FilmcraftApp) -> Option<PathBuf> {
    app.session
        .persistence
        .as_ref()
        .map(|p| p.data_dir.clone())
        .or_else(|| app.session.prefs_path.as_ref().and_then(|p| p.parent()).map(std::path::Path::to_path_buf))
}
fn selected_provider(app: &FilmcraftApp) -> super::provider::ProviderId {
    app.hooks.codex_connection.as_ref().map(|c| c.provider_id()).unwrap_or_default()
}
fn index_name(provider: super::provider::ProviderId) -> &'static str {
    match provider {
        super::provider::ProviderId::Codex => "sessions.json",
        super::provider::ProviderId::OpenAiApi => "api-sessions.json",
        super::provider::ProviderId::DesktopChatGpt => "desktop-sessions.json",
    }
}
fn archive_scope(app: &FilmcraftApp) -> Option<String> {
    local_root(app).map(|p| p.join("Console").join("Conversations").to_string_lossy().into_owned())
}
fn memory_scope(app: &FilmcraftApp) -> Option<String> {
    app.session.path.clone().or_else(|| local_root(app).map(|p| p.join("Console").join("Unsaved Project").to_string_lossy().into_owned()))
}
fn load_scopes(app: &FilmcraftApp, scope: String) -> Result<Sessions, String> {
    let mut sessions = Sessions::load_for(scope, selected_provider(app))?;
    if let Some(archive) = archive_scope(app) {
        let global = Sessions::load_for(archive.clone(), selected_provider(app))?;
        let mut records = global.records;
        for record in sessions.records.drain(..) {
            if !records.iter().any(|r| r.thread_id == record.thread_id) {
                records.push(record);
            }
        }
        if records.len() > MAX_SESSIONS {
            return Err("The local conversation archive is full; preserve older entries before creating more conversations".into());
        }
        records.sort_by(|a, b| b.updated.cmp(&a.updated));
        sessions.records = records;
        sessions.index_disk = global.index_disk;
        sessions.history_scope = Some(archive);
    }
    Ok(sessions)
}
pub(super) fn sync_project(app: &mut FilmcraftApp) {
    if app.hooks.codex_connection.is_none()
        || (!app.ui.console.open
            && app.ui.console.sessions.project.is_none()
            && !matches!(app.ui.console.connection, ConnectionStatus::Ready | ConnectionStatus::Connecting))
    {
        return;
    }
    let desired = memory_scope(app);
    if app.ui.console.sessions.provider == selected_provider(app)
        && app.ui.console.sessions.project == desired
        && app.ui.console.sessions.document_path == app.session.path
        && app.ui.console.sessions.epoch == app.session.document_epoch
    {
        return;
    }
    if let Err(error) = save_chat(app) {
        app.ui.status = error;
    }
    let active = app.ui.console.sessions.active.clone();
    let same_document = app.ui.console.sessions.project.is_some() && app.ui.console.sessions.epoch == app.session.document_epoch;
    if same_document
        && let (Some(old), Some(new)) = (app.ui.console.sessions.project.as_deref(), desired.as_deref())
        && let Err(error) = super::references::carry_selection(old, new)
    {
        app.ui.status = error;
        return;
    }
    let old_memory = app.ui.console.sessions.saved_memory.clone();
    let old_draft = app.ui.console.sessions.memory.clone();
    let mut next = match desired {
        None => Sessions::default(),
        Some(scope) => load_scopes(app, scope.clone()).unwrap_or_else(|error| {
            app.ui.status = error.clone();
            Sessions { project: Some(scope), error: Some(error), ..Default::default() }
        }),
    };
    // Saving or renaming the same document must not discard its conversation or creative memory.
    if same_document && next.memory == TEMPLATE && !old_memory.is_empty() && old_memory != TEMPLATE {
        if let Some(scope) = next.project.as_ref() {
            let dir = folder(scope);
            let migrated = (|| {
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                let _lock = Lock::take(&dir)?;
                if let Some(existing) = read(&dir.join("memory.md"), MAX_MEMORY)? {
                    if existing != old_memory {
                        return Err("Destination project memory changed while saving; reload before editing memory".into());
                    }
                } else {
                    filmcraft_format::atomic::atomic_write(&dir.join("memory.md"), old_memory.as_bytes()).map_err(|e| e.to_string())?;
                }
                Ok::<(), String>(())
            })();
            if let Err(error) = migrated {
                next.error = Some(error.clone());
                app.ui.status = error;
            }
        }
        next.memory = old_memory.clone();
        next.saved_memory = old_memory.clone();
    }
    if same_document && old_draft != old_memory {
        next.memory = old_draft;
    }
    if same_document {
        if let Some(generation) = app.ui.console.sessions.memory_generation.checked_add(1) {
            next.memory_generation = generation;
        } else {
            let error = "Memory generation exhausted; reopen the project".to_owned();
            app.ui.status = error.clone();
            next.error = Some(error);
        }
    }
    next.epoch = app.session.document_epoch;
    next.document_path = app.session.path.clone();
    next.active = active.filter(|id| next.records.iter().any(|r| &r.thread_id == id));
    if !same_document && selected_provider(app) == super::provider::ProviderId::OpenAiApi {
        next.active = None;
    }
    if same_document && selected_provider(app) == super::provider::ProviderId::OpenAiApi {
        next.pending_scope_aliases = app.ui.console.sessions.pending_scope_aliases.clone();
        for scope in [app.ui.console.sessions.project.as_ref(), next.project.as_ref()].into_iter().flatten() {
            if !next.pending_scope_aliases.contains(scope) {
                if next.pending_scope_aliases.len() >= 16 {
                    app.ui.status = "API session reached its saved-project alias limit; start a new conversation".into();
                    break;
                }
                next.pending_scope_aliases.push(scope.clone());
            }
        }
        if let Some(active) = next.active.as_ref()
            && let Some(record) = next.records.iter_mut().find(|record| &record.thread_id == active)
        {
            for scope in &next.pending_scope_aliases {
                if !record.scope_aliases.contains(scope) {
                    if record.scope_aliases.len() >= 16 {
                        app.ui.status = "API session reached its saved-project alias limit; start a new conversation".into();
                        break;
                    }
                    record.scope_aliases.push(scope.clone());
                }
            }
            if let Err(error) = next.write_index() {
                app.ui.status = error.clone();
                next.error = Some(error);
            }
        }
    }
    next.context_pending = !same_document && app.ui.console.connection == ConnectionStatus::Ready;
    // A microphone session belongs to its original project context. Stop it before rebinding;
    // otherwise refresh_context waits forever for Voice Off and native edits remain blocked.
    if next.context_pending
        && app.ui.console.chat.voice.on()
        && app.ui.console.chat.voice != VoiceStatus::Stopping
        && let Err(error) = super::commands::voice_off(app)
    {
        app.ui.status = format!("Project changed; reconnect Codex after voice could not stop: {error}");
    }
    if next.context_pending
        && app.ui.console.chat.busy
        && let Some(connection) = app.hooks.codex_connection.as_mut()
        && let Err(error) = connection.cancel()
    {
        app.ui.status = error;
    }
    if !same_document && app.ui.console.connection == ConnectionStatus::Connecting {
        if let Some(connection) = app.hooks.codex_connection.as_mut() {
            connection.disconnect();
        }
        app.ui.console.connection = ConnectionStatus::Disconnected;
        app.ui.console.chat.busy = false;
    }
    super::bridge::cancel_pending(app, "Project context changed");
    app.ui.console.sessions = next;
}
pub(super) fn refresh_context(app: &mut FilmcraftApp) {
    if !app.ui.console.sessions.context_pending
        || app.ui.console.chat.busy
        || app.ui.console.chat.voice.on()
        || app.ui.console.connection != ConnectionStatus::Ready
    {
        return;
    }
    let resume = app.ui.console.sessions.active.clone();
    let request = if selected_provider(app) == super::provider::ProviderId::Codex {
        app.ui
            .console
            .session_lock
            .resolve(resume, true)
            .and_then(|resume| app.ui.console.sessions.request_for_lock(resume, app.ui.console.session_lock.thread_id.is_some()))
    } else {
        app.ui.console.sessions.request(resume)
    };
    let request = request.and_then(|request| super::assistants::attach_context(app, request));
    match request {
        Ok(request) => {
            let result = app.hooks.codex_connection.as_mut().ok_or("Codex is unavailable".to_owned()).and_then(|c| c.switch_session(request));
            match result {
                Ok(()) => {
                    app.ui.console.sessions.context_pending = false;
                    app.ui.console.chat.busy = true;
                }
                Err(error) => {
                    app.ui.status = error.clone();
                    if selected_provider(app) != super::provider::ProviderId::Codex {
                        app.ui.console.connection = ConnectionStatus::Disconnected;
                        app.ui.console.sessions.context_pending = false;
                        app.ui.console.chat.busy = false;
                    } else {
                        app.ui.console.sessions.error = Some(error);
                    }
                }
            }
        }
        Err(error) => {
            app.ui.status = error.clone();
            app.ui.console.sessions.error = Some(error);
        }
    }
}
pub(crate) fn has_unsaved_memory(app: &FilmcraftApp) -> bool {
    app.ui.console.sessions.memory != app.ui.console.sessions.saved_memory
}
pub(crate) fn protect_memory(app: &mut FilmcraftApp) -> String {
    app.ui.console.open = true;
    app.ui.console.sessions.memory_open = true;
    "Save your memory notes before replacing or closing this project, or use Reload from Disk to discard those notes.".into()
}
pub(super) fn saved_memory(app: &FilmcraftApp) -> &str {
    &app.ui.console.sessions.saved_memory
}
pub(super) fn creative_context_changed(app: &mut FilmcraftApp) {
    if app.ui.console.connection == ConnectionStatus::Ready {
        app.ui.console.sessions.context_pending = true;
    }
}
pub(super) fn context_pending(app: &FilmcraftApp) -> bool {
    app.ui.console.sessions.context_pending
}
pub(super) fn disconnect(app: &mut FilmcraftApp) {
    if let Err(error) = super::assistants::flush(app) {
        app.ui.status = error;
    }
    if let Err(error) = save_chat(app) {
        app.ui.status = error;
    }
    if let Some(connection) = app.hooks.codex_connection.as_mut() {
        connection.disconnect();
    }
    super::bridge::cancel_pending(app, "User disconnected Codex");
    app.ui.console.connection = ConnectionStatus::Disconnected;
    app.ui.console.chat.busy = false;
    app.ui.console.chat.voice = VoiceStatus::Off;
    app.ui.console.chat.voice_muted = false;
    app.ui.console.chat.voice_input = VoiceInputStats::default();
}
pub(super) fn reconnect(app: &mut FilmcraftApp) -> Result<(), String> {
    sync_project(app);
    let sessions = &app.ui.console.sessions;
    // Choose a default only at Connect. Marking an archived thread active before restoring
    // its messages would let save_chat overwrite its cache with an empty cold-start chat.
    let resume = sessions.active.clone().or_else(|| {
        sessions
            .records
            .iter()
            .filter(|record| {
                selected_provider(app) != super::provider::ProviderId::OpenAiApi
                    || sessions.project.as_ref().is_some_and(|scope| record.scope_aliases.contains(scope))
            })
            .max_by(|a, b| a.updated.cmp(&b.updated))
            .map(|r| r.thread_id.clone())
    });
    let resume = if selected_provider(app) == super::provider::ProviderId::Codex { app.ui.console.session_lock.resolve(resume, true)? } else { resume };
    start(app, resume)
}
pub(super) fn save_chat(app: &mut FilmcraftApp) -> Result<(), String> {
    let sessions = &mut app.ui.console.sessions;
    if let Some(id) = &sessions.active
        && let Some(record) = sessions.records.iter_mut().find(|r| &r.thread_id == id)
    {
        record.messages = app.ui.console.chat.messages.clone();
        record.model = app.ui.console.chat.selected_model.clone();
        // sync_project runs after the engine has switched documents. Archive the outgoing
        // conversation against the document it actually described, not the newly opened path.
        if !sessions.context_pending {
            record.project = sessions.document_path.clone();
        }
        record.updated = chrono::Utc::now().to_rfc3339();
        if record.title == "New conversation"
            && let Some((_, text)) = record.messages.iter().find(|(role, _)| role == "You")
        {
            record.title = text.chars().take(64).collect();
        }
        sessions.write_index()?;
    }
    Ok(())
}
pub(super) fn started(app: &mut FilmcraftApp, id: String) -> Result<(), String> {
    if !valid_id(&id) {
        return Err("Codex returned an invalid saved conversation ID".into());
    }
    let sessions = &mut app.ui.console.sessions;
    sessions.active = Some(id.clone());
    if !sessions.records.iter().any(|r| r.thread_id == id) {
        if sessions.records.len() >= MAX_SESSIONS {
            return Err("Saved conversation limit reached".into());
        }
        sessions.records.insert(
            0,
            SavedSession {
                thread_id: id,
                scope_aliases: if sessions.provider == super::provider::ProviderId::OpenAiApi {
                    let mut aliases = sessions.pending_scope_aliases.clone();
                    if let Some(scope) = sessions.project.as_ref()
                        && !aliases.contains(scope)
                    {
                        if aliases.len() >= 16 {
                            return Err("API session scope alias limit reached".into());
                        }
                        aliases.push(scope.clone());
                    }
                    aliases
                } else {
                    Vec::new()
                },
                title: "New conversation".into(),
                updated: chrono::Utc::now().to_rfc3339(),
                model: app.ui.console.chat.selected_model.clone(),
                project: app.session.path.clone(),
                messages: VecDeque::new(),
            },
        );
    }
    save_chat(app)
}
pub(super) fn start(app: &mut FilmcraftApp, resume: Option<String>) -> Result<(), String> {
    if app.hooks.codex_connection.is_none() {
        return Err("Codex connections require the desktop app".into());
    }
    if app.ui.console.chat.busy || app.ui.console.chat.voice.on() {
        return Err("Finish or cancel the current turn and turn voice off before changing conversations".into());
    }
    super::api_settings::prepare(app)?;
    let codex = selected_provider(app) == super::provider::ProviderId::Codex;
    let resume = if codex { app.ui.console.session_lock.resolve(resume, false)? } else { resume };
    let locked = codex && app.ui.console.session_lock.thread_id.is_some();
    app.ui.console.open = true;
    sync_project(app);
    save_chat(app)?;
    let request = if app.ui.console.sessions.project.is_some() {
        Some(app.ui.console.sessions.request_for_lock(resume.clone(), locked)?)
    } else {
        if resume.is_some() {
            return Err("Save this project before resuming a conversation".into());
        }
        None
    };
    let mut request = request.map(|request| super::assistants::attach_context(app, request)).transpose()?;
    // An explicit API setup choice must not be overridden by a cached conversation model.
    if selected_provider(app) == super::provider::ProviderId::OpenAiApi
        && let Some(request) = request.as_mut()
        && let Some(model) = app.ui.console.chat.selected_model.as_ref()
    {
        request.model = Some(model.clone());
    }
    let preparing = request.is_some();
    let cached = resume.as_ref().and_then(|id| app.ui.console.sessions.records.iter().find(|r| &r.thread_id == id)).cloned();
    let connection = app.hooks.codex_connection.as_mut().ok_or("Codex is unavailable")?;
    let connected = app.ui.console.connection == ConnectionStatus::Ready;
    if connected {
        connection.switch_session(request.clone().ok_or("Save this project before starting another conversation")?)?;
    } else {
        connection.disconnect();
        let result = match request {
            Some(request) => connection.connect_session(request),
            None => connection.connect(),
        };
        if let Err(error) = result {
            app.ui.console.connection = ConnectionStatus::Failed(error.clone());
            return Err(error);
        }
    }
    let api_model = (selected_provider(app) == super::provider::ProviderId::OpenAiApi).then(|| app.ui.console.chat.selected_model.clone()).flatten();
    let models = app.ui.console.chat.models.clone();
    let default_model = app.ui.console.chat.default_model.clone();
    // Available voices belong to this transport, not the conversation being replaced.
    let voice_catalog =
        connected.then(|| (app.ui.console.chat.voices.clone(), app.ui.console.chat.voice_catalog_error.clone(), app.ui.console.chat.voice_catalog_pending));
    super::bridge::cancel_pending(app, "Conversation or project changed");
    app.ui.console.chat = chat::Chat::default();
    app.ui.console.chat.busy = preparing;
    app.ui.console.chat.models = models;
    app.ui.console.chat.default_model = default_model;
    if let Some((voices, error, pending)) = voice_catalog {
        app.ui.console.chat.voices = voices;
        app.ui.console.chat.voice_catalog_error = error;
        app.ui.console.chat.voice_catalog_pending = pending;
    }
    if let Some(cached) = cached {
        app.ui.console.chat.messages = cached.messages;
        app.ui.console.chat.selected_model = cached.model;
    }
    if api_model.is_some() {
        app.ui.console.chat.selected_model = api_model;
    }
    app.ui.console.sessions.active = resume;
    app.ui.console.sessions.context_pending = false;
    app.ui.console.sessions.picker_open = false;
    app.ui.console.open = true;
    if !connected {
        app.ui.console.connection = ConnectionStatus::Connecting;
    }
    Ok(())
}
pub(super) fn invoke(app: &mut FilmcraftApp, id: &str, params: &serde_json::Map<String, Value>) -> Result<Value, String> {
    if app.hooks.codex_connection.is_none() {
        return Err("Project memory and saved conversations require the desktop Codex connection".into());
    }
    app.ui.console.open = true;
    sync_project(app);
    match id {
        "console.sessions.reload" => {
            if app.ui.console.chat.busy || app.ui.console.chat.voice.on() {
                return Err("Finish the current turn and turn voice off before reloading".into());
            }
            let project = memory_scope(app).ok_or("Local conversation storage is unavailable")?;
            let mut loaded = load_scopes(app, project)?;
            loaded.memory_generation = app.ui.console.sessions.memory_generation.checked_add(1).ok_or("Memory generation exhausted; reopen the project")?;
            loaded.document_path = app.session.path.clone();
            loaded.epoch = app.session.document_epoch;
            loaded.memory_open = true;
            if let Some(c) = app.hooks.codex_connection.as_mut() {
                c.disconnect();
            }
            app.ui.console.sessions = loaded;
            super::bridge::cancel_pending(app, "Conversation or project changed");
            app.ui.console.chat = chat::Chat::default();
            app.ui.console.connection = ConnectionStatus::Disconnected;
        }
        "console.session.lock" | "console.session.unlock" => {
            if selected_provider(app) != super::provider::ProviderId::Codex {
                return Err("Session locking is available for the Codex connection only".into());
            }
            let live = app.ui.console.chat.busy || app.ui.console.chat.voice.on();
            if id == "console.session.unlock" && live {
                return Err("Finish the current turn and turn voice off before unlocking the session".into());
            }
            if id == "console.session.lock" && matches!(app.ui.console.connection, ConnectionStatus::Connecting) {
                return Err("Wait for Codex to connect before locking its conversation".into());
            }
            let target = if id == "console.session.lock" {
                Some(match params.get("threadId") {
                    Some(value) => value.as_str().ok_or("threadId must be a Codex conversation UUID")?.to_owned(),
                    None => app.ui.console.sessions.active.clone().ok_or("Connect to a Codex session before locking it")?,
                })
            } else {
                None
            };
            if target.is_some()
                && target != app.ui.console.sessions.active
                && (live || matches!(app.ui.console.connection, ConnectionStatus::Ready | ConnectionStatus::Connecting))
            {
                return Err("Disconnect Codex before locking a different conversation".into());
            }
            app.ui.console.session_lock.store(target)?;
        }
        "console.session.new" => start(app, None)?,
        "console.session.resume" => {
            let thread = params.get("threadId").and_then(Value::as_str).filter(|s| valid_id(s)).ok_or("Choose a saved conversation")?;
            start(app, Some(thread.into()))?;
        }
        "console.sessions.open" => {
            app.ui.console.sessions.picker_open = !app.ui.console.sessions.picker_open;
        }
        "console.memory.open" => {
            app.ui.console.sessions.memory_open = !app.ui.console.sessions.memory_open;
        }
        "console.memory.save" => {
            let text = params.get("text").and_then(Value::as_str).ok_or("Memory text must be a string")?;
            save_memory_with(&mut app.ui.console.sessions, text, |path, bytes| {
                filmcraft_format::atomic::atomic_write(path, bytes).map_err(|e| format!("Cannot save project memory: {e}"))
            })?;
        }
        _ => return Err("Unknown conversation command".into()),
    }
    Ok(json!({"ok":true}))
}
fn action(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: Value) {
    if let Err(error) = menus::invoke(app, ctx, id, params) {
        app.ui.status = error;
    }
}
pub(super) fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    if app.hooks.codex_connection.is_none() {
        return;
    }
    app.ui.console.session_lock.ensure_loaded();
    let is_codex = selected_provider(app) == super::provider::ProviderId::Codex;
    let locked = is_codex && (app.ui.console.session_lock.thread_id.is_some() || app.ui.console.session_lock.error.is_some());
    let saved = app.ui.console.sessions.project.is_some();
    let can_resume = selected_provider(app) == super::provider::ProviderId::Codex
        || app.hooks.codex_connection.as_ref().is_some_and(|connection| connection.provider_capabilities()["resume"].as_bool() == Some(true));
    let resume_label = match selected_provider(app) {
        super::provider::ProviderId::Codex => tl!("Resume Codex Session"),
        super::provider::ProviderId::OpenAiApi => "Resume API Session",
        super::provider::ProviderId::DesktopChatGpt => "Resume in ChatGPT",
    };
    let idle = !app.ui.console.chat.busy && !app.ui.console.chat.voice.on();
    ui.horizontal_wrapped(|ui| {
        for (id, label, icon, enabled) in [
            ("console.session.new", tl!("New Conversation"), crate::icons::Icon::NewItem, idle && saved && !locked && app.hooks.codex_connection.is_some()),
            ("console.sessions.open", resume_label, crate::icons::Icon::Folder, idle && saved && can_resume),
        ] {
            let response = super::layout::icon_button(app, ui, id, icon, enabled, false, label);
            if response.clicked() {
                action(app, ui.ctx(), id, json!({}));
            }
        }
        if is_codex {
            let (id, label, icon) = if locked {
                ("console.session.unlock", "Unlock Session", crate::icons::Icon::Lock)
            } else {
                ("console.session.lock", "LOCK SESSION", crate::icons::Icon::Unlock)
            };
            let enabled = if locked {
                idle
            } else {
                app.ui.console.sessions.active.is_some()
                    && !matches!(app.ui.console.connection, ConnectionStatus::Connecting)
                    && (idle || matches!(app.ui.console.connection, ConnectionStatus::Ready))
            };
            if super::layout::icon_button(app, ui, id, icon, enabled, locked, label).clicked() {
                action(app, ui.ctx(), id, json!({}));
            }
        }
        ui.separator();
        super::layout::undo(app, ui);
        super::layout::settings(app, ui);
    });
    if is_codex {
        if let Some(thread) = &app.ui.console.session_lock.thread_id {
            ui.small("Session locked")
                .on_hover_text(format!("Codex session locked: {thread}\nConnect resumes only this conversation. The lock is shared across FilmCraft builds."));
        }
        if let Some(error) = &app.ui.console.session_lock.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
    }
    if !saved {
        ui.label(tl!("Local conversation storage is unavailable. Check the application data folder."));
    }
    if let Some(error) = &app.ui.console.sessions.error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    if app.ui.console.sessions.memory_open {
        if let Some(project) = &app.ui.console.sessions.project {
            ui.monospace(folder(project).join("memory.md").display().to_string());
        }
        ui.label(tl!("Memory loads into new and resumed conversations. Save changes before continuing."));
        ui.label(format!("{} / {} UTF-8 bytes saved capacity", app.ui.console.sessions.memory.len(), MAX_MEMORY));
        let editor = egui::ScrollArea::vertical().id_salt("console.memory.editor.scroll").max_height(240.0).show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut app.ui.console.sessions.memory)
                    .desired_rows(6)
                    .desired_width(f32::INFINITY)
                    .char_limit(MAX_MEMORY)
                    .hint_text(tl!("Project memory")),
            )
        });
        app.auto.add("console.memory.input", editor.inner.rect.intersect(editor.inner_rect), "Project memory");
        ui.label(tl!("Reload from Disk discards unsaved memory changes and disconnects Codex."));
        let reload = ui.add_enabled(idle, egui::Button::new(tl!("Reload from Disk")));
        app.auto.add("console.sessions.reload", reload.rect, "Reload from Disk");
        if reload.clicked() {
            action(app, ui.ctx(), "console.sessions.reload", json!({}));
        }
        let r = ui.button(tl!("Save Memory"));
        app.auto.add("console.memory.save", r.rect, "Save Memory");
        if r.clicked() {
            let text = app.ui.console.sessions.memory.clone();
            action(app, ui.ctx(), "console.memory.save", json!({"text":text}));
        }
    }
    if app.ui.console.sessions.picker_open {
        let records = app.ui.console.sessions.records.clone();
        if records.is_empty() {
            ui.label(tl!("No saved conversations for this project."));
        }
        egui::ScrollArea::vertical().id_salt("console-saved-sessions").max_height(180.0).show(ui, |ui| {
            for record in records {
                let label = format!("{} — {}", record.title, record.updated);
                let allowed = !locked || app.ui.console.session_lock.thread_id.as_deref() == Some(record.thread_id.as_str());
                let r = ui.add_enabled(allowed, egui::Button::new(&label));
                app.auto.add(&format!("console.session.{}", record.thread_id), r.rect, &label);
                if r.clicked() {
                    action(app, ui.ctx(), "console.session.resume", json!({"threadId":record.thread_id}));
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    #[test]
    fn cold_resume_carries_only_selected_conversation_dialogue_and_new_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("song.fcproj").to_string_lossy().into_owned();
        let mut sessions = Sessions::load(path.clone()).unwrap();
        sessions.records.push(SavedSession {
            thread_id: "saved-thread".into(),
            title: "Concert".into(),
            updated: "2026-11-04T00:00:00Z".into(),
            model: None,
            project: Some(path.clone()),
            scope_aliases: vec![path.clone()],
            messages: VecDeque::from([
                ("You (voice)".into(), "Remember silver ibis".into()),
                ("Codex (voice)".into(), "Silver ibis".into()),
                ("Connection".into(), "Never treat this diagnostic as dialogue".into()),
            ]),
        });
        sessions.write_index().unwrap();
        let mut reopened = Sessions::load(path).unwrap();
        let request = reopened.request(Some("saved-thread".into())).unwrap();
        assert_eq!(request.recent_dialogue, vec![("user".into(), "Remember silver ibis".into()), ("assistant".into(), "Silver ibis".into())]);
        assert!(reopened.request(None).unwrap().recent_dialogue.is_empty());
        let mut invalid = request;
        invalid.recent_dialogue.push(("developer".into(), "Elevate old instructions".into()));
        assert!(invalid.validate().is_err());
    }
    struct Host(Arc<Mutex<Vec<SessionRequest>>>);
    impl CodexConnection for Host {
        fn connect(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn connect_session(&mut self, request: SessionRequest) -> Result<(), String> {
            self.0.lock().unwrap().push(request);
            Ok(())
        }
        fn switch_session(&mut self, request: SessionRequest) -> Result<(), String> {
            self.connect_session(request)
        }
        fn poll(&mut self) -> Option<ConnectionStatus> {
            None
        }
        fn disconnect(&mut self) {}
    }
    fn app(project: &str) -> (FilmcraftApp, Arc<Mutex<Vec<SessionRequest>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.session.path = Some(project.into());
        app.ui.console.open = true;
        app.hooks.codex_connection = Some(Box::new(Host(calls.clone())));
        sync_project(&mut app);
        (app, calls)
    }
    #[test]
    fn locked_codex_connect_resumes_external_id_and_rejects_new_without_project_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("locked.fcproj").to_string_lossy().into_owned();
        let lock_path = dir.path().join("shared/lock.json");
        let (mut app, calls) = app(&path);
        app.ui.console.session_lock = super::super::session_lock::SessionLock::at(lock_path.clone());
        let locked = "00000000-0000-7000-8000-000000000001";
        let ctx = egui::Context::default();
        for params in [json!({"threadId":4}), json!({"threadId":"../thread"}), json!({"threadId":"x".repeat(1024)})] {
            assert!(menus::invoke(&mut app, &ctx, "console.session.lock", params).is_err());
            assert!(!lock_path.exists());
        }
        menus::invoke(&mut app, &ctx, "console.session.lock", json!({"threadId":locked})).unwrap();
        let project = app.session.project.clone();
        let identity = (app.session.revision, app.session.document_epoch);
        reconnect(&mut app).unwrap();
        assert_eq!(calls.lock().unwrap().last().unwrap().resume.as_deref(), Some(locked));
        app.ui.console.chat.busy = false;
        assert!(start(&mut app, None).is_err());
        assert!(start(&mut app, Some("other-thread".into())).is_err());
        assert_eq!(calls.lock().unwrap().len(), 1);
        assert!(Arc::ptr_eq(&project, &app.session.project));
        assert_eq!((app.session.revision, app.session.document_epoch), identity);
        disconnect(&mut app);
        let (mut reopened, second_calls) = self::app(&path);
        reopened.ui.console.session_lock = super::super::session_lock::SessionLock::at(lock_path);
        reconnect(&mut reopened).unwrap();
        assert_eq!(second_calls.lock().unwrap().last().unwrap().resume.as_deref(), Some(locked));
    }

    #[test]
    fn lock_current_session_during_voice_preserves_call_and_reopens_exact_thread() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live.fcproj").to_string_lossy().into_owned();
        let lock_path = dir.path().join("shared/lock.json");
        let (mut app, calls) = app(&path);
        let thread = "00000000-0000-7000-8000-000000000003";
        app.ui.console.session_lock = super::super::session_lock::SessionLock::at(lock_path.clone());
        app.ui.console.sessions.active = Some(thread.into());
        app.ui.console.connection = ConnectionStatus::Ready;
        app.ui.console.chat.voice = VoiceStatus::Active;
        app.ui.console.chat.voice_muted = true;
        app.ui.console.chat.busy = true;
        app.ui.console.chat.voice_sent = 123;
        app.ui.console.chat.voice_received = 456;
        let project = app.session.project.clone();
        let identity = (app.session.revision, app.session.document_epoch);
        let ctx = egui::Context::default();
        menus::invoke(&mut app, &ctx, "console.session.lock", json!({})).unwrap();
        assert_eq!(app.ui.console.session_lock.thread_id.as_deref(), Some(thread));
        assert_eq!(app.ui.console.chat.voice, VoiceStatus::Active);
        assert!(app.ui.console.chat.voice_muted);
        assert!(app.ui.console.chat.busy);
        assert_eq!((app.ui.console.chat.voice_sent, app.ui.console.chat.voice_received), (123, 456));
        assert!(calls.lock().unwrap().is_empty());
        assert!(Arc::ptr_eq(&project, &app.session.project));
        assert_eq!((app.session.revision, app.session.document_epoch), identity);
        assert!(menus::invoke(&mut app, &ctx, "console.session.unlock", json!({})).is_err());
        assert!(menus::invoke(&mut app, &ctx, "console.session.lock", json!({"threadId":"00000000-0000-7000-8000-000000000001"})).is_err());
        assert_eq!(app.ui.console.session_lock.thread_id.as_deref(), Some(thread));
        let (mut reopened, requests) = self::app(&path);
        reopened.ui.console.session_lock = super::super::session_lock::SessionLock::at(lock_path);
        reconnect(&mut reopened).unwrap();
        assert_eq!(requests.lock().unwrap().last().unwrap().resume.as_deref(), Some(thread));
    }

    #[test]
    fn speaking_voice_catalog_survives_conversation_change_but_not_new_transport() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("voice.fcproj").to_string_lossy().into_owned();
        let (mut app, calls) = app(&path);
        app.ui.console.connection = ConnectionStatus::Ready;
        app.ui.console.chat.voices = vec!["sol".into(), "cove".into()];
        app.session.prefs.general.codex_speaking_voice = "sol".into();
        let project = app.session.project.clone();
        let identity = (app.session.revision, app.session.document_epoch);
        start(&mut app, None).unwrap();
        assert_eq!(app.ui.console.chat.voices, ["sol", "cove"]);
        assert!(app.ui.console.chat.busy, "context preparation keeps voice controls disabled");
        assert_eq!(app.session.prefs.general.codex_speaking_voice, "sol");
        app.ui.console.chat.busy = false;
        app.ui.console.connection = ConnectionStatus::Disconnected;
        start(&mut app, None).unwrap();
        assert!(app.ui.console.chat.voices.is_empty());
        assert!(app.ui.console.chat.voice_catalog_error.is_none());
        assert_eq!(app.session.prefs.general.codex_speaking_voice, "sol");
        assert_eq!(calls.lock().unwrap().len(), 2);
        assert!(Arc::ptr_eq(&project, &app.session.project));
        assert_eq!((app.session.revision, app.session.document_epoch), identity);
    }

    #[test]
    fn switching_projects_stops_voice_and_refreshes_before_editing() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct VoiceHost {
            calls: Arc<Mutex<Vec<SessionRequest>>>,
            stopped: Arc<AtomicUsize>,
            events: VecDeque<ChatEvent>,
        }
        impl CodexConnection for VoiceHost {
            fn connect(&mut self) -> Result<(), String> {
                Ok(())
            }
            fn switch_session(&mut self, request: SessionRequest) -> Result<(), String> {
                self.calls.lock().unwrap().push(request);
                Ok(())
            }
            fn poll(&mut self) -> Option<ConnectionStatus> {
                None
            }
            fn disconnect(&mut self) {}
            fn stop_voice(&mut self) -> Result<(), String> {
                self.stopped.fetch_add(1, Ordering::SeqCst);
                self.events.push_back(ChatEvent::Voice(VoiceStatus::Off));
                Ok(())
            }
            fn poll_chat(&mut self) -> Option<ChatEvent> {
                self.events.pop_front()
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("first.fcproj").to_string_lossy().into_owned();
        let destination = dir.path().join("second.fcproj").to_string_lossy().into_owned();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let stopped = Arc::new(AtomicUsize::new(0));
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.session.path = Some(original.clone());
        app.session.prefs_path = Some(dir.path().join("prefs.json"));
        app.ui.console.open = true;
        app.hooks.codex_connection = Some(Box::new(VoiceHost { calls: calls.clone(), stopped: stopped.clone(), events: VecDeque::new() }));
        sync_project(&mut app);
        app.ui.console.connection = ConnectionStatus::Ready;
        started(&mut app, "existing-thread".into()).unwrap();
        app.ui.console.chat.voice = VoiceStatus::Active;
        app.session.path = Some(destination.clone());
        app.session.document_epoch += 1;
        sync_project(&mut app);
        assert_eq!(stopped.load(Ordering::SeqCst), 1);
        assert_eq!(app.ui.console.chat.voice, VoiceStatus::Stopping);
        assert!(context_pending(&app));
        assert!(super::super::execution::prepare(&app, "markers.add", json!({})).is_err());
        refresh_context(&mut app);
        assert!(calls.lock().unwrap().is_empty());
        let destination = dir.path().join("third.fcproj").to_string_lossy().into_owned();
        app.session.path = Some(destination.clone());
        app.session.document_epoch += 1;
        sync_project(&mut app);
        assert_eq!(stopped.load(Ordering::SeqCst), 1, "a rapid second switch must not restart the voice stop");
        let ctx = egui::Context::default();
        chat::poll(&mut app, &ctx);
        assert_eq!(app.ui.console.chat.voice, VoiceStatus::Off);
        assert!(
            super::super::commands::invoke(&mut app, &ctx, "console.chat.send", json!({"message":"Edit the new project"}))
                .unwrap_err()
                .contains("project context")
        );
        assert_eq!(app.ui.console.sessions.records[0].project.as_ref(), Some(&original));
        refresh_context(&mut app);
        let requests = calls.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].project, destination);
        assert!(!context_pending(&app));
        assert!(app.ui.console.chat.busy);
    }
    #[test]
    fn connected_project_switch_without_saved_thread_starts_fresh_context() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("first.fcproj").to_string_lossy().into_owned();
        let (mut app, calls) = app(&project);
        app.ui.console.connection = ConnectionStatus::Ready;
        app.session.path = Some(dir.path().join("second.fcproj").to_string_lossy().into_owned());
        app.session.document_epoch += 1;
        let error = super::super::commands::invoke(&mut app, &egui::Context::default(), "console.voice.on", json!({})).unwrap_err();
        assert!(error.contains("project context"));
        assert!(context_pending(&app));
        refresh_context(&mut app);
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].project, app.session.path.as_ref().unwrap().as_str());
        assert!(calls[0].resume.is_none());
        assert!(!context_pending(&app));
    }
    struct FailedExternalContext(super::super::provider::ProviderId, Arc<Mutex<Vec<SessionRequest>>>);
    impl CodexConnection for FailedExternalContext {
        fn provider_id(&self) -> super::super::provider::ProviderId {
            self.0
        }
        fn connect(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn connect_session(&mut self, request: SessionRequest) -> Result<(), String> {
            self.1.lock().unwrap().push(request);
            Ok(())
        }
        fn switch_session(&mut self, _request: SessionRequest) -> Result<(), String> {
            Err("External project context changed; reconnect".into())
        }
        fn poll(&mut self) -> Option<ConnectionStatus> {
            None
        }
        fn disconnect(&mut self) {}
    }
    #[test]
    fn external_provider_context_error_allows_reconnect_without_memory_reload() {
        for provider in [super::super::provider::ProviderId::OpenAiApi, super::super::provider::ProviderId::DesktopChatGpt] {
            let directory = tempfile::tempdir().unwrap();
            let first = directory.path().join("first.fcproj").to_string_lossy().into_owned();
            let (mut app, _) = app(&first);
            let requests = Arc::new(Mutex::new(Vec::new()));
            app.hooks.codex_connection = Some(Box::new(FailedExternalContext(provider, requests.clone())));
            sync_project(&mut app);
            app.ui.console.connection = ConnectionStatus::Ready;
            let second = directory.path().join("second.fcproj").to_string_lossy().into_owned();
            app.session.path = Some(second.clone());
            app.session.document_epoch += 1;
            sync_project(&mut app);
            assert!(context_pending(&app));
            refresh_context(&mut app);
            assert!(app.ui.console.sessions.error.is_none(), "Connection errors must not poison the saved memory store");
            assert_eq!(app.ui.console.connection, ConnectionStatus::Disconnected);
            assert!(!context_pending(&app));
            reconnect(&mut app).unwrap();
            let request = requests.lock().unwrap().last().unwrap().clone();
            assert_eq!(request.project, second);
            assert!(request.resume.is_none());
        }
    }
    struct ApiHost(Host);
    impl CodexConnection for ApiHost {
        fn provider_id(&self) -> super::super::provider::ProviderId {
            super::super::provider::ProviderId::OpenAiApi
        }
        fn provider_capabilities(&self) -> Value {
            json!({"resume":true,"embeddedComposer":true,"voice":false})
        }
        fn connect(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn connect_session(&mut self, request: SessionRequest) -> Result<(), String> {
            self.0.connect_session(request)
        }
        fn switch_session(&mut self, request: SessionRequest) -> Result<(), String> {
            self.0.switch_session(request)
        }
        fn poll(&mut self) -> Option<ConnectionStatus> {
            None
        }
        fn disconnect(&mut self) {}
    }
    #[test]
    fn api_explicit_model_survives_resuming_a_cached_conversation() {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.session.path = Some(dir.path().join("api.fcproj").to_string_lossy().into_owned());
        app.session.prefs_path = Some(dir.path().join("preferences.json"));
        app.hooks.codex_connection = Some(Box::new(ApiHost(Host(calls.clone()))));
        sync_project(&mut app);
        start(&mut app, None).unwrap();
        started(&mut app, "api-cached-session".into()).unwrap();
        app.ui.console.sessions.records.first_mut().unwrap().model = Some("old-model".into());
        app.ui.console.sessions.active = None;
        app.ui.console.chat.busy = false;
        app.ui.console.connection = ConnectionStatus::Disconnected;
        app.ui.console.chat.selected_model = Some("explicit-model".into());
        start(&mut app, Some("api-cached-session".into())).unwrap();
        let request = calls.lock().unwrap().last().unwrap().clone();
        assert_eq!(request.model.as_deref(), Some("explicit-model"));
        assert_eq!(app.ui.console.chat.selected_model.as_deref(), Some("explicit-model"));
    }

    #[test]
    fn api_first_save_aliases_survive_disconnection_and_archive_reload_without_cross_project_resume() {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.session.prefs_path = Some(dir.path().join("preferences.json"));
        app.hooks.codex_connection = Some(Box::new(ApiHost(Host(calls.clone()))));
        app.ui.console.open = true;
        sync_project(&mut app);
        start(&mut app, None).unwrap();
        app.ui.console.chat.busy = false;
        app.ui.console.connection = ConnectionStatus::Ready;
        started(&mut app, "api-draft-session".into()).unwrap();
        let old = app.ui.console.sessions.project.clone().unwrap();
        disconnect(&mut app);
        let saved = dir.path().join("saved.fcproj").to_string_lossy().into_owned();
        app.session.path = Some(saved.clone());
        sync_project(&mut app);
        assert!(app.ui.console.sessions.records[0].scope_aliases.contains(&old));
        assert!(app.ui.console.sessions.records[0].scope_aliases.contains(&saved));
        let loaded = load_scopes(&app, saved.clone()).unwrap();
        app.ui.console.sessions = loaded;
        reconnect(&mut app).unwrap();
        let request = calls.lock().unwrap().last().unwrap().clone();
        assert_eq!(request.resume.as_deref(), Some("api-draft-session"));
        assert!(request.scope_aliases.contains(&old));
        assert!(request.scope_aliases.contains(&saved));
        app.ui.console.chat.busy = false;
        app.ui.console.connection = ConnectionStatus::Disconnected;
        app.session.path = Some(dir.path().join("other.fcproj").to_string_lossy().into_owned());
        app.session.document_epoch += 1;
        sync_project(&mut app);
        reconnect(&mut app).unwrap();
        assert!(calls.lock().unwrap().last().unwrap().resume.is_none());
    }
    #[test]
    fn unsaved_conversation_survives_disconnect_restart_and_first_save() {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.session.prefs_path = Some(dir.path().join("preferences.json"));
        app.hooks.codex_connection = Some(Box::new(Host(calls.clone())));
        app.ui.console.open = true;
        sync_project(&mut app);
        assert!(app.session.path.is_none());
        assert!(app.ui.console.sessions.project.is_some());
        start(&mut app, None).unwrap();
        assert!(calls.lock().unwrap()[0].resume.is_none());
        app.ui.console.chat.busy = false;
        app.ui.console.connection = ConnectionStatus::Ready;
        started(&mut app, "draft-thread".into()).unwrap();
        chat::message(&mut app.ui.console.chat, "You", "Remember this draft".into());
        chat::message(&mut app.ui.console.chat, "You (voice)", "Remember copper otter".into());
        chat::message(&mut app.ui.console.chat, "Codex (voice)", "Copper otter".into());
        save_chat(&mut app).unwrap();
        disconnect(&mut app);
        assert!(!app.ui.console.chat.busy);
        assert_eq!(app.ui.console.sessions.active.as_deref(), Some("draft-thread"));
        reconnect(&mut app).unwrap();
        assert_eq!(calls.lock().unwrap().last().unwrap().resume.as_deref(), Some("draft-thread"));
        assert!(calls.lock().unwrap().last().unwrap().recent_dialogue.contains(&("user".into(), "Remember copper otter".into())));
        app.ui.console.chat.busy = false;
        app.ui.console.connection = ConnectionStatus::Ready;
        started(&mut app, "draft-thread".into()).unwrap();
        app.session.path = Some(dir.path().join("First Save.fcproj").to_string_lossy().into_owned());
        sync_project(&mut app);
        assert_eq!(app.ui.console.sessions.active.as_deref(), Some("draft-thread"));
        assert_eq!(app.ui.console.connection, ConnectionStatus::Ready);
        save_chat(&mut app).unwrap();
        let mut reopened = FilmcraftApp::new(filmcraft_engine::Session::default());
        reopened.session.prefs_path = Some(dir.path().join("preferences.json"));
        reopened.ui.console.open = true;
        reopened.hooks.codex_connection = Some(Box::new(Host(calls.clone())));
        sync_project(&mut reopened);
        assert!(reopened.ui.console.sessions.records.iter().any(|r| r.thread_id == "draft-thread"));
        reconnect(&mut reopened).unwrap();
        assert_eq!(calls.lock().unwrap().last().unwrap().resume.as_deref(), Some("draft-thread"));
        assert!(calls.lock().unwrap().last().unwrap().recent_dialogue.contains(&("assistant".into(), "Copper otter".into())));
        assert!(reopened.ui.console.chat.messages.iter().any(|(_, text)| text == "Remember this draft"));
        assert_eq!(reopened.ui.console.chat.voice, VoiceStatus::Off);
    }
    #[test]
    fn cold_connect_resumes_previous_conversation_across_projects_and_new_remains_explicit() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("song.fcproj").to_string_lossy().into_owned();
        let other = dir.path().join("other.fcproj").to_string_lossy().into_owned();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.session.path = Some(project.clone());
        app.session.prefs_path = Some(dir.path().join("preferences.json"));
        app.hooks.codex_connection = Some(Box::new(Host(calls.clone())));
        app.ui.console.open = true;
        let archive = archive_scope(&app).unwrap();
        let records = [("old", "2026-10-28", &project), ("latest", "2026-10-29", &project), ("different", "2026-10-30", &other)]
            .into_iter()
            .map(|(id, updated, path)| SavedSession {
                thread_id: id.into(),
                scope_aliases: Vec::new(),
                title: id.into(),
                updated: updated.into(),
                model: None,
                project: Some(path.clone()),
                messages: VecDeque::new(),
            })
            .collect();
        std::fs::create_dir_all(folder(&archive)).unwrap();
        std::fs::write(folder(&archive).join("sessions.json"), serde_json::to_vec(&Index { version: 1, project: archive, sessions: records }).unwrap())
            .unwrap();
        sync_project(&mut app);
        assert!(app.ui.console.sessions.active.is_none());
        reconnect(&mut app).unwrap();
        assert_eq!(calls.lock().unwrap().last().unwrap().resume.as_deref(), Some("different"));
        assert_eq!(calls.lock().unwrap().last().unwrap().project, project);
        app.ui.console.chat.busy = false;
        app.ui.console.connection = ConnectionStatus::Ready;
        start(&mut app, None).unwrap();
        assert!(calls.lock().unwrap().last().unwrap().resume.is_none());
        assert!(app.ui.console.sessions.active.is_none());
    }
    #[test]
    fn project_opening_does_not_read_console_files_without_feature_intent_or_host() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("song.fcproj").to_string_lossy().into_owned();
        std::fs::create_dir(folder(&project)).unwrap();
        std::fs::write(folder(&project).join("sessions.json"), "corrupt").unwrap();
        let mut no_host = FilmcraftApp::new(filmcraft_engine::Session::default());
        no_host.session.path = Some(project.clone());
        no_host.ui.console.open = true;
        sync_project(&mut no_host);
        assert!(no_host.ui.console.sessions.project.is_none());
        assert!(no_host.ui.status.is_empty());
        let requests = Arc::new(Mutex::new(Vec::new()));
        no_host.hooks.codex_connection = Some(Box::new(Host(requests)));
        no_host.ui.console.open = false;
        sync_project(&mut no_host);
        assert!(no_host.ui.console.sessions.project.is_none());
        assert!(no_host.ui.status.is_empty());
        no_host.ui.console.open = true;
        sync_project(&mut no_host);
        assert!(no_host.ui.console.sessions.error.is_some());
    }
    #[test]
    fn large_memory_survives_save_reopen_and_paged_reads_without_expanding_context() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("large.fcproj").to_string_lossy().into_owned();
        let (mut editor, calls) = app(&project);
        let unit = "café 🎬 notes\n";
        let body = MAX_MEMORY - "START\n".len() - "\nTAIL".len();
        let original = format!("START\n{}{}\nTAIL", unit.repeat(body / unit.len()), "x".repeat(body % unit.len()));
        assert_eq!(original.len(), MAX_MEMORY);
        let before = Arc::clone(&editor.session.project);
        let revision = editor.session.revision;
        crate::menus::invoke(&mut editor, &egui::Context::default(), "console.memory.save", json!({"text":original})).unwrap();
        let mut page = memory(&mut editor, json!({"action":"read"}).as_object().unwrap()).unwrap();
        assert_eq!(page["totalBytes"], original.len());
        assert_eq!(page["maxBytes"], MAX_MEMORY);
        assert_eq!(page["truncated"], true);
        let mut rebuilt = String::new();
        for _ in 0..512 {
            assert!(serde_json::to_vec(&page).unwrap().len() <= 32 * 1024);
            rebuilt.push_str(page["text"].as_str().unwrap());
            if page["nextOffset"].is_null() {
                break;
            }
            let params = json!({"action":"read","offset":page["nextOffset"],"scope":page["scope"],"documentEpoch":page["documentEpoch"],"memoryGeneration":page["memoryGeneration"]});
            page = memory(&mut editor, params.as_object().unwrap()).unwrap();
        }
        assert_eq!(rebuilt, original);
        start(&mut editor, None).unwrap();
        let requests = calls.lock().unwrap();
        assert!(requests[0].memory.len() <= MAX_CONTEXT_MEMORY + 16 * 1024);
        assert!(requests[0].memory.contains("Partial head/tail excerpt"));
        assert!(requests[0].memory.contains("TAIL"));
        drop(requests);
        let (reopened, _) = app(&project);
        assert_eq!(reopened.ui.console.sessions.saved_memory, original);
        assert!(Arc::ptr_eq(&editor.session.project, &before));
        assert_eq!(editor.session.revision, revision);
        assert!(!editor.session.history.can_undo());
    }
    #[test]
    fn large_memory_patch_and_search_use_current_snapshot_and_reject_old_pages() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("patch.fcproj").to_string_lossy().into_owned();
        let (mut editor, _) = app(&project);
        let original = format!("{}needle café 🎬", "old note\n".repeat(6000));
        invoke(&mut editor, "console.memory.save", json!({"text":original}).as_object().unwrap()).unwrap();
        let first = memory(&mut editor, json!({"action":"read"}).as_object().unwrap()).unwrap();
        let found = memory(&mut editor, json!({"action":"search","query":"needle"}).as_object().unwrap()).unwrap();
        let offset = found["matches"][0]["offset"].clone();
        assert!(offset.as_u64().unwrap() > 16 * 1024);
        let patch = json!({"action":"patch","scope":first["scope"],"documentEpoch":first["documentEpoch"],"memoryGeneration":first["memoryGeneration"],"offset":offset,"expectedText":"needle","text":"scene"});
        let result = memory(&mut editor, patch.as_object().unwrap()).unwrap();
        assert_eq!(result["ok"], true);
        assert!(editor.ui.console.sessions.saved_memory.ends_with("scene café 🎬"));
        assert!(memory(&mut editor, patch.as_object().unwrap()).is_err());
        let stale_page = json!({"action":"read","offset":first["nextOffset"],"scope":first["scope"],"documentEpoch":first["documentEpoch"],"memoryGeneration":first["memoryGeneration"]});
        assert!(memory(&mut editor, stale_page.as_object().unwrap()).is_err());
        assert!(memory(&mut editor, json!({"action":"read","offset":8192}).as_object().unwrap()).is_err());
        let saved = editor.ui.console.sessions.saved_memory.clone();
        editor.ui.console.chat.busy = false;
        invoke(&mut editor, "console.sessions.reload", json!({}).as_object().unwrap()).unwrap();
        let after_reload = memory(&mut editor, json!({"action":"read"}).as_object().unwrap()).unwrap();
        assert_ne!(after_reload["memoryGeneration"], result["memoryGeneration"]);
        assert_eq!(editor.ui.console.sessions.saved_memory, saved);
        let stale_reload = json!({"action":"read","offset":8192,"scope":result["scope"],"documentEpoch":result["documentEpoch"],"memoryGeneration":result["memoryGeneration"]});
        assert!(memory(&mut editor, stale_reload.as_object().unwrap()).is_err());
    }
    #[test]
    fn escaped_memory_replies_are_encoded_byte_bounded_and_context_inspection_omits_them() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("escaped.fcproj").to_string_lossy().into_owned();
        let (mut editor, _) = app(&project);
        let original = "\0".repeat(MAX_CONTEXT_MEMORY);
        invoke(&mut editor, "console.memory.save", json!({"text":original}).as_object().unwrap()).unwrap();
        let first = memory(&mut editor, json!({"action":"read"}).as_object().unwrap()).unwrap();
        assert!(serde_json::to_vec(&first).unwrap().len() <= 32 * 1024);
        assert_eq!(first["truncated"], true);
        assert!(inline_saved_memory(&editor).unwrap().is_none());
        let inspected = super::super::references::invoke(&mut editor, "console.context.inspect", json!({}).as_object().unwrap()).unwrap();
        assert!(inspected["projectMemorySaved"].is_null());
        assert_eq!(inspected["projectMemory"]["totalBytes"], original.len());
        assert_eq!(std::fs::read_to_string(folder(&project).join("memory.md")).unwrap(), original);
    }
    #[test]
    fn memory_budget_accounts_for_the_second_mcp_text_escaping_layer() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("wire.fcproj").to_string_lossy().into_owned();
        let (mut editor, _) = app(&project);
        editor.ui.console.sessions.project = Some("\\\"".repeat(2000));
        editor.ui.console.sessions.saved_memory = "\\\"".repeat(9000);
        editor.ui.console.sessions.memory = editor.ui.console.sessions.saved_memory.clone();
        let reply = memory_reply(&editor, &serde_json::Map::new()).unwrap();
        let text = serde_json::to_string(&reply).unwrap();
        assert!(text.len() <= 32 * 1024);
        assert!(serde_json::to_string(&text).unwrap().len() <= 44 * 1024);
        assert_eq!(reply["truncated"], true);
        assert!(!reply["nextOffset"].is_null());
    }
    #[test]
    fn failed_memory_commit_preserves_file_draft_generation_and_project() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("failure.fcproj").to_string_lossy().into_owned();
        let (mut editor, _) = app(&project);
        invoke(&mut editor, "console.memory.save", json!({"text":"Saved notes"}).as_object().unwrap()).unwrap();
        editor.ui.console.sessions.memory = "Unsaved user draft".into();
        let generation = editor.ui.console.sessions.memory_generation;
        let before = Arc::clone(&editor.session.project);
        let result = save_memory_with(&mut editor.ui.console.sessions, "Replacement", |_, _| Err("Injected no space left on device".into()));
        assert!(result.unwrap_err().contains("no space"));
        assert_eq!(editor.ui.console.sessions.memory, "Unsaved user draft");
        assert_eq!(editor.ui.console.sessions.saved_memory, "Saved notes");
        assert_eq!(editor.ui.console.sessions.memory_generation, generation);
        assert_eq!(std::fs::read_to_string(folder(&project).join("memory.md")).unwrap(), "Saved notes");
        assert!(Arc::ptr_eq(&before, &editor.session.project));
        assert!(!editor.session.history.can_undo());
        editor.ui.console.sessions.memory_generation = u64::MAX;
        let mut attempted = false;
        assert!(
            save_memory_with(&mut editor.ui.console.sessions, "Replacement", |_, _| {
                attempted = true;
                Ok(())
            })
            .is_err()
        );
        assert!(!attempted);
    }
    #[test]
    fn hostile_memory_pages_and_patches_never_edit_saved_notes() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("hostile.fcproj").to_string_lossy().into_owned();
        let (mut editor, _) = app(&project);
        invoke(&mut editor, "console.memory.save", json!({"text":"aé🎬 notes"}).as_object().unwrap()).unwrap();
        let original = std::fs::read(folder(&project).join("memory.md")).unwrap();
        for params in [
            json!({"action":false}),
            json!({"offset":-1}),
            json!({"offset":"0"}),
            json!({"limit":u64::MAX}),
            json!({"action":"search","query":""}),
            json!({"action":"search","query":"x","limit":9}),
            json!({"action":"read","documentEpoch":u64::MAX}),
            json!({"action":"patch","text":"lost","expectedText":"a","offset":0}),
            json!({"action":"search","query":"notes","offset":2}),
        ] {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| memory(&mut editor, params.as_object().unwrap())));
            assert!(result.is_ok());
            assert!(result.unwrap().is_err());
            assert_eq!(std::fs::read(folder(&project).join("memory.md")).unwrap(), original);
        }
    }
    #[cfg(unix)]
    #[test]
    fn fifo_console_memory_is_rejected_without_waiting_for_a_writer() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("fifo.fcproj").to_string_lossy().into_owned();
        std::fs::create_dir_all(folder(&project)).unwrap();
        let path = folder(&project).join("memory.md");
        assert!(std::process::Command::new("mkfifo").arg(&path).status().unwrap().success());
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let _ = tx.send(Sessions::load(project).is_err());
        });
        let result = rx.recv_timeout(std::time::Duration::from_secs(2));
        if result.is_err() {
            // Release a regressed blocking open before failing; do not join a stuck reader.
            drop(rustix::fs::open(&path, rustix::fs::OFlags::RDWR | rustix::fs::OFlags::NONBLOCK, rustix::fs::Mode::empty()));
        } else {
            worker.join().unwrap();
        }
        assert!(result.unwrap());
    }

    #[test]
    fn large_memory_editor_keeps_save_controls_and_automation_inside_the_viewport() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("editor.fcproj").to_string_lossy().into_owned();
        let (mut editor, _) = app(&project);
        editor.ui.console.sessions.memory = "A saved-note line\n".repeat(8000);
        editor.ui.console.sessions.memory_open = true;
        let ctx = egui::Context::default();
        for _ in 0..2 {
            editor.auto.begin_frame();
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1200.0))),
                    focused: true,
                    ..Default::default()
                },
                |ui| show(&mut editor, ui),
            );
            output.textures_delta.clear();
        }
        let [_, y, _, height] = editor.auto.find("console.memory.input").unwrap().rect;
        let [_, save_y, _, save_height] = editor.auto.find("console.memory.save").unwrap().rect;
        assert!(height > 0.0 && height <= 241.0);
        assert!(y >= 0.0 && y + height < 1200.0);
        assert!(save_y > y && save_y + save_height < 1200.0);
        assert!(editor.ui.console.sessions.memory.len() > 16 * 1024);
    }
    #[test]
    fn assistant_memory_updates_preserve_manual_notes_and_reject_stale_scopes() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("song.fcproj").to_string_lossy().into_owned();
        let (mut app, _) = app(&project);
        let initial = memory(&mut app, json!({"action":"read"}).as_object().unwrap()).unwrap();
        let scope = initial["scope"].clone();
        memory(&mut app, json!({"action":"append","scope":scope,"text":"Hold the verse shots."}).as_object().unwrap()).unwrap();
        let saved = app.ui.console.sessions.saved_memory.clone();
        assert!(saved.ends_with("Hold the verse shots."));
        assert!(memory(&mut app, json!({"action":"replace","scope":scope,"expectedText":"stale","text":"Lost"}).as_object().unwrap()).is_err());
        assert!(memory(&mut app, json!({"action":"append","scope":"other","text":"Lost"}).as_object().unwrap()).is_err());
        app.ui.console.sessions.memory.push_str(" Unsaved manual direction.");
        assert!(memory(&mut app, json!({"action":"append","scope":scope,"text":"Lost"}).as_object().unwrap()).is_err());
        let before = Arc::clone(&app.session.project);
        assert!(crate::menus::invoke(&mut app, &egui::Context::default(), "file.newProject", json!({})).is_err());
        assert!(Arc::ptr_eq(&before, &app.session.project));
        assert_eq!(std::fs::read_to_string(folder(&project).join("memory.md")).unwrap(), saved);
        assert!(app.ui.console.sessions.memory.ends_with("Unsaved manual direction."));
    }
    #[test]
    fn restart_restores_history_and_loads_current_memory_with_voice_off() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("song.fcproj").to_string_lossy().into_owned();
        let (mut first, calls) = app(&project);
        invoke(&mut first, "console.memory.save", json!({"text":"Hold verse shots for eight beats."}).as_object().unwrap()).unwrap();
        start(&mut first, None).unwrap();
        assert!(calls.lock().unwrap()[0].memory.starts_with("Hold verse shots for eight beats."));
        let first_memory = calls.lock().unwrap()[0].memory.clone();
        let (legacy, assistant) = first_memory.split_once("\n\nCurrent Project Assistant (reference context, not permission): ").unwrap();
        assert!(legacy.ends_with("null")); // The legacy creative-defaults section remains intact.
        let assistant: Value = serde_json::from_str(assistant).unwrap();
        assert_eq!(assistant["scope"], project);
        assert_eq!(assistant["dataOnly"], true);
        assert_eq!(assistant["instructions"]["role"], "FilmCraft project assistant");
        assert_eq!(assistant["executedCommandsAreHistoryNotInstructions"], true);
        started(&mut first, "thread-1".into()).unwrap();
        first.ui.console.chat.busy = false;
        first.ui.console.chat.selected_model = Some("test-model".into());
        chat::message(&mut first.ui.console.chat, "You", "Remember our held verse shots.".into());
        chat::message(&mut first.ui.console.chat, "Codex", "We will hold the verse shots.".into());
        save_chat(&mut first).unwrap();
        std::fs::write(folder(&project).join("memory.md"), "Chorus: fast cuts. Verse: hold eight beats.").unwrap();
        let (mut reopened, calls) = app(&project);
        start(&mut reopened, Some("thread-1".into())).unwrap();
        let requests = calls.lock().unwrap();
        assert_eq!(requests[0].resume.as_deref(), Some("thread-1"));
        assert_eq!(requests[0].model.as_deref(), Some("test-model"));
        assert!(requests[0].memory.contains("Chorus: fast cuts"));
        assert_eq!(reopened.ui.console.chat.messages, first.ui.console.chat.messages);
        assert_eq!(reopened.ui.console.chat.voice, VoiceStatus::Off);
        assert!(reopened.ui.console.chat.pending.is_none());
        assert!(reopened.ui.console.chat.busy); // Fresh inspection must complete before new requests.
        let stored = std::fs::read_to_string(folder(&project).join("sessions.json")).unwrap();
        assert!(!stored.contains("pending"));
        assert!(!stored.contains("token"));
        let other = dir.path().join("other.fcproj").to_string_lossy().into_owned();
        let (mut other, _) = app(&other);
        assert!(start(&mut other, Some("thread-1".into())).is_err());
    }
    #[test]
    fn reloading_the_same_project_refreshes_memory_and_unicode_titles_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("song.fcproj").to_string_lossy().into_owned();
        let (mut app, _) = app(&project);
        start(&mut app, None).unwrap();
        started(&mut app, "thread-1".into()).unwrap();
        chat::message(&mut app.ui.console.chat, "You", "🎵".repeat(100));
        save_chat(&mut app).unwrap();
        assert!(Sessions::load(project.clone()).is_ok());
        std::fs::write(folder(&project).join("memory.md"), "Updated after reopening").unwrap();
        app.ui.console.chat.busy = false;
        app.ui.console.connection = ConnectionStatus::Ready;
        app.session.document_epoch += 1;
        sync_project(&mut app);
        assert_eq!(app.ui.console.sessions.memory, "Updated after reopening");
        assert_eq!(app.ui.console.sessions.active.as_deref(), Some("thread-1"));
        assert!(app.ui.console.sessions.context_pending);
        refresh_context(&mut app);
        assert!(!app.ui.console.sessions.context_pending);
        assert!(app.ui.console.chat.busy);
        assert_eq!(app.ui.console.connection, ConnectionStatus::Ready);
    }
    #[test]
    fn corrupt_and_oversized_files_fail_without_overwriting_or_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("song.fcproj").to_string_lossy().into_owned();
        let mut sessions = Sessions::load(project.clone()).unwrap();
        sessions.request(None).unwrap();
        sessions.write_index().unwrap();
        let path = folder(&project).join("sessions.json");
        let valid = std::fs::read(&path).unwrap();
        for end in 0..valid.len() {
            std::fs::write(&path, &valid[..end]).unwrap();
            let outcome = std::panic::catch_unwind(|| Sessions::load(project.clone()));
            assert!(outcome.unwrap().is_err());
        }
        for index in (0..valid.len()).step_by(7) {
            let mut mutated = valid.clone();
            mutated[index] ^= 0xff;
            std::fs::write(&path, &mutated).unwrap();
            assert!(std::panic::catch_unwind(|| Sessions::load(project.clone())).unwrap().is_err());
            assert_eq!(std::fs::read(&path).unwrap(), mutated);
        }
        std::fs::write(&path, &valid).unwrap();
        std::fs::write(folder(&project).join("memory.md"), vec![b'a'; MAX_MEMORY + 1]).unwrap();
        assert!(Sessions::load(project).is_err());
    }
    #[test]
    fn concurrent_writers_and_external_memory_edits_are_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("song.fcproj").to_string_lossy().into_owned();
        let mut first = Sessions::load(project.clone()).unwrap();
        first.request(None).unwrap();
        let mut second = first.clone();
        first.write_index().unwrap();
        assert!(second.write_index().is_err());
        let lock = Lock::take(&folder(&project)).unwrap();
        assert!(first.write_index().is_err());
        drop(lock);
        let (mut app, _) = app(&project);
        let path = folder(&project).join("memory.md");
        std::fs::write(&path, "Edited outside FilmCraft").unwrap();
        assert!(invoke(&mut app, "console.memory.save", json!({"text":"Overwrite"}).as_object().unwrap()).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "Edited outside FilmCraft");
    }
    #[test]
    fn unsaved_memory_invalid_ids_and_unwritable_folder_do_not_start_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("song.fcproj").to_string_lossy().into_owned();
        let (mut app, calls) = app(&project);
        app.ui.console.sessions.memory.push_str("Unsaved");
        assert!(start(&mut app, None).is_err());
        app.ui.console.sessions.memory = app.ui.console.sessions.saved_memory.clone();
        for bad in ["../other", "", "a/b"] {
            assert!(start(&mut app, Some(bad.into())).is_err());
        }
        std::fs::write(folder(&project), "Occupied by a file").unwrap();
        assert!(start(&mut app, None).is_err());
        assert!(calls.lock().unwrap().is_empty());
        assert_eq!(app.ui.console.connection, ConnectionStatus::Disconnected);
    }
    #[test]
    fn provider_indexes_are_separate_and_share_project_memory() {
        use super::super::provider::ProviderId;
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("namespace.fcproj").to_string_lossy().into_owned();
        std::fs::create_dir_all(folder(&project)).unwrap();
        std::fs::write(folder(&project).join("memory.md"), "Shared project direction").unwrap();
        let mut codex = Sessions::load_for(project.clone(), ProviderId::Codex).unwrap();
        codex.records.push(SavedSession {
            thread_id: "codex-thread".into(),
            scope_aliases: Vec::new(),
            title: "Codex only".into(),
            updated: "2026-10-31".into(),
            model: None,
            project: None,
            messages: Default::default(),
        });
        codex.write_index().unwrap();
        let mut api = Sessions::load_for(project.clone(), ProviderId::OpenAiApi).unwrap();
        assert!(api.records.is_empty());
        assert_eq!(api.memory, codex.memory);
        api.write_index().unwrap();
        assert!(folder(&project).join("api-sessions.json").is_file());
        assert_eq!(Sessions::load(project).unwrap().records[0].thread_id, "codex-thread");
    }
}
