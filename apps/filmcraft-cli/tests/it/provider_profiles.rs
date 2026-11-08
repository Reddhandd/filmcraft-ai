//! Actual CLI profile startup, without a GUI, account credentials or paid requests.
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.0.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_filmcraft-cli")).args(args).env_remove("OPENAI_API_KEY").output().unwrap()
}
#[test]
fn private_profile_flags_are_console_only_and_require_a_path() {
    for arguments in [
        vec!["mcp", "--provider-profile", "missing.json"],
        vec!["mcp", "--controller-console", "--provider-profile"],
        vec!["mcp", "--controller-console", "--provider-profile="],
        vec!["mcp", "--controller-console", "--provider-profile", "missing.json", "--bridge", "127.0.0.1:9876"],
        vec!["mcp", "--controller-console", "--provider-profile", "missing.json", "--demo"],
        vec!["mcp", "--controller-console", "--provider-profile", "missing.json", "--data-dir", "other"],
        vec!["mcp", "--controller-edit", "--provider-profile", "missing.json"],
    ] {
        let output = cli(&arguments);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));
    }
}
#[test]
fn actual_cli_profile_inventory_is_passive_then_native_call_activates() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut profile = tempfile::NamedTempFile::new().unwrap();
    profile
        .write_all(
            &serde_json::to_vec(&json!({"version":1,"provider":"desktopChatGpt","port":port,"controlToken":"a".repeat(64),"permit":"b".repeat(64)})).unwrap(),
        )
        .unwrap();
    let (events, observed) = mpsc::channel();
    let app = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let (stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
                    other => panic!("Owned fixture accept failed: {other:?}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            stream.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            Read::by_ref(&mut reader).take(16 * 1024).read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["token"], "a".repeat(64));
            assert_eq!(request["params"]["permit"], "b".repeat(64));
            let (event, result) = if request["method"] == "provider.external" {
                (request["params"]["event"].as_str().unwrap().to_owned(), json!({"accepted":true}))
            } else {
                assert_eq!(request["method"], "provider.execute");
                assert_eq!(request["params"]["params"]["command"], "project.inspectPage");
                ("native-inspect".to_owned(), json!({"name":"Private CLI fixture","revision":1,"documentEpoch":1,"rows":[]}))
            };
            let reply = json!({"id":request["id"],"ok":true,"result":result});
            reader.get_mut().write_all(format!("{reply}\n").as_bytes()).unwrap();
            events.send(event.clone()).unwrap();
            if event == "detach" {
                break;
            }
        }
    });
    let mut child = OwnedChild(
        Command::new(env!("CARGO_BIN_EXE_filmcraft-cli"))
            .args(["mcp", "--controller-console", "--provider-profile"])
            .arg(profile.path())
            .env("FILMCRAFT_CONTROL_TOKEN", "invalid-inherited-token")
            .env("FILMCRAFT_PROVIDER_PERMIT", "invalid-inherited-permit")
            .env("FILMCRAFT_EXTERNAL_PROVIDER", "invalid-inherited-mode")
            .env_remove("OPENAI_API_KEY")
            .env_remove("FILMCRAFT_OPENAI_API_KEY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let stdout = child.0.stdout.take().unwrap();
    let (lines, received) = mpsc::channel();
    let output = std::thread::spawn(move || {
        let reader = BufReader::new(stdout.take(1024 * 1024));
        for line in reader.lines() {
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    let mut input = child.0.stdin.take().unwrap();
    let init = json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"owned-cli-fixture","version":"1"}}});
    writeln!(input, "{init}").unwrap();
    input.flush().unwrap();
    let reply: Value = serde_json::from_str(&received.recv_timeout(Duration::from_secs(5)).unwrap().unwrap()).unwrap();
    assert!(reply.get("result").is_some(), "{reply}");
    assert_eq!(observed.recv_timeout(Duration::from_secs(5)).unwrap(), "authenticate");
    writeln!(input, "{}", json!({"jsonrpc":"2.0","method":"notifications/initialized"})).unwrap();
    input.flush().unwrap();
    writeln!(input, "{}", json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})).unwrap();
    input.flush().unwrap();
    let list: Value = serde_json::from_str(&received.recv_timeout(Duration::from_secs(5)).unwrap().unwrap()).unwrap();
    assert!(list["result"]["tools"].as_array().unwrap().iter().any(|tool| tool["name"] == "command_run"));
    assert!(observed.recv_timeout(Duration::from_millis(2250)).is_err(), "Inventory must not consume active ownership");
    writeln!(input, "{}", json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"project_inspect","arguments":{}}})).unwrap();
    input.flush().unwrap();
    let inspected: Value = serde_json::from_str(&received.recv_timeout(Duration::from_secs(5)).unwrap().unwrap()).unwrap();
    assert!(inspected.get("result").is_some(), "{inspected}");
    assert_eq!(observed.recv_timeout(Duration::from_secs(5)).unwrap(), "attach");
    assert_eq!(observed.recv_timeout(Duration::from_secs(5)).unwrap(), "native-inspect");
    drop(input);
    loop {
        let event = observed.recv_timeout(Duration::from_secs(5)).unwrap();
        if event == "detach" {
            break;
        }
        assert_eq!(event, "heartbeat");
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "Owned CLI did not stop after EOF");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success());
    output.join().unwrap();
    app.join().unwrap();
}

#[cfg(unix)]
#[test]
fn non_unicode_profile_argument_is_rejected_without_lossy_file_lookup() {
    use std::os::unix::ffi::OsStringExt;
    let output = Command::new(env!("CARGO_BIN_EXE_filmcraft-cli"))
        .args(["mcp", "--controller-console", "--provider-profile"])
        .arg(std::ffi::OsString::from_vec(b"profile-\xff".to_vec()))
        .env_remove("OPENAI_API_KEY")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Unicode"));
}
