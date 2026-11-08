use super::*;
use serde_json::{Value, json};
use std::path::PathBuf;
pub(super) fn root(app: &FilmcraftApp) -> Result<PathBuf, String> {
    app.session
        .persistence
        .as_ref()
        .map(|p| p.data_dir.clone())
        .or_else(|| app.session.prefs_path.as_ref().and_then(|p| p.parent()).map(|p| p.to_path_buf()))
        .map(|path| path.join("Console").join("Assistants"))
        .ok_or_else(|| "A local data directory is required for saved assistants".into())
}
fn read(dir: &std::path::Path) -> Result<library::Library, String> {
    catalog_store::read(dir)
}
pub(super) fn reload(app: &mut FilmcraftApp) -> Result<(), String> {
    let dir = root(app)?;
    let library = read(&dir)?;
    app.assistants.library = Some(library);
    app.assistants.library_dir = Some(dir);
    Ok(())
}
pub(super) fn ensure(app: &mut FilmcraftApp) -> Result<(), String> {
    if app.assistants.library.is_none() || app.assistants.library_dir.as_ref() != Some(&root(app)?) {
        reload(app)?;
    }
    Ok(())
}
pub(super) fn invoke(app: &mut FilmcraftApp, id: &str, p: &serde_json::Map<String, Value>) -> Result<Value, String> {
    poll(app)?;
    if !matches!(id, "console.assistant.inspect" | "console.assistant.history" | "console.assistant.review")
        && app.assistants.current.as_ref().and_then(|runtime| runtime.autosave_state.as_ref()).is_some_and(|state| state.pending())
    {
        return Err("Wait for the pending assistant save before switching or updating assistants".into());
    }
    match id {
        "console.assistant.memoryHistory" => {
            let runtime = app.assistants.current.as_ref().ok_or("Open a project before reading memory originals")?;
            Ok(json!({"originals":runtime.memory_originals()?,"retainedOriginals":10}))
        }
        "console.assistant.memoryRestore" => {
            let runtime = app.assistants.current.as_mut().ok_or("Open a project before restoring working notes")?;
            let current = runtime.snapshot();
            if p.get("confirm").and_then(Value::as_bool) != Some(true)
                || p.get("scope").and_then(Value::as_str) != Some(current.project_scope.as_str())
                || p.get("documentEpoch").and_then(Value::as_u64) != Some(current.document_epoch)
                || p.get("memoryGeneration").and_then(Value::as_u64) != Some(current.context_version)
            {
                return Err("Confirm the current project and memory snapshot before restoring working notes".into());
            }
            let id = p.get("original").and_then(Value::as_u64).filter(|id| *id > 0).ok_or("Choose a retained memory original")?;
            runtime.restore_original(id, Instant::now())?;
            milestone(app);
            Ok(json!({"restored":true,"projectEditsReplayed":false,"standingTeachingPreserved":true}))
        }

        "console.assistant.history" => {
            reload(app)?;
            let dir = root(app)?;
            match p.get("revision") {
                None => Ok(json!({"revisions":super::catalog_history::list(&dir)?,"retainedVersions":20})),
                Some(value) => {
                    let revision = value.as_u64().filter(|id| *id > 0).ok_or("Choose a positive assistant revision")?;
                    let name = p.get("name").and_then(Value::as_str).ok_or("Choose an assistant to preview")?;
                    let library = super::catalog_history::load(&dir, revision)?;
                    Ok(json!({"revision":revision,"name":name,"instructions":library.instructions(name)?}))
                }
            }
        }
        "console.assistant.restore" => {
            if p.get("confirm").and_then(Value::as_bool) != Some(true) {
                return Err("Explicitly confirm restoring the reviewed assistant version".into());
            }
            let name = p.get("name").and_then(Value::as_str).ok_or("Choose a saved assistant")?;
            let revision = p.get("revision").and_then(Value::as_u64).filter(|id| *id > 0).ok_or("Choose a positive assistant revision")?;
            let expected: Instructions = serde_json::from_value(p.get("expectedInstructions").cloned().ok_or("Review current instructions before restoring")?)
                .map_err(|_| "Invalid expected assistant instructions")?;
            let current = app.assistants.current.as_ref().ok_or("Open a project before restoring its assistant")?.snapshot();
            if current.assistant_name.as_deref() != Some(name)
                || p.get("scope").and_then(Value::as_str) != Some(current.project_scope.as_str())
                || p.get("documentEpoch").and_then(Value::as_u64) != Some(current.document_epoch)
                || p.get("memoryGeneration").and_then(Value::as_u64) != Some(current.context_version)
            {
                return Err("Current project or teaching changed; review the revision again".into());
            }
            let mut checkpoint = current.clone();
            checkpoint.context_version = checkpoint.context_version.checked_add(1).ok_or("Assistant context version exhausted")?;
            let dir = root(app)?;
            let restored = super::catalog_history::load(&dir, revision)?;
            checkpoint.assistant = restored.instructions(name)?;
            checkpoint.reference_documents = restored.reference_selection(name)?;
            if let Some(settings) = &mut checkpoint.autosave {
                settings.expected = checkpoint.assistant.clone();
                settings.saved_at = chrono::Utc::now().to_rfc3339();
            }
            checkpoint.encode()?;
            let next = catalog_store::restore(&dir, name, revision, &expected)?;
            app.assistants.library = Some(next);
            app.assistants.current.as_mut().ok_or("Project changed during assistant restore")?.update(checkpoint, Instant::now())?;
            changed(app);
            milestone(app);
            Ok(json!({"restored":true,"name":name,"revision":revision,"projectEditsReplayed":false}))
        }
        "console.assistant.review" => {
            let runtime = app.assistants.current.as_mut().ok_or("Open a project before reviewing assistant teaching")?;
            let current = runtime.snapshot();
            if p.get("scope").and_then(Value::as_str) != Some(current.project_scope.as_str())
                || p.get("documentEpoch").and_then(Value::as_u64) != Some(current.document_epoch)
                || p.get("memoryGeneration").and_then(Value::as_u64) != Some(current.context_version)
            {
                return Err("Assistant teaching changed; refresh the review before applying it".into());
            }
            let teaching = serde_json::from_value::<super::preservation::Teaching>(p.get("teaching").cloned().ok_or("Choose a teaching entry to review")?)
                .map_err(|_| "Invalid teaching identity")?;
            let action = p.get("action").and_then(Value::as_str).ok_or("Choose accept, reject or forget")?;
            let mut next = current.clone();
            next.assistant = super::preservation::review(&next.assistant, &teaching, action)?;
            next.context_version = next.context_version.checked_add(1).ok_or("Assistant context version exhausted")?;
            runtime.update(next, Instant::now())?;
            milestone(app);
            changed(app);
            Ok(json!({"reviewed":true,"action":action,"checkpointRequested":true}))
        }
        "console.assistant.inspect" => {
            reload(app)?;
            Ok(
                json!({"maintenance":super::maintenance::diagnostics(app,Instant::now()),"names":app.assistants.library.as_ref().map(|library|library.assistants.iter().map(|a|a.name.clone()).collect::<Vec<_>>()),
                "selected":app.assistants.current.as_ref().and_then(|r|r.snapshot().assistant_name.clone()),"recoveredPrevious":app.assistants.current.as_ref().is_some_and(|r|r.recovered_previous),
                "autosaveEnabled":app.assistants.current.as_ref().is_some_and(|r|r.snapshot().autosave.is_some()),
                "assistantSavedAt":app.assistants.current.as_ref().and_then(|r|r.snapshot().autosave.as_ref()).map(|s|s.saved_at.clone()),
                "autosavePending":app.assistants.current.as_ref().and_then(|r|r.autosave_state.as_ref()).is_some_and(|s|s.pending()),
                "autosaveError":app.assistants.current.as_ref().and_then(|r|r.autosave_state.as_ref()).and_then(|s|s.error.clone()),"error":app.assistants.error}),
            )
        }
        "console.assistant.autosave" => {
            let enabled = p.get("enabled").and_then(Value::as_bool).ok_or("enabled must be boolean")?;
            reload(app)?;
            let runtime = app.assistants.current.as_mut().ok_or("Open a project before changing Autosave")?;
            if runtime.autosave_state.as_ref().is_some_and(|state| state.pending()) {
                return Err("Wait for the pending assistant save before changing Autosave".into());
            }
            let mut checkpoint = runtime.snapshot().clone();
            checkpoint.autosave = if enabled {
                let name = checkpoint.assistant_name.as_deref().ok_or("Save or select a named assistant before enabling Autosave")?;
                let expected = app.assistants.library.as_ref().ok_or("Assistant library unavailable")?.instructions(name)?;
                Some(AssistantAutosave { expected, saved_at: String::new() })
            } else {
                None
            };
            runtime.update(checkpoint, Instant::now())?;
            runtime.autosave_state = None;
            milestone(app);
            Ok(json!({"enabled":enabled,"intervalSeconds":900,"projectHistoryExported":false}))
        }
        "console.assistant.save" => {
            let name = p.get("name").and_then(Value::as_str).ok_or("Name the assistant before saving")?;
            let replace = p.get("replace").map(|v| v.as_bool().ok_or("replace must be boolean")).transpose()?.unwrap_or(false);
            let mut instructions = app.assistants.current.as_ref().ok_or("Open a project before saving its assistant")?.snapshot().assistant.clone();
            if let Some(style) = super::super::references::active(app)?
                && !instructions.preferences.contains(&style.description)
            {
                if instructions.preferences.len() >= 64 {
                    return Err("Assistant preference list is full; review it before saving".into());
                }
                instructions.preferences.push(style.description);
            }
            let dir = root(app)?;
            let expected: Option<Instructions> = if replace {
                let value: Instructions =
                    serde_json::from_value(p.get("expectedInstructions").cloned().ok_or("Inspect the existing assistant before updating it")?)
                        .map_err(|e| e.to_string())?;
                Some(value)
            } else {
                None
            };
            let expected_references = serde_json::from_value(p.get("expectedReferenceDocuments").cloned().unwrap_or(Value::Null))
                .map_err(|e| format!("Invalid expected reference selection: {e}"))?;
            let selected_references =
                app.assistants.current.as_ref().ok_or("Open a project before saving its references")?.snapshot().reference_documents.clone();
            let next = catalog_store::save_with_selection(
                &dir,
                name,
                instructions,
                expected.as_ref(),
                Some(catalog_store::SelectionUpdate { expected: expected_references, selected: selected_references }),
            )?;
            let saved_instructions = next.instructions(name.trim())?;
            app.assistants.library = Some(next);
            app.assistants.library_dir = Some(dir);
            let runtime = app.assistants.current.as_mut().ok_or("Project changed after saving assistant; reload its checkpoint")?;
            runtime.autosave_state = None;
            let mut checkpoint = runtime.snapshot().clone();
            if checkpoint.assistant != saved_instructions {
                checkpoint.context_version = checkpoint.context_version.checked_add(1).ok_or("Assistant context version exhausted; save and reopen")?;
                checkpoint.assistant = saved_instructions.clone();
            }
            if checkpoint.assistant_name.as_deref() == Some(name.trim()) {
                if let Some(settings) = &mut checkpoint.autosave {
                    settings.expected = saved_instructions;
                    settings.saved_at = chrono::Utc::now().to_rfc3339();
                }
            } else {
                checkpoint.autosave = None;
            }
            checkpoint.assistant_name = Some(name.trim().into());
            runtime.update(checkpoint, Instant::now())?;
            milestone(app);
            Ok(json!({"saved":true,"name":name.trim(),"projectHistoryExported":false}))
        }
        "console.assistant.load" | "console.assistant.detach" => {
            reload(app)?;
            let selected = if id.ends_with(".load") { Some(p.get("name").and_then(Value::as_str).ok_or("Choose a saved assistant")?) } else { None };
            let instructions = selected.map(|name| app.assistants.library.as_ref().ok_or("Assistant library unavailable")?.instructions(name)).transpose()?;
            let references =
                selected.map(|name| app.assistants.library.as_ref().ok_or("Assistant library unavailable")?.reference_selection(name)).transpose()?;
            let runtime = app.assistants.current.as_mut().ok_or("Open a project before loading an assistant")?;
            runtime.autosave_state = None;
            let mut checkpoint = runtime.snapshot().clone();
            checkpoint.context_version = checkpoint.context_version.checked_add(1).ok_or("Assistant context version exhausted; save and reopen")?;
            checkpoint.assistant_name = selected.map(str::to_owned);
            checkpoint.autosave = None;
            if let Some(references) = references {
                checkpoint.reference_documents = references;
            }
            if let Some(instructions) = instructions {
                checkpoint.assistant = instructions;
            }
            runtime.update(checkpoint, Instant::now())?;
            milestone(app);
            super::super::sessions::creative_context_changed(app);
            Ok(json!({"loaded":selected,"projectHistoryChanged":false}))
        }
        _ => Err("Unknown project assistant command".into()),
    }
}
