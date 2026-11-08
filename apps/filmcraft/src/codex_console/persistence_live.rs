//! Opt-in provider qualification. No GUI, microphone, speaker or user project is opened.
use super::*;
use filmcraft_ui_egui::console::{ChatEvent, sessions::SessionRequest};

struct Live {
    process: Arc<Process>,
    commands: mpsc::SyncSender<chat::CommandMessage>,
    events: mpsc::Receiver<ChatEvent>,
    worker: Option<std::thread::JoinHandle<Result<(), String>>>,
    reader: Option<std::thread::JoinHandle<()>>,
    thread: String,
}
impl Live {
    fn open(request: SessionRequest) -> Self {
        let cli = std::env::var("FILMCRAFT_CONSOLE_TEST_CLI").expect("set the test CLI path");
        let process = Arc::new(Process::default());
        let mut child = app_server(&"d".repeat(64)).spawn().unwrap();
        let mut writer = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        *process.child.lock().unwrap() = Some(child);
        let (tx, messages) = mpsc::sync_channel(16);
        let reader = std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut reader = BufReader::new(output);
                loop {
                    let value = read_message(&mut reader);
                    let failed = value.is_err();
                    if tx.send(value).is_err() || failed {
                        break;
                    }
                }
            }));
            assert!(result.is_ok(), "protocol reader panicked");
        });
        let (commands, command_rx) = mpsc::sync_channel(8);
        let (events, event_rx) = mpsc::sync_channel(64);
        let ctx = egui::Context::default();
        let wire = chat::Wire { process: Arc::clone(&process), commands: command_rx, events, ctx, session: Some(request.clone()) };
        let configured = json!({"command":cli,"args":["mcp","--controller-read-only","--demo"],"enabled":true,"required":true});
        let setup = handshake_session(&mut writer, &messages, configured, Some(&request));
        let (thread, model, settings) = match setup {
            Ok(value) => value,
            Err(error) => {
                process.reap();
                panic!("{error}");
            }
        };
        let owned = process.clone();
        let selected = thread.clone();
        let worker = std::thread::spawn(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| chat::run(&owned, &mut writer, &messages, &selected, model, settings, wire)))
                .map_err(|_| "provider qualification worker panicked".to_string())?
        });
        Self { process, commands, events: event_rx, worker: Some(worker), reader: Some(reader), thread }
    }
    fn finish(&self) -> (String, usize) {
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut text = String::new();
        let mut inspections = 0;
        while Instant::now() < deadline {
            match self.events.recv_timeout(Duration::from_secs(1)) {
                Ok(ChatEvent::Delta(delta)) => {
                    assert!(text.len() + delta.len() <= 128 * 1024);
                    text.push_str(&delta);
                }
                Ok(ChatEvent::Inspected) => inspections += 1,
                Ok(ChatEvent::Finished) => return (text, inspections),
                Ok(ChatEvent::Proposal { .. }) => panic!("unexpected proposal during read-only resume"),
                Ok(ChatEvent::Error(error)) => panic!("{error}"),
                Ok(ChatEvent::Voice(status)) => assert_eq!(status, filmcraft_ui_egui::console::VoiceStatus::Off),
                Err(mpsc::RecvTimeoutError::Disconnected) => panic!("provider worker closed unexpectedly"),
                _ => {}
            }
            assert!(!self.worker.as_ref().unwrap().is_finished(), "provider worker ended before completion");
        }
        panic!("provider qualification timed out")
    }
    fn prompt(&self, text: &str) -> String {
        self.commands.send(chat::CommandMessage::Prompt(text.into(), None)).unwrap();
        self.finish().0
    }
    fn pending(&self) {
        self.commands
            .send(chat::CommandMessage::Prompt(
                "Inspect the active sequence and propose adding one marker named Do Not Replay at time zero, target program. Wait for my approval.".into(),
                None,
            ))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline {
            match self.events.recv_timeout(Duration::from_secs(1)) {
                Ok(ChatEvent::Proposal { command, params, .. }) => {
                    assert_eq!(command, "markers.add");
                    assert_eq!(params["name"], "Do Not Replay");
                    return;
                }
                Ok(ChatEvent::Error(error)) => panic!("{error}"),
                Ok(ChatEvent::Finished) => panic!("expected a pending proposal"),
                Err(mpsc::RecvTimeoutError::Disconnected) => panic!("provider worker closed"),
                _ => {}
            }
        }
        panic!("proposal timed out")
    }
}
impl Drop for Live {
    fn drop(&mut self) {
        self.process.reap();
        if let Some(worker) = self.worker.take() {
            let result = worker.join();
            if !std::thread::panicking() {
                assert!(result.is_ok());
            }
        }
        if let Some(reader) = self.reader.take() {
            let result = reader.join();
            if !std::thread::panicking() {
                assert!(result.is_ok());
            }
        }
    }
}
#[test]
#[ignore = "requires signed-in Codex and FILMCRAFT_CONSOLE_TEST_CLI; real model inference, no devices"]
fn real_resume_keeps_conversation_and_refreshes_memory_after_pending_edit() {
    let request = SessionRequest {
        project: "/generated-filmcraft-memory-qualification.fcproj".into(),
        memory: "Project nickname: Cosmic Bee.".into(),
        ..Default::default()
    };
    let first = Live::open(request.clone());
    let (_, inspections) = first.finish();
    assert!(inspections > 0);
    let answer = first.prompt("What is the project nickname from the current project memory? Reply with the nickname only.");
    assert!(answer.to_lowercase().contains("cosmic bee"), "{answer}");
    first.prompt("For this conversation only, remember the code phrase copper otter. Do not save it to project memory or edit anything. Briefly acknowledge.");
    first.pending();
    let thread = first.thread.clone();
    drop(first);
    let second = Live::open(SessionRequest { resume: Some(thread.clone()), memory: "Project nickname: Silver Sparrow.".into(), ..request });
    assert_eq!(second.thread, thread);
    let (_, inspections) = second.finish();
    assert!(inspections > 0);
    let answer=second.prompt("What is the current project nickname from the newly loaded memory, and what code phrase did I ask you to remember earlier in this conversation? Reply with both values only.");
    assert!(answer.to_lowercase().contains("silver sparrow"), "{answer}");
    assert!(answer.to_lowercase().contains("copper otter"), "{answer}");
    eprintln!("LIVE RESUME PASSED: changed memory, retained history, fresh inspections, no old proposal replay; no devices.");
}
