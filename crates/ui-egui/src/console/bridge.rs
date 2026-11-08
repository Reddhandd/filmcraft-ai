//! Authenticated MCP-to-UI command dispatch and legacy explicit-review tickets.
use super::*;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
#[derive(Clone, Debug)]
struct Ticket {
    created: Instant,
    result: Option<Result<Value, String>>,
}
#[derive(Clone, Debug, Default)]
pub struct State {
    tickets: BTreeMap<String, Ticket>,
}
impl State {
    fn insert(&mut self, id: String) -> Result<(), String> {
        self.tickets.retain(|_, t| t.created.elapsed() < Duration::from_secs(610));
        if self.tickets.len() >= 32 {
            let oldest = self.tickets.iter().filter(|(_, t)| t.result.is_some()).min_by_key(|(_, t)| t.created).map(|(id, _)| id.clone());
            if let Some(id) = oldest {
                self.tickets.remove(&id);
            } else {
                return Err("Too many outstanding console requests".into());
            }
        }
        self.tickets.insert(id, Ticket { created: Instant::now(), result: None });
        Ok(())
    }
    pub(super) fn complete(&mut self, id: &Value, result: Result<Value, String>) -> bool {
        let Some(ticket) = id.as_str().and_then(|id| self.tickets.get_mut(id)) else { return false };
        if ticket.result.is_none() {
            ticket.result = Some(result);
        }
        true
    }
}
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn ticket_id() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| format!("Cannot create console review ticket: {e}"))?;
    Ok(format!("console-{}", bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()))
}
#[cfg(target_arch = "wasm32")]
pub(super) fn ticket_id() -> Result<String, String> {
    Err("The console controller requires the desktop app".into())
}
pub(super) fn cancel_pending(app: &mut FilmcraftApp, reason: &str) {
    if let Some(p) = app.ui.console.chat.pending.take() {
        app.ui.console.bridge.complete(&p.id, Err(reason.into()));
    }
}
pub(super) fn invoke(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, p: &serde_json::Map<String, Value>) -> Result<Value, String> {
    if super::assistants::maintenance_active(app)
        && !matches!(id, "console.bridge.memory" | "console.bridge.interface" | "console.bridge.commands" | "console.bridge.capabilities")
    {
        return Err("Memory maintenance only permits reference-memory access; finish or cancel it before project operations".into());
    }
    match id {
        "console.bridge.interface" => Ok(
            json!({"requestDiagnostics":app.ui.console.diagnostics.json_recent(8),"revision":app.session.revision,"activeSequence":app.session.state.active_sequence.map(|id|id.0),"playhead":app.session.state.active_sequence.and_then(|id|app.session.state.playheads.get(&id)).map(|t|t.0),"selectionCount":app.session.state.selection.len(),"selection":app.session.state.selection.iter().take(64).map(|id|id.0).collect::<Vec<_>>(),"selectionTruncated":app.session.state.selection.len()>64,"console":{"open":app.ui.console.open,"busy":app.ui.console.chat.busy,"voiceOn":app.ui.console.chat.voice.on(),"microphoneMuted":app.ui.console.chat.voice_muted},"coverage":"Compact status; use sequence_inspect kind selection for all selected IDs, command_list for native interface actions. Chat history and widget trees are omitted."}),
        ),
        "console.bridge.memory" => super::sessions::memory(app, p),
        "console.bridge.commands" => super::execution::catalog(app, p),
        "console.bridge.execute" => super::execution::run(app, ctx, p),
        "console.bridge.batch" => super::execution::batch(app, p),
        "console.bridge.prepare" => {
            if let Err(error) = super::assistants::poll(app) {
                app.ui.status = error;
            }
            let command = p.get("command").and_then(Value::as_str).ok_or("Missing command")?;
            let params = p.get("params").cloned().unwrap_or_else(|| json!({}));
            let prepared = super::execution::prepare(app, command, params.clone())?;
            super::assistants::proposed(app, command, &params);
            Ok(prepared)
        }
        "console.bridge.analyze" => super::analysis::start(app, p),
        "console.bridge.analysis.read" => super::analysis::read(app, p),
        "console.bridge.capabilities" => Ok(json!({"version":capabilities::VERSION,"operatingGuide":capabilities::operating_guide(),
            "legacyPreviewEdits":capabilities::EDIT_COMMANDS.iter().filter_map(|id|filmcraft_engine::commands::find(id).map(|spec|json!({"command":id,"params":super::analysis::schema(id).unwrap_or(spec.params)}))).collect::<Vec<_>>(),
            "ownedApplications":(["clip.analyzeMusic.apply","clip.matchPerformance.apply","clip.trimPerformancePassage"].iter().map(|id|json!({"command":id,"params":super::analysis::schema(id)})).collect::<Vec<_>>()),"analysis":[{"kind":"passages","song":"studio audio timeline clip ID","video":"live video timeline clip ID","durationSeconds":{"min":2,"max":600}},{"kind":"music","song":"audio timeline clip ID","durationSeconds":{"min":6,"max":600}},{"kind":"performance","song":"studio audio timeline clip ID","video":"live video timeline clip ID","durationSeconds":{"min":2,"max":600}}],"execution":"command_run executes native commands directly. command_batch groups supported project edits into one Undo step. No routine Apply/Reject is required.","commandDiscovery":"command_list: live engine and UI commands with filtering and pagination",
            "activeSequence":app.session.state.active_sequence.map(|id|id.0),"revision":app.session.revision,"ticksPerSecond":filmcraft_time::TICKS_PER_SECOND})),
        "console.bridge.propose" => {
            if app.ui.console.connection != ConnectionStatus::Ready || !app.ui.console.chat.busy {
                return Err("A live console turn is required to create a proposal".into());
            }
            if app.ui.console.chat.pending.is_some() {
                return Err("Another edit is awaiting review; finish it first".into());
            }
            let command = p.get("command").and_then(Value::as_str).ok_or("Missing edit command")?;
            let params = p.get("params").filter(|v| v.is_object()).cloned().ok_or("Edit parameters must be an object")?;
            let ticket = ticket_id()?;
            let proposal = if matches!(command, "clip.analyzeMusic.apply" | "clip.matchPerformance.apply") {
                super::analysis::proposal(app, json!(ticket), command, params)?
            } else {
                chat::Proposal::prepare(&app.session, json!(ticket), command.into(), params)?
            };
            app.ui.console.bridge.insert(ticket.clone())?;
            if let Err(error) = super::assistants::poll(app) {
                app.ui.status = error;
            }
            super::assistants::proposed(app, command, &p.get("params"));
            app.ui.console.chat.pending = Some(proposal);
            app.ui.console.open = true;
            Ok(json!({"ticket":ticket,"state":"pending","reviewRequired":true}))
        }
        "console.bridge.status" => {
            let id = p.get("ticket").and_then(Value::as_str).filter(|id| id.len() <= 80).ok_or("Invalid console ticket")?;
            let ticket = app.ui.console.bridge.tickets.get(id).ok_or("Unknown or expired console ticket; inspect before retrying")?;
            Ok(match &ticket.result {
                None => json!({"state":"pending"}),
                Some(Ok(value)) => json!({"state":"done","result":value}),
                Some(Err(error)) => json!({"state":"failed","error":error}),
            })
        }
        "console.bridge.cancel" => {
            let id = p.get("ticket").and_then(Value::as_str).filter(|id| id.len() <= 80).ok_or("Invalid console ticket")?;
            if app.ui.console.chat.pending.as_ref().is_some_and(|pending| pending.id.as_str() == Some(id)) {
                cancel_pending(app, "Console request cancelled");
            }
            if !app.ui.console.bridge.complete(&json!(id), Err("Console request cancelled".into())) {
                return Err("Unknown console ticket".into());
            }
            Ok(json!({"cancelled":true}))
        }
        _ => Err("Unknown console bridge operation".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    fn app() -> FilmcraftApp {
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.session.execute("file.newSequence", json!({"width":16,"height":16})).unwrap();
        app.ui.console.connection = ConnectionStatus::Ready;
        app.ui.console.chat.busy = true;
        app
    }
    fn call(app: &mut FilmcraftApp, method: &str, params: Value) -> Result<Value, String> {
        invoke(app, &egui::Context::default(), method, params.as_object().unwrap())
    }
    fn propose(app: &mut FilmcraftApp) -> Value {
        call(app, "console.bridge.propose", json!({"command":"markers.add","params":{"target":"program","time":0,"name":"Review"}})).unwrap()
    }
    #[test]
    fn review_ticket_waits_and_cancel_never_edits() {
        let mut app = app();
        let before = Arc::clone(&app.session.project);
        let started = propose(&mut app);
        let ticket = started.get("ticket").unwrap();
        assert_eq!(*before, *app.session.project);
        assert_eq!(call(&mut app, "console.bridge.status", json!({"ticket":ticket})).unwrap()["state"], "pending");
        assert!(call(&mut app, "console.bridge.propose", json!({"command":"edit.undo","params":{}})).is_err());
        call(&mut app, "console.bridge.cancel", json!({"ticket":ticket})).unwrap();
        assert!(app.ui.console.chat.pending.is_none());
        assert_eq!(*before, *app.session.project);
        assert_eq!(call(&mut app, "console.bridge.status", json!({"ticket":ticket})).unwrap()["state"], "failed");
    }
    #[test]
    fn human_apply_reject_and_stale_review_return_actual_outcomes() {
        let mut app = app();
        let ctx = egui::Context::default();
        let before = Arc::clone(&app.session.project);
        let rejected = propose(&mut app);
        super::super::commands::invoke(&mut app, &ctx, "console.chat.reject", json!({})).unwrap();
        assert_eq!(*before, *app.session.project);
        assert_eq!(call(&mut app, "console.bridge.status", json!({"ticket":rejected["ticket"]})).unwrap()["state"], "failed");
        let approved = propose(&mut app);
        super::super::commands::invoke(&mut app, &ctx, "console.chat.approve", json!({})).unwrap();
        let result = call(&mut app, "console.bridge.status", json!({"ticket":approved["ticket"]})).unwrap();
        assert_eq!(result["result"]["status"], "applied");
        assert_ne!(*before, *app.session.project);
        app.session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(*before, *app.session.project);
        let stale = propose(&mut app);
        app.session.state.source_playhead.0 += 1;
        assert!(super::super::commands::invoke(&mut app, &ctx, "console.chat.approve", json!({})).is_err());
        assert_eq!(*before, *app.session.project);
        assert_eq!(call(&mut app, "console.bridge.status", json!({"ticket":stale["ticket"]})).unwrap()["state"], "failed");
    }
    #[test]
    fn imported_media_sequence_placement_and_linked_delete_share_review_and_undo() {
        let mut app = app();
        let ctx = egui::Context::default();
        app.session.execute("file.openDemoProject", json!({})).unwrap();
        let media = app.session.project.items.values().find(|item| item.has_video() && item.has_audio() && item.as_media().is_some()).unwrap().id;
        let before = Arc::clone(&app.session.project);
        for (command, params) in [
            ("file.newSequence", json!({"fromItem":media.0,"name":"Console integration"})),
            ("timeline.place", json!({"item":media.0,"track":"V2","time":0,"insert":false,"video":true,"audio":false})),
        ] {
            let started = call(&mut app, "console.bridge.propose", json!({"command":command,"params":params})).unwrap();
            let preview = Arc::clone(&app.session.project);
            assert_eq!(call(&mut app, "console.bridge.status", json!({"ticket":started["ticket"]})).unwrap()["state"], "pending");
            super::super::commands::invoke(&mut app, &ctx, "console.chat.approve", json!({})).unwrap();
            assert_ne!(*preview, *app.session.project);
        }
        let sequence = app.session.active_sequence().unwrap();
        let clip = sequence.video_tracks[0].items[0].id;
        let linked = sequence.video_tracks[0].items[0].link.unwrap();
        let partners =
            sequence.audio_tracks.iter().flat_map(|track| &track.items).filter(|item| item.link == Some(linked)).map(|item| item.id).collect::<Vec<_>>();
        assert!(!partners.is_empty());
        let before_delete = Arc::clone(&app.session.project);
        call(&mut app, "console.bridge.propose", json!({"command":"edit.clear","params":{"clips":[clip.0]}})).unwrap();
        assert_eq!(*before_delete, *app.session.project);
        super::super::commands::invoke(&mut app, &ctx, "console.chat.approve", json!({})).unwrap();
        let sequence = app.session.active_sequence().unwrap();
        assert!(sequence.find_item(clip).is_none());
        assert!(partners.iter().all(|id| sequence.find_item(*id).is_none()));
        app.session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(*before_delete, *app.session.project);
        app.session.execute("edit.undo", json!({})).unwrap();
        app.session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(*before, *app.session.project);
    }
    #[test]
    fn completed_ticket_cannot_be_overwritten() {
        let mut state = State::default();
        state.insert("one".into()).unwrap();
        assert!(state.complete(&json!("one"), Ok(json!({"status":"applied"}))));
        assert!(state.complete(&json!("one"), Err("late cancellation".into())));
        assert!(state.tickets["one"].result.as_ref().unwrap().is_ok());
        assert!(!state.complete(&json!("unknown"), Ok(json!({}))));
        for i in 0..32 {
            state.insert(format!("pending-{i}")).unwrap();
        }
        assert!(state.insert("overflow".into()).is_err());
    }
}
