//! Capture provenance is the matching native tool identity, never model-supplied metadata.
use super::protocol::Call;
use serde_json::{Value, json};
use std::collections::HashMap;
pub const DISCARDED: &str = "FilmCraft capture pixels are not retained in this history. Request a fresh capture if needed.";
fn selected_reference(call: &Call) -> bool {
    call.name == "reference_image"
        && call.arguments.as_object().is_some_and(|args| {
            args.len() == 2
                && args.get("snapshot_id").and_then(Value::as_u64).is_some_and(|id| id > 0)
                && args.get("document_epoch").and_then(Value::as_u64).is_some()
        })
}
fn has_image(result: &Value) -> bool {
    result.get("output").and_then(Value::as_array).is_some_and(|parts| parts.iter().any(|part| part.get("type").and_then(Value::as_str) == Some("input_image")))
}
/// Selected snapshots are admitted by reference_image; other native images are ephemeral.
/// Discard all arbitrary capture text/structured data too, so duplicated base64 cannot hide there.
pub fn tool_result(call: &Call, delivered: &Value) -> Value {
    if selected_reference(call) || (!matches!(call.name.as_str(), "render_current_frame" | "capture_timeline" | "capture_source") && !has_image(delivered)) {
        return delivered.clone();
    }
    let mut metadata = serde_json::Map::new();
    if let Some(parts) = delivered.get("output").and_then(Value::as_array) {
        for part in parts {
            if let Some(text) = part.get("text").and_then(Value::as_str)
                && let Ok(Value::Object(object)) = serde_json::from_str::<Value>(text)
            {
                for key in ["width", "height", "revision", "activeSequence", "documentEpoch", "playhead", "retainedFiles"] {
                    if let Some(value) = object.get(key).filter(|v| v.is_i64() || v.is_u64()) {
                        metadata.insert(key.into(), value.clone());
                    }
                }
            }
        }
    }
    json!({"type":"function_call_output","call_id":call.id,"output":[{"type":"input_text","text":DISCARDED},{"type":"input_text","text":Value::Object(metadata).to_string()}]})
}
/// Defense for old checkpoints and all save paths. User-selected input images are untouched.
pub fn discard_captures(input: &mut [Value]) -> bool {
    let mut calls = HashMap::new();
    let mut changed = false;
    for item in input {
        match item.get("type").and_then(Value::as_str) {
            Some("function_call") => {
                if let (Some(id), Some(name)) = (item.get("call_id").and_then(Value::as_str), item.get("name").and_then(Value::as_str)) {
                    let arguments = item.get("arguments").and_then(Value::as_str).and_then(|s| serde_json::from_str(s).ok()).unwrap_or(Value::Null);
                    calls.insert(id.to_owned(), Call { id: id.into(), name: name.into(), arguments });
                }
            }
            Some("function_call_output") => {
                let id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
                let fallback = Call { id: id.into(), name: String::new(), arguments: Value::Null };
                let retained = tool_result(calls.get(id).unwrap_or(&fallback), item);
                if *item != retained {
                    *item = retained;
                    changed = true;
                }
            }
            _ => {}
        }
    }
    changed
}
