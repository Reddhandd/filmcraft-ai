use super::super::turn::{Responses, Tools};
use super::*;
use serde_json::{Value, json};
use std::{cell::RefCell, collections::VecDeque, sync::atomic::AtomicBool};
struct Http {
    replies: RefCell<VecDeque<Vec<u8>>>,
    requests: RefCell<Vec<Value>>,
}
impl Http {
    fn new(replies: Vec<Value>) -> Self {
        Self {
            replies: RefCell::new(replies.into_iter().map(|output| serde_json::to_vec(&json!({"status":"completed","output":output})).unwrap()).collect()),
            requests: RefCell::new(Vec::new()),
        }
    }
}
impl Responses for Http {
    fn post(&self, body: &[u8], _: &AtomicBool) -> Result<Vec<u8>, String> {
        self.requests.borrow_mut().push(serde_json::from_slice(body).unwrap());
        self.replies.borrow_mut().pop_front().ok_or_else(|| "Unexpected request replay".into())
    }
}
struct Native {
    calls: usize,
    png: String,
}
impl Tools for Native {
    fn definitions(&self) -> Result<Vec<protocol::Tool>, String> {
        Ok(vec![protocol::Tool { name: "command_run".into(), description: "Synthetic command".into(), parameters: json!({"type":"object","properties":{}}) }])
    }
    fn is_current(&self) -> bool {
        true
    }
    fn call_once(&mut self, _: &protocol::Call) -> Result<Value, String> {
        self.calls += 1;
        Ok(json!({"content":[{"type":"text","text":"synthetic marker added"},{"type":"image","mimeType":"image/png","data":self.png}]}))
    }
}
fn native_fixture() -> Native {
    use base64::Engine as _;
    let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 123])));
    let mut output = std::io::Cursor::new(Vec::new());
    image.write_to(&mut output, image::ImageFormat::Png).unwrap();
    Native { calls: 0, png: base64::engine::general_purpose::STANDARD.encode(output.into_inner()) }
}
fn call(id: &str) -> Value {
    json!({"type":"function_call","name":"command_run","call_id":id,"arguments":"{}"})
}
fn answer(text: &str) -> Value {
    json!([{"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}])
}
#[test]
fn restart_preserves_reasoning_selected_pixels_tool_results_and_duplicate_protection() {
    let root = tempfile::tempdir().unwrap();
    let mut native = native_fixture();
    let defs = native.definitions().unwrap();
    let (journal, mut conversation) = Journal::open(root.path(), "project", "fake-model", None, &defs, &[]).unwrap();
    let id = journal.id().to_owned();
    let cancel = AtomicBool::new(false);
    let http = Http::new(vec![
        json!([{"type":"reasoning","id":"r1","encrypted_content":"opaque-test-ciphertext","summary":[]},call("call_1")]),
        answer("First edit complete"),
    ]);
    journal.save("fake-model", true, &conversation).unwrap();
    let selected_image = format!("data:image/png;base64,{}", native.png);
    assert_eq!(
        conversation.run(&http, &mut native, "fake-model", "old instructions", "Add one marker", &[selected_image], &cancel).unwrap(),
        "First edit complete"
    );
    assert_eq!(native.calls, 1);
    journal.save("fake-model", false, &conversation).unwrap();
    drop(journal);
    let (journal, mut restored) = Journal::open(root.path(), "project", "fake-model", Some(&id), &defs, &[]).unwrap();
    let followup = Http::new(vec![answer("Read-only followup"), json!([call("call_1")])]);
    let mut next_native = native_fixture();
    assert_eq!(
        restored.run(&followup, &mut next_native, "fake-model", "new instructions and memory", "What changed?", &[], &cancel).unwrap(),
        "Read-only followup"
    );
    assert_eq!(next_native.calls, 0);
    let requests = followup.requests.borrow();
    let request = requests.first().unwrap();
    assert_eq!(request["instructions"], "new instructions and memory");
    assert_eq!(request["include"], json!(["reasoning.encrypted_content"]));
    let input = request["input"].as_array().unwrap();
    assert!(input.iter().any(|item| item["encrypted_content"] == "opaque-test-ciphertext"));
    assert!(input.iter().any(|item| item["role"] == "user"
        && item["content"].as_array().unwrap().iter().any(|part| part["image_url"] == format!("data:image/png;base64,{}", native.png))));
    assert!(input.iter().filter(|item| item["type"] == "function_call_output").all(|item| !item.to_string().contains("input_image")));
    drop(requests);
    let error = restored.run(&followup, &mut next_native, "fake-model", "new instructions", "Another request", &[], &cancel).unwrap_err();
    assert!(error.contains("repeated a tool identity"));
    assert_eq!(next_native.calls, 0);
    drop(journal);
}
#[test]
fn malformed_hostile_and_oversized_saved_states_never_panic_or_escape_scope() {
    let root = tempfile::tempdir().unwrap();
    let (journal, _) = Journal::open(root.path(), "project", "fake-model", None, &[], &[]).unwrap();
    let path = journal.path.clone();
    let id = journal.id().to_owned();
    let original = std::fs::read(&path).unwrap();
    drop(journal);
    for bad in ["../elsewhere", "codex-thread", "api-../other", "api-💥"] {
        assert!(Journal::open(root.path(), "project", "fake-model", Some(bad), &[], &[]).is_err());
    }
    for cut in 0..original.len() {
        std::fs::write(&path, &original[..cut]).unwrap();
        let result = std::panic::catch_unwind(|| Journal::open(root.path(), "project", "fake-model", Some(&id), &[], &[]));
        assert!(result.is_ok());
        assert!(result.unwrap().is_err());
    }
    for offset in 0..original.len() {
        let mut corrupt = original.clone();
        corrupt[offset] ^= 0x80;
        std::fs::write(&path, corrupt).unwrap();
        assert!(std::panic::catch_unwind(|| Journal::open(root.path(), "project", "fake-model", Some(&id), &[], &[])).is_ok());
    }
    let mut state: Value = serde_json::from_slice(&original).unwrap();
    state["conversation"]["input"] = json!([{"type":"function_call_output","call_id":"orphan","output":"no call"}]);
    std::fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
    assert!(Journal::open(root.path(), "project", "fake-model", Some(&id), &[], &[]).is_err());
    std::fs::OpenOptions::new().write(true).open(path).unwrap().set_len((MAX_STATE as u64) + 1).unwrap();
    assert!(Journal::open(root.path(), "project", "fake-model", Some(&id), &[], &[]).err().unwrap().contains("byte budget"));
}

#[test]
fn json_expansion_depth_and_duplicate_keys_are_bounded_before_history_construction() {
    for body in [format!("[{}]", vec!["0"; 70000].join(",")), format!("{}0{}", "[".repeat(80), "]".repeat(80)), "{\"version\":1,\"version\":1}".into()] {
        let result = std::panic::catch_unwind(|| decode(body.as_bytes()));
        assert!(result.is_ok());
        assert!(result.unwrap().is_err());
    }
}

#[test]
fn only_host_verified_same_document_aliases_allow_saved_scope_migration() {
    let root = tempfile::tempdir().unwrap();
    let (journal, _) = Journal::open(root.path(), "unsaved-scope", "fake-model", None, &[], &[]).unwrap();
    let id = journal.id().to_owned();
    drop(journal);
    assert!(Journal::open(root.path(), "saved-project", "fake-model", Some(&id), &[], &[]).is_err());
    assert!(Journal::open(root.path(), "saved-project", "fake-model", Some(&id), &[], &["saved-project".into()]).is_err());
    let (journal, _) = Journal::open(root.path(), "saved-project", "fake-model", Some(&id), &[], &["unsaved-scope".into(), "saved-project".into()]).unwrap();
    drop(journal);
    assert!(Journal::open(root.path(), "saved-project", "fake-model", Some(&id), &[], &[]).is_ok());
    assert!(Journal::open(root.path(), "unrelated", "fake-model", Some(&id), &[], &[]).is_err());
}

#[test]
fn actual_persisted_turn_marks_uncertain_native_completion_and_never_replays_after_restart() {
    let root = tempfile::tempdir().unwrap();
    let mut native = native_fixture();
    let defs = native.definitions().unwrap();
    let (journal, mut conversation) = Journal::open(root.path(), "project", "fake-model", None, &defs, &[]).unwrap();
    let id = journal.id().to_owned();
    let http = Http::new(vec![json!([call("native_before_disconnect")])]);
    let error = super::super::turn::run_persisted(
        &mut conversation,
        Some(&journal),
        &http,
        &mut native,
        "fake-model",
        "current context",
        "Add one marker",
        &[],
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(error.contains("Earlier operations may have applied"));
    assert_eq!(native.calls, 1);
    assert_eq!(http.requests.borrow().len(), 2);
    drop(journal);
    assert!(Journal::open(root.path(), "project", "fake-model", Some(&id), &defs, &[]).err().unwrap().contains("interrupted"));
    assert_eq!(native.calls, 1);
    assert_eq!(http.requests.borrow().len(), 2);
}
#[test]
fn failed_begin_checkpoint_prevents_both_http_and_native_dispatch() {
    let root = tempfile::tempdir().unwrap();
    let mut native = native_fixture();
    let defs = native.definitions().unwrap();
    let (journal, _) = Journal::open(root.path(), "project", "fake-model", None, &defs, &[]).unwrap();
    let legacy_result = protocol::tool_result(
        &protocol::Call { id: "old_capture".into(), name: "command_run".into(), arguments: json!({}) },
        &json!({"content":[{"type":"image","mimeType":"image/png","data":native.png}]}),
    )
    .unwrap();
    let mut conversation =
        serde_json::from_value(json!({"input":[call("old_capture"),legacy_result,answer("Previous turn")[0]],"seen_calls":["old_capture"]})).unwrap();
    std::fs::remove_file(&journal.path).unwrap();
    std::fs::create_dir(&journal.path).unwrap();
    let http = Http::new(vec![answer("Must not be requested")]);
    assert!(
        super::super::turn::run_persisted(
            &mut conversation,
            Some(&journal),
            &http,
            &mut native,
            "fake-model",
            "current context",
            "Add one marker",
            &[],
            &AtomicBool::new(false)
        )
        .is_err()
    );
    assert!(http.requests.borrow().is_empty());
    assert!(!serde_json::to_string(&conversation).unwrap().contains(&native.png));
    assert_eq!(native.calls, 0);
}

#[test]
fn relative_and_absolute_scope_keys_identify_the_same_launch_directory() {
    let root = tempfile::tempdir().unwrap();
    let (journal, _) = Journal::open(root.path(), "relative-project.fcproj", "fake-model", None, &[], &[]).unwrap();
    let id = journal.id().to_owned();
    drop(journal);
    let absolute = std::path::absolute("relative-project.fcproj").unwrap().into_os_string().into_string().unwrap();
    assert!(Journal::open(root.path(), &absolute, "fake-model", Some(&id), &[], &[]).is_ok());
}
#[cfg(unix)]
#[test]
fn host_relative_data_directory_reopens_the_same_private_state() {
    let root = tempfile::tempdir().unwrap();
    let cwd = std::env::current_dir().unwrap();
    let mut relative = std::path::PathBuf::new();
    for _ in cwd.components().skip(1) {
        relative.push("..");
    }
    relative.push(root.path().strip_prefix("/").unwrap());
    assert!(!relative.is_absolute());
    let (journal, _) = Journal::open(&relative, "project", "fake-model", None, &[], &[]).unwrap();
    let id = journal.id().to_owned();
    drop(journal);
    assert!(Journal::open(root.path(), "project", "fake-model", Some(&id), &[], &[]).is_ok());
}
