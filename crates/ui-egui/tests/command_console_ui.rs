//! Console input and control-channel commands share dispatch and Undo.
use std::sync::mpsc::channel;

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::{
    FilmcraftApp,
    console::{CodexConnection, ConnectionStatus},
    control::ControlRequest,
};
use std::sync::{Arc, Mutex};

struct FakeConnection {
    calls: Arc<Mutex<[usize; 2]>>,
    pending: bool,
}
impl CodexConnection for FakeConnection {
    fn connect(&mut self) -> Result<(), String> {
        let mut calls = self.calls.lock().unwrap();
        calls[0] += 1;
        if calls[0] == 2 {
            return Err("Synthetic connection failure; retry".into());
        }
        self.pending = true;
        Ok(())
    }
    fn poll(&mut self) -> Option<ConnectionStatus> {
        std::mem::take(&mut self.pending).then_some(ConnectionStatus::Ready)
    }
    fn disconnect(&mut self) {
        self.calls.lock().unwrap()[1] += 1;
        self.pending = false;
    }
}
use serde_json::{Value, json};

#[test]
fn console_is_drivable_and_preserves_edit_history() {
    let (tx, rx) = channel();
    let calls = Arc::new(Mutex::new([0, 0]));
    let mut app = FilmcraftApp::new(Session::default()).with_control(rx);
    app.hooks.codex_connection = Some(Box::new(FakeConnection { calls: Arc::clone(&calls), pending: false }));
    let capture = std::env::var_os("FILMCRAFT_CONSOLE_SCREENSHOT");
    let mut builder = Harness::builder().with_size(egui::vec2(1300.0, 900.0)).with_max_steps(10000);
    if capture.is_some() {
        builder = builder.wgpu();
    }
    let mut harness = builder.build_eframe(move |_cc| app);
    fn frame(harness: &mut Harness<'_, FilmcraftApp>) {
        let ctx = harness.ctx.clone();
        let mut raw = std::mem::take(harness.input_mut());
        eframe::App::raw_input_hook(harness.state_mut(), &ctx, &mut raw);
        *harness.input_mut() = raw;
        harness.step();
    }
    fn request(harness: &mut Harness<'_, FilmcraftApp>, tx: &std::sync::mpsc::Sender<ControlRequest>, method: &str, params: Value) -> Value {
        let (request, response) = ControlRequest::new(method, params);
        tx.send(request).unwrap();
        for _ in 0..60 {
            frame(harness);
            if let Ok(value) = response.try_recv() {
                return value;
            }
        }
        panic!("no reply to {method}");
    }
    fn call(harness: &mut Harness<'_, FilmcraftApp>, tx: &std::sync::mpsc::Sender<ControlRequest>, command: &str, params: Value) -> Value {
        request(harness, tx, "engine.execute", json!({"command": command, "params": params}))
    }

    let before = harness.state().session.project.clone();
    assert_eq!(call(&mut harness, &tx, "window.commandConsole", json!({}))["ok"], true);
    assert_eq!(call(&mut harness, &tx, "console.run", json!({"line": "file.newSequence {\"width\":16,\"height\":16}"}))["ok"], true);
    assert_ne!(*harness.state().session.project, *before);
    assert_eq!(call(&mut harness, &tx, "console.run", json!({"line": "edit.undo"}))["ok"], true);
    assert_eq!(*harness.state().session.project, *before);
    assert_eq!(call(&mut harness, &tx, "console.run", json!({"line": "file.newSequence []"}))["ok"], false);
    for _ in 0..4 {
        frame(&mut harness);
    }
    assert!(harness.state().auto.find("console.input").is_none(), "Manual controls should start collapsed");
    assert!(harness.state().auto.find("console.undo").is_some(), "Undo remains visible outside advanced controls");
    assert_eq!(request(&mut harness, &tx, "ui.click", json!({"id":"console.advanced.toggle"}))["ok"], true);
    for _ in 0..12 {
        frame(&mut harness);
    }
    for id in ["console.input", "console.run", "console.undo", "console.clear", "console.history.0"] {
        assert!(harness.state().auto.find(id).is_some(), "missing {id}");
    }
    // Added connection controls can move the manual command section below the
    // initial scroll viewport. Scroll like a user before clicking its widgets.
    let undo_rect = harness.state().auto.find("console.undo").unwrap().rect;
    let scroll_point = json!({"x":undo_rect[0] + 10.0,"y":undo_rect[1] + 45.0,"dy":-600.0});
    assert_eq!(request(&mut harness, &tx, "ui.scroll", scroll_point)["ok"], true);
    for _ in 0..12 {
        frame(&mut harness);
    }
    assert_eq!(request(&mut harness, &tx, "ui.click", json!({"id": "console.input"}))["ok"], true);
    assert_eq!(request(&mut harness, &tx, "ui.type", json!({"text": "file.newSequence {\"width\":16,\"height\":16}"}))["ok"], true);
    assert_eq!(request(&mut harness, &tx, "ui.click", json!({"id": "console.run"}))["ok"], true);
    for _ in 0..4 {
        frame(&mut harness);
    }
    assert_ne!(*harness.state().session.project, *before, "the Run button must dispatch typed input");
    assert_eq!(request(&mut harness, &tx, "ui.scroll", json!({"x":undo_rect[0] + 10.0,"y":undo_rect[1] + 45.0,"dy":600.0}))["ok"], true);
    for _ in 0..12 {
        frame(&mut harness);
    }
    assert_eq!(request(&mut harness, &tx, "ui.click", json!({"id": "console.undo"}))["ok"], true);
    for _ in 0..4 {
        frame(&mut harness);
    }
    assert_eq!(*harness.state().session.project, *before, "the Undo button must restore the project");
    for id in ["console.codex.toggle", "console.codex.connect", "console.codex.disconnect"] {
        assert!(harness.state().auto.find(id).is_some(), "missing {id}");
    }
    assert_eq!(request(&mut harness, &tx, "ui.click", json!({"id": "console.codex.connect"}))["ok"], true);
    for _ in 0..4 {
        frame(&mut harness);
    }
    assert_eq!(harness.state().ui.console.connection, ConnectionStatus::Ready);
    assert_eq!(request(&mut harness, &tx, "ui.inspect", json!({}))["result"]["console"]["codex"], "ready");
    assert_eq!(request(&mut harness, &tx, "ui.click", json!({"id": "console.codex.disconnect"}))["ok"], true);
    for _ in 0..4 {
        frame(&mut harness);
    }
    assert_eq!(harness.state().ui.console.connection, ConnectionStatus::Disconnected);
    assert_eq!(request(&mut harness, &tx, "ui.click", json!({"id": "console.codex.connect"}))["ok"], true);
    for _ in 0..4 {
        frame(&mut harness);
    }
    assert!(matches!(harness.state().ui.console.connection, ConnectionStatus::Failed(_)));
    assert_eq!(request(&mut harness, &tx, "ui.click", json!({"id": "console.codex.connect"}))["ok"], true);
    for _ in 0..4 {
        frame(&mut harness);
    }
    assert_eq!(harness.state().ui.console.connection, ConnectionStatus::Ready);
    assert_eq!(request(&mut harness, &tx, "ui.click", json!({"id": "console.codex.disconnect"}))["ok"], true);
    for _ in 0..4 {
        frame(&mut harness);
    }
    assert_eq!(calls.lock().unwrap()[0], 3);
    assert_eq!(*harness.state().session.project, *before, "connection controls must not edit the project");
    if let Some(path) = capture {
        harness.render().unwrap().save(path).unwrap();
    }
}

struct ChatFake {
    selected_models: Arc<Mutex<Vec<Option<String>>>>,
    events: std::collections::VecDeque<filmcraft_ui_egui::console::ChatEvent>,
    answers: Arc<Mutex<Vec<bool>>>,
}
impl CodexConnection for ChatFake {
    fn connect(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn poll(&mut self) -> Option<ConnectionStatus> {
        None
    }
    fn disconnect(&mut self) {}
    fn poll_chat(&mut self) -> Option<filmcraft_ui_egui::console::ChatEvent> {
        self.events.pop_front()
    }
    fn send_prompt(&mut self, prompt: String) -> Result<(), String> {
        use filmcraft_ui_egui::console::ChatEvent;
        self.events.push_back(ChatEvent::Delta("I inspected the project. Adding this marker.".into()));
        self.events.push_back(ChatEvent::Proposal {
            id: json!("edit"),
            command: if prompt.contains("invalid") { "invalid.command" } else { "markers.add" }.into(),
            params: json!({"target":"program","time":0,"name":"Verse"}),
        });
        Ok(())
    }
    fn send_prompt_with_model(&mut self, prompt: String, model: Option<String>) -> Result<(), String> {
        self.selected_models.lock().unwrap().push(model);
        self.send_prompt(prompt)
    }
    fn start_voice(&mut self, _model: Option<String>) -> Result<(), String> {
        self.events.push_back(filmcraft_ui_egui::console::ChatEvent::Voice(filmcraft_ui_egui::console::VoiceStatus::Active));
        Ok(())
    }
    fn stop_voice(&mut self) -> Result<(), String> {
        self.events.push_back(filmcraft_ui_egui::console::ChatEvent::Voice(filmcraft_ui_egui::console::VoiceStatus::Off));
        Ok(())
    }
    fn cancel(&mut self) -> Result<(), String> {
        self.events.clear();
        self.events.push_back(filmcraft_ui_egui::console::ChatEvent::Finished);
        Ok(())
    }
    fn answer(&mut self, _id: Value, result: Result<Value, String>) -> Result<(), String> {
        self.answers.lock().unwrap().push(result.is_ok());
        self.events.push_back(filmcraft_ui_egui::console::ChatEvent::Finished);
        Ok(())
    }
}
#[test]
fn chat_buttons_execute_directly_undo_report_failures_and_cancel() {
    let answers = Arc::new(Mutex::new(Vec::new()));
    let selected_models = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = channel();
    let mut app = FilmcraftApp::new(Session::default()).with_control(rx);
    app.session.execute("file.newSequence", json!({"width":16,"height":16})).unwrap();
    app.ui.console.open = true;
    app.ui.console.connection = ConnectionStatus::Ready;
    app.hooks.codex_connection = Some(Box::new(ChatFake {
        events: std::collections::VecDeque::from([filmcraft_ui_egui::console::ChatEvent::Models(
            vec![filmcraft_ui_egui::console::ModelOption { id: "test-model".into(), label: "Test model".into(), effort: None }],
            Some("default-model".into()),
        )]),
        answers: Arc::clone(&answers),
        selected_models: Arc::clone(&selected_models),
    }));
    let before = app.session.project.clone();
    let mut harness = Harness::builder().with_size(egui::vec2(1300.0, 1000.0)).with_max_steps(10000).build_eframe(move |_cc| app);
    fn frame(h: &mut Harness<'_, FilmcraftApp>) {
        let ctx = h.ctx.clone();
        let mut raw = std::mem::take(h.input_mut());
        eframe::App::raw_input_hook(h.state_mut(), &ctx, &mut raw);
        *h.input_mut() = raw;
        h.step();
    }
    fn call(h: &mut Harness<'_, FilmcraftApp>, tx: &std::sync::mpsc::Sender<ControlRequest>, method: &str, params: Value) -> Value {
        let (req, rx) = ControlRequest::new(method, params);
        tx.send(req).unwrap();
        for _ in 0..60 {
            frame(h);
            if let Ok(v) = rx.try_recv() {
                return v;
            }
        }
        panic!("No reply to {method}")
    }
    fn click(h: &mut Harness<'_, FilmcraftApp>, tx: &std::sync::mpsc::Sender<ControlRequest>, id: &str) {
        assert_eq!(call(h, tx, "ui.click", json!({"id":id}))["ok"], true);
        for _ in 0..4 {
            frame(h);
        }
    }
    fn prompt(h: &mut Harness<'_, FilmcraftApp>, tx: &std::sync::mpsc::Sender<ControlRequest>, text: &str) {
        click(h, tx, "console.chat.input");
        assert_eq!(call(h, tx, "ui.type", json!({"text":text}))["ok"], true);
        click(h, tx, "console.chat.send");
        assert!(h.state().ui.console.chat.pending.is_none());
    }
    for _ in 0..5 {
        frame(&mut harness);
    }
    click(&mut harness, &tx, "console.settings.toggle");
    click(&mut harness, &tx, "console.model");
    click(&mut harness, &tx, "console.model.option.test-model");
    click(&mut harness, &tx, "console.settings.toggle");
    click(&mut harness, &tx, "console.chat.input");
    assert_eq!(call(&mut harness, &tx, "ui.type", json!({"text":"Add a Verse marker at zero"}))["ok"], true);
    assert_eq!(call(&mut harness, &tx, "ui.key", json!({"key":"Shift+Enter"}))["ok"], true);
    for _ in 0..4 {
        frame(&mut harness);
    }
    assert!(selected_models.lock().unwrap().is_empty(), "Shift+Return must not dispatch the full Console draft");
    assert_eq!(*before, *harness.state().session.project, "Shift+Return must not send");
    assert_eq!(call(&mut harness, &tx, "ui.key", json!({"key":"Enter"}))["ok"], true);
    for _ in 0..8 {
        frame(&mut harness);
    }
    assert_eq!(selected_models.lock().unwrap().len(), 1, "Return must dispatch the full Console draft once");
    assert_eq!(selected_models.lock().unwrap().first(), Some(&Some("test-model".into())));
    assert_ne!(*before, *harness.state().session.project);
    assert_eq!(harness.state().session.active_sequence().unwrap().markers.len(), 1);
    assert!(harness.state().ui.console.chat.pending.is_none());
    click(&mut harness, &tx, "console.undo");
    assert_eq!(*before, *harness.state().session.project);
    prompt(&mut harness, &tx, "Try the invalid command");
    assert_eq!(*before, *harness.state().session.project);
    assert_eq!(*answers.lock().unwrap(), [true, false]);
    harness.state_mut().ui.console.chat.busy = true;
    click(&mut harness, &tx, "console.chat.cancel");
    assert!(!harness.state().ui.console.chat.busy);
    assert!(harness.state().ui.console.chat.pending.is_none());
    assert_eq!(*before, *harness.state().session.project);
    // Chat history can move the voice footer below the initial window viewport.
    // Scroll the outer Console from its toolbar before clicking the footer.
    let toolbar = harness.state().auto.find("console.undo").unwrap().rect;
    assert_eq!(call(&mut harness, &tx, "ui.scroll", json!({"x":toolbar[0]+10.0,"y":toolbar[1]+10.0,"dy":-600.0}))["ok"], true);
    for _ in 0..12 {
        frame(&mut harness);
    }
    click(&mut harness, &tx, "console.voice.on");
    assert_eq!(harness.state().ui.console.chat.voice, filmcraft_ui_egui::console::VoiceStatus::Active);
    click(&mut harness, &tx, "console.voice.off");
    assert_eq!(harness.state().ui.console.chat.voice, filmcraft_ui_egui::console::VoiceStatus::Off);
    click(&mut harness, &tx, "console.voice.on");
    harness.state_mut().ui.console.open = false;
    for _ in 0..5 {
        frame(&mut harness);
    }
    assert_eq!(harness.state().ui.console.chat.voice, filmcraft_ui_egui::console::VoiceStatus::Off, "closing the console must stop microphone activity");

    assert!(serde_json::to_value(&harness.state().ui).unwrap().get("console").is_none());
}

#[test]
fn project_memory_and_resume_controls_restore_the_selected_conversation() {
    use filmcraft_ui_egui::console::{ChatEvent, sessions::SessionRequest};
    struct SavedFake {
        events: std::collections::VecDeque<ChatEvent>,
        requests: Arc<Mutex<Vec<SessionRequest>>>,
        ready: bool,
        next: usize,
    }
    impl SavedFake {
        fn session(&mut self, request: SessionRequest) -> Result<(), String> {
            self.next += 1;
            let id = request.resume.clone().unwrap_or_else(|| format!("saved-{}", self.next));
            self.requests.lock().unwrap().push(request);
            self.events.extend([ChatEvent::SessionStarted(id), ChatEvent::Finished]);
            self.ready = true;
            Ok(())
        }
    }
    impl CodexConnection for SavedFake {
        fn connect(&mut self) -> Result<(), String> {
            Err("Expected a saved-project session".into())
        }
        fn connect_session(&mut self, r: SessionRequest) -> Result<(), String> {
            self.session(r)
        }
        fn switch_session(&mut self, r: SessionRequest) -> Result<(), String> {
            self.session(r)
        }
        fn poll(&mut self) -> Option<ConnectionStatus> {
            std::mem::take(&mut self.ready).then_some(ConnectionStatus::Ready)
        }
        fn poll_chat(&mut self) -> Option<ChatEvent> {
            self.events.pop_front()
        }
        fn disconnect(&mut self) {}
    }
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("memory.fcproj");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = channel();
    let mut app = FilmcraftApp::new(Session::default()).with_control(rx);
    app.session.path = Some(project.to_string_lossy().into_owned());
    app.ui.console.open = true;
    app.hooks.codex_connection = Some(Box::new(SavedFake { events: Default::default(), requests: requests.clone(), ready: false, next: 0 }));
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1100.0)).with_max_steps(10000).build_eframe(move |_cc| app);
    fn frame(h: &mut Harness<'_, FilmcraftApp>) {
        let ctx = h.ctx.clone();
        let mut input = std::mem::take(h.input_mut());
        eframe::App::raw_input_hook(h.state_mut(), &ctx, &mut input);
        *h.input_mut() = input;
        h.step();
    }
    fn click(h: &mut Harness<'_, FilmcraftApp>, tx: &std::sync::mpsc::Sender<ControlRequest>, id: &str) {
        let (req, rx) = ControlRequest::new("ui.click", json!({"id":id}));
        tx.send(req).unwrap();
        let mut result = None;
        for _ in 0..30 {
            frame(h);
            if let Ok(r) = rx.try_recv() {
                result = Some(r);
                break;
            }
        }
        assert_eq!(result.unwrap()["ok"], true, "{id}");
        for _ in 0..4 {
            frame(h);
        }
    }
    for _ in 0..4 {
        frame(&mut h);
    }
    click(&mut h, &tx, "console.memory.open");
    h.state_mut().ui.console.sessions.memory = "Verse: hold the singer for eight beats.".into();
    click(&mut h, &tx, "console.memory.save");
    click(&mut h, &tx, "console.memory.open");
    click(&mut h, &tx, "console.codex.connect");
    assert_eq!(h.state().ui.console.sessions.active.as_deref(), Some("saved-1"));
    click(&mut h, &tx, "console.session.new");
    assert_eq!(h.state().ui.console.sessions.active.as_deref(), Some("saved-2"));
    click(&mut h, &tx, "console.sessions.open");
    click(&mut h, &tx, "console.session.saved-1");
    assert_eq!(h.state().ui.console.sessions.active.as_deref(), Some("saved-1"));
    let calls = requests.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[2].resume.as_deref(), Some("saved-1"));
    assert!(calls[2].memory.starts_with("Verse: hold the singer for eight beats."));
    assert!(calls[2].memory.contains("Current selected creative defaults"));
    assert_eq!(std::fs::read_to_string(format!("{}.console/memory.md", project.display())).unwrap(), "Verse: hold the singer for eight beats.");
    assert_eq!(h.state().ui.console.chat.voice, filmcraft_ui_egui::console::VoiceStatus::Off);
    assert!(h.state().ui.console.chat.pending.is_none());
}
