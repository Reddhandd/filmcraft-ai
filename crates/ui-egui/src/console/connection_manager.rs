//! Connection ownership wraps transports; current Codex stays the default.
use super::{
    ChatEvent, CodexConnection, ConnectionStatus,
    provider::{Ownership, Permit, ProviderId},
    sessions,
};
use serde_json::{Value, json};
pub struct Manager {
    codex: Box<dyn CodexConnection>,
    api: Option<Box<dyn CodexConnection>>,
    desktop: Option<Box<dyn CodexConnection>>,
    owner: Ownership,
    permit: Option<Permit>,
    error: Option<String>,
}
impl Manager {
    pub fn new(codex: Box<dyn CodexConnection>) -> Self {
        Self { codex, api: None, desktop: None, owner: Ownership::default(), permit: None, error: None }
    }
    pub fn with_api(mut self, api: Box<dyn CodexConnection>) -> Self {
        self.api = Some(api);
        self
    }
    pub fn with_desktop(mut self, desktop: Box<dyn CodexConnection>) -> Self {
        self.desktop = Some(desktop);
        self
    }
    fn endpoint(&mut self) -> Result<&mut (dyn CodexConnection + '_), String> {
        match self.owner.selected() {
            ProviderId::Codex => Ok(self.codex.as_mut()),
            ProviderId::OpenAiApi => match self.api.as_mut() {
                Some(connection) => Ok(connection.as_mut()),
                None => Err("The API adapter is not configured or qualified in this build; Codex is available in AI connection".into()),
            },
            ProviderId::DesktopChatGpt => match self.desktop.as_mut() {
                Some(connection) => Ok(connection.as_mut()),
                None => Err("External ChatGPT adapter is not installed in this build".into()),
            },
        }
    }
    fn start(&mut self, request: Option<sessions::SessionRequest>) -> Result<(), String> {
        if self.owner.selected() != ProviderId::Codex
            && self.provider_capabilities()["resume"].as_bool() != Some(true)
            && request.as_ref().is_some_and(|request| request.resume.is_some())
        {
            return Err("Resuming another provider's conversation is not supported".into());
        }
        if let Some(request) = request.as_ref() {
            self.endpoint()?.validate_session_request(request)?;
        }
        if self.owner.selected() == ProviderId::DesktopChatGpt {
            self.disconnect();
        }
        self.error = None;
        let permit = self.owner.begin(self.owner.selected())?;
        let token = permit.transport_token();
        self.permit = Some(permit);
        let result = (|| {
            let endpoint = self.endpoint()?;
            endpoint.bind_provider_permit(token)?;
            match request {
                Some(request) => endpoint.connect_session(request),
                None => endpoint.connect(),
            }
        })();
        if result.is_err() {
            self.owner.revoke()?;
            self.permit = None;
        }
        result
    }
    fn ready_endpoint(&mut self) -> Result<&mut (dyn CodexConnection + '_), String> {
        if self.owner.state() != super::provider::LinkState::Ready {
            return Err("Connect the selected provider and wait for its verified transport before sending".into());
        }
        self.endpoint()
    }
}
impl CodexConnection for Manager {
    fn configure_api(&mut self, key: Option<String>, model: String) -> Result<(), String> {
        if self.owner.selected() != ProviderId::OpenAiApi || self.permit.is_some() {
            return Err("Select OpenAI API and disconnect before changing API settings".into());
        }
        self.endpoint()?.configure_api(key, model)
    }
    fn api_configuration(&self) -> Option<super::api_settings::Status> {
        if self.owner.selected() != ProviderId::OpenAiApi {
            return None;
        }
        self.api.as_ref()?.api_configuration()
    }
    fn forget_api_configuration(&mut self) -> Result<(), String> {
        if self.owner.selected() != ProviderId::OpenAiApi || self.permit.is_some() {
            return Err("Select OpenAI API and disconnect before clearing its key".into());
        }
        self.endpoint()?.forget_api_configuration()
    }

    fn provider_id(&self) -> ProviderId {
        self.owner.selected()
    }
    fn provider_capabilities(&self) -> Value {
        match self.owner.selected() {
            ProviderId::Codex => self.codex.provider_capabilities(),
            ProviderId::OpenAiApi => self.api.as_ref().map(|connection| connection.provider_capabilities()).unwrap_or_else(
                || json!({"embeddedComposer":false,"nativeTools":false,"images":"unknown","audioAnalysis":false,"voice":false,"qualified":false}),
            ),
            ProviderId::DesktopChatGpt => self
                .desktop
                .as_ref()
                .map(|connection| connection.provider_capabilities())
                .unwrap_or_else(|| json!({"embeddedComposer":false,"nativeTools":false,"voice":false,"qualified":false})),
        }
    }
    fn select_provider(&mut self, provider: ProviderId) -> Result<(), String> {
        if provider == ProviderId::OpenAiApi && self.api.is_none() {
            return Err(super::provider::API_UNRESOLVED.into());
        }
        if self.owner.selected() == provider {
            return Ok(());
        }
        self.disconnect();
        self.owner.select(provider)?;
        self.error = None;
        Ok(())
    }
    fn authorizes_provider(&self, token: &str) -> bool {
        self.owner.authorizes_token(token)
            && (self.owner.selected() != ProviderId::DesktopChatGpt || self.desktop.as_ref().is_some_and(|connection| connection.authorizes_provider(token)))
    }
    fn external_provider_event(&mut self, token: &str, event: &str) -> Result<(), String> {
        if self.owner.selected() != ProviderId::DesktopChatGpt || !self.owner.matches_token(token) {
            return Err("External attachment was revoked or its provider is not selected".into());
        }
        self.endpoint()?.external_provider_event(token, event)?;
        if event == "attach" {
            self.owner.ready(self.permit.as_ref().ok_or("External provider permit is missing")?)?;
        } else if event == "detach" {
            self.owner.revoke()?;
            self.permit = None;
            self.error = Some("External MCP client disconnected; reconnect for a fresh attachment".into());
        }
        Ok(())
    }
    fn connect(&mut self) -> Result<(), String> {
        self.start(None)
    }
    fn connect_session(&mut self, request: sessions::SessionRequest) -> Result<(), String> {
        self.start(Some(request))
    }
    fn switch_session(&mut self, request: sessions::SessionRequest) -> Result<(), String> {
        if self.owner.selected() == ProviderId::OpenAiApi && self.provider_capabilities()["resume"].as_bool() == Some(true) {
            self.endpoint()?.validate_session_request(&request)?;
        }
        if self.owner.selected() != ProviderId::Codex {
            self.disconnect();
            if self.owner.selected() == ProviderId::OpenAiApi && self.provider_capabilities()["resume"].as_bool() == Some(true) {
                request.validate()?;
                return self.start(Some(request));
            }
            let error = "Project context changed; provider ownership revoked. Reconnect for a fresh conversation".to_string();
            self.error = Some(error.clone());
            return Err(error);
        }
        // Rebinding context rotates the transport permit too: requests already queued
        // by the old project/session must fail at UI execution.
        self.disconnect();
        self.start(Some(request))
    }
    fn disconnect(&mut self) {
        if let Ok(endpoint) = self.endpoint() {
            endpoint.disconnect();
        }
        if let Err(error) = self.owner.revoke() {
            self.error = Some(error);
        }
        self.permit = None;
    }
    fn poll(&mut self) -> Option<ConnectionStatus> {
        if let Some(error) = self.error.take() {
            return Some(ConnectionStatus::Failed(error));
        }
        let (token, status) = self.endpoint().ok()?.poll_owned_status()?;
        if !self.owner.matches_token(&token) {
            return None;
        }
        match &status {
            ConnectionStatus::Ready => {
                let permit = self.permit.as_ref()?;
                if let Err(error) = self.owner.ready(permit) {
                    return Some(ConnectionStatus::Failed(error));
                }
            }
            ConnectionStatus::Failed(_) | ConnectionStatus::Disconnected => {
                if let Err(error) = self.owner.revoke() {
                    self.error = Some(error);
                }
                self.permit = None;
            }
            ConnectionStatus::Connecting => {}
        }
        Some(status)
    }
    fn poll_chat(&mut self) -> Option<ChatEvent> {
        let (token, event) = self.endpoint().ok()?.poll_owned_chat()?;
        // Catalog metadata may arrive between the UI's status and chat polls. Accepting
        // current-token metadata does not grant native access before Ready.
        let setup_event = (self.owner.selected() == ProviderId::DesktopChatGpt && matches!(event, ChatEvent::Finished))
            || (self.owner.selected() == ProviderId::Codex && matches!(event, ChatEvent::VoiceChoices { .. }));
        (self.owner.authorizes_token(&token) || (setup_event && self.owner.matches_token(&token))).then_some(event)
    }
    fn send_prompt(&mut self, prompt: String) -> Result<(), String> {
        self.ready_endpoint()?.send_prompt(prompt)
    }
    fn send_prompt_with_model(&mut self, prompt: String, model: Option<String>) -> Result<(), String> {
        self.ready_endpoint()?.send_prompt_with_model(prompt, model)
    }
    fn send_prompt_with_images(&mut self, prompt: String, model: Option<String>, images: Vec<String>) -> Result<(), String> {
        self.ready_endpoint()?.send_prompt_with_images(prompt, model, images)
    }
    fn start_voice(&mut self, model: Option<String>) -> Result<(), String> {
        self.ready_endpoint()?.start_voice(model)
    }
    fn start_voice_channel(&mut self, model: Option<String>, channel: u16) -> Result<(), String> {
        self.ready_endpoint()?.start_voice_channel(model, channel)
    }
    fn start_voice_selected(&mut self, model: Option<String>, channel: u16, voice: Option<String>) -> Result<(), String> {
        self.ready_endpoint()?.start_voice_selected(model, channel, voice)
    }
    fn set_voice_echo_reduction(&mut self, enabled: bool) -> Result<(), String> {
        self.ready_endpoint()?.set_voice_echo_reduction(enabled)
    }
    fn set_voice_muted(&mut self, muted: bool) -> Result<(), String> {
        self.ready_endpoint()?.set_voice_muted(muted)
    }
    fn stop_voice(&mut self) -> Result<(), String> {
        self.ready_endpoint()?.stop_voice()
    }
    fn cancel(&mut self) -> Result<(), String> {
        if self.owner.selected() == ProviderId::OpenAiApi {
            self.disconnect();
            self.error =
                Some("API turn cancelled and ownership revoked; reconnect for a fresh conversation. Inspect native state before repeating edits".into());
            Ok(())
        } else {
            self.ready_endpoint()?.cancel()
        }
    }
    fn answer(&mut self, id: Value, result: Result<Value, String>) -> Result<(), String> {
        self.ready_endpoint()?.answer(id, result)
    }
}
/// The selector uses native commands; credentials and permits never enter serialized UI state or inspection.
pub(super) fn selector(app: &mut crate::FilmcraftApp, ui: &mut egui::Ui) {
    let selected = app.hooks.codex_connection.as_ref().map(|connection| connection.provider_id()).unwrap_or_default();
    let mut choice = None;
    let response = craft_ui::ComboBox::from_id_salt("console-ai-provider")
        .selected_text(selected.label())
        .show_ui(ui, |ui| {
            for provider in [ProviderId::Codex, ProviderId::DesktopChatGpt] {
                let option = ui.selectable_label(provider == selected, provider.label());
                app.auto.add(&format!("console.provider.option.{provider:?}"), option.rect, provider.label());
                if option.clicked() {
                    choice = Some(provider);
                }
            }
            let unavailable = ui.add_enabled(false, egui::Button::new("OpenAI API — unresolved"));
            app.auto.add("console.provider.option.OpenAiApi", unavailable.rect, "OpenAI API — unresolved (disabled)");
            unavailable.on_disabled_hover_text(super::provider::API_UNRESOLVED);
        })
        .response;
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, response.enabled(), "AI connection"));
    app.auto.add("console.provider", response.rect, "AI connection");
    if let Some(provider) = choice
        && let Err(error) = super::commands::invoke(app, ui.ctx(), "console.provider.select", json!({"provider":provider}))
    {
        app.ui.status = error;
    }
    if selected == ProviderId::DesktopChatGpt {
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
        ui.label("Send messages and choose your model and speaking voice in ChatGPT. After reconnecting here, ask your existing chat to reconnect to FilmCraft using connection_rebind.");
        if let Some(path) = app
            .hooks
            .codex_connection
            .as_ref()
            .and_then(|connection| connection.provider_capabilities().get("setupFile").and_then(Value::as_str).map(str::to_owned))
        {
            ui.label(format!("MCP setup file (configure once): {path}"));
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    #[derive(Default)]
    struct State {
        token: String,
        status: std::collections::VecDeque<(String, ConnectionStatus)>,
        events: std::collections::VecDeque<(String, ChatEvent)>,
        disconnects: usize,
    }
    struct Fake(Arc<Mutex<State>>);
    impl CodexConnection for Fake {
        fn bind_provider_permit(&mut self, token: String) -> Result<(), String> {
            self.0.lock().unwrap().token = token;
            Ok(())
        }
        fn connect(&mut self) -> Result<(), String> {
            let mut state = self.0.lock().unwrap();
            let token = state.token.clone();
            state.status.push_back((token, ConnectionStatus::Ready));
            Ok(())
        }
        fn disconnect(&mut self) {
            self.0.lock().unwrap().disconnects += 1;
        }
        fn poll(&mut self) -> Option<ConnectionStatus> {
            None
        }
        fn poll_owned_status(&mut self) -> Option<(String, ConnectionStatus)> {
            self.0.lock().unwrap().status.pop_front()
        }
        fn poll_owned_chat(&mut self) -> Option<(String, ChatEvent)> {
            self.0.lock().unwrap().events.pop_front()
        }
    }
    #[test]
    fn unavailable_api_selection_preserves_the_ready_codex_connection() {
        let state = Arc::new(Mutex::new(State::default()));
        let mut manager = Manager::new(Box::new(Fake(state.clone())));
        manager.connect().unwrap();
        assert_eq!(manager.poll(), Some(ConnectionStatus::Ready));
        let token = state.lock().unwrap().token.clone();
        assert!(manager.select_provider(ProviderId::OpenAiApi).unwrap_err().contains("unresolved"));
        assert_eq!(manager.provider_id(), ProviderId::Codex);
        assert!(manager.authorizes_provider(&token));
        assert_eq!(state.lock().unwrap().disconnects, 0);
    }
    #[test]
    fn voice_metadata_before_ready_is_current_token_only_and_does_not_authorize_tools() {
        let state = Arc::new(Mutex::new(State::default()));
        let mut manager = Manager::new(Box::new(Fake(state.clone())));
        manager.connect().unwrap();
        let token = state.lock().unwrap().token.clone();
        state.lock().unwrap().events.push_back((token.clone(), ChatEvent::VoiceChoices { voices: Vec::new(), error: None, loading: true }));
        assert!(matches!(manager.poll_chat(), Some(ChatEvent::VoiceChoices { loading: true, .. })));
        assert!(!manager.authorizes_provider(&token));
        state
            .lock()
            .unwrap()
            .events
            .push_back(("invalid-old-token".into(), ChatEvent::VoiceChoices { voices: vec!["sol".into()], error: None, loading: false }));
        assert!(manager.poll_chat().is_none());
        state.lock().unwrap().events.push_back((token.clone(), ChatEvent::Delta("not ready".into())));
        assert!(manager.poll_chat().is_none());
        assert_eq!(manager.poll(), Some(ConnectionStatus::Ready));
        assert!(manager.authorizes_provider(&token));
        manager.disconnect();
        state.lock().unwrap().events.push_back((token.clone(), ChatEvent::VoiceChoices { voices: vec!["sol".into()], error: None, loading: false }));
        assert!(manager.poll_chat().is_none());
        assert!(!manager.authorizes_provider(&token));
    }

    struct ResumableApi(Fake);
    impl CodexConnection for ResumableApi {
        fn provider_capabilities(&self) -> Value {
            json!({"resume":true})
        }
        fn validate_session_request(&self, request: &sessions::SessionRequest) -> Result<(), String> {
            request.validate()?;
            if request.resume.as_deref().is_some_and(|id| !id.starts_with("api-")) {
                return Err("Choose an API conversation".into());
            }
            Ok(())
        }
        fn bind_provider_permit(&mut self, token: String) -> Result<(), String> {
            self.0.bind_provider_permit(token)
        }
        fn connect(&mut self) -> Result<(), String> {
            self.0.connect()
        }
        fn disconnect(&mut self) {
            self.0.disconnect();
        }
        fn poll(&mut self) -> Option<ConnectionStatus> {
            None
        }
        fn poll_owned_status(&mut self) -> Option<(String, ConnectionStatus)> {
            self.0.poll_owned_status()
        }
    }
    #[test]
    fn api_resume_identity_is_rejected_before_switch_disconnects_current_owner() {
        let state = Arc::new(Mutex::new(State::default()));
        let mut manager = Manager::new(Box::new(Fake(Arc::new(Mutex::new(State::default()))))).with_api(Box::new(ResumableApi(Fake(state.clone()))));
        manager.select_provider(ProviderId::OpenAiApi).unwrap();
        manager.connect().unwrap();
        assert_eq!(manager.poll(), Some(ConnectionStatus::Ready));
        let token = state.lock().unwrap().token.clone();
        let request = sessions::SessionRequest { project: "project".into(), resume: Some("codex-thread".into()), ..Default::default() };
        assert!(manager.switch_session(request).is_err());
        assert!(manager.authorizes_provider(&token));
        assert_eq!(state.lock().unwrap().disconnects, 0);
    }
    struct ExternalFake(Fake);
    impl CodexConnection for ExternalFake {
        fn bind_provider_permit(&mut self, token: String) -> Result<(), String> {
            self.0.bind_provider_permit(token)
        }
        fn connect(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn disconnect(&mut self) {
            self.0.disconnect();
        }
        fn poll(&mut self) -> Option<ConnectionStatus> {
            None
        }
        fn poll_owned_status(&mut self) -> Option<(String, ConnectionStatus)> {
            self.0.poll_owned_status()
        }
        fn authorizes_provider(&self, token: &str) -> bool {
            self.0.0.lock().unwrap().token == token
        }
        fn external_provider_event(&mut self, _token: &str, event: &str) -> Result<(), String> {
            if event == "attach" {
                let mut state = self.0.0.lock().unwrap();
                let token = state.token.clone();
                state.status.push_back((token, ConnectionStatus::Ready));
            }
            Ok(())
        }
    }
    #[test]
    fn external_attachment_authenticates_current_permit_and_revokes_on_detach_and_switch() {
        let codex = Arc::new(Mutex::new(State::default()));
        let desktop = Arc::new(Mutex::new(State::default()));
        let mut manager = Manager::new(Box::new(Fake(codex))).with_desktop(Box::new(ExternalFake(Fake(desktop.clone()))));
        manager.select_provider(ProviderId::DesktopChatGpt).unwrap();
        manager.connect().unwrap();
        let old = desktop.lock().unwrap().token.clone();
        assert!(!manager.authorizes_provider(&old));
        assert!(manager.external_provider_event(&"f".repeat(64), "attach").is_err());
        manager.external_provider_event(&old, "attach").unwrap();
        assert!(manager.authorizes_provider(&old));
        manager.external_provider_event(&old, "detach").unwrap();
        assert!(!manager.authorizes_provider(&old));
        assert!(manager.external_provider_event(&old, "heartbeat").is_err());
        manager.connect().unwrap();
        let current = desktop.lock().unwrap().token.clone();
        assert_ne!(old, current);
        assert!(manager.external_provider_event(&old, "attach").is_err());
        manager.external_provider_event(&current, "attach").unwrap();
        manager.select_provider(ProviderId::Codex).unwrap();
        assert!(!manager.authorizes_provider(&current));
        assert!(manager.external_provider_event(&current, "attach").is_err());
    }
    #[test]
    fn disconnect_revokes_tools_and_queued_events() {
        let codex = Arc::new(Mutex::new(State::default()));
        let api = Arc::new(Mutex::new(State::default()));
        let mut manager = Manager::new(Box::new(Fake(codex.clone()))).with_api(Box::new(Fake(api.clone())));
        manager.connect().unwrap();
        assert_eq!(manager.poll(), Some(ConnectionStatus::Ready));
        let old = codex.lock().unwrap().token.clone();
        assert!(manager.authorizes_provider(&old));
        manager.select_provider(ProviderId::OpenAiApi).unwrap();
        assert!(!manager.authorizes_provider(&old));
        assert_eq!(codex.lock().unwrap().disconnects, 1);
        manager.connect().unwrap();
        assert_eq!(manager.poll(), Some(ConnectionStatus::Ready));
        api.lock().unwrap().events.push_back((old, ChatEvent::Delta("stale provider text".into())));
        assert!(manager.poll_chat().is_none());
        let new = api.lock().unwrap().token.clone();
        api.lock().unwrap().events.push_back((new, ChatEvent::Delta("current".into())));
        assert!(matches!(manager.poll_chat(),Some(ChatEvent::Delta(text)) if text=="current"));
    }
    #[test]
    fn unconfigured_selection_never_claims_connected() {
        let state = Arc::new(Mutex::new(State::default()));
        let mut manager = Manager::new(Box::new(Fake(state)));
        assert_eq!(manager.provider_id(), ProviderId::Codex);
        manager.select_provider(ProviderId::DesktopChatGpt).unwrap();
        assert!(manager.connect().is_err());
        assert!(manager.poll().is_none());
    }
    #[test]
    fn api_cancel_and_project_switch_revoke_before_returning() {
        for project_switch in [false, true] {
            let codex = Arc::new(Mutex::new(State::default()));
            let api = Arc::new(Mutex::new(State::default()));
            let mut manager = Manager::new(Box::new(Fake(codex))).with_api(Box::new(Fake(api.clone())));
            manager.select_provider(ProviderId::OpenAiApi).unwrap();
            manager.connect().unwrap();
            assert_eq!(manager.poll(), Some(ConnectionStatus::Ready));
            let token = api.lock().unwrap().token.clone();
            assert!(manager.authorizes_provider(&token));
            if project_switch {
                assert!(manager.switch_session(sessions::SessionRequest::default()).is_err());
            } else {
                manager.cancel().unwrap();
            }
            assert!(!manager.authorizes_provider(&token));
            assert!(matches!(manager.poll(), Some(ConnectionStatus::Failed(_))));
        }
    }
    #[test]
    fn codex_context_rebind_rotates_ownership_before_new_connection() {
        let state = Arc::new(Mutex::new(State::default()));
        let mut manager = Manager::new(Box::new(Fake(state.clone())));
        manager.connect().unwrap();
        assert_eq!(manager.poll(), Some(ConnectionStatus::Ready));
        let old = state.lock().unwrap().token.clone();
        manager.switch_session(sessions::SessionRequest::default()).unwrap();
        assert!(!manager.authorizes_provider(&old));
        assert_eq!(manager.poll(), Some(ConnectionStatus::Ready));
        let new = state.lock().unwrap().token.clone();
        assert_ne!(new, old);
        assert!(manager.authorizes_provider(&new));
    }
    #[test]
    fn unsupported_api_resume_does_not_replace_an_active_connection_or_accept_codex_ids() {
        let state = Arc::new(Mutex::new(State::default()));
        let mut manager = Manager::new(Box::new(Fake(Arc::new(Mutex::new(State::default()))))).with_api(Box::new(Fake(state.clone())));
        manager.select_provider(ProviderId::OpenAiApi).unwrap();
        manager.connect().unwrap();
        assert_eq!(manager.poll(), Some(ConnectionStatus::Ready));
        let token = state.lock().unwrap().token.clone();
        let request = sessions::SessionRequest {
            project: "current-project".into(),
            scope_aliases: Vec::new(),
            resume: Some("codex-previous-thread".into()),
            memory: "current memory".into(),
            model: None,
            recent_dialogue: Vec::new(),
            assistant_summary: String::new(),
        };
        assert!(manager.connect_session(request).unwrap_err().contains("Resuming"));
        assert!(manager.authorizes_provider(&token));
        assert_eq!(state.lock().unwrap().disconnects, 0);
    }
}
