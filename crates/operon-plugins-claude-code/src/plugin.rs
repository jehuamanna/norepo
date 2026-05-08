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
use crate::stream::{drive_stream, ClaudeProcess};

#[derive(Clone, Debug)]
pub struct ClaudeCodeConfig {
    /// Absolute path to the `claude` binary. Required because the harness
    /// can't depend on the host PATH being inherited.
    pub claude_bin: PathBuf,
    /// Optional model override (forwarded as `--model`).
    pub model: Option<String>,
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
    /// When true, every `claude` spawn passes `--permission-mode=plan`
    /// so the assistant produces a plan before any tool use. Toggled
    /// from the companion toolbar.
    pub plan_mode: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct SessionBinding {
    pub cwd: PathBuf,
    pub claude_session_id: Option<String>,
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
        let claude_session_id = match existing {
            Some(b) if b.cwd == cwd => b.claude_session_id,
            _ => None,
        };
        s.bindings.insert(
            operon_session,
            SessionBinding {
                cwd,
                claude_session_id,
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

    /// Toggle plan mode for subsequent turns. When `true`, `claude` is
    /// spawned with `--permission-mode=plan`; the assistant emits a
    /// plan before any tool use and asks for approval to execute it.
    pub fn set_plan_mode(&self, on: bool) {
        let mut s = self.state.lock().expect("plugin state mutex poisoned");
        s.plan_mode = on;
    }

    pub fn current_default_model(&self) -> Option<String> {
        self.state.lock().ok().and_then(|s| s.default_model.clone())
    }

    pub fn current_plan_mode(&self) -> bool {
        self.state.lock().map(|s| s.plan_mode).unwrap_or(false)
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
                ClaudeCodeEvent::Thinking(_) | ClaudeCodeEvent::ToolResult { .. } => None,
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
        let (cwd, claude_session_id, model_override, plan_on) = {
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
            (
                binding.cwd.clone(),
                binding.claude_session_id.clone(),
                s.default_model.clone(),
                s.plan_mode,
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
        // default. Plan mode is global (PluginState) so toggling from
        // the toolbar affects every session.
        let effective_model = model_override.or_else(|| self.cfg.model.clone());
        if let Some(model) = &effective_model {
            cmd.arg("--model").arg(model);
        }
        if plan_on {
            cmd.arg("--permission-mode").arg("plan");
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
    fn unbind_session_drops_the_binding() {
        let plugin = ClaudeCodeChatPlugin::new(ClaudeCodeConfig {
            claude_bin: "/usr/bin/false".into(),
            model: None,
        });
        let sid = Uuid::new_v4();
        plugin.bind_session(sid, "/tmp/repo".into());
        plugin.unbind_session(sid);
        assert!(plugin.current_claude_session(sid).is_none());
        assert!(plugin.state.lock().unwrap().bindings.get(&sid).is_none());
    }

    #[test]
    fn parallel_sessions_have_isolated_state() {
        let plugin = ClaudeCodeChatPlugin::new(ClaudeCodeConfig {
            claude_bin: "/usr/bin/false".into(),
            model: None,
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
