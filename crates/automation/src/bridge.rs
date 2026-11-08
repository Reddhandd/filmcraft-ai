//! Client for the desktop app's JSON-lines control protocol. A connection per request in flight,
//! reused for explicitly retry-safe inspections once its reply is validated. Requests that must
//! never be replayed start on a fresh connection. Only explicitly read-only requests may be retried.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

use crate::AutomationError;
#[path = "bridge/profile.rs"]
mod profile;

/// How often a blocking export's app job is polled.
const JOB_POLL: Duration = Duration::from_millis(250);
/// Idle connections kept for reuse; the app serves at most 16 at once.
const MAX_IDLE: usize = 4;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);

type Conn = (BufReader<tokio::net::tcp::OwnedReadHalf>, tokio::net::tcp::OwnedWriteHalf);

pub struct BridgeClient {
    addr: String,
    /// Inherited from the same owner-controlled environment as the desktop app.
    token: Option<String>,
    /// Ephemeral AI ownership credential; omitted for existing manual controller clients.
    provider_permit: Option<String>,
    external_provider: bool,
    provider_profile: Option<std::path::PathBuf>,
    /// Connections with no request in flight. A request takes one (or opens one) for its whole
    /// round trip and puts it back only once the reply is read, so a slow or abandoned request
    /// never holds up the others or leaves its late reply for the next one to read (#1538).
    idle: Mutex<Vec<Conn>>,
    next_id: AtomicU64,
}

impl BridgeClient {
    /// `addr` such as `127.0.0.1:9876` (loopback only). Reads `FILMCRAFT_CONTROL_TOKEN`
    /// from the environment; the desktop app requires this credential on every request.
    pub fn new(addr: impl Into<String>) -> Result<Self, AutomationError> {
        let addr = addr.into();
        let host = addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(&addr);
        if !matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1") {
            return Err(AutomationError::BadRequest(format!("bridge address must be loopback, got `{addr}`")));
        }
        let external_provider = match std::env::var("FILMCRAFT_EXTERNAL_PROVIDER") {
            Ok(value) if value == "desktopChatGpt" => true,
            Err(std::env::VarError::NotPresent) => false,
            _ => return Err(AutomationError::BadRequest("Invalid external provider configuration".into())),
        };
        let client = Self {
            addr,
            token: std::env::var("FILMCRAFT_CONTROL_TOKEN").ok(),
            provider_permit: None,
            external_provider,
            provider_profile: None,
            idle: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
        };
        match std::env::var("FILMCRAFT_PROVIDER_PERMIT") {
            Ok(permit) => client.with_provider_permit(permit),
            Err(std::env::VarError::NotPresent) if !external_provider => Ok(client),
            Err(std::env::VarError::NotPresent) => Err(AutomationError::BadRequest("External provider requires a private ownership permit".into())),
            Err(_) => Err(AutomationError::BadRequest("Invalid provider permit encoding".into())),
        }
    }

    pub fn with_provider_permit(mut self, permit: String) -> Result<Self, AutomationError> {
        if permit.len() != 64 || !permit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(AutomationError::BadRequest("Provider permit must be 64 hexadecimal characters".into()));
        }
        self.provider_permit = Some(permit);
        Ok(self)
    }

    /// Opt into the external lifecycle only with an explicitly bound private permit.
    pub fn with_external_provider(mut self) -> Result<Self, AutomationError> {
        if self.provider_permit.is_none() {
            return Err(AutomationError::BadRequest("External provider needs an ownership permit".into()));
        }
        self.external_provider = true;
        Ok(self)
    }
    pub(crate) fn is_external_provider(&self) -> bool {
        self.external_provider
    }

    pub(crate) fn provider_profile(&self) -> Option<&std::path::Path> {
        self.provider_profile.as_deref()
    }
    pub(crate) fn same_provider_permit(&self, other: &Self) -> bool {
        self.provider_permit == other.provider_permit
    }

    pub(crate) fn provider_permit_identity(&self) -> Option<&str> {
        self.provider_permit.as_deref()
    }
    pub(crate) async fn discard_idle_connections(&self) {
        self.idle.lock().await.clear();
    }

    /// Lifecycle uses the control credential and opaque permit, never a tool wrapper or retry.
    pub(crate) async fn external_event(&self, event: &str) -> Result<Value, AutomationError> {
        if !self.external_provider || !["authenticate", "attach", "heartbeat", "detach"].contains(&event) {
            return Err(AutomationError::BadRequest("Invalid external provider lifecycle".into()));
        }
        let permit = self.provider_permit.as_ref().ok_or_else(|| AutomationError::BadRequest("Missing external provider permit".into()))?;
        let raw = Self {
            addr: self.addr.clone(),
            token: self.token.clone(),
            provider_permit: None,
            external_provider: false,
            provider_profile: None,
            idle: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
        };
        tokio::time::timeout(Duration::from_secs(3), raw.call_with_timeout("provider.external", json!({"permit":permit,"event":event}), Duration::from_secs(3)))
            .await
            .map_err(|_| AutomationError::Bridge("External provider lifecycle timed out; attachment may have expired".into()))?
    }

    /// Run engine command `id` in the app. A blocking export (`file.exportMedia` with
    /// `wait: true`, see [`crate::long_job::LONG_COMMANDS`]) is started as an app job and polled
    /// here until it finishes, so the app keeps repainting (its status bar shows the progress) and
    /// answering other requests, and no request waits on the control server's 60 s reply limit
    /// (#91, #92). The reply is the same as a blocking export's: `{job, path, result}`, or the
    /// export's error.
    pub async fn execute(&self, id: &str, mut params: Value) -> Result<Value, AutomationError> {
        if !crate::long_job::is_long(id, &params) {
            return self.call("engine.execute", json!({"command": id, "params": params})).await;
        }
        if let Some(p) = params.as_object_mut() {
            p.insert("wait".into(), json!(false));
        }
        let mut start = self.call("engine.execute", json!({"command": id, "params": params})).await?;
        let Some(job) = start.get("job").and_then(Value::as_u64) else { return Ok(start) };
        let (result, offline) = loop {
            tokio::time::sleep(JOB_POLL).await;
            let jobs = self.call("engine.execute", json!({"command": "jobs.list", "params": {}})).await?;
            let Some(j) = jobs.as_array().and_then(|a| a.iter().find(|j| j["id"].as_u64() == Some(job))).cloned() else {
                break (Value::Null, None);
            };
            if j["finished"].as_bool() == Some(true) {
                break (j["result"].clone(), j.get("offlineMedia").cloned());
            }
        };
        if let Some(e) = result.get("error").and_then(Value::as_str) {
            return Err(AutomationError::App(format!("export failed: {e}")));
        }
        if let Some(o) = start.as_object_mut() {
            o.insert("result".into(), result);
            if let Some(off) = offline {
                o.insert("offlineMedia".into(), off);
            }
        }
        Ok(start)
    }

    /// Call a control method; returns `result` or the app's error. A transport failure after
    /// sending a mutation is an unknown outcome, not proof of rejection: never replay it.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, AutomationError> {
        self.call_with_timeout(method, params, REQUEST_TIMEOUT).await
    }

    async fn call_with_timeout(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, AutomationError> {
        let retry_safe = read_retry_safe(method, &params);
        for attempt in 0..2 {
            // Failed or cancelled exchanges are dropped, never pooled. Even a stale pooled
            // connection can have accepted part/all of a write before reporting an error.
            // The native server closes idle sockets after 120 seconds. A successful earlier
            // inspection does not prove its pooled transport is still live. Start ticket reads,
            // edits and other non-replayable requests fresh rather than discovering expiry after
            // writing, when their outcome would already be uncertain. This is not a retry.
            let idle = if retry_safe && attempt == 0 { self.idle.lock().await.pop() } else { None };
            let mut conn = match idle {
                Some(c) => c,
                None => self.connect().await?,
            };
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let request = match self.provider_permit.as_deref() {
                Some(permit) => json!({"id":id,"token":self.token,"method":"provider.execute","params":{"permit":permit,"method":method,"params":params}}),
                None => json!({"id":id,"token":self.token,"method":method,"params":params}),
            };
            let line = format!("{request}\n");
            match exchange(&mut conn.0, &mut conn.1, line.as_bytes(), id, timeout).await {
                Ok(v) if !retry_safe && v.get("ok").and_then(Value::as_bool) == Some(false) && v.get("error").and_then(Value::as_str) == Some("timeout") => {
                    // The server's UI-reply timeout does not cancel its already queued request.
                    return Err(unknown_outcome(method, &params, id, "app timed out waiting for the UI"));
                }
                Ok(v) => {
                    if retry_safe {
                        let mut idle = self.idle.lock().await;
                        if idle.len() < MAX_IDLE {
                            idle.push(conn);
                        }
                    }
                    return if v.get("ok").and_then(Value::as_bool) == Some(true) {
                        Ok(v.get("result").cloned().unwrap_or(Value::Null))
                    } else {
                        Err(AutomationError::App(v.get("error").and_then(Value::as_str).unwrap_or("error").to_string()))
                    };
                }
                Err(_) if retry_safe && attempt == 0 => {}
                Err(e) if retry_safe => return Err(e),
                Err(e) => return Err(unknown_outcome(method, &params, id, &e.to_string())),
            }
        }
        Err(AutomationError::Bridge("read-only request failed after reconnecting".into()))
    }

    async fn connect(&self) -> Result<Conn, AutomationError> {
        let s = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&self.addr))
            .await
            .map_err(|_| AutomationError::Bridge(format!("timed out connecting to {}", self.addr)))?
            .map_err(|e| AutomationError::Bridge(format!("cannot connect to {} ({e}); start the app with `filmcraft --control <port>`", self.addr)))?;
        let (r, w) = s.into_split();
        Ok((BufReader::new(r), w))
    }
}

fn unknown_outcome(method: &str, params: &Value, id: u64, reason: &str) -> AutomationError {
    let command = params.get("command").and_then(Value::as_str).unwrap_or(method);
    AutomationError::Bridge(format!(
        "outcome unknown for {command} (request {id}): {reason}. The request was not retried: it may already have been applied or may still execute. Do not automatically repeat it. Inspect the project/history or jobs.list and let the original request settle before deciding whether to retry; do not assume nothing changed."
    ))
}

/// A deliberately small allowlist, not `journal == false`: many non-journaled commands
/// change selection/playback, start jobs, allocate preview tickets or write files.
fn read_retry_safe(method: &str, params: &Value) -> bool {
    match method {
        "engine.commands"
        | "ui.menu.list"
        | "ui.inspect"
        | "ui.docking"
        | "perf.stats"
        | "ui.elements"
        | "ui.timeline.hit"
        | "ui.timeline.locate"
        | "controller.capabilities" => true,
        "ui.screenshot.inline" => params.get("path").is_none_or(Value::is_null),
        "engine.execute" => matches!(
            params.get("command").and_then(Value::as_str),
            Some(
                "project.inspect"
                    | "sequence.inspect"
                    | "jobs.list"
                    | "history.list"
                    | "prefs.get"
                    | "console.bridge.capabilities"
                    | "console.bridge.commands"
                    | "console.bridge.prepare"
                    | "console.bridge.status"
                    | "console.bridge.analysis.read"
            )
        ),
        _ => false,
    }
}

async fn exchange(
    reader: &mut (impl AsyncBufRead + Unpin),
    writer: &mut (impl AsyncWrite + Unpin),
    line: &[u8],
    id: u64,
    timeout: Duration,
) -> Result<Value, AutomationError> {
    tokio::time::timeout(timeout, async {
        writer.write_all(line).await.map_err(|e| AutomationError::Bridge(format!("request write failed: {e}")))?;
        let mut buf = String::new();
        reader.read_line(&mut buf).await.map_err(|e| AutomationError::Bridge(format!("reply read failed: {e}")))?;
        if !buf.ends_with('\n') {
            return Err(AutomationError::Bridge("connection closed before a complete reply".into()));
        }
        let reply: Value = serde_json::from_str(&buf).map_err(|e| AutomationError::Bridge(format!("bad reply: {e}")))?;
        if reply.get("id").and_then(Value::as_u64) != Some(id)
            || reply.get("ok").and_then(Value::as_bool).is_none()
            || (reply.get("ok").and_then(Value::as_bool) == Some(false) && reply.get("error").and_then(Value::as_str).is_none())
        {
            return Err(AutomationError::Bridge("reply has a mismatched request id or invalid status".into()));
        }
        Ok(reply)
    })
    .await
    .map_err(|_| AutomationError::Bridge("timed out sending request or waiting for reply".into()))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::net::TcpListener;

    /// A stand-in for the app's control server: the export starts job 7, which `jobs.list` reports
    /// running twice and then finished with `result`. Every request is recorded.
    async fn fake_app(result: Value) -> (String, Arc<Mutex<Vec<Value>>>) {
        fake_app_with(result, None).await
    }

    /// [`fake_app`] whose finished job also lists `offline` as its `offlineMedia`.
    async fn fake_app_with(result: Value, offline: Option<Value>) -> (String, Arc<Mutex<Vec<Value>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            while let Ok((s, _)) = listener.accept().await {
                let log = log.clone();
                let result = result.clone();
                let offline = offline.clone();
                tokio::spawn(async move {
                    let (r, mut w) = s.into_split();
                    let mut lines = BufReader::new(r).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        let req: Value = serde_json::from_str(&line).unwrap();
                        let polls = {
                            let mut requests = log.lock().await;
                            requests.push(req.clone());
                            requests.iter().filter(|q| q["params"]["command"] == "jobs.list").count()
                        };
                        let reply = match req["params"]["command"].as_str() {
                            Some("file.exportMedia") => json!({"job": 7, "path": "/tmp/out.mov"}),
                            Some("jobs.list") => {
                                let finished = polls >= 3;
                                let mut j = json!({"id": 7, "finished": finished, "result": if finished { result.clone() } else { Value::Null }});
                                if let Some(o) = offline.clone().filter(|_| finished) {
                                    j["offlineMedia"] = o;
                                }
                                json!([j])
                            }
                            _ => json!(null),
                        };
                        let line = format!("{}\n", json!({"id": req["id"], "ok": true, "result": reply}));
                        if w.write_all(line.as_bytes()).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        (addr, seen)
    }

    /// A blocking bridge export runs as an app job (#91, #92): the app never sees `wait: true` (it
    /// would encode on its UI thread and the reply would time out after 60 s), and the caller still
    /// gets the finished result.
    #[tokio::test]
    async fn a_blocking_export_is_polled_as_a_job() {
        let (addr, seen) = fake_app(json!({"frames": 642})).await;
        let b = BridgeClient::new(addr).unwrap();
        let r = b.execute("file.exportMedia", json!({"path": "/tmp/out.mov", "wait": true})).await.unwrap();
        assert_eq!(r, json!({"job": 7, "path": "/tmp/out.mov", "result": {"frames": 642}}));
        let seen = seen.lock().await;
        assert_eq!(seen[0]["params"]["params"]["wait"], json!(false));
        assert!(seen.iter().all(|q| q["params"]["params"]["wait"] != json!(true)));
        assert_eq!(seen.iter().filter(|q| q["params"]["command"] == "file.exportMedia").count(), 1, "started once");
        assert_eq!(seen.iter().filter(|q| q["params"]["command"] == "jobs.list").count(), 3);
    }

    /// The offline media a polled export drew comes back as in a blocking export's reply (#1054).
    #[tokio::test]
    async fn a_polled_export_reports_its_offline_media() {
        let off = json!([{"item": 8, "status": "missing"}]);
        let (addr, _) = fake_app_with(json!({"frames": 3}), Some(off.clone())).await;
        let b = BridgeClient::new(addr).unwrap();
        let r = b.execute("file.exportMedia", json!({"path": "/tmp/out.mov", "wait": true})).await.unwrap();
        assert_eq!(r["offlineMedia"], off, "{r}");
    }

    #[tokio::test]
    async fn a_failed_blocking_export_is_an_error() {
        let (addr, _) = fake_app(json!({"error": "encode: disk full"})).await;
        let b = BridgeClient::new(addr).unwrap();
        let e = b.execute("file.exportMedia", json!({"path": "/tmp/out.mov", "wait": true})).await.unwrap_err();
        assert!(e.to_string().contains("export failed: encode: disk full"), "{e}");
    }

    /// A stand-in for the app that serves any number of connections and answers each request on
    /// its own, as the app does: `slow` after `slow_reply` (never when `None`), anything else at
    /// once with its command id. The ids of the requests it receives are sent to the returned channel.
    async fn slow_app(slow_reply: Option<Duration>) -> (String, tokio::sync::mpsc::UnboundedReceiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let (seen, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok((s, _)) = listener.accept().await {
                let seen = seen.clone();
                tokio::spawn(async move {
                    let (r, w) = s.into_split();
                    let w = Arc::new(Mutex::new(w));
                    let mut lines = BufReader::new(r).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        let req: Value = serde_json::from_str(&line).unwrap();
                        let cmd = req["params"]["command"].as_str().unwrap_or_default().to_string();
                        let _ = seen.send(cmd.clone());
                        let delay = if cmd == "slow" { slow_reply } else { Some(Duration::ZERO) };
                        let w = w.clone();
                        tokio::spawn(async move {
                            let Some(d) = delay else { return };
                            tokio::time::sleep(d).await;
                            let line = format!("{}\n", json!({"id": req["id"], "ok": true, "result": cmd}));
                            let _ = w.lock().await.write_all(line.as_bytes()).await;
                        });
                    }
                });
            }
        });
        (addr, rx)
    }

    /// A command the app is slow to answer holds up only itself: a later call is answered at once
    /// instead of queueing behind it, and behind every call queued after it (#1538).
    #[tokio::test]
    async fn a_slow_command_does_not_block_later_calls() {
        let (addr, mut seen) = slow_app(None).await;
        let b = Arc::new(BridgeClient::new(addr).unwrap());
        let slow = tokio::spawn({
            let b = b.clone();
            async move { b.execute("slow", json!({})).await }
        });
        assert_eq!(seen.recv().await.as_deref(), Some("slow"));
        let fast = tokio::time::timeout(Duration::from_secs(5), b.execute("fast", json!({}))).await;
        assert_eq!(fast.expect("the later call waited on the slow one").unwrap(), json!("fast"));
        slow.abort();
    }

    /// A call given up while it waits leaves no late reply on a connection for the next call to read
    /// as its own (#1538).
    #[tokio::test]
    async fn an_abandoned_call_leaves_no_late_reply_behind() {
        let (addr, mut seen) = slow_app(Some(Duration::from_millis(100))).await;
        let b = Arc::new(BridgeClient::new(addr).unwrap());
        let slow = tokio::spawn({
            let b = b.clone();
            async move { b.execute("slow", json!({})).await }
        });
        assert_eq!(seen.recv().await.as_deref(), Some("slow"));
        slow.abort();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(b.execute("fast", json!({})).await.unwrap(), json!("fast"));
    }

    #[tokio::test]
    async fn reconnect_discards_idle_sockets_without_replaying_a_request() {
        let (addr, seen) = fake_app(Value::Null).await;
        let client = BridgeClient::new(addr).unwrap();
        client.execute("sequence.inspect", json!({})).await.unwrap();
        assert_eq!(client.idle.lock().await.len(), 1);
        client.discard_idle_connections().await;
        assert!(client.idle.lock().await.is_empty());
        assert_eq!(seen.lock().await.len(), 1);
    }

    /// Everything else is forwarded unchanged.
    #[tokio::test]
    async fn other_commands_are_forwarded() {
        let (addr, seen) = fake_app(Value::Null).await;
        let b = BridgeClient::new(addr).unwrap();
        b.execute("file.exportMedia", json!({"path": "/tmp/out.mov"})).await.unwrap();
        b.execute("sequence.inspect", json!({})).await.unwrap();
        let seen = seen.lock().await;
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[1]["params"]["command"], "sequence.inspect");
    }

    #[tokio::test]
    async fn credential_is_sent_on_every_request() {
        let (addr, seen) = fake_app(Value::Null).await;
        let mut b = BridgeClient::new(addr).unwrap();
        // Inject directly to avoid mutating the process environment in parallel tests.
        let token = "0123456789abcdef".repeat(4);
        b.token = Some(token.clone());
        b.call("ui.inspect", json!({})).await.unwrap();
        b.execute("file.saveAs", json!({"path": "/owner/project.fcproj"})).await.unwrap();
        let seen = seen.lock().await;
        assert_eq!(seen.len(), 2);
        assert!(seen.iter().all(|request| request["token"] == token));
        assert_eq!(seen[1]["params"]["params"]["path"], "/owner/project.fcproj");
    }
    #[tokio::test]
    async fn external_lifecycle_is_authenticated_without_recursive_tool_wrapping() {
        let (addr, seen) = fake_app(json!({"accepted":true})).await;
        let permit = "b".repeat(64);
        let mut client = BridgeClient::new(addr).unwrap().with_provider_permit(permit.clone()).unwrap();
        client.external_provider = true;
        client.token = Some("synthetic-control".into());
        client.external_event("authenticate").await.unwrap();
        client.external_event("attach").await.unwrap();
        client.external_event("heartbeat").await.unwrap();
        assert!(client.external_event("execute").await.is_err());
        client.external_event("detach").await.unwrap();
        let requests = seen.lock().await;
        assert_eq!(requests.len(), 4);
        for (request, event) in requests.iter().zip(["authenticate", "attach", "heartbeat", "detach"]) {
            assert_eq!(request["method"], "provider.external");
            assert_eq!(request["token"], "synthetic-control");
            assert_eq!(request["params"], json!({"permit":permit,"event":event}));
        }
    }
    #[tokio::test]
    async fn provider_guard_wraps_the_original_method_without_changing_its_parameters() {
        let (addr, seen) = fake_app(json!({})).await;
        let permit = "a".repeat(64);
        let client = BridgeClient::new(addr).unwrap().with_provider_permit(permit.clone()).unwrap();
        client.call("engine.commands", json!({"filter":"markers"})).await.unwrap();
        let requests = seen.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["method"], "provider.execute");
        assert_eq!(requests[0]["params"]["permit"], permit);
        assert_eq!(requests[0]["params"]["method"], "engine.commands");
        assert_eq!(requests[0]["params"]["params"], json!({"filter":"markers"}));
    }
}

#[cfg(test)]
#[path = "bridge_fault_tests.rs"]
mod fault_tests;
