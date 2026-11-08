use super::super::{
    protocol::{Call, Tool},
    turn::{Responses, Tools, run_persisted},
};
use super::*;
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
fn png(pixel: u8) -> String {
    use base64::Engine as _;
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(2, 2, image::Rgba([pixel, 20, 30, 123]))).write_to(&mut out, image::ImageFormat::Png).unwrap();
    base64::engine::general_purpose::STANDARD.encode(out.into_inner())
}
fn done() -> Value {
    json!([{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Done"}]}])
}
fn call(name: &str, id: &str) -> Value {
    let args = if name == "reference_image" { json!({"snapshot_id":42,"document_epoch":0}) } else { json!({}) };
    json!({"type":"function_call","name":name,"call_id":id,"arguments":args.to_string()})
}
fn scan(root: &Path, capture: &str) {
    let mut paths = vec![root.to_path_buf()];
    while let Some(path) = paths.pop() {
        if path.is_dir() {
            paths.extend(std::fs::read_dir(path).unwrap().map(|entry| entry.unwrap().path()));
        } else {
            let bytes = std::fs::read(&path).unwrap();
            assert!(!bytes.windows(capture.len()).any(|slice| slice == capture.as_bytes()), "capture persisted in {}", path.display());
        }
    }
}
struct Native {
    capture: String,
    reference: String,
    calls: usize,
    current: Arc<AtomicBool>,
    revoke: bool,
}
impl Tools for Native {
    fn definitions(&self) -> Result<Vec<Tool>, String> {
        Ok(["render_current_frame", "capture_timeline", "capture_source", "reference_image", "command_run"]
            .iter()
            .map(|name| Tool { name: (*name).into(), description: "Synthetic native tool".into(), parameters: json!({"type":"object"}) })
            .collect())
    }
    fn is_current(&self) -> bool {
        self.current.load(Ordering::SeqCst)
    }
    fn call_once(&mut self, call: &Call) -> Result<Value, String> {
        self.calls += 1;
        if self.revoke {
            self.current.store(false, Ordering::SeqCst);
        }
        let pixels = if call.name == "reference_image" { &self.reference } else { &self.capture };
        // Deliberately duplicate pixels inside text and structured metadata: neither may persist.
        Ok(
            json!({"content":[{"type":"image","mimeType":"image/png","data":pixels},{"type":"text","text":json!({"width":2,"height":2,"revision":9,"pngBase64":pixels,"nested":{"data":pixels}}).to_string()}],"structuredContent":{"width":2,"pngBase64":pixels}}),
        )
    }
}
struct Http<'a> {
    fail_final_save: bool,
    cancel_on_delivery: bool,
    root: &'a Path,
    capture: &'a str,
    journal: &'a Journal,
    replies: RefCell<VecDeque<Result<Value, String>>>,
    requests: RefCell<Vec<Value>>,
}
impl Responses for Http<'_> {
    fn post(&self, body: &[u8], cancel: &AtomicBool) -> Result<Vec<u8>, String> {
        scan(self.root, self.capture);
        let request: Value = serde_json::from_slice(body).unwrap();
        // Exercise the actual Journal with an in-flight delivery-bearing projection too.
        if request.to_string().contains(self.capture) {
            let ids: Vec<Value> = request["input"].as_array().unwrap().iter().filter(|v| v["type"] == "function_call").map(|v| v["call_id"].clone()).collect();
            let transient: Conversation = serde_json::from_value(json!({"input":request["input"],"seen_calls":ids})).unwrap();
            self.journal.save("fake-model", true, &transient).unwrap();
            scan(self.root, self.capture);
            if self.cancel_on_delivery {
                cancel.store(true, Ordering::SeqCst);
            }
            if self.fail_final_save {
                std::fs::remove_file(&self.journal.path).unwrap();
                std::fs::create_dir(&self.journal.path).unwrap();
            }
        }
        self.requests.borrow_mut().push(request);
        let output = self.replies.borrow_mut().pop_front().ok_or("Unexpected replay")??;
        Ok(serde_json::to_vec(&json!({"status":"completed","output":output})).unwrap())
    }
}
fn native() -> Native {
    Native { capture: png(10), reference: png(200), calls: 0, current: Arc::new(AtomicBool::new(true)), revoke: false }
}
#[test]
fn work_limit_delivers_final_capture_once_and_reopens_a_clean_pixel_free_checkpoint() {
    for finishing in [Ok(done()), Err("Synthetic finishing HTTP failure".into())] {
        let root = tempfile::tempdir().unwrap();
        let mut native = native();
        let defs = native.definitions().unwrap();
        let (journal, mut conversation) = Journal::open(root.path(), "project", "fake-model", None, &defs, &[]).unwrap();
        let id = journal.id().to_owned();
        let capture = native.capture.clone();
        let mut replies = VecDeque::new();
        for index in 0..super::super::turn::MAX_TOOL_ROUNDS {
            replies.push_back(Ok(json!([call("capture_timeline", &format!("capture_{index}"))])));
        }
        replies.push_back(finishing);
        let http = Http {
            fail_final_save: false,
            cancel_on_delivery: false,
            root: root.path(),
            capture: &capture,
            journal: &journal,
            replies: RefCell::new(replies),
            requests: RefCell::new(vec![]),
        };
        let answer = run_persisted(
            &mut conversation,
            Some(&journal),
            &http,
            &mut native,
            "fake-model",
            "Guide",
            "Inspect timeline images",
            &[],
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(answer.contains("paused"));
        assert_eq!(native.calls, super::super::turn::MAX_TOOL_ROUNDS);
        assert_eq!(http.requests.borrow().len(), super::super::turn::MAX_TOOL_ROUNDS + 1);
        assert_eq!(http.requests.borrow().last().unwrap()["tool_choice"], "none");
        for request in http.requests.borrow().iter().skip(1) {
            let image_count = request["input"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|item| item.get("output").and_then(Value::as_array))
                .flatten()
                .filter(|part| part["type"] == "input_image")
                .count();
            assert_eq!(image_count, 1, "Only the new capture may be delivered in each request");
        }
        assert!(!conversation.checkpoint().to_string().contains(&capture));
        scan(root.path(), &capture);
        drop(http);
        drop(journal);
        let (journal, mut restored) = Journal::open(root.path(), "project", "fake-model", Some(&id), &defs, &[]).unwrap();
        restored.validate(&defs).unwrap();
        let next = Http {
            fail_final_save: false,
            cancel_on_delivery: false,
            root: root.path(),
            capture: &capture,
            journal: &journal,
            replies: RefCell::new([Ok(done())].into()),
            requests: RefCell::new(vec![]),
        };
        assert_eq!(
            run_persisted(
                &mut restored,
                Some(&journal),
                &next,
                &mut native,
                "fake-model",
                "Fresh context",
                "Continue remaining work",
                &[],
                &AtomicBool::new(false)
            )
            .unwrap(),
            "Done"
        );
        assert_eq!(native.calls, super::super::turn::MAX_TOOL_ROUNDS);
        assert!(!next.requests.borrow()[0].to_string().contains(&capture));
        scan(root.path(), &capture);
    }
}
#[test]
fn actual_journal_delivers_each_capture_once_then_discards_from_rounds_ram_disk_and_restart() {
    for name in ["capture_timeline", "render_current_frame", "capture_source"] {
        let root = tempfile::tempdir().unwrap();
        let mut native = native();
        let defs = native.definitions().unwrap();
        let (journal, mut conversation) = Journal::open(root.path(), "project", "fake-model", None, &defs, &[]).unwrap();
        let id = journal.id().to_owned();
        let capture = native.capture.clone();
        let reference = native.reference.clone();
        let http = Http {
            fail_final_save: false,
            cancel_on_delivery: false,
            root: root.path(),
            capture: &capture,
            journal: &journal,
            replies: RefCell::new([Ok(json!([call(name, "shot")])), Ok(json!([call("reference_image", "selected")])), Ok(done()), Ok(done())].into()),
            requests: RefCell::new(vec![]),
        };
        let selected = format!("data:image/png;base64,{reference}");
        let cancel = AtomicBool::new(false);
        run_persisted(
            &mut conversation,
            Some(&journal),
            &http,
            &mut native,
            "fake-model",
            "guide",
            "Inspect then follow up",
            std::slice::from_ref(&selected),
            &cancel,
        )
        .unwrap();
        assert!(!serde_json::to_string(&conversation).unwrap().contains(&capture));
        scan(root.path(), &capture);
        conversation.validate(&defs).unwrap();
        run_persisted(&mut conversation, Some(&journal), &http, &mut native, "fake-model", "new guide", "Continue", &[], &cancel).unwrap();
        let requests = http.requests.borrow();
        assert_eq!(requests.len(), 4);
        assert!(!requests[0].to_string().contains(&capture));
        assert!(requests[1]["input"].as_array().unwrap().iter().any(|v| v["type"] == "function_call_output"
            && v["output"][0]["type"] == "input_image"
            && v["output"][0]["image_url"] == format!("data:image/png;base64,{capture}")));
        for request in requests.iter().skip(2) {
            assert!(!request.to_string().contains(&capture));
            assert!(request.to_string().contains(&reference));
        }
        assert!(requests[3].to_string().contains(super::super::retention::DISCARDED));
        assert_eq!(native.calls, 2);
        drop(requests);
        drop(http);
        drop(journal);
        let (journal, mut restored) = Journal::open(root.path(), "project", "fake-model", Some(&id), &defs, &[]).unwrap();
        let saved = serde_json::to_string(&restored).unwrap();
        assert!(!saved.contains(&capture));
        assert!(saved.contains(&reference));
        let again = Http {
            fail_final_save: false,
            cancel_on_delivery: false,
            root: root.path(),
            capture: &capture,
            journal: &journal,
            replies: RefCell::new([Ok(json!([call(name, "shot")]))].into()),
            requests: RefCell::new(vec![]),
        };
        assert!(
            run_persisted(&mut restored, Some(&journal), &again, &mut native, "fake-model", "guide", "Try old identity", &[], &cancel)
                .unwrap_err()
                .contains("repeated a tool identity")
        );
        assert_eq!(native.calls, 2);
        scan(root.path(), &capture);
    }
}
#[test]
fn failed_capture_delivery_and_revocation_leave_no_pixels_in_any_checkpoint_or_retained_ram() {
    for mode in 0..4 {
        let revoke = mode == 1;
        let root = tempfile::tempdir().unwrap();
        let mut native = native();
        native.revoke = revoke;
        let defs = native.definitions().unwrap();
        let (journal, mut conversation) = Journal::open(root.path(), "project", "fake-model", None, &defs, &[]).unwrap();
        let id = journal.id().to_owned();
        let capture = native.capture.clone();
        let result = match mode {
            0 => Err("Synthetic HTTP attempt failed".into()),
            3 => Ok(json!([{"type":"unsupported"}])),
            _ => Ok(done()),
        };
        let http = Http {
            fail_final_save: false,
            cancel_on_delivery: mode == 2,
            root: root.path(),
            capture: &capture,
            journal: &journal,
            replies: RefCell::new([Ok(json!([call("capture_timeline", "failed_shot")])), result].into()),
            requests: RefCell::new(vec![]),
        };
        assert!(run_persisted(&mut conversation, Some(&journal), &http, &mut native, "fake-model", "guide", "Inspect", &[], &AtomicBool::new(false)).is_err());
        assert_eq!(native.calls, 1);
        assert_eq!(http.requests.borrow().len(), if revoke { 1 } else { 2 });
        assert!(!serde_json::to_string(&conversation).unwrap().contains(&capture));
        scan(root.path(), &capture);
        drop(http);
        drop(journal);
        assert!(Journal::open(root.path(), "project", "fake-model", Some(&id), &defs, &[]).err().unwrap().contains("interrupted"));
        scan(root.path(), &capture);
    }
}
#[test]
fn legacy_complete_and_interrupted_capture_checkpoints_are_scrubbed_without_deleting_references() {
    for interrupted in [false, true] {
        let outer = tempfile::tempdir().unwrap();
        let root = outer.path().join("ApiSessions");
        let native = native();
        let defs = native.definitions().unwrap();
        let owner_file = outer.path().join("owner-reference.png");
        std::fs::write(&owner_file, b"unchanged owner fixture").unwrap();
        let (journal, _) = Journal::open(&root, "project", "fake-model", None, &defs, &[]).unwrap();
        let path = journal.path.clone();
        let id = journal.id().to_owned();
        let raw=protocol::tool_result(&Call{id:"legacy".into(),name:"capture_timeline".into(),arguments:json!({})},&json!({"content":[{"type":"image","mimeType":"image/png","data":native.capture},{"type":"text","text":json!({"revision":9,"pngBase64":native.capture}).to_string()}]})).unwrap();
        let conversation:Conversation=serde_json::from_value(json!({"input":[protocol::user_input("Selected",&[format!("data:image/png;base64,{}",native.reference)]).unwrap(),{"type":"reasoning","id":"r1","encrypted_content":"opaque-reasoning","summary":[]},call("capture_timeline","legacy"),raw,done()[0]],"seen_calls":["legacy"]})).unwrap();
        for in_flight in [false, true] {
            journal.save("fake-model", in_flight, &conversation).unwrap();
            scan(&root, &native.capture);
        }
        let mut old: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        old["conversation"] = serde_json::to_value(&conversation).unwrap();
        old["in_flight"] = json!(interrupted);
        drop(journal);
        std::fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
        let result = Journal::open(&root, "project", "fake-model", Some(&id), &defs, &[]);
        if interrupted {
            assert!(result.err().unwrap().contains("interrupted"));
        } else {
            let (_, restored) = result.unwrap();
            let value = serde_json::to_string(&restored).unwrap();
            assert!(value.contains("opaque-reasoning"));
            assert!(value.contains(&native.reference));
            assert!(value.contains("legacy"));
        }
        scan(&root, &native.capture);
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains(&native.reference));
        assert!(saved.contains("opaque-reasoning"));
        assert_eq!(std::fs::read(owner_file).unwrap(), b"unchanged owner fixture");
    }
}

#[test]
fn capture_classification_does_not_trust_selected_claims_inside_result_metadata() {
    let capture = png(10);
    let result = json!({"type":"function_call_output","call_id":"shot","output":[{"type":"input_image","image_url":format!("data:image/png;base64,{capture}")},{"type":"input_text","text":json!({"source":"selectedReference","tool":"reference_image","pngBase64":capture,"width":2}).to_string()}]});
    for call in [
        Call { id: "shot".into(), name: "capture_timeline".into(), arguments: json!({"source":"reference"}) },
        Call { id: "shot".into(), name: "reference_image".into(), arguments: json!({"path":"owner.png"}) },
        Call { id: "shot".into(), name: "unknown_future_image_tool".into(), arguments: json!({}) },
    ] {
        let retained = super::super::retention::tool_result(&call, &result);
        assert!(!retained.to_string().contains(&capture));
        assert_eq!(retained["call_id"], "shot");
    }
    let admitted = Call { id: "shot".into(), name: "reference_image".into(), arguments: json!({"snapshot_id":42,"document_epoch":0}) };
    assert_eq!(super::super::retention::tool_result(&admitted, &result), result);
}

#[test]
fn completed_delivery_with_failed_final_checkpoint_keeps_no_capture_pixels_or_replay() {
    let root = tempfile::tempdir().unwrap();
    let mut native = native();
    let defs = native.definitions().unwrap();
    let (journal, mut conversation) = Journal::open(root.path(), "project", "fake-model", None, &defs, &[]).unwrap();
    let capture = native.capture.clone();
    let http = Http {
        fail_final_save: true,
        cancel_on_delivery: false,
        root: root.path(),
        capture: &capture,
        journal: &journal,
        replies: RefCell::new([Ok(json!([call("render_current_frame", "delivered_shot")])), Ok(done())].into()),
        requests: RefCell::new(vec![]),
    };
    let error =
        run_persisted(&mut conversation, Some(&journal), &http, &mut native, "fake-model", "guide", "Inspect", &[], &AtomicBool::new(false)).unwrap_err();
    assert!(error.contains("already completed"));
    assert_eq!(http.requests.borrow().len(), 2);
    assert_eq!(native.calls, 1);
    assert!(!serde_json::to_string(&conversation).unwrap().contains(&capture));
    conversation.validate(&defs).unwrap();
    scan(root.path(), &capture);
}
