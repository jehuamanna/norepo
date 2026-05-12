//! ClaudeCodeChatPlugin — drives the `claude` CLI as a subprocess per turn.
//!
//! Each `complete()` spawns one `claude --print --input-format stream-json
//! --output-format stream-json` process bound to whatever cwd was registered
//! for the in-flight Operon session. The session UUID is taken from the most
//! recent `Message.session` in the `ChatRequest` (`AgentRuntime` already
//! threads it through). After a turn finishes, the `session_id` carried in
//! the `result` event is cached against that Operon UUID so the next turn
//! passes `--resume <claude_session_id>` and the conversation continues.
//!
//! Multiple Operon sessions can cohabit a single plugin instance — each gets
//! its own `(cwd, claude_session_id)` binding, so a Project-A chat, a
//! Project-B chat, and a vault-scoped global chat can run in parallel
//! without stepping on each other.

#![cfg(not(target_arch = "wasm32"))]

use async_trait::async_trait;
use futures::channel::mpsc::{self, UnboundedReceiver};
use futures::StreamExt;
use operon_core::error::{OperonError, OperonResult};
use operon_core::traits::{
    Capabilities, ChatDelta, ChatPlugin, ChatRequest, ChatStream, ContentBlock, Message, Plugin,
    Role,
};
use operon_core::traits::CancellationToken;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use uuid::Uuid;

use crate::event::ClaudeCodeEvent;
use crate::permission_bridge::{
    build_mcp_config, permission_prompt_tool_arg, PermissionBridge,
};
use crate::stream::{drive_stream, ClaudeProcess};

#[derive(Clone, Debug)]
pub struct ClaudeCodeConfig {
    /// Absolute path to the `claude` binary. Required because the harness
    /// can't depend on the host PATH being inherited.
    pub claude_bin: PathBuf,
    /// Optional model override (forwarded as `--model`).
    pub model: Option<String>,
    /// Absolute path to the `operon-mcp-permission` shim binary. When
    /// `None`, the inline-permission-prompt wiring is skipped entirely
    /// (claude falls back to whatever its `--permission-mode` says).
    /// Set this once at app startup; the spawn flow reads it on every
    /// turn alongside any session-bound `PermissionBridge`.
    pub shim_bin: Option<PathBuf>,
}

pub struct ClaudeCodeChatPlugin {
    cfg: ClaudeCodeConfig,
    state: Arc<Mutex<PluginState>>,
}

#[derive(Default)]
pub(crate) struct PluginState {
    /// Per-Operon-session bindings. Each entry pins a cwd and (after the
    /// first turn completes) the claude-side session id used to resume
    /// the conversation on subsequent turns.
    pub bindings: BTreeMap<Uuid, SessionBinding>,
    /// Runtime override for the model id forwarded as `--model`. When
    /// `None`, the per-spawn default from `ClaudeCodeConfig.model` is
    /// used (which may itself be `None` to let claude pick).
    pub default_model: Option<String>,
    /// One of `default / acceptEdits / plan / bypassPermissions` (the
    /// values claude's `--permission-mode` accepts) or `None` to omit
    /// the flag and let claude pick its own default. Set from the
    /// companion toolbar's permission picker.
    pub permission_mode: Option<String>,
}

#[derive(Clone)]
pub(crate) struct SessionBinding {
    pub cwd: PathBuf,
    pub claude_session_id: Option<String>,
    /// Optional override of the global `permission_mode` for THIS
    /// session only. Set by callers that need a stricter or looser
    /// policy than the user's companion-toolbar default — e.g. the
    /// artifact runner sets `Some("acceptEdits")` so its automated
    /// Write tool calls don't hang waiting for stdin approval, while
    /// the user's normal companion chats keep using whatever mode
    /// they picked from the toolbar. `None` = fall back to the
    /// global `PluginState.permission_mode`.
    pub permission_mode: Option<String>,
    /// When set + `ClaudeCodeConfig.shim_bin` is also set, the next
    /// `spawn_turn` writes a per-turn MCP config pointing at the shim
    /// binary which proxies stdio to this bridge's socket, and adds
    /// `--permission-prompt-tool mcp__operon__permission_prompt` so
    /// claude routes every gated tool-use through the bridge instead
    /// of silently denying. Skipped when `permission_mode` is
    /// `bypassPermissions` (claude won't ask in that mode anyway).
    pub bridge: Option<Arc<PermissionBridge>>,
}

impl ClaudeCodeChatPlugin {
    pub fn new(cfg: ClaudeCodeConfig) -> Self {
        Self {
            cfg,
            state: Arc::new(Mutex::new(PluginState::default())),
        }
    }

    /// Bind (or rebind) a chat session to a working directory. Subsequent
    /// `complete()` calls whose latest user message carries this session
    /// UUID spawn `claude` with that cwd. If the cwd changed, any cached
    /// `claude_session_id` for the session is cleared so the next turn
    /// starts fresh inside the new directory.
    pub fn bind_session(&self, operon_session: Uuid, cwd: PathBuf) {
        let mut s = self.state.lock().expect("plugin state mutex poisoned");
        let existing = s.bindings.get(&operon_session).cloned();
        let (claude_session_id, permission_mode, bridge) = match existing {
            Some(b) if b.cwd == cwd => (b.claude_session_id, b.permission_mode, b.bridge),
            // cwd changed: invalidate the cached claude session id (it
            // was tied to the previous directory) but keep the
            // permission-mode override + bridge — those are deliberate
            // caller decisions that don't depend on the working
            // directory.
            Some(b) => (None, b.permission_mode, b.bridge),
            None => (None, None, None),
        };
        s.bindings.insert(
            operon_session,
            SessionBinding {
                cwd,
                claude_session_id,
                permission_mode,
                bridge,
            },
        );
    }

    /// Drop a session's binding. After this, `complete()` calls referencing
    /// this UUID error out with "session not bound".
    pub fn unbind_session(&self, operon_session: Uuid) {
        let mut s = self.state.lock().expect("plugin state mutex poisoned");
        s.bindings.remove(&operon_session);
    }

    /// Currently-cached claude session id for an Operon session, if any.
    /// Useful for diagnostics and persisting back to the `chat_session`
    /// table on app shutdown.
    pub fn current_claude_session(&self, operon_session: Uuid) -> Option<String> {
        self.state
            .lock()
            .ok()
            .and_then(|s| s.bindings.get(&operon_session).and_then(|b| b.claude_session_id.clone()))
    }

    /// Pre-seed the plugin with a known claude_session_id (e.g., loaded from
    /// SQLite at app startup so an existing session resumes on first turn).
    /// Requires that `bind_session` has already set the cwd.
    pub fn restore_claude_session(&self, operon_session: Uuid, claude_session_id: String) {
        let mut s = self.state.lock().expect("plugin state mutex poisoned");
        if let Some(b) = s.bindings.get_mut(&operon_session) {
            b.claude_session_id = Some(claude_session_id);
        }
    }

    /// Override the model id forwarded as `--model` on subsequent turns.
    /// Pass `None` to fall back to the `ClaudeCodeConfig.model` default.
    pub fn set_default_model(&self, model: Option<String>) {
        let mut s = self.state.lock().expect("plugin state mutex poisoned");
        s.default_model = model;
    }

    /// Set the value passed to `claude --permission-mode`. Pass `None`
    /// to omit the flag entirely (claude picks its own default —
    /// usually "default", which auto-approves in --print mode).
    pub fn set_permission_mode(&self, mode: Option<String>) {
        let mut s = self.state.lock().expect("plugin state mutex poisoned");
        s.permission_mode = mode;
    }

    /// Set a per-session permission-mode override. The value passed
    /// here is preferred over the global `set_permission_mode` value
    /// when spawning subsequent turns for `operon_session`. Pass
    /// `None` to clear the override and fall back to the global
    /// state. No-op if `bind_session` hasn't been called for the
    /// session id yet.
    pub fn set_session_permission_mode(&self, operon_session: Uuid, mode: Option<String>) {
        let mut s = self.state.lock().expect("plugin state mutex poisoned");
        if let Some(b) = s.bindings.get_mut(&operon_session) {
            b.permission_mode = mode;
        }
    }

    /// Attach (or detach) a `PermissionBridge` to a session. While set,
    /// `spawn_turn` writes a per-turn MCP config pointing at
    /// `cfg.shim_bin` and adds `--permission-prompt-tool` so claude
    /// routes gated tool-uses through the bridge instead of silently
    /// failing in `--print` mode. No-op if `bind_session` hasn't run
    /// yet — call `bind_session` first, then this.
    pub fn set_session_bridge(
        &self,
        operon_session: Uuid,
        bridge: Option<Arc<PermissionBridge>>,
    ) {
        let mut s = self.state.lock().expect("plugin state mutex poisoned");
        if let Some(b) = s.bindings.get_mut(&operon_session) {
            b.bridge = bridge;
        }
    }

    pub fn current_default_model(&self) -> Option<String> {
        self.state.lock().ok().and_then(|s| s.default_model.clone())
    }

    pub fn current_permission_mode(&self) -> Option<String> {
        self.state
            .lock()
            .ok()
            .and_then(|s| s.permission_mode.clone())
    }

    /// Send a single user prompt for `operon_session` and return a stream
    /// of rich `ClaudeCodeEvent`s. Bypasses the lossy `ChatPlugin::complete`
    /// adapter so callers (the companion pane) can render tool-use cards,
    /// thinking blocks, and per-turn usage without an intermediate
    /// translation. Cancellation kills the spawned subprocess.
    pub async fn send_rich(
        &self,
        prompt: String,
        operon_session: Uuid,
        ct: CancellationToken,
    ) -> OperonResult<UnboundedReceiver<ClaudeCodeEvent>> {
        self.spawn_turn(prompt, operon_session, ct)
    }
}

#[async_trait]
impl Plugin for ClaudeCodeChatPlugin {
    fn name(&self) -> &str {
        "claude-code"
    }
    fn version(&self) -> &str {
        "0.2.0"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::STREAMING | Capabilities::TOOL_USE | Capabilities::PROMPT_CACHE
    }
}

#[async_trait]
impl ChatPlugin for ClaudeCodeChatPlugin {
    async fn complete(&self, req: ChatRequest, ct: CancellationToken) -> OperonResult<ChatStream> {
        let operon_session = extract_session_uuid(&req).ok_or_else(|| OperonError::Provider {
            provider: "claude-code".into(),
            message: "request has no user message; cannot derive session id".into(),
            retryable: false,
        })?;
        let text = extract_user_text(&req).ok_or_else(|| OperonError::Provider {
            provider: "claude-code".into(),
            message: "no user message text in request".into(),
            retryable: false,
        })?;
        let rich_rx = self.spawn_turn(text, operon_session, ct)?;
        // Adapt rich events back to ChatDelta for trait callers. Tool
        // results, thinking blocks, and protocol errors collapse into the
        // text-only stream — callers that need the full picture should use
        // `send_rich` instead.
        let stream = rich_rx.filter_map(|ev| async move {
            match ev {
                ClaudeCodeEvent::Text(t) => Some(Ok(ChatDelta::Text(t))),
                ClaudeCodeEvent::ToolUse { id, name, input } => {
                    Some(Ok(ChatDelta::ToolUse { id, name, input }))
                }
                ClaudeCodeEvent::Done { stop_reason, usage } => Some(Ok(ChatDelta::Stop {
                    reason: stop_reason,
                    usage,
                })),
                ClaudeCodeEvent::Error(msg) => Some(Err(OperonError::Provider {
                    provider: "claude-code".into(),
                    message: msg,
                    retryable: false,
                })),
                ClaudeCodeEvent::Thinking(_)
                | ClaudeCodeEvent::ToolResult { .. }
                | ClaudeCodeEvent::SessionInit { .. } => None,
            }
        });
        Ok(Box::pin(stream))
    }
}

impl ClaudeCodeChatPlugin {
    /// Spawn one `claude --print` turn for `operon_session` and return the
    /// rich event channel. Internal — `send_rich` is the public entry
    /// point and `complete` adapts this into a `ChatStream`.
    fn spawn_turn(
        &self,
        prompt: String,
        operon_session: Uuid,
        ct: CancellationToken,
    ) -> OperonResult<UnboundedReceiver<ClaudeCodeEvent>> {
        let (cwd, claude_session_id, model_override, permission_mode, bridge) = {
            let s = self.state.lock().expect("plugin state mutex poisoned");
            let binding = s.bindings.get(&operon_session).ok_or_else(|| {
                OperonError::Provider {
                    provider: "claude-code".into(),
                    message: format!(
                        "session {operon_session} is not bound to a repository; \
                         call ClaudeCodeChatPlugin::bind_session before chatting"
                    ),
                    retryable: false,
                }
            })?;
            // Per-session permission_mode override wins over the
            // global one. Lets the artifact runner force
            // `acceptEdits` for its automated runs while normal
            // companion chats keep using whatever mode the user
            // picked from the toolbar.
            let effective_mode = binding
                .permission_mode
                .clone()
                .or_else(|| s.permission_mode.clone());
            (
                binding.cwd.clone(),
                binding.claude_session_id.clone(),
                s.default_model.clone(),
                effective_mode,
                binding.bridge.clone(),
            )
        };

        let mut cmd = tokio::process::Command::new(&self.cfg.claude_bin);
        cmd.current_dir(&cwd)
            .arg("--print")
            .arg("--input-format")
            .arg("stream-json")
            .arg("--output-format")
            .arg("stream-json")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(sid) = &claude_session_id {
            cmd.arg("--resume").arg(sid);
        }
        // Per-call model override → static cfg fallback → claude's own
        // default. Permission mode is per-session-override-then-global
        // (see effective_mode resolution above): the artifact runner's
        // session uses `acceptEdits` even when the toolbar default is
        // `default`, so automated Write tool calls don't hang waiting
        // for stdin approval.
        let effective_model = model_override.or_else(|| self.cfg.model.clone());
        if let Some(model) = &effective_model {
            cmd.arg("--model").arg(model);
        }
        if let Some(mode) = &permission_mode {
            cmd.arg("--permission-mode").arg(mode);
        }

        // Wire the inline-permission-prompt MCP tool when a session has
        // a `PermissionBridge` attached AND the runtime knows where the
        // shim binary lives AND we're not in `bypassPermissions` mode
        // (claude won't ask in that mode anyway). The tempfile holding
        // the generated MCP config is kept alive in the spawned task
        // below so it outlives claude's startup read.
        let mut mcp_config_keepalive: Option<tempfile::NamedTempFile> = None;
        let bridge_active = bridge.is_some()
            && self.cfg.shim_bin.is_some()
            && permission_mode.as_deref() != Some("bypassPermissions");
        if bridge_active {
            // Both checked above — unwraps are infallible here.
            let shim = self.cfg.shim_bin.as_ref().expect("shim_bin");
            let socket = bridge.as_ref().expect("bridge").socket_path().to_path_buf();
            match tempfile::Builder::new()
                .prefix("operon-mcp-")
                .suffix(".json")
                .tempfile()
            {
                Ok(mut f) => {
                    let cfg_json = build_mcp_config(shim, &socket).to_string();
                    use std::io::Write;
                    if let Err(e) = f.write_all(cfg_json.as_bytes()) {
                        tracing::warn!(
                            target: "operon::permission",
                            "write mcp config tempfile: {e}; skipping prompt-tool wiring"
                        );
                    } else {
                        cmd.arg("--mcp-config").arg(f.path());
                        cmd.arg("--permission-prompt-tool")
                            .arg(permission_prompt_tool_arg());
                        mcp_config_keepalive = Some(f);
                    }
                }
                Err(e) => tracing::warn!(
                    target: "operon::permission",
                    "create mcp config tempfile: {e}; skipping prompt-tool wiring"
                ),
            }
        }

        let mut child = cmd.spawn().map_err(|e| OperonError::Provider {
            provider: "claude-code".into(),
            message: format!("spawn {:?}: {e}", self.cfg.claude_bin),
            retryable: false,
        })?;

        let mut stdin = child.stdin.take().ok_or_else(|| OperonError::Provider {
            provider: "claude-code".into(),
            message: "child stdin missing".into(),
            retryable: false,
        })?;
        let stdout = child.stdout.take().ok_or_else(|| OperonError::Provider {
            provider: "claude-code".into(),
            message: "child stdout missing".into(),
            retryable: false,
        })?;
        let stderr = child.stderr.take();

        let frame = serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": prompt },
        });
        let frame_line = format!("{}\n", frame);

        // Spawn the writer + reader in one task so this method can return
        // the receiver synchronously (no .await on stdin write here).
        let (tx, rx) = mpsc::unbounded::<ClaudeCodeEvent>();
        let stdout_reader = BufReader::new(stdout).lines();
        let stderr_reader = stderr.map(|e| BufReader::new(e).lines());
        let proc = ClaudeProcess {
            child,
            stdout: stdout_reader,
            stderr: stderr_reader,
        };
        let state = self.state.clone();
        let claude_bin_diag = self.cfg.claude_bin.clone();
        tokio::spawn(async move {
            // Hold the MCP config tempfile alive until claude exits.
            // Dropping it inside the task (rather than after `cmd.spawn`)
            // avoids a TOCTOU where claude tries to read the path after
            // the file is unlinked.
            let _mcp_config_keepalive = mcp_config_keepalive;
            // Push the user frame, then close stdin so claude --print emits
            // its result and exits.
            if let Err(e) = stdin.write_all(frame_line.as_bytes()).await {
                let _ = tx.unbounded_send(ClaudeCodeEvent::Error(format!(
                    "write stdin to {claude_bin_diag:?}: {e}"
                )));
                return;
            }
            stdin.flush().await.ok();
            drop(stdin);
            drive_stream(proc, tx, ct, state, operon_session).await;
        });
        Ok(rx)
    }
}

fn extract_session_uuid(req: &ChatRequest) -> Option<Uuid> {
    // Walk back through messages and pick the freshest user-role session id.
    // Falls back to the freshest message of any role if no user message is
    // present (matches `extract_user_text`'s tolerant behaviour).
    req.messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, Role::User))
        .map(|m| m.session)
        .or_else(|| req.messages.last().map(|m| m.session))
}

fn extract_user_text(req: &ChatRequest) -> Option<String> {
    req.messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, Role::User))
        .and_then(message_to_text)
        .or_else(|| {
            let mut acc = String::new();
            for m in &req.messages {
                if let Some(t) = message_to_text(m) {
                    if !acc.is_empty() {
                        acc.push('\n');
                    }
                    acc.push_str(&t);
                }
            }
            if acc.is_empty() {
                None
            } else {
                Some(acc)
            }
        })
}

fn message_to_text(m: &Message) -> Option<String> {
    let mut acc = String::new();
    for block in &m.content {
        if let ContentBlock::Text(t) = block {
            if !acc.is_empty() {
                acc.push('\n');
            }
            acc.push_str(t);
        }
    }
    if acc.is_empty() {
        None
    } else {
        Some(acc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use operon_core::traits::{ContentBlock, Message, Role};
    use std::collections::HashMap;

    fn user_msg(text: &str, session: Uuid) -> Message {
        Message {
            id: Uuid::new_v4(),
            role: Role::User,
            content: vec![ContentBlock::Text(text.into())],
            created_at_ms: 0,
            session,
            metadata: HashMap::new(),
        }
    }

    fn asst_msg(text: &str, session: Uuid) -> Message {
        Message {
            id: Uuid::new_v4(),
            role: Role::Assistant,
            content: vec![ContentBlock::Text(text.into())],
            created_at_ms: 0,
            session,
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn extract_session_uuid_prefers_latest_user_message() {
        let s_first = Uuid::new_v4();
        let s_last = Uuid::new_v4();
        let req = ChatRequest {
            system: None,
            messages: vec![
                user_msg("first", s_first),
                asst_msg("ack", s_first),
                user_msg("second", s_last),
            ],
            tools: vec![],
            model: None,
            max_tokens: None,
        };
        assert_eq!(extract_session_uuid(&req), Some(s_last));
    }

    #[test]
    fn extract_user_text_returns_last_user_message_text() {
        let s = Uuid::new_v4();
        let req = ChatRequest {
            system: None,
            messages: vec![
                user_msg("first", s),
                asst_msg("ack", s),
                user_msg("second", s),
            ],
            tools: vec![],
            model: None,
            max_tokens: None,
        };
        assert_eq!(extract_user_text(&req), Some("second".to_string()));
    }

    #[test]
    fn bind_session_stores_cwd_with_no_session_id() {
        let plugin = ClaudeCodeChatPlugin::new(ClaudeCodeConfig {
            claude_bin: "/usr/bin/false".into(),
            model: None,
            shim_bin: None,
        });
        let sid = Uuid::new_v4();
        plugin.bind_session(sid, "/tmp/repo-a".into());
        let st = plugin.state.lock().unwrap();
        let b = st.bindings.get(&sid).expect("binding present");
        assert_eq!(b.cwd, PathBuf::from("/tmp/repo-a"));
        assert!(b.claude_session_id.is_none());
    }

    #[test]
    fn bind_session_with_new_cwd_clears_cached_claude_session_id() {
        let plugin = ClaudeCodeChatPlugin::new(ClaudeCodeConfig {
            claude_bin: "/usr/bin/false".into(),
            model: None,
            shim_bin: None,
        });
        let sid = Uuid::new_v4();
        plugin.bind_session(sid, "/tmp/repo-a".into());
        plugin.restore_claude_session(sid, "claude-session-A".into());
        assert_eq!(
            plugin.current_claude_session(sid).as_deref(),
            Some("claude-session-A")
        );

        // Same UUID, different cwd → cached id must reset.
        plugin.bind_session(sid, "/tmp/repo-b".into());
        assert!(plugin.current_claude_session(sid).is_none());
    }

    #[test]
    fn bind_session_with_same_cwd_preserves_cached_claude_session_id() {
        let plugin = ClaudeCodeChatPlugin::new(ClaudeCodeConfig {
            claude_bin: "/usr/bin/false".into(),
            model: None,
            shim_bin: None,
        });
        let sid = Uuid::new_v4();
        plugin.bind_session(sid, "/tmp/repo-a".into());
        plugin.restore_claude_session(sid, "claude-session-A".into());

        // No-op rebind (same cwd) leaves the session id intact.
        plugin.bind_session(sid, "/tmp/repo-a".into());
        assert_eq!(
            plugin.current_claude_session(sid).as_deref(),
            Some("claude-session-A")
        );
    }

    #[test]
    fn set_session_permission_mode_overrides_global() {
        let plugin = ClaudeCodeChatPlugin::new(ClaudeCodeConfig {
            claude_bin: "/usr/bin/false".into(),
            model: None,
            shim_bin: None,
        });
        let sid = Uuid::new_v4();
        plugin.set_permission_mode(Some("default".into()));
        plugin.bind_session(sid, "/tmp/repo".into());
        plugin.set_session_permission_mode(sid, Some("acceptEdits".into()));
        // The per-session value should win when reading the binding.
        let st = plugin.state.lock().unwrap();
        assert_eq!(
            st.bindings.get(&sid).and_then(|b| b.permission_mode.as_deref()),
            Some("acceptEdits")
        );
        assert_eq!(st.permission_mode.as_deref(), Some("default"));
    }

    #[test]
    fn set_session_permission_mode_falls_back_to_global_when_cleared() {
        let plugin = ClaudeCodeChatPlugin::new(ClaudeCodeConfig {
            claude_bin: "/usr/bin/false".into(),
            model: None,
            shim_bin: None,
        });
        let sid = Uuid::new_v4();
        plugin.set_permission_mode(Some("default".into()));
        plugin.bind_session(sid, "/tmp/repo".into());
        plugin.set_session_permission_mode(sid, Some("acceptEdits".into()));
        plugin.set_session_permission_mode(sid, None);
        let st = plugin.state.lock().unwrap();
        assert!(
            st.bindings.get(&sid).and_then(|b| b.permission_mode.as_ref()).is_none(),
            "override should be cleared so the spawn picks up the global value"
        );
    }

    #[test]
    fn bind_session_with_new_cwd_preserves_permission_override() {
        let plugin = ClaudeCodeChatPlugin::new(ClaudeCodeConfig {
            claude_bin: "/usr/bin/false".into(),
            model: None,
            shim_bin: None,
        });
        let sid = Uuid::new_v4();
        plugin.bind_session(sid, "/tmp/repo-a".into());
        plugin.set_session_permission_mode(sid, Some("acceptEdits".into()));
        // Re-bind with a different cwd — the cached claude_session_id
        // resets but the permission override is the caller's
        // intent, unrelated to working directory.
        plugin.bind_session(sid, "/tmp/repo-b".into());
        let st = plugin.state.lock().unwrap();
        assert_eq!(
            st.bindings.get(&sid).and_then(|b| b.permission_mode.as_deref()),
            Some("acceptEdits")
        );
    }

    #[test]
    fn unbind_session_drops_the_binding() {
        let plugin = ClaudeCodeChatPlugin::new(ClaudeCodeConfig {
            claude_bin: "/usr/bin/false".into(),
            model: None,
            shim_bin: None,
        });
        let sid = Uuid::new_v4();
        plugin.bind_session(sid, "/tmp/repo".into());
        plugin.unbind_session(sid);
        assert!(plugin.current_claude_session(sid).is_none());
        assert!(plugin.state.lock().unwrap().bindings.get(&sid).is_none());
    }

    #[tokio::test]
    async fn set_session_bridge_attaches_and_clears() {
        use crate::permission_bridge::{PermissionBridge, PermissionDecision, PermissionRequest};
        use tempfile::tempdir;
        use tokio::sync::oneshot;

        let plugin = ClaudeCodeChatPlugin::new(ClaudeCodeConfig {
            claude_bin: "/usr/bin/false".into(),
            model: None,
            shim_bin: None,
        });
        let sid = Uuid::new_v4();
        plugin.bind_session(sid, "/tmp/repo".into());

        let dir = tempdir().unwrap();
        let sock = dir.path().join("perm.sock");
        let handler = |_req: PermissionRequest, respond: oneshot::Sender<PermissionDecision>| {
            let _ = respond.send(PermissionDecision::Allow { updated_input: None });
        };
        let bridge = Arc::new(
            PermissionBridge::bind(sock, handler).await.unwrap(),
        );
        plugin.set_session_bridge(sid, Some(bridge));
        assert!(plugin
            .state
            .lock()
            .unwrap()
            .bindings
            .get(&sid)
            .and_then(|b| b.bridge.as_ref())
            .is_some());

        plugin.set_session_bridge(sid, None);
        assert!(plugin
            .state
            .lock()
            .unwrap()
            .bindings
            .get(&sid)
            .and_then(|b| b.bridge.as_ref())
            .is_none());
    }

    #[tokio::test]
    async fn bind_session_with_new_cwd_preserves_bridge() {
        use crate::permission_bridge::{PermissionBridge, PermissionDecision, PermissionRequest};
        use tempfile::tempdir;
        use tokio::sync::oneshot;

        let plugin = ClaudeCodeChatPlugin::new(ClaudeCodeConfig {
            claude_bin: "/usr/bin/false".into(),
            model: None,
            shim_bin: None,
        });
        let sid = Uuid::new_v4();
        plugin.bind_session(sid, "/tmp/repo-a".into());

        let dir = tempdir().unwrap();
        let sock = dir.path().join("perm.sock");
        let handler = |_req: PermissionRequest, respond: oneshot::Sender<PermissionDecision>| {
            let _ = respond.send(PermissionDecision::Allow { updated_input: None });
        };
        let bridge = Arc::new(
            PermissionBridge::bind(sock, handler).await.unwrap(),
        );
        plugin.set_session_bridge(sid, Some(bridge));

        // Re-bind with a different cwd. The cached claude_session_id
        // gets cleared but the bridge — like permission_mode — is a
        // caller intent unrelated to the working directory.
        plugin.bind_session(sid, "/tmp/repo-b".into());
        assert!(plugin
            .state
            .lock()
            .unwrap()
            .bindings
            .get(&sid)
            .and_then(|b| b.bridge.as_ref())
            .is_some());
    }

    #[test]
    fn parallel_sessions_have_isolated_state() {
        let plugin = ClaudeCodeChatPlugin::new(ClaudeCodeConfig {
            claude_bin: "/usr/bin/false".into(),
            model: None,
            shim_bin: None,
        });
        let s_a = Uuid::new_v4();
        let s_b = Uuid::new_v4();
        plugin.bind_session(s_a, "/tmp/proj-a".into());
        plugin.bind_session(s_b, "/tmp/proj-b".into());
        plugin.restore_claude_session(s_a, "claude-A".into());
        plugin.restore_claude_session(s_b, "claude-B".into());
        assert_eq!(plugin.current_claude_session(s_a).as_deref(), Some("claude-A"));
        assert_eq!(plugin.current_claude_session(s_b).as_deref(), Some("claude-B"));

        // Rebinding A's cwd doesn't disturb B.
        plugin.bind_session(s_a, "/tmp/proj-a-renamed".into());
        assert!(plugin.current_claude_session(s_a).is_none());
        assert_eq!(plugin.current_claude_session(s_b).as_deref(), Some("claude-B"));
    }
}
