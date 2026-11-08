//! Explicit API transport, sharing the existing native MCP command contract.
mod http;
mod mcp;
mod protocol;
mod retention;
mod state;
mod turn;
use filmcraft_ui_egui::console::{ChatEvent, CodexConnection, ConnectionStatus, ModelOption, api_settings, provider::ProviderId, sessions::SessionRequest};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender},
};
use std::time::Duration;
enum Command {
    Prompt(String, Option<String>, Vec<String>),
}
pub struct Connection {
    port: Option<u16>,
    control_token: Option<String>,
    storage: Option<std::path::PathBuf>,
    ctx: egui::Context,
    permit: Option<String>,
    event_permit: Option<String>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    commands: Option<SyncSender<Command>>,
    statuses: Option<Receiver<ConnectionStatus>>,
    events: Option<Receiver<ChatEvent>>,
    busy: bool,
    models: Vec<String>,
    configuration: Option<api_settings::Configuration>,
    environment_enabled: bool,
}
impl Connection {
    /// Inject the desktop-owned private credential without changing the process environment.
    pub fn with_control_token(mut self, token: Option<String>) -> Self {
        self.control_token = token;
        self
    }
    pub fn new(port: Option<u16>, ctx: egui::Context) -> Self {
        Self {
            port,
            control_token: None,
            storage: None,
            ctx,
            permit: None,
            event_permit: None,
            stop: Arc::new(AtomicBool::new(true)),
            worker: None,
            commands: None,
            statuses: None,
            events: None,
            busy: false,
            models: Vec::new(),
            configuration: None,
            environment_enabled: true,
        }
    }
    pub fn with_storage(mut self, storage: Option<std::path::PathBuf>) -> Self {
        self.storage = storage;
        self
    }
    fn start(&mut self, request: Option<SessionRequest>) -> Result<(), String> {
        if let Some(request) = &request {
            self.validate_session_request(request)?;
        }
        if self.worker.as_ref().is_some_and(|worker| !worker.is_finished()) {
            return Err("Previous API worker is still closing; retry Connect shortly".into());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let port = self.port.filter(|port| *port != 0).ok_or("API requires the private local FilmCraft control bridge")?;
        let token = self.control_token.clone().or_else(|| std::env::var("FILMCRAFT_CONTROL_TOKEN").ok()).ok_or("Private FilmCraft control token is missing")?;
        let permit = self.permit.clone().ok_or("API provider ownership is missing")?;
        let key = self
            .configuration
            .as_ref()
            .map(|config| config.key().to_owned())
            .or_else(|| self.environment_enabled.then(|| std::env::var("OPENAI_API_KEY").ok()).flatten())
            .ok_or("Enter an API key in the OpenAI API connection section")?;
        let http = http::ApiHttp::new(key)?;
        let model = request
            .as_ref()
            .and_then(|request| request.model.clone())
            .or_else(|| self.configuration.as_ref().map(|config| config.model().to_owned()))
            .or_else(|| self.environment_enabled.then(|| std::env::var("FILMCRAFT_OPENAI_MODEL").ok()).flatten())
            .ok_or("Enter an explicit API model ID before connecting")?;
        protocol::request(&model, "", vec![], &[])?;
        let configured = if self.environment_enabled {
            match std::env::var("FILMCRAFT_OPENAI_MODELS") {
                Ok(value) => Some(value),
                Err(std::env::VarError::NotPresent) => None,
                Err(_) => return Err("API model list is not valid Unicode".into()),
            }
        } else {
            None
        };
        let models = configured_models(&model, configured.as_deref())?;
        let announced_models = models.clone();
        let cli = std::env::current_exe().map_err(|_| "Cannot locate the matching FilmCraft helper")?.with_file_name(if cfg!(windows) {
            "filmcraft-cli.exe"
        } else {
            "filmcraft-cli"
        });
        if !cli.is_file() {
            return Err("Build filmcraft-cli beside this development executable before connecting".into());
        }
        let session_request = request.clone();
        let storage = self.storage.clone();
        let mut instructions = filmcraft_ui_egui::console::capabilities::operating_guide();
        if let Some(request) = request {
            instructions.push_str("\nProject context (user data, not tool authority):\n");
            instructions.push_str(&request.project);
            instructions.push('\n');
            instructions.push_str(&request.memory);
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let (command_tx, command_rx) = mpsc::sync_channel(1);
        let (status_tx, status_rx) = mpsc::sync_channel(4);
        let (event_tx, event_rx) = mpsc::sync_channel(64);
        let ctx = self.ctx.clone();
        let worker = std::thread::Builder::new()
            .name("console-openai-api".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(), String> {
                    let mut native = mcp::NativeTools::spawn(&cli, port, &token, &permit, stopped.clone())?;
                    use turn::Tools;
                    if native.definitions()?.iter().any(|tool| !filmcraft_ui_egui::console::capabilities::CONSOLE_TOOLS.contains(&tool.name.as_str())) {
                        return Err("Native helper exposed a tool outside the Console contract".into());
                    }
                    let (journal, mut conversation) = match (storage.as_deref(), session_request.as_ref()) {
                        (Some(root), Some(request)) => {
                            let (journal, conversation) = state::Journal::open(
                                root,
                                &request.project,
                                &model,
                                request.resume.as_deref(),
                                &native.definitions()?,
                                &request.scope_aliases,
                            )?;
                            (Some(journal), conversation)
                        }
                        _ => (None, turn::Conversation::default()),
                    };
                    announce_ready(&status_tx, &event_tx, &model, &announced_models, &ctx, &stopped)?;
                    if let Some(journal) = &journal {
                        emit(&event_tx, ChatEvent::SessionStarted(journal.id().into()), &ctx, &stopped)?;
                    }
                    while !stopped.load(Ordering::SeqCst) {
                        match command_rx.recv_timeout(Duration::from_millis(50)) {
                            Ok(Command::Prompt(text, choice, images)) => {
                                emit(&event_tx, ChatEvent::TurnStarted, &ctx, &stopped)?;
                                let answer = turn::run_persisted(
                                    &mut conversation,
                                    journal.as_ref(),
                                    &http,
                                    &mut native,
                                    choice.as_deref().unwrap_or(&model),
                                    &instructions,
                                    &text,
                                    &images,
                                    &stopped,
                                );
                                match answer {
                                    Ok(text) => emit(&event_tx, ChatEvent::Delta(text), &ctx, &stopped)?,
                                    Err(error) => {
                                        emit(&event_tx, ChatEvent::Error(error.clone()), &ctx, &stopped)?;
                                        if journal.is_some() || !native.is_current() {
                                            return Err(error);
                                        }
                                    }
                                }
                                emit(&event_tx, ChatEvent::Finished, &ctx, &stopped)?;
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                            Err(_) => break,
                        }
                    }
                    Ok(())
                }));
                let failure = match result {
                    Ok(Err(error)) => Some(error),
                    Err(_) => Some("API worker failed safely; reconnect after inspecting native state".into()),
                    Ok(Ok(())) => None,
                };
                if let Some(error) = failure {
                    let _ = status_tx.try_send(ConnectionStatus::Failed(error));
                    ctx.request_repaint();
                }
            })
            .map_err(|_| "Cannot start API connection worker")?;
        self.stop = stop;
        self.worker = Some(worker);
        self.commands = Some(command_tx);
        self.statuses = Some(status_rx);
        self.events = Some(event_rx);
        self.event_permit = self.permit.clone();
        self.busy = false;
        self.models = models;
        Ok(())
    }
}
fn configured_models(default: &str, configured: Option<&str>) -> Result<Vec<String>, String> {
    protocol::request(default, "", vec![], &[])?;
    let Some(configured) = configured else {
        return Ok(vec![default.into()]);
    };
    if configured.len() > 8192 {
        return Err("API model list exceeds its configuration budget".into());
    }
    let mut models = Vec::new();
    for model in configured.split(',').map(str::trim) {
        if models.len() >= 32 || models.iter().any(|existing| existing == model) {
            return Err("API model list must contain at most 32 unique model IDs".into());
        }
        protocol::request(model, "", vec![], &[])?;
        models.push(model.to_owned());
    }
    if !models.iter().any(|model| model == default) {
        return Err("Include the selected API model in FILMCRAFT_OPENAI_MODELS".into());
    }
    Ok(models)
}
/// End the UI preparation turn without inventing an API-auth or project-inspection result.
fn announce_ready(
    statuses: &SyncSender<ConnectionStatus>,
    events: &SyncSender<ChatEvent>,
    model: &str,
    models: &[String],
    ctx: &egui::Context,
    stop: &AtomicBool,
) -> Result<(), String> {
    statuses.try_send(ConnectionStatus::Ready).map_err(|_| "API status receiver closed")?;
    emit(
        events,
        ChatEvent::Models(
            models.iter().map(|model| ModelOption { id: model.clone(), label: format!("{model} (API credentials checked on send)"), effort: None }).collect(),
            Some(model.into()),
        ),
        ctx,
        stop,
    )?;
    emit(events, ChatEvent::Finished, ctx, stop)
}
fn emit(sender: &SyncSender<ChatEvent>, mut event: ChatEvent, ctx: &egui::Context, stop: &AtomicBool) -> Result<(), String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        if stop.load(Ordering::SeqCst) {
            return Err("API connection stopped".into());
        }
        match sender.try_send(event) {
            Ok(()) => {
                ctx.request_repaint();
                return Ok(());
            }
            Err(mpsc::TrySendError::Disconnected(_)) => return Err("API event receiver closed".into()),
            Err(mpsc::TrySendError::Full(returned)) => event = returned,
        }
        if std::time::Instant::now() >= deadline {
            return Err("API interface event budget exhausted".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
impl CodexConnection for Connection {
    fn configure_api(&mut self, key: Option<String>, model: String) -> Result<(), String> {
        if self.worker.as_ref().is_some_and(|worker| !worker.is_finished()) {
            return Err("Disconnect and wait for the API worker to finish before changing its key".into());
        }
        let key = key
            .or_else(|| self.configuration.as_ref().map(|config| config.key().to_owned()))
            .or_else(|| self.environment_enabled.then(|| std::env::var("OPENAI_API_KEY").ok()).flatten())
            .ok_or("Enter an API key before connecting")?;
        let configuration = api_settings::Configuration::new(key, model)?;
        protocol::request(configuration.model(), "", vec![], &[])?;
        self.configuration = Some(configuration);
        self.environment_enabled = false;
        self.models.clear();
        Ok(())
    }
    fn api_configuration(&self) -> Option<api_settings::Status> {
        if let Some(config) = &self.configuration {
            return Some(api_settings::Status { configured: true, model: Some(config.model().to_owned()) });
        }
        let model = self
            .environment_enabled
            .then(|| std::env::var("FILMCRAFT_OPENAI_MODEL").ok())
            .flatten()
            .filter(|model| api_settings::validate_model(model).is_ok());
        Some(api_settings::Status { configured: self.environment_enabled && std::env::var_os("OPENAI_API_KEY").is_some(), model })
    }
    fn forget_api_configuration(&mut self) -> Result<(), String> {
        if self.worker.as_ref().is_some_and(|worker| !worker.is_finished()) {
            return Err("API worker is still closing; retry clearing the key shortly".into());
        }
        self.configuration = None;
        self.environment_enabled = false;
        self.models.clear();
        Ok(())
    }

    fn provider_id(&self) -> ProviderId {
        ProviderId::OpenAiApi
    }
    fn provider_capabilities(&self) -> serde_json::Value {
        serde_json::json!({"embeddedComposer":true,"nativeTools":true,"images":"modelDependent","nativeImageResults":"modelDependent","audioAnalysis":false,"voice":false,"resume":self.storage.is_some(),"apiAuthentication":"checked on first submitted turn","qualified":false})
    }
    fn bind_provider_permit(&mut self, permit: String) -> Result<(), String> {
        if permit.len() != 64 || !permit.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Invalid API provider ownership".into());
        }
        self.permit = Some(permit);
        Ok(())
    }
    fn validate_session_request(&self, request: &SessionRequest) -> Result<(), String> {
        request.validate()?;
        if request.resume.as_deref().is_some_and(|id| !state::valid_id(id)) || (request.resume.is_some() && self.storage.is_none()) {
            return Err("Choose a saved API session with local storage; another provider's conversation cannot be resumed here".into());
        }
        Ok(())
    }
    fn connect(&mut self) -> Result<(), String> {
        self.start(None)
    }
    fn connect_session(&mut self, request: SessionRequest) -> Result<(), String> {
        self.start(Some(request))
    }
    fn poll(&mut self) -> Option<ConnectionStatus> {
        self.statuses.as_ref()?.try_recv().ok()
    }
    fn poll_owned_status(&mut self) -> Option<(String, ConnectionStatus)> {
        let status = self.poll()?;
        Some((self.event_permit.clone()?, status))
    }
    fn disconnect(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.commands = None;
        self.statuses = None;
        self.events = None;
        self.event_permit = None;
        self.busy = false;
    }
    fn poll_chat(&mut self) -> Option<ChatEvent> {
        let event = self.events.as_ref()?.try_recv().ok()?;
        if matches!(event, ChatEvent::Finished | ChatEvent::Error(_)) {
            self.busy = false;
        }
        Some(event)
    }
    fn poll_owned_chat(&mut self) -> Option<(String, ChatEvent)> {
        let event = self.poll_chat()?;
        Some((self.event_permit.clone()?, event))
    }
    fn send_prompt(&mut self, prompt: String) -> Result<(), String> {
        self.send_prompt_with_images(prompt, None, vec![])
    }
    fn send_prompt_with_model(&mut self, prompt: String, model: Option<String>) -> Result<(), String> {
        self.send_prompt_with_images(prompt, model, vec![])
    }
    fn send_prompt_with_images(&mut self, prompt: String, model: Option<String>, images: Vec<String>) -> Result<(), String> {
        if self.busy {
            return Err("Wait for the active API turn or cancel it before sending again".into());
        }
        if model.as_ref().is_some_and(|choice| !self.models.contains(choice)) {
            return Err("Choose one of this connection's configured API models".into());
        }
        protocol::user_input(&prompt, &images)?;
        self.commands
            .as_ref()
            .ok_or("Connect the API first")?
            .try_send(Command::Prompt(prompt, model, images))
            .map_err(|_| "API worker is unavailable or its queue is full")?;
        self.busy = true;
        Ok(())
    }
    fn cancel(&mut self) -> Result<(), String> {
        self.disconnect();
        Ok(())
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.disconnect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_api_settings_validate_replace_and_forget_without_network() {
        let mut connection = Connection::new(None, egui::Context::default());
        connection.environment_enabled = false;
        assert!(!connection.api_configuration().unwrap().configured);
        let secret = "sk-synthetic-private-api-test-value";
        connection.configure_api(Some(secret.into()), "gpt-6-luna".into()).unwrap();
        assert_eq!(connection.api_configuration().unwrap().model.as_deref(), Some("gpt-6-luna"));
        for (key, model) in [(Some("bad\nkey".into()), "model"), (None, "invalid model")] {
            assert!(connection.configure_api(key, model.into()).is_err());
            assert_eq!(connection.configuration.as_ref().unwrap().key(), secret);
            assert_eq!(connection.configuration.as_ref().unwrap().model(), "gpt-6-luna");
        }
        connection.configure_api(None, "model-b".into()).unwrap();
        assert_eq!(connection.configuration.as_ref().unwrap().key(), secret);
        connection.forget_api_configuration().unwrap();
        assert!(!connection.api_configuration().unwrap().configured);
        assert!(connection.configuration.is_none());
        assert!(connection.configure_api(None, "model-b".into()).is_err());
        assert!(connection.worker.is_none());
    }

    #[test]
    fn private_api_key_cannot_be_replaced_or_cleared_while_worker_is_alive() {
        let mut connection = Connection::new(None, egui::Context::default());
        connection.environment_enabled = false;
        let secret = "sk-synthetic-private-api-test-value";
        connection.configure_api(Some(secret.into()), "model-a".into()).unwrap();
        let (finish, wait) = mpsc::channel::<()>();
        connection.worker = Some(std::thread::spawn(move || {
            let _ = wait.recv();
        }));
        assert!(connection.configure_api(Some("sk-replacement-private-test-value".into()), "model-b".into()).is_err());
        assert!(connection.forget_api_configuration().is_err());
        assert_eq!(connection.configuration.as_ref().unwrap().key(), secret);
        finish.send(()).unwrap();
        connection.worker.take().unwrap().join().unwrap();
        connection.forget_api_configuration().unwrap();
        assert!(!connection.api_configuration().unwrap().configured);
    }

    #[test]
    fn configured_transport_finishes_preparation_and_accepts_the_first_prompt() {
        let ctx = egui::Context::default();
        let (statuses, status_rx) = mpsc::sync_channel(4);
        let (events, event_rx) = mpsc::sync_channel(8);
        let (commands, command_rx) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        announce_ready(&statuses, &events, "fake-model", &["fake-model".into()], &ctx, &stop).unwrap();
        let mut connection = Connection::new(None, ctx);
        connection.statuses = Some(status_rx);
        connection.events = Some(event_rx);
        connection.commands = Some(commands);
        connection.stop = stop;
        connection.busy = true;
        assert_eq!(connection.poll(), Some(ConnectionStatus::Ready));
        assert!(matches!(connection.poll_chat(), Some(ChatEvent::Models(_, _))));
        assert!(connection.send_prompt("Inspect current project".into()).is_err());
        assert!(matches!(connection.poll_chat(), Some(ChatEvent::Finished)));
        assert!(connection.poll_chat().is_none(), "Setup must not claim project inspection or API session resume");
        connection.send_prompt("Inspect current project".into()).unwrap();
        assert!(matches!(command_rx.try_recv(),Ok(Command::Prompt(text,_,_)) if text=="Inspect current project"));
    }
    #[test]
    fn explicit_api_model_catalog_is_bounded_and_rejects_unconfigured_sends() {
        assert_eq!(configured_models("model-a", Some("model-a, model-b")).unwrap(), vec!["model-a", "model-b"]);
        for invalid in ["model-a,model-a", "model-b", "model-a,", "model-a,../bad"] {
            assert!(configured_models("model-a", Some(invalid)).is_err());
        }
        assert!(configured_models("model-a", Some(&"x".repeat(9000))).is_err());
        let mut connection = Connection::new(None, egui::Context::default());
        connection.models = vec!["model-a".into()];
        assert!(connection.send_prompt_with_model("Hello".into(), Some("model-b".into())).unwrap_err().contains("configured API models"));
        let request = SessionRequest { project: "project".into(), resume: Some("codex-thread".into()), ..Default::default() };
        assert!(connection.validate_session_request(&request).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn disconnect_unblocks_owned_helper_worker_and_reconnect_passes_the_closing_gate() {
        use super::turn::Tools;
        let stopped = Arc::new(AtomicBool::new(false));
        let mut helper = mcp::owned_helper_fixture(stopped.clone(), false);
        let (started, rx) = mpsc::sync_channel(1);
        let (outcome, errors) = mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            started.send(()).unwrap();
            let error = helper
                .call_once(&protocol::Call {
                    id: "owned_fake".into(),
                    name: "command_run".into(),
                    arguments: serde_json::json!({"padding":"x".repeat(120*1024)}),
                })
                .unwrap_err();
            drop(helper);
            outcome.send(error).unwrap();
        });
        let mut connection = Connection::new(None, egui::Context::default());
        connection.worker = Some(worker);
        connection.stop = stopped;
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        let start = std::time::Instant::now();
        connection.disconnect();
        assert!(start.elapsed() < Duration::from_millis(100));
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while connection.worker.as_ref().is_some_and(|worker| !worker.is_finished()) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let finished = connection.worker.as_ref().is_some_and(|worker| worker.is_finished());
        if !finished && let Some(worker) = connection.worker.take() {
            worker.join().unwrap();
        }
        assert!(finished, "Disconnect must not strand the helper worker");
        assert!(errors.recv_timeout(Duration::from_secs(1)).unwrap().contains("write cancelled"));
        // No bridge is configured, so this fails before any environment key read or API call.
        let error = connection.start(None).unwrap_err();
        assert!(!error.contains("Previous API worker"), "{error}");
        assert!(connection.worker.is_none());
    }
}
