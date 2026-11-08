//! Direct console control uses the app's registry and dispatcher, not a second editing engine.
use super::*;
use std::collections::BTreeMap;
use std::sync::Arc;

pub(super) const MAX_PARAMS: usize = 512 * 1024;
fn reserved(id: &str) -> bool {
    id.starts_with("console.")
        || crate::links::url_for(id).is_some()
        || id == "help.revealLogFiles"
        || matches!(id, "audio.voiceover.start" | "voiceover.recordToggle" | "app.checkForUpdates")
}
fn known(app: &FilmcraftApp, id: &str) -> bool {
    filmcraft_engine::find_command(id).is_some() || menus::external_commands().iter().any(|c| c.id == id) || menus::menu_items(app).iter().any(|c| c.id == id)
}
pub(super) fn catalog(app: &FilmcraftApp, p: &serde_json::Map<String, Value>) -> Result<Value, String> {
    let filter = match p.get("filter") {
        None | Some(Value::Null) => "",
        Some(v) => v.as_str().filter(|s| s.len() <= 256).ok_or("Command filter must contain at most 256 bytes")?,
    }
    .to_lowercase();
    let offset = number(p, "offset", 0, 4096)?;
    let limit = number(p, "limit", 64, 128)?.max(1);
    let enabled = match p.get("enabledOnly") {
        None | Some(Value::Null) => false,
        Some(v) => v.as_bool().ok_or("enabledOnly must be boolean")?,
    };
    let mut commands = BTreeMap::new();
    for spec in filmcraft_engine::command_specs() {
        commands.insert(spec.id.to_owned(),json!({"id":spec.id,"label":spec.label,"params":super::analysis::schema(spec.id).unwrap_or(spec.params),"scope":"engine","enabled":(spec.enabled)(&app.session).is_ok(),"journaled":spec.journal,"consoleAccess":if reserved(spec.id){"explicit user control"}else{"direct"}}));
    }
    for spec in menus::external_commands() {
        commands.entry(spec.id.clone()).or_insert_with(||json!({"id":spec.id,"label":spec.label,"scope":"ui","params":ui_schema(&spec.id),"enabled":true,"consoleAccess":if reserved(&spec.id){"explicit user control"}else{"direct"}}));
    }
    for spec in menus::menu_items(app) {
        commands.entry(spec.id.clone()).or_insert_with(||json!({"id":spec.id,"label":spec.label,"scope":"ui","enabled":spec.enabled,"params":"{}","consoleAccess":if reserved(&spec.id){"explicit user control"}else{"direct"}}));
    }
    let matching: Vec<_> = commands
        .into_values()
        .filter(|c| {
            let matches = c.get("id").and_then(Value::as_str).is_some_and(|s| s.to_lowercase().contains(&filter))
                || c.get("label").and_then(Value::as_str).is_some_and(|s| s.to_lowercase().contains(&filter));
            matches && (!enabled || c.get("enabled").and_then(Value::as_bool) == Some(true))
        })
        .collect();
    Ok(
        json!({"total":matching.len(),"offset":offset,"commands":matching.iter().skip(offset).take(limit).collect::<Vec<_>>(),"nextOffset":(offset.saturating_add(limit)<matching.len()).then_some(offset.saturating_add(limit))}),
    )
}
fn ui_schema(id: &str) -> &'static str {
    match id {
        "console.library.inspect" => r#"{"offset":integer?,"limit":1-16?,"expectedGeneration":integer?} follow nextOffset with generation guard"#,
        "console.library.recover" => "{}",
        "console.library.search" => r#"{"query":string,"limit":1-5?} bounded indexed text excerpts with physical page references; poll pending ticket"#,
        "console.library.status" => r#"{"ticket":integer} poll pending library operation without repeating it"#,
        "console.library.import" => r#"{"path":string} user-selected local PDF/TXT/MD only; native controls, unavailable to model tools"#,
        "console.library.remove" => r#"{"documentId":integer} removes original and indexed text"#,
        "console.library.enable" => r#"{"documentId":integer,"enabled":boolean}"#,
        "console.chat.copy" => r#"{"index":0-based displayed message integer,"selection":optional boolean; copy current selection when available}"#,
        "console.chat.paste" => "{} requests clipboard paste into the message draft; does not send",
        "console.voice.mute" => r#"{"muted":bool?} user's microphone only; omitted toggles"#,
        "console.context.inspect" | "console.attachments.inspect" | "console.styles.inspect" | "console.styles.disable" => "{}",
        "console.attachments.add" | "console.styles.import" | "console.styles.export" => r#"{"path":string}"#,
        "console.attachments.project" => r#"{"item":id,"start":ticks?,"end":ticks?} metadata only; range requires active sequence ID"#,
        "console.attachments.pdf.read" => {
            r#"{"snapshotId":integer,"documentEpoch":integer,"page":1-based-integer,"offset":byte-offset?,"limit":1-4096?} returns pending text ticket"#
        }
        "console.attachments.pdf.status" => r#"{"snapshotId":integer,"documentEpoch":integer,"ticket":integer} consumes completed text result"#,
        "console.attachments.image" => r#"{"snapshotId":integer,"documentEpoch":integer} selected PNG pixels only; no file path"#,
        "console.attachments.remove" => r#"{"index":0-based-integer}"#,
        "console.styles.load" | "console.styles.delete" => r#"{"name":string}"#,
        "console.styles.rename" => r#"{"name":string,"newName":string}"#,
        "console.styles.draft" => r#"{"name":string,"description":string} opens review; does not save"#,
        "console.styles.save" => {
            r#"{"name":string,"description":string,"replace":bool=false,"expectedDescription":string?} review before save; replacement requires exact previous description"#
        }
        "window.dockPanel" => r#"{"panel"?:panelName,"anchor"?:panelName,"zone"?:"center|left|right|top|bottom","before"?:panelName}"#,
        "window.floatPanel" => r#"{"panel"?:panelName,"rect"?:[x,y,width,height]}"#,
        "window.closePanel" => r#"{"panel"?:panelName}"#,
        "view.guideTemplates.apply" => r#"{"name":string,"monitor"?:"source|program"}"#,
        "view.guideTemplates.delete" => r#"{"name":string}"#,
        "window.dockAction" => "Advanced native docking action object; prefer window.dockPanel/window.floatPanel aliases.",
        _ => {
            "{} opens or invokes the native action. Engine commands with the same ID expose their parameter schema instead. A dialog result requires user interaction; opening a dialog does not complete its operation."
        }
    }
}
fn number(p: &serde_json::Map<String, Value>, key: &str, default: usize, max: usize) -> Result<usize, String> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v.as_u64().and_then(|n| usize::try_from(n).ok()).filter(|n| *n <= max).ok_or_else(|| format!("{key} must be an integer from 0 to {max}")),
    }
}
pub(super) fn prepare(app: &FilmcraftApp, command: &str, params: Value) -> Result<Value, String> {
    super::assistants::maintenance_guard(app)?;
    if super::sessions::context_pending(app) && filmcraft_engine::find_command(command).is_none_or(|c| c.journal) {
        let recovery = if app.ui.console.chat.voice.on() {
            "Ask the user to choose Voice Off, then finish or cancel this turn so FilmCraft can refresh."
        } else {
            "Finish or cancel this turn and let FilmCraft refresh the current context."
        };
        return Err(format!(
            "The project or assistant context changed. {recovery} Read-only inspection remains available; inspect the refreshed project before requesting edits."
        ));
    }
    if command.is_empty() || command.len() > 256 || !known(app, command) {
        return Err("Unknown FilmCraft command; discover the live registry with command_list".into());
    }
    if reserved(command) {
        return Err(
            "This command requires the user's explicit application control; the console cannot control its own connection or start microphone capture".into()
        );
    }
    if !params.is_object() || params.to_string().len() > MAX_PARAMS {
        return Err("Command parameters must be a bounded JSON object".into());
    }
    if app.session.is_dirty()
        && (command.starts_with("file.openRecent.")
            || matches!(
                command,
                "file.newProject"
                    | "file.open"
                    | "file.close"
                    | "file.closeProject"
                    | "file.closeAllProjects"
                    | "file.closeAllOtherProjects"
                    | "file.recover"
                    | "file.revert"
                    | "file.openDemoProject"
            ))
    {
        return Err("The current project has unsaved edits. Save it before replacing or closing the project".into());
    }
    if (command.starts_with("file.export") || matches!(command, "file.save" | "file.saveAs" | "file.saveCopy"))
        && let Some(path) = params.get("path")
    {
        let path = path.as_str().filter(|s| !s.is_empty() && s.len() <= 4096).ok_or("Destination path must contain 1–4096 bytes")?;
        let destination = std::path::Path::new(path);
        let current = matches!(command, "file.save" | "file.saveAs") && app.session.path.as_deref().is_some_and(|p| std::path::Path::new(p) == destination);
        if destination.exists() && !current && params.get("overwrite").and_then(Value::as_bool) != Some(true) {
            return Err(
                "Destination already exists. Choose a new path, or supply overwrite:true only when the user explicitly requests replacing that file".into()
            );
        }
    }
    Ok(json!({"command":command,"params":params}))
}
pub(super) fn run(app: &mut FilmcraftApp, ctx: &egui::Context, p: &serde_json::Map<String, Value>) -> Result<Value, String> {
    super::assistants::maintenance_guard(app)?;
    if let Err(error) = super::assistants::poll(app) {
        app.ui.status = error;
    }
    let command = p.get("command").and_then(Value::as_str).filter(|id| known(app, id)).unwrap_or("rejected-command");
    let before = app.session.revision;
    let result = run_inner(app, ctx, p);
    let empty = json!({});
    super::assistants::record(app, command, p.get("params").unwrap_or(&empty), before, &result);
    result
}
fn run_inner(app: &mut FilmcraftApp, ctx: &egui::Context, p: &serde_json::Map<String, Value>) -> Result<Value, String> {
    let command = p.get("command").and_then(Value::as_str).ok_or("Missing command ID")?;
    let params = p.get("params").cloned().unwrap_or_else(|| json!({}));
    prepare(app, command, params.clone())?;
    if matches!(command, "clip.analyzeMusic.apply" | "clip.matchPerformance.apply" | "clip.trimPerformancePassage") {
        let proposal = super::analysis::proposal(app, Value::Null, command, params)?;
        let result = proposal.apply(app, ctx)?;
        return Ok(json!({"status":"executed","command":command,"result":result,"undoAvailable":app.session.history.can_undo()}));
    }
    if matches!(command, "clip.analyzeMusic" | "clip.matchPerformance" | "clip.findPerformancePassages") && params.as_object().is_some_and(|p| !p.is_empty()) {
        return Err("Use console_analyze for native background analysis; do not run analysis synchronously on the UI thread".into());
    }
    if command == "file.exportMedia" && params.get("wait").and_then(Value::as_bool) == Some(true) {
        return Err("Use the controller's long-job route for a blocking export".into());
    }
    let before = Arc::clone(&app.session.project);
    // Legacy IDs remain discoverable, but the Console must never dump a complete timeline.
    let dispatched = match command {
        "sequence.inspect" => "sequence.inspectPage",
        "project.inspect" => "project.inspectPage",
        _ => command,
    };
    let result = menus::invoke(app, ctx, dispatched, params)?;
    if !Arc::ptr_eq(&before, &app.session.project) {
        chat::message(&mut app.ui.console.chat, "Edit", format!("Executed {command}. Undo is available."));
    }
    Ok(json!({"status":"executed","command":command,"result":result,"revision":app.session.revision,"undoAvailable":app.session.history.can_undo()}))
}
fn grouped(command: &str) -> bool {
    // Tracking publishes keyframes asynchronously, outside the project's edit transaction.
    if command == "masks.track" {
        return false;
    }
    if command == "file.newSequence" {
        return true;
    }
    if command.contains("export")
        || command.contains("import")
        || command.contains("preset")
        || command.contains("template")
        || command.contains("render")
        || command.contains("analyze")
        || command.contains("Performance")
    {
        return false;
    }
    ["timeline.", "markers.", "effects.", "captions.", "masks."].iter().any(|prefix| command.starts_with(prefix))
        || matches!(command, "edit.clear" | "edit.rippleDelete" | "edit.duplicate" | "edit.removeAttributes" | "sequence.addTracks" | "sequence.deleteTracks")
}
pub(super) fn batch(app: &mut FilmcraftApp, p: &serde_json::Map<String, Value>) -> Result<Value, String> {
    super::assistants::maintenance_guard(app)?;
    if let Err(error) = super::assistants::poll(app) {
        app.ui.status = error;
    }
    let before = app.session.revision;
    let result = batch_inner(app, p);
    super::assistants::record(app, "command_batch", p, before, &result);
    if result.is_ok() && before != app.session.revision {
        super::assistants::milestone(app);
    }
    result
}
fn batch_inner(app: &mut FilmcraftApp, p: &serde_json::Map<String, Value>) -> Result<Value, String> {
    let steps = p.get("steps").and_then(Value::as_array).filter(|s| !s.is_empty() && s.len() <= 64).ok_or("A batch requires 1–64 steps")?;
    if steps.iter().fold(0usize, |sum, s| sum.saturating_add(s.to_string().len())) > MAX_PARAMS {
        return Err("Batch parameters are too large".into());
    }
    let label = p.get("label").and_then(Value::as_str).unwrap_or("Console edit");
    if label.is_empty() || label.len() > 128 {
        return Err("Undo label must contain 1–128 bytes".into());
    }
    let mut calls = Vec::with_capacity(steps.len());
    for step in steps {
        let map = step.as_object().ok_or("Each batch step must be an object")?;
        if map.keys().any(|k| k != "id" && k != "params") {
            return Err("Unexpected batch step field".into());
        }
        let id = map.get("id").and_then(Value::as_str).ok_or("Missing batch command ID")?;
        if !grouped(id) {
            return Err(format!("{id} runs individually with command_run; it cannot join an atomic project-edit batch"));
        }
        let params = map.get("params").cloned().unwrap_or_else(|| json!({}));
        prepare(app, id, params.clone())?;
        calls.push((id.to_owned(), params));
    }
    app.stop();
    let result = app
        .session
        .edit_group(label, |session| calls.into_iter().map(|(id, params)| session.execute(&id, params)).collect::<filmcraft_engine::Result<Vec<_>>>())
        .map_err(|e| e.to_string())?;
    chat::message(&mut app.ui.console.chat, "Edit", format!("Executed {} steps: {label}. Undo reverses this group.", result.len()));
    Ok(json!({"status":"executed","results":result,"undoLabel":label,"undoAvailable":app.session.history.can_undo()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app() -> FilmcraftApp {
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.session.execute("file.newSequence", json!({"width":16,"height":16})).unwrap();
        app
    }
    fn execute(app: &mut FilmcraftApp, id: &str, params: Value) -> Result<Value, String> {
        run(app, &egui::Context::default(), json!({"command":id,"params":params}).as_object().unwrap())
    }
    #[test]
    fn legacy_inspection_commands_use_small_pages_without_changing_history() {
        let mut app = app();
        app.session.execute("file.openDemoProject", json!({})).unwrap();
        let before = app.session.project.clone();
        let undo = app.session.history.undo.len();
        let revision = app.session.revision;
        for (command, params) in [("sequence.inspect", json!({})), ("project.inspect", json!({})), ("sequence.inspectPage", json!({"kind":"tracks"}))] {
            let result = execute(&mut app, command, params).unwrap();
            assert!(result["result"]["rows"].is_array());
            assert!(filmcraft_engine::inspection::json_fits(&result, 32 * 1024));
        }
        assert_eq!(*app.session.project, *before);
        assert_eq!(app.session.revision, revision);
        assert_eq!(app.session.history.undo.len(), undo);
    }
    #[test]
    fn native_registry_exposes_navigation_and_direct_edits_without_review() {
        let mut app = app();
        app.session.execute("file.openDemoProject", json!({})).unwrap();
        let selected = app.session.active_sequence().unwrap().video_tracks[0].items[0].id;
        let sequence = app.session.state.active_sequence.unwrap();
        let before = Arc::clone(&app.session.project);
        let history = app.session.history.undo.len();
        let markers = app.session.active_sequence().unwrap().markers.len();
        let catalog = catalog(&app, json!({"filter":"playhead.set"}).as_object().unwrap()).unwrap();
        assert_eq!(catalog["commands"][0]["id"], "playhead.set");
        assert_eq!(catalog["commands"][0]["consoleAccess"], "direct");
        execute(&mut app, "playhead.set", json!({"frame":48})).unwrap();
        assert_eq!(*app.session.state.playheads.get(&sequence).unwrap(), app.session.sequence_rate().tick_of(48));
        execute(&mut app, "timeline.select", json!({"clips":[selected.0],"add":false,"toggle":false,"linked":false})).unwrap();
        assert_eq!(app.session.state.selection, vec![selected]);
        assert_eq!(*before, *app.session.project);
        assert_eq!(app.session.history.undo.len(), history);
        execute(&mut app, "markers.add", json!({"target":"program","time":0,"name":"Direct"})).unwrap();
        assert!(app.ui.console.chat.pending.is_none());
        assert_eq!(app.session.active_sequence().unwrap().markers.len(), markers + 1);
        execute(&mut app, "edit.undo", json!({})).unwrap();
        assert_eq!(*before, *app.session.project);
        execute(&mut app, "edit.redo", json!({})).unwrap();
        assert_eq!(app.session.active_sequence().unwrap().markers.len(), markers + 1);
    }
    #[test]
    fn direct_export_refuses_existing_destination_without_explicit_overwrite() {
        let app = app();
        let destination = tempfile::NamedTempFile::new().unwrap();
        let path = destination.path().to_str().unwrap();
        assert!(prepare(&app, "file.exportMedia", json!({"path":path})).is_err());
        assert!(prepare(&app, "file.exportMedia", json!({"path":path,"overwrite":true})).is_ok());
        assert!(prepare(&app, "file.saveCopy", json!({"path":path})).is_err());
    }
    #[test]
    fn grouped_request_is_one_undo_and_failure_preserves_redo() {
        let mut app = app();
        let before = Arc::clone(&app.session.project);
        let count = app.session.history.undo.len();
        let steps = json!({"label":"Add phrase markers","steps":[{"id":"markers.add","params":{"target":"program","time":0,"name":"One"}},{"id":"markers.add","params":{"target":"program","time":1,"name":"Two"}}]});
        batch(&mut app, steps.as_object().unwrap()).unwrap();
        assert_eq!(app.session.history.undo.len(), count + 1);
        execute(&mut app, "edit.undo", json!({})).unwrap();
        assert_eq!(*before, *app.session.project);
        let redo = app.session.history.redo.len();
        let bad = json!({"steps":[{"id":"markers.add","params":{"target":"program","time":0,"name":"Rolled back"}},{"id":"effects.setParam","params":{"clip":u64::MAX,"effect":"Missing","param":"opacity","value":1}}]});
        assert!(batch(&mut app, bad.as_object().unwrap()).is_err());
        assert_eq!(*before, *app.session.project);
        assert_eq!(app.session.history.redo.len(), redo);
        assert_eq!(app.session.history.undo.len(), count);
    }
    #[test]
    fn hostile_and_reserved_control_requests_do_not_modify_project() {
        let mut app = app();
        let before = Arc::clone(&app.session.project);
        for id in ["console.run", "console.voice.on", "voiceover.recordToggle", "audio.voiceover.start", "help.website", "no.such.command"] {
            assert!(execute(&mut app, id, json!({})).is_err(), "{id}");
        }
        assert!(execute(&mut app, "markers.add", json!([])).is_err());
        assert!(batch(&mut app, json!({"steps":[]}).as_object().unwrap()).is_err());
        assert!(batch(&mut app, json!({"steps":[{"id":"edit.undo"}]}).as_object().unwrap()).is_err());
        assert!(catalog(&app, json!({"offset":-1}).as_object().unwrap()).is_err());
        assert!(catalog(&app, json!({"limit":u64::MAX}).as_object().unwrap()).is_err());
        assert_eq!(*before, *app.session.project);
    }
    #[test]
    fn rejected_batch_cannot_start_mask_tracking_that_outlives_rollback() {
        let mut app = app();
        app.session.execute("file.openDemoProject", json!({})).unwrap();
        let clip = app.session.active_sequence().unwrap().video_tracks[0].items[0].clone();
        app.session.state.selection = vec![clip.id];
        app.session.set_playhead(clip.start);
        app.session.execute("masks.add", json!({"effect":"opacity","shape":"ellipse"})).unwrap();
        let before = Arc::clone(&app.session.project);
        let history = app.session.history.undo.len();
        let jobs = app.session.jobs.len();
        let request = json!({"steps":[
            {"id":"markers.add","params":{"target":"program","time":0,"name":"Must not run"}},
            {"id":"masks.track","params":{"frames":1,"wait":false}},
            {"id":"effects.setParam","params":{"clip":u64::MAX,"effect":"Missing","param":"opacity","value":1}}
        ]});
        let error = batch(&mut app, request.as_object().unwrap()).unwrap_err();
        assert!(error.contains("masks.track") && error.contains("command_run"), "{error}");
        assert!(app.session.mask_jobs.is_empty());
        assert_eq!(app.session.jobs.len(), jobs);
        filmcraft_engine::masks::poll(&mut app.session);
        assert_eq!(*app.session.project, *before);
        assert_eq!(app.session.history.undo.len(), history);
        // The same operation remains available through the explicit individual-command route.
        assert!(prepare(&app, "masks.track", json!({"frames":1,"wait":false})).is_ok());
    }
}
