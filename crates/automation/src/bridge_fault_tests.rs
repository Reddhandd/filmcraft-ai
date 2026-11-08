//! Fault injection against a disposable native session and an in-memory export queue.
use super::*;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::net::TcpListener;
use tokio::task::{JoinHandle, JoinSet};

#[tokio::test]
async fn a_ticket_request_does_not_reuse_an_expired_inspection_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = BridgeClient::new(listener.local_addr().unwrap().to_string()).unwrap();
    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        let inspection: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(inspection["method"], "ui.inspect");
        write.write_all(format!("{}\n", json!({"id":inspection["id"],"ok":true,"result":{}})).as_bytes()).await.unwrap();
        // Model the native server's 120-second idle expiry without sleeping for two minutes.
        drop(lines);
        drop(write);
        closed_tx.send(()).unwrap();
        let (stream, _) = listener.accept().await.unwrap();
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        let ticket: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(ticket["params"]["command"], "console.attachments.pdf.read");
        write.write_all(format!("{}\n", json!({"id":ticket["id"],"ok":true,"result":{"ticket":7}})).as_bytes()).await.unwrap();
        ticket
    });
    client.call("ui.inspect", json!({})).await.unwrap();
    closed_rx.await.unwrap();
    let result =
        tokio::time::timeout(Duration::from_secs(2), client.execute("console.attachments.pdf.read", json!({"snapshotId":1,"documentEpoch":1,"page":317})))
            .await
            .unwrap();
    assert_eq!(result.unwrap(), json!({"ticket":7}));
    let request = server.await.unwrap();
    assert_eq!(request["params"]["params"]["page"], 317);
}

#[derive(Clone, Copy, Debug)]
enum Fault {
    Lost,
    Partial,
    Unterminated,
    Malformed,
    WrongId,
    MissingStatus,
    InvalidStatus,
    InvalidError,
    Timeout,
    UiReplyTimeout,
    DeferredUiReplyTimeout,
    OmittedResult,
    Rejected,
}
struct App {
    session: filmcraft_engine::Session,
    requests: Vec<String>,
    jobs: Vec<Value>,
    first: bool,
    deferred: Arc<tokio::sync::Notify>,
}
async fn fault_app(fault: Fault) -> (BridgeClient, Arc<Mutex<App>>, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut session = filmcraft_engine::Session::default();
    session.execute("file.newSequence", json!({"width":16,"height":16})).unwrap();
    let app = Arc::new(Mutex::new(App { session, requests: vec![], jobs: vec![], first: true, deferred: Arc::new(tokio::sync::Notify::new()) }));
    let client = BridgeClient::new(listener.local_addr().unwrap().to_string()).unwrap();
    let state = Arc::clone(&app);
    let server = tokio::spawn(async move {
        // Dropping the server also aborts blackholed connections from timeout cases.
        let mut connections = JoinSet::new();
        while let Ok((stream, _)) = listener.accept().await {
            let state = Arc::clone(&state);
            connections.spawn(async move {
                let (r, mut w) = stream.into_split();
                let mut lines = BufReader::new(r).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let q: Value = serde_json::from_str(&line).unwrap();
                    let command = if q["method"] == "engine.execute" { q["params"]["command"].as_str().unwrap() } else { q["method"].as_str().unwrap() };
                    let (first, result) = {
                        let mut app = state.lock().await;
                        app.requests.push(command.into());
                        let first = std::mem::take(&mut app.first);
                        let result = match command {
                            "sequence.inspect" => app.session.execute(command, json!({})).unwrap(),
                            "jobs.list" => json!(app.jobs),
                            "file.exportMedia" => {
                                let id = app.jobs.len() + 1;
                                app.jobs.push(json!({"id":id,"finished":false,"result":null}));
                                json!({"job":id,"path":"/test-only/not-written.mp4"})
                            }
                            _ if first && matches!(fault, Fault::Rejected | Fault::DeferredUiReplyTimeout) => Value::Null,
                            _ => app.session.execute("markers.add", json!({"target":"program","time":0,"name":"Applied once"})).unwrap(),
                        };
                        (first, result)
                    };
                    let reply = if first {
                        match fault {
                            Fault::Lost => return,
                            Fault::Partial => {
                                w.write_all(b"{\"id\":").await.unwrap();
                                return;
                            }
                            Fault::Unterminated => {
                                w.write_all(json!({"id":q["id"],"ok":true,"result":result}).to_string().as_bytes()).await.unwrap();
                                return;
                            }
                            Fault::Malformed => "not-json\n".to_string(),
                            Fault::WrongId => format!("{}\n", json!({"id":999,"ok":true,"result":result})),
                            Fault::MissingStatus => format!("{}\n", json!({"id":q["id"],"result":result})),
                            Fault::InvalidStatus => format!("{}\n", json!({"id":q["id"],"ok":"true","result":result})),
                            Fault::InvalidError => format!("{}\n", json!({"id":q["id"],"ok":false,"error":{}})),
                            Fault::Timeout => std::future::pending::<String>().await,
                            Fault::UiReplyTimeout | Fault::DeferredUiReplyTimeout => format!("{}\n", json!({"id":q["id"],"ok":false,"error":"timeout"})),
                            Fault::OmittedResult => format!("{}\n", json!({"id":q["id"],"ok":true})),
                            Fault::Rejected => format!("{}\n", json!({"id":q["id"],"ok":false,"error":"synthetic rejection"})),
                        }
                    } else {
                        format!("{}\n", json!({"id":q["id"],"ok":true,"result":result}))
                    };
                    if w.write_all(reply.as_bytes()).await.is_err() {
                        return;
                    }
                    if first && matches!(fault, Fault::DeferredUiReplyTimeout) {
                        let gate = Arc::clone(&state.lock().await.deferred);
                        gate.notified().await;
                        state.lock().await.session.execute("markers.add", json!({"target":"program","time":0,"name":"Applied later"})).unwrap();
                    }
                }
            });
            while connections.try_join_next().is_some() {}
        }
    });
    (client, app, server)
}

#[tokio::test]
async fn applied_edits_with_uncertain_replies_are_never_replayed() {
    for fault in [
        Fault::Lost,
        Fault::Partial,
        Fault::Unterminated,
        Fault::Malformed,
        Fault::WrongId,
        Fault::MissingStatus,
        Fault::InvalidStatus,
        Fault::InvalidError,
        Fault::Timeout,
        Fault::UiReplyTimeout,
    ] {
        let (client, app, server) = fault_app(fault).await;
        let err = client.call_with_timeout("engine.execute", json!({"command":"markers.add","params":{}}), Duration::from_millis(150)).await.unwrap_err();
        assert!(matches!(err, AutomationError::Bridge(_)), "{fault:?}: {err}");
        let message = err.to_string();
        for expected in ["outcome unknown", "not retried", "Inspect", "jobs.list"] {
            assert!(message.contains(expected), "{fault:?}: {message}");
        }
        let actual = client.execute("sequence.inspect", json!({})).await.unwrap();
        assert_eq!(actual["markers"].as_array().unwrap().len(), 1, "{fault:?}: {actual}");
        assert_eq!(app.lock().await.requests, ["markers.add", "sequence.inspect"]);
        server.abort();
    }
}

#[tokio::test]
async fn a_server_timeout_can_precede_the_original_edit_executing() {
    let (client, app, server) = fault_app(Fault::DeferredUiReplyTimeout).await;
    let error = client.execute("markers.add", json!({})).await.unwrap_err();
    assert!(error.to_string().contains("may still execute"), "{error}");
    let before = client.execute("sequence.inspect", json!({})).await.unwrap();
    assert_eq!(before["markers"], json!([]), "absence is not proof that the request was cancelled");
    app.lock().await.deferred.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if app.lock().await.session.active_sequence().unwrap().markers.len() == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let after = client.execute("sequence.inspect", json!({})).await.unwrap();
    assert_eq!(after["markers"].as_array().unwrap().len(), 1);
    assert_eq!(app.lock().await.requests.iter().filter(|s| s.as_str() == "markers.add").count(), 1);
    server.abort();
}

#[tokio::test]
async fn lost_export_start_reply_creates_only_one_job_even_with_wait_true() {
    let (client, app, server) = fault_app(Fault::Lost).await;
    let error = client.execute("file.exportMedia", json!({"path":"/test-only/not-written.mp4","wait":true})).await.unwrap_err();
    assert!(error.to_string().contains("outcome unknown"), "{error}");
    let jobs = client.execute("jobs.list", json!({})).await.unwrap();
    assert_eq!(jobs.as_array().unwrap().len(), 1);
    assert_eq!(app.lock().await.requests, ["file.exportMedia", "jobs.list"]);
    server.abort();
}

#[tokio::test]
async fn nested_console_and_ui_requests_are_not_assumed_read_only() {
    for (method, params) in [
        ("engine.execute", json!({"command":"console.bridge.execute","params":{"command":"markers.add","params":{}}})),
        ("engine.execute", json!({"command":"console.bridge.batch","params":{"steps":[{"id":"markers.add","params":{}}]}})),
        ("engine.execute", json!({"command":"console.bridge.analyze","params":{"kind":"music","song":1}})),
        ("ui.menu.invoke", json!({"id":"markers.add","params":{}})),
    ] {
        let (client, app, server) = fault_app(Fault::Lost).await;
        let error = client.call(method, params).await.unwrap_err();
        assert!(error.to_string().contains("outcome unknown"), "{error}");
        let actual = client.execute("sequence.inspect", json!({})).await.unwrap();
        assert_eq!(actual["markers"].as_array().unwrap().len(), 1);
        assert_eq!(app.lock().await.requests.len(), 2);
        server.abort();
    }
}

#[tokio::test]
async fn inspections_retry_once_on_a_fresh_connection() {
    for fault in [
        Fault::Lost,
        Fault::Partial,
        Fault::Unterminated,
        Fault::Malformed,
        Fault::WrongId,
        Fault::MissingStatus,
        Fault::InvalidStatus,
        Fault::InvalidError,
        Fault::Timeout,
    ] {
        let (client, app, server) = fault_app(fault).await;
        let actual = client.call_with_timeout("engine.execute", json!({"command":"sequence.inspect","params":{}}), Duration::from_millis(150)).await.unwrap();
        assert_eq!(actual["markers"], json!([]), "{fault:?}");
        assert_eq!(app.lock().await.requests, ["sequence.inspect", "sequence.inspect"]);
        server.abort();
    }
}

#[tokio::test]
async fn a_valid_app_rejection_is_not_an_unknown_transport_outcome() {
    let (client, app, server) = fault_app(Fault::Rejected).await;
    let error = client.execute("markers.add", json!({})).await.unwrap_err();
    assert!(matches!(error, AutomationError::App(_)), "{error}");
    assert!(!error.to_string().contains("outcome unknown"));
    let actual = client.execute("sequence.inspect", json!({})).await.unwrap();
    assert_eq!(actual["markers"], json!([]));
    assert_eq!(app.lock().await.requests.len(), 2);
    server.abort();
}

#[tokio::test]
async fn a_valid_reply_without_result_remains_compatible() {
    let (client, app, server) = fault_app(Fault::OmittedResult).await;
    assert_eq!(client.execute("markers.add", json!({})).await.unwrap(), Value::Null);
    let actual = client.execute("sequence.inspect", json!({})).await.unwrap();
    assert_eq!(actual["markers"].as_array().unwrap().len(), 1);
    assert_eq!(app.lock().await.requests.len(), 2);
    server.abort();
}

struct PartialWrite {
    writes: usize,
    bytes: usize,
}
impl AsyncWrite for PartialWrite {
    fn poll_write(mut self: Pin<&mut Self>, _: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
        self.writes += 1;
        if self.writes == 1 {
            let n = bytes.len().min(3);
            self.bytes += n;
            Poll::Ready(Ok(n))
        } else {
            Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()))
        }
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
#[tokio::test]
async fn a_partial_write_error_does_not_rewind_or_repeat_the_request() {
    let mut writer = PartialWrite { writes: 0, bytes: 0 };
    let mut reader = BufReader::new(tokio::io::empty());
    let error = exchange(&mut reader, &mut writer, b"an incomplete request\n", 1, Duration::from_secs(1)).await.unwrap_err();
    assert!(error.to_string().contains("request write failed"), "{error}");
    assert_eq!((writer.writes, writer.bytes), (2, 3));
    assert!(!read_retry_safe("engine.execute", &json!({"command":"markers.add"})));
}
#[tokio::test]
async fn a_blocked_write_has_a_deadline_and_leaves_an_incomplete_request() {
    use tokio::io::AsyncReadExt;
    let (mut writer, mut peer) = tokio::io::duplex(3);
    let mut reader = BufReader::new(tokio::io::empty());
    let error = exchange(&mut reader, &mut writer, b"an incomplete request\n", 1, Duration::from_millis(20)).await.unwrap_err();
    assert!(error.to_string().contains("timed out"), "{error}");
    drop(writer);
    let mut bytes = vec![];
    peer.read_to_end(&mut bytes).await.unwrap();
    assert_eq!(bytes, b"an ");
}
#[test]
fn retry_safety_is_explicit_and_excludes_transient_and_nested_mutations() {
    for command in [
        "markers.add",
        "playhead.set",
        "jobs.cancel",
        "controller.marker.preview",
        "controller.marker.commit",
        "console.bridge.execute",
        "console.bridge.batch",
        "console.bridge.analyze",
        "console.bridge.memory",
        "future.command",
    ] {
        assert!(!read_retry_safe("engine.execute", &json!({"command":command})), "{command}");
    }
    for method in ["ui.click", "ui.type", "ui.key", "ui.screenshot", "ui.menu.invoke", "app.quit", "future.inspect"] {
        assert!(!read_retry_safe(method, &json!({})), "{method}");
    }
    assert!(read_retry_safe("ui.inspect", &json!({})));
    assert!(read_retry_safe("engine.execute", &json!({"command":"jobs.list"})));
}
