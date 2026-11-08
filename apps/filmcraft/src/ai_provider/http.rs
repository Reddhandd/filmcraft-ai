//! Fixed-origin, bounded Responses HTTP requests. API credentials never enter protocol JSON.
use std::{
    io::Read,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
const MAX_REQUEST: usize = 16 * 1024 * 1024;
const MAX_RESPONSE: usize = 2 * 1024 * 1024;
pub struct ApiHttp {
    endpoint: String,
    key: String,
}
impl std::fmt::Debug for ApiHttp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiHttp").field("endpoint", &self.endpoint).field("key", &"[redacted]").finish()
    }
}
impl ApiHttp {
    pub fn new(key: String) -> Result<Self, String> {
        if key.is_empty() || key.len() > 512 || !key.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err("Supply an API key in the session/environment; whitespace and oversized credentials are rejected".into());
        }
        Ok(Self { endpoint: "https://api.openai.com/v1/responses".into(), key })
    }
    pub fn post(&self, body: &[u8], cancel: &AtomicBool) -> Result<Vec<u8>, String> {
        if cancel.load(Ordering::SeqCst) {
            return Err("API request cancelled; nothing was submitted".into());
        }
        if body.len() > MAX_REQUEST {
            return Err("API request exceeds 16 MiB; shorten the conversation or remove references".into());
        }
        // Each POST has a fresh pool. Never reuse an ambiguous stale connection or replay edits.
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .max_redirects(0)
            .http_status_as_error(false)
            .max_idle_connections(0)
            .build()
            .into();
        let mut request = agent
            .post(&self.endpoint)
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", self.key))
            .header("Connection", "close");
        if let Some(value) = request.headers_mut().and_then(|headers| headers.get_mut("authorization")) {
            value.set_sensitive(true);
        }
        let mut response = request.send(body).map_err(|_| "API transport failed or timed out; no automatic replay was attempted")?;
        if cancel.load(Ordering::SeqCst) {
            return Err("API request cancelled; late response was discarded".into());
        }
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(super::protocol::http_error(status).into());
        }
        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take((MAX_RESPONSE + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| "API response read failed; no automatic replay was attempted")?;
        if bytes.len() > MAX_RESPONSE {
            return Err("API response exceeds 2 MiB; no tool calls were executed".into());
        }
        if cancel.load(Ordering::SeqCst) {
            return Err("API request cancelled; late response was discarded".into());
        }
        Ok(bytes)
    }
    #[cfg(test)]
    fn local(endpoint: String) -> Self {
        Self { endpoint, key: "fake-test-key-never-a-user-credential".into() }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        sync::{Arc, atomic::AtomicUsize},
    };
    fn server(status: u16, response: &str) -> (String, Arc<AtomicUsize>, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1/responses", listener.local_addr().unwrap());
        let body = response.to_owned();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            count.fetch_add(1, Ordering::SeqCst);
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0usize;
            let mut header_bytes = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                header_bytes += line.len();
                assert!(header_bytes <= 8192);
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
            }
            assert!(length <= MAX_REQUEST);
            let mut request = vec![0; length];
            reader.read_exact(&mut request).unwrap();
            let header =
                format!("HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nLocation: http://127.0.0.1:1/forbidden\r\nConnection: close\r\n\r\n", body.len());
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(body.as_bytes());
        });
        (endpoint, calls, worker)
    }
    #[test]
    fn fake_http_success_and_authentication_errors_are_bounded_and_redacted() {
        let cancel = AtomicBool::new(false);
        let (endpoint, calls, worker) = server(200, "{\"status\":\"completed\",\"output\":[]}");
        let client = ApiHttp::local(endpoint);
        let reply = client.post(b"{}", &cancel).unwrap();
        assert!(super::super::protocol::parse_reply(&reply, &[]).is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        worker.join().unwrap();
        assert!(!format!("{client:?}").contains("fake-test-key"));
        for status in [401, 403, 429, 503] {
            let (endpoint, calls, worker) = server(status, "secret-error-body-do-not-log");
            let error = ApiHttp::local(endpoint).post(b"{}", &cancel).unwrap_err();
            assert!(!error.contains("secret-error-body"));
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            worker.join().unwrap();
        }
    }
    #[test]
    fn redirects_are_not_followed_and_oversized_reply_is_rejected() {
        let cancel = AtomicBool::new(false);
        let (endpoint, calls, worker) = server(302, "redirect");
        assert!(ApiHttp::local(endpoint).post(b"{}", &cancel).is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        worker.join().unwrap();
        let (endpoint, _, worker) = server(200, &"x".repeat(MAX_RESPONSE + 1));
        assert!(ApiHttp::local(endpoint).post(b"{}", &cancel).unwrap_err().contains("2 MiB"));
        worker.join().unwrap();
    }
    #[test]
    fn cancelled_or_oversized_work_never_connects() {
        let client = ApiHttp::local("http://127.0.0.1:1/unused".into());
        assert!(client.post(b"{}", &AtomicBool::new(true)).unwrap_err().contains("cancelled"));
        assert!(client.post(&vec![0; MAX_REQUEST + 1], &AtomicBool::new(false)).unwrap_err().contains("16 MiB"));
        assert!(ApiHttp::new("bad\ncredential".into()).is_err());
    }
    #[test]
    fn loopback_http_turn_uses_native_mcp_framing_and_returns_the_final_answer() {
        use super::super::{mcp, protocol, turn};
        use serde_json::json;
        struct Fixture(std::cell::RefCell<std::collections::VecDeque<ApiHttp>>);
        impl turn::Responses for Fixture {
            fn post(&self, body: &[u8], cancel: &AtomicBool) -> Result<Vec<u8>, String> {
                self.0.borrow_mut().pop_front().ok_or("Unexpected extra API POST")?.post(body, cancel)
            }
        }
        let first =
            json!({"status":"completed","output":[{"type":"function_call","call_id":"native_1","name":"command_run","arguments":"{\"id\":\"markers.add\"}"}]})
                .to_string();
        let last = json!({"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"Marker added."}]}]}).to_string();
        let (a, ac, aw) = server(200, &first);
        let (b, bc, bw) = server(200, &last);
        let http = Fixture(std::cell::RefCell::new([ApiHttp::local(a), ApiHttp::local(b)].into_iter().collect()));
        let mut native = mcp::test_client(vec![json!({"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"marker-created"}]}})]);
        let mut conversation = turn::Conversation::default();
        let text = conversation.run(&http, &mut native, "test-model", "Use native tools", "Add a marker", &[], &AtomicBool::new(false)).unwrap();
        assert_eq!(text, "Marker added.");
        assert_eq!(ac.load(Ordering::SeqCst), 1);
        assert_eq!(bc.load(Ordering::SeqCst), 1);
        aw.join().unwrap();
        bw.join().unwrap();
        assert!(protocol::user_input("done", &[]).is_ok());
    }
}
