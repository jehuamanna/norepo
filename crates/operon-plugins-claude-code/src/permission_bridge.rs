//! Per-session MCP permission bridge.
//!
//! `claude --print` won't render an interactive permission prompt to a
//! piped stdout, so when it needs to ask "may I run `node --test foo.js`?"
//! the request silently fails. The supported escape hatch is
//! `--permission-prompt-tool mcp__<server>__<tool>`: claude routes every
//! gated tool-use through an MCP tool the host serves and waits for a
//! `{behavior: "allow"|"deny", ...}` response before proceeding.
//!
//! `PermissionBridge` is the operon side. It binds a per-session Unix
//! socket and speaks the (very small) subset of MCP-over-stdio JSON-RPC
//! that claude needs:
//!   - `initialize` → minimal capabilities + serverInfo.
//!   - `tools/list` → exactly one tool: `permission_prompt`.
//!   - `tools/call permission_prompt` → invokes the host-supplied
//!     `PermissionHandler`, awaits the user's decision, returns the
//!     MCP-shaped allow/deny payload.
//!
//! The matching `operon-mcp-permission` shim binary is what claude
//! actually exec's via `--mcp-config`; it's a tiny stdio↔Unix-socket
//! proxy so this server can live in the host process.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{oneshot, Mutex};
use tokio::task::JoinHandle;

/// One pending permission request emitted to the host. The host arranges
/// to render a UI prompt and resolves the `oneshot::Sender` with the
/// chosen decision.
#[derive(Debug)]
pub struct PermissionRequest {
    /// Tool claude wants to run (e.g. `"Bash"`, `"Edit"`).
    pub tool_name: String,
    /// Proposed tool input verbatim. For Bash this typically contains
    /// `{"command": "...", "description": "..."}`.
    pub input: Value,
    /// Stable identifier claude uses to correlate this request with the
    /// downstream `tool_use` block. May be empty if the SDK didn't supply
    /// one.
    pub tool_use_id: Option<String>,
}

#[derive(Debug, Clone)]
pub enum PermissionDecision {
    /// Approve the tool call. `updated_input` lets the host rewrite
    /// claude's proposed args (e.g. trim a path) — `None` means "use
    /// the original input verbatim".
    Allow { updated_input: Option<Value> },
    /// Reject the tool call. `message` is surfaced back to the model as
    /// the tool result so it understands why and can adapt.
    Deny { message: String },
}

/// Host-supplied callback. Receives the request and a one-shot back
/// channel; the host wires the channel up to whatever UI prompt it
/// renders. If the channel is dropped without a response (e.g. because
/// the chat surface is torn down), the bridge defaults to deny so the
/// spawned claude doesn't hang.
pub trait PermissionHandler: Send + Sync + 'static {
    fn on_request(&self, req: PermissionRequest, respond: oneshot::Sender<PermissionDecision>);
}

impl<F> PermissionHandler for F
where
    F: Fn(PermissionRequest, oneshot::Sender<PermissionDecision>) + Send + Sync + 'static,
{
    fn on_request(&self, req: PermissionRequest, respond: oneshot::Sender<PermissionDecision>) {
        self(req, respond)
    }
}

/// MCP server name the spawned claude is told to look for. Must match
/// the `mcpServers` key in the generated MCP config + the
/// `--permission-prompt-tool mcp__<this>__permission_prompt` value.
pub const MCP_SERVER_NAME: &str = "operon";
/// Tool name claude calls to ask for permission. Combined with
/// `MCP_SERVER_NAME` to form `mcp__operon__permission_prompt`.
pub const PERMISSION_TOOL_NAME: &str = "permission_prompt";

/// Live per-session permission server. Drop it to revoke the binding
/// (any pending JSON-RPC requests then resolve to deny via the handler
/// channel close path) and unlink the socket file.
pub struct PermissionBridge {
    socket_path: PathBuf,
    accept_task: Option<JoinHandle<()>>,
}

impl PermissionBridge {
    /// Bind a Unix socket at `socket_path` and start accepting MCP
    /// JSON-RPC connections. The bridge keeps running until dropped.
    /// Pre-existing files at `socket_path` are unlinked first so a
    /// stale socket from a crashed prior session doesn't block the
    /// bind.
    pub async fn bind<H: PermissionHandler>(
        socket_path: PathBuf,
        handler: H,
    ) -> std::io::Result<Self> {
        if socket_path.exists() {
            // Best-effort — if the file isn't actually a stale socket,
            // the bind below will surface the real error.
            let _ = tokio::fs::remove_file(&socket_path).await;
        }
        if let Some(parent) = socket_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let listener = UnixListener::bind(&socket_path)?;
        let handler = Arc::new(handler);
        let accept_task = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let h = handler.clone();
                        tokio::spawn(handle_connection(stream, h));
                    }
                    Err(e) => {
                        tracing::warn!(target: "operon::permission", "accept: {e}");
                        // Brief backoff so a tight failure loop doesn't
                        // spin the CPU.
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                }
            }
        });
        Ok(Self {
            socket_path,
            accept_task: Some(accept_task),
        })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

impl Drop for PermissionBridge {
    fn drop(&mut self) {
        if let Some(h) = self.accept_task.take() {
            h.abort();
        }
        // Best-effort cleanup of the socket file. Errors are ignored —
        // a stale socket on the next bind triggers the unlink path
        // there.
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

async fn handle_connection<H: PermissionHandler>(stream: UnixStream, handler: Arc<H>) {
    let (read_half, write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half).lines();
    let writer = Arc::new(Mutex::new(write_half));
    while let Ok(Some(line)) = reader.next_line().await {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(target: "operon::permission", "bad json frame: {e}");
                continue;
            }
        };
        let id = msg.get("id").cloned();
        let method = msg
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        if method.is_empty() {
            // No `method` — likely a response we don't expect. Skip.
            continue;
        }
        match method.as_str() {
            "initialize" => {
                let resp = json_rpc_result(
                    id,
                    serde_json::json!({
                        "protocolVersion": "2024-11-05",
                        "capabilities": { "tools": {} },
                        "serverInfo": {
                            "name": MCP_SERVER_NAME,
                            "version": env!("CARGO_PKG_VERSION"),
                        }
                    }),
                );
                send(&writer, resp).await;
            }
            "notifications/initialized" | "notifications/cancelled" => {
                // Notifications: no response. Cancelled is best-effort —
                // the spawned task may have already pushed the request
                // to the UI and the user might still respond.
            }
            "tools/list" => {
                let resp = json_rpc_result(
                    id,
                    serde_json::json!({
                        "tools": [{
                            "name": PERMISSION_TOOL_NAME,
                            "description": "Ask the user to approve a Claude tool call",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "tool_name": { "type": "string" },
                                    "input": { "type": "object" },
                                    "tool_use_id": { "type": "string" }
                                },
                                "required": ["tool_name", "input"]
                            }
                        }]
                    }),
                );
                send(&writer, resp).await;
            }
            "tools/call" => {
                let params = msg.get("params").cloned().unwrap_or(Value::Null);
                let name = params
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                if name != PERMISSION_TOOL_NAME {
                    let resp = json_rpc_error(
                        id,
                        -32601,
                        format!("unknown tool: {name}"),
                    );
                    send(&writer, resp).await;
                    continue;
                }
                let args = params.get("arguments").cloned().unwrap_or(Value::Null);
                let req = PermissionRequest {
                    tool_name: args
                        .get("tool_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    input: args.get("input").cloned().unwrap_or(Value::Null),
                    tool_use_id: args
                        .get("tool_use_id")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                };
                let writer = writer.clone();
                let handler = handler.clone();
                tokio::spawn(async move {
                    let (tx, rx) = oneshot::channel();
                    handler.on_request(req, tx);
                    let decision = rx.await.unwrap_or(PermissionDecision::Deny {
                        message: "Operon: bridge closed before user responded".into(),
                    });
                    let payload = decision_to_payload(&decision);
                    let resp = json_rpc_result(
                        id,
                        serde_json::json!({
                            "content": [{
                                "type": "text",
                                "text": payload.to_string(),
                            }]
                        }),
                    );
                    send(&writer, resp).await;
                });
            }
            _ => {
                if id.is_some() {
                    let resp = json_rpc_error(
                        id,
                        -32601,
                        format!("method not found: {method}"),
                    );
                    send(&writer, resp).await;
                }
            }
        }
    }
}

fn json_rpc_result(id: Option<Value>, result: Value) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "result": result,
    })
}

fn json_rpc_error(id: Option<Value>, code: i64, message: String) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "error": { "code": code, "message": message },
    })
}

fn decision_to_payload(decision: &PermissionDecision) -> Value {
    match decision {
        PermissionDecision::Allow { updated_input } => serde_json::json!({
            "behavior": "allow",
            "updatedInput": updated_input.clone().unwrap_or(Value::Object(Default::default())),
        }),
        PermissionDecision::Deny { message } => serde_json::json!({
            "behavior": "deny",
            "message": message,
        }),
    }
}

async fn send(writer: &Arc<Mutex<tokio::net::unix::OwnedWriteHalf>>, msg: Value) {
    let mut guard = writer.lock().await;
    let line = format!("{msg}\n");
    if let Err(e) = guard.write_all(line.as_bytes()).await {
        tracing::warn!(target: "operon::permission", "write: {e}");
        return;
    }
    if let Err(e) = guard.flush().await {
        tracing::warn!(target: "operon::permission", "flush: {e}");
    }
}

/// Build the MCP config that points claude at the shim binary which in
/// turn proxies to `socket_path`. `shim_bin` is the absolute path to
/// the `operon-mcp-permission` binary. The result is intended to be
/// written to a tempfile and passed via `claude --mcp-config <path>`.
pub fn build_mcp_config(shim_bin: &Path, socket_path: &Path) -> Value {
    let mut servers = serde_json::Map::new();
    servers.insert(
        MCP_SERVER_NAME.to_string(),
        serde_json::json!({
            "type": "stdio",
            "command": shim_bin.to_string_lossy(),
            "args": ["--socket", socket_path.to_string_lossy()],
        }),
    );
    serde_json::json!({ "mcpServers": Value::Object(servers) })
}

/// `--permission-prompt-tool` value matching the bridge's tool.
pub fn permission_prompt_tool_arg() -> String {
    format!("mcp__{MCP_SERVER_NAME}__{PERMISSION_TOOL_NAME}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use tempfile::tempdir;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    /// Bridge handler that auto-allows every request. Used by tests
    /// that don't care about the handler invocation.
    fn allow_all() -> impl Fn(PermissionRequest, oneshot::Sender<PermissionDecision>)
           + Send
           + Sync
           + 'static {
        |_req: PermissionRequest, respond: oneshot::Sender<PermissionDecision>| {
            let _ = respond.send(PermissionDecision::Allow { updated_input: None });
        }
    }

    /// Helper: connect a fresh client to the bridge and run one
    /// JSON-RPC request/response round-trip.
    async fn rpc(stream: &mut UnixStream, frame: Value) -> Value {
        let line = format!("{frame}\n");
        stream.write_all(line.as_bytes()).await.unwrap();
        stream.flush().await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut response_line = String::new();
        reader.read_line(&mut response_line).await.unwrap();
        serde_json::from_str(response_line.trim()).unwrap()
    }

    #[tokio::test]
    async fn initialize_returns_capabilities_and_server_info() {
        let dir = tempdir().unwrap();
        let sock = dir.path().join("perm.sock");
        let _bridge = PermissionBridge::bind(sock.clone(), allow_all())
            .await
            .unwrap();

        let mut stream = UnixStream::connect(&sock).await.unwrap();
        let resp = rpc(
            &mut stream,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {}
            }),
        )
        .await;
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"]["serverInfo"]["name"], MCP_SERVER_NAME);
        assert!(resp["result"]["capabilities"]["tools"].is_object());
    }

    #[tokio::test]
    async fn tools_list_advertises_permission_prompt() {
        let dir = tempdir().unwrap();
        let sock = dir.path().join("perm.sock");
        let _bridge = PermissionBridge::bind(sock.clone(), allow_all())
            .await
            .unwrap();

        let mut stream = UnixStream::connect(&sock).await.unwrap();
        let resp = rpc(
            &mut stream,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list",
                "params": {}
            }),
        )
        .await;
        assert_eq!(resp["id"], 2);
        let tools = resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], PERMISSION_TOOL_NAME);
    }

    #[tokio::test]
    async fn tools_call_routes_to_handler_and_returns_allow() {
        let dir = tempdir().unwrap();
        let sock = dir.path().join("perm.sock");
        let captured = Arc::new(StdMutex::new(None::<PermissionRequest>));
        let captured_for_handler = captured.clone();
        let handler = move |req: PermissionRequest,
                            respond: oneshot::Sender<PermissionDecision>| {
            *captured_for_handler.lock().unwrap() = Some(PermissionRequest {
                tool_name: req.tool_name.clone(),
                input: req.input.clone(),
                tool_use_id: req.tool_use_id.clone(),
            });
            // Allow with the original input verbatim.
            let _ = respond.send(PermissionDecision::Allow { updated_input: None });
        };
        let _bridge = PermissionBridge::bind(sock.clone(), handler)
        .await
        .unwrap();

        let mut stream = UnixStream::connect(&sock).await.unwrap();
        let resp = rpc(
            &mut stream,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": PERMISSION_TOOL_NAME,
                    "arguments": {
                        "tool_name": "Bash",
                        "input": { "command": "node --test foo.js" },
                        "tool_use_id": "toolu_42"
                    }
                }
            }),
        )
        .await;
        assert_eq!(resp["id"], 3);
        let payload_text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(payload_text).unwrap();
        assert_eq!(payload["behavior"], "allow");
        // updatedInput defaults to {} when host doesn't supply one.
        assert!(payload["updatedInput"].is_object());

        let cap = captured.lock().unwrap();
        let r = cap.as_ref().expect("handler should have been invoked");
        assert_eq!(r.tool_name, "Bash");
        assert_eq!(r.tool_use_id.as_deref(), Some("toolu_42"));
        assert_eq!(r.input["command"], "node --test foo.js");
    }

    #[tokio::test]
    async fn tools_call_returns_deny_with_message() {
        let dir = tempdir().unwrap();
        let sock = dir.path().join("perm.sock");
        let handler = |_req: PermissionRequest, respond: oneshot::Sender<PermissionDecision>| {
            let _ = respond.send(PermissionDecision::Deny {
                message: "nope".into(),
            });
        };
        let _bridge = PermissionBridge::bind(sock.clone(), handler)
        .await
        .unwrap();

        let mut stream = UnixStream::connect(&sock).await.unwrap();
        let resp = rpc(
            &mut stream,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {
                    "name": PERMISSION_TOOL_NAME,
                    "arguments": {
                        "tool_name": "Bash",
                        "input": { "command": "rm -rf /" }
                    }
                }
            }),
        )
        .await;
        let payload_text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(payload_text).unwrap();
        assert_eq!(payload["behavior"], "deny");
        assert_eq!(payload["message"], "nope");
    }

    #[tokio::test]
    async fn dropped_responder_resolves_to_deny() {
        let dir = tempdir().unwrap();
        let sock = dir.path().join("perm.sock");
        let handler = |_req: PermissionRequest, respond: oneshot::Sender<PermissionDecision>| {
            // Drop the sender without responding — emulates UI tear-down.
            drop(respond);
        };
        let _bridge = PermissionBridge::bind(sock.clone(), handler)
        .await
        .unwrap();

        let mut stream = UnixStream::connect(&sock).await.unwrap();
        let resp = rpc(
            &mut stream,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": {
                    "name": PERMISSION_TOOL_NAME,
                    "arguments": { "tool_name": "Bash", "input": {} }
                }
            }),
        )
        .await;
        let payload_text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(payload_text).unwrap();
        assert_eq!(payload["behavior"], "deny");
        assert!(payload["message"]
            .as_str()
            .unwrap()
            .contains("bridge closed"));
    }

    #[tokio::test]
    async fn unknown_tool_returns_jsonrpc_error() {
        let dir = tempdir().unwrap();
        let sock = dir.path().join("perm.sock");
        let _bridge = PermissionBridge::bind(sock.clone(), allow_all())
            .await
            .unwrap();

        let mut stream = UnixStream::connect(&sock).await.unwrap();
        let resp = rpc(
            &mut stream,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 6,
                "method": "tools/call",
                "params": { "name": "not_a_real_tool", "arguments": {} }
            }),
        )
        .await;
        assert_eq!(resp["id"], 6);
        assert_eq!(resp["error"]["code"], -32601);
    }

    #[test]
    fn build_mcp_config_uses_shim_command_with_socket_arg() {
        let cfg = build_mcp_config(
            Path::new("/usr/local/bin/operon-mcp-permission"),
            Path::new("/run/operon/perm.sock"),
        );
        let server = &cfg["mcpServers"][MCP_SERVER_NAME];
        assert_eq!(server["type"], "stdio");
        assert_eq!(server["command"], "/usr/local/bin/operon-mcp-permission");
        let args = server["args"].as_array().unwrap();
        assert_eq!(args[0], "--socket");
        assert_eq!(args[1], "/run/operon/perm.sock");
    }

    #[test]
    fn permission_prompt_tool_arg_matches_server_and_tool_names() {
        assert_eq!(
            permission_prompt_tool_arg(),
            format!("mcp__{MCP_SERVER_NAME}__{PERMISSION_TOOL_NAME}")
        );
    }
}
