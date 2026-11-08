//! Session-only command console; all execution goes through the normal UI/engine dispatcher.
use crate::{FilmcraftApp, menus};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::Write;

mod analysis;
pub mod api_settings;
pub(crate) mod assistants;
mod bridge;
pub mod capabilities;
pub mod chat;
mod commands;
pub mod connection_manager;
pub(crate) mod diagnostics;
mod execution;
mod layout;
pub mod provider;
pub mod reference_library;
mod references;
mod session_lock;
pub mod sessions;
pub(crate) fn invoke_session(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: Value) -> Result<Value, String> {
    commands::invoke(app, ctx, id, params)
}
pub use chat::{ChatEvent, EDIT_COMMANDS, ModelOption, VoiceInputStats, VoiceStatus, VoiceTiming};

const MAX_LINE: usize = 16 * 1024;
const MAX_ENTRIES: usize = 64;
const MAX_OUTPUT: usize = 16 * 1024;

#[derive(Clone, Debug, Default)]
pub struct Console {
    pub open: bool,
    pub settings_open: bool,
    pub connection: ConnectionStatus,
    pub chat: chat::Chat,
    pub sessions: sessions::Sessions,
    pub session_lock: session_lock::SessionLock,
    pub bridge: bridge::State,
    pub(crate) diagnostics: diagnostics::State,
    analysis: analysis::State,
    pub voice_channel: u16,
    pub references: references::References,
    pub reference_library: reference_library::State,
    pub voice_channels: u16,
    input: String,
    entries: VecDeque<Entry>,
}

/// Host-provided local Codex transport. Never stored in a project or UI preferences.
pub trait CodexConnection {
    /// Secret settings travel only through this in-process method, never command JSON.
    fn configure_api(&mut self, _key: Option<String>, _model: String) -> Result<(), String> {
        Err("In-app API setup is unavailable in this transport".into())
    }
    fn api_configuration(&self) -> Option<api_settings::Status> {
        None
    }
    fn forget_api_configuration(&mut self) -> Result<(), String> {
        Err("In-app API setup is unavailable in this transport".into())
    }
    fn provider_id(&self) -> provider::ProviderId {
        provider::ProviderId::Codex
    }
    fn provider_capabilities(&self) -> Value {
        json!({"embeddedComposer":true,"nativeTools":true,"images":"modelDependent","audioAnalysis":false,"voice":true})
    }
    fn select_provider(&mut self, provider: provider::ProviderId) -> Result<(), String> {
        if provider == provider::ProviderId::Codex { Ok(()) } else { Err("This host has not installed the provider connection manager".into()) }
    }
    fn bind_provider_permit(&mut self, _permit: String) -> Result<(), String> {
        Err("This transport is not qualified for provider ownership".into())
    }
    fn poll_owned_status(&mut self) -> Option<(String, ConnectionStatus)> {
        None
    }
    fn poll_owned_chat(&mut self) -> Option<(String, ChatEvent)> {
        None
    }
    /// Opaque provider permit validated again by the UI control dispatcher.
    fn authorizes_provider(&self, _permit: &str) -> bool {
        false
    }
    /// Private external MCP lifecycle, authenticated by the current transport permit.
    fn external_provider_event(&mut self, _permit: &str, _event: &str) -> Result<(), String> {
        Err("This connection does not accept external attachments".into())
    }
    /// Validate provider-specific resume identity before replacing current ownership.
    fn validate_session_request(&self, _request: &sessions::SessionRequest) -> Result<(), String> {
        Ok(())
    }
    fn connect(&mut self) -> Result<(), String>;
    fn connect_session(&mut self, _request: sessions::SessionRequest) -> Result<(), String> {
        self.connect()
    }
    fn switch_session(&mut self, _request: sessions::SessionRequest) -> Result<(), String> {
        Err("Session switching is unavailable".into())
    }
    fn poll(&mut self) -> Option<ConnectionStatus>;
    fn disconnect(&mut self);
    fn poll_chat(&mut self) -> Option<ChatEvent> {
        None
    }
    fn send_prompt(&mut self, _prompt: String) -> Result<(), String> {
        Err("Chat is unavailable".into())
    }
    fn send_prompt_with_model(&mut self, prompt: String, _model: Option<String>) -> Result<(), String> {
        self.send_prompt(prompt)
    }
    fn send_prompt_with_images(&mut self, prompt: String, model: Option<String>, images: Vec<String>) -> Result<(), String> {
        if !images.is_empty() {
            return Err("This connection does not support image attachments".into());
        }
        self.send_prompt_with_model(prompt, model)
    }
    fn start_voice(&mut self, _model: Option<String>) -> Result<(), String> {
        Err("Codex voice is unavailable".into())
    }
    fn start_voice_channel(&mut self, model: Option<String>, _channel: u16) -> Result<(), String> {
        self.start_voice(model)
    }
    fn start_voice_selected(&mut self, model: Option<String>, channel: u16, voice: Option<String>) -> Result<(), String> {
        if voice.is_some() {
            return Err("This connection cannot select a speaking voice".into());
        }
        self.start_voice_channel(model, channel)
    }
    /// Full-duplex headphones are the base behavior. Speaker echo reduction is optional.
    fn set_voice_echo_reduction(&mut self, enabled: bool) -> Result<(), String> {
        if enabled { Err("Speaker echo reduction is unavailable for this voice connection".into()) } else { Ok(()) }
    }
    fn set_voice_muted(&mut self, _muted: bool) -> Result<(), String> {
        Err("Microphone muting is unavailable".into())
    }
    fn stop_voice(&mut self) -> Result<(), String> {
        Err("Codex voice is unavailable".into())
    }
    fn cancel(&mut self) -> Result<(), String> {
        Err("Chat is unavailable".into())
    }
    fn answer(&mut self, _id: Value, _result: Result<Value, String>) -> Result<(), String> {
        Err("Chat is unavailable".into())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ConnectionStatus {
    #[default]
    Disconnected,
    Connecting,
    Ready,
    Failed(String),
}
impl ConnectionStatus {
    pub fn id(&self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Connecting => "connecting",
            Self::Ready => "ready",
            Self::Failed(_) => "failed",
        }
    }
}

#[derive(Clone, Debug)]
struct Entry {
    line: String,
    output: String,
    failed: bool,
}

fn parse(line: &str) -> Result<(&str, Value), String> {
    if line.len() > MAX_LINE {
        return Err("Command is too long (maximum 16 KiB)".into());
    }
    let line = line.trim();
    let (id, params) = line.split_once(char::is_whitespace).unwrap_or((line, "{}"));
    if id.is_empty() {
        return Err("Enter a command ID, optionally followed by a JSON object".into());
    }
    if id == "console.run" {
        return Err("The console cannot recursively run console.run".into());
    }
    let params: Value = serde_json::from_str(params.trim()).map_err(|e| format!("Invalid JSON parameters: {e}"))?;
    if !params.is_object() {
        return Err("Command parameters must be a JSON object".into());
    }
    Ok((id, params))
}

/// Bounded JSON preview without allocating an unbounded serialized result.
struct Preview(Vec<u8>);
impl Write for Preview {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let left = MAX_OUTPUT.saturating_sub(self.0.len());
        if bytes.len() > left {
            self.0.extend(bytes.iter().take(left));
            return Err(std::io::Error::other("preview limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn preview(value: &Value) -> String {
    let mut writer = Preview(Vec::new());
    let truncated = serde_json::to_writer_pretty(&mut writer, value).is_err();
    let mut text = String::from_utf8_lossy(&writer.0).into_owned();
    if truncated {
        text.push_str("\n… result preview truncated");
    }
    text
}

pub(crate) fn execute(app: &mut FilmcraftApp, ctx: &egui::Context, line: &str) -> Result<Value, String> {
    // Oversized requests are rejected before copying them into history.
    if line.len() > MAX_LINE {
        return Err("Command is too long (maximum 16 KiB)".into());
    }
    let result = parse(line).and_then(|(id, params)| menus::invoke(app, ctx, id, params));
    let output = match &result {
        Ok(value) => preview(value),
        Err(error) => preview(&json!({"error": error})),
    };
    let entries = &mut app.ui.console.entries;
    if entries.len() >= MAX_ENTRIES {
        entries.pop_front();
    }
    entries.push_back(Entry { line: line.to_owned(), output, failed: result.is_err() });
    result
}

pub(crate) fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    assistants::set_context(app, ctx);
    sessions::sync_project(app);
    if let Some(connection) = app.hooks.codex_connection.as_mut() {
        for _ in 0..4 {
            let Some(status) = connection.poll() else { break };
            app.ui.console.connection = status;
        }
    }
    references::poll(app, ctx);
    chat::poll(app, ctx);
    sessions::refresh_context(app);
    if let Err(error) = assistants::poll(app) {
        app.ui.status = error;
    }
    if !app.ui.console.open && app.ui.console.chat.voice.on() && app.ui.console.chat.voice != VoiceStatus::Stopping {
        chat::stop_voice(app);
    }
    if !app.modal_open(ctx) {
        reference_library::show(app, ctx);
    }
    if !app.ui.console.open || app.modal_open(ctx) {
        return;
    }
    let mut open = true;
    let mut run = false;
    let mut recall = None;
    egui::Window::new(tl!("Command Console"))
        .id(egui::Id::unique("command-console"))
        .open(&mut open)
        .default_size(egui::vec2(660.0, 700.0))
        .vscroll(true)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
            let section = egui::CollapsingHeader::new("AI connection").default_open(true).show(ui, |ui| {
                connection_manager::selector(app, ui);
                let selected = app.hooks.codex_connection.as_ref().map(|connection| connection.provider_id()).unwrap_or_default();
                let provider_label = selected.label();
                if selected == provider::ProviderId::OpenAiApi {
                    api_settings::show(app, ui);
                }
                match &app.ui.console.connection {
                    ConnectionStatus::Disconnected => {
                        ui.label(tl!("Disconnected"));
                    }
                    ConnectionStatus::Connecting => {
                        if selected == provider::ProviderId::DesktopChatGpt {
                            ui.label("Waiting for ChatGPT to use FilmCraft tools.");
                        } else {
                            ui.label(format!("Checking {provider_label} and FilmCraft bridge…"));
                        }
                    }
                    ConnectionStatus::Ready => {
                        ui.label(format!("{provider_label} connected; FilmCraft inspection tools verified."));
                    }
                    ConnectionStatus::Failed(message) => {
                        ui.colored_label(ui.visuals().error_fg_color, message);
                    }
                }
                ui.horizontal(|ui| {
                    let active = matches!(app.ui.console.connection, ConnectionStatus::Connecting | ConnectionStatus::Ready);
                    let connect_label = format!("Connect {provider_label}");
                    let can_connect = !active && app.hooks.codex_connection.is_some();
                    let connect = layout::icon_button(app, ui, "console.codex.connect", crate::icons::Icon::Power, can_connect, false, &connect_label);
                    if connect.clicked()
                        && let Err(error) = sessions::reconnect(app)
                    {
                        app.ui.status = error;
                    }
                    let disconnect_label = format!("Disconnect {provider_label}");
                    let disconnect = layout::icon_button(app, ui, "console.codex.disconnect", crate::icons::Icon::Close, active, false, &disconnect_label);
                    if disconnect.clicked() {
                        sessions::disconnect(app);
                    }
                });
                if app.hooks.codex_connection.is_none() {
                    ui.label(tl!("Codex connections require the desktop app."));
                }
            });
            app.auto.add("console.codex.toggle", section.header_response.rect, "Local Codex connection");
            if section.body_returned.is_none()
                && let ConnectionStatus::Failed(message) = &app.ui.console.connection
            {
                ui.colored_label(ui.visuals().error_fg_color, message);
            }
            if app.hooks.codex_connection.is_none() {
                ui.horizontal_wrapped(|ui| {
                    layout::undo(app, ui);
                    layout::settings(app, ui);
                });
            }
            sessions::show(app, ui);
            chat::show(app, ui);
            let advanced = egui::CollapsingHeader::new(tl!("Diagnostics & manual commands")).id_salt("console-advanced").default_open(false).show(ui, |ui| {
                diagnostics::show(app, ui);
                ui.label(tl!("Enter a FilmCraft command ID and optional JSON parameters."));
                ui.monospace("playhead.set {\"seconds\": 2}");
                let input = ui.add(
                    egui::TextEdit::singleline(&mut app.ui.console.input)
                        .char_limit(MAX_LINE)
                        .desired_width(f32::INFINITY)
                        .hint_text("command.id {\"parameter\": \"value\"}"),
                );
                app.auto.add("console.input", input.rect, "Console command");
                let enter = input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    let response = ui.button(tl!("Run"));
                    app.auto.add("console.run", response.rect, "Run console command");
                    run = response.clicked() || enter;
                    let response = ui.button(tl!("Clear History"));
                    app.auto.add("console.clear", response.rect, "Clear console history");
                    if response.clicked() {
                        app.ui.console.entries.clear();
                    }
                });
                let query = app.ui.console.input.trim();
                if !query.is_empty() && !query.contains(char::is_whitespace) {
                    let commands = filmcraft_engine::command_specs()
                        .iter()
                        .map(|c| (c.id, c.label, c.params))
                        .chain(menus::UI_COMMANDS.iter().filter(|c| c.id != "console.run").map(|c| (c.id, c.label, "UI command")));
                    for (id, label, params) in commands.filter(|(id, _, _)| id.starts_with(query)).take(6) {
                        let response = ui.button(format!("{id} — {label}"));
                        app.auto.add(&format!("console.suggest.{id}"), response.rect, label);
                        if response.clicked() {
                            recall = Some(id.to_owned());
                        }
                        response.on_hover_text(params);
                    }
                }
                ui.separator();
                egui::ScrollArea::vertical().id_salt("console.history").max_height(140.0).stick_to_bottom(true).show(ui, |ui| {
                    for (index, entry) in app.ui.console.entries.iter().enumerate() {
                        let response = ui.button(&entry.line);
                        app.auto.add(&format!("console.history.{index}"), response.rect, "Recall console command");
                        if response.clicked() {
                            recall = Some(entry.line.clone());
                        }
                        if entry.failed {
                            ui.colored_label(ui.visuals().error_fg_color, &entry.output);
                        } else {
                            ui.monospace(&entry.output);
                        }
                    }
                });
                if run {
                    input.request_focus();
                }
            });
            app.auto.add("console.advanced.toggle", advanced.header_response.rect, "Diagnostics and manual commands");
        });
    app.ui.console.open = open;
    if !open && app.ui.console.chat.voice.on() {
        chat::stop_voice(app);
    }
    if let Some(line) = recall {
        app.ui.console.input = line;
    }
    if run {
        let line = app.ui.console.input.clone();
        if let Err(error) = execute(app, ctx, &line) {
            app.ui.status = error;
        }
    }
}

/// Consume a drop on the console before the editor's media-import route sees it.
pub(crate) fn drop_files(app: &mut FilmcraftApp, ctx: &egui::Context, pos: Option<egui::Pos2>, files: &[egui::DroppedFileHandle]) -> bool {
    if !app.ui.console.open || !pos.is_some_and(|p| app.ui.console.references.drop_rect.is_some_and(|r| r.contains(p))) {
        return false;
    }
    for file in files.iter().take(5) {
        if let Err(e) = commands::invoke(app, ctx, "console.attachments.add", json!({"path":file.path().to_string_lossy()})) {
            app.ui.status = e;
            break;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_engine::Session;

    #[test]
    fn hostile_input_does_not_edit_and_console_recovers() {
        let mut app = FilmcraftApp::new(Session::default());
        let ctx = egui::Context::default();
        let before = app.session.project.clone();
        for line in ["", "no.suchCommand", "file.newSequence []", "file.newSequence {", "console.run", "playhead.set {\"seconds\": -1e300}"] {
            assert!(execute(&mut app, &ctx, line).is_err(), "{line}");
            assert_eq!(*app.session.project, *before);
        }
        assert!(execute(&mut app, &ctx, &"x".repeat(MAX_LINE + 1)).is_err());
        execute(&mut app, &ctx, "file.newSequence {\"width\": 16, \"height\": 16}").unwrap();
        assert_ne!(*app.session.project, *before);
        execute(&mut app, &ctx, "edit.undo").unwrap();
        assert_eq!(*app.session.project, *before);
        execute(&mut app, &ctx, "edit.redo").unwrap();
        assert_ne!(*app.session.project, *before);
    }

    #[test]
    fn console_command_rejects_hostile_wrapper_parameters() {
        let mut app = FilmcraftApp::new(Session::default());
        let ctx = egui::Context::default();
        let before = app.session.project.clone();
        for params in
            [Value::Null, json!([]), json!({}), json!({"line": 1}), json!({"line": "file.newSequence", "extra": true}), json!({"line": "console.run"})]
        {
            assert!(menus::invoke(&mut app, &ctx, "console.run", params).is_err());
            assert_eq!(*app.session.project, *before);
        }
    }

    #[test]
    fn history_and_output_are_bounded_and_not_persisted() {
        let mut app = FilmcraftApp::new(Session::default());
        let ctx = egui::Context::default();
        for _ in 0..MAX_ENTRIES + 3 {
            let _ = execute(&mut app, &ctx, "no.suchCommand");
        }
        assert_eq!(app.ui.console.entries.len(), MAX_ENTRIES);
        let output = preview(&json!("é".repeat(MAX_OUTPUT)));
        assert!(output.ends_with("result preview truncated"));
        assert!(output.len() < MAX_OUTPUT + 100);
        app.ui.console.input = "private unsent console draft".into();
        app.ui.console.connection = ConnectionStatus::Failed("private connection failure".into());
        let saved = serde_json::to_value(&app.ui).unwrap();
        assert!(saved.get("console").is_none());
        assert!(!saved.to_string().contains("private unsent console draft"));
        assert!(!saved.to_string().contains("private connection failure"));
    }
}
