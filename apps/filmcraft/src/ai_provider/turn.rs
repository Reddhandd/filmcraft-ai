//! Bounded API turn execution, reusable with fake HTTP/native backends.
use super::protocol::{self, Call, Tool};
use serde_json::Value;
use std::{
    collections::HashSet,
    sync::atomic::{AtomicBool, Ordering},
};
pub(super) const MAX_TOOL_ROUNDS: usize = 24;
// Reserve a complete final response and one explicit host pause notice.
// Leave space for final output, host notice, request metadata and tool definitions.
const WORK_BYTE_LIMIT: usize = protocol::MAX_BODY - protocol::MAX_REPLY - 1024 * 1024;
const TOOL_RESULT_LIMIT: usize = 512 * 1024;
const HISTORY_PAUSE_NOTICE: &str = "FilmCraft paused at this conversation's history limit. Completed operations are preserved and will not be replayed. Save reusable instructions with Save Assistant and start a New Conversation for remaining work.";
const WORK_ITEM_LIMIT: usize = protocol::MAX_INPUT_ITEMS - protocol::MAX_OUTPUT_ITEMS - 1;
const HISTORY_LIMIT_NOTICE: &str = "FilmCraft reached this conversation's history limit. No new request or edit was submitted. Your completed conversation is preserved; save any reusable instructions with Save Assistant and start a New Conversation for more work.";
pub trait Responses {
    fn post(&self, body: &[u8], cancel: &AtomicBool) -> Result<Vec<u8>, String>;
}
impl Responses for super::http::ApiHttp {
    fn post(&self, body: &[u8], cancel: &AtomicBool) -> Result<Vec<u8>, String> {
        self.post(body, cancel)
    }
}
pub trait Tools {
    fn definitions(&self) -> Result<Vec<Tool>, String>;
    /// Recheck ownership immediately before dispatch; native project/revision guards still apply.
    fn is_current(&self) -> bool;
    /// Any rejected/uncertain native operation stops this turn, rather than inviting model replay.
    fn call_once(&mut self, call: &Call) -> Result<Value, String>;
}
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Conversation {
    input: Vec<Value>,
    seen_calls: HashSet<String>,
}
impl Conversation {
    /// Saved tool state is untrusted input; validate the complete graph before any request.
    pub fn validate(&self, tools: &[Tool]) -> Result<(), String> {
        if self.input.len() > protocol::MAX_INPUT_ITEMS
            || self.seen_calls.len() > 1024
            || self
                .seen_calls
                .iter()
                .any(|id| id.is_empty() || id.len() > 128 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')))
        {
            return Err("Saved API history exceeds its identity or item budget".into());
        }
        protocol::bounded_json(&self.input, 16 * 1024 * 1024)?;
        let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
        let mut pending = HashSet::new();
        let mut history_calls = HashSet::new();
        let mut readable_final = false;
        for item in &self.input {
            if item.get("role").and_then(Value::as_str) == Some("user") && item.get("type").is_none() {
                if !pending.is_empty() {
                    return Err("Saved API turn has missing tool results".into());
                }
                let content = item
                    .get("content")
                    .and_then(Value::as_array)
                    .filter(|parts| !parts.is_empty() && parts.len() <= 5)
                    .ok_or("Malformed saved API user message")?;
                let mut text = String::new();
                let mut images = Vec::new();
                for part in content {
                    match part.get("type").and_then(Value::as_str) {
                        Some("input_text") if text.is_empty() => {
                            text = part.get("text").and_then(Value::as_str).ok_or("Malformed saved API input text")?.into()
                        }
                        Some("input_image") => images.push(part.get("image_url").and_then(Value::as_str).ok_or("Malformed saved API input image")?.into()),
                        _ => return Err("Unsupported saved API input modality".into()),
                    }
                }
                protocol::user_input(&text, &images)?;
                for image in images {
                    protocol::png_url(image.strip_prefix("data:image/png;base64,").ok_or("Unsupported saved API input image")?)?;
                }
                readable_final = false;
                continue;
            }
            match item.get("type").and_then(Value::as_str) {
                Some("function_call_output") => {
                    readable_final = false;
                    let id = item.get("call_id").and_then(Value::as_str).ok_or("Saved API tool result has no identity")?;
                    if !pending.remove(id) {
                        return Err("Saved API tool result is duplicated or orphaned".into());
                    }
                    let output = item.get("output").ok_or("Saved API tool result is missing")?;
                    if let Some(text) = output.as_str() {
                        if text.len() > 64 * 1024 {
                            return Err("Saved API tool text exceeds its budget".into());
                        }
                    } else {
                        let parts = output.as_array().filter(|parts| !parts.is_empty() && parts.len() <= 65).ok_or("Malformed saved API tool output")?;
                        let mut text_bytes = 0usize;
                        let mut images = 0usize;
                        for part in parts {
                            match part.get("type").and_then(Value::as_str) {
                                Some("input_text") => {
                                    text_bytes =
                                        text_bytes.saturating_add(part.get("text").and_then(Value::as_str).ok_or("Malformed saved API tool text")?.len());
                                    if text_bytes > 64 * 1024 {
                                        return Err("Saved API tool text exceeds its budget".into());
                                    }
                                }
                                Some("input_image") => {
                                    let data = part
                                        .get("image_url")
                                        .and_then(Value::as_str)
                                        .and_then(|url| url.strip_prefix("data:image/png;base64,"))
                                        .ok_or("Unsupported saved API tool image")?;
                                    images = images.saturating_add(1);
                                    if images > 4 {
                                        return Err("Saved API tool images exceed their budget".into());
                                    }
                                    protocol::png_url(data)?;
                                }
                                _ => return Err("Unsupported saved API tool output modality".into()),
                            }
                        }
                    }
                }
                Some("message" | "reasoning" | "function_call") => {
                    if item.get("role").and_then(Value::as_str).is_some_and(|role| role != "assistant") {
                        return Err("Invalid saved API assistant role".into());
                    }
                    let parsed =
                        protocol::parse_reply(&protocol::bounded_json(&serde_json::json!({"status":"completed","output":[item]}), 2 * 1024 * 1024)?, &names)?;
                    if !parsed.text.trim().is_empty() {
                        readable_final = true;
                    }
                    for call in parsed.calls {
                        readable_final = false;
                        if !self.seen_calls.contains(&call.id) || !history_calls.insert(call.id.clone()) || !pending.insert(call.id) {
                            return Err("Saved API call identity is missing or duplicated".into());
                        }
                    }
                }
                _ => return Err("Unsupported saved API history item".into()),
            }
        }
        if !pending.is_empty() || (!self.input.is_empty() && !readable_final) {
            return Err("Saved API history is not a completed conversation boundary".into());
        }
        Ok(())
    }

    pub fn discard_captures(&mut self) -> bool {
        super::retention::discard_captures(&mut self.input)
    }
    /// Every checkpoint path uses a sanitized projection, even for legacy/deserialized state.
    pub fn checkpoint(&self) -> Value {
        let mut input = self.input.clone();
        super::retention::discard_captures(&mut input);
        serde_json::json!({"input":input,"seen_calls":self.seen_calls})
    }
    pub fn clear(&mut self) {
        self.input.clear();
    }
    fn request_input(&self, deliveries: &mut Vec<(usize, Value)>) -> Result<Vec<Value>, String> {
        let mut input = self.input.clone();
        for (index, delivery) in deliveries.drain(..) {
            let slot = input.get_mut(index).ok_or("Capture delivery identity is no longer available")?;
            *slot = delivery;
        }
        Ok(input)
    }
    fn finish_at_work_limit(
        &mut self,
        http: &impl Responses,
        tools: &impl Tools,
        model: &str,
        instructions: &str,
        definitions: &[Tool],
        deliveries: &mut Vec<(usize, Value)>,
        history_limit: bool,
        cancel: &AtomicBool,
    ) -> Result<String, String> {
        if cancel.load(Ordering::SeqCst) || !tools.is_current() {
            return Err("API turn cancelled or provider revoked before its finishing response".into());
        }
        let input = self.request_input(deliveries)?;
        // This phase has no native dispatch path, even if a provider ignores tool_choice.
        // A failed summary cannot make already-completed native results uncertain.
        let summary = protocol::finishing_request(model, instructions, input, definitions).and_then(|request| {
            let result = http.post(&request, cancel);
            drop(request); // Capture pixels have a single delivery attempt, including failure.
            result
        });
        if cancel.load(Ordering::SeqCst) || !tools.is_current() {
            return Err("API finishing response discarded after cancellation or provider switch".into());
        }
        let names: Vec<&str> = definitions.iter().map(|tool| tool.name.as_str()).collect();
        let summary = summary.and_then(|bytes| protocol::parse_reply(&bytes, &names));
        let work_notice = "FilmCraft paused this request at its work limit. Completed operations are recorded and will not be replayed automatically. Ask to continue the remaining work in this conversation.";
        let summary = summary.and_then(|reply| {
            if protocol::json_len(&reply.output)? > protocol::MAX_REPLY {
                return Err("Finishing output exceeds its reserved byte budget".into());
            }
            Ok(reply)
        });
        let history_limit = history_limit
            || summary.as_ref().is_ok_and(|reply| {
                self.input.len().saturating_add(reply.output.len()).saturating_add(2) > WORK_ITEM_LIMIT
                    || protocol::json_len(&self.input).unwrap_or(usize::MAX).saturating_add(protocol::json_len(&reply.output).unwrap_or(usize::MAX))
                        > WORK_BYTE_LIMIT
            });
        let notice = if history_limit { HISTORY_PAUSE_NOTICE } else { work_notice };
        let (text, status) = match summary {
            Ok(reply) if reply.calls.is_empty() && !reply.text.trim().is_empty() => {
                if self.input.len().saturating_add(reply.output.len()).saturating_add(1) > protocol::MAX_INPUT_ITEMS {
                    return Err("API finishing response exceeded its reserved history budget".into());
                }
                self.input.extend(reply.output);
                (format!("{notice}\n\n{}", reply.text), notice.to_owned())
            }
            _ => {
                let text = format!(
                    "{notice}\n\nThe finishing summary was unavailable. Inspect current project/history/jobs before continuing; no additional native operations were executed while finishing."
                );
                (text.clone(), text)
            }
        };
        self.input.push(json_status_message(&status));
        Ok(text)
    }
    pub fn run(
        &mut self,
        http: &impl Responses,
        tools: &mut impl Tools,
        model: &str,
        instructions: &str,
        text: &str,
        images: &[String],
        cancel: &AtomicBool,
    ) -> Result<String, String> {
        self.discard_captures();
        let definitions = tools.definitions()?;
        if cancel.load(Ordering::SeqCst) || !tools.is_current() {
            return Err("API turn cancelled or provider revoked".into());
        }
        let user_input = protocol::user_input(text, images)?;
        if self.input.len().saturating_add(1) > WORK_ITEM_LIMIT
            || protocol::json_len(&self.input)?.saturating_add(protocol::json_len(&user_input)?) > WORK_BYTE_LIMIT
        {
            return Ok(HISTORY_LIMIT_NOTICE.into());
        }
        let mut deliveries = Vec::new();
        let names: Vec<&str> = definitions.iter().map(|tool| tool.name.as_str()).collect();
        let mut completed = 0usize;
        let mut attempted = 0usize;
        self.input.push(user_input);
        let result = (|| {
            for _ in 0..MAX_TOOL_ROUNDS {
                if cancel.load(Ordering::SeqCst) || !tools.is_current() {
                    return Err("API turn cancelled or provider revoked".to_string());
                }
                // Only this request owns capture pixels. Replayable RAM is already sanitized.
                let input = self.request_input(&mut deliveries)?;
                let request = protocol::request(model, instructions, input, &definitions)?;
                let response = http.post(&request, cancel);
                drop(request); // Also discard after failed/cancelled/uncertain delivery; never replay.
                let bytes = response?;
                if cancel.load(Ordering::SeqCst) || !tools.is_current() {
                    return Err("Late API response discarded after cancellation or provider switch".into());
                }
                let reply = protocol::parse_reply(&bytes, &names)?;
                // Preserve all output items, including encrypted reasoning, in stateless mode.
                if !reply.calls.is_empty()
                    && (self.input.len().saturating_add(reply.output.len()).saturating_add(reply.calls.len()) > WORK_ITEM_LIMIT
                        || protocol::json_len(&self.input)?
                            .saturating_add(protocol::json_len(&reply.output)?)
                            .saturating_add(reply.calls.len().saturating_mul(TOOL_RESULT_LIMIT + 1))
                            > WORK_BYTE_LIMIT)
                {
                    // None of this response's proposed operations have been admitted.
                    return self.finish_at_work_limit(http, tools, model, instructions, &definitions, &mut deliveries, true, cancel);
                }
                if protocol::json_len(&reply.output)? > protocol::MAX_REPLY {
                    return Err("API output exceeds its reserved byte budget".into());
                }
                self.input.extend(reply.output);
                if reply.calls.is_empty() {
                    if reply.text.trim().is_empty() {
                        return Err("API returned no readable answer".into());
                    }
                    return Ok(reply.text);
                }
                for call in reply.calls {
                    if cancel.load(Ordering::SeqCst) || !tools.is_current() {
                        return Err("API tool ownership was revoked; no further calls were executed".into());
                    }
                    if self.seen_calls.len() >= 1024 {
                        return Err("API session reached its call identity budget; reconnect for a fresh conversation".into());
                    }
                    if !self.seen_calls.insert(call.id.clone()) {
                        return Err("API repeated a tool identity across responses; it was not replayed".into());
                    }
                    attempted = attempted.saturating_add(1);
                    let outcome = tools.call_once(&call)?;
                    completed = completed.saturating_add(1);
                    let delivered = protocol::tool_result(&call, &outcome)?;
                    if protocol::json_len(&delivered)? > TOOL_RESULT_LIMIT {
                        return Err("API tool result exceeds its reserved byte budget".into());
                    }
                    let retained = super::retention::tool_result(&call, &delivered);
                    let index = self.input.len();
                    if retained != delivered {
                        deliveries.push((index, delivered));
                    }
                    self.input.push(retained);
                }
            }
            self.finish_at_work_limit(http, tools, model, instructions, &definitions, &mut deliveries, false, cancel)
        })();
        if let Err(error) = result {
            // Failed protocol/tool conversations cannot be resumed implicitly with partial outputs.
            self.clear();
            if attempted == 0 {
                return Err(error);
            }
            return Err(format!(
                "{error}. {completed} tool calls completed before this stop. Earlier operations may have applied; inspect project/history/jobs before repeating edits."
            ));
        }
        result
    }
}
fn json_status_message(text: &str) -> Value {
    serde_json::json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]})
}
/// Checkpoint uncertainty before any request; a failed/incomplete turn is never auto-resumed.
pub fn run_persisted(
    conversation: &mut Conversation,
    journal: Option<&super::state::Journal>,
    http: &impl Responses,
    tools: &mut impl Tools,
    model: &str,
    instructions: &str,
    text: &str,
    images: &[String],
    cancel: &AtomicBool,
) -> Result<String, String> {
    // Sanitize retained RAM even if the begin checkpoint itself fails.
    conversation.discard_captures();
    if let Some(journal) = journal {
        journal.save(model, true, conversation)?;
    }
    let answer = conversation.run(http, tools, model, instructions, text, images, cancel)?;
    if let Some(journal) = journal {
        journal
            .save(model, false, conversation)
            .map_err(|error| format!("{error}. The answer and any native edits already completed; do not repeat them to recover this checkpoint"))?;
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        cell::RefCell,
        sync::{Arc, atomic::AtomicUsize},
    };
    struct FakeHttp {
        replies: RefCell<std::collections::VecDeque<Result<Vec<u8>, String>>>,
        posts: AtomicUsize,
    }
    impl Responses for FakeHttp {
        fn post(&self, _: &[u8], _: &AtomicBool) -> Result<Vec<u8>, String> {
            self.posts.fetch_add(1, Ordering::SeqCst);
            self.replies.borrow_mut().pop_front().expect("unexpected automatic retry")
        }
    }
    struct Backend {
        calls: usize,
        current: Arc<AtomicBool>,
        revoke_after_first: bool,
        fail: bool,
    }
    impl Tools for Backend {
        fn definitions(&self) -> Result<Vec<Tool>, String> {
            Ok(vec![Tool { name: "command_run".into(), description: "Native commands".into(), parameters: json!({"type":"object"}) }])
        }
        fn is_current(&self) -> bool {
            self.current.load(Ordering::SeqCst)
        }
        fn call_once(&mut self, _: &Call) -> Result<Value, String> {
            self.calls += 1;
            if self.fail {
                return Err("Native operation outcome is unknown".into());
            }
            if self.revoke_after_first {
                self.current.store(false, Ordering::SeqCst);
            }
            Ok(json!({"ok":true}))
        }
    }
    fn reply(output: Value) -> Result<Vec<u8>, String> {
        Ok(serde_json::to_vec(&json!({"status":"completed","output":output})).unwrap())
    }
    fn call(id: &str) -> Value {
        json!({"type":"function_call","name":"command_run","call_id":id,"arguments":"{}"})
    }
    fn backend() -> Backend {
        Backend { calls: 0, current: Arc::new(AtomicBool::new(true)), revoke_after_first: false, fail: false }
    }
    #[test]
    fn one_native_call_then_text_completes_without_replay() {
        let http = FakeHttp {
            replies: RefCell::new(
                [reply(json!([call("call_a")])), reply(json!([{"type":"message","content":[{"type":"output_text","text":"Done from native result"}]}]))].into(),
            ),
            posts: AtomicUsize::new(0),
        };
        let mut backend = backend();
        let text = Conversation::default().run(&http, &mut backend, "test-model", "Guide", "Edit", &[], &AtomicBool::new(false)).unwrap();
        assert!(text.contains("native result"));
        assert_eq!(backend.calls, 1);
        assert_eq!(http.posts.load(Ordering::SeqCst), 2);
    }
    #[test]
    fn near_byte_limit_preserves_completed_history_without_submitting_or_editing() {
        let mut native = backend();
        let mut conversation = Conversation::default();
        for _ in 0..130 {
            conversation.input.push(json_status_message(&"x".repeat(120 * 1024)));
        }
        conversation.validate(&native.definitions().unwrap()).unwrap();
        let before = conversation.checkpoint();
        let http = FakeHttp { replies: RefCell::new([Err("Must not submit another request".into())].into()), posts: AtomicUsize::new(0) };
        let answer = conversation.run(&http, &mut native, "test-model", "Guide", "Continue", &[], &AtomicBool::new(false)).unwrap();
        assert!(answer.contains("history limit"));
        assert_eq!(http.posts.load(Ordering::SeqCst), 0);
        assert_eq!(native.calls, 0);
        assert_eq!(conversation.checkpoint(), before);
        conversation.validate(&native.definitions().unwrap()).unwrap();
    }
    #[test]
    fn large_reasoning_stops_before_native_dispatch_and_reopens_cleanly() {
        let root = tempfile::tempdir().unwrap();
        let mut native = backend();
        let definitions = native.definitions().unwrap();
        let (journal, mut conversation) = super::super::state::Journal::open(root.path(), "project", "test-model", None, &definitions, &[]).unwrap();
        for _ in 0..105 {
            conversation.input.push(json_status_message(&"x".repeat(120 * 1024)));
        }
        conversation.validate(&definitions).unwrap();
        let id = journal.id().to_owned();
        let http = RecordedHttp {
            replies: RefCell::new(
                [
                    reply(json!([{"type":"reasoning","encrypted_content":"x".repeat(protocol::MAX_REPLY - 2048)},call("not_admitted")])),
                    reply(json!([json_status_message("Earlier work preserved; proposed edit not executed")])),
                ]
                .into(),
            ),
            requests: RefCell::new(vec![]),
            cancel_on_finish: false,
        };
        let answer =
            run_persisted(&mut conversation, Some(&journal), &http, &mut native, "test-model", "Guide", "Continue", &[], &AtomicBool::new(false)).unwrap();
        assert!(answer.contains("history limit"));
        assert_eq!(native.calls, 0);
        assert_eq!(http.requests.borrow().len(), 2);
        assert_eq!(http.requests.borrow()[1]["tool_choice"], "none");
        assert!(!conversation.seen_calls.contains("not_admitted"));
        conversation.validate(&definitions).unwrap();
        drop(journal);
        let (_, restored) = super::super::state::Journal::open(root.path(), "project", "test-model", Some(&id), &definitions, &[]).unwrap();
        assert_eq!(restored.checkpoint(), conversation.checkpoint());
    }
    #[test]
    fn nine_tool_rounds_can_finish_without_poisoning_the_conversation() {
        let mut replies = std::collections::VecDeque::new();
        for index in 0..9 {
            replies.push_back(reply(json!([call(&format!("inspection_{index}"))])));
        }
        replies.push_back(reply(json!([{"type":"message","content":[{"type":"output_text","text":"Inspected the sampled frames"}]}])));
        let http = FakeHttp { replies: RefCell::new(replies), posts: AtomicUsize::new(0) };
        let mut native = backend();
        let mut conversation = Conversation::default();
        let result = conversation.run(&http, &mut native, "test-model", "Guide", "Inspect the timeline frames", &[], &AtomicBool::new(false));
        assert_eq!(result.unwrap(), "Inspected the sampled frames");
        assert_eq!(native.calls, 9);
        assert_eq!(http.posts.load(Ordering::SeqCst), 10);
        conversation.validate(&native.definitions().unwrap()).unwrap();
    }
    struct RecordedHttp {
        replies: RefCell<std::collections::VecDeque<Result<Vec<u8>, String>>>,
        requests: RefCell<Vec<Value>>,
        cancel_on_finish: bool,
    }
    impl Responses for RecordedHttp {
        fn post(&self, bytes: &[u8], cancel: &AtomicBool) -> Result<Vec<u8>, String> {
            let body: Value = serde_json::from_slice(bytes).unwrap();
            if self.cancel_on_finish && body["tool_choice"] == "none" {
                cancel.store(true, Ordering::SeqCst);
            }
            self.requests.borrow_mut().push(body);
            self.replies.borrow_mut().pop_front().expect("Unbounded finishing request or automatic replay")
        }
    }
    fn limited_http(finishing: Result<Vec<u8>, String>) -> RecordedHttp {
        let mut replies = std::collections::VecDeque::new();
        for index in 0..MAX_TOOL_ROUNDS {
            replies.push_back(reply(json!([call(&format!("completed_{index}"))])));
        }
        replies.push_back(finishing);
        RecordedHttp { replies: RefCell::new(replies), requests: RefCell::new(vec![]), cancel_on_finish: false }
    }
    #[test]
    fn bounded_work_checkpoints_and_resumes_without_replaying_completed_operations() {
        for finishing in [
            reply(json!([json_status_message("Inspected the frames; alignment is still unfinished")])),
            Err("Synthetic finishing HTTP failure".into()),
            reply(json!([call("must_not_dispatch_during_finishing")])),
        ] {
            let root = tempfile::tempdir().unwrap();
            let mut native = backend();
            let definitions = native.definitions().unwrap();
            let (journal, mut conversation) = super::super::state::Journal::open(root.path(), "project", "test-model", None, &definitions, &[]).unwrap();
            let id = journal.id().to_owned();
            let http = limited_http(finishing);
            let answer = run_persisted(
                &mut conversation,
                Some(&journal),
                &http,
                &mut native,
                "test-model",
                "Guide",
                "Continue the workflow",
                &[],
                &AtomicBool::new(false),
            )
            .unwrap();
            assert!(answer.contains("paused") && answer.contains("remaining work"));
            assert_eq!(native.calls, MAX_TOOL_ROUNDS);
            assert_eq!(http.requests.borrow().len(), MAX_TOOL_ROUNDS + 1);
            assert_eq!(http.requests.borrow().last().unwrap()["tool_choice"], "none");
            assert!(!conversation.seen_calls.contains("must_not_dispatch_during_finishing"));
            conversation.validate(&definitions).unwrap();
            drop(journal);
            let (journal, mut restored) = super::super::state::Journal::open(root.path(), "project", "test-model", Some(&id), &definitions, &[]).unwrap();
            assert_eq!(restored.seen_calls.len(), MAX_TOOL_ROUNDS);
            let next = FakeHttp {
                replies: RefCell::new([reply(json!([call("remaining_operation")])), reply(json!([json_status_message("Remaining work finished")]))].into()),
                posts: AtomicUsize::new(0),
            };
            assert_eq!(
                run_persisted(
                    &mut restored,
                    Some(&journal),
                    &next,
                    &mut native,
                    "test-model",
                    "Fresh project context",
                    "Continue only remaining work",
                    &[],
                    &AtomicBool::new(false)
                )
                .unwrap(),
                "Remaining work finished"
            );
            assert_eq!(native.calls, MAX_TOOL_ROUNDS + 1);
            assert_eq!(restored.seen_calls.len(), MAX_TOOL_ROUNDS + 1);
            let repeated = FakeHttp { replies: RefCell::new([reply(json!([call("completed_0")]))].into()), posts: AtomicUsize::new(0) };
            assert!(restored.run(&repeated, &mut native, "test-model", "Guide", "Inspect", &[], &AtomicBool::new(false)).is_err());
            assert_eq!(native.calls, MAX_TOOL_ROUNDS + 1);
        }
    }
    #[test]
    fn cancellation_during_finishing_keeps_the_interrupted_session_guard() {
        let root = tempfile::tempdir().unwrap();
        let mut native = backend();
        let definitions = native.definitions().unwrap();
        let (journal, mut conversation) = super::super::state::Journal::open(root.path(), "project", "test-model", None, &definitions, &[]).unwrap();
        let id = journal.id().to_owned();
        let mut http = limited_http(reply(json!([json_status_message("Too late")])));
        http.cancel_on_finish = true;
        assert!(run_persisted(&mut conversation, Some(&journal), &http, &mut native, "test-model", "Guide", "Work", &[], &AtomicBool::new(false)).is_err());
        assert_eq!(native.calls, MAX_TOOL_ROUNDS);
        drop(journal);
        assert!(super::super::state::Journal::open(root.path(), "project", "test-model", Some(&id), &definitions, &[]).err().unwrap().contains("interrupted"));
    }
    #[test]
    fn history_reserve_stops_before_admitting_new_operations_and_stays_clean() {
        let mut conversation = Conversation { input: vec![json_status_message("Historical answer"); WORK_ITEM_LIMIT - 1], seen_calls: HashSet::new() };
        let http = RecordedHttp {
            replies: RefCell::new(
                [reply(json!([call("not_admitted")])), reply(Value::Array(vec![json_status_message("Known progress only"); protocol::MAX_OUTPUT_ITEMS]))]
                    .into(),
            ),
            requests: RefCell::new(vec![]),
            cancel_on_finish: false,
        };
        let mut native = backend();
        assert!(conversation.run(&http, &mut native, "test-model", "Guide", "Work", &[], &AtomicBool::new(false)).unwrap().contains("paused"));
        assert_eq!(native.calls, 0);
        assert!(!conversation.seen_calls.contains("not_admitted"));
        assert_eq!(conversation.input.len(), protocol::MAX_INPUT_ITEMS);
        conversation.validate(&native.definitions().unwrap()).unwrap();
        let before = conversation.checkpoint();
        let no_requests = FakeHttp { replies: RefCell::new([].into()), posts: AtomicUsize::new(0) };
        assert!(conversation.run(&no_requests, &mut native, "test-model", "Guide", "More", &[], &AtomicBool::new(false)).unwrap().contains("history limit"));
        assert_eq!(no_requests.posts.load(Ordering::SeqCst), 0);
        assert_eq!(conversation.checkpoint(), before);
    }
    #[test]
    fn uncertain_native_result_never_triggers_another_model_request() {
        let http = FakeHttp { replies: RefCell::new([reply(json!([call("call_a")]))].into()), posts: AtomicUsize::new(0) };
        let mut backend = backend();
        backend.fail = true;
        assert!(Conversation::default().run(&http, &mut backend, "test-model", "Guide", "Edit", &[], &AtomicBool::new(false)).unwrap_err().contains("inspect"));
        assert_eq!(backend.calls, 1);
        assert_eq!(http.posts.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn switching_after_first_call_rejects_remaining_queued_calls() {
        let http = FakeHttp { replies: RefCell::new([reply(json!([call("call_a"), call("call_b")]))].into()), posts: AtomicUsize::new(0) };
        let mut backend = backend();
        backend.revoke_after_first = true;
        assert!(Conversation::default().run(&http, &mut backend, "test-model", "Guide", "Edit", &[], &AtomicBool::new(false)).is_err());
        assert_eq!(backend.calls, 1);
    }
    #[test]
    fn repeated_call_id_in_later_round_does_not_execute_twice() {
        let http = FakeHttp { replies: RefCell::new([reply(json!([call("call_a")])), reply(json!([call("call_a")]))].into()), posts: AtomicUsize::new(0) };
        let mut backend = backend();
        assert!(Conversation::default().run(&http, &mut backend, "test-model", "Guide", "Edit", &[], &AtomicBool::new(false)).is_err());
        assert_eq!(backend.calls, 1);
    }
    #[test]
    fn cancelled_turn_has_no_http_or_native_effect() {
        let http = FakeHttp { replies: RefCell::new([].into()), posts: AtomicUsize::new(0) };
        let mut backend = backend();
        assert!(Conversation::default().run(&http, &mut backend, "test-model", "Guide", "Edit", &[], &AtomicBool::new(true)).is_err());
        assert_eq!(http.posts.load(Ordering::SeqCst), 0);
        assert_eq!(backend.calls, 0);
    }
    #[test]
    fn repeated_call_identity_in_another_user_turn_is_not_replayed() {
        let http = FakeHttp {
            replies: RefCell::new(
                [
                    reply(json!([call("call_a")])),
                    reply(json!([{"type":"message","content":[{"type":"output_text","text":"Done"}]}])),
                    reply(json!([call("call_a")])),
                ]
                .into(),
            ),
            posts: AtomicUsize::new(0),
        };
        let mut backend = backend();
        let mut conversation = Conversation::default();
        let cancel = AtomicBool::new(false);
        conversation.run(&http, &mut backend, "test-model", "Guide", "Edit", &[], &cancel).unwrap();
        assert!(conversation.run(&http, &mut backend, "test-model", "Guide", "Inspect", &[], &cancel).is_err());
        assert_eq!(backend.calls, 1);
        assert_eq!(http.posts.load(Ordering::SeqCst), 3);
    }
    #[test]
    fn image_result_reaches_next_request_and_new_turn_uses_current_context() {
        struct Spy {
            replies: RefCell<std::collections::VecDeque<Result<Vec<u8>, String>>>,
            requests: RefCell<Vec<Value>>,
        }
        impl Responses for Spy {
            fn post(&self, body: &[u8], _: &AtomicBool) -> Result<Vec<u8>, String> {
                self.requests.borrow_mut().push(serde_json::from_slice(body).unwrap());
                self.replies.borrow_mut().pop_front().expect("Unexpected replay")
            }
        }
        struct Pixels(usize);
        impl Tools for Pixels {
            fn definitions(&self) -> Result<Vec<Tool>, String> {
                backend().definitions()
            }
            fn is_current(&self) -> bool {
                true
            }
            fn call_once(&mut self, _: &Call) -> Result<Value, String> {
                self.0 += 1;
                Ok(json!({"content":[{"type":"image","mimeType":"image/png","data":protocol::tests::test_png()}]}))
            }
        }
        let done = || reply(json!([{"type":"message","content":[{"type":"output_text","text":"Native result inspected"}]}]));
        let http = Spy { replies: RefCell::new([reply(json!([call("image_call")])), done(), done()].into()), requests: RefCell::new(vec![]) };
        let mut tools = Pixels(0);
        let mut conversation = Conversation::default();
        let cancel = AtomicBool::new(false);
        conversation.run(&http, &mut tools, "test-model", "old memory snapshot", "Inspect image", &[], &cancel).unwrap();
        conversation
            .run(&http, &mut tools, "test-model", "LATEST scope/documentEpoch/memoryGeneration and preferences", "Continue from current context", &[], &cancel)
            .unwrap();
        assert_eq!(tools.0, 1);
        let requests = http.requests.borrow();
        assert_eq!(requests.len(), 3);
        let result = requests[1]["input"].as_array().unwrap().iter().find(|item| item["type"] == "function_call_output").unwrap();
        assert_eq!(result["output"][0]["type"], "input_image");
        assert!(result["output"][0]["image_url"].as_str().unwrap().starts_with("data:image/png;base64,"));
        assert!(!requests[2].to_string().contains("data:image/png;base64,"));
        assert_eq!(requests[2]["instructions"], "LATEST scope/documentEpoch/memoryGeneration and preferences");
        assert_eq!(requests[2]["input"].as_array().unwrap().last().unwrap()["role"], "user");
    }
}
