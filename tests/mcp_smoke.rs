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
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

/// Path to the binary cargo built for the integration test.
fn bin_path() -> std::path::PathBuf {
    // CARGO_BIN_EXE_<name> is set by Cargo for integration tests.
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_eventkit"))
}

struct McpClient {
    child: Child,
    stdin: ChildStdin,
    /// Lines pumped off the child's stdout by a reader thread. See `spawn`.
    lines: Receiver<String>,
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
        let stdout = child.stdout.take().unwrap();

        // Read on a separate thread and hand lines over a channel.
        //
        // `BufRead::read_line` cannot honour a deadline: it blocks until a line
        // arrives, so a timeout checked around it only fires if the server is
        // still TALKING. When the server goes SILENT — a wedged worker thread,
        // a handler that never answers — the read never returns and the
        // deadline is never reached. That turned a 10s assertion into an
        // opaque 60s harness kill with no message. `recv_timeout` on this side
        // makes every timeout in this file real.
        let (tx, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if tx.send(line).is_err() {
                    return; // client dropped
                }
            }
        });

        Self {
            child,
            stdin,
            lines,
            next_id: 0,
        }
    }

    /// Next line from the server, or `None` once `deadline` passes.
    fn next_line(&self, deadline: Instant) -> Option<String> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        self.lines.recv_timeout(remaining).ok()
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
            let Some(line) = self.next_line(deadline) else {
                panic!(
                    "timed out after {timeout:?} waiting for response id={id} — \
                     the server went silent or closed stdout"
                );
            };
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

    /// Send an arbitrary JSON-RPC request and return the response.
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        self.recv_response(id, Duration::from_secs(10))
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
        tools.len() >= 32,
        "expected at least 32 tools, got {}: {:?}",
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
fn mcp_event_tools_are_registered() {
    // Event-side parity check: every new event tool from the 1–8 plan must
    // show up in tools/list. Catches accidental tool deletion or rename.
    let mut c = McpClient::spawn();
    c.initialize();
    let tools = c.list_tools();
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    for expected in [
        "list_events",
        "create_event",
        "update_event",
        "delete_event",
        "get_event",
        "list_calendars",
        "create_event_calendar",
        "update_event_calendar",
        "delete_event_calendar",
        "set_event_availability",
        "get_default_event_calendar",
    ] {
        assert!(
            names.contains(&expected),
            "missing tool {expected:?} in tools/list. Got: {names:?}"
        );
    }
}

#[test]
fn mcp_update_event_schema_includes_new_fields() {
    // Schema drift catcher: if someone renames a field on UpdateEventRequest
    // the input schema changes and this fires.
    let mut c = McpClient::spawn();
    c.initialize();
    let tools = c.list_tools();
    let update_event = tools
        .iter()
        .find(|t| t["name"].as_str() == Some("update_event"))
        .expect("update_event tool missing");
    let props = &update_event["inputSchema"]["properties"];
    assert!(
        props.is_object(),
        "update_event inputSchema.properties missing: {update_event}"
    );
    for field in [
        "title",
        "notes",
        "location",
        "start",
        "end",
        "all_day",
        "calendar_name",
        "URL",
        "availability",
        "structured_location",
        "span",
        "alarms",
        "recurrence",
    ] {
        assert!(
            props.get(field).is_some(),
            "update_event inputSchema missing field {field:?}; got: {props}"
        );
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

/// EVERY tool must carry annotations, and the safety hints must be coherent.
///
/// The host groups a backend's tools by annotation
/// (`BackendNutritionCard.tsx::groupToolsByAnnotation`): `readOnlyHint` →
/// "Read-Only", `destructiveHint` → "Destructive", neither → "Safe Write",
/// and **no annotations at all → "Other"**. Before this, all 32 EventKit tools
/// had none, so the entire server collapsed into one undifferentiated "Other"
/// bucket and the user got no read-vs-destroy signal anywhere in the UI.
///
/// This fails if a new tool ships unannotated, which would silently put it
/// back in "Other".
#[test]
fn mcp_every_tool_is_annotated_and_coherent() {
    let mut c = McpClient::spawn();
    c.initialize();
    let tools = c.list_tools();
    assert!(!tools.is_empty(), "tools/list must not be empty");

    let mut unannotated = Vec::new();
    let mut incoherent = Vec::new();
    let (mut read_only, mut destructive, mut safe_write) = (0, 0, 0);

    for t in &tools {
        let name = t["name"].as_str().unwrap_or("<unnamed>").to_string();
        let Some(ann) = t.get("annotations").filter(|a| a.is_object()) else {
            unannotated.push(name);
            continue;
        };
        let ro = ann["readOnlyHint"].as_bool().unwrap_or(false);
        let de = ann["destructiveHint"].as_bool().unwrap_or(false);

        // A read-only tool cannot also be destructive — that is a contradiction,
        // and the grouping would silently prefer "Read-Only" and hide the risk.
        if ro && de {
            incoherent.push(name.clone());
        }
        // Every tool should carry a human title for the UI.
        if ann["title"].as_str().unwrap_or("").is_empty() {
            incoherent.push(format!("{name} (no title)"));
        }

        if ro {
            read_only += 1;
        } else if de {
            destructive += 1;
        } else {
            safe_write += 1;
        }
    }

    assert!(
        unannotated.is_empty(),
        "these tools have NO annotations and would fall into the host's \"Other\" \
         bucket: {unannotated:?}"
    );
    assert!(
        incoherent.is_empty(),
        "incoherent or untitled annotations: {incoherent:?}"
    );

    // Sanity: this server genuinely spans all three groups. If a whole class
    // vanished, the classification was probably flattened by accident.
    assert!(read_only > 0, "expected some read-only tools");
    assert!(destructive > 0, "expected some destructive tools (deletes)");
    assert!(
        safe_write > 0,
        "expected some safe-write tools (creates/updates)"
    );
}

/// Tasks are the SEP-2663 extension (`capabilities.extensions`), never the
/// SEP-1686 slot retired in 0.7.0, and no tool declares `execution`: the
/// server decides which calls become tasks, the client does not ask per tool.
#[test]
fn mcp_advertises_tasks_only_as_the_extension() {
    let mut c = McpClient::spawn();
    c.next_id += 1;
    let id = c.next_id;
    c.send(&json!({
        "jsonrpc": "2.0", "id": id, "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "eventkit-mcp-smoke", "version": "0"},
        },
    }));
    let resp = c.recv_response(id, Duration::from_secs(5));
    assert!(
        resp["result"]["capabilities"].is_object(),
        "initialize must answer with capabilities: {resp}"
    );
    assert!(
        resp["result"]["capabilities"].get("tasks").is_none(),
        "the v1 `tasks` slot must not be advertised: {resp}"
    );
    assert!(
        resp["result"]["capabilities"]["extensions"]
            .as_object()
            .is_some_and(|extensions| extensions.contains_key("io.modelcontextprotocol/tasks")),
        "the tasks extension is offered: {resp}"
    );
    let tools = c.list_tools();
    let declared: Vec<&str> = tools
        .iter()
        .filter(|t| t.get("execution").is_some())
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(
        declared.is_empty(),
        "no tool may declare `execution` (task support): {declared:?}"
    );
}

/// The SEP-1686 `task` parameter asks for nothing any more: SEP-2663 leaves
/// the decision to the server, only for a client that declared the extension
/// (this one did not), and `auth_status` is not a task tool anyway. So the
/// call RUNS as a plain call. `auth_status` needs no EventKit access, so the
/// run is a plain RESULT.
#[test]
fn mcp_runs_a_task_augmented_call_as_a_plain_call() {
    let mut c = McpClient::spawn();
    c.initialize();
    let resp = c.request(
        "tools/call",
        json!({
            "name": "auth_status",
            "arguments": {},
            "task": {}
        }),
    );
    assert!(
        resp.get("error").is_none() && resp["result"]["content"].is_array(),
        "a task-augmented call must run as a plain call: {resp}"
    );
    assert!(
        resp["result"].get("task").is_none(),
        "the answer is a CallToolResult, not a task handle: {resp}"
    );
}

/// The `_meta` a 2026-07-28 client puts on every request: its version,
/// identity and capabilities, since that revision has no handshake.
fn stateless_meta(version: &str, capabilities: &Value) -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": version,
        "io.modelcontextprotocol/clientInfo": {"name": "eventkit-mcp-smoke", "version": "0"},
        "io.modelcontextprotocol/clientCapabilities": capabilities,
    })
}

/// The probe a 2026-07-28 client opens with: `server/discover` carrying its
/// version, identity and capabilities in `_meta`.
fn discover_probe(version: &str) -> Value {
    json!({ "_meta": stateless_meta(version, &json!({})) })
}

/// An `initialize` asking for 2026-07-28 is answered in 2025-11-25: that
/// revision has no handshake, so the newest one with one is the answer.
#[test]
fn mcp_negotiates_a_2026_07_28_client_down_to_2025_11_25() {
    let mut c = McpClient::spawn();
    let resp = c.request(
        "initialize",
        json!({
            "protocolVersion": "2026-07-28",
            "capabilities": {},
            "clientInfo": {"name": "eventkit-mcp-smoke", "version": "0"},
        }),
    );
    assert_eq!(resp["result"]["protocolVersion"], "2025-11-25", "{resp}");
}

/// A 2026-07-28 probe opens that revision's lifecycle: `server/discover`
/// answers with every revision this server implements (the list a `-32022`
/// carries too), the tasks extension and this server's identity, and a
/// request carrying its own
/// `_meta`, with no `initialize` anywhere, is served in that revision's shape.
#[test]
fn mcp_server_discover_at_2026_07_28_opens_the_stateless_lifecycle() {
    let mut c = McpClient::spawn();
    let resp = c.request("server/discover", discover_probe("2026-07-28"));
    let result = &resp["result"];
    assert_eq!(
        result["supportedVersions"],
        json!([
            "2024-11-05",
            "2025-03-26",
            "2025-06-18",
            "2025-11-25",
            "2026-07-28"
        ]),
        "{resp}"
    );
    assert!(
        result["capabilities"]["extensions"]
            .as_object()
            .is_some_and(|extensions| extensions.contains_key("io.modelcontextprotocol/tasks")),
        "{resp}"
    );

    let resp = c.request(
        "tools/list",
        json!({ "_meta": stateless_meta("2026-07-28", &json!({})) }),
    );
    assert!(
        resp["result"]["tools"]
            .as_array()
            .is_some_and(|tools| !tools.is_empty()),
        "{resp}"
    );
    assert_eq!(resp["result"]["resultType"], "complete", "{resp}");
}

/// A probe at a 2025 version is told the method does not exist, and the
/// handshake still works on the same pipe: that is the fallback.
#[test]
fn mcp_server_discover_at_a_2025_version_is_method_not_found() {
    let mut c = McpClient::spawn();
    let resp = c.request("server/discover", discover_probe("2025-11-25"));
    assert_eq!(resp["error"]["code"], -32601, "{resp}");

    c.initialize();
    assert!(
        !c.list_tools().is_empty(),
        "the fallback session serves tools"
    );
}

/// A revision newer than 2026-07-28 is refused `-32022` with every version
/// this server speaks.
#[test]
fn mcp_refuses_an_unknown_revision_with_the_supported_list() {
    let mut c = McpClient::spawn();
    let resp = c.request("server/discover", discover_probe("2099-01-01"));
    assert_eq!(resp["error"]["code"], -32022, "{resp}");
    assert_eq!(
        resp["error"]["data"]["supported"],
        json!([
            "2024-11-05",
            "2025-03-26",
            "2025-06-18",
            "2025-11-25",
            "2026-07-28"
        ]),
    );
}

/// `tasks/*` belongs to clients that declared the extension: a session that
/// did not is refused `-32021`.
#[test]
fn mcp_refuses_tasks_methods_without_the_extension() {
    let mut c = McpClient::spawn();
    c.initialize();
    let resp = c.request("tasks/get", json!({ "taskId": "nope" }));
    assert_eq!(resp["error"]["code"], -32021, "{resp}");
}

/// The handshake names the server, not the SDK: without its own server info
/// it answered `rmcp` and rmcp's version.
#[test]
fn mcp_names_itself_calendar() {
    let mut c = McpClient::spawn();
    let resp = c.request(
        "initialize",
        json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "eventkit-mcp-smoke", "version": "0"},
        }),
    );
    let info = &resp["result"]["serverInfo"];
    assert_eq!(info["name"], "calendar", "{resp}");
    assert_eq!(info["title"], "Calendar & Reminders", "{resp}");
    assert_eq!(info["version"], env!("CARGO_PKG_VERSION"), "{resp}");
    let resources = &resp["result"]["capabilities"]["resources"];
    assert_eq!(
        resources["subscribe"], true,
        "resources are subscribable: {resp}"
    );
    assert_eq!(
        resources["listChanged"], true,
        "list changes are announced: {resp}"
    );
}

/// Calendars and reminders are resources, listed WITHOUT asking for access: a
/// host lists them when it connects, which must never raise the consent
/// dialog. The fixed four are there whatever this process has been granted;
/// one per calendar and per list follow only where full access already is.
#[test]
fn mcp_lists_calendars_and_reminders_as_resources() {
    let mut c = McpClient::spawn();
    c.initialize();
    let resp = c.request("resources/list", json!({}));
    let uris: Vec<&str> = resp["result"]["resources"]
        .as_array()
        .unwrap_or_else(|| panic!("a resources array: {resp}"))
        .iter()
        .filter_map(|r| r["uri"].as_str())
        .collect();
    for fixed in [
        "calendar://calendars",
        "calendar://events",
        "reminders://lists",
        "reminders://reminders",
    ] {
        assert!(uris.contains(&fixed), "{fixed} is listed: {uris:?}");
    }

    let resp = c.request("resources/templates/list", json!({}));
    let templates: Vec<&str> = resp["result"]["resourceTemplates"]
        .as_array()
        .unwrap_or_else(|| panic!("a resourceTemplates array: {resp}"))
        .iter()
        .filter_map(|t| t["uriTemplate"].as_str())
        .collect();
    assert_eq!(
        templates,
        [
            "calendar://events{?from,to}",
            "calendar://calendars/{calendarId}/events{?from,to}",
            "calendar://events/{eventId}",
            "reminders://lists/{listId}{?completed}",
            "reminders://reminders/{reminderId}",
        ]
    );
}

/// A URI naming nothing here is `-32002`, a bad parameter `-32602`: both are
/// answered before anything touches the store, so neither needs access.
#[test]
fn mcp_refuses_unknown_and_malformed_resource_uris() {
    let mut c = McpClient::spawn();
    c.initialize();
    let resp = c.request("resources/read", json!({"uri": "calendar://nope"}));
    assert_eq!(resp["error"]["code"], -32002, "{resp}");
    let resp = c.request(
        "resources/read",
        json!({"uri": "calendar://events?from=tomorrow"}),
    );
    assert_eq!(resp["error"]["code"], -32602, "{resp}");
}

/// A 2025-11-25 client subscribes URI by URI: to a resource here, and not to
/// one this server does not have. Subscribing never asks for access, so this
/// runs the same with or without it.
#[test]
fn mcp_subscribes_to_resources_here_and_nowhere_else() {
    let mut c = McpClient::spawn();
    c.initialize();
    let resp = c.request(
        "resources/subscribe",
        json!({"uri": "calendar://calendars"}),
    );
    assert!(resp["result"].is_object(), "{resp}");
    let resp = c.request("resources/subscribe", json!({"uri": "bridge://calendars"}));
    assert_eq!(resp["error"]["code"], -32002, "{resp}");
    let resp = c.request(
        "resources/unsubscribe",
        json!({"uri": "calendar://calendars"}),
    );
    assert!(resp["result"].is_object(), "{resp}");
}

/// A 2026-07-28 client listens instead: the acknowledgment carries what this
/// server serves of the filter asked for, which is resource list changes and
/// its own URIs. Its tools and prompts never change, and the email URI is
/// another server's.
#[test]
fn mcp_acknowledges_a_listen_with_what_it_serves() {
    let mut c = McpClient::spawn();
    c.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "subscriptions/listen",
        "params": {
            "notifications": {
                "toolsListChanged": true,
                "resourcesListChanged": true,
                "resourceSubscriptions": ["calendar://calendars", "email://work/INBOX"],
            },
            "_meta": stateless_meta("2026-07-28", &json!({})),
        },
    }));
    let deadline = Instant::now() + Duration::from_secs(5);
    let ack = loop {
        let line = c
            .next_line(deadline)
            .expect("an acknowledgment before the deadline");
        let message: Value = serde_json::from_str(line.trim()).expect("JSON");
        assert!(message.get("error").is_none(), "refused: {message}");
        if message["method"] == "notifications/subscriptions/acknowledged" {
            break message;
        }
    };
    assert_eq!(
        ack["params"]["notifications"],
        json!({
            "resourcesListChanged": true,
            "resourceSubscriptions": ["calendar://calendars"],
        }),
        "{ack}"
    );
}
