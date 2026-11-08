//! Preserve actionable error details, never raw protocol frames or credentials.
use serde_json::Value;

pub(super) fn safe_text(text: &str) -> String {
    let mut end = text.len().min(2048);
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    let text = text.get(..end).unwrap_or_default();
    let mut out = String::new();
    let mut hide_next = false;
    for word in text.split_whitespace().take(128) {
        let lowered = word.trim_matches(|c: char| !c.is_alphanumeric()).to_ascii_lowercase();
        let sensitive = lowered.contains("credential")
            || lowered.contains("password")
            || lowered.contains("secret")
            || lowered.contains("api-key")
            || lowered.starts_with("sk-")
            || lowered.starts_with("eyj")
            || lowered.contains("api_key=")
            || lowered.contains("apikey=")
            || lowered.contains("token=")
            || lowered.contains("authorization:")
            || lowered.starts_with("http://")
            || lowered.starts_with("https://");
        let next = lowered.contains("bearer")
            || lowered.starts_with("authorization")
            || matches!(lowered.as_str(), "api_key" | "password" | "secret" | "credential" | "token");
        let shown = if hide_next || sensitive || next { "[redacted]" } else { word };
        hide_next = next;
        if out.len().saturating_add(shown.len()).saturating_add(1) > 512 {
            out.push_str(" …");
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        for ch in shown.chars().filter(|c| !c.is_control()) {
            out.push(ch);
        }
    }
    out
}
pub(super) fn error_details(value: &Value) -> String {
    let error = value.get("error").unwrap_or(value);
    let code = error.get("code").and_then(|v| v.as_str().map(safe_text).or_else(|| v.as_i64().map(|n| n.to_string()))).or_else(|| {
        error.get("codexErrorInfo").and_then(|v| v.as_str().map(safe_text).or_else(|| v.as_object().and_then(|o| o.keys().next().map(|key| safe_text(key)))))
    });
    let message = error.get("message").and_then(Value::as_str).map(safe_text).unwrap_or_else(|| "No error message supplied".into());
    let code = code.map(|v| safe_text(&v)).unwrap_or_else(|| "unspecified".into());
    let http_status = error.get("codexErrorInfo").and_then(Value::as_object).and_then(|info| {
        ["httpConnectionFailed", "responseStreamConnectionFailed", "responseStreamDisconnected", "responseTooManyFailedAttempts"]
            .into_iter()
            .find_map(|kind| info.get(kind).and_then(|value| value.get("httpStatusCode")).and_then(Value::as_u64).filter(|status| (100..=599).contains(status)))
    });
    match http_status {
        Some(status) => format!("Codex error ({code}, HTTP {status}): {message}"),
        None => format!("Codex error ({code}): {message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn structured_stream_status_is_preserved_without_logging_response_headers_or_details() {
        let input = json!({"error":{"codexErrorInfo":{"responseStreamDisconnected":{"httpStatusCode":503}},"message":"Reconnecting... 2/5","additionalDetails":"set-cookie: private-cookie; Authorization: private-value"}});
        let detail = error_details(&input);
        assert!(detail.contains("responseStreamDisconnected, HTTP 503"));
        assert!(detail.contains("Reconnecting"));
        assert!(!detail.contains("private"));
        for status in [json!(null), json!(-1), json!(0), json!(600), json!(u64::MAX), json!("503"), json!(true)] {
            let detail = error_details(&json!({"error":{"codexErrorInfo":{"responseStreamDisconnected":{"httpStatusCode":status}},"message":"retry"}}));
            assert!(!detail.contains("HTTP"));
        }
    }
    #[test]
    fn errors_keep_codes_and_explanations_but_redact_credentials() {
        let message = error_details(
            &json!({"error":{"code":"server_overloaded","message":"Server busy; retry later. Bearer secret-value sk-proj-private https://example.test/?token=private"}}),
        );
        assert!(message.contains("server_overloaded"));
        assert!(message.contains("Server busy"));
        for secret in ["secret-value", "sk-proj-private", "token=private"] {
            assert!(!message.contains(secret));
        }
        assert!(safe_text(&"🙂".repeat(50000)).len() < 520);
        assert!(!error_details(&json!({"unexpected":"never log arbitrary fields"})).contains("arbitrary fields"));
    }
}

/// Only fixed protocol methods can enter setup breadcrumbs; never log a raw frame.
#[derive(Clone, Copy)]
pub(super) enum SetupMethod {
    Initialize,
    Config,
    Start,
    Resume,
    Catalog,
}
impl SetupMethod {
    pub fn name(self) -> &'static str {
        match self {
            Self::Initialize => "initialize",
            Self::Config => "config/read",
            Self::Start => "thread/start",
            Self::Resume => "thread/resume",
            Self::Catalog => "mcpServerStatus/list",
        }
    }
}
#[derive(Clone, Copy)]
pub(super) enum SetupOutcome {
    Pending,
    ReturnedOk,
    ReturnedError,
}
pub(super) fn setup_line(method: SetupMethod, id: u64, outcome: SetupOutcome, elapsed: std::time::Duration) -> String {
    let outcome = match outcome {
        SetupOutcome::Pending => "pending",
        SetupOutcome::ReturnedOk => "returned_ok",
        SetupOutcome::ReturnedError => "returned_error",
    };
    format!(
        "Codex setup {} method={} request={} outcome={} elapsedMs={:.1}",
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        method.name(),
        id,
        outcome,
        elapsed.as_secs_f64() * 1000.0
    )
}

#[cfg(test)]
mod setup_tests {
    use super::*;
    #[test]
    fn setup_breadcrumbs_are_fixed_metadata_and_bounded_even_at_counter_limits() {
        for method in [SetupMethod::Initialize, SetupMethod::Config, SetupMethod::Start, SetupMethod::Resume, SetupMethod::Catalog] {
            for outcome in [SetupOutcome::Pending, SetupOutcome::ReturnedOk, SetupOutcome::ReturnedError] {
                let line = setup_line(method, u64::MAX, outcome, std::time::Duration::MAX);
                assert!(line.len() < 256, "{line}");
                assert!(line.contains(method.name()));
                assert!(!line.contains("params"));
                assert!(!line.contains("threadId"));
            }
        }
    }
}
