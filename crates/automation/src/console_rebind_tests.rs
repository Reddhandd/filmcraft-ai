//! Synthetic lifecycle tests: no app, model, GUI, account or paid API.
use super::*;
use std::io::Write;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

struct HostState {
    permit: String,
    active: bool,
    events: Vec<String>,
    commands: Vec<String>,
}
struct Fixture {
    file: tempfile::NamedTempFile,
    port: u16,
    state: Arc<Mutex<HostState>>,
    pause_attach: Arc<AtomicBool>,
    pause_identity: Arc<AtomicBool>,
    fail_identity: Arc<AtomicBool>,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    task: tokio::task::JoinHandle<()>,
    service: FilmcraftConsoleMcp,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(Mutex::new(HostState { permit: "b".repeat(64), active: false, events: vec![], commands: vec![] }));
        let pause_attach = Arc::new(AtomicBool::new(false));
        let pause_identity = Arc::new(AtomicBool::new(false));
        let fail_identity = Arc::new(AtomicBool::new(false));
        let (native_pause, native_fail) = (pause_identity.clone(), fail_identity.clone());
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let (st, pause, en, rel) = (state.clone(), pause_attach.clone(), entered.clone(), release.clone());
        let task = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else { break };
                let (st, pause, en, rel) = (st.clone(), pause.clone(), en.clone(), rel.clone());
                let (native_pause, native_fail) = (native_pause.clone(), native_fail.clone());
                tokio::spawn(async move {
                    let mut reader = BufReader::new(socket);
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                            break;
                        }
                        let req: Value = serde_json::from_str(&line).unwrap();
                        let event = req["params"]["event"].as_str().unwrap_or("execute");
                        let accepted = {
                            let mut s = st.lock().unwrap();
                            s.events.push(event.to_owned());
                            if req["params"]["permit"] != s.permit {
                                false
                            } else {
                                match event {
                                    "authenticate" => true,
                                    "attach" if !s.active => {
                                        s.active = true;
                                        true
                                    }
                                    "heartbeat" => s.active,
                                    "detach" => {
                                        s.active = false;
                                        true
                                    }
                                    "execute" if s.active => {
                                        s.commands.push(req["params"]["params"]["command"].as_str().unwrap_or("").to_owned());
                                        true
                                    }
                                    _ => false,
                                }
                            }
                        };
                        if event == "attach" && accepted && pause.load(Ordering::SeqCst) {
                            en.notify_one();
                            rel.notified().await;
                        }
                        if event == "execute" && accepted && native_pause.load(Ordering::SeqCst) {
                            en.notify_one();
                            rel.notified().await;
                        }
                        let accepted = accepted && !(event == "execute" && native_fail.load(Ordering::SeqCst));
                        let reply = if accepted {
                            json!({"id":req["id"],"ok":true,"result":{"name":"Generated reconnect fixture","activeSequence":7,"revision":1,"rows":[]}})
                        } else {
                            json!({"id":req["id"],"ok":false,"error":"Provider connection was revoked; request was not executed"})
                        };
                        if reader.get_mut().write_all(format!("{reply}\n").as_bytes()).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        let mut file = tempfile::NamedTempFile::new().unwrap();
        Self::write_profile(&mut file, port, "b");
        let service = FilmcraftConsoleMcp::with_client(BridgeClient::from_provider_profile(file.path()).unwrap());
        service.external_initialized.store(true, Ordering::SeqCst);
        Self { file, port, state, pause_attach, pause_identity, fail_identity, entered, release, task, service }
    }
    fn write_profile(file: &mut tempfile::NamedTempFile, port: u16, permit: &str) {
        file.as_file_mut().set_len(0).unwrap();
        use std::io::{Seek, SeekFrom};
        file.as_file_mut().seek(SeekFrom::Start(0)).unwrap();
        file.write_all(
            serde_json::to_string(&json!({"version":1,"provider":"desktopChatGpt","port":port,"controlToken":"a".repeat(64),"permit":permit.repeat(64)}))
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
        file.flush().unwrap();
    }
    fn rotate(&mut self, permit: &str) {
        Self::write_profile(&mut self.file, self.port, permit);
        let mut s = self.state.lock().unwrap();
        s.permit = permit.repeat(64);
        s.active = false;
    }
    async fn bound(&self) -> FilmcraftConsoleMcp {
        FilmcraftConsoleMcp { generation: self.service.session.read().await.current.clone(), ..self.service.clone() }
    }
}
#[tokio::test]
async fn explicit_rebind_recovers_without_replaying_edits_and_rejects_old_generation() {
    let mut f = Fixture::new().await;
    f.service.request("markers.add", json!({"name":"once"})).await.unwrap();
    f.rotate("c");
    assert!(f.service.request("project.inspectPage", json!({})).await.is_err());
    assert!(f.service.rebind(1, false).await.is_err());
    let result = f.service.rebind(1, true).await.unwrap();
    assert_eq!(result["generation"], 2);
    assert_eq!(result["replayedCommands"], 0);
    assert!(f.service.request("markers.add", json!({})).await.is_err());
    f.bound().await.request("project.inspectPage", json!({})).await.unwrap();
    assert_eq!(f.state.lock().unwrap().commands.iter().filter(|s| s.as_str() == "markers.add").count(), 1);
    assert!(f.service.rebind(2, true).await.unwrap_err().contains("unchanged"));
    f.rotate("b");
    assert!(f.service.rebind(2, true).await.unwrap_err().contains("already attempted"));
}
#[tokio::test]
async fn concurrent_rebind_has_one_winner_and_second_owner_is_never_detached() {
    let mut f = Fixture::new().await;
    f.rotate("c");
    let (a, b) = tokio::join!(f.service.rebind(1, true), f.service.rebind(1, true));
    assert_ne!(a.is_ok(), b.is_ok());
    assert_eq!(f.state.lock().unwrap().events.iter().filter(|e| e.as_str() == "attach").count(), 1);
    f.rotate("d");
    f.state.lock().unwrap().active = true;
    assert!(f.service.rebind(2, true).await.is_err());
    assert!(f.state.lock().unwrap().active);
    assert!(!f.state.lock().unwrap().events.iter().any(|e| e == "detach"));
    assert!(f.service.rebind(2, true).await.unwrap_err().contains("already attempted"));
}
#[tokio::test]
async fn cancellation_after_host_accepts_attach_quarantines_permit_without_detaching_owner() {
    let mut f = Fixture::new().await;
    f.rotate("c");
    f.pause_attach.store(true, Ordering::SeqCst);
    let s = f.service.clone();
    let job = tokio::spawn(async move { s.rebind(1, true).await });
    tokio::time::timeout(Duration::from_secs(2), f.entered.notified()).await.unwrap();
    job.abort();
    let _ = job.await;
    f.release.notify_one();
    assert!(f.service.rebind(1, true).await.unwrap_err().contains("already attempted"));
    assert!(f.service.request("markers.add", json!({})).await.is_err());
    assert!(f.state.lock().unwrap().active);
    assert!(!f.state.lock().unwrap().events.iter().any(|e| e == "detach"));
    f.rotate("d");
    f.pause_attach.store(false, Ordering::SeqCst);
    f.service.rebind(1, true).await.unwrap();
    f.bound().await.request("project.inspectPage", json!({})).await.unwrap();
}
#[tokio::test]
async fn attach_timeout_and_initial_attach_cancellation_are_not_retried() {
    let mut f = Fixture::new().await;
    f.rotate("c");
    f.pause_attach.store(true, Ordering::SeqCst);
    assert!(f.service.rebind(1, true).await.is_err());
    assert!(f.service.rebind(1, true).await.unwrap_err().contains("already attempted"));
    f.release.notify_one();
    let f = Fixture::new().await;
    f.pause_attach.store(true, Ordering::SeqCst);
    let s = f.service.clone();
    let job = tokio::spawn(async move { s.request("markers.add", json!({})).await });
    f.entered.notified().await;
    job.abort();
    let _ = job.await;
    f.release.notify_one();
    assert!(f.service.request("markers.add", json!({})).await.is_err());
    assert_eq!(f.state.lock().unwrap().events.iter().filter(|e| e.as_str() == "attach").count(), 1);
}
#[tokio::test]
async fn exclusive_rebind_is_bounded_and_does_not_serialize_native_readers() {
    let mut f = Fixture::new().await;
    f.rotate("c");
    let read = f.service.session.read().await;
    let second = tokio::time::timeout(Duration::from_millis(100), f.service.session.read()).await.unwrap();
    assert!(f.service.rebind(1, true).await.unwrap_err().contains("busy"));
    drop(second);
    drop(read);
    f.service.rebind(1, true).await.unwrap();
    assert!(f.service.rebind(1, true).await.unwrap_err().contains("Stale"));
}
#[tokio::test]
async fn malformed_rebind_and_non_profile_clients_have_no_external_side_effects() {
    for v in [json!({}), json!({"confirm":"yes"}), json!({"confirm":true,"path":"/tmp/other"}), json!({"confirm":true,"permit":"x"})] {
        assert!(serde_json::from_value::<ConsoleRebindParams>(v).is_err());
    }
    let f = Fixture::new().await;
    let client = BridgeClient::from_provider_profile(f.file.path()).unwrap();
    let plain = FilmcraftConsoleMcp::with_client(BridgeClient::new(format!("127.0.0.1:{}", f.port)).unwrap());
    plain.external_initialized.store(true, Ordering::SeqCst);
    assert!(plain.rebind(1, true).await.is_err());
    assert!(f.state.lock().unwrap().events.is_empty());
    assert!(client.provider_profile().is_some());
}
#[tokio::test]
async fn request_queued_before_rebind_cannot_cross_into_new_generation() {
    let mut f = Fixture::new().await;
    f.rotate("c");
    f.pause_attach.store(true, Ordering::SeqCst);
    let s = f.service.clone();
    let reconnect = tokio::spawn(async move { s.rebind(1, true).await });
    f.entered.notified().await;
    let s = f.service.clone();
    let queued = tokio::spawn(async move { s.pin_generation(1).await.map(|g| g.current.id) });
    tokio::task::yield_now().await;
    assert!(!queued.is_finished());
    f.release.notify_one();
    reconnect.await.unwrap().unwrap();
    assert!(queued.await.unwrap().unwrap_err().contains("Stale queued"));
    assert_eq!(f.state.lock().unwrap().commands, vec!["project.inspectPage"]);
}
#[tokio::test]
async fn late_old_heartbeat_failure_and_cleanup_do_not_revoke_new_owner() {
    let mut f = Fixture::new().await;
    f.service.request("project.inspectPage", json!({})).await.unwrap();
    let old = f.service.generation.clone();
    let old_bridge = old.inner.bridge_client().unwrap();
    f.rotate("c");
    f.service.rebind(1, true).await.unwrap();
    assert!(old_bridge.external_event("heartbeat").await.is_err());
    old.revoked.store(true, Ordering::SeqCst);
    assert!(old_bridge.external_event("detach").await.is_err());
    let bound = f.bound().await;
    assert!(!bound.generation.revoked.load(Ordering::SeqCst));
    bound.request("project.inspectPage", json!({})).await.unwrap();
    assert!(f.state.lock().unwrap().active);
}
#[tokio::test]
async fn rejected_initial_attachment_is_one_shot_without_detach() {
    let f = Fixture::new().await;
    f.state.lock().unwrap().active = true;
    assert!(f.service.request("markers.add", json!({})).await.is_err());
    assert!(f.service.request("markers.add", json!({})).await.is_err());
    let s = f.state.lock().unwrap();
    assert_eq!(s.events, vec!["attach"]);
    assert!(s.commands.is_empty());
    assert!(s.active);
}

#[tokio::test]
async fn cancelled_profile_load_retains_single_worker_admission_until_actual_exit() {
    let f = Fixture::new().await;
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let entered = Arc::new(tokio::sync::Notify::new());
    let signal = entered.clone();
    let s = f.service.clone();
    let job = tokio::spawn(async move {
        s.load_profile(move || {
            signal.notify_one();
            let _ = release_rx.recv();
            Err(AutomationError::BadRequest("generated read failure".into()))
        })
        .await
    });
    entered.notified().await;
    job.abort();
    let _ = job.await;
    assert!(f.service.load_profile(|| Err(AutomationError::BadRequest("must not run".into()))).await.err().unwrap().contains("still pending"));
    release_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.service.profile_loader.available_permits() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(f.service.load_profile(|| Err(AutomationError::BadRequest("worker exited".into()))).await.err().unwrap().contains("worker exited"));
}
#[tokio::test]
async fn profile_load_timeout_bounds_waiter_and_does_not_free_worker_admission() {
    let f = Fixture::new().await;
    let (tx, rx) = std::sync::mpsc::channel();
    let error = f
        .service
        .load_profile(move || {
            let _ = rx.recv();
            Err(AutomationError::BadRequest("generated read failure".into()))
        })
        .await
        .err()
        .unwrap();
    assert!(error.contains("timed out"));
    assert_eq!(f.service.profile_loader.available_permits(), 0);
    assert!(f.service.load_profile(|| Err(AutomationError::BadRequest("must not run".into()))).await.err().unwrap().contains("still pending"));
    tx.send(()).unwrap();
}

#[tokio::test]
async fn cancellation_after_confirmed_attach_during_identity_does_not_publish_or_replay() {
    let mut f = Fixture::new().await;
    f.rotate("c");
    f.pause_identity.store(true, Ordering::SeqCst);
    let s = f.service.clone();
    let job = tokio::spawn(async move { s.rebind(1, true).await });
    tokio::time::timeout(Duration::from_secs(2), f.entered.notified()).await.unwrap();
    assert!(f.state.lock().unwrap().active);
    job.abort();
    let _ = job.await;
    assert_eq!(f.service.epoch.load(Ordering::SeqCst), 1);
    assert!(f.service.rebind(1, true).await.unwrap_err().contains("already attempted"));
    assert!(f.service.request("markers.add", json!({})).await.is_err());
    // Unpublished attachment receives no heartbeat; native host's lease is the cancellation fallback.
    assert!(!f.state.lock().unwrap().events.iter().any(|e| e == "heartbeat" || e == "detach"));
    f.rotate("d");
    f.pause_identity.store(false, Ordering::SeqCst);
    f.service.rebind(1, true).await.unwrap();
    f.release.notify_one();
    f.bound().await.request("project.inspectPage", json!({})).await.unwrap();
    assert!(!f.state.lock().unwrap().commands.iter().any(|c| c == "markers.add"));
}
#[tokio::test]
async fn failed_identity_after_confirmed_attach_quarantines_candidate_until_fresh_connect() {
    let mut f = Fixture::new().await;
    f.rotate("c");
    f.fail_identity.store(true, Ordering::SeqCst);
    assert!(f.service.rebind(1, true).await.is_err());
    assert_eq!(f.service.epoch.load(Ordering::SeqCst), 1);
    assert!(f.service.rebind(1, true).await.unwrap_err().contains("already attempted"));
    assert!(f.service.request("markers.add", json!({})).await.is_err());
    assert!(!f.state.lock().unwrap().events.iter().any(|e| e == "heartbeat" || e == "detach"));
    f.rotate("d");
    f.fail_identity.store(false, Ordering::SeqCst);
    f.service.rebind(1, true).await.unwrap();
    f.bound().await.request("project.inspectPage", json!({})).await.unwrap();
    assert!(!f.state.lock().unwrap().commands.iter().any(|c| c == "markers.add"));
}
