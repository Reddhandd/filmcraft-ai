//! Supervised asynchronous MCP pipes: cancellation/deadlines include writes and reads.
use super::{
    protocol::{Call, Tool},
    turn::Tools,
};
use serde_json::{Value, json};
use std::{
    path::Path,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    runtime::{Builder, Runtime},
    time::{Instant, timeout, timeout_at},
};
const MAX_MESSAGE: usize = 16 * 1024 * 1024;
fn api_definitions(catalog: &Value) -> Result<Vec<Tool>, String> {
    if catalog.get("nextCursor").is_some_and(|value| !value.is_null()) {
        return Err("Unexpected paginated native tool catalog; helper does not match this build".into());
    }
    let tools = catalog.get("tools").and_then(Value::as_array).filter(|tools| !tools.is_empty() && tools.len() <= 32).ok_or("Invalid native tool catalog")?;
    let mut names = std::collections::HashSet::new();
    let mut definitions = Vec::new();
    for tool in tools {
        let name = tool.get("name").and_then(Value::as_str).filter(|name| name.len() <= 64).ok_or("Native tool is missing its name")?;
        if !names.insert(name.to_owned()) {
            return Err("Duplicate native tool name".into());
        }
        let description = tool.get("description").and_then(Value::as_str).unwrap_or("");
        if description.len() > 8192 {
            return Err("Native tool description exceeds budget".into());
        }
        let parameters = tool.get("inputSchema").filter(|schema| schema.is_object()).ok_or("Native tool is missing its input schema")?.clone();
        // The shared helper also advertises Desktop's explicit reconnect endpoint.
        // API ownership is managed in-process: neither advertise nor dispatch this tool.
        // Validate its bounds/schema above, and reject every other unknown tool below.
        if name == "connection_rebind" {
            continue;
        }
        if !filmcraft_ui_egui::console::capabilities::CONSOLE_TOOLS.contains(&name) {
            return Err("Native helper exposed a tool outside the Console contract".into());
        }
        definitions.push(Tool { name: name.into(), description: description.into(), parameters });
    }
    if definitions.is_empty() {
        return Err("Native helper exposed no API Console tools".into());
    }
    Ok(definitions)
}
struct Pipe<W, R> {
    writer: W,
    reader: R,
    stopped: Arc<AtomicBool>,
    next: u64,
    definitions: Vec<Tool>,
    broken: bool,
}
pub struct Client<W: AsyncWrite + Unpin, R: AsyncBufRead + Unpin> {
    pipe: Pipe<W, R>,
    runtime: Option<Runtime>,
    child: Option<Child>,
}
pub type NativeTools = Client<ChildStdin, BufReader<ChildStdout>>;
async fn frame(reader: &mut (impl AsyncBufRead + Unpin)) -> Result<Option<Value>, String> {
    let mut bytes = Vec::new();
    reader.take((MAX_MESSAGE + 1) as u64).read_until(b'\n', &mut bytes).await.map_err(|_| "Native MCP helper read failed")?;
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes.len() > MAX_MESSAGE || bytes.last() != Some(&b'\n') {
        return Err("Native MCP helper sent an oversized or incomplete message".into());
    }
    serde_json::from_slice(&bytes).map(Some).map_err(|_| "Native MCP helper sent malformed JSON".into())
}
async fn cancelled(stop: &AtomicBool) {
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
async fn bounded<T>(future: impl std::future::Future<Output = Result<T, String>>, stop: &AtomicBool, deadline: Instant, phase: &str) -> Result<T, String> {
    tokio::select! {biased;
        ()=cancelled(stop)=>Err(format!("Native MCP {phase} cancelled; its outcome may be unknown and was not replayed")),
        result=timeout_at(deadline,future)=>result.map_err(|_|format!("Native MCP {phase} timed out; its outcome may be unknown and was not replayed"))?,
    }
}
async fn write_frame(writer: &mut (impl AsyncWrite + Unpin), value: &Value, stop: &AtomicBool, deadline: Instant) -> Result<(), String> {
    let mut bytes = super::protocol::bounded_json(value, 128 * 1024 - 1)?;
    bytes.push(b'\n');
    bounded(
        async {
            writer.write_all(&bytes).await.map_err(|_| "Native MCP request write failed; its outcome may be unknown")?;
            writer.flush().await.map_err(|_| "Native MCP request flush failed; its outcome may be unknown".into())
        },
        stop,
        deadline,
        "write",
    )
    .await
}
async fn read_reply(reader: &mut (impl AsyncBufRead + Unpin), id: u64, stop: &AtomicBool, deadline: Instant) -> Result<Value, String> {
    for _ in 0..4096 {
        let value = bounded(frame(reader), stop, deadline, "read").await?.ok_or("Native MCP helper stopped; inspect state before repeating edits")?;
        if value.get("id").is_none() && value.get("method").and_then(Value::as_str).is_some() {
            continue;
        }
        if value.get("id").and_then(Value::as_u64) != Some(id) {
            return Err("Native MCP reply identity mismatch; request was not replayed".into());
        }
        if value.get("error").is_some() {
            return Err("Native MCP request was rejected; inspect parameters and current state".into());
        }
        return value.get("result").filter(|value| value.is_object()).cloned().ok_or_else(|| "Native MCP response omitted its result".into());
    }
    Err("Native MCP notification budget exceeded".into())
}
impl<W: AsyncWrite + Unpin, R: AsyncBufRead + Unpin> Pipe<W, R> {
    async fn send(&mut self, value: &Value, deadline: Instant) -> Result<(), String> {
        write_frame(&mut self.writer, value, &self.stopped, deadline).await
    }
    async fn rpc(&mut self, method: &str, params: Value, deadline: Instant) -> Result<Value, String> {
        if self.broken || self.stopped.load(Ordering::SeqCst) {
            return Err("Native MCP connection is closed".into());
        }
        let id = self.next;
        self.next = self.next.checked_add(1).ok_or("Native MCP request identity exhausted")?;
        let request = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        // Drain notifications while writing too: neither pipe may wait for the other to empty.
        let write = write_frame(&mut self.writer, &request, &self.stopped, deadline);
        let read = read_reply(&mut self.reader, id, &self.stopped, deadline);
        let ((), result) = tokio::try_join!(biased;write,read)?;
        Ok(result)
    }
}
impl NativeTools {
    pub fn spawn(cli: &Path, port: u16, control_token: &str, permit: &str, stop: Arc<AtomicBool>) -> Result<Self, String> {
        if !cli.is_file() || port == 0 || [control_token, permit].iter().any(|token| token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit())) {
            return Err("API needs the matching local helper and a valid private FilmCraft bridge".into());
        }
        let mut command = Command::new(cli);
        command
            .args(["mcp", "--controller-console", "--bridge", &format!("127.0.0.1:{port}")])
            .env("FILMCRAFT_CONTROL_TOKEN", control_token)
            .env("FILMCRAFT_PROVIDER_PERMIT", permit)
            .env_remove("FILMCRAFT_EXTERNAL_PROVIDER")
            .env_remove("OPENAI_API_KEY")
            .env_remove("FILMCRAFT_OPENAI_API_KEY");
        let mut client = Self::spawn_command(command, stop)?;
        let init = client.rpc(
            "initialize",
            json!({"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"filmcraft_api","version":env!("CARGO_PKG_VERSION")}}),
            Duration::from_secs(30),
        )?;
        if !["2025-03-26", "2025-06-18", "2025-11-25"].contains(&init.get("protocolVersion").and_then(Value::as_str).unwrap_or("")) {
            return Err("Native MCP protocol version is unqualified; build the matching helper".into());
        }
        client.notify(json!({"jsonrpc":"2.0","method":"notifications/initialized"}), Duration::from_secs(30))?;
        let catalog = client.rpc("tools/list", json!({}), Duration::from_secs(30))?;
        client.pipe.definitions = api_definitions(&catalog)?;
        Ok(client)
    }
    fn spawn_command(mut command: Command, stop: Arc<AtomicBool>) -> Result<Self, String> {
        let runtime = Builder::new_current_thread().enable_all().build().map_err(|_| "Cannot start native MCP I/O runtime")?;
        let mut child = {
            let _entered = runtime.enter();
            command
                .kill_on_drop(true)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_| "Cannot start the local FilmCraft MCP helper")?
        };
        let writer = child.stdin.take().ok_or("Native MCP helper has no input pipe")?;
        let reader = BufReader::new(child.stdout.take().ok_or("Native MCP helper has no output pipe")?);
        Ok(Self { pipe: Pipe { writer, reader, stopped: stop, next: 1, definitions: vec![], broken: false }, runtime: Some(runtime), child: Some(child) })
    }
}
impl<W: AsyncWrite + Unpin, R: AsyncBufRead + Unpin> Client<W, R> {
    fn terminate(&mut self) -> Result<(), String> {
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        let signal_failed = child.start_kill().is_err();
        let runtime = self.runtime.as_ref().ok_or("Native MCP cleanup runtime is closed")?;
        // A successful wait confirms termination even if the child exited just before kill.
        // kill_on_drop is retained if waiting cannot confirm cleanup within this bound.
        match runtime.block_on(async { timeout(Duration::from_millis(500), child.wait()).await }) {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(_)) => Err("Native MCP cleanup could not reap the owned helper".into()),
            Err(_) if signal_failed => Err("Native MCP cleanup could not signal or confirm the owned helper stopped".into()),
            Err(_) => Err("Native MCP cleanup did not confirm termination within 500 ms".into()),
        }
    }
    fn finish<T>(&mut self, result: Result<T, String>) -> Result<T, String> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                self.pipe.broken = true;
                match self.terminate() {
                    Ok(()) => Err(error),
                    Err(cleanup) => Err(format!("{error}; {cleanup}")),
                }
            }
        }
    }
    fn notify(&mut self, value: Value, duration: Duration) -> Result<(), String> {
        let runtime = self.runtime.as_ref().ok_or("Native MCP runtime is closed")?;
        let result = runtime.block_on(async {
            let deadline = Instant::now() + duration;
            self.pipe.send(&value, deadline).await
        });
        self.finish(result)
    }
    fn rpc(&mut self, method: &str, params: Value, duration: Duration) -> Result<Value, String> {
        let runtime = self.runtime.as_ref().ok_or("Native MCP runtime is closed")?;
        // Deadline is created before encoding/writing, not after a potentially blocked flush.
        let result = runtime.block_on(async {
            let deadline = Instant::now() + duration;
            self.pipe.rpc(method, params, deadline).await
        });
        self.finish(result)
    }
}
impl<W: AsyncWrite + Unpin, R: AsyncBufRead + Unpin> Tools for Client<W, R> {
    fn definitions(&self) -> Result<Vec<Tool>, String> {
        Ok(self.pipe.definitions.clone())
    }
    fn is_current(&self) -> bool {
        !self.pipe.broken && !self.pipe.stopped.load(Ordering::SeqCst)
    }
    fn call_once(&mut self, call: &Call) -> Result<Value, String> {
        if !self.pipe.definitions.iter().any(|tool| tool.name == call.name) {
            return Err("Unadvertised native tool was rejected".into());
        }
        let result = self.rpc("tools/call", json!({"name":call.name,"arguments":call.arguments}), Duration::from_secs(600))?;
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            return Err("Native tool reported a failure or uncertain outcome; no automatic replay was attempted".into());
        }
        Ok(result)
    }
}
impl<W: AsyncWrite + Unpin, R: AsyncBufRead + Unpin> Drop for Client<W, R> {
    fn drop(&mut self) {
        if let Err(error) = self.terminate() {
            log::warn!(target:"filmcraft::console","{error}");
        }
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(Duration::from_millis(250));
        }
    }
}
#[cfg(test)]
type FakeClient = Client<Vec<u8>, BufReader<std::io::Cursor<Vec<u8>>>>;
#[cfg(test)]
pub(super) fn test_client(values: Vec<Value>) -> FakeClient {
    tests::fake(values)
}
#[cfg(all(test, unix))]
pub(super) fn owned_helper_fixture(stopped: Arc<AtomicBool>, responsive: bool) -> NativeTools {
    let mut command = Command::new("/usr/bin/python3");
    let code = if responsive {
        "import sys,json,signal; signal.alarm(5)\nfor line in sys.stdin:\n r=json.loads(line); print(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':{'ok':True}}),flush=True)"
    } else {
        "import time,signal; signal.alarm(5); time.sleep(5)"
    };
    command.args(["-I", "-S", "-c"]).arg(code).env_clear();
    let mut client = NativeTools::spawn_command(command, stopped).unwrap();
    client.pipe.definitions.push(Tool { name: "command_run".into(), description: "Owned fake helper only".into(), parameters: json!({"type":"object"}) });
    client
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires FILMCRAFT_TEST_NATIVE_HELPER pointing to the matching packaged CLI"]
    fn packaged_api_catalog_excludes_desktop_rebind() {
        let path = std::env::var_os("FILMCRAFT_TEST_NATIVE_HELPER").expect("matching packaged CLI");
        let token = "d".repeat(64);
        let native = NativeTools::spawn(Path::new(&path), 9, &token, &token, Arc::new(AtomicBool::new(false))).unwrap();
        let definitions = native.definitions().unwrap();
        let allowed = filmcraft_ui_egui::console::capabilities::CONSOLE_TOOLS;
        assert_eq!(definitions.len(), allowed.len());
        assert!(definitions.iter().all(|tool| allowed.contains(&tool.name.as_str())));
        assert!(!definitions.iter().any(|tool| tool.name == "connection_rebind"));
    }
    fn catalog_tool(name: &str) -> Value {
        json!({"name":name,"description":"Native fixture","inputSchema":{"type":"object"}})
    }
    #[test]
    fn desktop_rebind_is_neither_advertised_nor_dispatchable_by_api() {
        let catalog = json!({"tools":[catalog_tool("command_run"),catalog_tool("connection_rebind")]});
        let mut client = fake(vec![]);
        client.pipe.definitions = api_definitions(&catalog).unwrap();
        assert_eq!(client.definitions().unwrap().iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>(), vec!["command_run"]);
        let error = client.call_once(&Call { id: "desktop-only".into(), name: "connection_rebind".into(), arguments: json!({"confirm":true}) }).unwrap_err();
        assert_eq!(error, "Unadvertised native tool was rejected");
        assert!(client.pipe.writer.is_empty());
    }
    #[test]
    fn api_catalog_keeps_the_complete_console_contract_and_rejects_other_tools() {
        let tools: Vec<_> = filmcraft_ui_egui::console::capabilities::CONSOLE_TOOLS.iter().map(|name| catalog_tool(name)).collect();
        let definitions = api_definitions(&json!({"tools":tools})).unwrap();
        assert_eq!(definitions.len(), tools.len());
        for name in ["connection_rebind_extra", "ui_click", "shell", ""] {
            let mut hostile = tools.clone();
            hostile.push(catalog_tool(name));
            assert_eq!(api_definitions(&json!({"tools":hostile})).err().as_deref(), Some("Native helper exposed a tool outside the Console contract"));
        }
    }
    #[test]
    fn excluded_desktop_endpoint_still_obeys_catalog_validation() {
        let valid = catalog_tool("command_run");
        for tools in [
            vec![valid.clone(), catalog_tool("connection_rebind"), catalog_tool("connection_rebind")],
            vec![valid.clone(), json!({"name":"connection_rebind","inputSchema":null})],
            vec![valid.clone(), json!({"name":"connection_rebind","description":"x".repeat(8193),"inputSchema":{}})],
            vec![catalog_tool("connection_rebind")],
            vec![valid; 33],
        ] {
            assert!(api_definitions(&json!({"tools":tools})).is_err());
        }
        assert!(api_definitions(&json!({"tools":[catalog_tool("command_run")],"nextCursor":"unexpected"})).is_err());
    }
    pub(super) fn fake(values: Vec<Value>) -> FakeClient {
        let mut bytes = Vec::new();
        for value in values {
            serde_json::to_writer(&mut bytes, &value).unwrap();
            bytes.push(b'\n');
        }
        Client {
            pipe: Pipe {
                writer: vec![],
                reader: BufReader::new(std::io::Cursor::new(bytes)),
                stopped: Arc::new(AtomicBool::new(false)),
                next: 1,
                definitions: vec![Tool { name: "command_run".into(), description: "Native".into(), parameters: json!({"type":"object"}) }],
                broken: false,
            },
            runtime: Some(Builder::new_current_thread().enable_all().build().unwrap()),
            child: None,
        }
    }
    #[test]
    fn actual_rpc_framing_accepts_one_matching_reply() {
        let mut client = fake(vec![
            json!({"jsonrpc":"2.0","method":"notifications/progress","params":{}}),
            json!({"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"ok"}]}}),
        ]);
        let result = client.call_once(&Call { id: "call_a".into(), name: "command_run".into(), arguments: json!({"id":"markers.add"}) }).unwrap();
        assert!(result["content"].is_array());
        let line = String::from_utf8(client.pipe.writer.clone()).unwrap();
        assert_eq!(line.lines().count(), 1);
        assert!(line.contains("tools/call"));
    }
    #[test]
    fn mismatched_native_reply_is_never_replayed() {
        let mut client = fake(vec![json!({"jsonrpc":"2.0","id":9,"result":{}})]);
        assert!(client.call_once(&Call { id: "call_a".into(), name: "command_run".into(), arguments: json!({}) }).is_err());
        assert!(client.pipe.broken);
        assert_eq!(client.pipe.writer.iter().filter(|byte| **byte == b'\n').count(), 1);
    }
    #[test]
    fn truncated_and_mutated_native_frames_never_panic() {
        let runtime = Builder::new_current_thread().enable_all().build().unwrap();
        let valid = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n";
        for length in 0..valid.len() {
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runtime.block_on(frame(&mut BufReader::new(std::io::Cursor::new(&valid[..length]))))))
                    .is_ok()
            );
        }
        assert!(runtime.block_on(frame(&mut BufReader::new(std::io::Cursor::new(vec![b'x'; MAX_MESSAGE + 1])))).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn stalled_stdin_write_is_cancelled_and_cleanup_allows_a_fresh_helper() {
        let stopped = Arc::new(AtomicBool::new(false));
        let mut client = owned_helper_fixture(stopped.clone(), false);
        let start = std::time::Instant::now();
        let cancel = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            stopped.store(true, Ordering::SeqCst);
        });
        let error = client.rpc("tools/call", json!({"padding":"x".repeat(120*1024)}), Duration::from_secs(10)).unwrap_err();
        assert!(error.contains("write cancelled"), "{error}");
        assert!(client.pipe.broken);
        assert!(client.child.is_none());
        assert!(!error.contains("cleanup"), "{error}");
        drop(client);
        cancel.join().unwrap();
        assert!(start.elapsed() < Duration::from_secs(2));
        let mut fresh = owned_helper_fixture(Arc::new(AtomicBool::new(false)), true);
        assert_eq!(fresh.rpc("ping", json!({}), Duration::from_secs(2)).unwrap()["ok"], true);
    }
    #[cfg(unix)]
    #[test]
    fn write_deadline_stalled_reader_and_drop_cleanup_are_bounded() {
        for large in [true, false] {
            let mut client = owned_helper_fixture(Arc::new(AtomicBool::new(false)), false);
            let start = std::time::Instant::now();
            let params = if large { json!({"padding":"x".repeat(120*1024)}) } else { json!({}) };
            let error = client.rpc("tools/call", params, Duration::from_millis(150)).unwrap_err();
            assert!(error.contains(if large { "write timed out" } else { "read timed out" }), "{error}");
            assert!(client.pipe.broken);
            drop(client);
            assert!(start.elapsed() < Duration::from_secs(2));
        }
        let client = owned_helper_fixture(Arc::new(AtomicBool::new(false)), false);
        let start = std::time::Instant::now();
        drop(client);
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[cfg(unix)]
    #[test]
    fn stdout_backpressure_is_drained_while_a_large_request_is_written() {
        let mut command = Command::new("/usr/bin/python3");
        command.args(["-I","-S","-c"]).arg("import sys,json,signal; signal.alarm(5)\nprint(json.dumps({'jsonrpc':'2.0','method':'notifications/progress','params':{'padding':'z'*120000}}),flush=True)\nr=json.loads(sys.stdin.readline()); print(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':{'ok':True}}),flush=True)").env_clear();
        let mut client = NativeTools::spawn_command(command, Arc::new(AtomicBool::new(false))).unwrap();
        let start = std::time::Instant::now();
        assert_eq!(client.rpc("ping", json!({"padding":"x".repeat(120*1024)}), Duration::from_secs(2)).unwrap()["ok"], true);
        drop(client);
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
