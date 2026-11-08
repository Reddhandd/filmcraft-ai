//! Local Codex App Server chat; FilmCraft owns native command execution.
//! The credential travels only through the inherited environment and authenticated loopback.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

use filmcraft_ui_egui::console::{CodexConnection, ConnectionStatus};
use serde_json::{Value, json};

mod chat;
mod diagnostic;
#[cfg(test)]
mod persistence_live;
mod voice;

// Four supported 2 MiB PNG attachments can appear in an echoed input notification as
// base64. Bound framing separately from the much smaller paginated tool-reply budget.
const MAX_MESSAGE: usize = 16 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);
const SERVER: &str = "filmcraft_console";
type Messages = mpsc::Receiver<Result<Value, String>>;

#[derive(Default)]
struct Process {
    stopped: AtomicBool,
    microphone_gate: Arc<voice::MicrophoneGate>,
    child: Mutex<Option<Child>>,
}
impl Process {
    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Some(child) = self.child.lock().unwrap_or_else(PoisonError::into_inner).as_mut() {
            let _ = child.kill();
        }
    }
    fn reap(&self) {
        self.stop();
        // Never hold the shared lock while waiting for OS process cleanup: Disconnect runs
        // on the UI thread and must remain responsive even if a child is slow to exit.
        let child = self.child.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(mut child) = child {
            match child.wait() {
                Ok(status) if status.success() => log::info!(target:"filmcraft::console", "Codex child reaped: {status}"),
                Ok(status) => log::warn!(target:"filmcraft::console", "Codex child stopped (including deliberate cleanup): {status}"),
                Err(error) => log::warn!(target:"filmcraft::console", "Could not reap Codex child: {error}"),
            }
        }
    }
}

pub struct Connection {
    port: Option<u16>,
    control_token: Option<String>,
    process: Option<Arc<Process>>,
    updates: Option<mpsc::Receiver<ConnectionStatus>>,
    ctx: egui::Context,
    worker: Option<std::thread::JoinHandle<()>>,
    commands: Option<mpsc::SyncSender<chat::CommandMessage>>,
    events: Option<mpsc::Receiver<filmcraft_ui_egui::console::ChatEvent>>,
    session_request: Option<filmcraft_ui_egui::console::sessions::SessionRequest>,
    pending_connect: bool,
    provider_permit: Option<String>,
    event_permit: Option<String>,
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
            process: None,
            updates: None,
            ctx,
            worker: None,
            commands: None,
            events: None,
            session_request: None,
            pending_connect: false,
            provider_permit: None,
            event_permit: None,
        }
    }
}
impl CodexConnection for Connection {
    fn bind_provider_permit(&mut self, permit: String) -> Result<(), String> {
        if permit.len() != 64 || !permit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("Invalid AI ownership permit".into());
        }
        self.provider_permit = Some(permit);
        Ok(())
    }
    fn poll_owned_status(&mut self) -> Option<(String, ConnectionStatus)> {
        let status = self.poll()?;
        Some((self.event_permit.clone()?, status))
    }
    fn poll_owned_chat(&mut self) -> Option<(String, filmcraft_ui_egui::console::ChatEvent)> {
        let event = self.poll_chat()?;
        Some((self.event_permit.clone()?, event))
    }

    fn connect(&mut self) -> Result<(), String> {
        if self.worker.as_ref().is_some_and(|worker| !worker.is_finished()) {
            if self.process.is_some() {
                return Err("Disconnect the active connection before reconnecting".into());
            }
            self.pending_connect = true;
            self.ctx.request_repaint_after(Duration::from_millis(50));
            return Ok(());
        }
        self.pending_connect = false;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if self.process.is_some() {
            return Err("Disconnect the previous Codex connection before reconnecting".into());
        }
        let port = self
            .port
            .filter(|p| *p != 0)
            .ok_or("Restart this development build with --control PORT and FILMCRAFT_CONTROL_TOKEN (see docs/control-protocol.md)")?;
        let token = self
            .control_token
            .clone()
            .or_else(|| std::env::var("FILMCRAFT_CONTROL_TOKEN").ok())
            .ok_or("Private Console connection is unavailable; restart FilmCraft")?;
        if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("FILMCRAFT_CONTROL_TOKEN must contain 64 random hex characters".into());
        }
        let cli = std::env::current_exe().map_err(|e| format!("Cannot locate FilmCraft: {e}"))?.with_file_name(if cfg!(windows) {
            "filmcraft-cli.exe"
        } else {
            "filmcraft-cli"
        });
        if !cli.is_file() {
            return Err("Build filmcraft-cli beside this development FilmCraft executable before connecting".into());
        }
        let mut server_command = app_server(&token);
        if let Some(permit) = self.provider_permit.as_ref() {
            server_command.env("FILMCRAFT_PROVIDER_PERMIT", permit);
        }
        let process = Arc::new(Process::default());
        let worker = Arc::clone(&process);
        let ctx = self.ctx.clone();
        let (tx, rx) = mpsc::sync_channel(2);
        let (commands, commands_rx) = mpsc::sync_channel(8);
        let (events_tx, events) = mpsc::sync_channel(64);
        let wire =
            chat::Wire { process: Arc::clone(&process), commands: commands_rx, events: events_tx, ctx: ctx.clone(), session: self.session_request.clone() };
        let handle = std::thread::Builder::new()
            .name("console-codex".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    probe(
                        &worker,
                        port,
                        &token,
                        server_command,
                        server_config(&cli, port),
                        || {
                            let _ = tx.try_send(ConnectionStatus::Ready);
                            ctx.request_repaint();
                        },
                        Some(wire),
                    )
                }));
                let failure = match result {
                    Ok(Ok(())) => None,
                    Ok(Err(message)) => Some(message),
                    Err(_) => Some("Codex connection worker failed; disconnect and retry".into()),
                };
                if !worker.stopped.load(Ordering::SeqCst) {
                    if let Some(message) = failure {
                        log::warn!(target:"filmcraft::console", "Console connection failed: {}", diagnostic::safe_text(&message));
                        let _ = tx.try_send(ConnectionStatus::Failed(message));
                    }
                    ctx.request_repaint();
                }
                worker.reap();
            })
            .map_err(|e| format!("Cannot start Codex connection worker: {e}"))?;
        self.event_permit = self.provider_permit.clone();
        self.commands = Some(commands);
        self.events = Some(events);
        self.worker = Some(handle);
        self.process = Some(process);
        self.updates = Some(rx);
        Ok(())
    }
    fn connect_session(&mut self, request: filmcraft_ui_egui::console::sessions::SessionRequest) -> Result<(), String> {
        request.validate()?;
        self.session_request = Some(request);
        self.connect()
    }
    fn switch_session(&mut self, request: filmcraft_ui_egui::console::sessions::SessionRequest) -> Result<(), String> {
        request.validate()?;
        self.submit(chat::CommandMessage::Session(request))
    }
    fn poll(&mut self) -> Option<ConnectionStatus> {
        if self.pending_connect {
            if self.worker.as_ref().is_some_and(|worker| !worker.is_finished()) {
                self.ctx.request_repaint_after(Duration::from_millis(50));
                return None;
            }
            return Some(match self.connect() {
                Ok(()) => ConnectionStatus::Connecting,
                Err(e) => ConnectionStatus::Failed(e),
            });
        }
        self.updates.as_ref().and_then(|rx| rx.try_recv().ok())
    }
    fn disconnect(&mut self) {
        self.pending_connect = false;
        if let Some(process) = self.process.take() {
            process.stop();
        }
        self.updates = None;
        self.commands = None;
        self.events = None;
        self.event_permit = None;
        self.session_request = None;
    }
    fn poll_chat(&mut self) -> Option<filmcraft_ui_egui::console::ChatEvent> {
        self.events.as_ref().and_then(|rx| rx.try_recv().ok())
    }
    fn send_prompt(&mut self, prompt: String) -> Result<(), String> {
        if prompt.trim().is_empty() || prompt.len() > 16 * 1024 {
            return Err("Message must contain 1–16384 bytes".into());
        }
        self.submit(chat::CommandMessage::Prompt(prompt, None))
    }
    fn send_prompt_with_model(&mut self, prompt: String, model: Option<String>) -> Result<(), String> {
        if prompt.trim().is_empty() || prompt.len() > 16 * 1024 {
            return Err("Message must contain 1–16384 bytes".into());
        }
        self.submit(chat::CommandMessage::Prompt(prompt, model))
    }
    fn send_prompt_with_images(&mut self, prompt: String, model: Option<String>, images: Vec<String>) -> Result<(), String> {
        if prompt.is_empty()
            || prompt.len() > 128 * 1024
            || images.len() > 4
            || images.iter().any(|i| i.len() > 3 * 1024 * 1024 || !i.starts_with("data:image/png;base64,"))
        {
            return Err("Invalid or oversized reference input".into());
        }
        self.submit(chat::CommandMessage::Images(prompt, model, images))
    }
    fn start_voice(&mut self, model: Option<String>) -> Result<(), String> {
        self.submit(chat::CommandMessage::VoiceStart(model, 0, None))
    }
    fn start_voice_channel(&mut self, model: Option<String>, channel: u16) -> Result<(), String> {
        if channel > 255 {
            return Err("Microphone channel is out of range".into());
        }
        self.submit(chat::CommandMessage::VoiceStart(model, channel, None))
    }
    fn start_voice_selected(&mut self, model: Option<String>, channel: u16, voice: Option<String>) -> Result<(), String> {
        if channel > 255 || voice.as_deref().is_some_and(|id| !voice::selection::valid_id(id)) {
            return Err("Invalid microphone channel or speaking voice".into());
        }
        self.submit(chat::CommandMessage::VoiceStart(model, channel, voice))
    }
    fn stop_voice(&mut self) -> Result<(), String> {
        self.submit(chat::CommandMessage::VoiceStop)
    }
    fn set_voice_echo_reduction(&mut self, enabled: bool) -> Result<(), String> {
        let process = self.process.as_ref().filter(|p| !p.stopped.load(Ordering::SeqCst)).ok_or("Codex voice is disconnected")?;
        process.microphone_gate.set_echo_reduction(enabled);
        Ok(())
    }
    fn set_voice_muted(&mut self, muted: bool) -> Result<(), String> {
        let process = self.process.as_ref().filter(|p| !p.stopped.load(Ordering::SeqCst)).ok_or("Codex voice is disconnected")?;
        // Privacy control must not wait behind a protocol request or a full command queue.
        process.microphone_gate.set(muted)
    }
    fn cancel(&mut self) -> Result<(), String> {
        self.submit(chat::CommandMessage::Cancel)
    }
    fn answer(&mut self, id: Value, result: Result<Value, String>) -> Result<(), String> {
        self.submit(chat::CommandMessage::Answer(id, result))
    }
}
impl Connection {
    fn submit(&self, command: chat::CommandMessage) -> Result<(), String> {
        self.commands.as_ref().ok_or("Codex is disconnected")?.try_send(command).map_err(|_| "Codex is busy or disconnected; retry or reconnect".into())
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.disconnect();
    }
}

fn read_message(reader: &mut impl BufRead) -> Result<Value, String> {
    let mut line = Vec::new();
    reader.take((MAX_MESSAGE + 1) as u64).read_until(b'\n', &mut line).map_err(|_| "Cannot read Codex protocol message")?;
    if line.is_empty() {
        return Err("Codex App Server closed its output".into());
    }
    if line.len() > MAX_MESSAGE || line.last() != Some(&b'\n') {
        log::warn!(target:"filmcraft::console", "Protocol frame rejected: observed {} bytes, limit {MAX_MESSAGE}, newline={}",line.len(),line.last()==Some(&b'\n'));
        return Err("Codex protocol message is oversized or incomplete".into());
    }
    let value: Value = serde_json::from_slice(&line).map_err(|_| "Codex returned malformed JSON")?;
    if !value.is_object() {
        return Err("Codex protocol message must be an object".into());
    }
    Ok(value)
}
fn send(writer: &mut impl Write, value: &Value) -> Result<(), String> {
    serde_json::to_writer(&mut *writer, value).map_err(|_| "Cannot write Codex protocol message")?;
    writer.write_all(b"\n").and_then(|()| writer.flush()).map_err(|_| "Codex App Server stopped accepting messages".into())
}
fn response(writer: &mut impl Write, messages: &Messages, id: u64) -> Result<Value, String> {
    response_with_catalog(writer, messages, id, &mut None)
}
fn bounded_voice_catalog_reply(message: &Value) -> Value {
    let result = message.get("result").ok_or("Voice catalog request rejected").map_err(str::to_owned).and_then(voice::selection::catalog);
    match result {
        Ok(voices) => json!({"id":9,"result":{"voices":{"v3":voices}}}),
        Err(_) => json!({"id":9,"error":{"message":"Speaking-voice catalog unavailable"}}),
    }
}
fn response_with_catalog(writer: &mut impl Write, messages: &Messages, id: u64, catalog: &mut Option<Value>) -> Result<Value, String> {
    let deadline = Instant::now() + TIMEOUT;
    // Bounded notification traffic cannot keep a broken handshake alive forever.
    for _ in 0..256 {
        let message = messages
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| "Codex connection timed out or closed; check Codex CLI installation and bridge configuration")??;
        if message.get("method").is_some() {
            if let Some(id) = message.get("id") {
                // No approvals, elicitation or tool execution in the connection-check stage.
                send(writer, &json!({"id": id, "error": {"code": -32601, "message": "FilmCraft connection check does not accept server requests"}}))?;
            }
            continue;
        }
        // ID9 is reserved for the one optional catalog query. Resume can begin before its
        // reply arrives; retain that reply for the event loop instead of rejecting or losing it.
        if id != 9 && message.get("id").and_then(Value::as_u64) == Some(9) {
            if catalog.is_none() {
                *catalog = Some(bounded_voice_catalog_reply(&message));
            }
            continue;
        }
        if message.get("id").and_then(Value::as_u64) != Some(id) {
            return Err("Codex returned an unexpected response ID".into());
        }
        if message.get("error").is_some() {
            // Do not echo arbitrary server output: it can contain credentials and private paths.
            return Err(format!("Codex rejected connection request {id}; update Codex CLI and check its configuration"));
        }
        return message.get("result").filter(|v| v.is_object()).cloned().ok_or("Codex response is missing an object result".into());
    }
    Err("Codex sent too many notifications during connection setup".into())
}
fn setup_call(writer: &mut impl Write, messages: &Messages, id: u64, method: diagnostic::SetupMethod, params: Value) -> Result<Value, String> {
    setup_call_with_catalog(writer, messages, id, method, params, &mut None)
}
fn setup_call_with_catalog(
    writer: &mut impl Write,
    messages: &Messages,
    id: u64,
    method: diagnostic::SetupMethod,
    params: Value,
    catalog: &mut Option<Value>,
) -> Result<Value, String> {
    use diagnostic::{SetupOutcome, setup_line};
    let started = Instant::now();
    log::info!(target:"filmcraft::console","{}",setup_line(method,id,SetupOutcome::Pending,Duration::ZERO));
    let result = send(writer, &json!({"id":id,"method":method.name(),"params":params})).and_then(|()| response_with_catalog(writer, messages, id, catalog));
    let outcome = if result.is_ok() { SetupOutcome::ReturnedOk } else { SetupOutcome::ReturnedError };
    let line = setup_line(method, id, outcome, started.elapsed());
    if result.is_ok() {
        log::info!(target:"filmcraft::console","{line}");
    } else {
        log::warn!(target:"filmcraft::console","{line}");
    }
    result
}
#[cfg(test)]
fn handshake(writer: &mut impl Write, messages: &Messages, server: Value) -> Result<(String, Option<String>, Value), String> {
    handshake_session(writer, messages, server, None)
}
fn handshake_session(
    writer: &mut impl Write,
    messages: &Messages,
    server: Value,
    request: Option<&filmcraft_ui_egui::console::sessions::SessionRequest>,
) -> Result<(String, Option<String>, Value), String> {
    setup_call(
        writer,
        messages,
        1,
        diagnostic::SetupMethod::Initialize,
        json!({"capabilities":{"experimentalApi":true},"clientInfo": {"name": "filmcraft_console", "title": "FilmCraft Console", "version": env!("CARGO_PKG_VERSION")}}),
    )?;
    send(writer, &json!({"method": "initialized", "params": {}}))?;
    let config = setup_call(writer, messages, 4, diagnostic::SetupMethod::Config, json!({"includeLayers":false}))?;
    let mut servers = serde_json::Map::new();
    if let Some(configured) = config.pointer("/config/mcp_servers").and_then(Value::as_object) {
        if configured.len() > 256 {
            return Err("Too many configured MCP servers".into());
        }
        for name in configured.keys() {
            servers.insert(name.clone(), json!({"enabled":false}));
        }
    }
    // Only names come from display config. Build the complete owned transport ourselves;
    // display values contain null options that cannot be round-tripped through TOML.
    servers.insert(SERVER.into(), server);
    let mut plugins = serde_json::Map::new();
    if let Some(configured) = config.pointer("/config/plugins").and_then(Value::as_object) {
        if configured.len() > 256 {
            return Err("Too many configured Codex plugins".into());
        }
        for name in configured.keys() {
            plugins.insert(name.clone(), json!({"enabled":false}));
        }
    }
    let overrides = json!({"mcp_servers":servers,"plugins":plugins,"web_search":"live"});

    let (thread, model) = select_session(writer, messages, &overrides, request)?;
    let default_model = config.pointer("/config/model").and_then(Value::as_str).filter(|s| !s.is_empty() && s.len() <= 256).map(str::to_owned).or(model);
    Ok((thread, default_model, overrides))
}
fn select_session(
    writer: &mut impl Write,
    messages: &Messages,
    overrides: &Value,
    request: Option<&filmcraft_ui_egui::console::sessions::SessionRequest>,
) -> Result<(String, Option<String>), String> {
    select_session_with_catalog(writer, messages, overrides, request, &mut None)
}
fn select_session_with_catalog(
    writer: &mut impl Write,
    messages: &Messages,
    overrides: &Value,
    request: Option<&filmcraft_ui_egui::console::sessions::SessionRequest>,
    voice_catalog: &mut Option<Value>,
) -> Result<(String, Option<String>), String> {
    if let Some(request) = request {
        request.validate()?;
    }
    let instructions = chat::instructions();
    let mut params = json!({"ephemeral":request.is_none(),"sandbox":"read-only","approvalPolicy":"never","config":overrides,"developerInstructions":instructions,"dynamicTools":[chat::tool()]});
    if overrides
        .pointer("/mcp_servers/filmcraft_console/args")
        .and_then(Value::as_array)
        .is_some_and(|a| a.iter().any(|v| v.as_str() == Some("--controller-console")))
        && let Some(object) = params.as_object_mut()
    {
        object.remove("dynamicTools");
    }
    if let Some(model) = request.and_then(|r| r.model.as_ref()) {
        params["model"] = json!(model);
    }
    let method = if let Some(id) = request.and_then(|r| r.resume.as_ref()) {
        let object = params.as_object_mut().ok_or("Invalid session parameters")?;
        object.remove("ephemeral");
        object.remove("dynamicTools");
        object.insert("threadId".into(), json!(id));
        object.insert("excludeTurns".into(), json!(true));
        diagnostic::SetupMethod::Resume
    } else {
        diagnostic::SetupMethod::Start
    };
    let result = setup_call_with_catalog(writer,messages,2,method,params,voice_catalog).map_err(|error| {
        if request.and_then(|r| r.resume.as_ref()).is_some() {
            format!("Could not resume the saved Codex conversation. It remains in Resume Codex Session; retry Connect or explicitly choose New Conversation. No previous edits were replayed. {error}")
        } else {
            error
        }
    })?;
    let model = result.get("model").and_then(Value::as_str).filter(|s| !s.is_empty() && s.len() <= 256).map(str::to_owned);
    let thread = result
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 256)
        .ok_or("Codex did not return a valid conversation ID")?
        .to_owned();
    if request.and_then(|r| r.resume.as_deref()).is_some_and(|expected| expected != thread) {
        return Err("Codex resumed a different conversation; nothing was restored".into());
    }
    let catalog = setup_call_with_catalog(
        writer,
        messages,
        3,
        diagnostic::SetupMethod::Catalog,
        json!({"threadId":thread,"detail":"toolsAndAuthOnly","limit":100}),
        voice_catalog,
    )?;
    let verified = verify_catalog(&catalog);
    log::info!(target:"filmcraft::console","Codex setup bridge_catalog_verified={}",verified.is_ok());
    verified?;
    Ok((thread, model))
}
fn verify_catalog(result: &Value) -> Result<(), String> {
    let data = result.get("data").and_then(Value::as_array).ok_or("Codex did not return an MCP catalog")?;
    if data.iter().any(|v| v.get("name").and_then(Value::as_str) != Some(SERVER) && v.get("runtimeStatus").and_then(Value::as_str) != Some("disabled")) {
        return Err("Other MCP servers are enabled; the console requires an isolated FilmCraft tool set".into());
    }
    let server = data.iter().take(64).find(|v| v.get("name").and_then(Value::as_str) == Some(SERVER)).ok_or("Codex cannot find the FilmCraft bridge")?;
    let tools = server.get("tools").and_then(Value::as_object).ok_or("FilmCraft bridge has no tool catalog")?;
    if server.get("runtimeStatus").and_then(Value::as_str) != Some("connected")
        || !tools.contains_key("project_inspect")
        || !tools.contains_key("sequence_inspect")
        || filmcraft_ui_egui::console::capabilities::CONSOLE_TOOLS.iter().any(|name| !tools.contains_key(*name))
        || tools.keys().any(|k| !filmcraft_ui_egui::console::capabilities::CONSOLE_TOOLS.contains(&k.as_str()) && k != "request_plan")
    {
        return Err("FilmCraft inspection bridge is unavailable or has unexpected editing tools".into());
    }
    Ok(())
}
fn bridge_probe(port: u16, token: &str) -> Result<(), String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|_| "FilmCraft control server is unavailable; check --control PORT")?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .and_then(|()| stream.set_write_timeout(Some(Duration::from_secs(5))))
        .map_err(|_| "Cannot set FilmCraft connection deadlines")?;
    send(&mut stream, &json!({"id": 1, "method": "engine.execute", "params": {"command": "project.inspect", "params": {}}, "token": token}))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut line = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("FilmCraft inspection probe timed out".into());
        }
        stream.set_read_timeout(Some(remaining)).map_err(|_| "Cannot set FilmCraft read deadline")?;
        let mut bytes = [0; 4096];
        let count = stream.read(&mut bytes).map_err(|_| "FilmCraft inspection probe timed out or closed")?;
        if count == 0 {
            return Err("FilmCraft closed the inspection probe".into());
        }
        let received = bytes.get(..count).ok_or("Invalid FilmCraft read length")?;
        if line.len().saturating_add(received.len()) > MAX_MESSAGE {
            return Err("FilmCraft inspection reply is too large".into());
        }
        line.extend_from_slice(received);
        if line.contains(&b'\n') {
            break;
        }
    }
    let result = read_message(&mut std::io::Cursor::new(line))?;
    if result.get("ok").and_then(Value::as_bool) != Some(true)
        || result.get("id").and_then(Value::as_u64) != Some(1)
        || !result.get("result").is_some_and(Value::is_object)
    {
        return Err("FilmCraft rejected the inspection probe; check its control credential and active session".into());
    }
    Ok(())
}
fn server_config(cli: &std::path::Path, port: u16) -> Value {
    json!({"command":cli.to_string_lossy(),"args":["mcp","--controller-console","--bridge",format!("127.0.0.1:{port}")],"env_vars":["FILMCRAFT_CONTROL_TOKEN","FILMCRAFT_PROVIDER_PERMIT"],"enabled_tools":filmcraft_ui_egui::console::capabilities::CONSOLE_TOOLS,"tools":{"command_run":{"approval_mode":"approve"},"command_batch":{"approval_mode":"approve"},"console_references":{"approval_mode":"approve"}},"tool_timeout_sec":600,"required":true,"enabled":true})
}
fn app_server(token: &str) -> Command {
    let mut command = Command::new("codex");
    // Finder omits Homebrew from PATH. Let exec retain its normal permission/search semantics;
    // do not resolve the first existing file ourselves (it may not be executable).
    #[cfg(target_os = "macos")]
    command.env("PATH", mac_codex_path(std::env::var_os("PATH")));
    command
        .args([
            "app-server",
            "--listen",
            "stdio://",
            "-c",
            "mcp_servers={}",
            "-c",
            "features.shell_tool=false",
            "-c",
            "features.hooks=false",
            "-c",
            "features.apps=false",
            "-c",
            "plugins={}",
            "-c",
            "features.multi_agent=false",
            "-c",
            "web_search=\"live\"",
            "-c",
            "tools.view_image=false",
            "-c",
            "project_doc_max_bytes=0",
        ])
        .current_dir(std::env::temp_dir())
        // Preserve the old desktop launcher's isolation: the Codex selection must not
        // silently inherit the separate API provider's key or another bridge's route.
        .env_remove("OPENAI_API_KEY")
        .env_remove("FILMCRAFT_PROVIDER_PERMIT")
        .env_remove("FILMCRAFT_EXTERNAL_PROVIDER")
        .env("FILMCRAFT_CONTROL_TOKEN", token)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
}
#[cfg(any(target_os = "macos", test))]
fn mac_codex_path(path: Option<std::ffi::OsString>) -> std::ffi::OsString {
    let mut path = path.unwrap_or_else(|| std::ffi::OsString::from("/usr/bin:/bin:/usr/sbin:/sbin"));
    path.push(":/opt/homebrew/bin:/usr/local/bin");
    path
}

fn probe(
    process: &Arc<Process>,
    port: u16,
    token: &str,
    mut command: Command,
    server: Value,
    ready: impl FnOnce(),
    wire: Option<chat::Wire>,
) -> Result<(), String> {
    bridge_probe(port, token)?;
    if process.stopped.load(Ordering::SeqCst) {
        return Err("Connection cancelled".into());
    }
    let mut child =
        command.spawn().map_err(|error| format!("Cannot start Codex App Server ({:?}); install Codex CLI and make codex available on PATH", error.kind()))?;
    let writer = child.stdin.take();
    let reader = child.stdout.take();
    *process.child.lock().unwrap_or_else(PoisonError::into_inner) = Some(child);
    if process.stopped.load(Ordering::SeqCst) {
        return Err("Connection cancelled".into());
    }
    let mut writer = writer.ok_or("Codex stdin is unavailable")?;
    let reader = reader.ok_or("Codex stdout is unavailable")?;
    // Backpressure keeps only two parsed frames queued, including large valid image inputs.
    let (tx, rx) = mpsc::sync_channel(2);
    std::thread::Builder::new()
        .name("console-codex-read".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut reader = BufReader::new(reader);
                loop {
                    let message = read_message(&mut reader);
                    let failed = message.is_err();
                    if tx.send(message).is_err() || failed {
                        break;
                    }
                }
            }));
            if result.is_err() {
                let _ = tx.try_send(Err("Codex protocol reader failed".into()));
            }
        })
        .map_err(|e| format!("Cannot start Codex protocol reader: {e}"))?;
    let (thread, default_model, settings) = handshake_session(&mut writer, &rx, server, wire.as_ref().and_then(|wire| wire.session.as_ref()))?;
    ready();
    if let Some(wire) = wire {
        return chat::run(process, &mut writer, &rx, &thread, default_model, settings, wire);
    }
    // Keep stdin and output drain alive after setup. The service's stop kills the child.
    while !process.stopped.load(Ordering::SeqCst) {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Ok(message)) => {
                if message.get("method").is_some() && message.get("id").is_some() {
                    send(&mut writer, &json!({"id": message.get("id"), "error": {"code": -32601, "message": "Connection check only"}}))?;
                }
            }
            Ok(Err(error)) => return Err(error),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err("Codex protocol reader closed".into()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finder_cli_path_keeps_custom_precedence_and_os_executable_search() {
        assert_eq!(mac_codex_path(Some("/custom/bin:/usr/bin".into())), "/custom/bin:/usr/bin:/opt/homebrew/bin:/usr/local/bin");
        assert_eq!(mac_codex_path(None), "/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/usr/local/bin");
        // An explicitly empty PATH retains its leading current-directory entry.
        assert_eq!(mac_codex_path(Some("".into())), ":/opt/homebrew/bin:/usr/local/bin");
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let original = std::ffi::OsString::from_vec(b"/custom-\xff/bin".to_vec());
            assert!(mac_codex_path(Some(original.clone())).into_vec().starts_with(&original.into_vec()));
        }
    }
    use std::io::Cursor;

    #[test]
    fn launch_enables_live_web_search_without_enabling_local_shell_or_other_integrations() {
        let command = app_server("synthetic-test-token");
        let args: Vec<_> = command.get_args().map(|arg| arg.to_string_lossy().into_owned()).collect();
        for setting in [
            "web_search=\"live\"",
            "features.shell_tool=false",
            "features.hooks=false",
            "features.apps=false",
            "features.multi_agent=false",
            "tools.view_image=false",
        ] {
            assert!(args.windows(2).any(|pair| pair == ["-c", setting]), "Missing launch policy: {setting}");
        }
        assert!(!args.iter().any(|arg| arg == "web_search=\"disabled\""));
        for key in ["OPENAI_API_KEY", "FILMCRAFT_PROVIDER_PERMIT", "FILMCRAFT_EXTERNAL_PROVIDER"] {
            assert!(command.get_envs().any(|(name, value)| name == key && value.is_none()), "Inherited provider state must be removed: {key}");
        }
    }

    fn catalog() -> Value {
        let tools: serde_json::Map<String, Value> =
            filmcraft_ui_egui::console::capabilities::CONSOLE_TOOLS.iter().map(|name| ((*name).to_owned(), json!({}))).collect();
        json!({"data": [{"name": SERVER, "runtimeStatus": "connected", "tools": tools}]})
    }
    fn replies(values: impl IntoIterator<Item = Value>) -> Messages {
        let (tx, rx) = mpsc::channel();
        for value in values {
            tx.send(Ok(value)).unwrap();
        }
        rx
    }
    #[test]
    fn microphone_mute_bypasses_a_full_protocol_command_queue() {
        let mut connection = Connection::new(None, egui::Context::default());
        let process = Arc::new(Process::default());
        connection.process = Some(Arc::clone(&process));
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.try_send(chat::CommandMessage::VoiceStop).unwrap();
        connection.commands = Some(sender);
        connection.set_voice_muted(true).unwrap();
        assert!(voice::MicrophoneGate::muted(process.microphone_gate.snapshot()));
        connection.set_voice_muted(false).unwrap();
        assert!(!voice::MicrophoneGate::muted(process.microphone_gate.snapshot()));
        assert!(matches!(receiver.try_recv(), Ok(chat::CommandMessage::VoiceStop)));
        assert!(receiver.try_recv().is_err());
    }
    #[test]
    fn handshake_only_discovers_tools_and_refuses_server_requests() {
        let rx = replies([
            json!({"id": 1, "result": {"userAgent": "test"}}),
            json!({"id":4,"result":{"config":{"mcp_servers":{"unrelated":{},"filmcraft_console":{}}}}}),
            json!({"id": "approval-1", "method": "item/commandExecution/requestApproval", "params": {}}),
            json!({"id": 2, "result": {"thread": {"id": "thread-1"}}}),
            json!({"id": 3, "result": catalog()}),
        ]);
        let mut output = Vec::new();
        handshake(&mut output, &rx, json!({"command":"filmcraft-cli","args":["mcp","--controller-read-only"]})).unwrap();
        let messages: Vec<Value> = String::from_utf8(output).unwrap().lines().map(|s| serde_json::from_str(s).unwrap()).collect();
        let methods: Vec<_> = messages.iter().filter_map(|v| v.get("method").and_then(Value::as_str)).collect();
        assert_eq!(methods, ["initialize", "initialized", "config/read", "thread/start", "mcpServerStatus/list"]);
        assert!(messages.iter().all(|v| v.get("jsonrpc").is_none()));
        assert_eq!(messages[3]["params"]["ephemeral"], true);
        assert_eq!(messages[3]["params"]["sandbox"], "read-only");
        assert_eq!(messages[3]["params"]["config"]["web_search"], "live");
        assert_eq!(messages[4]["error"]["code"], -32601);
        assert_eq!(messages[5]["params"]["threadId"], "thread-1");
    }

    #[test]
    fn resume_requests_live_search_even_when_the_previous_default_disabled_it() {
        use filmcraft_ui_egui::console::sessions::SessionRequest;
        let request = SessionRequest { project: "/project/song.fcproj".into(), resume: Some("saved-thread".into()), ..Default::default() };
        let rx = replies([
            json!({"id":1,"result":{}}),
            json!({"id":4,"result":{"config":{"web_search":"disabled","mcp_servers":{}}}}),
            json!({"id":2,"result":{"thread":{"id":"saved-thread"}}}),
            json!({"id":3,"result":catalog()}),
        ]);
        let mut output = Vec::new();
        let (thread, _, settings) =
            handshake_session(&mut output, &rx, json!({"command":"filmcraft-cli","args":["mcp","--controller-console"]}), Some(&request)).unwrap();
        assert_eq!(thread, "saved-thread");
        assert_eq!(settings["web_search"], "live");
        let messages: Vec<Value> = String::from_utf8(output).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        let resumed = messages.iter().find(|message| message["method"] == "thread/resume").unwrap();
        assert_eq!(resumed["params"]["threadId"], "saved-thread");
        assert_eq!(resumed["params"]["config"]["web_search"], "live");
        assert!(messages.iter().all(|message| message["method"] != "turn/start" && message["method"] != "thread/start"));
    }
    #[test]
    fn optional_voice_catalog_survives_interleaved_resume_without_replaying_any_edit() {
        use filmcraft_ui_egui::console::sessions::SessionRequest;
        let request = SessionRequest { project: "/project/song.fcproj".into(), resume: Some("thread-1".into()), ..Default::default() };
        let voice = json!({"id":9,"result":{"voices":{"v3":["sol","cove"]}}});
        for before_thread_reply in [true, false] {
            let thread = json!({"id":2,"result":{"thread":{"id":"thread-1"}}});
            let values = if before_thread_reply {
                vec![voice.clone(), thread, json!({"id":3,"result":catalog()})]
            } else {
                vec![thread, voice.clone(), json!({"id":3,"result":catalog()})]
            };
            let mut captured = None;
            let mut output = Vec::new();
            let (thread, _) = select_session_with_catalog(&mut output, &replies(values), &json!({}), Some(&request), &mut captured).unwrap();
            assert_eq!(thread, "thread-1");
            assert_eq!(captured.as_ref(), Some(&voice));
            let methods: Vec<String> = String::from_utf8(output)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap()["method"].as_str().unwrap().to_owned())
                .collect();
            assert_eq!(methods, ["thread/resume", "mcpServerStatus/list"]);
        }
        let mut captured = None;
        let result = response_with_catalog(
            &mut Vec::new(),
            &replies([voice.clone(), json!({"id":9,"result":{"voices":{"v3":["other"]}}}), json!({"id":2,"result":{}})]),
            2,
            &mut captured,
        );
        assert!(result.is_ok());
        assert_eq!(captured, Some(voice));
        assert!(response_with_catalog(&mut Vec::new(), &replies([json!({"id":88,"result":{}})]), 2, &mut None).is_err());
    }

    #[test]
    fn retained_voice_catalog_contains_only_bounded_validated_choices() {
        let reply = json!({"id":9,"result":{"voices":{"v3":["sol"]},"private":"secret".repeat(100000)}});
        let bounded = bounded_voice_catalog_reply(&reply);
        assert_eq!(bounded, json!({"id":9,"result":{"voices":{"v3":["sol"]}}}));
        for reply in [json!({"id":9,"error":{"message":"secret".repeat(100000)}}), json!({"id":9,"result":{"voices":{"v3":vec!["sol";65]}}})] {
            let bounded = bounded_voice_catalog_reply(&reply).to_string();
            assert!(bounded.len() < 256);
            assert!(!bounded.contains("secret"));
        }
    }

    #[test]
    fn durable_start_and_resume_rebind_current_tools_and_memory() {
        use filmcraft_ui_egui::console::sessions::SessionRequest;
        let mut request = SessionRequest { project: "/project/song.fcproj".into(), memory: "Hold the verse shots.".into(), ..Default::default() };
        for resume in [false, true] {
            request.resume = resume.then(|| "thread-1".into());
            let messages = replies([json!({"id":2,"result":{"thread":{"id":"thread-1"},"model":"default-model"}}), json!({"id":3,"result":catalog()})]);
            let mut output = Vec::new();
            select_session(&mut output, &messages, &json!({"mcp_servers":{"current":{}}}), Some(&request)).unwrap();
            let sent: Vec<Value> = String::from_utf8(output).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
            assert_eq!(sent[0]["method"], if resume { "thread/resume" } else { "thread/start" });
            let params = &sent[0]["params"];
            assert!(!params["developerInstructions"].as_str().unwrap().contains("Hold the verse shots."));
            assert_eq!(params["sandbox"], "read-only");
            assert_eq!(params["approvalPolicy"], "never");
            assert_eq!(params["config"]["mcp_servers"], json!({"current":{}}));
            if resume {
                assert!(params.get("dynamicTools").is_none());
                assert_eq!(params["excludeTurns"], true);
            } else {
                assert_eq!(params["ephemeral"], false);
                assert_eq!(params["dynamicTools"][0]["name"], "filmcraft_propose_edit");
            }
        }
    }
    #[test]
    fn failed_resume_is_actionable_and_never_falls_back_to_new_thread() {
        let request = filmcraft_ui_egui::console::sessions::SessionRequest {
            project: "test-project".into(),
            scope_aliases: Vec::new(),
            resume: Some("saved-thread".into()),
            memory: String::new(),
            model: None,
            recent_dialogue: Vec::new(),
            assistant_summary: String::new(),
        };
        let mut out = Vec::new();
        let error = select_session(&mut out, &replies([json!({"id":2,"error":{"message":"secret credential"}})]), &json!({}), Some(&request)).unwrap_err();
        assert!(error.contains("Could not resume"));
        assert!(error.contains("New Conversation"));
        assert!(!error.contains("secret credential"));
        let sent: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["method"], "thread/resume");
        assert_eq!(sent[0]["params"]["threadId"], "saved-thread");
    }
    #[test]
    fn legitimate_large_notifications_do_not_disconnect_or_lose_the_next_frame() {
        // The reported 533-clip project produced a 1.69 MB inspection notification.
        // Framing must also accept legitimate echoed PNG input payloads; tool output is
        // independently paginated and byte-budgeted before reaching this reader.
        let value =
            json!({"method":"item/completed","params":{"item":{"type":"mcpToolCall","result":{"content":[{"type":"text","text":"x".repeat(1_700_000)}]}}}});
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.extend_from_slice(b"\n{\"method\":\"turn/completed\"}\n");
        let mut input = Cursor::new(bytes);
        assert_eq!(read_message(&mut input).unwrap(), value);
        assert_eq!(read_message(&mut input).unwrap()["method"], "turn/completed");
    }
    #[test]
    fn malformed_frames_and_mutations_return_errors_without_panicking() {
        for bytes in [b"".as_slice(), b"{}", b"[]\n", b"{\n", b"\xff\n"] {
            assert!(read_message(&mut Cursor::new(bytes)).is_err());
        }
        let mut oversized = vec![b' '; MAX_MESSAGE + 1];
        oversized.push(b'\n');
        assert!(read_message(&mut Cursor::new(oversized)).is_err());
        let valid = b"{\"id\":1,\"result\":{}}\n";
        for length in 0..valid.len() {
            assert!(std::panic::catch_unwind(|| read_message(&mut Cursor::new(&valid[..length]))).unwrap().is_err());
        }
        for index in 0..valid.len() {
            let mut bytes = valid.to_vec();
            bytes[index] ^= 0xff;
            let _ = std::panic::catch_unwind(|| read_message(&mut Cursor::new(bytes))).unwrap();
        }
    }
    #[test]
    fn rejects_wrong_ids_errors_and_unexpected_tools_without_echoing_secrets() {
        let mut out = Vec::new();
        assert!(response(&mut out, &replies([json!({"id": 9, "result": {}})]), 1).is_err());
        let error = response(&mut out, &replies([json!({"id": 1, "error": {"message": "secret credential"}})]), 1).unwrap_err();
        assert!(!error.contains("secret credential"));
        assert!(response(&mut out, &replies([json!({"id": 1, "result": null})]), 1).is_err());
        assert!(response(&mut out, &replies([]), 1).is_err());
        let mut value = catalog();
        value["data"][0]["tools"]["arbitrary_edit"] = json!({});
        assert!(verify_catalog(&value).is_err());
        value = catalog();
        value["data"][0]["runtimeStatus"] = json!("failed");
        assert!(verify_catalog(&value).is_err());
        for value in [json!({}), json!({"data": []}), json!({"data": [{"name": SERVER, "tools": {}}]})] {
            assert!(verify_catalog(&value).is_err());
        }
    }
    #[test]
    fn runtime_config_scopes_direct_native_commands_without_embedding_token() {
        let config = server_config(std::path::Path::new("/a folder/filmcraft-cli"), 9876);
        assert_eq!(config["args"][1], "--controller-console");
        assert_eq!(
            config["tools"],
            json!({"command_run":{"approval_mode":"approve"},"command_batch":{"approval_mode":"approve"},"console_references":{"approval_mode":"approve"}})
        );
        assert_eq!(config["env_vars"], json!(["FILMCRAFT_CONTROL_TOKEN", "FILMCRAFT_PROVIDER_PERMIT"]));
        assert_eq!(config["args"][3], "127.0.0.1:9876");
        assert!(!config.to_string().contains("--controller-edit"));
    }
    #[test]
    fn missing_control_endpoint_is_actionable_and_disconnect_is_repeatable() {
        let mut connection = Connection::new(None, egui::Context::default());
        assert!(connection.connect().unwrap_err().contains("--control PORT"));
        connection.disconnect();
        connection.disconnect();
        assert!(connection.poll().is_none());
    }

    #[test]
    fn rapid_reconnect_cannot_accumulate_connection_workers() {
        let (tx, rx) = mpsc::channel::<()>();
        let mut connection = Connection::new(None, egui::Context::default());
        connection.worker = Some(std::thread::spawn(move || {
            let _ = rx.recv();
        }));
        for _ in 0..100 {
            connection.disconnect();
            connection.connect().unwrap();
            assert!(connection.pending_connect);
            assert!(connection.process.is_none());
        }
        tx.send(()).unwrap();
        connection.worker.take().unwrap().join().unwrap();
        assert!(matches!(connection.poll(),Some(ConnectionStatus::Failed(message)) if message.contains("--control PORT")));
    }

    /// No model call, credentials or native GUI. Uses a fresh Codex home and a synthetic app.
    #[test]
    #[ignore = "requires installed Codex CLI and FILMCRAFT_CONSOLE_TEST_CLI; no model inference"]
    fn installed_app_server_discovers_real_filmcraft_mcp() {
        use std::net::TcpListener;
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let token = "ab".repeat(32);
        let expected_token = token.clone();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_message(&mut BufReader::new(stream.try_clone().unwrap())).unwrap();
            assert_eq!(request["token"], expected_token);
            assert_eq!(request["params"]["command"], "project.inspect");
            send(&mut stream, &json!({"ok": true, "id": 1, "result": {}})).unwrap();
        });
        let cli = std::path::PathBuf::from(std::env::var_os("FILMCRAFT_CONSOLE_TEST_CLI").expect("set CLI path"));
        let home = std::env::temp_dir().join(format!("filmcraft-console-codex-smoke-{}", std::process::id()));
        std::fs::create_dir(&home).unwrap();
        std::fs::write(home.join("config.toml"), "[mcp_servers.unrelated]\ncommand = \"does-not-exist\"\nrequired = true\n").unwrap();
        let process = Arc::new(Process::default());
        let server_config = server_config(&cli, port);
        let mut command = app_server(&token);
        command.env("CODEX_HOME", &home);
        let mut ready = false;
        let result = probe(
            &process,
            port,
            &token,
            command,
            server_config,
            || {
                ready = true;
                process.stop();
            },
            None,
        );
        process.reap();
        server.join().unwrap();
        std::fs::remove_dir_all(&home).unwrap();
        assert!(result.is_ok(), "{result:?}");
        assert!(ready);
        assert!(process.child.lock().unwrap().is_none());
    }
}

#[cfg(test)]
mod echo_mode_tests {
    use super::*;
    #[test]
    fn echo_configuration_does_not_unmute_and_rejects_a_stopped_transport() {
        let mut connection = Connection::new(None, egui::Context::default());
        let process = Arc::new(Process::default());
        connection.process = Some(process.clone());
        process.microphone_gate.set(true).unwrap();
        let muted = process.microphone_gate.snapshot();
        connection.set_voice_echo_reduction(true).unwrap();
        assert!(process.microphone_gate.suppress_for_output(true));
        assert_eq!(process.microphone_gate.snapshot(), muted);
        connection.set_voice_echo_reduction(false).unwrap();
        assert!(!process.microphone_gate.suppress_for_output(true));
        let mut payload = vec![5; 160];
        assert!(process.microphone_gate.protect_packet(muted, &mut payload));
        assert_eq!(payload, vec![255; 160]);
        process.stopped.store(true, Ordering::SeqCst);
        assert!(connection.set_voice_echo_reduction(true).is_err());
        assert!(!process.microphone_gate.suppress_for_output(true));
    }
}
