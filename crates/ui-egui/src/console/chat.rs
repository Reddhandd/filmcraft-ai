//! Session-only chat and explicitly approved, revision-guarded edits.
pub use super::capabilities::EDIT_COMMANDS;
use super::*;
use filmcraft_engine::Session;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
#[derive(Clone, Debug)]
pub enum ChatEvent {
    TurnStarted,
    SessionStarted(String),
    Voice(VoiceStatus),
    VoiceTiming(Option<VoiceTiming>),
    VoiceText {
        role: String,
        text: String,
        partial: bool,
    },
    VoiceStats {
        sent: u64,
        received: u64,
    },
    VoiceInput(VoiceInputStats),
    Models(Vec<ModelOption>, Option<String>),
    VoiceChoices {
        voices: Vec<String>,
        error: Option<String>,
        loading: bool,
    },
    Inspected,
    Delta(String),
    Finished,
    /// A recoverable provider error; the current turn remains in progress.
    Retrying(String),
    Error(String),
    Proposal {
        id: Value,
        command: String,
        params: Value,
    },
}
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct VoiceInputStats {
    pub device: String,
    pub channel: u16,
    pub channels: u16,
    pub input_peak: f32,
    pub transmit_peak: f32,
    pub gated: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelOption {
    pub id: String,
    pub label: String,
    pub effort: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum VoiceStatus {
    #[default]
    Off,
    Connecting,
    Active,
    Stopping,
    Failed(String),
}
impl VoiceStatus {
    pub fn on(&self) -> bool {
        matches!(self, Self::Connecting | Self::Active | Self::Stopping)
    }
    pub fn id(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Connecting => "connecting",
            Self::Active => "active",
            Self::Stopping => "stopping",
            Self::Failed(_) => "failed",
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct Chat {
    pub inspection_count: u64,
    pub voice: VoiceStatus,
    pub voice_muted: bool,
    pub voice_timing: Option<VoiceTiming>,
    pub voice_sent: u64,
    pub voice_received: u64,
    pub voice_input: VoiceInputStats,
    voice_partial: bool,
    pub busy: bool,
    pub models: Vec<ModelOption>,
    pub selected_model: Option<String>,
    pub default_model: Option<String>,
    pub voices: Vec<String>,
    pub voice_catalog_error: Option<String>,
    pub voice_catalog_pending: bool,
    pub(super) prompt: String,
    pub(super) paste_requested: bool,
    input_composing: bool,
    pub(super) messages: VecDeque<(String, String)>,
    pub pending: Option<Proposal>,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceTiming {
    pub elapsed_seconds: u64,
    pub renew_after_seconds: u64,
    pub renewing: bool,
}
#[derive(Clone, Debug)]
pub struct Proposal {
    pub(super) id: Value,
    pub(super) command: String,
    params: Value,
    revision: u64,
    epoch: u64,
    project: Arc<filmcraft_project::Project>,
    state: Value,
    prefs: Value,
    created: Instant,
    result: String,
}
impl Chat {
    pub fn transcript(&self) -> Value {
        json!(self.messages.iter().map(|(role, text)| json!({"role":role,"text":text})).collect::<Vec<_>>())
    }
}
impl Proposal {
    pub(super) fn prepare(session: &Session, id: Value, command: String, params: Value) -> Result<Self, String> {
        Self::prepare_inner(session, id, command, params, false)
    }
    pub(super) fn prepare_owned(session: &Session, id: Value, command: String, params: Value) -> Result<Self, String> {
        Self::prepare_inner(session, id, command, params, true)
    }
    fn prepare_inner(session: &Session, id: Value, command: String, params: Value, owned: bool) -> Result<Self, String> {
        if matches!(command.as_str(), "clip.analyzeMusic.apply" | "clip.matchPerformance.apply" | "clip.trimPerformancePassage") && !owned {
            return Err("Use console_propose_edit with an owned analysisId; model-supplied plans are not accepted".into());
        }
        if !(EDIT_COMMANDS.contains(&command.as_str()) || owned && command == "clip.trimPerformancePassage")
            || !params.is_object()
            || params.to_string().len() > if owned { 512 * 1024 } else { MAX_LINE }
        {
            return Err("Unsupported edit or invalid parameters; only bounded timeline, marker and effect edits are available".into());
        }
        if command == "markers.add" && params.get("target").and_then(Value::as_str) != Some("program") {
            return Err("Marker proposals must explicitly target program".into());
        }
        if command == "markers.add"
            && (params.get("time").is_none_or(|v| v.as_i64().is_none_or(|n| n < 0))
                || params.get("durationFrames").is_some_and(|v| v.as_i64().is_none_or(|n| n < 0)))
        {
            return Err("Marker time and duration must be non-negative integer values".into());
        }
        if command == "edit.clear" && !params.get("clips").and_then(Value::as_array).is_some_and(|clips| !clips.is_empty()) {
            return Err("Deletion proposals must list explicit clip IDs".into());
        }
        if command == "timeline.place"
            && (params.get("time").and_then(Value::as_i64).is_none_or(|t| t < 0)
                || params.get("track").is_none()
                || params.get("insert").and_then(Value::as_bool).is_none())
        {
            return Err("Placement proposals require an explicit track, non-negative Tick time and insert boolean".into());
        }
        if command == "timeline.razor"
            && (params.get("clip").and_then(Value::as_u64).is_none() || params.get("time").and_then(Value::as_i64).is_none_or(|t| t < 0))
        {
            return Err("Split proposals require explicit clip IDs and a non-negative Tick time".into());
        }
        let mut dry = Session::default();
        dry.revision = session.revision;
        dry.document_epoch = session.document_epoch;
        dry.prefs = session.prefs.clone();
        dry.path = session.path.clone();
        dry.services = Arc::clone(&session.services);
        dry.project = Arc::clone(&session.project);
        dry.state = session.state.clone();
        dry.media = Arc::clone(&session.media);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| dry.execute(&command, params.clone())))
            .map_err(|_| "Edit preview failed; the working project is unchanged")?
            .map_err(|e| e.to_string())?;
        if *dry.project == *session.project {
            return Err("This proposal would not change the project".into());
        }
        Ok(Self {
            id,
            command,
            params,
            revision: session.revision,
            epoch: session.document_epoch,
            project: Arc::clone(&session.project),
            state: serde_json::to_value(&session.state).map_err(|e| e.to_string())?,
            prefs: serde_json::to_value(&session.prefs).map_err(|e| e.to_string())?,
            created: Instant::now(),
            result: preview(&result),
        })
    }
    pub(super) fn apply(self, app: &mut FilmcraftApp, ctx: &egui::Context) -> Result<Value, String> {
        if self.created.elapsed() > Duration::from_secs(300)
            || self.revision != app.session.revision
            || self.epoch != app.session.document_epoch
            || !Arc::ptr_eq(&self.project, &app.session.project)
            || self.state != serde_json::to_value(&app.session.state).map_err(|e| e.to_string())?
            || self.prefs != serde_json::to_value(&app.session.prefs).map_err(|e| e.to_string())?
        {
            return Err("The project or editing context changed; request a fresh proposal".into());
        }
        let before = app.session.revision;
        let result = menus::invoke(app, ctx, &self.command, self.params.clone());
        super::assistants::record(app, &self.command, &self.params, before, &result);
        result
    }
}
pub(super) fn message(chat: &mut Chat, role: &str, text: String) {
    if chat.messages.len() == 32 {
        chat.messages.pop_front();
    }
    chat.messages.push_back((role.into(), text));
}
pub(super) fn reply(app: &mut FilmcraftApp, id: Value, result: Result<Value, String>) {
    if app.ui.console.bridge.complete(&id, result.clone()) {
        return;
    }
    if let Some(connection) = app.hooks.codex_connection.as_mut()
        && let Err(error) = connection.answer(id, result)
    {
        app.ui.status = error;
    }
}
pub(super) fn poll(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let mut persist = false;
    for _ in 0..64 {
        let Some(event) = app.hooks.codex_connection.as_mut().and_then(|c| c.poll_chat()) else { break };
        if matches!(
            &event,
            ChatEvent::SessionStarted(_)
                | ChatEvent::TurnStarted
                | ChatEvent::Delta(_)
                | ChatEvent::VoiceText { .. }
                | ChatEvent::Finished
                | ChatEvent::Error(_)
        ) {
            super::assistants::changed(app);
        }
        match event {
            ChatEvent::SessionStarted(id) => {
                if let Err(error) = super::sessions::started(app, id) {
                    app.ui.status = error;
                }
                message(&mut app.ui.console.chat, "Session", "Project memory loaded; conversation ready. Inspect current project state before editing.".into());
            }
            ChatEvent::TurnStarted => {
                app.ui.console.chat.busy = true;
            }
            ChatEvent::Voice(status) => {
                persist |= status == VoiceStatus::Off;
                if status == VoiceStatus::Off {
                    app.ui.console.chat.voice_input = VoiceInputStats::default();
                }
                if !status.on() {
                    app.ui.console.chat.voice_muted = false;
                }
                app.ui.console.chat.voice = status;
                app.ui.console.chat.voice_partial = false;
            }
            ChatEvent::VoiceTiming(timing) => app.ui.console.chat.voice_timing = timing,
            ChatEvent::VoiceInput(mut stats) => {
                if app.ui.console.chat.voice.on() {
                    stats.input_peak = if stats.input_peak.is_finite() { stats.input_peak.clamp(0.0, 1.0) } else { 0.0 };
                    stats.transmit_peak = if stats.transmit_peak.is_finite() { stats.transmit_peak.clamp(0.0, 1.0) } else { 0.0 };
                    app.ui.console.voice_channels = stats.channels.min(256);
                    app.ui.console.chat.voice_input = stats;
                }
            }
            ChatEvent::VoiceStats { sent, received } => {
                app.ui.console.chat.voice_sent = sent;
                app.ui.console.chat.voice_received = received;
            }
            ChatEvent::VoiceText { role, text, partial } => {
                let role = if role == "user" { tl!("You (voice)") } else { tl!("Codex (voice)") };
                let chat = &mut app.ui.console.chat;
                let mut end = text.len().min(64 * 1024);
                while !text.is_char_boundary(end) {
                    end = end.saturating_sub(1);
                }
                let text = text.get(..end).unwrap_or("");
                if chat.voice_partial && chat.messages.back().is_some_and(|(old, _)| old == role) {
                    if let Some((_, old)) = chat.messages.back_mut() {
                        if partial {
                            let left = (64usize * 1024).saturating_sub(old.len());
                            let mut end = text.len().min(left);
                            while !text.is_char_boundary(end) {
                                end = end.saturating_sub(1);
                            }
                            if let Some(text) = text.get(..end) {
                                old.push_str(text);
                            }
                        } else {
                            *old = text.into();
                        }
                    }
                } else if !chat.messages.back().is_some_and(|(old_role, old)| old_role == role && old == text) {
                    message(chat, role, text.into());
                }
                chat.voice_partial = partial;
            }
            ChatEvent::VoiceChoices { voices, error, loading } => {
                let valid = voices.len() <= 64
                    && voices.iter().all(|id| !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_lowercase() || b == b'-' || b == b'_'));
                let accepted = valid && error.is_none();
                app.ui.console.chat.voices = if accepted { voices } else { Vec::new() };
                app.ui.console.chat.voice_catalog_pending = loading && accepted;
                app.ui.console.chat.voice_catalog_error = if !valid || error.is_some() {
                    Some("Speaking-voice choices are unavailable; Use Default remains available. Reconnect to refresh choices.".into())
                } else {
                    None
                };
            }
            ChatEvent::Models(models, default_model) => {
                app.ui.console.chat.default_model = default_model;
                if app.ui.console.chat.selected_model.as_ref().is_some_and(|id| !models.iter().any(|m| &m.id == id)) {
                    app.ui.console.chat.selected_model = None;
                }
                app.ui.console.chat.models = models;
            }
            ChatEvent::Inspected => {
                app.ui.console.chat.inspection_count = app.ui.console.chat.inspection_count.saturating_add(1);
            }
            ChatEvent::Delta(delta) => {
                if super::assistants::maintenance_active(app) {
                    continue;
                }
                let role = app.hooks.codex_connection.as_ref().map(|connection| connection.provider_id().label()).unwrap_or("Codex");
                let chat = &mut app.ui.console.chat;
                if !chat.busy {
                    continue;
                }
                if !chat.messages.back().is_some_and(|(old_role, _)| old_role == role) {
                    message(chat, role, String::new());
                }
                if let Some((_, text)) = chat.messages.back_mut() {
                    const TRUNCATED: &str = "\n… reply display truncated";
                    if text.ends_with(TRUNCATED) {
                        continue;
                    }
                    let mut end = delta.len().min((64usize * 1024).saturating_sub(text.len()));
                    while !delta.is_char_boundary(end) {
                        end = end.saturating_sub(1);
                    }
                    if let Some(part) = delta.get(..end) {
                        text.push_str(part);
                    }
                    if end < delta.len() {
                        text.push_str(TRUNCATED);
                    }
                }
            }
            ChatEvent::Finished => {
                super::assistants::milestone(app);
                persist = true;
                app.ui.console.chat.busy = false;
                super::bridge::cancel_pending(app, "The console turn ended before approval");
            }
            ChatEvent::Retrying(detail) => {
                app.ui.status = detail.clone();
                message(&mut app.ui.console.chat, "Connection", detail);
            }
            ChatEvent::Error(error) => {
                persist = true;
                app.ui.console.chat.busy = false;
                super::bridge::cancel_pending(app, "The console turn failed before approval");
                message(&mut app.ui.console.chat, "Error", error);
            }
            ChatEvent::Proposal { id, command, params } => {
                if app.ui.console.chat.pending.is_some() || !app.ui.console.chat.busy {
                    reply(app, id, Err("Another proposal is pending, or the turn has ended".into()));
                    continue;
                }
                let mut request = serde_json::Map::new();
                request.insert("command".into(), json!(command));
                request.insert("params".into(), params);
                let result = super::execution::run(app, ctx, &request);
                reply(app, id, result);
            }
        }
    }
    if persist && let Err(error) = super::sessions::save_chat(app) {
        app.ui.status = error;
    }
    if !matches!(app.ui.console.connection, ConnectionStatus::Ready | ConnectionStatus::Connecting) {
        app.ui.console.chat.busy = false;
        super::bridge::cancel_pending(app, "Codex disconnected before approval");
        app.ui.console.chat.voice = VoiceStatus::Off;
    }
    if app.ui.console.chat.pending.as_ref().is_some_and(|p| p.created.elapsed() > Duration::from_secs(300))
        && let Some(p) = app.ui.console.chat.pending.take()
    {
        reply(app, p.id, Err("Proposal expired; request a fresh proposal".into()));
    }
}
pub(super) fn stop_voice(app: &mut FilmcraftApp) {
    if let Err(error) = super::commands::voice_off(app) {
        app.ui.status = error;
    }
}
// Consume plain Return only for the focused draft. IME commits and held-key repeats
// must never submit a message. Shift+Return remains owned by TextEdit.
fn draft_return(ctx: &egui::Context, id: egui::Id, composing: &mut bool) -> bool {
    if !ctx.memory(|m| m.has_focus(id)) || !ctx.input(|input| input.focused) {
        *composing = false;
        return false;
    }
    ctx.input_mut(|input| {
        let ime_frame = input.events.iter().any(|event| matches!(event, egui::Event::Ime(_)));
        for event in &input.events {
            match event {
                egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => *composing = !text.is_empty(),
                egui::Event::Ime(egui::ImeEvent::Commit(_)) => *composing = false,
                _ => {}
            }
        }
        if *composing || ime_frame {
            return false;
        }
        let mut submit = false;
        input.events.retain(|event| {
            if let egui::Event::Key { key: egui::Key::Enter, pressed, repeat, modifiers, .. } = event
                && *modifiers == egui::Modifiers::NONE
            {
                submit |= *pressed && !*repeat;
                false
            } else {
                true
            }
        });
        submit
    })
}

fn action(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: Value) {
    if let Err(error) = menus::invoke(app, ctx, id, params) {
        app.ui.status = error;
    }
}
fn show_history(app: &mut FilmcraftApp, ui: &mut egui::Ui, embedded: bool) {
    egui::ScrollArea::vertical().id_salt("console.chat.history").max_height(240.0).min_scrolled_height(100.0).stick_to_bottom(true).show(ui, |ui| {
        let mut clipboard_action = None;
        let mut labels = Vec::new();
        let mut menu_open = false;
        for (index, (role, text)) in app.ui.console.chat.messages.iter().enumerate() {
            labels.push(egui::Label::new(egui::RichText::new(role).strong()).selectable(true).layout_in_ui(ui));
            let (pos, galley, response) = egui::Label::new(text).selectable(true).layout_in_ui(ui);
            app.auto.add(&format!("console.chat.message.{index}"), response.rect, "Console message");
            menu_open |= response.context_menu_opened();
            response.context_menu(|ui| {
                let copy = ui.button(tl!("Copy"));
                app.auto.add("console.chat.context.copy", copy.rect, "Copy message");
                if copy.clicked() {
                    clipboard_action = Some(("console.chat.copy", json!({"index":index,"selection":true})));
                    ui.close();
                }
                let paste = ui.add_enabled(embedded, egui::Button::new(tl!("Paste into Message")));
                app.auto.add("console.chat.context.paste", paste.rect, "Paste into message");
                if paste.clicked() {
                    clipboard_action = Some(("console.chat.paste", json!({})));
                    ui.close();
                }
            });
            menu_open |= response.context_menu_opened();
            labels.push((pos, galley, response));
        }
        // Resolve the menu before painting any labels so Copy can collect a selection
        // spanning several messages, using egui's existing Unicode-aware selection.
        if let Some((id, params)) = clipboard_action {
            action(app, ui.ctx(), id, params);
        }
        for (pos, galley, mut response) in labels {
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, ui.is_enabled(), galley.text()));
            if !ui.is_rect_visible(response.rect) {
                continue;
            }
            // This pinned egui revision starts/extends label selections on *any*
            // pointer button. Keep secondary clicks available to the context menu,
            // but do not pass them to the selection gesture handler.
            if menu_open || ui.input(|i| i.pointer.secondary_down() || i.pointer.secondary_released()) {
                response.sense = egui::Sense::hover();
                response.flags.remove(egui::response::Flags::IS_POINTER_BUTTON_DOWN_ON);
                if menu_open {
                    // Menu presses belong to this selection, rather than being a
                    // click elsewhere that egui should use to clear the highlight.
                    response.flags.insert(egui::response::Flags::HOVERED);
                }
            }
            egui::text_selection::LabelSelectionState::label_text_selection(ui, &response, pos, galley, ui.visuals().text_color(), egui::Stroke::NONE);
        }
    });
}
fn show_voice_controls(app: &mut FilmcraftApp, ui: &mut egui::Ui, capabilities: &Value, connected: bool) {
    let selected_voice_available = app.hooks.codex_connection.as_ref().is_none_or(|c| c.provider_id() != super::provider::ProviderId::Codex)
        || app.session.prefs.general.codex_speaking_voice.is_empty()
        || app.ui.console.chat.voices.contains(&app.session.prefs.general.codex_speaking_voice);
    if !app.ui.console.settings_open {
        if !selected_voice_available {
            if app.ui.console.chat.voice_catalog_pending {
                ui.small("Refreshing speaking voices…");
            } else {
                ui.colored_label(ui.visuals().error_fg_color, "Saved speaking voice is unavailable. Open settings to choose a voice or Use Default.");
            }
        }
        if let Some(error) = &app.ui.console.chat.voice_catalog_error {
            ui.colored_label(ui.visuals().warn_fg_color, error);
        }
    }
    ui.horizontal_wrapped(|ui| {
        let can_start = connected
            && capabilities["voice"].as_bool() == Some(true)
            && selected_voice_available
            && !app.ui.console.chat.busy
            && !app.ui.console.chat.voice.on();
        let on = super::layout::icon_button(app, ui, "console.voice.on", crate::icons::Icon::Mic, can_start, false, tl!("Voice On"));
        if on.clicked() {
            action(app, ui.ctx(), "console.voice.on", json!({}));
        }
        let can_stop = app.ui.console.chat.voice.on();
        let off = super::layout::icon_button(app, ui, "console.voice.off", crate::icons::Icon::Square, can_stop, false, tl!("Voice Off"));
        if off.clicked() {
            action(app, ui.ctx(), "console.voice.off", json!({}));
        }
        let muted = app.ui.console.chat.voice_muted;
        let mute_label = if muted { tl!("Unmute Microphone") } else { tl!("Mute Microphone") };
        let can_mute = connected && app.ui.console.chat.voice == VoiceStatus::Active;
        let mute_icon = if muted { crate::icons::Icon::Mic } else { crate::icons::Icon::MicOff };
        let tip = format!("{mute_label} — AI speech remains audible");
        let mute = super::layout::icon_button(app, ui, "console.voice.mute", mute_icon, can_mute, muted, &tip);
        if mute.clicked() {
            action(app, ui.ctx(), "console.voice.mute", json!({"muted":!muted}));
        }
        if muted {
            ui.strong(tl!("Microphone muted — AI speech remains audible"));
        }
        match &app.ui.console.chat.voice {
            VoiceStatus::Off => {
                ui.label(tl!("Voice off"));
            }
            VoiceStatus::Connecting => {
                ui.spinner();
                ui.label(tl!("Connecting Codex voice…"));
            }
            VoiceStatus::Active => {
                ui.strong(if muted { tl!("Voice on — microphone muted") } else { tl!("Voice on — microphone active") });
            }
            VoiceStatus::Stopping => {
                ui.label(tl!("Stopping voice…"));
            }
            VoiceStatus::Failed(error) => {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
        }
    });
    if app.ui.console.chat.voice.on() {
        if let Some(timing) = &app.ui.console.chat.voice_timing {
            let elapsed = timing.elapsed_seconds;
            let remaining = timing.renew_after_seconds.saturating_sub(elapsed);
            ui.small(format!("Voice {:02}:{:02} · renewal in {:02}:{:02}", elapsed / 60, elapsed % 60, remaining / 60, remaining % 60));
            if timing.renewing {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Renewing voice in the same conversation. Your microphone setting is preserved; Stop cancels renewal.",
                );
            } else if remaining <= 60 {
                ui.colored_label(ui.visuals().warn_fg_color, "Voice will briefly reconnect at 55 minutes. A pending voice task may be cancelled; inspect before repeating edits. Your conversation and mute setting are preserved.");
            }
        }
        ui.label(tl!("Closing the console stops voice. Requested edits can be undone."));
    }
    if app.ui.console.chat.voice.on() {
        let stats = &app.ui.console.chat.voice_input;
        if !stats.device.is_empty() {
            ui.label(format!("{} · Input {} / {}", stats.device, stats.channel, stats.channels));
        }
        ui.add(egui::ProgressBar::new(stats.input_peak).text(tl!("Microphone level")));
        ui.add(egui::ProgressBar::new(if app.ui.console.chat.voice_muted { 0.0 } else { stats.transmit_peak }).text(tl!("Audio sent to Codex")));
        if stats.gated && !app.ui.console.chat.voice_muted {
            ui.label(tl!("Microphone paused during the spoken reply."));
        }
    }
}
pub(super) fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    app.ui.console.references.drop_rect = Some(ui.max_rect());
    let capabilities = app.hooks.codex_connection.as_ref().map(|connection| connection.provider_capabilities()).unwrap_or_else(|| json!({}));
    let label = app.hooks.codex_connection.as_ref().map(|connection| connection.provider_id().label()).unwrap_or("AI");
    let embedded = capabilities["embeddedComposer"].as_bool() != Some(false);
    if embedded {
        ui.label(format!("Ask {label} about this project or request an edit."));
    } else {
        ui.label("Continue your conversation in the ChatGPT desktop app. FilmCraft receives its project commands here.");
    }
    ui.label(if app.ui.console.chat.pending.is_some() {
        tl!("Edit status: awaiting your review")
    } else {
        tl!("Editing is direct. Use Undo to reverse edits.")
    });
    let connected = app.ui.console.connection == ConnectionStatus::Ready;
    if capabilities["voice"].as_bool() == Some(true) && !app.ui.console.chat.voice.on() {
        app.ui.console.voice_channels = app.session.voiceover.input.as_ref().map(|input| input.channels("")).unwrap_or(1).min(256);
        if let Some(error) = app.session.voiceover.input.as_ref().and_then(|input| input.discovery_error()) {
            ui.colored_label(ui.visuals().error_fg_color, error);
        } else if app.ui.console.voice_channels == 0 {
            ui.label(tl!("Detecting microphone input channels…"));
        }
        // Refresh inexpensive cached metadata, including after a device change or hotplug.
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
    }
    if app.ui.console.settings_open && !embedded {
        ui.small("Choose your model, speaking voice and microphone in ChatGPT.");
    }
    if app.ui.console.settings_open && embedded {
        egui::Frame::group(ui.style()).inner_margin(12).show(ui, |ui| {
            let default_model_label = match app.hooks.codex_connection.as_ref().map(|connection| connection.provider_id()).unwrap_or_default() {
                super::provider::ProviderId::Codex => tl!("Codex default"),
                super::provider::ProviderId::OpenAiApi => "API default",
                super::provider::ProviderId::DesktopChatGpt => "Choose in ChatGPT",
            };
            let mut model_choice = None;
            ui.add_enabled_ui(connected && embedded && !app.ui.console.chat.busy && !app.ui.console.chat.voice.on(), |ui| {
                let model_label = ui.label(tl!("Model"));
                let label = app
                    .ui
                    .console
                    .chat
                    .models
                    .iter()
                    .find(|m| Some(&m.id) == app.ui.console.chat.selected_model.as_ref())
                    .map(|m| m.label.clone())
                    .unwrap_or_else(|| default_model_label.into());
                let cb = craft_ui::ComboBox::from_id_salt("console-model").selected_text(label).width(240.0).show_ui(ui, |ui| {
                    let r = ui.selectable_label(app.ui.console.chat.selected_model.is_none(), default_model_label);
                    app.auto.add("console.model.default", r.rect, default_model_label);
                    if r.clicked() {
                        model_choice = Some(None);
                    }
                    for model in &app.ui.console.chat.models {
                        let r = ui.selectable_label(app.ui.console.chat.selected_model.as_ref() == Some(&model.id), &model.label);
                        app.auto.add(&format!("console.model.option.{}", model.id), r.rect, &model.label);
                        if r.clicked() {
                            model_choice = Some(Some(model.id.clone()));
                        }
                    }
                });
                let response = cb.response.labelled_by(model_label.id);
                app.auto.add("console.model", response.rect, "Model");
            });
            if let Some(choice) = model_choice {
                action(app, ui.ctx(), "console.model.set", json!({"model":choice}));
            }
            if capabilities["voice"].as_bool() == Some(true)
                && app.hooks.codex_connection.as_ref().is_some_and(|c| c.provider_id() == super::provider::ProviderId::Codex)
            {
                let selected = &app.session.prefs.general.codex_speaking_voice;
                let unavailable = !app.ui.console.chat.voice_catalog_pending && !selected.is_empty() && !app.ui.console.chat.voices.contains(selected);
                let text = if selected.is_empty() {
                    "Use Default".to_owned()
                } else if unavailable {
                    format!("{selected} (unavailable)")
                } else {
                    selected.clone()
                };
                let mut choice = None;
                ui.add_enabled_ui(connected && !app.ui.console.chat.busy && !app.ui.console.chat.voice.on(), |ui| {
                    let label = ui.label("Speaking voice");
                    let combo = craft_ui::ComboBox::from_id_salt("console-speaking-voice").selected_text(text).width(240.0).show_ui(ui, |ui| {
                        let default = ui.selectable_label(selected.is_empty(), "Use Default");
                        app.auto.add("console.voice.choice.default", default.rect, "Use Default");
                        if default.clicked() {
                            choice = Some(None);
                        }
                        for voice in &app.ui.console.chat.voices {
                            let option = ui.selectable_label(selected == voice, voice);
                            app.auto.add(&format!("console.voice.choice.option.{voice}"), option.rect, voice);
                            if option.clicked() {
                                choice = Some(Some(voice.clone()));
                            }
                        }
                    });
                    app.auto.add("console.voice.choice", combo.response.labelled_by(label.id).rect, "Speaking voice");
                });
                if let Some(voice) = choice {
                    action(app, ui.ctx(), "console.voice.choice.set", json!({"voice":voice}));
                }
                if unavailable {
                    ui.colored_label(ui.visuals().error_fg_color, "Saved speaking voice is unavailable. Choose an available voice or Use Default.");
                }
                if app.ui.console.chat.voice_catalog_pending {
                    ui.small("Refreshing speaking voices…");
                }
                if let Some(error) = &app.ui.console.chat.voice_catalog_error {
                    ui.small(error);
                }
            }
            // Share one row so the microphone mode does not push the message controls
            // below the initial Console viewport. Narrow windows can wrap the columns.
            let codex_voice = capabilities["voice"].as_bool() == Some(true)
                && app.hooks.codex_connection.as_ref().is_some_and(|c| c.provider_id() == super::provider::ProviderId::Codex);
            ui.horizontal_wrapped(|ui| {
                ui.allocate_ui_with_layout(egui::vec2(210.0, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.add_enabled_ui(
                        connected && capabilities["voice"].as_bool() == Some(true) && !app.ui.console.chat.busy && !app.ui.console.chat.voice.on(),
                        |ui| {
                            let label = ui.label(tl!("Microphone input channel"));
                            let mut choice = None;
                            let selected = format!("Input {}", app.ui.console.voice_channel.saturating_add(1));
                            let combo = craft_ui::ComboBox::from_id_salt("console-voice-channel").selected_text(selected).width(150.0).show_ui(ui, |ui| {
                                for channel in 0..app.ui.console.voice_channels {
                                    let text = format!("Input {}", channel.saturating_add(1));
                                    let r = ui.selectable_label(app.ui.console.voice_channel == channel, &text);
                                    app.auto.add(&format!("console.voice.channel.{}", channel.saturating_add(1)), r.rect, &text);
                                    if r.clicked() {
                                        choice = Some(channel.saturating_add(1));
                                    }
                                }
                            });
                            let response = combo.response.labelled_by(label.id);
                            app.auto.add("console.voice.channel", response.rect, "Microphone input channel");
                            if let Some(channel) = choice {
                                action(app, ui.ctx(), "console.voice.channel.set", json!({"channel":channel}));
                            }
                        },
                    );
                });
                if codex_voice {
                    ui.allocate_ui_with_layout(egui::vec2(ui.available_width().min(310.0), 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                        let speakers = app.session.prefs.general.codex_voice_echo_reduction;
                        let mut choice = None;
                        ui.add_enabled_ui(!app.ui.console.chat.busy && !app.ui.console.chat.voice.on(), |ui| {
                            let label = ui.label("Microphone mode");
                            let combo = craft_ui::ComboBox::from_id_salt("console-voice-input-mode")
                                .selected_text(if speakers { "Speakers — echo reduction" } else { "Headphones — can interrupt" })
                                .width(ui.available_width().min(290.0))
                                .show_ui(ui, |ui| {
                                    for (mode, text, selected) in [
                                        ("headphones", "Headphones — can interrupt", !speakers),
                                        ("speakers", "Speakers — echo reduction; wait to reply", speakers),
                                    ] {
                                        let response = ui.selectable_label(selected, text);
                                        app.auto.add(&format!("console.voice.inputMode.{mode}"), response.rect, text);
                                        if response.clicked() {
                                            choice = Some(mode);
                                        }
                                    }
                                });
                            app.auto.add("console.voice.inputMode", combo.response.labelled_by(label.id).rect, "Microphone mode");
                        });
                        if let Some(mode) = choice {
                            action(app, ui.ctx(), "console.voice.inputMode.set", json!({"mode":mode}));
                        }
                    });
                }
            });
        });
    }
    show_history(app, ui, embedded);
    super::references::ui(app, ui);
    if embedded {
        let input_id = ui.make_persistent_id("console.chat.input");
        let return_pressed = draft_return(ui.ctx(), input_id, &mut app.ui.console.chat.input_composing);
        let input = ui.add(
            egui::TextEdit::multiline(&mut app.ui.console.chat.prompt)
                .id(input_id)
                .return_key(egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, egui::Key::Enter))
                .hint_text(format!("Message to {label}"))
                .desired_rows(2)
                .char_limit(MAX_LINE)
                .desired_width(f32::INFINITY),
        );
        app.auto.add("console.chat.input", input.rect, &format!("Message to {label}"));
        if std::mem::take(&mut app.ui.console.chat.paste_requested) {
            input.request_focus();
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::RequestPaste);
        }
        ui.small(tl!("Return sends · Shift+Return adds a line"));
        ui.horizontal(|ui| {
            let send = ui.add_enabled(
                connected && embedded && !app.ui.console.chat.busy && !app.ui.console.chat.voice.on() && !app.ui.console.chat.prompt.trim().is_empty(),
                egui::Button::new(tl!("Send")),
            );
            app.auto.add("console.chat.send", send.rect, &format!("Send to {label}"));
            if send.clicked() || (return_pressed && send.enabled()) {
                let prompt = app.ui.console.chat.prompt.clone();
                action(app, ui.ctx(), "console.chat.send", json!({"message":prompt}));
            }
            let busy = app.ui.console.chat.busy;
            let cancel = super::layout::icon_button(app, ui, "console.chat.cancel", crate::icons::Icon::Square, busy, false, tl!("Cancel active task"));
            if cancel.clicked() {
                action(app, ui.ctx(), "console.chat.cancel", json!({}));
            }
            if app.ui.console.chat.busy {
                ui.spinner();
            }
        });
    } else {
        ui.label("Use ChatGPT's message box to ask about this project or request an edit.");
    }
    if embedded {
        show_voice_controls(app, ui, &capabilities, connected);
    }
    if let Some(pending) = app.ui.console.chat.pending.as_ref() {
        ui.separator();
        ui.strong(tl!("Review proposed edit"));
        let details = egui::ScrollArea::vertical().id_salt("console.chat.review.details").max_height(180.0).show(ui, |ui| {
            ui.monospace(&pending.command);
            ui.label(preview(&pending.params));
            ui.label(tl!("Validated on a project copy. Apply creates a normal Undo step."));
            ui.monospace(&pending.result);
        });
        app.auto.add("console.chat.review.details", details.inner_rect, "Proposed edit details");
        let mut decision = None;
        ui.horizontal(|ui| {
            let approve = ui.button(tl!("Apply proposed edit"));
            app.auto.add("console.chat.approve", approve.rect, "Approve proposed edit");
            if approve.clicked() {
                decision = Some(true);
            }
            let reject = ui.button(tl!("Reject proposed edit"));
            app.auto.add("console.chat.reject", reject.rect, "Reject proposed edit");
            if reject.clicked() {
                decision = Some(false);
            }
        });
        if let Some(approve) = decision {
            action(app, ui.ctx(), if approve { "console.chat.approve" } else { "console.chat.reject" }, json!({}));
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn output_frame(app: &mut FilmcraftApp, ctx: &egui::Context, events: Vec<egui::Event>) -> egui::FullOutput {
        app.auto.begin_frame();
        let mut output = ctx.run_ui(
            egui::RawInput {
                focused: true,
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1200.0))),
                events,
                ..Default::default()
            },
            |ui| show(app, ui),
        );
        output.textures_delta.clear();
        output
    }

    fn pointer_button(pos: egui::Pos2, button: egui::PointerButton, pressed: bool) -> egui::Event {
        egui::Event::PointerButton { pos, button, pressed, modifiers: egui::Modifiers::NONE }
    }

    fn selected_text_vertices(output: &egui::FullOutput, color: egui::Color32) -> usize {
        fn count(shape: &egui::Shape, color: egui::Color32) -> usize {
            match shape {
                egui::Shape::Text(text) => {
                    text.galley.rows.iter().map(|row| row.visuals.mesh.vertices.iter().filter(|vertex| vertex.color == color).count()).sum()
                }
                egui::Shape::Vec(shapes) => shapes.iter().map(|shape| count(shape, color)).sum(),
                _ => 0,
            }
        }
        output.shapes.iter().map(|shape| count(&shape.shape, color)).sum()
    }

    fn assert_output_context_copy(messages: &[&str], selection: Option<((usize, usize), (usize, usize))>, click: (usize, usize), expected: &str) {
        let ctx = egui::Context::default();
        let mut app = app();
        for text in messages {
            app.ui.console.chat.messages.push_back(("Codex".into(), (*text).into()));
        }
        app.ui.console.chat.prompt = "Keep this draft".into();
        let before = app.session.project.clone();
        output_frame(&mut app, &ctx, vec![]);
        output_frame(&mut app, &ctx, vec![]);
        let at = |message: usize, index: usize| {
            let [x, y, _, _] = app.auto.find(&format!("console.chat.message.{message}")).unwrap().rect;
            let font = egui::TextStyle::Body.resolve(&ctx.global_style());
            let galley = ctx.fonts_mut(|fonts| fonts.layout_no_wrap(messages[message].into(), font, egui::Color32::WHITE));
            egui::pos2(x, y) + galley.pos_from_cursor(egui::text::CCursor::new(index)).center().to_vec2()
        };
        let positions = selection.map(|(start, end)| (at(start.0, start.1), at(end.0, end.1)));
        let right_click = at(click.0, click.1);
        let color = ctx.global_style().visuals.selection.bg_fill;
        if let Some((start, end)) = positions {
            output_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(start), pointer_button(start, egui::PointerButton::Primary, true)]);
            output_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(end)]);
            let selected = output_frame(&mut app, &ctx, vec![pointer_button(end, egui::PointerButton::Primary, false)]);
            assert!(selected_text_vertices(&selected, color) > 0, "test must first create a visible text highlight");
        }
        output_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(right_click), pointer_button(right_click, egui::PointerButton::Secondary, true)]);
        let opened = output_frame(&mut app, &ctx, vec![pointer_button(right_click, egui::PointerButton::Secondary, false)]);
        if selection.is_some() {
            assert!(selected_text_vertices(&opened, color) > 0, "opening the context menu must retain the highlight");
        }
        // Popup areas finish their initial sizing before their buttons become interactive.
        output_frame(&mut app, &ctx, vec![]);
        output_frame(&mut app, &ctx, vec![]);
        let [x, y, w, h] = app.auto.find("console.chat.context.copy").unwrap().rect;
        let copy = egui::pos2(x + w / 2.0, y + h / 2.0);
        let pressed = output_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(copy), pointer_button(copy, egui::PointerButton::Primary, true)]);
        if selection.is_some() {
            assert!(selected_text_vertices(&pressed, color) > 0, "pressing Copy must retain the highlight");
        }
        let copied = output_frame(&mut app, &ctx, vec![pointer_button(copy, egui::PointerButton::Primary, false)]);
        let text = copied.platform_output.commands.iter().find_map(
            |command| {
                if let egui::OutputCommand::CopyText(text) = command { Some(text.as_str()) } else { None }
            },
        );
        assert_eq!(text, Some(expected), "status: {}; output: {:?}", app.ui.status, copied.platform_output.commands);
        assert_eq!(app.ui.console.chat.prompt, "Keep this draft");
        assert!(!app.ui.console.chat.busy);
        assert_eq!(*app.session.project, *before);
    }

    #[test]
    fn right_click_preserves_output_highlight_and_copies_only_selected_text() {
        assert_output_context_copy(&["alpha beta gamma"], Some(((0, 6), (0, 10))), (0, 7), "beta");
    }
    #[test]
    fn output_context_copy_preserves_unicode_selection() {
        assert_output_context_copy(&["one café 🎬 two"], Some(((0, 4), (0, 10))), (0, 5), "café 🎬");
    }
    #[test]
    fn output_context_copy_preserves_selection_across_messages() {
        assert_output_context_copy(&["alpha beta gamma", "second response"], Some(((0, 6), (1, 6))), (1, 3), "beta gamma\nCodex\nsecond");
    }
    #[test]
    fn output_context_copy_without_selection_copies_the_whole_message() {
        assert_output_context_copy(&["alpha beta gamma"], None, (0, 7), "alpha beta gamma");
    }

    fn draft_frame(ctx: &egui::Context, text: &mut String, composing: &mut bool, events: Vec<egui::Event>) -> bool {
        let id = egui::Id::unique("draft-return-test");
        let mut submit = false;
        let mut output = ctx.run_ui(egui::RawInput { focused: true, events, ..Default::default() }, |ui| {
            let ctx = ui.ctx().clone();
            ctx.memory_mut(|m| m.request_focus(id));
            submit = draft_return(&ctx, id, composing);
            ui.add(egui::TextEdit::multiline(text).id(id).return_key(egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, egui::Key::Enter)));
        });
        output.textures_delta.clear();
        submit
    }
    fn enter(shift: bool, repeat: bool) -> egui::Event {
        egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat,
            modifiers: if shift { egui::Modifiers::SHIFT } else { egui::Modifiers::NONE },
        }
    }
    fn release_enter() -> egui::Event {
        let mut event = enter(false, false);
        if let egui::Event::Key { pressed, .. } = &mut event {
            *pressed = false;
        }
        event
    }
    #[test]
    fn draft_return_submits_but_shift_return_inserts_a_newline() {
        for shift in [false, true] {
            let ctx = egui::Context::default();
            let mut text = "hello".to_string();
            let mut composing = false;
            draft_frame(&ctx, &mut text, &mut composing, vec![]);
            let submit = draft_frame(&ctx, &mut text, &mut composing, vec![enter(shift, false)]);
            assert_eq!(submit, !shift);
            assert_eq!(text.chars().filter(|c| *c == '\n').count(), usize::from(shift));
        }
    }
    #[test]
    fn draft_return_never_sends_ime_composition_or_key_repeats() {
        let ctx = egui::Context::default();
        let mut text = "draft".to_string();
        let mut composing = false;
        draft_frame(&ctx, &mut text, &mut composing, vec![]);
        assert!(draft_frame(&ctx, &mut text, &mut composing, vec![enter(false, false)]));
        assert!(!draft_frame(&ctx, &mut text, &mut composing, vec![enter(false, true)]));
        assert!(!draft_frame(&ctx, &mut text, &mut composing, vec![release_enter()]));
        assert!(!draft_frame(
            &ctx,
            &mut text,
            &mut composing,
            vec![egui::Event::Ime(egui::ImeEvent::Preedit { text: "候補".into(), active_range_chars: None })]
        ));
        assert!(composing);
        assert!(!draft_frame(&ctx, &mut text, &mut composing, vec![enter(false, false)]));
        assert!(!draft_frame(&ctx, &mut text, &mut composing, vec![release_enter()]));
        assert!(!draft_frame(&ctx, &mut text, &mut composing, vec![egui::Event::Ime(egui::ImeEvent::Commit("確定".into())), enter(false, false)]));
        assert!(!composing);
        assert!(!text.contains('\n'));
    }
    #[test]
    fn draft_return_recovers_after_cancelled_ime_composition() {
        let ctx = egui::Context::default();
        let mut text = "draft".to_string();
        let mut composing = false;
        draft_frame(&ctx, &mut text, &mut composing, vec![]);
        assert!(!draft_frame(
            &ctx,
            &mut text,
            &mut composing,
            vec![egui::Event::Ime(egui::ImeEvent::Preedit { text: "候補".into(), active_range_chars: None })]
        ));
        assert!(composing);
        assert!(!draft_frame(
            &ctx,
            &mut text,
            &mut composing,
            vec![egui::Event::Ime(egui::ImeEvent::Preedit { text: String::new(), active_range_chars: None })]
        ));
        assert!(!composing, "Cancelling the input method must not leave Return blocked until focus changes");
        assert!(draft_frame(&ctx, &mut text, &mut composing, vec![enter(false, false)]));
    }
    #[test]
    fn draft_return_does_not_steal_return_from_another_field() {
        let ctx = egui::Context::default();
        let mut composing = true;
        let mut output = ctx.run_ui(egui::RawInput { focused: true, events: vec![enter(false, false)], ..Default::default() }, |ui| {
            let id = ui.make_persistent_id("another-field");
            ctx.memory_mut(|m| m.request_focus(id));
            assert!(!draft_return(&ctx, ui.make_persistent_id("console.chat.input"), &mut composing));
            assert!(!composing, "leaving the draft must clear its composition state");
            assert!(ctx.input(|input| input.key_pressed(egui::Key::Enter)), "the focused field still owns Return");
        });
        output.textures_delta.clear();
    }
    #[test]
    fn draft_return_uses_send_guards_and_preserves_failed_drafts() {
        use std::sync::Mutex;
        struct Sender {
            calls: Arc<Mutex<usize>>,
            fail: bool,
        }
        impl CodexConnection for Sender {
            fn connect(&mut self) -> Result<(), String> {
                Ok(())
            }
            fn poll(&mut self) -> Option<ConnectionStatus> {
                None
            }
            fn disconnect(&mut self) {}
            fn send_prompt(&mut self, _prompt: String) -> Result<(), String> {
                *self.calls.lock().unwrap() += 1;
                if self.fail { Err("Synthetic send failure".into()) } else { Ok(()) }
            }
        }
        for mode in ["ready", "disconnected", "busy", "voice", "error", "empty"] {
            let calls = Arc::new(Mutex::new(0));
            let mut app = app();
            app.ui.console.open = true;
            app.ui.console.connection = if mode == "disconnected" { ConnectionStatus::Disconnected } else { ConnectionStatus::Ready };
            app.ui.console.chat.prompt = if mode == "empty" { "   " } else { "Keep this draft" }.into();
            app.ui.console.chat.busy = mode == "busy";
            if mode == "voice" {
                app.ui.console.chat.voice = VoiceStatus::Active;
            }
            app.hooks.codex_connection = Some(Box::new(Sender { calls: calls.clone(), fail: mode == "error" }));
            let before = app.session.project.clone();
            let ctx = egui::Context::default();
            for events in [vec![], vec![enter(false, false)], vec![enter(false, true)]] {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        focused: true,
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1200.0))),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        ui.ctx().memory_mut(|m| m.request_focus(ui.make_persistent_id("console.chat.input")));
                        show(&mut app, ui);
                    },
                );
                output.textures_delta.clear();
            }
            assert_eq!(*calls.lock().unwrap(), usize::from(matches!(mode, "ready" | "error")), "{mode}");
            assert_eq!(
                app.ui.console.chat.prompt,
                if mode == "ready" {
                    ""
                } else if mode == "empty" {
                    "   "
                } else {
                    "Keep this draft"
                },
                "{mode}"
            );
            if mode == "error" {
                assert!(app.ui.status.contains("Synthetic send failure"));
            }
            assert_eq!(*app.session.project, *before, "{mode}: sending must not edit the project");
        }
    }
    #[test]
    fn unavailable_voice_warning_remains_visible_with_settings_closed() {
        struct Host;
        impl CodexConnection for Host {
            fn connect(&mut self) -> Result<(), String> {
                Err("No connection in this rendering test".into())
            }
            fn poll(&mut self) -> Option<ConnectionStatus> {
                None
            }
            fn disconnect(&mut self) {}
        }
        let mut app = FilmcraftApp::new(Session::default());
        app.hooks.codex_connection = Some(Box::new(Host));
        app.session.prefs.general.codex_speaking_voice = "no-longer-available".into();
        assert!(!app.ui.console.settings_open);
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            show_voice_controls(&mut app, ui, &json!({"voice":true}), true);
        });
        output.textures_delta.clear();
        fn warning_is_painted(shape: &egui::Shape) -> bool {
            match shape {
                egui::Shape::Text(text) => text.galley.text().contains("Saved speaking voice is unavailable"),
                egui::Shape::Vec(shapes) => shapes.iter().any(warning_is_painted),
                _ => false,
            }
        }
        assert!(output.shapes.iter().any(|shape| warning_is_painted(&shape.shape)), "The warning must be painted outside collapsed settings");
    }
    #[test]
    fn voice_timer_and_renewal_warning_are_visible_without_opening_settings() {
        let mut app = FilmcraftApp::new(Session::default());
        app.ui.console.chat.voice = VoiceStatus::Active;
        app.ui.console.chat.voice_muted = true;
        app.ui.console.chat.voice_timing = Some(VoiceTiming { elapsed_seconds: 3245, renew_after_seconds: 3300, renewing: false });
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| show_voice_controls(&mut app, ui, &json!({"voice":true}), true));
        output.textures_delta.clear();
        fn texts(shape: &egui::Shape, found: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => found.push(text.galley.text().into()),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| texts(shape, found)),
                _ => {}
            }
        }
        let mut painted = Vec::new();
        for shape in &output.shapes {
            texts(&shape.shape, &mut painted);
        }
        assert!(painted.iter().any(|text| text.contains("Voice 54:05") && text.contains("00:55")));
        assert!(painted.iter().any(|text| text.contains("55 minutes") && text.contains("preserved")));
        assert!(app.ui.console.chat.voice_muted);
        assert!(!app.ui.console.settings_open);
    }
    fn app() -> FilmcraftApp {
        let mut app = FilmcraftApp::new(Session::default());
        app.session.execute("file.newSequence", json!({"width":16,"height":16})).unwrap();
        app
    }
    fn proposal(app: &FilmcraftApp) -> Proposal {
        Proposal::prepare(&app.session, json!("call-1"), "markers.add".into(), json!({"target":"program","time":0,"name":"Verse"})).unwrap()
    }
    #[test]
    fn resume_inspection_does_not_append_to_the_saved_assistant_message() {
        struct Events(VecDeque<ChatEvent>);
        impl CodexConnection for Events {
            fn connect(&mut self) -> Result<(), String> {
                Ok(())
            }
            fn poll(&mut self) -> Option<ConnectionStatus> {
                None
            }
            fn disconnect(&mut self) {}
            fn poll_chat(&mut self) -> Option<ChatEvent> {
                self.0.pop_front()
            }
        }
        let mut app = app();
        app.ui.console.connection = ConnectionStatus::Ready;
        app.ui.console.chat.busy = true;
        message(&mut app.ui.console.chat, "Codex", "Saved response.".into());
        app.hooks.codex_connection = Some(Box::new(Events(VecDeque::from([
            ChatEvent::SessionStarted("saved-thread".into()),
            ChatEvent::Delta("Fresh inspection.".into()),
            ChatEvent::Finished,
        ]))));
        poll(&mut app, &egui::Context::default());
        assert_eq!(app.ui.console.chat.messages.front().unwrap().1, "Saved response.");
        assert_eq!(app.ui.console.chat.messages.back().unwrap().1, "Fresh inspection.");
        assert_eq!(app.ui.console.chat.messages.len(), 3);
    }
    #[test]
    fn reconnect_notice_preserves_busy_voice_and_project_until_turn_completes() {
        struct Events(VecDeque<ChatEvent>);
        impl CodexConnection for Events {
            fn connect(&mut self) -> Result<(), String> {
                Ok(())
            }
            fn poll(&mut self) -> Option<ConnectionStatus> {
                None
            }
            fn disconnect(&mut self) {}
            fn poll_chat(&mut self) -> Option<ChatEvent> {
                self.0.pop_front()
            }
        }
        let mut app = app();
        let project = Arc::clone(&app.session.project);
        app.ui.console.connection = ConnectionStatus::Ready;
        app.ui.console.chat.busy = true;
        app.ui.console.chat.voice = VoiceStatus::Active;
        app.hooks.codex_connection = Some(Box::new(Events(VecDeque::from([ChatEvent::Retrying("Reconnecting... 2/5".into())]))));
        poll(&mut app, &egui::Context::default());
        assert!(app.ui.console.chat.busy);
        assert_eq!(app.ui.console.connection, ConnectionStatus::Ready);
        assert_eq!(app.ui.console.chat.voice, VoiceStatus::Active);
        assert_eq!(app.ui.console.chat.messages.back().unwrap().1, "Reconnecting... 2/5");
        assert_eq!(app.session.project, project);
    }

    #[test]
    fn preview_never_edits_and_approved_edit_uses_undo() {
        let mut app = app();
        let before = Arc::clone(&app.session.project);
        let p = proposal(&app);
        assert_eq!(*before, *app.session.project);
        p.apply(&mut app, &egui::Context::default()).unwrap();
        assert_ne!(*before, *app.session.project);
        app.session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(*before, *app.session.project);
    }
    #[test]
    fn stale_and_hostile_proposals_do_not_edit() {
        let mut app = app();
        for command in ["file.save", "console.run", "edit.undo", "no.such"] {
            assert!(Proposal::prepare(&app.session, json!(1), command.into(), json!({})).is_err());
        }
        for params in [json!([]), json!({"target":"source"}), json!({"target":"program","time":-1e300})] {
            assert!(Proposal::prepare(&app.session, json!(1), "markers.add".into(), params).is_err());
        }
        let p = proposal(&app);
        app.session.state.source_playhead.0 += 1;
        let before = Arc::clone(&app.session.project);
        assert!(p.apply(&mut app, &egui::Context::default()).is_err());
        assert_eq!(*before, *app.session.project);
        let mut p = proposal(&app);
        p.created = Instant::now() - Duration::from_secs(301);
        assert!(p.apply(&mut app, &egui::Context::default()).is_err());
        let p = proposal(&app);
        app.session.revision += 1;
        assert!(p.apply(&mut app, &egui::Context::default()).is_err());
    }
    #[test]
    fn pending_input_discovery_does_not_cache_a_fake_single_channel() {
        use filmcraft_engine::voiceover::{AudioInput, InputFormat};
        use std::sync::{
            Arc,
            atomic::{AtomicU16, Ordering},
        };
        struct Pending(Arc<AtomicU16>);
        impl AudioInput for Pending {
            fn devices(&self) -> Vec<String> {
                Vec::new()
            }
            fn channels(&self, _: &str) -> u16 {
                self.0.load(Ordering::Acquire)
            }
            fn start(&mut self, _: &str, _: u32) -> std::result::Result<InputFormat, String> {
                Err("not used".into())
            }
            fn read(&mut self, _: usize) -> Vec<Vec<f32>> {
                Vec::new()
            }
            fn discard(&mut self) {}
            fn stop(&mut self) {}
        }
        struct VoiceProvider;
        impl CodexConnection for VoiceProvider {
            fn connect(&mut self) -> Result<(), String> {
                Err("Synthetic provider is never connected".into())
            }
            fn poll(&mut self) -> Option<ConnectionStatus> {
                None
            }
            fn disconnect(&mut self) {}
        }
        let channels = Arc::new(AtomicU16::new(0));
        let mut app = app();
        app.hooks.codex_connection = Some(Box::new(VoiceProvider));
        app.session.voiceover.input = Some(Box::new(Pending(channels.clone())));
        let ctx = egui::Context::default();
        let project = app.session.project.clone();
        output_frame(&mut app, &ctx, vec![]);
        assert_eq!(app.ui.console.voice_channels, 0);
        channels.store(8, Ordering::Release);
        output_frame(&mut app, &ctx, vec![]);
        assert_eq!(app.ui.console.voice_channels, 8);
        channels.store(0, Ordering::Release);
        output_frame(&mut app, &ctx, vec![]);
        assert_eq!(app.ui.console.voice_channels, 0);
        app.ui.console.voice_channels = 6;
        app.ui.console.chat.voice = VoiceStatus::Active;
        output_frame(&mut app, &ctx, vec![]);
        assert_eq!(app.ui.console.voice_channels, 6);
        assert_eq!(*app.session.project, *project);
    }
    #[test]
    fn provider_without_voice_does_not_query_input_devices() {
        use filmcraft_engine::voiceover::{AudioInput, InputFormat};
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct NoVoice;
        impl CodexConnection for NoVoice {
            fn provider_capabilities(&self) -> Value {
                json!({"embeddedComposer":true,"voice":false})
            }
            fn connect(&mut self) -> Result<(), String> {
                Err("Synthetic provider is never connected".into())
            }
            fn poll(&mut self) -> Option<ConnectionStatus> {
                None
            }
            fn disconnect(&mut self) {}
        }
        struct InputProbe(Arc<AtomicUsize>);
        impl AudioInput for InputProbe {
            fn devices(&self) -> Vec<String> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Vec::new()
            }
            fn channels(&self, _: &str) -> u16 {
                self.0.fetch_add(1, Ordering::SeqCst);
                0
            }
            fn start(&mut self, _: &str, _: u32) -> Result<InputFormat, String> {
                Err("Not used".into())
            }
            fn read(&mut self, _: usize) -> Vec<Vec<f32>> {
                Vec::new()
            }
            fn discard(&mut self) {}
            fn stop(&mut self) {}
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let mut app = app();
        app.hooks.codex_connection = Some(Box::new(NoVoice));
        app.session.voiceover.input = Some(Box::new(InputProbe(calls.clone())));
        app.ui.console.connection = ConnectionStatus::Ready;
        let project = app.session.project.clone();
        output_frame(&mut app, &egui::Context::default(), vec![]);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(*app.session.project, *project);
    }
}

#[cfg(test)]
mod voice_catalog_tests {
    use super::*;
    struct Events(VecDeque<ChatEvent>);
    impl CodexConnection for Events {
        fn connect(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn poll(&mut self) -> Option<ConnectionStatus> {
            None
        }
        fn disconnect(&mut self) {}
        fn poll_chat(&mut self) -> Option<ChatEvent> {
            self.0.pop_front()
        }
    }
    fn event(app: &mut FilmcraftApp, event: ChatEvent) {
        app.hooks.codex_connection = Some(Box::new(Events(VecDeque::from([event]))));
        poll(app, &egui::Context::default());
    }
    #[test]
    fn reconnect_catalog_clears_stale_choices_without_losing_saved_preference() {
        let mut app = FilmcraftApp::new(Session::default());
        app.ui.console.chat.voices = vec!["old".into()];
        app.session.prefs.general.codex_speaking_voice = "sol".into();
        let project = app.session.project.clone();
        let identity = (app.session.revision, app.session.document_epoch);
        event(&mut app, ChatEvent::VoiceChoices { voices: Vec::new(), error: None, loading: true });
        assert!(app.ui.console.chat.voices.is_empty());
        assert!(app.ui.console.chat.voice_catalog_pending);
        assert_eq!(app.session.prefs.general.codex_speaking_voice, "sol");
        event(&mut app, ChatEvent::VoiceChoices { voices: vec!["sol".into(), "cove".into()], error: None, loading: false });
        assert_eq!(app.ui.console.chat.voices, ["sol", "cove"]);
        assert!(!app.ui.console.chat.voice_catalog_pending);
        event(&mut app, ChatEvent::VoiceChoices { voices: vec!["sol".into()], error: Some("private error".repeat(8192)), loading: false });
        assert!(app.ui.console.chat.voices.is_empty());
        assert!(!app.ui.console.chat.voice_catalog_error.as_deref().unwrap().contains("private error"));
        event(&mut app, ChatEvent::VoiceChoices { voices: vec!["sol".into(); 65], error: None, loading: false });
        assert!(app.ui.console.chat.voices.is_empty());
        assert!(app.ui.console.chat.voice_catalog_error.is_some());
        assert_eq!(app.session.prefs.general.codex_speaking_voice, "sol");
        assert!(Arc::ptr_eq(&project, &app.session.project));
        assert_eq!((app.session.revision, app.session.document_epoch), identity);
    }
}
