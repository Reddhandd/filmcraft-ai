//! Saved-note persistence, bounded retrieval and snapshot-guarded edits.
use super::*;

fn optional_u64(p: &serde_json::Map<String, Value>, key: &str) -> Result<Option<u64>, String> {
    p.get(key).filter(|v| !v.is_null()).map(|v| v.as_u64().ok_or_else(|| format!("{key} must be a nonnegative integer"))).transpose()
}
fn memory_offset(p: &serde_json::Map<String, Value>) -> Result<usize, String> {
    usize::try_from(optional_u64(p, "offset")?.unwrap_or(0)).map_err(|_| "Memory offset is too large".into())
}
fn memory_limit(p: &serde_json::Map<String, Value>, default: usize) -> Result<usize, String> {
    optional_u64(p, "limit")?.map(|v| usize::try_from(v).map_err(|_| "Memory limit is too large".into())).transpose().map(|v| v.unwrap_or(default))
}
pub(crate) fn inline_saved_memory(app: &FilmcraftApp) -> Result<Option<&str>, String> {
    let text = saved_memory(app);
    if text.len() > MAX_CONTEXT_MEMORY {
        return Ok(None);
    }
    let encoded = serde_json::to_string(text).map_err(|e| e.to_string())?;
    Ok((encoded.len() <= MAX_CONTEXT_MEMORY).then_some(text))
}
pub(crate) fn memory_metadata(app: &FilmcraftApp) -> Value {
    json!({"totalBytes":app.ui.console.sessions.saved_memory.len(),"maxBytes":MAX_MEMORY,"contextMaxBytes":MAX_CONTEXT_MEMORY,"memoryGeneration":app.ui.console.sessions.memory_generation,"documentEpoch":app.session.document_epoch,"readTool":"console_memory","unsavedDraft":has_unsaved_memory(app)})
}
fn memory_guard(app: &FilmcraftApp, p: &serde_json::Map<String, Value>, required: bool) -> Result<(), String> {
    let sessions = &app.ui.console.sessions;
    if let Some(error) = &sessions.error {
        return Err(format!("Project memory is unavailable: {error}"));
    }
    let scope = sessions.project.as_deref().ok_or("Local project memory is unavailable")?;
    if (required || p.get("scope").is_some_and(|v| !v.is_null())) && p.get("scope").and_then(Value::as_str) != Some(scope) {
        return Err("Memory scope changed; read current memory before continuing".into());
    }
    let epoch = optional_u64(p, "documentEpoch")?;
    let generation = optional_u64(p, "memoryGeneration")?;
    if (required && (epoch.is_none() || generation.is_none()))
        || epoch.is_some_and(|v| v != app.session.document_epoch)
        || generation.is_some_and(|v| v != sessions.memory_generation)
    {
        return Err("Memory snapshot changed; read offset 0 again before continuing".into());
    }
    Ok(())
}
fn memory_response_fits(value: &Value) -> Result<bool, String> {
    let text = serde_json::to_string(value).map_err(|e| e.to_string())?;
    if text.len() > 32 * 1024 {
        return Ok(false);
    }
    // MCP wraps this JSON in a text string. Account for that second escaping layer,
    // reserving space below the 48 KiB tool budget for the content/result envelope.
    Ok(serde_json::to_string(&text).map_err(|e| e.to_string())?.len() <= 44 * 1024)
}
pub(super) fn memory_reply(app: &FilmcraftApp, p: &serde_json::Map<String, Value>) -> Result<Value, String> {
    let offset = memory_offset(p)?;
    memory_guard(app, p, offset > 0)?;
    let sessions = &app.ui.console.sessions;
    let mut limit = memory_limit(p, memory_text::MAX_PAGE_BYTES)?;
    if !(4..=memory_text::MAX_PAGE_BYTES).contains(&limit) {
        return Err("Memory page limit must be 4–8192 UTF-8 bytes".into());
    }
    let mut whole = p.get("limit").is_none_or(Value::is_null) && offset == 0 && sessions.saved_memory.len() <= MAX_CONTEXT_MEMORY;
    for _ in 0..16 {
        let page = if whole {
            memory_text::Page { text: &sessions.saved_memory, next_offset: None }
        } else {
            memory_text::page(&sessions.saved_memory, offset, limit)?
        };
        let mut value = memory_metadata(app);
        let object = value.as_object_mut().ok_or("Invalid memory response")?;
        object.insert("scope".into(), json!(sessions.project));
        object.insert("text".into(), json!(page.text));
        object.insert("offset".into(), json!(offset));
        object.insert("nextOffset".into(), json!(page.next_offset));
        object.insert("truncated".into(), json!(offset > 0 || page.next_offset.is_some()));
        object.insert("unsavedDraft".into(), json!(has_unsaved_memory(app)));
        object.insert("maxPageBytes".into(), json!(memory_text::MAX_PAGE_BYTES));
        if memory_response_fits(&value)? {
            return Ok(value);
        }
        if whole {
            whole = false;
        } else if limit > 4 {
            limit = (limit / 2).max(4);
        } else {
            break;
        }
    }
    Err("Memory response metadata exceeds the safe output budget".into())
}
fn memory_search(app: &FilmcraftApp, p: &serde_json::Map<String, Value>) -> Result<Value, String> {
    let offset = memory_offset(p)?;
    memory_guard(app, p, offset > 0)?;
    let query = p.get("query").and_then(Value::as_str).ok_or("Supply a literal memory search query")?;
    let mut limit = memory_limit(p, 8)?;
    for _ in 0..4 {
        let found = memory_text::search(&app.ui.console.sessions.saved_memory, query, offset, limit)?;
        let mut value = memory_metadata(app);
        let object = value.as_object_mut().ok_or("Invalid memory response")?;
        object.insert("scope".into(), json!(app.ui.console.sessions.project));
        object.insert(
            "matches".into(),
            json!(found.hits.iter().map(|hit| json!({"offset":hit.offset,"snippetOffset":hit.snippet_offset,"text":hit.text})).collect::<Vec<_>>()),
        );
        object.insert("nextOffset".into(), json!(found.next_offset));
        if memory_response_fits(&value)? {
            return Ok(value);
        }
        if limit <= 1 {
            break;
        }
        limit /= 2;
    }
    Err("Memory search response exceeds the safe output budget".into())
}
pub(crate) fn memory(app: &mut FilmcraftApp, p: &serde_json::Map<String, Value>) -> Result<Value, String> {
    sync_project(app);
    let action = match p.get("action").filter(|v| !v.is_null()) {
        None => "read",
        Some(v) => v.as_str().ok_or("Memory action must be a string")?,
    };
    if crate::console::assistants::maintenance_active(app)
        && !matches!(action, "read" | "search" | "assistant_read" | "assistant_history" | "assistant_compact")
    {
        return Err("Memory maintenance cannot change teaching or project notes".into());
    }
    if matches!(action, "assistant_read" | "assistant_history" | "assistant_update" | "assistant_compact") {
        return crate::console::assistants::memory(app, p);
    }
    if action == "read" {
        return memory_reply(app, p);
    }
    if action == "search" {
        return memory_search(app, p);
    }
    memory_guard(app, p, action == "patch")?;
    let sessions = &app.ui.console.sessions;
    let scope = sessions.project.as_deref().ok_or("Local project memory is unavailable")?;
    if p.get("scope").and_then(Value::as_str) != Some(scope) {
        return Err("Memory scope changed; read current memory before updating".into());
    }
    if has_unsaved_memory(app) {
        return Err("The memory editor contains unsaved notes. Save or reload those notes before updating through the assistant".into());
    }
    let text = p.get("text").and_then(Value::as_str).filter(|s| s.len() <= MAX_MEMORY).ok_or("Supply bounded memory text")?;
    let next = match action {
        "append" if !text.trim().is_empty() && text.len() <= 4096 => {
            if sessions.saved_memory.contains(text) {
                let mut value = memory_reply(app, &serde_json::Map::new())?;
                let object = value.as_object_mut().ok_or("Invalid memory response")?;
                object.insert("ok".into(), json!(true));
                object.insert("unchanged".into(), json!(true));
                return Ok(value);
            }
            memory_text::patch(&sessions.saved_memory, sessions.saved_memory.len(), "", &format!("\n\n{text}"))?
        }
        "replace" if !text.trim().is_empty() && p.get("expectedText").and_then(Value::as_str) == Some(sessions.saved_memory.as_str()) => text.to_owned(),
        "patch" => {
            let expected = p.get("expectedText").and_then(Value::as_str).ok_or("Supply the exact current passage as expectedText")?;
            if optional_u64(p, "offset")?.is_none() {
                return Err("Supply the memory patch byte offset".into());
            }
            memory_text::patch(&sessions.saved_memory, memory_offset(p)?, expected, text)?
        }
        _ => return Err("Use append (at most 4096 bytes), replace with exact expectedText, or a snapshot-guarded patch".into()),
    };
    invoke(app, "console.memory.save", json!({"text":next}).as_object().ok_or("Invalid memory update")?)?;
    let mut value = memory_reply(app, &serde_json::Map::new())?;
    value.as_object_mut().ok_or("Invalid memory response")?.insert("ok".into(), json!(true));
    Ok(value)
}
pub(super) fn save_memory_with(sessions: &mut Sessions, text: &str, write: impl FnOnce(&std::path::Path, &[u8]) -> Result<(), String>) -> Result<(), String> {
    if text.len() > MAX_MEMORY {
        return Err("Memory must contain at most 1 MiB (1048576 UTF-8 bytes)".into());
    }
    if sessions.error.is_some() {
        return Err("Repair the saved console files before saving memory".into());
    }
    let generation = sessions.memory_generation.checked_add(1).ok_or("Memory generation exhausted; reopen the project")?;
    let project = sessions.project.as_ref().ok_or("Save this project before saving memory")?;
    let dir = folder(project);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create console folder: {e}"))?;
    let _lock = Lock::take(&dir)?;
    let path = dir.join("memory.md");
    if read(&path, MAX_MEMORY)?.is_some_and(|old| old != sessions.saved_memory) {
        return Err("Memory changed on disk; reload the project before saving to avoid overwriting it".into());
    }
    // Prepare both fallible cache allocations before committing the file. A memory
    // allocation failure must leave the old file, unsaved draft and generation intact.
    let mut draft = String::new();
    draft.try_reserve_exact(text.len()).map_err(|_| "Not enough memory to save notes; the original file and draft are unchanged")?;
    draft.push_str(text);
    let mut saved = String::new();
    saved.try_reserve_exact(text.len()).map_err(|_| "Not enough memory to save notes; the original file and draft are unchanged")?;
    saved.push_str(text);
    write(&path, text.as_bytes())?;
    sessions.memory = draft;
    sessions.saved_memory = saved;
    sessions.memory_generation = generation;
    Ok(())
}
