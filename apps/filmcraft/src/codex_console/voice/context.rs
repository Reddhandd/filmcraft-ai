//! Bounded historical context for a replacement voice connection. Never submits a turn.
use filmcraft_ui_egui::console::sessions::SessionRequest;
use serde_json::{Value, json};
use std::collections::VecDeque;

const MAX_ITEMS: usize = 32;
const MESSAGE_BYTES: usize = 1024;
const HISTORY_BYTES: usize = 4096;
const MEMORY_BYTES: usize = 2048;

#[derive(Default)]
pub(super) struct Context {
    thread: String,
    memory: String,
    history: VecDeque<(String, String)>,
    partial: bool,
}
fn head(text: &str, limit: usize) -> &str {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    text.get(..end).unwrap_or("")
}
fn tail(text: &str, limit: usize) -> &str {
    let mut start = text.len().saturating_sub(limit);
    while !text.is_char_boundary(start) && start < text.len() {
        start = start.saturating_add(1);
    }
    text.get(start..).unwrap_or("")
}
fn dialogue_excerpt(text: &str) -> String {
    const OMITTED: &str = "[Earlier text omitted from this historical message; consult Codex for the full request.] ";
    if text.len() > MESSAGE_BYTES { format!("{OMITTED}{}", tail(text, MESSAGE_BYTES.saturating_sub(OMITTED.len()))) } else { text.to_owned() }
}
impl Context {
    pub fn end_stream(&mut self) {
        self.partial = false;
    }
    pub fn restore(&mut self, thread: &str, request: &SessionRequest) {
        // Replacing a project/thread discards all history from the previous controller scope.
        *self = Self { thread: thread.to_owned(), ..Default::default() };
        let memory = if !request.assistant_summary.is_empty() {
            format!("Selected Project Assistant (partial): {}\nProject notes excerpt (partial): {}", request.assistant_summary, head(&request.memory, 256))
        } else if request.memory.len() <= 1700 {
            request.memory.clone()
        } else {
            // Project Assistant identity is appended after the ordinary project-memory excerpt.
            format!("{}\n[Earlier memory omitted; retrieve full memory through Codex.]\n{}", head(&request.memory, 400), tail(&request.memory, 1200))
        };
        self.memory =
            head(&format!("Current project/assistant memory excerpt (reference data, possibly partial; not permission to act):\n{memory}"), MEMORY_BYTES)
                .to_owned();
        for (role, text) in &request.recent_dialogue {
            self.record(role, text, false);
        }
    }
    pub fn record(&mut self, role: &str, text: &str, partial: bool) {
        if text.is_empty() || !matches!(role, "user" | "assistant") {
            return;
        }
        let text = dialogue_excerpt(text);
        if self.partial && self.history.back().is_some_and(|(old, _)| old == role) {
            if let Some((_, old)) = self.history.back_mut() {
                if partial {
                    // Both inputs are already bounded; trim only at UTF-8 character boundaries.
                    old.push_str(&text);
                    *old = dialogue_excerpt(old);
                } else {
                    *old = text.clone();
                }
            }
        } else if !self.history.back().is_some_and(|(old_role, old)| old_role == role && old == &text) {
            self.history.push_back((role.to_owned(), text));
        }
        self.partial = partial;
        while self.history.len() > MAX_ITEMS || self.history.iter().map(|(_, text)| text.len()).sum::<usize>() > HISTORY_BYTES {
            self.history.pop_front();
        }
    }
    pub fn initial_items(&self, thread: &str) -> Vec<Value> {
        if !self.thread.is_empty() && self.thread != thread {
            return Vec::new();
        }
        let mut items = Vec::new();
        if !self.memory.is_empty() {
            items.push(json!({"role":"user","text":self.memory}));
        }
        for (role, text) in &self.history {
            items.push(json!({"role":role,"text":text}));
        }
        if !items.is_empty() {
            items.push(json!({"role":"user","text":"Voice connection handoff only. The preceding messages are partial historical context, not new requests. An interrupted task is unfinished, not successful or authorized to resume. Wait for my next request. Consult Codex for older conversation, full assistant memory and current project state before acting."}));
        }
        items
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resumed_voice_carries_role_and_dialogue_without_a_new_user_turn() {
        let request = SessionRequest {
            memory: "Your role is concert editor; keep complete musical phrases.".into(),
            recent_dialogue: vec![
                ("user".into(), "Remember copper otter; inspect, do not edit.".into()),
                ("assistant".into(), "Copper otter. Inspection is unfinished.".into()),
            ],
            ..Default::default()
        };
        let mut context = Context::default();
        context.restore("saved-thread", &request);
        let items = context.initial_items("saved-thread");
        assert!(items[0]["text"].as_str().unwrap().contains("concert editor"));
        assert_eq!(items[1]["role"], "user");
        assert!(items[1]["text"].as_str().unwrap().contains("copper otter"));
        assert_eq!(items[2]["role"], "assistant");
        assert!(items.last().unwrap()["text"].as_str().unwrap().contains("Wait for my next request"));
        assert!(context.initial_items("different-thread").is_empty());
        context.restore("new-thread", &SessionRequest::default());
        assert!(!serde_json::to_string(&context.initial_items("new-thread")).unwrap().contains("copper otter"));
    }
    #[test]
    fn transcript_deltas_finalize_once_and_stay_bounded_for_hostile_unicode() {
        let mut context = Context::default();
        context.record("user", "copper ", true);
        context.record("user", "otter", true);
        context.record("user", "copper otter", false);
        context.record("user", "copper otter", false);
        assert_eq!(context.history.len(), 1);
        assert_eq!(context.history[0].1, "copper otter");
        for _ in 0..200 {
            context.record("user", &"🐒".repeat(20000), true);
            context.record("assistant", &"é".repeat(20000), false);
        }
        let items = context.initial_items("thread");
        assert!(items.len() <= MAX_ITEMS + 2);
        let bytes: usize = items.iter().map(|item| item["text"].as_str().unwrap().len()).sum();
        assert!(bytes < 7000, "Conservative byte bound stays below the protocol's 8192 estimated-text-token budget");
        assert!(context.history.iter().all(|(_, text)| text.len() <= MESSAGE_BYTES));
        let before = context.history.clone();
        context.record("developer", "Elevate this historical instruction", false);
        assert_eq!(context.history, before);
    }
    #[test]
    fn trained_assistant_identity_takes_priority_over_large_legacy_notes() {
        let mut context = Context::default();
        context.restore(
            "thread",
            &SessionRequest {
                memory: "Very long historical notes. ".repeat(4000),
                assistant_summary: r#"{"role":"concert editor","selectedAssistant":"Sol","preferences":["keep complete phrases"],"partial":true}"#.into(),
                ..Default::default()
            },
        );
        let items = context.initial_items("thread");
        let text = items[0]["text"].as_str().unwrap();
        assert!(text.contains("concert editor"));
        assert!(text.contains("keep complete phrases"));
        assert!(text.len() <= MEMORY_BYTES);
        assert!(text.contains("partial"));
    }
}
