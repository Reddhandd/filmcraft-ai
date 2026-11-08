//! Opt-in real-provider lifecycle qualification with generated audio and no devices.
use super::*;
use filmcraft_ui_egui::console::{ChatEvent, VoiceStatus};

struct Service {
    process: Arc<Process>,
    writer: std::process::ChildStdin,
    messages: Messages,
    reader: Option<std::thread::JoinHandle<()>>,
    thread: String,
}
impl Service {
    fn open() -> Self {
        let cli = std::env::var("FILMCRAFT_CONSOLE_TEST_CLI").expect("set isolated MCP fixture executable");
        let process = Arc::new(Process::default());
        let mut child = super::super::app_server(&"d".repeat(64)).spawn().unwrap();
        let writer = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        *process.child.lock().unwrap() = Some(child);
        let (tx, messages) = mpsc::sync_channel(64);
        let reader = std::thread::spawn(move || {
            let mut output = BufReader::new(output);
            loop {
                let result = super::super::read_message(&mut output);
                let failed = result.is_err();
                if tx.send(result).is_err() || failed {
                    break;
                }
            }
        });
        let configured = json!({"command":cli,"args":["mcp","--controller-read-only","--demo"],"enabled":true,"required":true});
        let mut service = Self { process, writer, messages, reader: Some(reader), thread: String::new() };
        service.thread = super::super::handshake(&mut service.writer, &service.messages, configured).unwrap().0;
        service
    }
    fn pump(&mut self, controller: &mut Controller, wire: &chat::Wire) -> Vec<Value> {
        controller.poll(&mut self.writer, wire, &self.thread).unwrap();
        let mut observed = Vec::new();
        for _ in 0..64 {
            let message = match self.messages.try_recv() {
                Ok(Ok(message)) => message,
                Ok(Err(error)) => panic!("{error}"),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => panic!("provider transport closed"),
            };
            controller.message(&mut self.writer, wire, &self.thread, &message).unwrap();
            if message.get("method").is_some() && message.get("id").is_some() {
                super::super::send(&mut self.writer, &json!({"id":message["id"],"error":{"code":-32601,"message":"Voice lifecycle test refuses tool calls"}}))
                    .unwrap();
            }
            observed.push(message);
        }
        observed
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.process.reap();
        // Drain so a bounded reader cannot remain blocked on a final notification.
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.reader.as_ref().is_some_and(|reader| !reader.is_finished()) && Instant::now() < deadline {
            let _ = self.messages.recv_timeout(Duration::from_millis(20));
        }
        if self.reader.as_ref().is_some_and(|reader| reader.is_finished()) {
            self.reader.take().unwrap().join().unwrap();
        }
    }
}

#[test]
#[ignore = "requires signed-in Codex, isolated FILMCRAFT_CONSOLE_TEST_CLI and generated FILMCRAFT_VOICE_TEST_PCM"]
fn expiry_cleanup_restarts_voice_on_same_thread_without_devices() {
    let path = std::env::var_os("FILMCRAFT_VOICE_TEST_PCM").expect("set generated PCM16LE fixture");
    let mut bytes = Vec::new();
    std::fs::File::open(path).unwrap().take(256 * 1024 + 1).read_to_end(&mut bytes).unwrap();
    assert!(!bytes.is_empty() && bytes.len() <= 256 * 1024 && bytes.len() % 2 == 0);
    let samples: Vec<f32> = bytes.as_chunks::<2>().0.iter().map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0).collect();
    let mut service = Service::open();
    let original_thread = service.thread.clone();
    let (_, commands) = mpsc::channel();
    let (events, received) = mpsc::sync_channel(64);
    let wire = chat::Wire { process: Arc::clone(&service.process), commands, events, ctx: egui::Context::default(), session: None };
    let mut controller = Controller::default();
    for cycle in 0..2 {
        controller.prepare(&wire, 0).unwrap();
        // Test-only input: no microphone or speaker is ever opened.
        controller.peer =
            Some(Peer::start(Input::Fixture { samples: samples.clone(), lifetime: Duration::from_secs(if cycle == 0 { 20 } else { 60 }) }).unwrap());
        let deadline = Instant::now() + Duration::from_secs(60);
        let (mut active, mut sent, mut audio) = (false, 0, 0);
        while Instant::now() < deadline {
            service.pump(&mut controller, &wire);
            for event in received.try_iter() {
                match event {
                    ChatEvent::Voice(VoiceStatus::Active) => active = true,
                    ChatEvent::Voice(VoiceStatus::Failed(error)) => panic!("voice cycle {cycle}: {error}"),
                    ChatEvent::VoiceStats { sent: s, received: r } => {
                        sent = s;
                        audio = r;
                    }
                    _ => {}
                }
            }
            if active && sent > 8000 && audio > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(active && sent > 8000 && audio > 0, "cycle {cycle}: active={active} sent={sent} received={audio}");
        assert_eq!(service.thread, original_thread);
        if cycle == 0 {
            // Exercise the actual worker expiry and controller cleanup with a test-only
            // shorter lifetime. The production microphone lifetime stays one hour.
            let deadline = Instant::now() + Duration::from_secs(30);
            let mut expired = false;
            while Instant::now() < deadline && !expired {
                service.pump(&mut controller, &wire);
                for event in received.try_iter() {
                    if let ChatEvent::Voice(VoiceStatus::Failed(error)) = event {
                        assert!(error.contains("one-hour limit"), "unexpected voice failure: {error}");
                        expired = true;
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(expired, "worker session expiry did not reach the controller");
        } else {
            controller.stop(&mut service.writer, &wire, &service.thread).unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline && (controller.stopping.is_some() || AUDIO_SLOT.load(Ordering::SeqCst)) {
            service.pump(&mut controller, &wire);
            for _ in received.try_iter() {}
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(controller.stopping.is_none(), "remote stop was not acknowledged");
        assert!(!AUDIO_SLOT.load(Ordering::SeqCst), "audio worker did not release its slot");
        for _ in received.try_iter() {}
        eprintln!("voice lifecycle cycle={cycle} active=true audio_received=true remote_stop_ack=true worker_released=true same_thread=true");
    }
    super::super::send(
        &mut service.writer,
        &json!({"id":991,"method":"turn/start","params":{
            "threadId":service.thread,"input":[{"type":"text","text":"Reply exactly VOICE_RECOVERY_READY. Do not use tools or edit anything."}]
        }}),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut answer = String::new();
    let mut complete = false;
    while Instant::now() < deadline && !complete {
        for message in service.pump(&mut controller, &wire) {
            let p = &message["params"];
            if p.get("threadId").and_then(Value::as_str) != Some(original_thread.as_str()) {
                continue;
            }
            match message.get("method").and_then(Value::as_str) {
                Some("item/agentMessage/delta") => {
                    let delta = p.get("delta").and_then(Value::as_str).unwrap_or_default();
                    assert!(answer.len().saturating_add(delta.len()) <= 16384);
                    answer.push_str(delta);
                }
                Some("turn/completed") => {
                    assert_eq!(p["turn"]["status"], "completed");
                    complete = true;
                }
                Some("error") if p.get("willRetry").and_then(Value::as_bool) != Some(true) => panic!("{}", super::super::diagnostic::error_details(p)),
                _ => {}
            }
        }
        for _ in received.try_iter() {}
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(complete && answer.contains("VOICE_RECOVERY_READY"), "text unavailable after voice restart");
    eprintln!("voice lifecycle text_after_restart=true same_thread=true generated_audio_only=true");
}

#[test]
#[ignore = "requires signed-in Codex, refusal-only MCP fixture, generated teach/recall PCM; no devices"]
fn short_reconnect_recalls_voice_only_phrase_without_devices() {
    fn samples(variable: &str) -> Vec<f32> {
        let path = std::env::var_os(variable).expect("set generated PCM fixture");
        let mut bytes = Vec::new();
        std::fs::File::open(path).unwrap().take(512 * 1024 + 1).read_to_end(&mut bytes).unwrap();
        assert!(!bytes.is_empty() && bytes.len() <= 512 * 1024 && bytes.len() % 2 == 0);
        bytes.as_chunks::<2>().0.iter().map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0).collect()
    }
    let fixtures = [samples("FILMCRAFT_VOICE_CONTEXT_TEACH_PCM"), samples("FILMCRAFT_VOICE_CONTEXT_RECALL_PCM")];
    let mut service = Service::open();
    let original_thread = service.thread.clone();
    let (_, commands) = mpsc::channel();
    let (events, received) = mpsc::sync_channel(64);
    let wire = chat::Wire { process: Arc::clone(&service.process), commands, events, ctx: egui::Context::default(), session: None };
    let mut controller = Controller::default();
    controller.restore_context(
        &service.thread,
        &filmcraft_ui_egui::console::sessions::SessionRequest {
            memory: "Generated voice recall test only; no tools or edits.".into(),
            assistant_summary: r#"{"role":"concert editor","partial":true}"#.into(),
            ..Default::default()
        },
    );
    for (cycle, samples) in fixtures.into_iter().enumerate() {
        if cycle == 1 {
            let handoff = serde_json::to_string(&controller.context.initial_items(&service.thread)).unwrap();
            assert!(handoff.to_lowercase().contains("copper"), "First generated voice instruction did not reach the handoff");
            if std::env::var_os("FILMCRAFT_VOICE_CONTEXT_BASELINE").is_some() {
                // Test-only comparison reproduces the old empty voice-start history.
                controller.context = Default::default();
            }
        }
        controller.prepare(&wire, 0).unwrap();
        controller.peer = Some(Peer::start(Input::Fixture { samples, lifetime: Duration::from_secs(90) }).unwrap());
        let deadline = Instant::now() + Duration::from_secs(85);
        let mut transcript = String::new();
        let mut active = false;
        while Instant::now() < deadline {
            service.pump(&mut controller, &wire);
            for event in received.try_iter() {
                match event {
                    ChatEvent::Voice(VoiceStatus::Active) => active = true,
                    ChatEvent::Voice(VoiceStatus::Failed(error)) => panic!("context cycle {cycle}: {error}"),
                    ChatEvent::VoiceText { role, text, .. } if role == "assistant" => {
                        assert!(transcript.len().saturating_add(text.len()) <= 128 * 1024);
                        transcript.push_str(&text);
                    }
                    _ => {}
                }
            }
            if active && transcript.to_lowercase().contains("copper otter") && (cycle == 0 || transcript.to_lowercase().contains("concert editor")) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(active && transcript.to_lowercase().contains("copper otter"), "cycle {cycle} missing generated voice phrase: {transcript}");
        assert!(cycle == 0 || transcript.to_lowercase().contains("concert editor"), "Voice lost its seeded editing role: {transcript}");
        eprintln!("VOICE CONTEXT TRANSCRIPT cycle={cycle} {transcript:?}");
        assert_eq!(service.thread, original_thread);
        controller.stop(&mut service.writer, &wire, &service.thread).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline && (controller.stopping.is_some() || AUDIO_SLOT.load(Ordering::SeqCst)) {
            service.pump(&mut controller, &wire);
            for _ in received.try_iter() {}
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(controller.stopping.is_none());
        assert!(!AUDIO_SLOT.load(Ordering::SeqCst));
        eprintln!("VOICE CONTEXT cycle={cycle} same_thread=true phrase_recalled=true generated_audio_only=true no_devices=true");
    }
}
