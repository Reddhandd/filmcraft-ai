//! Bounded, payload-free control request breadcrumbs. Proximity is not proof of incident cause.
use super::*;
use web_time::Instant;

const LIMIT: usize = 32;
const COMMAND_LIMIT: usize = 16;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Identity {
    revision: u64,
    epoch: u64,
}
impl Identity {
    pub(crate) fn of(app: &FilmcraftApp) -> Self {
        Self { revision: app.session.revision, epoch: app.session.document_epoch }
    }
    fn json(self) -> Value {
        json!({"revision":self.revision,"documentEpoch":self.epoch})
    }
}
#[derive(Clone, Debug)]
struct Entry {
    id: u64,
    started: Instant,
    utc: String,
    kind: &'static str,
    commands: Vec<&'static str>,
    omitted: usize,
    before: Identity,
    after: Option<Identity>,
    outcome: &'static str,
    elapsed_ms: Option<f64>,
    job: Option<u64>,
}
impl Entry {
    fn json(&self) -> Value {
        json!({"request":self.id,"startedUtc":self.utc,"kind":self.kind,"commands":self.commands,
            "commandsOmitted":self.omitted,"before":self.before.json(),"after":self.after.map(Identity::json),
            "outcome":self.outcome,"elapsedMs":self.elapsed_ms,"job":self.job})
    }
}
#[derive(Clone, Debug, Default)]
pub(crate) struct State {
    next: u64,
    entries: VecDeque<Entry>,
    last_change: Option<Entry>,
    unavailable: bool,
}
fn registered(id: &str) -> &'static str {
    if let Some(spec) = filmcraft_engine::find_command(id) {
        return spec.id;
    }
    menus::UI_COMMANDS.iter().chain(crate::panels::keyboard::COMMANDS).find(|spec| spec.id == id).map_or("<unregistered>", |spec| spec.id)
}
impl State {
    fn begin(&mut self, id: &str, params: &Value, before: Identity) -> Option<u64> {
        let (kind, commands, omitted) = match id {
            "console.bridge.execute" => ("console_command", vec![registered(params.get("command").and_then(Value::as_str).unwrap_or(""))], 0),
            "console.bridge.batch" => {
                let steps = params.get("steps").and_then(Value::as_array);
                let commands =
                    steps.into_iter().flatten().take(COMMAND_LIMIT).map(|step| registered(step.get("id").and_then(Value::as_str).unwrap_or(""))).collect();
                ("console_batch", commands, steps.map_or(0, |steps| steps.len().saturating_sub(COMMAND_LIMIT)))
            }
            _ => ("native_command", vec![registered(id)], 0),
        };
        let Some(next) = self.next.checked_add(1) else {
            self.unavailable = true;
            return None;
        };
        if self.entries.len() == LIMIT {
            self.entries.pop_front();
        }
        if self.entries.try_reserve(1).is_err() {
            self.unavailable = true;
            return None;
        }
        self.next = next;
        self.entries.push_back(Entry {
            id: next,
            started: Instant::now(),
            utc: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            kind,
            commands,
            omitted,
            before,
            after: None,
            outcome: "pending",
            elapsed_ms: None,
            job: None,
        });
        if let Some(entry) = self.entries.back() {
            log::info!(target:"filmcraft::console","Control request {}",entry.json());
        }
        Some(next)
    }
    fn finish(&mut self, id: Option<u64>, after: Identity, result: &Result<Value, String>) {
        let Some(entry) = id.and_then(|id| self.entries.iter_mut().find(|entry| entry.id == id)) else {
            return;
        };
        entry.after = Some(after);
        entry.elapsed_ms = Some(entry.started.elapsed().as_secs_f64() * 1000.0);
        entry.outcome = if result.is_ok() { "returned_ok" } else { "returned_error" };
        entry.job =
            result.as_ref().ok().and_then(|value| value.get("job").or_else(|| value.get("result").and_then(|value| value.get("job")))).and_then(Value::as_u64);
        log::info!(target:"filmcraft::console","Control request {}",entry.json());
        if entry.before != after {
            self.last_change = Some(entry.clone());
        }
    }
    pub(crate) fn json(&self) -> Value {
        self.json_recent(LIMIT)
    }
    pub(crate) fn json_recent(&self, limit: usize) -> Value {
        json!({"recent":self.entries.iter().skip(self.entries.len().saturating_sub(limit.min(LIMIT))).map(Entry::json).collect::<Vec<_>>(),"lastRevisionChange":self.last_change.as_ref().map(Entry::json),"unavailable":self.unavailable,
            "coverage":"Control command admission and return only; the same fixed metadata is written to capped app logs. Returned is not delivery or asynchronous job completion. Pending has unknown outcome; inspect before retrying. Last revision change is a clue, not proof of cause. No parameters, prompts, paths or result/error payloads retained."})
    }
}
pub(crate) fn begin(app: &mut FilmcraftApp, id: &str, params: &Value) -> Option<u64> {
    let before = Identity::of(app);
    app.ui.console.diagnostics.begin(id, params, before)
}
pub(crate) fn finish(app: &mut FilmcraftApp, id: Option<u64>, result: &Result<Value, String>) {
    let after = Identity::of(app);
    app.ui.console.diagnostics.finish(id, after, result);
}
pub(super) fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let section = egui::CollapsingHeader::new(tl!("Command diagnostics")).show(ui, |ui| {
        ui.label(tl!("Recent command returns are clues, not proof of a failure's cause. Pending outcomes must be inspected before retrying."));
        ui.label(tl!("Connection setup logs are available through Help → Reveal Log Files."));
        if let Some(entry) = &app.ui.console.diagnostics.last_change {
            ui.label(format!("Last observed revision change: request #{} · {}", entry.id, entry.commands.join(", ")));
        }
        if app.ui.console.diagnostics.unavailable {
            ui.label(tl!("Some diagnostics could not be recorded."));
        }
        if app.ui.console.diagnostics.entries.is_empty() {
            ui.label(tl!("No control commands recorded in this app session."));
        }
        for entry in app.ui.console.diagnostics.entries.iter().rev() {
            ui.label(format!(
                "#{} {} · {} · {} · {}",
                entry.id,
                entry.utc,
                entry.commands.join(", "),
                entry.outcome,
                entry.elapsed_ms.map_or_else(|| "pending".into(), |ms| format!("{ms:.1} ms"))
            ));
            if let Some(after) = entry.after {
                ui.small(format!(
                    "Revision {} → {} · Document {} → {}{}",
                    entry.before.revision,
                    after.revision,
                    entry.before.epoch,
                    after.epoch,
                    entry.job.map_or_else(String::new, |job| format!(" · Job {job}"))
                ));
            }
        }
    });
    app.auto.add("console.diagnostics", section.header_response.rect, "Command diagnostics");
}

#[cfg(test)]
mod tests {
    use super::*;
    fn identity(revision: u64) -> Identity {
        Identity { revision, epoch: 1 }
    }
    #[test]
    fn bounded_history_preserves_last_revision_change_and_never_retains_payloads() {
        let mut state = State::default();
        let id = state.begin("markers.add", &json!({"name":"SECRET PROMPT /private/path token"}), identity(1));
        state.finish(id, identity(2), &Ok(json!({"job":7,"payload":"SECRET RESULT"})));
        for _ in 0..100 {
            let id = state.begin("an unknown SECRET COMMAND", &Value::Null, identity(2));
            state.finish(id, identity(2), &Err("SECRET ERROR".into()));
        }
        let json = state.json();
        assert_eq!(json["recent"].as_array().unwrap().len(), LIMIT);
        assert_eq!(json["lastRevisionChange"]["commands"][0], "markers.add");
        assert_eq!(json["lastRevisionChange"]["job"], 7);
        assert_eq!(json["lastRevisionChange"]["outcome"], "returned_ok");
        assert!(!json.to_string().contains("SECRET"));
    }
    #[test]
    fn batches_and_pending_outcomes_are_bounded_without_fabricated_completion() {
        let mut state = State::default();
        let steps = vec![json!({"id":"effects.setParam","params":{"private":"SECRET"}}); 1000];
        let id = state.begin("console.bridge.batch", &json!({"label":"SECRET LABEL","steps":steps}), identity(1));
        let json = state.json();
        assert_eq!(json["recent"][0]["commands"].as_array().unwrap().len(), COMMAND_LIMIT);
        assert_eq!(json["recent"][0]["commandsOmitted"], 984);
        assert_eq!(json["recent"][0]["outcome"], "pending");
        assert!(json["recent"][0]["after"].is_null());
        assert!(!json.to_string().contains("SECRET"));
        state.finish(id, identity(1), &Ok(json!({"job":"SECRET"})));
        assert!(state.json()["recent"][0]["job"].is_null());
        state.next = u64::MAX;
        assert!(state.begin("markers.add", &Value::Null, identity(1)).is_none());
        assert!(state.json()["unavailable"].as_bool().unwrap());
    }
    #[test]
    fn authenticated_control_dispatch_records_underlying_command_and_revisions() {
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        let ctx = egui::Context::default();
        let (request, _) = crate::control::ControlRequest::new(
            "engine.execute",
            json!({"command":"console.bridge.execute","params":{"command":"file.newSequence","params":{"width":16,"height":16,"name":"SECRET SEQUENCE"}}}),
        );
        let crate::control::Outcome::Done(reply) = crate::control::handle(&mut app, &ctx, &request) else { panic!("expected a direct command return") };
        assert_eq!(reply["ok"], true, "{reply}");
        let json = app.ui.console.diagnostics.json();
        assert_eq!(json["recent"][0]["commands"][0], "file.newSequence");
        assert_eq!(json["recent"][0]["outcome"], "returned_ok");
        assert_ne!(json["recent"][0]["before"], json["recent"][0]["after"]);
        assert!(!json.to_string().contains("SECRET"));
        assert!(serde_json::to_vec(&app.ui.console.diagnostics.json_recent(8)).unwrap().len() < 24 * 1024);
    }
}
