//! Provider ownership is checked at execution, including requests queued before a switch.
use super::{ControlRequest, FilmcraftApp, Outcome, err};
pub(super) fn external(app: &mut FilmcraftApp, req: &ControlRequest) -> Outcome {
    let Some(params) = req.params.as_object() else {
        return err("Invalid external provider lifecycle");
    };
    if params.len() != 2 || params.keys().any(|key| !["permit", "event"].contains(&key.as_str())) {
        return err("Invalid external provider lifecycle");
    }
    let Some(token) =
        params.get("permit").and_then(serde_json::Value::as_str).filter(|token| token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()))
    else {
        return err("Invalid external provider permit");
    };
    let Some(event) = params.get("event").and_then(serde_json::Value::as_str).filter(|event| ["authenticate", "attach", "heartbeat", "detach"].contains(event))
    else {
        return err("Unknown external provider event");
    };
    match app
        .hooks
        .codex_connection
        .as_mut()
        .ok_or_else(|| "AI connection manager is unavailable".to_string())
        .and_then(|connection| connection.external_provider_event(token, event))
    {
        Ok(()) => super::ok(serde_json::json!({"accepted":true})),
        Err(error) => err(error),
    }
}
pub(super) fn dispatch(app: &mut FilmcraftApp, ctx: &egui::Context, req: &ControlRequest) -> Outcome {
    let Some(params) = req.params.as_object() else {
        return err("Invalid provider request envelope");
    };
    if params.len() != 3 || params.keys().any(|key| !["permit", "method", "params"].contains(&key.as_str())) {
        return err("Invalid provider request envelope");
    }
    let Some(permit) =
        params.get("permit").and_then(serde_json::Value::as_str).filter(|token| token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()))
    else {
        return err("Invalid provider permit");
    };
    if !app.hooks.codex_connection.as_ref().is_some_and(|connection| connection.authorizes_provider(permit)) {
        return err("Provider connection was revoked; request was not executed");
    }
    let Some(method) = params.get("method").and_then(serde_json::Value::as_str) else {
        return err("Provider request needs a method");
    };
    // Owned providers use native commands, not deferred synthetic keyboard/mouse input.
    // Manual editor controls and existing explicitly configured controller profiles are unchanged.
    if !["engine.execute", "engine.commands", "ui.menu.invoke", "ui.menu.list", "ui.inspect", "ui.elements", "ui.screenshot.inline", "controller.capabilities"]
        .contains(&method)
    {
        return err("Unsupported provider method; use the native command bridge");
    }
    let Some(body) = params.get("params").filter(|value| value.is_object()) else {
        return err("Provider parameters must be an object");
    };
    let inner = ControlRequest { method: method.to_owned(), params: body.clone(), reply: req.reply.clone() };
    super::handle(app, ctx, &inner)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::{
        CodexConnection, ConnectionStatus,
        provider::{Ownership, ProviderId},
    };
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    struct Host(Arc<Mutex<Ownership>>);
    impl CodexConnection for Host {
        fn connect(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn disconnect(&mut self) {}
        fn poll(&mut self) -> Option<ConnectionStatus> {
            None
        }
        fn authorizes_provider(&self, token: &str) -> bool {
            self.0.lock().unwrap().authorizes_token(token)
        }
    }
    #[test]
    fn queued_old_provider_cannot_edit_after_switch() {
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.session.execute("file.openDemoProject", json!({})).unwrap();
        let shared = Arc::new(Mutex::new(Ownership::default()));
        let old = {
            let mut owner = shared.lock().unwrap();
            let permit = owner.begin(ProviderId::Codex).unwrap();
            owner.ready(&permit).unwrap();
            permit.transport_token()
        };
        app.hooks.codex_connection = Some(Box::new(Host(shared.clone())));
        let (request, _) = ControlRequest::new(
            "provider.execute",
            json!({"permit":old,"method":"engine.execute","params":{"command":"markers.add","params":{"target":"program","time":0,"name":"Must not appear"}}}),
        );
        {
            let mut owner = shared.lock().unwrap();
            owner.select(ProviderId::OpenAiApi).unwrap();
            let next = owner.begin(ProviderId::OpenAiApi).unwrap();
            owner.ready(&next).unwrap();
        }
        let project = app.session.project.clone();
        let undo = app.session.undo_mark();
        let Outcome::Done(reply) = super::dispatch(&mut app, &egui::Context::default(), &request) else {
            panic!("Expected a bounded rejection");
        };
        assert_eq!(reply["ok"], false);
        assert_eq!(app.session.project, project);
        assert_eq!(app.session.undo_mark(), undo);
    }
    #[test]
    fn malformed_and_recursive_envelopes_are_rejected() {
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        let ctx = egui::Context::default();
        for params in [
            json!(null),
            json!({}),
            json!({"permit":"x","method":"provider.execute","params":{}}),
            json!({"permit":"0".repeat(64),"method":"engine.execute","params":{},"extra":true}),
        ] {
            let (request, _) = ControlRequest::new("provider.execute", params);
            let Outcome::Done(reply) = super::dispatch(&mut app, &ctx, &request) else {
                panic!("Expected rejection");
            };
            assert_eq!(reply["ok"], false);
        }
    }
}
