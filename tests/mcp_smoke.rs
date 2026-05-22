//! MCP smoke tests — spawn the `eventkit --mcp` binary as a child process and
//! drive it over JSON-RPC on stdio, the same way a real MCP client would.
//!
//! These catch the class of bug that unit tests miss: runtime configuration
//! issues, tool registration regressions, JSON shape regressions in tool
//! responses.
//!
//! Only `auth_status` is exercised end-to-end here — it's the one tool that
//! never triggers a TCC dialog or mutates state, so it's safe to run in CI
//! and on developer machines with any authorization state.

#![cfg(target_os = "macos")]

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

/// Path to the binary cargo built for the integration test.
fn bin_path() -> std::path::PathBuf {
    // CARGO_BIN_EXE_<name> is set by Cargo for integration tests.
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_eventkit"))
}

struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl McpClient {
    fn spawn() -> Self {
        let mut child = Command::new(bin_path())
            .arg("--mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn eventkit --mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
            next_id: 0,
        }
    }

    fn send(&mut self, msg: &Value) {
        let line = serde_json::to_string(msg).unwrap();
        writeln!(self.stdin, "{line}").expect("write to MCP stdin");
        self.stdin.flush().ok();
    }

    /// Read JSON-RPC messages until one with the given id arrives, or timeout.
    fn recv_response(&mut self, id: i64, timeout: Duration) -> Value {
        let deadline = Instant::now() + timeout;
        loop {
            if Instant::now() >= deadline {
                panic!("timed out waiting for response id={id}");
            }
            let mut line = String::new();
            let n = self.stdout.read_line(&mut line).expect("read MCP stdout");
            if n == 0 {
                panic!("MCP server closed stdout before response id={id}");
            }
            let v: Value = serde_json::from_str(line.trim())
                .unwrap_or_else(|e| panic!("non-JSON line from MCP server: {line:?} ({e})"));
            if v.get("id").and_then(Value::as_i64) == Some(id) {
                return v;
            }
        }
    }

    fn initialize(&mut self) {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "eventkit-mcp-smoke", "version": "0"},
            },
        }));
        let resp = self.recv_response(id, Duration::from_secs(5));
        assert!(resp.get("result").is_some(), "initialize returned: {resp}");
        self.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    }

    fn list_tools(&mut self) -> Vec<Value> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": "tools/list"}));
        let resp = self.recv_response(id, Duration::from_secs(5));
        resp["result"]["tools"]
            .as_array()
            .expect("tools array")
            .clone()
    }

    fn call_tool(&mut self, name: &str, args: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": name, "arguments": args},
        }));
        self.recv_response(id, Duration::from_secs(5))
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        // Closing stdin signals EOF; the server should exit cleanly. Don't
        // wait forever if it doesn't.
        drop(self.child.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn mcp_initialize_and_list_tools_does_not_panic() {
    let mut c = McpClient::spawn();
    c.initialize();
    let tools = c.list_tools();
    assert!(
        tools.len() >= 27,
        "expected at least 27 tools, got {}: {:?}",
        tools.len(),
        tools.iter().map(|t| t["name"].as_str()).collect::<Vec<_>>()
    );
}

#[test]
fn mcp_auth_status_tool_is_registered() {
    let mut c = McpClient::spawn();
    c.initialize();
    let tools = c.list_tools();
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(
        names.contains(&"auth_status"),
        "auth_status not in tools/list. Got: {names:?}"
    );
}

#[test]
fn mcp_auth_status_returns_valid_structured_response() {
    // Calls auth_status, which is read-only and never fires a TCC dialog.
    // Asserts the response shape, not the values — values depend on the
    // developer's local TCC state.
    let mut c = McpClient::spawn();
    c.initialize();
    let resp = c.call_tool("auth_status", json!({}));
    let structured = &resp["result"]["structuredContent"];
    assert!(
        structured.is_object(),
        "auth_status missing structuredContent: {resp}"
    );
    let valid = [
        "FullAccess",
        "WriteOnly",
        "Denied",
        "NotDetermined",
        "Restricted",
    ];
    for field in ["reminders", "events"] {
        let v = structured[field]
            .as_str()
            .unwrap_or_else(|| panic!("auth_status.{field} missing or not a string: {structured}"));
        assert!(
            valid.contains(&v),
            "auth_status.{field} has unexpected value {v:?}; want one of {valid:?}"
        );
    }
    // remediation is Option<String>; either absent or a non-empty string.
    if let Some(r) = structured.get("remediation").and_then(Value::as_str) {
        assert!(!r.is_empty(), "remediation present but empty");
    }
}

#[test]
fn mcp_handles_multiple_sequential_requests_without_panic() {
    let mut c = McpClient::spawn();
    c.initialize();
    let _ = c.list_tools();
    let _ = c.call_tool("auth_status", json!({}));
    let _ = c.list_tools();
    let _ = c.call_tool("auth_status", json!({}));
}
