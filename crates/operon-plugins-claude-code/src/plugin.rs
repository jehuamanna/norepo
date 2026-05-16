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
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// One-shot guard so the "shim not built" warning fires at most once
    /// per plugin lifetime instead of on every turn.
    missing_shim_warned: AtomicBool,
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
    /// Extra MCP server entries merged into the per-spawn
    /// `.mcp.json` alongside the permission_bridge's `operon` entry.
    /// Shape: `{"<server_name>": {<server_config>}, ...}` — same
    /// nested-map shape as `.mcp.json`'s `mcpServers` field. Set by
    /// the GUI via [`ClaudeCodeChatPlugin::set_extra_mcp_servers`]
    /// to surface tools from the in-tree operon-bridge (note CRUD,
    /// etc.) in chat-mode claude. `None` = legacy behaviour
    /// (permission_bridge only).
    pub extra_mcp_servers: Option<serde_json::Value>,
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
    /// When `true`, `spawn_turn` adds `--disallowedTools Bash` so
    /// claude's built-in Bash tool is unavailable; the model should
    /// reach for `mcp__operon__operon_bash` instead, which the
    /// bridge advertises in `tools/list` when its shell executor is
    /// installed. Opt-in via the per-repo `AutoApprovePolicy
    /// .bash_via_operon` setting.
    pub bash_via_operon: bool,
    /// Additional directories passed to claude via `--add-dir` on
    /// each `spawn_turn`. Lets the model `Read`/`Edit`/`Write` files
    /// outside the session's `cwd` — primarily the vault's
    /// `notes_dir` so `@[..](note:..)`-referenced notes can be
    /// modified from a project-scoped chat whose cwd is the project
    /// repo, not the vault. Preserved across `cwd` rebinds (the dirs
    /// are orthogonal to which working directory claude runs in).
    pub extra_dirs: Vec<PathBuf>,
}

impl ClaudeCodeChatPlugin {
    pub fn new(cfg: ClaudeCodeConfig) -> Self {
        Self {
            cfg,
            state: Arc::new(Mutex::new(PluginState::default())),
            missing_shim_warned: AtomicBool::new(false),
        }
    }

    /// Returns `true` when the inline-permission-prompt MCP shim is
    /// configured. The UI uses this to surface a one-line notice
    /// explaining why Allow/Deny cards aren't appearing.
    pub fn shim_available(&self) -> bool {
        self.cfg.shim_bin.is_some()
    }

    /// Bind (or rebind) a chat session to a working directory. Subsequent
    /// `complete()` calls whose latest user message carries this session
    /// UUID spawn `claude` with that cwd. If the cwd changed, any cached
    /// `claude_session_id` for the session is cleared so the next turn
    /// starts fresh inside the new directory.
    pub fn bind_session(&self, operon_session: Uuid, cwd: PathBuf) {
        let mut s = self.state.lock().expect("plugin state mutex poisoned");
        let existing = s.bindings.get(&operon_session).cloned();
        let (claude_session_id, permission_mode, bridge, extra_dirs, bash_via_operon) = match existing {
            Some(b) if b.cwd == cwd => (
                b.claude_session_id,
                b.permission_mode,
                b.bridge,
                b.extra_dirs,
                b.bash_via_operon,
            ),
            // cwd changed: invalidate the cached claude session id (it
            // was tied to the previous directory) but keep the
            // permission-mode override, bridge, extra_dirs, and
            // bash_via_operon — those are deliberate caller decisions
            // that don't depend on the working directory.
            Some(b) => (
                None,
                b.permission_mode,
                b.bridge,
                b.extra_dirs,
                b.bash_via_operon,
            ),
            None => (None, None, None, Vec::new(), false),
        };
        s.bindings.insert(
            operon_session,
            SessionBinding {
                cwd,
                claude_session_id,
                permission_mode,
                bridge,
                bash_via_operon,
                extra_dirs,
            },
        );
    }

    /// Toggle the per-session bash-via-operon flag. Reads back in
    /// `spawn_turn` to decide whether to add `--disallowedTools Bash`
    /// to the claude CLI. Defaults to `false`; the shell-side wiring
    /// flips it on after consulting `AutoApprovePolicy.bash_via_operon`.
    /// No-op if `bind_session` hasn't been called for `operon_session`.
    pub fn set_session_bash_via_operon(&self, operon_session: Uuid, enabled: bool) {
        let mut s = self.state.lock().expect("plugin state mutex poisoned");
        if let Some(b) = s.bindings.get_mut(&operon_session) {
            b.bash_via_operon = enabled;
        }
    }

    /// Replace the extra-dirs list for `operon_session`. Companion shell
    /// uses this to pin `<vault>/notes` so referenced-note edits work
    /// in project-scoped chats whose `cwd` is the project repo. The
    /// dirs are emitted as `--add-dir <path>` on each `spawn_turn`.
    /// No-op when the session isn't bound — call `bind_session` first.
    pub fn set_session_extra_dirs(&self, operon_session: Uuid, dirs: Vec<PathBuf>) {
        let mut s = self.state.lock().expect("plugin state mutex poisoned");
        if let Some(b) = s.bindings.get_mut(&operon_session) {
            b.extra_dirs = dirs;
        }
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
    /// Inject extra MCP server entries (e.g. operon-bridge's
    /// `operon_notes`) into the chat-mode spawn's `.mcp.json`. The
    /// value must be a JSON object whose keys are server names and
    /// whose values are MCP server configs — same shape as the
    /// `mcpServers` field of `.mcp.json`. Pass `None` to clear.
    ///
    /// Called once at app boot from
    /// `local_mode::desktop::provide_local_app_signals` after the
    /// bridge runtime is up; per-spawn merging happens in
    /// `spawn_turn` below.
    pub fn set_extra_mcp_servers(&self, extra: Option<serde_json::Value>) {
        let mut s = self.state.lock().expect("plugin state mutex poisoned");
        s.extra_mcp_servers = extra;
    }

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
        let (
            cwd,
            claude_session_id,
            model_override,
            permission_mode,
            bridge,
            bash_via_operon,
            extra_dirs,
        ) = {
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
                binding.bash_via_operon,
                binding.extra_dirs.clone(),
            )
        };

        let mut cmd = tokio::process::Command::new(&self.cfg.claude_bin);
        cmd.current_dir(&cwd)
            .arg("--print")
            .arg("--verbose")
            .arg("--input-format")
            .arg("stream-json")
            .arg("--output-format")
            .arg("stream-json")
            // Surface extended-thinking as it streams. Claude Code v2.1.8+
            // suppresses `thinking` content blocks from default stream-json
            // output (regression vs 2.1.7); the only way to see them now is
            // the `stream_event` envelope this flag enables. Without it the
            // companion would render assistant text only, with no visible
            // reasoning trace.
            .arg("--include-partial-messages")
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
        // Allow tool access to dirs outside cwd. Typically populated
        // with the vault's `notes_dir` by the companion shell so the
        // `Read`/`Edit`/`Write` calls against `@[..](note:..)`
        // references succeed in project-scoped chats whose cwd is the
        // project repo. `--add-dir` accepts a list; one flag per path
        // is the simplest call shape.
        for dir in &extra_dirs {
            cmd.arg("--add-dir").arg(dir);
        }
        // Log the resolved spawn shape once per turn so a hung tool
        // call against a vault note can be diagnosed without
        // attaching a debugger. Visible with `RUST_LOG=operon=info`
        // (or equivalent tracing-subscriber config).
        tracing::info!(
            target: "operon::claude_spawn",
            "spawn cwd={} extra_dirs={:?} perm_mode={:?} bridge={} bash_via_operon={}",
            cwd.display(),
            extra_dirs,
            permission_mode,
            bridge.is_some(),
            bash_via_operon,
        );
        // Compose the `--disallowedTools` argument as a single
        // comma-separated value. Claude's CLI accepts the flag at
        // most once — passing it multiple times silently keeps only
        // the last invocation, so two separate `.arg("--disallowedTools")`
        // calls would let one of the entries through (regression that
        // caused Bash and/or AskUserQuestion to not actually be blocked
        // depending on order).
        //
        // AskUserQuestion is always disabled because the harness
        // intercepts its tool_result frames in non-TUI mode (rewrites
        // host responses to is_error=true, or auto-synthesises empty
        // answers), which hangs the chat surface — see the 2026-05-15
        // spike findings. The custom `mcp__operon__ask_user` MCP tool
        // (advertised by the bridge, executor lives in
        // `operon-dioxus::shell::bridge_ask_user_executor`) replaces
        // it and surfaces a real picker.
        //
        // Bash is additionally disabled under bash_via_operon (Phase
        // 6): forces the model to call `mcp__operon__operon_bash`
        // instead, which streams stdout/stderr live into the chat UI
        // and supports per-tool Cancel.
        let mut disallowed: Vec<&str> = vec!["AskUserQuestion"];
        if bash_via_operon {
            disallowed.push("Bash");
        }
        cmd.arg("--disallowedTools").arg(disallowed.join(","));
        // Steer the model to the replacement tool. Without this hint
        // Claude knows about the harness's AskUserQuestion from its
        // training but won't necessarily reach for the unfamiliar
        // `mcp__operon__ask_user` — it'll fall back to plain-text
        // questions instead, which is fine but loses the picker UX.
        // Kept as a single short paragraph so it's cheap to pay the
        // cache cost on every turn.
        //
        // When the GUI has wired the operon-bridge into chat mode
        // via `set_extra_mcp_servers`, also list the note tools so
        // the model proactively reaches for them instead of asking
        // the user to enumerate / create notes manually. We detect
        // this by inspecting `extra_mcp_servers` — same source the
        // mcp-config merge above reads.
        let extra_has_notes = {
            let s = self.state.lock().expect("plugin state mutex poisoned");
            s.extra_mcp_servers
                .as_ref()
                .and_then(|v| v.as_object())
                .map(|m| m.contains_key("operon_notes"))
                .unwrap_or(false)
        };
        let base_prompt = "When you want to ask the user a clarifying question with structured options, \
             call the `mcp__operon__ask_user` MCP tool. Its input schema mirrors the built-in \
             AskUserQuestion verbatim (a `questions` array of \
             `{question, header, multiSelect, options:[{label, description}]}`), and the \
             response will be `{questions, answers}` where `answers` maps each question to \
             the chosen option label (string for single-select, array for multiSelect). \
             The built-in AskUserQuestion is disabled in this environment — do not call it.";
        let notes_prompt = if extra_has_notes {
            "\n\nOperon notes (the user's vault) are exposed via `mcp__operon_notes__*` tools: \
             `list_projects`, `get_note`, `list_notes`, `list_recent_notes`, `search_notes`, \
             `crawl_note_graph`, `create_note`, `append_note`, `replace_note_range`, \
             `rename_note`, `delete_note`, `reorder_note`, `move_note`, `open_note`, \
             `get_vault_info`, `create_image_note`, `attach_image_to_note`, \
             `list_attachments`, `delete_attachment`. Note bodies referenced in this turn via \
             `@[Title](note:<uuid>)` mentions are already inlined below as \
             `--- referenced note ---` blocks — you do NOT need to call `get_note` for those. \
             \
             Usage hints: \
             - Call `list_projects` FIRST when the user mentions a project by name — every \
               other tool that takes `project_id` needs one of these ids. \
             - Use `search_notes` with `in_content: true` when titles alone wouldn't find a \
               match (e.g. \"find where I wrote about X\"); default title-only is cheaper. \
             - Use `list_recent_notes` for \"what did I work on recently / yesterday / this week\". \
             - Use `crawl_note_graph` (with `direction: out|in|both`) for tree-shaped questions \
               like \"everything connected to this note\" — ONE call with cycle detection \
               instead of looping `get_note` + `search_notes` per link. \
             - `delete_note` ALWAYS shows a confirmation card to the user — it's blocking; \
               just call it and wait. \
             - When the user pastes a screenshot: pick `create_image_note` (standalone vault \
               entry) or `attach_image_to_note` (pinned to an existing note as a sidecar). \
               Pass the bytes base64-encoded as `image_base64` along with `mime_type` \
               (image/png etc.). Both return an `embed_markdown` field you can paste into \
               another note's body via `replace_note_range`. Practical size limit is ~600 KB \
               pre-base64. Use `list_attachments` to see what's pinned to a note; \
               `delete_attachment` to drop one. \
             - `open_note` focuses a note tab in the editor so the user can see what you're \
               talking about — useful after edits."
        } else {
            ""
        };
        cmd.arg("--append-system-prompt").arg(format!("{base_prompt}{notes_prompt}"));

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
        // A bridge was requested but the shim binary isn't on disk:
        // claude will run without `--permission-prompt-tool` and the
        // harness will auto-deny gated tool calls in headless mode.
        // Warn once so the cause is visible in logs instead of
        // silently rejecting `npm i` / Edit / Write.
        if bridge.is_some()
            && self.cfg.shim_bin.is_none()
            && permission_mode.as_deref() != Some("bypassPermissions")
            && !self.missing_shim_warned.swap(true, Ordering::Relaxed)
        {
            tracing::warn!(
                target: "operon::permission",
                "operon-mcp-permission shim not found — claude tool calls (Bash, Edit, Write) \
                 will be auto-denied in 'default'/'plan'/'acceptEdits' modes. \
                 Build the shim with `cargo build -p operon-plugins-claude-code \
                 --bin operon-mcp-permission` or set OPERON_MCP_PERMISSION_BIN."
            );
        }
        // Build the per-spawn `mcpServers` map. Two sources contribute:
        //
        // 1. When `bridge_active`, the permission_bridge's `operon` entry
        //    (shim binary + socket) is added; we also tag the spawn with
        //    `--permission-prompt-tool` so claude routes gated tool calls
        //    through the bridge.
        // 2. The extras the GUI set via `set_extra_mcp_servers` — most
        //    importantly the in-tree `operon_notes` server that fronts
        //    every `mcp__operon_notes__*` tool (create_note, list_notes,
        //    search_notes, …). These are independent of the permission
        //    bridge: in `bypassPermissions` mode, or when the shim binary
        //    is missing, the bridge entry is skipped but the extras still
        //    need to ship — otherwise chat-mode claude can't see any of
        //    the operon note tools and silently falls back to plain Bash.
        let extras_snapshot = {
            let s = self.state.lock().expect("plugin state mutex poisoned");
            s.extra_mcp_servers.clone()
        };
        let mut servers_map = serde_json::Map::new();
        if bridge_active {
            let shim = self.cfg.shim_bin.as_ref().expect("shim_bin");
            let socket = bridge.as_ref().expect("bridge").socket_path().to_path_buf();
            if let Some(bridge_servers) = build_mcp_config(shim, &socket)
                .get("mcpServers")
                .and_then(|v| v.as_object())
            {
                for (k, v) in bridge_servers {
                    servers_map.insert(k.clone(), v.clone());
                }
            }
        }
        if let Some(extra_obj) = extras_snapshot.as_ref().and_then(|v| v.as_object()) {
            for (k, v) in extra_obj {
                servers_map.insert(k.clone(), v.clone());
            }
        }
        if !servers_map.is_empty() {
            match tempfile::Builder::new()
                .prefix("operon-mcp-")
                .suffix(".json")
                .tempfile()
            {
                Ok(mut f) => {
                    let server_names: Vec<String> = servers_map.keys().cloned().collect();
                    let cfg_value = serde_json::json!({
                        "mcpServers": serde_json::Value::Object(servers_map),
                    });
                    let cfg_json = cfg_value.to_string();
                    use std::io::Write;
                    if let Err(e) = f.write_all(cfg_json.as_bytes()) {
                        tracing::warn!(
                            target: "operon::permission",
                            "write mcp config tempfile: {e}; skipping mcp-config wiring"
                        );
                    } else {
                        tracing::info!(
                            target: "operon::claude_spawn",
                            config = %f.path().display(),
                            servers = ?server_names,
                            bridge_active,
                            "wrote per-spawn mcp config"
                        );
                        tracing::debug!(
                            target: "operon::claude_spawn",
                            "mcp config body: {cfg_json}"
                        );
                        cmd.arg("--mcp-config").arg(f.path());
                        if bridge_active {
                            cmd.arg("--permission-prompt-tool")
                                .arg(permission_prompt_tool_arg());
                        }
                        mcp_config_keepalive = Some(f);
                    }
                }
                Err(e) => tracing::warn!(
                    target: "operon::permission",
                    "create mcp config tempfile: {e}; skipping mcp-config wiring"
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
    fn set_session_extra_dirs_replaces_list_and_persists_across_rebind() {
        let plugin = ClaudeCodeChatPlugin::new(ClaudeCodeConfig {
            claude_bin: "/usr/bin/false".into(),
            model: None,
            shim_bin: None,
        });
        let sid = Uuid::new_v4();
        plugin.bind_session(sid, "/tmp/repo-a".into());
        plugin.set_session_extra_dirs(
            sid,
            vec![PathBuf::from("/vault/notes"), PathBuf::from("/extra")],
        );
        {
            let st = plugin.state.lock().unwrap();
            let b = st.bindings.get(&sid).unwrap();
            assert_eq!(b.extra_dirs.len(), 2);
            assert_eq!(b.extra_dirs[0], PathBuf::from("/vault/notes"));
            assert_eq!(b.extra_dirs[1], PathBuf::from("/extra"));
        }

        // Same UUID, new cwd → extra_dirs survives (orthogonal to cwd).
        plugin.bind_session(sid, "/tmp/repo-b".into());
        {
            let st = plugin.state.lock().unwrap();
            let b = st.bindings.get(&sid).unwrap();
            assert_eq!(b.extra_dirs.len(), 2);
        }

        // Replace with a different list — old entries gone.
        plugin.set_session_extra_dirs(sid, vec![PathBuf::from("/new")]);
        {
            let st = plugin.state.lock().unwrap();
            let b = st.bindings.get(&sid).unwrap();
            assert_eq!(b.extra_dirs, vec![PathBuf::from("/new")]);
        }
    }

    #[test]
    fn set_session_extra_dirs_no_op_when_unbound() {
        let plugin = ClaudeCodeChatPlugin::new(ClaudeCodeConfig {
            claude_bin: "/usr/bin/false".into(),
            model: None,
            shim_bin: None,
        });
        let sid = Uuid::new_v4();
        // No bind_session call → setter must NOT create a binding.
        plugin.set_session_extra_dirs(sid, vec![PathBuf::from("/x")]);
        let st = plugin.state.lock().unwrap();
        assert!(st.bindings.get(&sid).is_none());
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
