//! One UI/control-channel command path for conversation and voice controls.
use super::*;
pub(super) fn invoke(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: Value) -> Result<Value, String> {
    let allowed: &[&str] = match id {
        "console.library.inspect" => &["offset", "limit", "expectedGeneration"],
        "console.library.recover" | "console.library.ocrInstall" => &[],
        "console.library.selection" => &["documentIds"],
        "console.library.search" => &["query", "limit"],
        "console.library.illustration" => &["documentId", "resource", "expectedGeneration"],
        "console.library.page" => &["documentId", "page", "maxSide", "expectedGeneration"],
        "console.library.status" | "console.library.cancel" => &["ticket"],
        "console.library.import" => &["path"],
        "console.library.remove" => &["documentId"],
        "console.library.enable" => &["documentId", "enabled"],
        "console.provider.inspect" | "console.api.clear" | "console.settings.toggle" => &[],
        "console.assistant.inspect" | "console.assistant.detach" => &[],
        "console.assistant.memoryHistory" => &[],
        "console.assistant.memoryRestore" => &["original", "scope", "documentEpoch", "memoryGeneration", "confirm"],
        "console.assistant.review" => &["scope", "documentEpoch", "memoryGeneration", "teaching", "action"],
        "console.assistant.history" => &["name", "revision"],
        "console.assistant.restore" => &["name", "revision", "expectedInstructions", "scope", "documentEpoch", "memoryGeneration", "confirm"],
        "console.assistant.autosave" => &["enabled"],
        "console.assistant.save" => &["name", "replace", "expectedInstructions", "expectedReferenceDocuments"],
        "console.assistant.load" => &["name"],
        "console.provider.select" => &["provider"],
        "console.context.inspect" | "console.attachments.inspect" | "console.styles.inspect" | "console.styles.disable" => &[],
        "console.attachments.add" | "console.styles.import" | "console.styles.export" => &["path"],
        "console.attachments.project" => &["item", "start", "end"],
        "console.attachments.pdf.read" => &["snapshotId", "documentEpoch", "page", "offset", "limit"],
        "console.attachments.pdf.status" => &["snapshotId", "documentEpoch", "ticket"],
        "console.attachments.image" => &["snapshotId", "documentEpoch"],
        "console.attachments.remove" => &["index"],
        "console.styles.load" | "console.styles.delete" => &["name"],
        "console.styles.rename" => &["name", "newName"],
        "console.styles.draft" => &["name", "description"],
        "console.styles.save" => &["name", "description", "replace", "expectedDescription"],
        "console.bridge.capabilities" | "console.bridge.interface" => &[],
        "console.bridge.memory" => {
            &["action", "scope", "text", "expectedText", "offset", "limit", "query", "documentEpoch", "memoryGeneration", "historyGeneration"]
        }
        "console.bridge.commands" => &["filter", "enabledOnly", "offset", "limit"],
        "console.bridge.execute" | "console.bridge.prepare" => &["command", "params"],
        "console.bridge.batch" => &["steps", "label"],
        "console.bridge.analyze" => &["kind", "song", "video"],
        "console.bridge.analysis.read" => &["analysisId", "offset"],
        "console.bridge.propose" => &["command", "params"],
        "console.bridge.status" | "console.bridge.cancel" => &["ticket"],
        "console.memory.save" => &["text"],
        "console.session.resume" | "console.session.lock" => &["threadId"],
        "console.session.unlock" => &[],
        "console.session.new" | "console.sessions.open" | "console.sessions.reload" | "console.memory.open" => &[],
        "console.voice.channel.set" => &["channel"],
        "console.voice.choice.set" => &["voice"],
        "console.voice.inputMode.set" => &["mode"],
        "console.voice.mute" => &["muted"],
        "console.model.set" => &["model"],
        "console.chat.send" => &["message", "model"],
        "console.chat.copy" => &["index", "selection"],
        "console.chat.paste" => &[],
        "console.chat.cancel" | "console.chat.approve" | "console.chat.reject" | "console.voice.on" | "console.voice.off" => &[],
        _ => return Err("Unknown console command".into()),
    };
    let object = params.as_object().ok_or("Console command parameters must be an object")?;
    if object.len() > allowed.len() || object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err("Unexpected console command parameter".into());
    }
    if id.starts_with("console.library.") {
        return super::reference_library::invoke(app, id, object);
    }
    if id.starts_with("console.assistant.") {
        return super::assistants::invoke(app, id, object);
    }
    if id == "console.settings.toggle" {
        app.ui.console.settings_open = !app.ui.console.settings_open;
        return Ok(json!({"open":app.ui.console.settings_open}));
    }
    if id == "console.api.clear" {
        super::api_settings::forget(app)?;
        return Ok(json!({"cleared":true}));
    }
    if id == "console.provider.inspect" {
        let connection = app.hooks.codex_connection.as_ref().ok_or("AI connections are unavailable")?;
        return Ok(json!({"provider":connection.provider_id(),"capabilities":connection.provider_capabilities(),"connection":app.ui.console.connection.id()}));
    }
    if id == "console.provider.select" {
        let provider: super::provider::ProviderId =
            serde_json::from_value(object.get("provider").cloned().ok_or("Choose an AI provider")?).map_err(|_| "Unknown AI provider")?;
        if provider == super::provider::ProviderId::OpenAiApi && !super::provider::API_CONNECTIONS_ENABLED {
            return Err(super::provider::API_UNRESOLVED.into());
        }
        if app.hooks.codex_connection.as_ref().ok_or("AI connections are unavailable")?.provider_id() == provider {
            return Ok(json!({"provider":provider,"changed":false}));
        }
        super::sessions::save_chat(app)?;
        super::bridge::cancel_pending(app, "AI provider changed; old requests were cancelled");
        app.hooks.codex_connection.as_mut().ok_or("AI connections are unavailable")?.select_provider(provider)?;
        app.ui.console.connection = ConnectionStatus::Disconnected;
        let draft = std::mem::take(&mut app.ui.console.chat.prompt);
        app.ui.console.chat = super::chat::Chat::default();
        app.ui.console.chat.prompt = draft;
        app.ui.console.sessions.active = None;
        return Ok(json!({"provider":provider,"changed":true,"connected":false}));
    }
    if id == "console.context.inspect" || id.starts_with("console.attachments.") || id.starts_with("console.styles.") {
        return super::references::invoke(app, id, object);
    }
    if id.starts_with("console.bridge.") {
        return super::bridge::invoke(app, ctx, id, object);
    }
    if id.starts_with("console.session") || id.starts_with("console.memory.") {
        return super::sessions::invoke(app, id, object);
    }
    match id {
        "console.voice.mute" => {
            if app.ui.console.connection != ConnectionStatus::Ready || app.ui.console.chat.voice != VoiceStatus::Active {
                return Err("Connect voice before muting the microphone".into());
            }
            let muted = match object.get("muted") {
                None => !app.ui.console.chat.voice_muted,
                Some(Value::Bool(value)) => *value,
                _ => return Err("muted must be boolean".into()),
            };
            app.hooks.codex_connection.as_mut().ok_or("Codex is unavailable")?.set_voice_muted(muted)?;
            app.ui.console.chat.voice_muted = muted;
            Ok(json!({"microphoneMuted":muted,"speakerMuted":false}))
        }
        "console.voice.inputMode.set" => {
            if app.ui.console.chat.voice.on() || app.ui.console.chat.busy {
                return Err("Turn voice off and finish the current turn before changing microphone mode".into());
            }
            let connection = app.hooks.codex_connection.as_ref().ok_or("AI connection unavailable")?;
            if connection.provider_id() != super::provider::ProviderId::Codex || connection.provider_capabilities()["voice"].as_bool() != Some(true) {
                return Err("Use the selected provider's own voice controls".into());
            }
            let echo_reduction = match object.get("mode").and_then(Value::as_str) {
                Some("headphones") => false,
                Some("speakers") => true,
                _ => return Err("Choose headphones or speakers for microphone mode".into()),
            };
            app.session.execute("prefs.set", json!({"key":"general.codexVoiceEchoReduction","value":echo_reduction})).map_err(|e| e.to_string())?;
            Ok(json!({"mode":if echo_reduction {"speakers"} else {"headphones"},"applies":"next voice start"}))
        }
        "console.voice.choice.set" => {
            if app.ui.console.chat.voice.on() || app.ui.console.chat.busy {
                return Err("Turn voice off and finish the current turn before choosing a speaking voice".into());
            }
            let connection = app.hooks.codex_connection.as_ref().ok_or("AI connection unavailable")?;
            if connection.provider_id() != super::provider::ProviderId::Codex || connection.provider_capabilities()["voice"].as_bool() != Some(true) {
                return Err("Choose the speaking voice in your selected provider's own controls".into());
            }
            let voice = match object.get("voice") {
                Some(Value::Null) => String::new(),
                Some(Value::String(id)) if app.ui.console.chat.voices.contains(id) => id.clone(),
                _ => return Err("Choose an available speaking voice or null for Use Default".into()),
            };
            app.session.execute("prefs.set", json!({"key":"general.codexSpeakingVoice","value":voice})).map_err(|e| e.to_string())?;
            Ok(json!({"voice":voice,"applies":"next voice start"}))
        }
        "console.voice.channel.set" => {
            if app.ui.console.chat.voice.on() || app.ui.console.chat.busy {
                return Err("Turn voice off and finish the current turn before choosing an input channel".into());
            }
            let channel = object.get("channel").and_then(Value::as_u64).filter(|n| (1..=256).contains(n)).ok_or("Choose an input channel from 1 to 256")?;
            app.ui.console.voice_channel = (channel - 1) as u16;
            Ok(json!({"channel":channel}))
        }
        "console.model.set" => {
            if app.ui.console.chat.busy || app.ui.console.chat.voice.on() {
                return Err("Finish the current turn or stop voice before changing models".into());
            }
            let model = match object.get("model") {
                Some(Value::Null) => None,
                Some(Value::String(model)) if app.ui.console.chat.models.iter().any(|m| &m.id == model) => Some(model.clone()),
                _ => return Err("Choose an available model or null for Codex default".into()),
            };
            app.ui.console.chat.selected_model = model;
            Ok(json!({"model":app.ui.console.chat.selected_model}))
        }
        "console.chat.send" => {
            super::assistants::maintenance_guard(app)?;
            super::sessions::sync_project(app);
            if super::sessions::context_pending(app) {
                return Err("Wait for the new project context to refresh before sending a message".into());
            }
            if app.ui.console.connection != ConnectionStatus::Ready || app.ui.console.chat.busy || app.ui.console.chat.voice.on() {
                return Err("Connect Codex and finish the current turn or stop voice before sending".into());
            }
            let text = object
                .get("message")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty() && s.len() <= MAX_LINE)
                .ok_or("Message must contain 1–16384 bytes")?
                .trim()
                .to_owned();
            let model = match object.get("model") {
                None => app.ui.console.chat.selected_model.clone(),
                Some(Value::Null) => None,
                Some(Value::String(model)) if app.ui.console.chat.models.iter().any(|m| &m.id == model) => Some(model.clone()),
                _ => return Err("Choose an available model".into()),
            };
            let prepared = super::references::context(app, &text)?;
            let images = super::references::images(app);
            app.hooks.codex_connection.as_mut().ok_or("Codex is unavailable")?.send_prompt_with_images(prepared, model, images)?;
            let recorded = super::references::message_record(app, &text);
            super::references::sent(app);
            chat::message(&mut app.ui.console.chat, "You", recorded);
            app.ui.console.chat.busy = true;
            app.ui.console.chat.prompt.clear();
            Ok(json!({"sent":true}))
        }
        "console.chat.copy" => {
            let index = object.get("index").and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok()).ok_or("Choose a displayed message index")?;
            let (_, text) = app.ui.console.chat.messages.get(index).ok_or("This message is no longer displayed")?;
            let selection = match object.get("selection") {
                None => false,
                Some(Value::Bool(value)) => *value,
                Some(_) => return Err("selection must be a boolean".into()),
            };
            let selected = selection && ctx.plugin::<egui::text_selection::LabelSelectionState>().lock().has_selection();
            if selected {
                // The history paints after this command, then egui copies the selected
                // text at end of pass. The draft has already handled its input.
                ctx.input_mut(|input| input.events.push(egui::Event::Copy));
            } else {
                ctx.copy_text(text.clone());
            }
            Ok(json!({"copied":true,"selection":selected}))
        }
        "console.chat.paste" => {
            app.ui.console.open = true;
            app.ui.console.chat.paste_requested = true;
            ctx.request_repaint();
            Ok(json!({"pasteRequested":true,"destination":"message draft","sent":false}))
        }
        "console.chat.cancel" => {
            app.hooks.codex_connection.as_mut().ok_or("Codex is unavailable")?.cancel()?;
            super::bridge::cancel_pending(app, "User cancelled the console turn");
            Ok(json!({"cancelRequested":true}))
        }
        "console.voice.on" => {
            super::assistants::maintenance_guard(app)?;
            super::sessions::sync_project(app);
            if super::sessions::context_pending(app) {
                return Err("Wait for creative/project context to refresh before starting voice".into());
            }
            if app.ui.console.connection != ConnectionStatus::Ready || app.ui.console.chat.busy || app.ui.console.chat.voice.on() {
                return Err("Connect Codex and finish the current turn before starting voice".into());
            }
            app.ui.console.open = true;
            let connection = app.hooks.codex_connection.as_ref().ok_or("AI connection unavailable")?;
            if connection.provider_capabilities()["voice"].as_bool() != Some(true) {
                return Err("FilmCraft voice is unavailable for this provider; use its external voice controls when supported".into());
            }
            let codex = connection.provider_id() == super::provider::ProviderId::Codex;
            let echo_reduction = app.session.prefs.general.codex_voice_echo_reduction;
            let selected = if codex { app.session.prefs.general.codex_speaking_voice.clone() } else { String::new() };
            let selected = (!selected.is_empty()).then_some(selected);
            if selected.as_ref().is_some_and(|id| !app.ui.console.chat.voices.contains(id)) {
                return Err(if app.ui.console.chat.voice_catalog_pending {
                    "Wait for speaking voices to refresh, or choose Use Default".into()
                } else {
                    "Selected speaking voice is unavailable; choose an available voice or Use Default".into()
                });
            }
            let result = app.hooks.codex_connection.as_mut().ok_or("Codex is unavailable".into()).and_then(|c| {
                if codex {
                    c.set_voice_echo_reduction(echo_reduction)?;
                }
                c.start_voice_selected(app.ui.console.chat.selected_model.clone(), app.ui.console.voice_channel, selected)
            });
            match result {
                Ok(()) => {
                    app.ui.console.chat.voice_muted = false;
                    app.ui.console.chat.voice = VoiceStatus::Connecting;
                    Ok(json!({"voice":"connecting"}))
                }
                Err(error) => {
                    app.ui.console.chat.voice = VoiceStatus::Failed(error.clone());
                    Err(error)
                }
            }
        }
        "console.voice.off" => voice_off(app),
        "console.chat.approve" | "console.chat.reject" => {
            let approve = id == "console.chat.approve";
            let pending = app.ui.console.chat.pending.take().ok_or("No edit proposal is pending")?;
            let call_id = pending.id.clone();
            decide(app, ctx, pending, call_id, approve)
        }
        _ => Err("Unsupported console command".into()),
    }
}
pub(super) fn voice_off(app: &mut FilmcraftApp) -> Result<Value, String> {
    if !app.ui.console.chat.voice.on() {
        app.ui.console.chat.voice = VoiceStatus::Off;
        return Ok(json!({"voice":"off"}));
    }
    let result = app.hooks.codex_connection.as_mut().ok_or("Codex is unavailable".into()).and_then(|c| c.stop_voice());
    match result {
        Ok(()) => {
            app.ui.console.chat.voice = VoiceStatus::Stopping;
            Ok(json!({"voice":"stopping"}))
        }
        Err(error) => {
            if let Some(c) = app.hooks.codex_connection.as_mut() {
                c.disconnect();
            }
            app.ui.console.connection = ConnectionStatus::Disconnected;
            app.ui.console.chat.voice = VoiceStatus::Failed(error.clone());
            Err(error)
        }
    }
}
fn decide(app: &mut FilmcraftApp, ctx: &egui::Context, pending: chat::Proposal, id: Value, approve: bool) -> Result<Value, String> {
    let command = pending.command.clone();
    let result = if approve { pending.apply(app, ctx) } else { Err("User rejected the proposed edit".into()) };
    chat::message(
        &mut app.ui.console.chat,
        "Edit",
        match &result {
            Ok(value) => preview(value),
            Err(error) => error.clone(),
        },
    );
    let returned = if approve { result.clone() } else { Ok(json!({"rejected":true})) };
    chat::reply(app, id, result.map(|value| json!({"status":"applied","command":command,"result":value,"undoAvailable":true})));
    returned
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn microphone_mute_preserves_voice_and_project_and_rejects_bad_parameters() {
        use std::sync::{Arc, Mutex};
        struct Host(Arc<Mutex<Vec<bool>>>);
        impl CodexConnection for Host {
            fn connect(&mut self) -> Result<(), String> {
                Ok(())
            }
            fn poll(&mut self) -> Option<ConnectionStatus> {
                None
            }
            fn disconnect(&mut self) {}
            fn set_voice_muted(&mut self, muted: bool) -> Result<(), String> {
                self.0.lock().unwrap().push(muted);
                Ok(())
            }
        }
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        let calls = Arc::new(Mutex::new(Vec::new()));
        app.hooks.codex_connection = Some(Box::new(Host(calls.clone())));
        app.ui.console.connection = ConnectionStatus::Ready;
        app.ui.console.chat.voice = VoiceStatus::Active;
        app.ui.console.chat.busy = true;
        let revision = app.session.revision;
        let ctx = egui::Context::default();
        let result = invoke(&mut app, &ctx, "console.voice.mute", json!({"muted":true})).unwrap();
        assert_eq!(result["speakerMuted"], false);
        assert!(app.ui.console.chat.voice_muted);
        assert_eq!(app.ui.console.chat.voice, VoiceStatus::Active);
        assert!(app.ui.console.chat.busy);
        invoke(&mut app, &ctx, "console.voice.mute", json!({})).unwrap();
        assert!(!app.ui.console.chat.voice_muted);
        for params in [json!({"muted":"yes"}), json!({"muted":1}), json!({"muted":true,"extra":0})] {
            assert!(invoke(&mut app, &ctx, "console.voice.mute", params).is_err());
        }
        assert_eq!(*calls.lock().unwrap(), [true, false]);
        assert_eq!(app.session.revision, revision);
        app.ui.console.chat.voice = VoiceStatus::Off;
        assert!(invoke(&mut app, &ctx, "console.voice.mute", json!({})).is_err());
    }
    #[test]
    fn output_clipboard_actions_keep_transcript_and_never_send_or_edit() {
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        chat::message(&mut app.ui.console.chat, "Codex", "Keep this response".into());
        let messages = app.ui.console.chat.messages.clone();
        let ctx = egui::Context::default();
        let result = invoke(&mut app, &ctx, "console.chat.copy", json!({"index":0})).unwrap();
        assert_eq!(result["copied"], true);
        for index in [json!(-1), json!(1), json!(u64::MAX), json!("0")] {
            assert!(invoke(&mut app, &ctx, "console.chat.copy", json!({"index":index})).is_err());
        }
        let result = invoke(&mut app, &ctx, "console.chat.copy", json!({"index":0,"selection":true})).unwrap();
        assert_eq!(result["selection"], false, "no highlight falls back to the full message");
        for selection in [json!(null), json!("yes"), json!(1), json!([]), json!({})] {
            assert!(invoke(&mut app, &ctx, "console.chat.copy", json!({"index":0,"selection":selection})).is_err());
        }
        let result = invoke(&mut app, &ctx, "console.chat.paste", json!({})).unwrap();
        assert_eq!(result["sent"], false);
        assert!(app.ui.console.chat.paste_requested);
        assert!(app.ui.console.open);
        assert!(!app.ui.console.chat.busy);
        assert_eq!(app.ui.console.chat.messages, messages);
        assert!(!app.session.history.can_undo());
    }

    #[test]
    fn hostile_session_command_parameters_do_not_edit_the_project() {
        struct NoSend;
        impl CodexConnection for NoSend {
            fn connect(&mut self) -> Result<(), String> {
                Ok(())
            }
            fn poll(&mut self) -> Option<ConnectionStatus> {
                None
            }
            fn disconnect(&mut self) {}
            fn send_prompt(&mut self, _prompt: String) -> Result<(), String> {
                panic!("hostile parameters reached the transport")
            }
            fn forget_api_configuration(&mut self) -> Result<(), String> {
                panic!("hostile clear parameters reached the transport")
            }
        }

        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.ui.console.connection = ConnectionStatus::Ready;
        app.hooks.codex_connection = Some(Box::new(NoSend));
        let ctx = egui::Context::default();
        let before = app.session.project.clone();
        app.ui.console.connection = ConnectionStatus::Disconnected;
        for params in [Value::Null, json!([]), json!({"unexpected":true}), json!({"key":"sk-synthetic-private-test-value"})] {
            let error = invoke(&mut app, &ctx, "console.api.clear", params).unwrap_err();
            assert!(!error.contains("sk-synthetic-private-test-value"));
            assert_eq!(*before, *app.session.project);
        }
        app.ui.console.connection = ConnectionStatus::Ready;
        for id in [
            "console.voice.channel.set",
            "console.model.set",
            "console.chat.send",
            "console.chat.cancel",
            "console.chat.approve",
            "console.chat.reject",
            "console.voice.on",
            "console.voice.off",
        ] {
            for params in [Value::Null, json!([]), json!({"unexpected":true})] {
                assert!(invoke(&mut app, &ctx, id, params).is_err(), "{id}");
                assert_eq!(*before, *app.session.project);
            }
        }
        for params in [json!({}), json!({"model":false}), json!({"model":"missing-model"})] {
            assert!(invoke(&mut app, &ctx, "console.model.set", params).is_err());
        }
        for params in [json!({}), json!({"message":false}), json!({"message":""}), json!({"message":"x".repeat(MAX_LINE+1)})] {
            assert!(invoke(&mut app, &ctx, "console.chat.send", params).is_err());
        }
        for channel in [Value::Null, json!(0), json!(257), json!(-1), json!(1.5), json!(u64::MAX), json!("1")] {
            assert!(invoke(&mut app, &ctx, "console.voice.channel.set", json!({"channel":channel})).is_err());
            assert_eq!(app.ui.console.voice_channel, 0);
        }
        assert_eq!(invoke(&mut app, &ctx, "console.voice.channel.set", json!({"channel":12})).unwrap()["channel"], 12);
        assert_eq!(app.ui.console.voice_channel, 11);
        app.ui.console.chat.busy = true;
        assert!(invoke(&mut app, &ctx, "console.voice.channel.set", json!({"channel":1})).is_err());
        app.ui.console.chat.busy = false;
        assert!(invoke(&mut app, &ctx, "console.voice.on", json!({})).is_err());
        assert!(invoke(&mut app, &ctx, "console.voice.off", json!({})).is_ok());
        assert_eq!(*before, *app.session.project);
    }
}

#[cfg(test)]
mod speaking_voice_tests {
    use super::*;
    #[test]
    fn deferred_api_command_is_rejected_before_touching_current_session_or_draft() {
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.hooks.codex_connection = Some(Box::new(NoAudio));
        app.ui.console.connection = ConnectionStatus::Ready;
        app.ui.console.chat.prompt = "Preserve my unsent draft".into();
        app.ui.console.chat.busy = true;
        let project = app.session.project.clone();
        let stamp = (app.session.revision, app.session.document_epoch);
        let result = invoke(&mut app, &egui::Context::default(), "console.provider.select", json!({"provider":"openAiApi"}));
        assert!(result.unwrap_err().contains("unresolved"));
        assert_eq!(app.ui.console.connection, ConnectionStatus::Ready);
        assert!(app.ui.console.chat.busy);
        assert_eq!(app.ui.console.chat.prompt, "Preserve my unsent draft");
        assert!(std::sync::Arc::ptr_eq(&project, &app.session.project));
        assert_eq!((app.session.revision, app.session.document_epoch), stamp);
    }
    struct NoAudio;
    impl CodexConnection for NoAudio {
        fn connect(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn poll(&mut self) -> Option<ConnectionStatus> {
            None
        }
        fn disconnect(&mut self) {}
        fn start_voice_selected(&mut self, _: Option<String>, _: u16, _: Option<String>) -> Result<(), String> {
            panic!("Selecting a voice must not open microphone or start realtime")
        }
    }
    #[test]
    fn microphone_mode_configuration_precedes_voice_start_and_failure_does_not_start_audio() {
        use std::sync::{Arc, Mutex};
        struct ModeHost {
            calls: Arc<Mutex<Vec<&'static str>>>,
            reject: bool,
        }
        impl CodexConnection for ModeHost {
            fn connect(&mut self) -> Result<(), String> {
                Ok(())
            }
            fn poll(&mut self) -> Option<ConnectionStatus> {
                None
            }
            fn disconnect(&mut self) {}
            fn set_voice_echo_reduction(&mut self, enabled: bool) -> Result<(), String> {
                self.calls.lock().unwrap().push(if enabled { "speakers" } else { "headphones" });
                if self.reject { Err("Mode configuration failed".into()) } else { Ok(()) }
            }
            fn start_voice_selected(&mut self, _: Option<String>, _: u16, _: Option<String>) -> Result<(), String> {
                self.calls.lock().unwrap().push("start");
                Ok(())
            }
        }
        let ctx = egui::Context::default();
        for (speakers, reject, expected) in
            [(false, false, vec!["headphones", "start"]), (true, false, vec!["speakers", "start"]), (true, true, vec!["speakers"])]
        {
            let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
            let calls = Arc::new(Mutex::new(Vec::new()));
            app.hooks.codex_connection = Some(Box::new(ModeHost { calls: calls.clone(), reject }));
            app.ui.console.connection = ConnectionStatus::Ready;
            app.session.prefs.general.codex_voice_echo_reduction = speakers;
            let result = invoke(&mut app, &ctx, "console.voice.on", json!({}));
            assert_eq!(result.is_err(), reject);
            assert_eq!(*calls.lock().unwrap(), expected);
            if reject {
                assert!(matches!(app.ui.console.chat.voice, VoiceStatus::Failed(_)));
            } else {
                assert_eq!(app.ui.console.chat.voice, VoiceStatus::Connecting);
            }
        }
    }

    #[test]
    fn microphone_mode_is_guarded_and_does_not_open_devices_or_change_the_project() {
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.hooks.codex_connection = Some(Box::new(NoAudio));
        let ctx = egui::Context::default();
        let project = app.session.project.clone();
        let identity = (app.session.revision, app.session.document_epoch);
        invoke(&mut app, &ctx, "console.voice.inputMode.set", json!({"mode":"speakers"})).unwrap();
        assert!(app.session.prefs.general.codex_voice_echo_reduction);
        for params in [
            json!(null),
            json!({}),
            json!({"mode":false}),
            json!({"mode":"unknown"}),
            json!({"mode":"headphones","extra":true}),
            json!({"mode":"x".repeat(65536)}),
        ] {
            assert!(invoke(&mut app, &ctx, "console.voice.inputMode.set", params).is_err());
            assert!(app.session.prefs.general.codex_voice_echo_reduction);
        }
        for state in [VoiceStatus::Connecting, VoiceStatus::Active, VoiceStatus::Stopping] {
            app.ui.console.chat.voice = state;
            assert!(invoke(&mut app, &ctx, "console.voice.inputMode.set", json!({"mode":"headphones"})).is_err());
        }
        app.ui.console.chat.voice = VoiceStatus::Off;
        app.ui.console.chat.busy = true;
        assert!(invoke(&mut app, &ctx, "console.voice.inputMode.set", json!({"mode":"headphones"})).is_err());
        app.ui.console.chat.busy = false;
        invoke(&mut app, &ctx, "console.voice.inputMode.set", json!({"mode":"headphones"})).unwrap();
        assert!(!app.session.prefs.general.codex_voice_echo_reduction);
        assert!(std::sync::Arc::ptr_eq(&project, &app.session.project));
        assert_eq!((app.session.revision, app.session.document_epoch), identity);
    }

    #[test]
    fn speaking_voice_choice_is_explicit_guarded_and_does_not_edit_or_start_audio() {
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.hooks.codex_connection = Some(Box::new(NoAudio));
        app.ui.console.chat.voices = vec!["sol".into(), "cove".into()];
        let ctx = egui::Context::default();
        let project = app.session.project.clone();
        let identity = (app.session.revision, app.session.document_epoch);
        invoke(&mut app, &ctx, "console.voice.choice.set", json!({"voice":"sol"})).unwrap();
        assert_eq!(app.session.prefs.general.codex_speaking_voice, "sol");
        for parameters in [
            json!(null),
            json!([]),
            json!({}),
            json!({"voice":false}),
            json!({"voice":"marin"}),
            json!({"voice":"sol","extra":0}),
            json!({"voice":"x".repeat(65536)}),
        ] {
            assert!(invoke(&mut app, &ctx, "console.voice.choice.set", parameters).is_err());
            assert_eq!(app.session.prefs.general.codex_speaking_voice, "sol");
        }
        app.ui.console.chat.busy = true;
        assert!(invoke(&mut app, &ctx, "console.voice.choice.set", json!({"voice":null})).is_err());
        app.ui.console.chat.busy = false;
        for state in [VoiceStatus::Connecting, VoiceStatus::Active, VoiceStatus::Stopping] {
            app.ui.console.chat.voice = state;
            assert!(invoke(&mut app, &ctx, "console.voice.choice.set", json!({"voice":null})).is_err());
        }
        app.ui.console.chat.voice = VoiceStatus::Off;
        invoke(&mut app, &ctx, "console.voice.choice.set", json!({"voice":null})).unwrap();
        assert_eq!(app.session.prefs.general.codex_speaking_voice, "");
        assert!(std::sync::Arc::ptr_eq(&project, &app.session.project));
        assert_eq!((app.session.revision, app.session.document_epoch), identity);
    }
}
