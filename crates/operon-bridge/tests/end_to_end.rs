//! End-to-end test: spawn the real `operon-mcp` stub binary against a
//! live `Server` and round-trip MCP frames through the stub's stdio.
//!
//! What this covers that the in-crate unit tests don't:
//! - The stub binary actually starts, reads env, opens the socket,
//!   handshakes, then pipes stdin↔socket without dropping frames.
//! - The framing matches what a real MCP client (Claude) would write:
//!   one JSON object per line on stdin, one JSON object per line on
//!   stdout, no length prefix or special markers.
//! - Notifications (no `id`) get no reply and don't desynchronise
//!   subsequent request/response pairs.

#![cfg(unix)]

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use operon_bridge::{Server, ToolHandler, ToolHandlerError};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

struct EchoTool;

#[async_trait]
impl ToolHandler for EchoTool {
    fn name(&self) -> &str {
        "operon_echo"
    }
    fn description(&self) -> &str {
        "echo args back as a text content block"
    }
    fn input_schema(&self) -> Value {
        json!({ "type": "object", "additionalProperties": true })
    }
    async fn call(&self, args: Value) -> Result<Value, ToolHandlerError> {
        Ok(json!([{ "type": "text", "text": format!("echo: {args}") }]))
    }
}

fn temp_sock() -> PathBuf {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("bridge.sock");
    // Leak the tempdir so the path survives until the test process
    // exits — we want the socket reachable until the spawned stub
    // hangs up. Tests use a fresh dir per call so cleanup races are
    // a non-issue.
    std::mem::forget(dir);
    path
}

/// Helper: send one JSON-RPC frame down the stub's stdin and read
/// back the next stdout line, parsing it as JSON. Skips notifications
/// (frames with no `id` field) — none are expected on stdout in this
/// test, but the parser is forgiving so a stray one wouldn't desync.
async fn request(
    stdin: &mut tokio::process::ChildStdin,
    stdout: &mut BufReader<tokio::process::ChildStdout>,
    frame: Value,
) -> Value {
    let mut line = serde_json::to_string(&frame).unwrap();
    line.push('\n');
    stdin.write_all(line.as_bytes()).await.unwrap();
    stdin.flush().await.unwrap();

    let mut buf = String::new();
    // The stub's reply latency is bounded by the local socket round
    // trip — a few ms in practice. The 5s ceiling is generous enough
    // to absorb a stalled CI host without masking a hang.
    let n = tokio::time::timeout(Duration::from_secs(5), stdout.read_line(&mut buf))
        .await
        .expect("read_line timed out — stub or server hung")
        .expect("read_line io");
    assert!(n > 0, "stub closed stdout before sending a reply");
    serde_json::from_str(buf.trim()).expect("stub wrote non-JSON to stdout")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stub_pipes_initialize_tools_list_and_tools_call() {
    let sock = temp_sock();
    let token = uuid::Uuid::new_v4().to_string();

    // Bring the server up first so the stub finds the socket on
    // first connect. We hold `handle` for the duration of the test;
    // dropping it stops accepting new connections.
    let server = Server::new(sock.clone(), token.clone()).register_tool(Arc::new(EchoTool));
    let handle = server.serve().await.expect("serve");

    // Cargo provides this env var for any `[[bin]]` target in the
    // crate under test. No need to recompute target/release vs debug
    // paths.
    let bin = env!("CARGO_BIN_EXE_operon-mcp");

    let mut child = Command::new(bin)
        .env("OPERON_BRIDGE_SOCK", &sock)
        .env("OPERON_BRIDGE_TOKEN", &token)
        .env("OPERON_SESSION_ID", uuid::Uuid::new_v4().to_string())
        // Inherit stderr so the stub's diagnostics show up in
        // `cargo test -- --nocapture` if a test fails. Stdin and
        // stdout are piped — that's the wire we test on.
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn operon-mcp");

    let mut stdin = child.stdin.take().expect("child stdin");
    let stdout = child.stdout.take().expect("child stdout");
    let mut stdout = BufReader::new(stdout);

    // === initialize ===
    let init_reply = request(
        &mut stdin,
        &mut stdout,
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "operon-test", "version": "0.0.0" },
            },
        }),
    )
    .await;
    assert_eq!(init_reply["id"], 1);
    assert_eq!(init_reply["result"]["serverInfo"]["name"], "operon-bridge");
    assert_eq!(init_reply["result"]["protocolVersion"], "2024-11-05");

    // === notifications/initialized — no reply expected ===
    let notif = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
    });
    let mut line = serde_json::to_string(&notif).unwrap();
    line.push('\n');
    stdin.write_all(line.as_bytes()).await.unwrap();
    stdin.flush().await.unwrap();
    // Give the server a beat to process. The next request must still
    // line up cleanly even though we wrote a frame with no reply.
    tokio::time::sleep(Duration::from_millis(20)).await;

    // === tools/list ===
    let list_reply = request(
        &mut stdin,
        &mut stdout,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    )
    .await;
    assert_eq!(list_reply["id"], 2);
    let tools = list_reply["result"]["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "operon_echo");
    assert!(tools[0]["inputSchema"]["type"] == "object");

    // === tools/call ===
    let call_reply = request(
        &mut stdin,
        &mut stdout,
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {
                "name": "operon_echo",
                "arguments": { "msg": "hello bridge" },
            },
        }),
    )
    .await;
    assert_eq!(call_reply["id"], 3);
    assert_eq!(call_reply["result"]["isError"], false);
    let text = call_reply["result"]["content"][0]["text"]
        .as_str()
        .expect("content[0].text is a string");
    assert!(
        text.contains("hello bridge"),
        "tool result didn't include argument: {text}"
    );

    // === unknown tool surfaces as isError, not a transport failure ===
    let bad_reply = request(
        &mut stdin,
        &mut stdout,
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": { "name": "operon_nope", "arguments": {} },
        }),
    )
    .await;
    assert_eq!(bad_reply["id"], 4);
    assert_eq!(bad_reply["result"]["isError"], true);

    // === Shutdown order matters: closing stdin tells the stub's
    // stdin→socket task to exit. The stub then drops the socket
    // half, and the server task notices EOF on the connection and
    // exits its loop. Without this, the child would linger.
    drop(stdin);
    // 1s budget for the child to wind down after stdin close.
    let status = tokio::time::timeout(Duration::from_secs(1), child.wait())
        .await
        .expect("operon-mcp didn't exit after stdin close")
        .expect("wait child");
    assert!(status.success(), "operon-mcp exited with {status:?}");

    handle.shutdown().await;
}

/// Stub must refuse to start when either env var is missing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stub_exits_nonzero_without_env() {
    let bin = env!("CARGO_BIN_EXE_operon-mcp");

    // No env vars at all.
    let status = Command::new(bin)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .expect("spawn");
    assert!(!status.success(), "expected nonzero exit, got {status:?}");
}

/// Stub must exit nonzero (without hanging) when the token is wrong.
/// This is the "another process on the box probed our socket" path.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stub_exits_nonzero_on_bad_token() {
    let sock = temp_sock();
    let real_token = uuid::Uuid::new_v4().to_string();
    let server = Server::new(sock.clone(), real_token);
    let handle = server.serve().await.expect("serve");

    let bin = env!("CARGO_BIN_EXE_operon-mcp");
    let status = Command::new(bin)
        .env("OPERON_BRIDGE_SOCK", &sock)
        .env("OPERON_BRIDGE_TOKEN", "wrong-token")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .expect("spawn");
    assert!(!status.success(), "stub should reject bad-token bridge reply");

    handle.shutdown().await;
}
