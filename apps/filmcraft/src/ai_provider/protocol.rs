//! Bounded OpenAI Responses function-calling protocol. No credentials or network I/O.
use base64::Engine as _;
use serde::Serialize;
use serde_json::{Value, json};
use std::{collections::HashSet, io::Write};
pub(super) const MAX_BODY: usize = 16 * 1024 * 1024;
pub(super) const MAX_REPLY: usize = 2 * 1024 * 1024;
const MAX_TEXT: usize = 128 * 1024;
const MAX_CALLS: usize = 16;
pub(super) const MAX_INPUT_ITEMS: usize = 256;
pub(super) const MAX_OUTPUT_ITEMS: usize = 64;
#[derive(Clone, Serialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}
#[derive(Clone)]
pub struct Call {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}
pub struct Reply {
    pub text: String,
    pub calls: Vec<Call>,
    pub output: Vec<Value>,
}
struct Limited {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for Limited {
    fn write(&mut self, input: &[u8]) -> std::io::Result<usize> {
        if input.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other("message budget exceeded"));
        }
        self.bytes.extend_from_slice(input);
        Ok(input.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub(super) fn bounded_json(value: &impl Serialize, limit: usize) -> Result<Vec<u8>, String> {
    let mut out = Limited { bytes: Vec::new(), limit };
    serde_json::to_writer(&mut out, value).map_err(|_| "API message exceeds its byte budget")?;
    Ok(out.bytes)
}
/// Measure serialized data without allocating another copy of a large history.
pub(super) fn json_len(value: &impl Serialize) -> Result<usize, String> {
    struct Counter(usize);
    impl Write for Counter {
        fn write(&mut self, input: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.checked_add(input.len()).ok_or_else(|| std::io::Error::other("JSON size overflow"))?;
            Ok(input.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value).map_err(|_| "Unable to measure API message")?;
    Ok(counter.0)
}
fn identifier(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
}
pub fn request(model: &str, instructions: &str, input: Vec<Value>, tools: &[Tool]) -> Result<Vec<u8>, String> {
    request_with_policy(model, instructions, input, tools, false)
}
pub fn finishing_request(model: &str, instructions: &str, input: Vec<Value>, tools: &[Tool]) -> Result<Vec<u8>, String> {
    const FOOTER: &str = "\n\nFILMCRAFT WORK LIMIT\nNo further tool operations are allowed in this response. Summarize only completed results and images actually received, and clearly identify unfinished work. Do not claim the entire task is complete without evidence. The host will explain how to continue remaining work; completed operations must not be replayed.";
    let capacity = instructions.len().checked_add(FOOTER.len()).filter(|size| *size <= 64 * 1024).ok_or("API finishing instructions exceed budget")?;
    let mut finishing_instructions = String::with_capacity(capacity);
    finishing_instructions.push_str(instructions);
    finishing_instructions.push_str(FOOTER);
    request_with_policy(model, &finishing_instructions, input, tools, true)
}
fn request_with_policy(model: &str, instructions: &str, input: Vec<Value>, tools: &[Tool], finishing: bool) -> Result<Vec<u8>, String> {
    let valid_model =
        !model.is_empty() && model.len() <= 256 && model.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'));
    if !valid_model || instructions.len() > 64 * 1024 || input.len() > MAX_INPUT_ITEMS || tools.len() > 32 {
        return Err("Invalid model, instructions or API message count".into());
    }
    let mut names = HashSet::new();
    let mut definitions = Vec::with_capacity(tools.len());
    for tool in tools {
        if !identifier(&tool.name, 64) || !names.insert(tool.name.as_str()) || tool.description.len() > 8192 || !tool.parameters.is_object() {
            return Err("Invalid or duplicate API tool definition".into());
        }
        bounded_json(&tool.parameters, 16 * 1024)?;
        definitions.push(json!({"type":"function","name":tool.name,"description":tool.description,"parameters":tool.parameters,"strict":false}));
    }
    // Validate size before constructing the request; selected-file payloads remain bounded.
    bounded_json(&input, MAX_BODY)?;
    let mut body = json!({"model":model,"instructions":instructions,"input":input,"tools":definitions,"parallel_tool_calls":false,"store":false,"include":["reasoning.encrypted_content"],"max_output_tokens":8192});
    if finishing {
        body["tool_choice"] = json!("none");
        body["max_output_tokens"] = json!(2048);
    }
    bounded_json(&body, MAX_BODY)
}
pub fn user_input(text: &str, images: &[String]) -> Result<Value, String> {
    if text.trim().is_empty()
        || text.len() > 128 * 1024
        || images.len() > 4
        || images.iter().any(|image| image.len() > 3 * 1024 * 1024 || !image.starts_with("data:image/png;base64,"))
    {
        return Err("Invalid text or image input budget".into());
    }
    let mut content = vec![json!({"type":"input_text","text":text})];
    content.extend(images.iter().map(|image| json!({"type":"input_image","image_url":image})));
    Ok(json!({"role":"user","content":content}))
}
pub fn parse_reply(bytes: &[u8], allowed: &[&str]) -> Result<Reply, String> {
    if bytes.len() > MAX_REPLY || allowed.len() > 32 {
        return Err("API response exceeds its byte or tool budget".into());
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|_| "API returned malformed JSON")?;
    match value.get("status").and_then(Value::as_str) {
        Some("completed") => {}
        Some("incomplete") => return Err("API response was incomplete; no tool calls were executed".into()),
        _ => return Err("API did not return a completed response; no tool calls were executed".into()),
    }
    let output =
        value.get("output").and_then(Value::as_array).filter(|items| items.len() <= MAX_OUTPUT_ITEMS).ok_or("API output is missing or exceeds 64 items")?;
    let mut text = String::new();
    let mut calls = Vec::new();
    let mut ids = HashSet::new();
    for item in output {
        match item.get("type").and_then(Value::as_str) {
            Some("reasoning") => {}
            Some("message") => {
                if item.get("role").and_then(Value::as_str).is_some_and(|role| role != "assistant") {
                    return Err("Invalid API output message role".into());
                }
                let content = item.get("content").and_then(Value::as_array).filter(|parts| parts.len() <= 64).ok_or("Invalid API message content")?;
                for part in content {
                    match part.get("type").and_then(Value::as_str) {
                        Some("output_text") => {
                            let more = part.get("text").and_then(Value::as_str).ok_or("Invalid API text")?;
                            if text.len().saturating_add(more.len()).saturating_add(1) > MAX_TEXT {
                                return Err("API text exceeds 128 KiB".into());
                            }
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(more);
                        }
                        Some("refusal") => return Err("The API declined this request; no tool calls were executed".into()),
                        _ => return Err("Unsupported API output modality; no tool calls were executed".into()),
                    }
                }
            }
            Some("function_call") => {
                if calls.len() >= MAX_CALLS {
                    return Err("API returned too many tool calls".into());
                }
                let name = item.get("name").and_then(Value::as_str).filter(|name| allowed.contains(name)).ok_or("API requested an unadvertised tool")?;
                let id = item.get("call_id").and_then(Value::as_str).filter(|id| identifier(id, 128)).ok_or("Invalid API tool call identity")?;
                if !ids.insert(id) {
                    return Err("API repeated a tool call identity; no calls were executed".into());
                }
                let raw = item.get("arguments").and_then(Value::as_str).filter(|raw| raw.len() <= 64 * 1024).ok_or("Invalid API tool arguments")?;
                let arguments: Value = serde_json::from_str(raw).map_err(|_| "Malformed API tool arguments")?;
                if !arguments.is_object() {
                    return Err("API tool arguments must be an object".into());
                }
                calls.push(Call { id: id.into(), name: name.into(), arguments });
            }
            _ => return Err("Unsupported API response item; no tool calls were executed".into()),
        }
    }
    Ok(Reply { text, calls, output: output.clone() })
}
/// Native MCP pixels are typed image inputs, never an opaque base64 text dump.
pub(super) fn png_url(data: &str) -> Result<String, String> {
    if data.is_empty() || data.len() > 3 * 1024 * 1024 {
        return Err("Native PNG exceeds its encoded image budget".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|_| "Native PNG has malformed base64")?;
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|_| "Native PNG is corrupt or exceeds its decode budget")?;
    let (width, height) = (decoded.width(), decoded.height());
    if width == 0 || height == 0 || width > 4096 || height > 4096 || u64::from(width).saturating_mul(u64::from(height)) > 16 * 1024 * 1024 {
        return Err("Native PNG dimensions exceed the image budget".into());
    }
    Ok(format!("data:image/png;base64,{data}"))
}
pub fn tool_result(call: &Call, result: &Value) -> Result<Value, String> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err("Native tool returned an error; it was not replayed".into());
    }
    let output = if let Some(content) = result.get("content") {
        let content = content.as_array().filter(|parts| !parts.is_empty() && parts.len() <= 64).ok_or("Invalid native MCP content list")?;
        let mut outputs = Vec::new();
        let mut text_bytes = 0usize;
        let mut images = 0usize;
        for part in content {
            match part.get("type").and_then(Value::as_str) {
                Some("text") => {
                    let text = part.get("text").and_then(Value::as_str).ok_or("Invalid native text output")?;
                    text_bytes = text_bytes.saturating_add(text.len());
                    if text_bytes > 64 * 1024 {
                        return Err("Native text output exceeds 64 KiB".into());
                    }
                    outputs.push(json!({"type":"input_text","text":text}));
                }
                Some("image") => {
                    images = images.saturating_add(1);
                    if images > 4 || part.get("mimeType").and_then(Value::as_str) != Some("image/png") {
                        return Err("Only up to four bounded native PNG outputs are supported".into());
                    }
                    let data = part.get("data").and_then(Value::as_str).ok_or("Native image has no data")?;
                    outputs.push(json!({"type":"input_image","image_url":png_url(data)?,"detail":"auto"}));
                }
                _ => return Err("Unsupported native output modality; no automatic replay was attempted".into()),
            }
        }
        if let Some(structured) = result.get("structuredContent") {
            let bytes = bounded_json(structured, (64 * 1024_usize).saturating_sub(text_bytes))?;
            let text = String::from_utf8(bytes).map_err(|_| "Invalid structured native output encoding")?;
            outputs.push(json!({"type":"input_text","text":text}));
        }
        Value::Array(outputs)
    } else {
        Value::String(String::from_utf8(bounded_json(result, 64 * 1024)?).map_err(|_| "Invalid tool output encoding")?)
    };
    Ok(json!({"type":"function_call_output","call_id":call.id,"output":output}))
}
/// HTTP diagnostics never echo arbitrary provider bodies, request headers or credentials.
pub fn http_error(status: u16) -> &'static str {
    match status {
        401 | 403 => "API authentication or access failed; check your environment API key and model access",
        408 | 504 => "API timed out; inspect FilmCraft state before repeating edits",
        429 => "API rate limit or quota reached; no automatic request replay was attempted",
        500..=599 => "API service failed; no automatic request replay was attempted",
        _ => "API request failed; check the configured model and supported input capabilities",
    }
}
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    fn body(output: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({"status":"completed","output":output})).unwrap()
    }
    fn call(id: &str) -> Value {
        json!({"type":"function_call","name":"command_run","call_id":id,"arguments":"{\"id\":\"markers.add\",\"params\":{}}"})
    }
    #[test]
    fn model_ids_allow_official_fine_tuned_syntax_without_weakening_tool_ids() {
        let model = "ft:synthetic-model:fixture-org:suffix:modelid";
        let body = request(model, "", vec![], &[]).unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["model"], model);
        assert!(request(&"a".repeat(257), "", vec![], &[]).is_err());
        for bad in ["bad model", "bad\nmodel", "bad/model"] {
            assert!(request(bad, "", vec![], &[]).is_err());
        }
        assert!(request(model, "", vec![], &[Tool { name: "bad:tool".into(), description: String::new(), parameters: json!({}) }]).is_err());
    }
    #[test]
    fn finishing_policy_disables_tools_and_preserves_bounded_stateless_history() {
        let input = vec![user_input("Inspect", &[]).unwrap(); MAX_INPUT_ITEMS];
        let tools = [Tool { name: "command_run".into(), description: "Native".into(), parameters: json!({"type":"object"}) }];
        let body: Value = serde_json::from_slice(&finishing_request("test-model", "Guide", input.clone(), &tools).unwrap()).unwrap();
        assert_eq!(body["tool_choice"], "none");
        assert_eq!(body["store"], false);
        assert_eq!(body["parallel_tool_calls"], false);
        assert_eq!(body["max_output_tokens"], 2048);
        assert_eq!(body["input"].as_array().unwrap().len(), MAX_INPUT_ITEMS);
        assert_eq!(body["tools"][0]["name"], "command_run");
        assert!(body["instructions"].as_str().unwrap().contains("No further tool operations"));
        assert!(finishing_request("test-model", &"x".repeat(64 * 1024), vec![], &tools).is_err());
        assert!(request("test-model", "Guide", vec![json!({}); MAX_INPUT_ITEMS + 1], &tools).is_err());
        let normal: Value = serde_json::from_slice(&request("test-model", "Guide", vec![], &tools).unwrap()).unwrap();
        assert!(normal.get("tool_choice").is_none());
    }
    #[test]
    fn normal_reply_and_calls_are_parsed_before_execution() {
        let parsed = parse_reply(&body(json!([{"type":"message","content":[{"type":"output_text","text":"Plan"}]},call("call_a")])), &["command_run"]).unwrap();
        assert_eq!(parsed.text, "Plan");
        assert_eq!(parsed.calls.len(), 1);
        assert_eq!(parsed.calls[0].name, "command_run");
        assert_eq!(parsed.calls[0].arguments["id"], "markers.add");
        assert_eq!(tool_result(&parsed.calls[0], &json!({"ok":true})).unwrap()["call_id"], "call_a");
    }
    #[test]
    fn duplicates_unknown_tools_and_incomplete_messages_never_yield_calls() {
        assert!(parse_reply(&body(json!([call("call_a"), call("call_a")])), &["command_run"]).is_err());
        assert!(parse_reply(&body(json!([call("call_a")])), &["console_memory"]).is_err());
        assert!(parse_reply(br#"{"status":"incomplete","output":[]}"#, &[]).is_err());
    }
    #[test]
    fn malformed_mutated_and_oversized_responses_are_safe() {
        let valid = body(json!([call("call_a")]));
        for n in 0..valid.len() {
            assert!(std::panic::catch_unwind(|| parse_reply(&valid[..n], &["command_run"])).is_ok());
        }
        for n in (0..valid.len()).step_by(7) {
            let mut changed = valid.clone();
            changed[n] ^= 255;
            assert!(std::panic::catch_unwind(|| parse_reply(&changed, &["command_run"])).is_ok());
        }
        assert!(parse_reply(&vec![0; MAX_REPLY + 1], &[]).is_err());
    }
    #[test]
    fn requests_use_stateless_explicit_tools_and_bounded_inputs() {
        let input = user_input("Inspect", &[]).unwrap();
        let encoded = request("test-model", "Guide", vec![input], &[]).unwrap();
        let request: Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(request["store"], false);
        assert_eq!(request["parallel_tool_calls"], false);
        assert!(user_input("Inspect", &["https://private.example/image".into()]).is_err());
        assert!(self::request("bad\nmodel", "", vec![], &[]).is_err());
    }
    #[test]
    fn errors_do_not_include_provider_bodies_or_keys() {
        assert!(http_error(429).contains("quota"));
        assert!(http_error(401).contains("authentication"));
    }
    pub(crate) fn test_png() -> String {
        let mut bytes = std::io::Cursor::new(Vec::new());
        let rgba = image::RgbaImage::from_pixel(2, 2, image::Rgba([12, 34, 56, 100]));
        image::DynamicImage::ImageRgba8(rgba).write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        base64::engine::general_purpose::STANDARD.encode(bytes.into_inner())
    }
    #[test]
    fn native_images_are_typed_and_keep_the_original_png_and_transparency() {
        let png = test_png();
        let call = Call { id: "image_1".into(), name: "render_current_frame".into(), arguments: json!({}) };
        let output =
            tool_result(&call, &json!({"content":[{"type":"text","text":"Native frame"},{"type":"image","mimeType":"image/png","data":png}]})).unwrap();
        assert_eq!(output["output"][0]["type"], "input_text");
        assert_eq!(output["output"][1]["type"], "input_image");
        assert_eq!(output["output"][1]["image_url"], format!("data:image/png;base64,{png}"));
    }
    #[test]
    fn hostile_native_image_outputs_are_rejected_without_panics() {
        let call = Call { id: "image_1".into(), name: "render_current_frame".into(), arguments: json!({}) };
        let valid = test_png();
        for data in ["!bad!".to_string(), "x".repeat(3 * 1024 * 1024 + 1), valid.get(..valid.len() / 2).unwrap().to_string()] {
            let result = json!({"content":[{"type":"image","mimeType":"image/png","data":data}]});
            assert!(std::panic::catch_unwind(|| tool_result(&call, &result)).unwrap().is_err());
        }
        for part in [json!({"type":"audio","data":"abc"}), json!({"type":"image","mimeType":"image/jpeg","data":valid})] {
            assert!(tool_result(&call, &json!({"content":[part]})).is_err());
        }
        let raw = base64::engine::general_purpose::STANDARD.decode(test_png()).unwrap();
        for length in 0..raw.len() {
            let data = base64::engine::general_purpose::STANDARD.encode(&raw[..length]);
            let result = json!({"content":[{"type":"image","mimeType":"image/png","data":data}]});
            assert!(std::panic::catch_unwind(|| tool_result(&call, &result)).is_ok());
        }
        for index in (0..raw.len()).step_by(7) {
            let mut changed = raw.clone();
            changed[index] ^= 255;
            let data = base64::engine::general_purpose::STANDARD.encode(changed);
            let result = json!({"content":[{"type":"image","mimeType":"image/png","data":data}]});
            assert!(std::panic::catch_unwind(|| tool_result(&call, &result)).is_ok());
        }
        let part = json!({"type":"image","mimeType":"image/png","data":test_png()});
        assert!(tool_result(&call, &json!({"content":vec![part;5]})).is_err());
    }
}
