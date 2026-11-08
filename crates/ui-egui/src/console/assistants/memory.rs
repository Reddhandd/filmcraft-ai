//! Structured assistant context shares the bounded memory tool, not executable command dispatch.
use super::*;
use serde_json::{Value, json};
fn number(p: &serde_json::Map<String, Value>, key: &str) -> Result<Option<u64>, String> {
    p.get(key).filter(|v| !v.is_null()).map(|v| v.as_u64().ok_or_else(|| format!("{key} must be a nonnegative integer"))).transpose()
}
pub(super) fn invoke(app: &mut FilmcraftApp, p: &serde_json::Map<String, Value>) -> Result<Value, String> {
    poll(app)?;
    let runtime = app.assistants.current.as_mut().ok_or("Open a project before reading assistant memory")?;
    let history_version = runtime.version();
    let recovered_previous = runtime.recovered_previous;
    let current = runtime.snapshot();
    let action = p.get("action").and_then(Value::as_str).ok_or("Choose an assistant memory action")?;
    let offset = usize::try_from(number(p, "offset")?.unwrap_or(0)).map_err(|_| "Assistant memory offset is too large")?;
    let required = matches!(action, "assistant_update" | "assistant_compact") || offset > 0;
    if (required || p.get("scope").is_some_and(|v| !v.is_null())) && p.get("scope").and_then(Value::as_str) != Some(current.project_scope.as_str()) {
        return Err("Assistant project scope changed; read current assistant memory".into());
    }
    for (key, actual) in
        [("documentEpoch", current.document_epoch), ("memoryGeneration", if action == "assistant_history" { history_version } else { current.context_version })]
    {
        let expected = number(p, key)?;
        if (required && expected.is_none()) || expected.is_some_and(|v| v != actual) {
            return Err("Assistant memory snapshot changed; read offset 0 again".into());
        }
    }
    if action == "assistant_compact" {
        if number(p, "historyGeneration")? != Some(history_version) {
            return Err("Working-memory history changed or its guard is missing; read assistant_history at offset 0 again before summarizing".into());
        }
        let text = p.get("text").and_then(Value::as_str).ok_or("Supply JSON summary and decisions")?;
        runtime.compact(text.as_bytes())?;
        return Ok(
            json!({"queued":true,"persisted":false,"originalRetainedBeforeAdoption":true,"note":"Summary is adopted only after the original is archived and its exact source is rechecked. Read assistant_read for completion status."}),
        );
    }
    if action == "assistant_update" {
        let text = p.get("text").and_then(Value::as_str).ok_or("Supply structured assistant context as JSON text")?;
        let mut next = context_update::apply(
            current,
            current.context_version,
            context_update::Guard { scope: &current.project_scope, document_epoch: current.document_epoch, version: current.context_version },
            text.as_bytes(),
        )?;
        next.context_version = next.context_version.checked_add(1).ok_or("Assistant context version exhausted")?;
        let version = next.context_version;
        runtime.update(next, Instant::now())?;
        milestone(app);
        // The current assistant authored this scoped update and already knows it. It must
        // not block its remaining edits behind a refresh that waits for voice to end.
        // Explicit assistant/style loading and document changes retain their refresh guard.
        return Ok(
            json!({"updated":true,"memoryGeneration":version,"checkpointRequested":true,"persisted":false,"note":"Context updated; durable save is asynchronous. Check checkpoint status before closing."}),
        );
    }
    if !matches!(action, "assistant_read" | "assistant_history") {
        return Err("Use assistant_read, assistant_history or assistant_update".into());
    }
    let text = if action == "assistant_history" {
        String::from_utf8(current.encode()?).map_err(|e| e.to_string())?
    } else {
        serde_json::to_string(&json!({"instructions":current.assistant,"decisions":current.decisions,"pending":current.pending,"errors":current.errors}))
            .map_err(|e| e.to_string())?
    };
    if offset > text.len() || !text.is_char_boundary(offset) {
        return Err("Assistant memory offset must be a UTF-8 boundary within the current context".into());
    }
    let mut limit = usize::try_from(number(p, "limit")?.unwrap_or(8192)).map_err(|_| "Assistant memory page limit is too large")?;
    if !(4..=8192).contains(&limit) {
        return Err("Assistant memory page limit must be 4–8192 bytes".into());
    }
    for _ in 0..16 {
        let mut end = offset.saturating_add(limit).min(text.len());
        while !text.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        let page = text.get(offset..end).ok_or("Assistant memory page changed")?;
        let value = json!({"scope":current.project_scope,"documentEpoch":current.document_epoch,"memoryGeneration":if action=="assistant_history"{history_version}else{current.context_version},"offset":offset,"nextOffset":(end<text.len()).then_some(end),"totalBytes":text.len(),"text":page,"contextGeneration":current.context_version,"historyGeneration":history_version,"checkpointSavedAt":current.saved_at,"selectedAssistant":current.assistant_name,"recoveredPrevious":recovered_previous,"dataOnly":true,"compactionStatus":runtime.compaction_status,"compactionRecommended":super::compaction::due(current)});
        let encoded = serde_json::to_string(&value).map_err(|e| e.to_string())?;
        if encoded.len() <= 32 * 1024 && serde_json::to_string(&encoded).map_err(|e| e.to_string())?.len() <= 44 * 1024 {
            return Ok(value);
        }
        if limit <= 4 {
            break;
        }
        limit = (limit / 2).max(4);
    }
    Err("Assistant memory metadata exceeds its safe response budget".into())
}
