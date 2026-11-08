//! Observe completed commands as data; this module has no command dispatcher.
use super::*;
use serde::Serialize;
use serde_json::{Value, json};
use std::io::{self, Write};
struct Budget(Vec<u8>);
impl Write for Budget {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > 1024 {
            return Err(io::Error::other("checkpoint parameter budget"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn redact(value: &mut Value, depth: usize) {
    if depth >= 16 {
        *value = json!({"omitted":true,"reason":"depth limit"});
        return;
    }
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                let key: String = key.chars().filter(|c| c.is_ascii_alphanumeric()).flat_map(char::to_lowercase).collect();
                if ["token", "secret", "apikey", "authorization", "password", "credential"].iter().any(|part| key.contains(part)) {
                    *value = json!("[redacted]");
                } else {
                    redact(value, depth.saturating_add(1));
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact(value, depth.saturating_add(1));
            }
        }
        _ => {}
    }
}
fn parameters(params: &impl Serialize) -> Value {
    let mut output = Budget(Vec::new());
    if serde_json::to_writer(&mut output, params).is_err() {
        return json!({"omitted":true,"reason":"parameters exceed 1024 serialized bytes"});
    }
    match serde_json::from_slice::<Value>(&output.0) {
        Ok(mut value) if value.is_object() => {
            redact(&mut value, 0);
            if serde_json::to_writer(&mut Budget(Vec::new()), &value).is_err() {
                return json!({"omitted":true,"reason":"redacted parameters exceed budget"});
            }
            value
        }
        _ => json!({"omitted":true,"reason":"invalid command parameters"}),
    }
}
pub(super) fn record(app: &mut FilmcraftApp, command: &str, params: &impl Serialize, before: u64, result: &Result<Value, String>) {
    outcome(app, command, params, before, if result.is_ok() { CommandState::Executed } else { CommandState::Failed }, result.as_ref().err());
}
pub(super) fn proposed(app: &mut FilmcraftApp, command: &str, params: &impl Serialize) {
    let before = app.session.revision;
    outcome(app, command, params, before, CommandState::Proposed, None);
}
fn outcome(app: &mut FilmcraftApp, command: &str, params: &impl Serialize, before: u64, state: CommandState, error: Option<&String>) {
    let Some(runtime) = app.assistants.current.as_mut() else {
        return;
    };
    let mut next = runtime.snapshot().clone();
    if next.commands.len() >= 64 {
        next.commands.remove(0);
    }
    next.commands.push(CommandRecord {
        command: bounded(command, 256),
        params: parameters(params),
        state,
        revision_before: before,
        revision_after: app.session.revision,
        error: error.map(|e| bounded(e, 512)),
    });
    match runtime.update(next, Instant::now()) {
        Ok(_) => {
            super::changed(app);
        }
        Err(error) => {
            app.ui.status = format!("Command completed, but assistant memory could not record its outcome: {error}");
            app.assistants.error = Some(error);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credentials_and_oversized_parameters_do_not_enter_command_memory() {
        let value = parameters(&json!({"clip":5,"nested":{"api_key":"private","controlToken":"private","name":"keep"}}));
        assert_eq!(value["clip"], 5);
        assert_eq!(value["nested"]["api_key"], "[redacted]");
        assert_eq!(value["nested"]["controlToken"], "[redacted]");
        assert_eq!(value["nested"]["name"], "keep");
        let omitted = parameters(&json!({"text":"x".repeat(8192)}));
        assert_eq!(omitted["omitted"], true);
    }
}
