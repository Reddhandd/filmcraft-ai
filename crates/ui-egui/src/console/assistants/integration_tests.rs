use super::*;
use crate::console::{CodexConnection, ConnectionStatus, VoiceStatus};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc};
struct Host;
impl CodexConnection for Host {
    fn connect(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn poll(&mut self) -> Option<ConnectionStatus> {
        None
    }
    fn disconnect(&mut self) {}
}
fn app(dir: &Path, name: &str) -> FilmcraftApp {
    let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
    app.session.execute("file.newSequence", json!({"width":16,"height":16})).unwrap();
    app.session.path = Some(dir.join(format!("{name}.fcproj")).to_string_lossy().into_owned());
    app.session.prefs_path = Some(dir.join("data/preferences.json"));
    app.ui.console.open = true;
    app.hooks.codex_connection = Some(Box::new(Host));
    crate::console::sessions::sync_project(&mut app);
    poll(&mut app).unwrap();
    app
}
fn command(app: &mut FilmcraftApp, id: &str, p: Value) -> Result<Value, String> {
    crate::menus::invoke(app, &egui::Context::default(), id, p)
}
fn read(app: &mut FilmcraftApp) -> Value {
    command(app, "console.bridge.memory", json!({"action":"assistant_read"})).unwrap()
}
fn learn(app: &mut FilmcraftApp, role: &str) -> Value {
    let snapshot = read(app);
    command(app,"console.bridge.memory",json!({"action":"assistant_update","scope":snapshot["scope"],"documentEpoch":snapshot["documentEpoch"],"memoryGeneration":snapshot["memoryGeneration"],"text":json!({"instructions":{"role":role,"goal":"Maintain continuity","preferences":["Keep full phrases"],"workflow":["Inspect before editing"]},"decisions":["Use the wide view for this project"],"pending":["Review chorus"],"errors":[]}).to_string()})).unwrap()
}
/// Test-operator approval, distinct from model-authored teaching proposals.
fn approve_teaching(app: &mut FilmcraftApp) {
    let proposals = app.assistants.current.as_ref().unwrap().snapshot().assistant.proposed.clone();
    for teaching in proposals {
        let snapshot = read(app);
        command(
            app,
            "console.assistant.review",
            json!({"scope":snapshot["scope"],"documentEpoch":snapshot["documentEpoch"],
            "memoryGeneration":snapshot["memoryGeneration"],"teaching":teaching,"action":"accept"}),
        )
        .unwrap();
    }
}
fn propose_instructions(app: &mut FilmcraftApp, instructions: Instructions) {
    let snapshot = read(app);
    assert!(snapshot["nextOffset"].is_null());
    let mut body: Value = serde_json::from_str(snapshot["text"].as_str().unwrap()).unwrap();
    body["instructions"] = json!(instructions);
    command(
        app,
        "console.bridge.memory",
        json!({"action":"assistant_update","scope":snapshot["scope"],"documentEpoch":snapshot["documentEpoch"],
        "memoryGeneration":snapshot["memoryGeneration"],"text":body.to_string()}),
    )
    .unwrap();
}

#[test]
fn partial_manual_updates_autosave_and_fresh_context_preserve_band_sync_teaching() {
    let dir = tempfile::tempdir().unwrap();
    let mut first = app(dir.path(), "band-A");
    let band =
        Instructions { role: "Band editor".into(), workflow: vec!["Synchronize selected band performances to the master song".into()], ..Default::default() };
    propose_instructions(&mut first, band.clone());
    command(&mut first, "console.assistant.save", json!({"name":"Band assistant"})).unwrap();
    let original = first.assistants.library.as_ref().unwrap().instructions("Band assistant").unwrap();
    propose_instructions(&mut first, Instructions { preferences: vec!["Keep the opening wide shot".into()], ..Default::default() });
    command(&mut first, "console.assistant.save", json!({"name":"Band assistant","replace":true,"expectedInstructions":original})).unwrap();
    flush(&mut first).unwrap();

    let mut fresh = app(dir.path(), "band-B");
    command(&mut fresh, "console.assistant.load", json!({"name":"Band assistant"})).unwrap();
    let saved = fresh.assistants.current.as_ref().unwrap().snapshot().assistant.clone();
    assert_eq!(saved.role, band.role);
    assert_eq!(saved.workflow, band.workflow);
    assert_eq!(saved.proposed.len(), 1);
    assert_eq!(saved.proposed[0].text, "Keep the opening wide shot");
    approve_teaching(&mut fresh);
    command(&mut fresh, "console.assistant.autosave", json!({"enabled":true})).unwrap();
    flush(&mut fresh).unwrap();

    let mut third = app(dir.path(), "band-C");
    command(&mut third, "console.assistant.load", json!({"name":"Band assistant"})).unwrap();
    let restored = &third.assistants.current.as_ref().unwrap().snapshot().assistant;
    assert_eq!(restored.workflow, band.workflow);
    assert_eq!(restored.preferences, ["Keep the opening wide shot"]);
    let revision = third.session.revision;
    let snapshot = read(&mut third);
    let teaching = preservation::Teaching { section: preservation::Section::Workflow, text: band.workflow[0].clone() };
    let params = json!({"scope":snapshot["scope"],"documentEpoch":snapshot["documentEpoch"],"memoryGeneration":snapshot["memoryGeneration"],
        "teaching":teaching,"action":"forget"});
    command(&mut third, "console.assistant.review", params.clone()).unwrap();
    assert!(command(&mut third, "console.assistant.review", params).is_err(), "Review identity must become stale after applying");
    propose_instructions(&mut third, band);
    assert!(third.assistants.current.as_ref().unwrap().snapshot().assistant.workflow.is_empty());
    assert_eq!(third.session.revision, revision);
}
#[test]
fn self_authored_memory_checkpoint_during_voice_does_not_block_current_project_edits() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(dir.path(), "voice-memory");
    app.session.revision = 134;
    app.ui.console.connection = ConnectionStatus::Ready;
    app.ui.console.chat.voice = VoiceStatus::Active;
    app.ui.console.chat.busy = true;
    let before = app.session.project.clone();
    let identity = (app.session.revision, app.session.document_epoch, app.session.history.undo.len(), app.session.history.redo.len());
    assert!(super::super::execution::prepare(&app, "playhead.set", json!({"frame":1})).is_ok());
    learn(&mut app, "Current project editor");
    assert_eq!(app.ui.console.chat.voice, VoiceStatus::Active);
    assert!(app.ui.console.chat.busy);
    assert!(Arc::ptr_eq(&before, &app.session.project));
    assert_eq!((app.session.revision, app.session.document_epoch, app.session.history.undo.len(), app.session.history.redo.len()), identity);
    assert!(!super::super::sessions::context_pending(&app), "Saving self-authored context must not strand edits behind an idle-only refresh");
    let request = json!({"command":"playhead.set","params":{"frame":1}});
    super::super::execution::run(&mut app, &egui::Context::default(), request.as_object().unwrap()).unwrap();
    // A genuine pending context transition must still block subsequent edits.
    super::super::sessions::creative_context_changed(&mut app);
    learn(&mut app, "Current project editor");
    assert!(super::super::sessions::context_pending(&app));
    let error = super::super::execution::prepare(&app, "playhead.set", json!({"frame":2})).unwrap_err();
    assert!(error.contains("Voice Off"));
    assert!(error.contains("Read-only inspection"));
}

#[test]
fn named_assistant_survives_reopen_without_interrupting_voice_or_editing_project() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(dir.path(), "A");
    let project = app.session.project.clone();
    let identity = (app.session.revision, app.session.document_epoch, app.session.history.undo.len(), app.session.history.redo.len());
    learn(&mut app, "Concert performance editor");
    app.ui.console.chat.voice = VoiceStatus::Active;
    app.ui.console.chat.voice_muted = true;
    assert!(command(&mut app, "console.assistant.save", json!({"name":" "})).is_err());
    command(&mut app, "console.assistant.save", json!({"name":"Concert editor"})).unwrap();
    assert_eq!(app.ui.console.chat.voice, VoiceStatus::Active);
    assert!(app.ui.console.chat.voice_muted);
    assert!(Arc::ptr_eq(&project, &app.session.project));
    assert_eq!((app.session.revision, app.session.document_epoch, app.session.history.undo.len(), app.session.history.redo.len()), identity);
    flush(&mut app).unwrap();
    drop(app);
    let mut reopened = self::app(dir.path(), "A");
    let snapshot = reopened.assistants.current.as_ref().unwrap().snapshot();
    assert_eq!(snapshot.assistant.role, "Concert performance editor");
    assert_eq!(snapshot.assistant_name.as_deref(), Some("Concert editor"));
    let request = super::super::sessions::SessionRequest {
        project: reopened.session.path.clone().unwrap(),
        resume: None,
        memory: String::new(),
        model: None,
        scope_aliases: vec![],
        recent_dialogue: Vec::new(),
        assistant_summary: String::new(),
    };
    let request = attach_context(&mut reopened, request).unwrap();
    assert!(request.memory.contains("Concert performance editor"));
    assert!(request.assistant_summary.contains("Concert performance editor"));
    assert!(request.memory.contains("not permission"));
}
#[test]
fn loading_named_assistant_keeps_destination_project_decisions_and_history() {
    let dir = tempfile::tempdir().unwrap();
    let mut first = app(dir.path(), "A");
    learn(&mut first, "Continuity editor");
    command(&mut first, "console.assistant.save", json!({"name":"Continuity"})).unwrap();
    flush(&mut first).unwrap();
    let mut second = app(dir.path(), "B");
    let project = second.session.project.clone();
    let runtime = second.assistants.current.as_mut().unwrap();
    let mut snapshot = runtime.snapshot().clone();
    snapshot.decisions = vec!["Project B decision".into()];
    snapshot.pending = vec!["Project B next shot".into()];
    snapshot.commands.push(CommandRecord {
        command: "markers.add".into(),
        params: json!({"name":"proposal only"}),
        state: CommandState::Proposed,
        revision_before: 2,
        revision_after: 2,
        error: None,
    });
    runtime.update(snapshot, Instant::now()).unwrap();
    command(&mut second, "console.assistant.load", json!({"name":"Continuity"})).unwrap();
    let loaded = second.assistants.current.as_ref().unwrap().snapshot();
    assert_eq!(loaded.assistant.role, "Continuity editor");
    assert_eq!(loaded.decisions, ["Project B decision"]);
    assert_eq!(loaded.pending, ["Project B next shot"]);
    assert_eq!(loaded.commands.len(), 1);
    assert_eq!(loaded.commands[0].state, CommandState::Proposed);
    assert_eq!(loaded.project_scope, second.session.path.clone().unwrap());
    assert!(Arc::ptr_eq(&project, &second.session.project));
}
#[test]
fn raw_conversation_changes_do_not_stale_semantic_updates_but_semantic_changes_do() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(dir.path(), "A");
    let original = read(&mut app);
    crate::console::chat::message(&mut app.ui.console.chat, "You", "An ordinary new message".into());
    changed(&mut app);
    poll(&mut app).unwrap();
    let update = json!({"instructions":{"role":"Editor","goal":"","preferences":[],"workflow":[]},"decisions":[],"pending":[],"errors":[]}).to_string();
    let request = json!({"action":"assistant_update","scope":original["scope"],"documentEpoch":original["documentEpoch"],"memoryGeneration":original["memoryGeneration"],"text":update});
    command(&mut app, "console.bridge.memory", request.clone()).unwrap();
    assert!(command(&mut app, "console.bridge.memory", request).is_err());
    let fresh = read(&mut app);
    assert_eq!(fresh["memoryGeneration"], 1);
    assert_eq!(serde_json::from_str::<Value>(fresh["text"].as_str().unwrap()).unwrap()["instructions"]["role"], "Editor");
}
#[test]
fn command_capture_preserves_atomic_undo_and_never_replays_on_restore() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(dir.path(), "A");
    let before = app.session.project.clone();
    let history = app.session.history.undo.len();
    let steps = json!({"label":"Phrase markers","steps":[{"id":"markers.add","params":{"target":"program","time":0,"name":"One"}},{"id":"markers.add","params":{"target":"program","time":1,"name":"Two"}}]});
    crate::console::execution::batch(&mut app, steps.as_object().unwrap()).unwrap();
    assert_eq!(app.session.history.undo.len(), history + 1);
    crate::console::execution::run(&mut app, &egui::Context::default(), json!({"command":"edit.undo"}).as_object().unwrap()).unwrap();
    assert_eq!(*before, *app.session.project);
    let redo = app.session.history.redo.len();
    let bad = json!({"steps":[{"id":"markers.add","params":{"target":"program","time":0,"name":"Must roll back"}},{"id":"effects.setParam","params":{"clip":u64::MAX,"effect":"Missing","param":"opacity","value":1}}]});
    assert!(crate::console::execution::batch(&mut app, bad.as_object().unwrap()).is_err());
    assert_eq!(*before, *app.session.project);
    assert_eq!(app.session.history.redo.len(), redo);
    let records = &app.assistants.current.as_ref().unwrap().snapshot().commands;
    assert_eq!(records.len(), 3);
    assert_eq!(records[0].state, CommandState::Executed);
    assert_eq!(records[2].state, CommandState::Failed);
    flush(&mut app).unwrap();
    drop(app);
    let reopened = self::app(dir.path(), "A");
    assert_eq!(reopened.session.active_sequence().unwrap().markers.len(), 0);
    assert_eq!(reopened.assistants.current.as_ref().unwrap().snapshot().commands.len(), 3);
}

#[test]
fn save_as_carries_learned_context_and_preserves_source_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = app(dir.path(), "Original");
    learn(&mut host, "Continuity editor");
    flush(&mut host).unwrap();
    let original = host.assistants.current.as_ref().unwrap().snapshot().clone();
    let destination = dir.path().join("Renamed.fcproj").to_string_lossy().into_owned();
    host.assistants.sync(&destination, host.session.document_epoch, host.session.revision).unwrap();
    host.assistants.flush().unwrap();
    assert_eq!(host.assistants.current.as_ref().unwrap().snapshot().assistant.role, "Continuity editor");
    let source = disk::load(&super::super::sessions::folder(&original.project_scope), &original.project_scope).unwrap().unwrap().0;
    assert_eq!(source.assistant, original.assistant);
    assert_eq!(source.project_scope, original.project_scope);
}

#[test]
fn loading_named_instructions_invalidates_an_older_semantic_update() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = app(dir.path(), "A");
    learn(&mut host, "Saved role");
    command(&mut host, "console.assistant.save", json!({"name":"Saved"})).unwrap();
    let old = read(&mut host);
    command(&mut host, "console.assistant.load", json!({"name":"Saved"})).unwrap();
    let update = json!({"instructions":{"role":"Stale role","goal":"","preferences":[],"workflow":[]},"decisions":[],"pending":[],"errors":[]}).to_string();
    assert!(command(&mut host,"console.bridge.memory",json!({"action":"assistant_update","scope":old["scope"],"documentEpoch":old["documentEpoch"],"memoryGeneration":old["memoryGeneration"],"text":update})).is_err());
    assert_eq!(host.assistants.current.as_ref().unwrap().snapshot().assistant.role, "Saved role");
}

#[test]
fn save_as_conflict_retains_both_projects_assistant_memory() {
    let dir = tempfile::tempdir().unwrap();
    let mut destination = app(dir.path(), "Existing");
    learn(&mut destination, "Destination editor");
    flush(&mut destination).unwrap();
    let destination_scope = destination.assistants.current.as_ref().unwrap().snapshot().project_scope.clone();
    drop(destination);
    let mut source = app(dir.path(), "Source");
    learn(&mut source, "Source editor");
    let source_scope = source.assistants.current.as_ref().unwrap().snapshot().project_scope.clone();
    assert!(source.assistants.sync(&destination_scope, source.session.document_epoch, source.session.revision).is_err());
    assert_eq!(source.assistants.current.as_ref().unwrap().snapshot().project_scope, source_scope);
    assert_eq!(source.assistants.current.as_ref().unwrap().snapshot().assistant.role, "Source editor");
    for (scope, role) in [(source_scope, "Source editor"), (destination_scope, "Destination editor")] {
        let saved = disk::load(&super::super::sessions::folder(&scope), &scope).unwrap().unwrap().0;
        assert_eq!(saved.assistant.role, role);
    }
}

#[test]
fn prepared_command_is_saved_as_proposed_and_never_replayed_on_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = app(dir.path(), "A");
    let project = host.session.project.clone();
    let identity = (host.session.revision, host.session.history.undo.len(), host.session.history.redo.len());
    command(&mut host, "console.bridge.prepare", json!({"command":"markers.add","params":{"target":"program","time":0,"name":"Proposed verse"}})).unwrap();
    let records = &host.assistants.current.as_ref().unwrap().snapshot().commands;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].state, CommandState::Proposed);
    assert_eq!(records[0].revision_before, records[0].revision_after);
    assert!(Arc::ptr_eq(&project, &host.session.project));
    assert_eq!((host.session.revision, host.session.history.undo.len(), host.session.history.redo.len()), identity);
    flush(&mut host).unwrap();
    drop(host);
    let reopened = app(dir.path(), "A");
    assert!(reopened.session.active_sequence().unwrap().markers.is_empty());
    assert_eq!(reopened.assistants.current.as_ref().unwrap().snapshot().commands[0].state, CommandState::Proposed);
}

#[test]
fn dirty_context_keeps_an_idle_closed_console_scheduled() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = app(dir.path(), "Idle");
    learn(&mut host, "Idle continuity editor");
    flush(&mut host).unwrap();
    host.ui.console.open = false;
    let ctx = egui::Context::default();
    for _ in 0..4 {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| crate::console::show(&mut host, ui.ctx()));
        output.textures_delta.clear();
    }
    crate::console::chat::message(&mut host.ui.console.chat, "You", "Keep this new handoff context".into());
    changed(&mut host);
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| crate::console::show(&mut host, ui.ctx()));
    let delay = output.viewport_output.get(&egui::ViewportId::ROOT).unwrap().repaint_delay;
    output.textures_delta.clear();
    assert!(
        delay <= std::time::Duration::from_secs(1),
        "Dirty memory must schedule its own wakeup rather than depend on playback, media scans or mouse input: {delay:?}"
    );
}

#[test]
fn recovered_checkpoint_is_reported_in_memory_and_connection_context() {
    let dir = tempfile::tempdir().unwrap();
    let mut original = app(dir.path(), "Recovery");
    learn(&mut original, "Previous valid role");
    flush(&mut original).unwrap();
    learn(&mut original, "Newest role");
    flush(&mut original).unwrap();
    let scope = original.assistants.current.as_ref().unwrap().snapshot().project_scope.clone();
    drop(original);
    std::fs::write(super::super::sessions::folder(&scope).join("assistant-checkpoint.json"), b"{malformed").unwrap();
    let mut recovered = app(dir.path(), "Recovery");
    let memory = read(&mut recovered);
    assert_eq!(memory["recoveredPrevious"], true);
    assert_eq!(serde_json::from_str::<Value>(memory["text"].as_str().unwrap()).unwrap()["instructions"]["role"], "Previous valid role");
    assert_eq!(command(&mut recovered, "console.assistant.inspect", json!({})).unwrap()["recoveredPrevious"], true);
    assert_eq!(context(&mut recovered).unwrap()["recoveredPrevious"], true);
}

#[test]
fn unreadable_unloaded_memory_does_not_trap_close_or_existing_conversation_context() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = app(dir.path(), "Broken");
    learn(&mut host, "First role");
    flush(&mut host).unwrap();
    learn(&mut host, "Second role");
    flush(&mut host).unwrap();
    let scope = host.assistants.current.as_ref().unwrap().snapshot().project_scope.clone();
    drop(host);
    let folder = super::super::sessions::folder(&scope);
    for name in ["assistant-checkpoint.json", "assistant-checkpoint.previous.json"] {
        std::fs::write(folder.join(name), b"{broken").unwrap();
    }
    let mut host = FilmcraftApp::new(filmcraft_engine::Session::default());
    host.session.path = Some(scope.clone());
    host.session.prefs_path = Some(dir.path().join("data/preferences.json"));
    host.ui.console.open = true;
    host.hooks.codex_connection = Some(Box::new(Host));
    super::super::sessions::sync_project(&mut host);
    assert!(poll(&mut host).is_err());
    assert!(host.assistants.current.is_none());
    assert!(flush(&mut host).is_ok());
    assert!(host.assistants.error.is_some());
    let request = super::super::sessions::SessionRequest { project: scope, memory: "Existing project notes".into(), ..Default::default() };
    let request = attach_context(&mut host, request).unwrap();
    assert!(request.memory.starts_with("Existing project notes"));
    assert!(request.memory.contains("\"unavailable\":true"));
    for name in ["assistant-checkpoint.json", "assistant-checkpoint.previous.json"] {
        assert_eq!(std::fs::read(folder.join(name)).unwrap(), b"{broken");
    }
}

#[test]
fn loaded_unsaved_memory_still_blocks_a_failed_close_flush() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = app(dir.path(), "Busy");
    learn(&mut host, "Keep this unsaved role");
    let scope = host.assistants.current.as_ref().unwrap().snapshot().project_scope.clone();
    let held = disk::ScopeLock::take(&super::super::sessions::folder(&scope)).unwrap();
    assert!(flush(&mut host).is_err());
    assert_eq!(host.assistants.current.as_ref().unwrap().snapshot().assistant.role, "Keep this unsaved role");
    assert!(host.assistants.current.as_ref().unwrap().needs_polling());
    drop(held);
    flush(&mut host).unwrap();
    assert!(!host.assistants.current.as_ref().unwrap().needs_polling());
}

#[test]
fn named_update_retains_previous_catalog_and_rejects_stale_project_update() {
    let dir = tempfile::tempdir().unwrap();
    let mut first = app(dir.path(), "first");
    learn(&mut first, "Original editor");
    command(&mut first, "console.assistant.save", json!({"name":"Shared editor"})).unwrap();
    let original = first.assistants.library.as_ref().unwrap().instructions("Shared editor").unwrap();
    let mut second = app(dir.path(), "second");
    command(&mut second, "console.assistant.load", json!({"name":"Shared editor"})).unwrap();
    learn(&mut first, "Updated editor");
    command(&mut first, "console.assistant.save", json!({"name":"Shared editor","replace":true,"expectedInstructions":original})).unwrap();
    let catalog_dir = first.assistants.library_dir.as_ref().unwrap();
    let previous = std::fs::read(catalog_dir.join(catalog_store::PREVIOUS)).unwrap();
    assert_eq!(library::Library::decode(&previous).unwrap().instructions("Shared editor").unwrap(), original);
    let current = std::fs::read(catalog_dir.join(catalog_store::CURRENT)).unwrap();
    let identity = (second.session.revision, second.session.document_epoch, second.session.history.undo.len());
    learn(&mut second, "Stale editor");
    assert!(command(&mut second, "console.assistant.save", json!({"name":"Shared editor","replace":true,"expectedInstructions":original})).is_err());
    assert_eq!(std::fs::read(catalog_dir.join(catalog_store::CURRENT)).unwrap(), current);
    assert_eq!(std::fs::read(catalog_dir.join(catalog_store::PREVIOUS)).unwrap(), previous);
    assert_eq!((second.session.revision, second.session.document_epoch, second.session.history.undo.len()), identity);
}

#[test]
fn autosave_opt_in_restores_updated_instructions_without_exporting_project_history() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = app(dir.path(), "autosave");
    assert!(command(&mut host, "console.assistant.autosave", json!({"enabled":true})).is_err());
    assert!(command(&mut host, "console.assistant.autosave", json!({"enabled":"yes"})).is_err());
    learn(&mut host, "Original editor");
    command(&mut host, "console.assistant.save", json!({"name":"Reusable"})).unwrap();
    command(&mut host, "console.assistant.autosave", json!({"enabled":true})).unwrap();
    learn(&mut host, "Learned performance editor");
    approve_teaching(&mut host);
    let identity = (host.session.revision, host.session.document_epoch, host.session.history.undo.len());
    flush(&mut host).unwrap();
    let runtime = host.assistants.current.as_ref().unwrap();
    assert!(!runtime.snapshot().autosave.as_ref().unwrap().saved_at.is_empty());
    let catalog = catalog_store::read(host.assistants.library_dir.as_ref().unwrap()).unwrap();
    assert_eq!(catalog.instructions("Reusable").unwrap().role, "Learned performance editor");
    let encoded = String::from_utf8(catalog.encode().unwrap()).unwrap();
    assert!(!encoded.contains("Use the wide view for this project"));
    assert!(!encoded.contains("Review chorus"));
    assert_eq!((host.session.revision, host.session.document_epoch, host.session.history.undo.len()), identity);
    drop(host);
    let mut reopened = app(dir.path(), "autosave");
    assert!(reopened.assistants.current.as_ref().unwrap().snapshot().autosave.is_some());
    let mut fresh = app(dir.path(), "fresh");
    command(&mut fresh, "console.assistant.load", json!({"name":"Reusable"})).unwrap();
    let snapshot = fresh.assistants.current.as_ref().unwrap().snapshot();
    assert_eq!(snapshot.assistant.role, "Learned performance editor");
    assert!(snapshot.decisions.is_empty());
    assert!(snapshot.commands.is_empty());
    assert!(snapshot.autosave.is_none());
    command(&mut reopened, "console.assistant.autosave", json!({"enabled":false})).unwrap();
    flush(&mut reopened).unwrap();
}

#[test]
fn competing_autosave_reports_conflict_and_keeps_newer_catalog_and_local_memory() {
    let dir = tempfile::tempdir().unwrap();
    let mut first = app(dir.path(), "first-auto");
    learn(&mut first, "Original");
    command(&mut first, "console.assistant.save", json!({"name":"Shared"})).unwrap();
    let mut second = app(dir.path(), "second-auto");
    command(&mut second, "console.assistant.load", json!({"name":"Shared"})).unwrap();
    command(&mut first, "console.assistant.autosave", json!({"enabled":true})).unwrap();
    command(&mut second, "console.assistant.autosave", json!({"enabled":true})).unwrap();
    learn(&mut first, "First new role");
    approve_teaching(&mut first);
    flush(&mut first).unwrap();
    learn(&mut second, "Second local role");
    approve_teaching(&mut second);
    assert!(flush(&mut second).is_err());
    let catalog = catalog_store::read(first.assistants.library_dir.as_ref().unwrap()).unwrap();
    assert_eq!(catalog.instructions("Shared").unwrap().role, "First new role");
    assert_eq!(second.assistants.current.as_ref().unwrap().snapshot().assistant.role, "Second local role");
    assert!(second.assistants.current.as_ref().unwrap().autosave_state.as_ref().unwrap().error.is_some());
    let scope = second.assistants.current.as_ref().unwrap().snapshot().project_scope.clone();
    let durable = disk::load(&super::super::sessions::folder(&scope), &scope).unwrap().unwrap().0;
    assert_eq!(durable.assistant.role, "Second local role");

    command(&mut second, "console.assistant.autosave", json!({"enabled":false})).unwrap();
    flush(&mut second).unwrap();
}

#[test]
fn changed_named_instructions_wait_fifteen_minutes_without_a_milestone() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = app(dir.path(), "timed-auto");
    learn(&mut host, "Original");
    command(&mut host, "console.assistant.save", json!({"name":"Timed"})).unwrap();
    command(&mut host, "console.assistant.autosave", json!({"enabled":true})).unwrap();
    let catalog_dir = host.assistants.library_dir.clone().unwrap();
    let runtime = host.assistants.current.as_mut().unwrap();
    let now = Instant::now();
    let mut changed = runtime.snapshot().clone();
    let proposal = preservation::merge(&changed.assistant, &Instructions { role: "Changed".into(), ..Default::default() }, false).unwrap();
    let entry = proposal.proposed.iter().find(|entry| entry.text == "Changed").unwrap();
    changed.assistant = preservation::review(&proposal, entry, "accept").unwrap();
    runtime.update(changed, now).unwrap();
    autosave::tick(runtime, catalog_dir.clone(), now, false, "2026-11-05T15:00:00Z").unwrap();
    assert!(!runtime.autosave_state.as_ref().unwrap().pending());
    autosave::tick(runtime, catalog_dir.clone(), now + std::time::Duration::from_secs(899), false, "2026-11-05T15:14:59Z").unwrap();
    assert!(!runtime.autosave_state.as_ref().unwrap().pending());
    assert_eq!(catalog_store::read(&catalog_dir).unwrap().instructions("Timed").unwrap().role, "Original");
    autosave::tick(runtime, catalog_dir.clone(), now + CHECKPOINT_INTERVAL, false, "2026-11-05T15:15:00Z").unwrap();
    assert!(runtime.autosave_state.as_ref().unwrap().pending());
    autosave::flush(runtime, now + CHECKPOINT_INTERVAL, "2026-11-05T15:15:00Z").unwrap();
    assert_eq!(catalog_store::read(&catalog_dir).unwrap().instructions("Timed").unwrap().role, "Changed");
    assert!(!runtime.autosave_state.as_ref().unwrap().needs_polling());
}

#[test]
fn manual_update_preserves_autosave_and_loading_another_assistant_disables_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = app(dir.path(), "manual-auto");
    learn(&mut host, "Original");
    command(&mut host, "console.assistant.save", json!({"name":"One"})).unwrap();
    command(&mut host, "console.assistant.save", json!({"name":"Two"})).unwrap();
    command(&mut host, "console.assistant.load", json!({"name":"One"})).unwrap();
    command(&mut host, "console.assistant.autosave", json!({"enabled":true})).unwrap();
    flush(&mut host).unwrap();
    command(&mut host, "console.assistant.inspect", json!({})).unwrap();
    let expected = host.assistants.library.as_ref().unwrap().instructions("One").unwrap();
    command(&mut host, "console.assistant.save", json!({"name":"One","replace":true,"expectedInstructions":expected})).unwrap();
    assert!(host.assistants.current.as_ref().unwrap().snapshot().autosave.is_some());
    command(&mut host, "console.assistant.load", json!({"name":"Two"})).unwrap();
    assert!(host.assistants.current.as_ref().unwrap().snapshot().autosave.is_none());
}

#[test]
fn saved_assistant_loads_reference_selection_without_project_history() {
    let dir = tempfile::tempdir().unwrap();
    let mut first = app(dir.path(), "references-first");
    learn(&mut first, "Reference editor");
    let runtime = first.assistants.current.as_mut().unwrap();
    let mut snapshot = runtime.snapshot().clone();
    snapshot.reference_documents = Some(std::collections::BTreeSet::from([7, 9]));
    runtime.update(snapshot, Instant::now()).unwrap();
    command(&mut first, "console.assistant.save", json!({"name":"Reference editor"})).unwrap();
    let mut second = app(dir.path(), "references-second");
    command(&mut second, "console.assistant.load", json!({"name":"Reference editor"})).unwrap();
    let snapshot = second.assistants.current.as_ref().unwrap().snapshot();
    assert_eq!(snapshot.reference_documents, Some(std::collections::BTreeSet::from([7, 9])));
    assert!(snapshot.decisions.is_empty());
    assert!(snapshot.pending.is_empty());
    assert!(snapshot.commands.is_empty());
}

struct MaintenanceHost {
    sent: Arc<std::sync::Mutex<Vec<String>>>,
    cancels: Arc<std::sync::atomic::AtomicUsize>,
    events: Arc<std::sync::Mutex<std::collections::VecDeque<crate::console::ChatEvent>>>,
}
impl CodexConnection for MaintenanceHost {
    fn connect(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn poll(&mut self) -> Option<ConnectionStatus> {
        None
    }
    fn disconnect(&mut self) {}
    fn send_prompt_with_model(&mut self, prompt: String, _model: Option<String>) -> Result<(), String> {
        self.sent.lock().unwrap().push(prompt);
        Ok(())
    }
    fn send_prompt_with_images(&mut self, _: String, _: Option<String>, _: Vec<String>) -> Result<(), String> {
        Err("Maintenance must not deliver attachments".into())
    }
    fn cancel(&mut self) -> Result<(), String> {
        self.cancels.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn poll_chat(&mut self) -> Option<crate::console::ChatEvent> {
        self.events.lock().unwrap().pop_front()
    }
}
fn maintenance_history(app: &mut FilmcraftApp) {
    app.ui.console.sessions.active = Some("private-maintenance-test".into());
    for n in 0..28 {
        crate::console::chat::message(&mut app.ui.console.chat, "You", format!("Completed inspection {n}: {}", "historical detail ".repeat(150)));
    }
    changed(app);
}
#[test]
fn automatic_compaction_waits_for_owner_then_protects_edits_and_does_not_reexpand_history() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = app(dir.path(), "maintenance");
    let sent = Arc::new(std::sync::Mutex::new(Vec::new()));
    let cancels = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let events = Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));
    host.hooks.codex_connection = Some(Box::new(MaintenanceHost { sent: sent.clone(), cancels, events: events.clone() }));
    host.ui.console.connection = ConnectionStatus::Ready;
    maintenance_history(&mut host);
    host.ui.console.chat.prompt = "Owner's unsent instructions".into();
    poll(&mut host).unwrap();
    assert!(sent.lock().unwrap().is_empty());
    assert_eq!(host.ui.console.chat.prompt, "Owner's unsent instructions");
    host.ui.console.chat.prompt.clear();
    host.ui.console.chat.voice = VoiceStatus::Active;
    poll(&mut host).unwrap();
    assert!(sent.lock().unwrap().is_empty());
    host.ui.console.chat.voice = VoiceStatus::Off;
    poll(&mut host).unwrap();
    assert_eq!(sent.lock().unwrap().len(), 1);
    assert!(maintenance_active(&host));
    let original = Arc::clone(&host.session.project);
    assert!(command(&mut host, "console.bridge.execute", json!({"command":"markers.add","params":{"name":"must not run"}})).is_err());
    assert!(Arc::ptr_eq(&original, &host.session.project));
    assert!(command(&mut host, "console.bridge.memory", json!({"action":"append","text":"maintenance must not change notes"})).is_err());
    let snapshot = read(&mut host);
    let history = command(&mut host, "console.bridge.memory", json!({"action":"assistant_history"})).unwrap();
    command(&mut host,"console.bridge.memory",json!({"action":"assistant_compact","historyGeneration":history["memoryGeneration"],"scope":snapshot["scope"],"documentEpoch":snapshot["documentEpoch"],"memoryGeneration":snapshot["memoryGeneration"],"text":json!({"summary":"Completed the recorded inspections; no unfinished edits were requested.","decisions":[]}).to_string()})).unwrap();
    events
        .lock()
        .unwrap()
        .extend([crate::console::ChatEvent::Delta("Maintenance result should not duplicate the old transcript".into()), crate::console::ChatEvent::Finished]);
    crate::console::chat::poll(&mut host, &egui::Context::default());
    let deadline = Instant::now() + std::time::Duration::from_secs(2);
    while maintenance_active(&host) {
        poll(&mut host).unwrap();
        assert!(Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    for _ in 0..3 {
        changed(&mut host);
        poll(&mut host).unwrap();
    }
    assert_eq!(host.assistants.current.as_ref().unwrap().snapshot().recent_context.len(), 9);
    assert_eq!(sent.lock().unwrap().len(), 1);
}
#[test]
fn maintenance_timeout_cancels_once_and_keeps_edit_guard_until_provider_finishes() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = app(dir.path(), "maintenance-timeout");
    let sent = Arc::new(std::sync::Mutex::new(Vec::new()));
    let cancels = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let events = Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));
    host.hooks.codex_connection = Some(Box::new(MaintenanceHost { sent, cancels: cancels.clone(), events: events.clone() }));
    host.ui.console.connection = ConnectionStatus::Ready;
    maintenance_history(&mut host);
    poll(&mut host).unwrap();
    assert!(maintenance_active(&host));
    let future = Instant::now() + std::time::Duration::from_secs(121);
    maintenance::poll(&mut host, future).unwrap();
    maintenance::poll(&mut host, future).unwrap();
    assert_eq!(cancels.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(maintenance_guard(&host).is_err());
    events.lock().unwrap().push_back(crate::console::ChatEvent::Finished);
    crate::console::chat::poll(&mut host, &egui::Context::default());
    poll(&mut host).unwrap();
    assert!(!maintenance_active(&host));
}
#[test]
fn unsupported_maintenance_transport_does_not_break_memory_reads() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = app(dir.path(), "maintenance-deferred");
    maintenance_history(&mut host);
    host.ui.console.connection = ConnectionStatus::Ready;
    poll(&mut host).unwrap();
    assert!(!maintenance_active(&host));
    assert!(host.assistants.error.is_none());
    assert!(read(&mut host)["compactionStatus"].as_str().unwrap().contains("deferred"));
}

#[test]
fn compaction_requires_the_history_snapshot_even_when_semantic_generation_is_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = app(dir.path(), "history-source-guard");
    maintenance_history(&mut host);
    let semantic = read(&mut host);
    let history = command(&mut host, "console.bridge.memory", json!({"action":"assistant_history"})).unwrap();
    assert_eq!(history["contextGeneration"], semantic["memoryGeneration"]);
    crate::console::chat::message(&mut host.ui.console.chat, "You", "A newly completed inspection arrived after the summary source was read".into());
    changed(&mut host);
    let current = read(&mut host);
    assert_eq!(current["memoryGeneration"], semantic["memoryGeneration"]);
    let fresh_history = command(&mut host, "console.bridge.memory", json!({"action":"assistant_history"})).unwrap();
    assert_ne!(fresh_history["memoryGeneration"], history["memoryGeneration"]);
    let before = host.assistants.current.as_ref().unwrap().snapshot().clone();
    for guard in [Value::Null, history["memoryGeneration"].clone()] {
        let result = command(
            &mut host,
            "console.bridge.memory",
            json!({
                "action":"assistant_compact","scope":current["scope"],"documentEpoch":current["documentEpoch"],
                "memoryGeneration":current["memoryGeneration"],"historyGeneration":guard,
                "text":json!({"summary":"An outdated generated summary","decisions":[]}).to_string()
            }),
        );
        assert!(result.unwrap_err().contains("history changed"));
        assert_eq!(host.assistants.current.as_ref().unwrap().snapshot(), &before);
        assert!(!host.assistants.current.as_ref().unwrap().compaction_pending());
    }
}
