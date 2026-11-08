//! Session-only API configuration. Deliberately neither serializable nor cloned with UI state.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub configured: bool,
    pub model: Option<String>,
}

/// Private editor state lives on FilmcraftApp, outside cloned/serialized UiState.
#[derive(Default)]
pub(crate) struct Editor {
    key: String,
    model: String,
    initialized: bool,
    error: Option<String>,
}

pub struct Configuration {
    key: String,
    model: String,
}

impl std::fmt::Debug for Configuration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiConfiguration").field("key", &"[redacted]").field("model", &self.model).finish()
    }
}

pub fn validate_model(model: &str) -> Result<(), String> {
    if model.is_empty() || model.len() > 256 || !model.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte)) || model.starts_with("sk-") {
        return Err("Enter a valid API model ID (at most 256 ASCII characters), not an API key".into());
    }
    Ok(())
}

impl Configuration {
    pub fn new(key: String, model: String) -> Result<Self, String> {
        if key.len() > 512 {
            return Err("API key exceeds its 512-byte limit".into());
        }
        let key = key.trim();
        if key.len() < 16 || !key.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err("Enter an API key without spaces or control characters".into());
        }
        if model.len() > 256 {
            return Err("API model ID exceeds its 256-byte limit".into());
        }
        let model = model.trim();
        validate_model(model)?;
        Ok(Self { key: key.to_owned(), model: model.to_owned() })
    }

    /// For the HTTP adapter only. Never include this value in inspection or diagnostics.
    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// A rejected model change preserves the usable configuration and its secret.
    pub fn set_model(&mut self, model: &str) -> Result<(), String> {
        if model.len() > 256 {
            return Err("API model ID exceeds its 256-byte limit".into());
        }
        let model = model.trim();
        validate_model(model)?;
        self.model = model.to_owned();
        Ok(())
    }
}

fn initialize(app: &mut crate::FilmcraftApp) -> Option<Status> {
    let status = app.hooks.codex_connection.as_ref()?.api_configuration()?;
    if !app.api_setup.initialized {
        app.api_setup.model = status.model.clone().unwrap_or_default();
        app.api_setup.initialized = true;
    }
    Some(status)
}

pub(super) fn prepare(app: &mut crate::FilmcraftApp) -> Result<(), String> {
    if app.hooks.codex_connection.as_ref().is_none_or(|connection| connection.provider_id() != super::provider::ProviderId::OpenAiApi)
        || app.ui.console.connection == super::ConnectionStatus::Ready
    {
        return Ok(());
    }
    let Some(status) = initialize(app) else { return Ok(()) };
    let result = (|| {
        if app.api_setup.model.len() > 256 {
            return Err("API model ID exceeds its 256-byte limit".into());
        }
        let model = app.api_setup.model.trim().to_owned();
        validate_model(&model)?;
        if app.api_setup.key.len() > 512 {
            return Err("API key exceeds its 512-byte limit".into());
        }
        let key = if app.api_setup.key.is_empty() {
            if !status.configured {
                return Err("Enter an API key in the OpenAI API connection section".into());
            }
            None
        } else {
            // Validate before handing a bounded copy directly to the transport, never JSON.
            Configuration::new(app.api_setup.key.clone(), model.clone())?;
            Some(app.api_setup.key.clone())
        };
        app.hooks.codex_connection.as_mut().ok_or("API connection is unavailable")?.configure_api(key, model.clone())?;
        app.api_setup.key.clear();
        app.api_setup.model.clone_from(&model);
        app.ui.console.chat.selected_model = Some(model);
        Ok(())
    })();
    app.api_setup.error = result.as_ref().err().cloned();
    result
}

pub(super) fn forget(app: &mut crate::FilmcraftApp) -> Result<(), String> {
    if matches!(app.ui.console.connection, super::ConnectionStatus::Ready | super::ConnectionStatus::Connecting) {
        return Err("Disconnect the API connection before clearing its key".into());
    }
    app.hooks.codex_connection.as_mut().ok_or("API connection is unavailable")?.forget_api_configuration()?;
    app.api_setup.key.clear();
    app.api_setup.error = None;
    Ok(())
}

pub(super) fn show(app: &mut crate::FilmcraftApp, ui: &mut egui::Ui) {
    let Some(status) = initialize(app) else { return };
    let active = matches!(app.ui.console.connection, super::ConnectionStatus::Ready | super::ConnectionStatus::Connecting);
    ui.label("API key");
    let response = ui.add_enabled(
        !active,
        egui::TextEdit::singleline(&mut app.api_setup.key).id_salt("console-api-key").password(true).char_limit(512).hint_text(if status.configured {
            "Key configured — enter a replacement"
        } else {
            "Enter API key"
        }),
    );
    app.auto.add_response("console.api.key", &response, "API key (hidden)");
    // Undo buffers can retain old password values in memory; they are unnecessary here.
    if let Some(mut state) = egui::text_edit::TextEditState::load(ui.ctx(), response.id) {
        state.clear_undoer();
        state.store(ui.ctx(), response.id);
    }
    ui.label("API model ID");
    let model = ui.add_enabled(!active, egui::TextEdit::singleline(&mut app.api_setup.model).char_limit(256).hint_text("For example: gpt-6-luna"));
    app.auto.add_response("console.api.model", &model, "API model ID");
    ui.label("Key kept for this app session only. Authentication and model access are checked when you send.");
    let clear = ui.add_enabled(!active && (status.configured || !app.api_setup.key.is_empty()), egui::Button::new("Clear API key"));
    app.auto.add_response("console.api.clear", &clear, "Clear API key");
    if clear.clicked() {
        let result = if status.configured {
            super::commands::invoke(app, ui.ctx(), "console.api.clear", serde_json::json!({})).map(|_| ())
        } else {
            app.api_setup.key.clear();
            Ok(())
        };
        if let Err(error) = result {
            app.api_setup.error = Some(error);
        }
    }
    if let Some(error) = &app.api_setup.error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Host;
    impl super::super::CodexConnection for Host {
        fn provider_id(&self) -> super::super::provider::ProviderId {
            super::super::provider::ProviderId::OpenAiApi
        }
        fn api_configuration(&self) -> Option<Status> {
            Some(Status::default())
        }
        fn connect(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn disconnect(&mut self) {}
        fn poll(&mut self) -> Option<super::super::ConnectionStatus> {
            None
        }
    }

    #[test]
    fn password_render_and_automation_do_not_disclose_the_key() {
        let secret = "sk-synthetic-private-api-test-value";
        let mut app = crate::FilmcraftApp::new(filmcraft_engine::Session::default());
        app.hooks.codex_connection = Some(Box::new(Host));
        app.api_setup.key = secret.into();
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui));
        // This headless test inspects output without a renderer to consume texture updates.
        output.textures_delta.clear();
        assert!(!format!("{:?}", output.shapes).contains(secret));
        assert!(!format!("{:?}", output.platform_output.events).contains(secret));
        assert!(!format!("{:?}", output.platform_output.commands).contains(secret));
        assert!(output.platform_output.accesskit_update.as_ref().is_some_and(|update| !update.nodes.is_empty()));
        assert!(!format!("{:?}", output.platform_output.accesskit_update).contains(secret));
        assert!(!serde_json::to_string(app.auto.all()).unwrap().contains(secret));
        assert!(app.auto.all().iter().any(|element| element.id == "console.api.key"));
        assert_eq!(app.api_setup.key, secret);
        app.api_setup.key = "x".repeat(513);
        app.api_setup.model = "gpt-6-luna".into();
        app.api_setup.initialized = true;
        assert!(prepare(&mut app).is_err());
    }

    #[test]
    fn diagnostic_output_redacts_the_key_and_invalid_model_keeps_working_settings() {
        let secret = "sk-synthetic-private-api-test-value";
        let mut config = Configuration::new(secret.into(), "gpt-6-luna".into()).unwrap();
        let debug = format!("{config:?}");
        assert!(!debug.contains(secret));
        assert!(debug.contains("[redacted]"));
        for model in ["", "bad model", "💥", secret] {
            assert!(config.set_model(model).is_err());
            assert_eq!(config.model(), "gpt-6-luna");
            assert_eq!(config.key(), secret);
        }
    }

    #[test]
    fn hostile_configuration_is_rejected_without_echoing_secret_input() {
        for secret in ["".into(), "short".into(), "x".repeat(4097), "x\n".repeat(20), "💥".repeat(20)] {
            let result = std::panic::catch_unwind(|| Configuration::new(secret.clone(), "gpt-6-luna".into()));
            let error = result.unwrap().unwrap_err();
            if secret.len() > 8 {
                assert!(!error.contains(&secret));
            }
        }
        assert!(Configuration::new("x".repeat(32), "m".repeat(257)).is_err());
        assert!(Configuration::new("x".repeat(32), "ft:gpt-model:organization:custom".into()).is_ok());
    }
}
