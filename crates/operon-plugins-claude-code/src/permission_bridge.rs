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
#[cfg(unix)]
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::oneshot;
#[cfg(unix)]
use tokio::sync::Mutex;
#[cfg(unix)]
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
    /// "Skip this tool but continue the turn." Returns a synthetic
    /// result body to the model in place of running the tool. On the
    /// wire we send the same shape as `Deny` (claude treats it as a
    /// tool failure that the model can recover from); the host UI
    /// distinguishes Skipped from Denied for audit purposes via the
    /// status label, not the protocol.
    Skip { synthetic_result: String },
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
/// Optional companion tool: when the host registers a [`ShellExecutor`]
/// the bridge advertises this tool name so claude can route Bash calls
/// through Operon's own subprocess runner (which streams stdout/stderr
/// live + supports per-tool cancel). Disabled by default.
pub const SHELL_TOOL_NAME: &str = "operon_bash";
/// M4 — typed artifact creation tool. When the host registers an
/// [`ArtifactExecutor`] (via [`PermissionBridge::set_artifact_executor`])
/// the bridge advertises `mcp__operon__create_artifact` so Claude can
/// emit SDLC artifacts as typed tool calls (kind + parent_id are
/// arguments, not English instructions in the prompt). The matching
/// host-side executor creates a `NoteKind::Artifact` note under the
/// supplied parent, eliminating the legacy "Write to a directory and
/// mtime-scan after the run" handshake and the heuristic re-parenting
/// it required.
pub const CREATE_ARTIFACT_TOOL_NAME: &str = "create_artifact";
/// Custom replacement for the harness-owned `AskUserQuestion` built-in.
/// The harness intercepts AskUserQuestion tool_results in non-TUI mode
/// (rewriting them to `is_error: true` or auto-synthesising empty
/// answers), so the only way to ship interactive answers back to the
/// model is to expose our own MCP tool with the same input shape.
/// Advertised in `tools/list` when an [`AskUserExecutor`] is set via
/// [`PermissionBridge::set_ask_user_executor`].
pub const ASK_USER_TOOL_NAME: &str = "ask_user";

/// Host-supplied shell executor wired in via [`PermissionBridge::with_shell_executor`].
/// When set, the bridge advertises [`SHELL_TOOL_NAME`] in its `tools/list`
/// response and routes incoming `operon_bash` calls through this trait.
///
/// The host gets to decide how to actually run the command and where
/// to send streaming chunks — typically [`operon-plugins-tools::ShellTool`]
/// with a chunk sink that pushes into the Operon UI's `TOOL_STREAM_OUTPUT`
/// signal. Returning `None` from `execute` is the "shell executor declined
/// or failed" path; the bridge surfaces it to claude as a tool error so
/// the model can recover.
pub trait ShellExecutor: Send + Sync + 'static {
    fn execute<'a>(
        &'a self,
        tool_use_id: String,
        args: Value,
    ) -> futures::future::BoxFuture<'a, OperonResult<Value>>;
}

/// Host-supplied typed-artifact creator wired in via
/// [`PermissionBridge::set_artifact_executor`]. When set, the bridge
/// advertises [`CREATE_ARTIFACT_TOOL_NAME`] and routes
/// `tools/call create_artifact` invocations through `create`.
///
/// `args` is the JSON object Claude passed; the host validates fields
/// (kind, parent_id, title, body), performs the actual artifact note
/// creation, and returns either a success payload with the new note's
/// id or an error so the model can recover. The returned `Value` is
/// serialised verbatim into the MCP tool-result content.
///
/// Returns a [`LocalBoxFuture`] (not `BoxFuture`) because Operon's
/// [`Persistence`] futures hold wasm `JsValue` handles and are
/// intentionally `!Send`. The bridge awaits the future inline on the
/// connection task rather than spawning it.
pub trait ArtifactExecutor: Send + Sync + 'static {
    fn create<'a>(
        &'a self,
        args: Value,
    ) -> futures::future::LocalBoxFuture<'a, OperonResult<Value>>;
}

/// Host-supplied executor for the custom `ask_user` MCP tool.
/// When set, the bridge advertises [`ASK_USER_TOOL_NAME`] in
/// `tools/list` and routes `tools/call ask_user` invocations to
/// `ask`. The executor surfaces an interactive picker UI, awaits the
/// user's selection(s), and returns a JSON payload that gets handed
/// to Claude verbatim as the MCP tool result. The expected shape
/// mirrors the built-in AskUserQuestion's internal response:
/// `{ "questions": [...as-supplied...], "answers": { <question text>: <selected label or array> } }`.
///
/// Uses `BoxFuture` (Send) — unlike `ArtifactExecutor`, this path
/// doesn't touch the wasm-bound `Persistence` futures, so it can run
/// directly on the connection task without `spawn_blocking`.
pub trait AskUserExecutor: Send + Sync + 'static {
    fn ask<'a>(&'a self, args: Value) -> futures::future::BoxFuture<'a, OperonResult<Value>>;
}

use operon_core::error::OperonResult;

/// Live per-session permission server. Drop it to revoke the binding
/// (any pending JSON-RPC requests then resolve to deny via the handler
/// channel close path) and unlink the socket file.
///
/// Only functional on Unix (uses Unix domain sockets). On Windows the
/// struct is a stub that always returns `Unsupported` from `bind()`.
#[cfg(unix)]
pub struct PermissionBridge {
    socket_path: PathBuf,
    accept_task: Option<JoinHandle<()>>,
    shell_executor: Arc<std::sync::Mutex<Option<Arc<dyn ShellExecutor>>>>,
    artifact_executor: Arc<std::sync::Mutex<Option<Arc<dyn ArtifactExecutor>>>>,
    ask_user_executor: Arc<std::sync::Mutex<Option<Arc<dyn AskUserExecutor>>>>,
}

/// Stub for non-Unix platforms (Windows). The permission bridge is
/// unsupported — `bind()` always returns an error.
#[cfg(not(unix))]
pub struct PermissionBridge {
    socket_path: PathBuf,
}

#[cfg(not(unix))]
impl PermissionBridge {
    /// Unix domain sockets are not available on this platform.
    pub async fn bind<H: PermissionHandler>(
        socket_path: PathBuf,
        _handler: H,
    ) -> std::io::Result<Self> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "PermissionBridge requires Unix domain sockets (not available on Windows)",
        ))
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn set_shell_executor(&self, _executor: Option<Arc<dyn ShellExecutor>>) {}
    pub fn set_artifact_executor(&self, _executor: Option<Arc<dyn ArtifactExecutor>>) {}
    pub fn set_ask_user_executor(&self, _executor: Option<Arc<dyn AskUserExecutor>>) {}
}

#[cfg(unix)]
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
        let shell_executor: Arc<std::sync::Mutex<Option<Arc<dyn ShellExecutor>>>> =
            Arc::new(std::sync::Mutex::new(None));
        let artifact_executor: Arc<std::sync::Mutex<Option<Arc<dyn ArtifactExecutor>>>> =
            Arc::new(std::sync::Mutex::new(None));
        let ask_user_executor: Arc<std::sync::Mutex<Option<Arc<dyn AskUserExecutor>>>> =
            Arc::new(std::sync::Mutex::new(None));
        let shell_for_accept = shell_executor.clone();
        let artifact_for_accept = artifact_executor.clone();
        let ask_user_for_accept = ask_user_executor.clone();
        let accept_task = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let h = handler.clone();
                        let s = shell_for_accept.clone();
                        let a = artifact_for_accept.clone();
                        let q = ask_user_for_accept.clone();
                        tokio::spawn(handle_connection(stream, h, s, a, q));
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
            shell_executor,
            artifact_executor,
            ask_user_executor,
        })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Install a shell executor. Once set, the bridge advertises the
    /// `operon_bash` tool in subsequent `tools/list` responses and
    /// routes incoming `operon_bash` calls through `executor`. Pass
    /// `None` to detach; the bridge stops advertising the tool for
    /// new sessions (in-flight ones already consumed the previous
    /// `tools/list`).
    pub fn set_shell_executor(&self, executor: Option<Arc<dyn ShellExecutor>>) {
        if let Ok(mut s) = self.shell_executor.lock() {
            *s = executor;
        }
    }

    /// Install a typed-artifact executor. Once set, the bridge
    /// advertises `create_artifact` in subsequent `tools/list`
    /// responses and routes calls through `executor`. Pass `None` to
    /// detach.
    pub fn set_artifact_executor(&self, executor: Option<Arc<dyn ArtifactExecutor>>) {
        if let Ok(mut s) = self.artifact_executor.lock() {
            *s = executor;
        }
    }

    /// Install an ask-user executor. Once set, the bridge advertises
    /// `ask_user` in subsequent `tools/list` responses and routes calls
    /// through `executor`. Pass `None` to detach.
    pub fn set_ask_user_executor(&self, executor: Option<Arc<dyn AskUserExecutor>>) {
        if let Ok(mut s) = self.ask_user_executor.lock() {
            *s = executor;
        }
    }
}

#[cfg(unix)]
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

#[cfg(unix)]
async fn handle_connection<H: PermissionHandler>(
    stream: UnixStream,
    handler: Arc<H>,
    shell: Arc<std::sync::Mutex<Option<Arc<dyn ShellExecutor>>>>,
    artifact: Arc<std::sync::Mutex<Option<Arc<dyn ArtifactExecutor>>>>,
    ask_user: Arc<std::sync::Mutex<Option<Arc<dyn AskUserExecutor>>>>,
) {
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
                let mut tools = vec![serde_json::json!({
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
                })];
                // Advertise the bash-hijack tool only when the host
                // has installed an executor. The CLI-side
                // `bash_via_operon` toggle is what gates that
                // installation so users opt in explicitly.
                let shell_active = shell
                    .lock()
                    .ok()
                    .map(|g| g.is_some())
                    .unwrap_or(false);
                if shell_active {
                    tools.push(serde_json::json!({
                        "name": SHELL_TOOL_NAME,
                        "description": "Run a bash command through Operon's runner. Operon streams stdout/stderr live to the chat UI and supports per-tool cancellation.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "command":    { "type": "string" },
                                "cwd":        { "type": "string" },
                                "timeout_ms": { "type": "integer", "minimum": 1 }
                            },
                            "required": ["command"]
                        }
                    }));
                }
                let artifact_active = artifact
                    .lock()
                    .ok()
                    .map(|g| g.is_some())
                    .unwrap_or(false);
                if artifact_active {
                    tools.push(serde_json::json!({
                        "name": CREATE_ARTIFACT_TOOL_NAME,
                        "description": "Create an SDLC artifact note in Operon (typed alternative to writing a .md file). Use this when a skill says it produces an artifact of a declared `output_kind` — the kind and parent are validated against Operon's note tree at the moment of the call, so structural mistakes are rejected immediately instead of being patched up post-hoc.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "kind":      { "type": "string", "description": "Artifact kind, e.g. epic, feature, story, task, plan, architecture." },
                                "parent_id": { "type": "string", "description": "UUID of the parent artifact (or master_requirement). Must already exist in this project." },
                                "title":     { "type": "string", "description": "Human-readable title for the artifact note." },
                                "body":      { "type": "string", "description": "Markdown body. Operon ensures the YAML frontmatter declares `artifact_kind` and `parent` consistent with the typed arguments above." }
                            },
                            "required": ["kind", "parent_id", "title", "body"]
                        }
                    }));
                }
                let ask_user_active = ask_user
                    .lock()
                    .ok()
                    .map(|g| g.is_some())
                    .unwrap_or(false);
                if ask_user_active {
                    tools.push(serde_json::json!({
                        "name": ASK_USER_TOOL_NAME,
                        "description": "Ask the user a clarifying question with structured options. Use this whenever you would normally use the built-in AskUserQuestion tool — that one is disabled here, so this is the only way to surface a picker. Input shape mirrors AskUserQuestion exactly. The response will be `{questions, answers}` where `answers` maps each question text to the chosen option label (string for single-select, array for multiSelect).",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "questions": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "question":    { "type": "string", "description": "The complete question. Should end with a question mark." },
                                            "header":      { "type": "string", "description": "Very short label (max ~12 chars)." },
                                            "multiSelect": { "type": "boolean", "description": "If true, allow multiple selections. Defaults to false." },
                                            "options": {
                                                "type": "array",
                                                "items": {
                                                    "type": "object",
                                                    "properties": {
                                                        "label":       { "type": "string" },
                                                        "description": { "type": "string" }
                                                    },
                                                    "required": ["label", "description"]
                                                }
                                            }
                                        },
                                        "required": ["question", "header", "options"]
                                    }
                                }
                            },
                            "required": ["questions"]
                        }
                    }));
                }
                let resp = json_rpc_result(id, serde_json::json!({ "tools": tools }));
                send(&writer, resp).await;
            }
            "tools/call" => {
                let params = msg.get("params").cloned().unwrap_or(Value::Null);
                let name = params
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                // operon_bash branch: route to the host's ShellExecutor
                // if one is installed; otherwise fall through to the
                // unknown-tool error below.
                if name == SHELL_TOOL_NAME {
                    let executor = shell
                        .lock()
                        .ok()
                        .and_then(|g| g.as_ref().cloned());
                    if let Some(exec) = executor {
                        let args = params
                            .get("arguments")
                            .cloned()
                            .unwrap_or(Value::Null);
                        let tool_use_id = args
                            .get("tool_use_id")
                            .and_then(|v| v.as_str())
                            .map(String::from)
                            .unwrap_or_else(|| {
                                uuid::Uuid::new_v4().to_string()
                            });
                        let writer = writer.clone();
                        tokio::spawn(async move {
                            let result = exec.execute(tool_use_id, args).await;
                            let resp = match result {
                                Ok(v) => json_rpc_result(
                                    id,
                                    serde_json::json!({
                                        "content": [{
                                            "type": "text",
                                            "text": v.to_string(),
                                        }]
                                    }),
                                ),
                                Err(e) => json_rpc_result(
                                    id,
                                    serde_json::json!({
                                        "content": [{
                                            "type": "text",
                                            "text": format!("{{\"error\":\"{}\"}}", e),
                                        }],
                                        "isError": true,
                                    }),
                                ),
                            };
                            send(&writer, resp).await;
                        });
                        continue;
                    }
                    let resp = json_rpc_error(
                        id,
                        -32601,
                        "operon_bash advertised but no executor installed".to_string(),
                    );
                    send(&writer, resp).await;
                    continue;
                }
                if name == CREATE_ARTIFACT_TOOL_NAME {
                    let executor = artifact
                        .lock()
                        .ok()
                        .and_then(|g| g.as_ref().cloned());
                    if let Some(exec) = executor {
                        let args = params
                            .get("arguments")
                            .cloned()
                            .unwrap_or(Value::Null);
                        let writer = writer.clone();
                        // The executor's future is `!Send` because
                        // Operon's `Persistence` futures hold wasm
                        // `JsValue` handles. `spawn_blocking` parks
                        // it on a dedicated blocking thread where
                        // `block_on` can drive it without Send.
                        // Then we await the JoinHandle from the
                        // (Send) connection task.
                        tokio::spawn(async move {
                            let result = tokio::task::spawn_blocking(move || {
                                futures::executor::block_on(exec.create(args))
                            })
                            .await;
                            let inner = match result {
                                Ok(r) => r,
                                Err(join_err) => Err(operon_core::error::OperonError::Plugin {
                                    plugin: "create_artifact".into(),
                                    source: Box::new(std::io::Error::other(format!(
                                        "blocking task panicked: {join_err}"
                                    ))),
                                }),
                            };
                            let resp = match inner {
                                Ok(v) => json_rpc_result(
                                    id,
                                    serde_json::json!({
                                        "content": [{
                                            "type": "text",
                                            "text": v.to_string(),
                                        }]
                                    }),
                                ),
                                Err(e) => json_rpc_result(
                                    id,
                                    serde_json::json!({
                                        "content": [{
                                            "type": "text",
                                            "text": format!("{{\"error\":\"{}\"}}", e),
                                        }],
                                        "isError": true,
                                    }),
                                ),
                            };
                            send(&writer, resp).await;
                        });
                        continue;
                    }
                    let resp = json_rpc_error(
                        id,
                        -32601,
                        "create_artifact advertised but no executor installed".to_string(),
                    );
                    send(&writer, resp).await;
                    continue;
                }
                if name == ASK_USER_TOOL_NAME {
                    let executor = ask_user
                        .lock()
                        .ok()
                        .and_then(|g| g.as_ref().cloned());
                    if let Some(exec) = executor {
                        let args = params
                            .get("arguments")
                            .cloned()
                            .unwrap_or(Value::Null);
                        let writer = writer.clone();
                        tokio::spawn(async move {
                            let result = exec.ask(args).await;
                            let resp = match result {
                                Ok(v) => json_rpc_result(
                                    id,
                                    serde_json::json!({
                                        "content": [{
                                            "type": "text",
                                            "text": v.to_string(),
                                        }]
                                    }),
                                ),
                                Err(e) => json_rpc_result(
                                    id,
                                    serde_json::json!({
                                        "content": [{
                                            "type": "text",
                                            "text": format!("{{\"error\":\"{}\"}}", e),
                                        }],
                                        "isError": true,
                                    }),
                                ),
                            };
                            send(&writer, resp).await;
                        });
                        continue;
                    }
                    let resp = json_rpc_error(
                        id,
                        -32601,
                        "ask_user advertised but no executor installed".to_string(),
                    );
                    send(&writer, resp).await;
                    continue;
                }
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

#[cfg(unix)]
fn json_rpc_result(id: Option<Value>, result: Value) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "result": result,
    })
}

#[cfg(unix)]
fn json_rpc_error(id: Option<Value>, code: i64, message: String) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "error": { "code": code, "message": message },
    })
}

#[cfg(unix)]
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
        PermissionDecision::Skip { synthetic_result } => serde_json::json!({
            "behavior": "deny",
            "message": synthetic_result,
        }),
    }
}

#[cfg(unix)]
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

#[cfg(all(test, unix))]
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
        // Without artifact + shell executors, only the permission
        // prompt is advertised.
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], PERMISSION_TOOL_NAME);
    }

    // ===== M4: create_artifact tool surface =====

    /// Test artifact executor that records calls and returns a fixed
    /// JSON payload. Used by the two M4 tests below.
    struct TestArtifactExecutor {
        captured: Arc<StdMutex<Vec<Value>>>,
        result: Value,
    }

    impl ArtifactExecutor for TestArtifactExecutor {
        fn create<'a>(
            &'a self,
            args: Value,
        ) -> futures::future::LocalBoxFuture<'a, operon_core::error::OperonResult<Value>>
        {
            let captured = self.captured.clone();
            let result = self.result.clone();
            Box::pin(async move {
                captured.lock().unwrap().push(args);
                Ok(result)
            })
        }
    }

    #[tokio::test]
    async fn tools_list_advertises_create_artifact_when_executor_installed() {
        let dir = tempdir().unwrap();
        let sock = dir.path().join("perm.sock");
        let bridge = PermissionBridge::bind(sock.clone(), allow_all())
            .await
            .unwrap();
        bridge.set_artifact_executor(Some(Arc::new(TestArtifactExecutor {
            captured: Arc::new(StdMutex::new(Vec::new())),
            result: serde_json::json!({"id": "00000000-0000-0000-0000-000000000000"}),
        })));

        let mut stream = UnixStream::connect(&sock).await.unwrap();
        let resp = rpc(
            &mut stream,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "tools/list",
                "params": {}
            }),
        )
        .await;
        let tools = resp["result"]["tools"].as_array().unwrap();
        // Two tools now: permission_prompt + create_artifact.
        let names: Vec<&str> = tools
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&PERMISSION_TOOL_NAME));
        assert!(names.contains(&CREATE_ARTIFACT_TOOL_NAME));
    }

    #[tokio::test]
    async fn tools_call_create_artifact_routes_to_executor() {
        let dir = tempdir().unwrap();
        let sock = dir.path().join("perm.sock");
        let captured = Arc::new(StdMutex::new(Vec::new()));
        let bridge = PermissionBridge::bind(sock.clone(), allow_all())
            .await
            .unwrap();
        bridge.set_artifact_executor(Some(Arc::new(TestArtifactExecutor {
            captured: captured.clone(),
            result: serde_json::json!({
                "id": "abcd1234-0000-0000-0000-000000000000",
                "kind": "epic",
            }),
        })));

        let mut stream = UnixStream::connect(&sock).await.unwrap();
        let resp = rpc(
            &mut stream,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 8,
                "method": "tools/call",
                "params": {
                    "name": CREATE_ARTIFACT_TOOL_NAME,
                    "arguments": {
                        "kind": "epic",
                        "parent_id": "11111111-2222-3333-4444-555555555555",
                        "title": "Auth flow",
                        "body": "## Goals\n\n- ship it"
                    }
                }
            }),
        )
        .await;

        assert_eq!(resp["id"], 8);
        let payload_text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(payload_text.contains("abcd1234"), "{payload_text}");

        let cap = captured.lock().unwrap();
        assert_eq!(cap.len(), 1);
        assert_eq!(cap[0]["kind"], "epic");
        assert_eq!(cap[0]["title"], "Auth flow");
    }

    #[tokio::test]
    async fn tools_call_create_artifact_without_executor_returns_error() {
        let dir = tempdir().unwrap();
        let sock = dir.path().join("perm.sock");
        // Bind the bridge but DO NOT install an artifact executor.
        let _bridge = PermissionBridge::bind(sock.clone(), allow_all())
            .await
            .unwrap();

        let mut stream = UnixStream::connect(&sock).await.unwrap();
        let resp = rpc(
            &mut stream,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 9,
                "method": "tools/call",
                "params": {
                    "name": CREATE_ARTIFACT_TOOL_NAME,
                    "arguments": {}
                }
            }),
        )
        .await;
        // Since the tool isn't advertised (no executor installed),
        // calling it falls through to the unknown-tool branch.
        assert_eq!(resp["id"], 9);
        assert_eq!(resp["error"]["code"], -32601);
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

    // ===== ask_user tool surface =====

    /// Test ask-user executor: records the question payload it
    /// received and returns a canned answers map.
    struct TestAskUserExecutor {
        captured: Arc<StdMutex<Vec<Value>>>,
        result: Value,
    }

    impl AskUserExecutor for TestAskUserExecutor {
        fn ask<'a>(
            &'a self,
            args: Value,
        ) -> futures::future::BoxFuture<'a, operon_core::error::OperonResult<Value>> {
            let captured = self.captured.clone();
            let result = self.result.clone();
            Box::pin(async move {
                captured.lock().unwrap().push(args);
                Ok(result)
            })
        }
    }

    #[tokio::test]
    async fn tools_list_advertises_ask_user_when_executor_installed() {
        let dir = tempdir().unwrap();
        let sock = dir.path().join("perm.sock");
        let bridge = PermissionBridge::bind(sock.clone(), allow_all())
            .await
            .unwrap();
        bridge.set_ask_user_executor(Some(Arc::new(TestAskUserExecutor {
            captured: Arc::new(StdMutex::new(Vec::new())),
            result: serde_json::json!({"questions": [], "answers": {}}),
        })));

        let mut stream = UnixStream::connect(&sock).await.unwrap();
        let resp = rpc(
            &mut stream,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 10,
                "method": "tools/list",
                "params": {}
            }),
        )
        .await;
        let tools = resp["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&PERMISSION_TOOL_NAME));
        assert!(names.contains(&ASK_USER_TOOL_NAME));
    }

    #[tokio::test]
    async fn tools_call_ask_user_routes_to_executor() {
        let dir = tempdir().unwrap();
        let sock = dir.path().join("perm.sock");
        let captured = Arc::new(StdMutex::new(Vec::new()));
        let bridge = PermissionBridge::bind(sock.clone(), allow_all())
            .await
            .unwrap();
        bridge.set_ask_user_executor(Some(Arc::new(TestAskUserExecutor {
            captured: captured.clone(),
            result: serde_json::json!({
                "questions": [{
                    "question": "Pick one?",
                    "header": "Pick",
                    "options": [{"label": "a", "description": "alpha"}]
                }],
                "answers": { "Pick one?": "a" },
            }),
        })));

        let mut stream = UnixStream::connect(&sock).await.unwrap();
        let resp = rpc(
            &mut stream,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 11,
                "method": "tools/call",
                "params": {
                    "name": ASK_USER_TOOL_NAME,
                    "arguments": {
                        "questions": [{
                            "question": "Pick one?",
                            "header": "Pick",
                            "options": [{"label": "a", "description": "alpha"}]
                        }]
                    }
                }
            }),
        )
        .await;

        assert_eq!(resp["id"], 11);
        let payload_text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(payload_text).unwrap();
        assert_eq!(payload["answers"]["Pick one?"], "a");

        let cap = captured.lock().unwrap();
        assert_eq!(cap.len(), 1);
        assert_eq!(cap[0]["questions"][0]["question"], "Pick one?");
    }

    #[tokio::test]
    async fn tools_call_ask_user_without_executor_returns_error() {
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
                "id": 12,
                "method": "tools/call",
                "params": { "name": ASK_USER_TOOL_NAME, "arguments": { "questions": [] } }
            }),
        )
        .await;
        assert_eq!(resp["id"], 12);
        assert_eq!(resp["error"]["code"], -32601);
    }
}
