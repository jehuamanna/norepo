//! Companion chat surface — rail + chat + tool-card-aware transcript
//! (M1.5b).
//!
//! Wires the Claude Code CLI (`ClaudeCodeChatPlugin`) into a multi-session
//! chat UI inside the companion area. The left rail (`SessionRail`) is the
//! scope-tab + per-scope session list. The right side renders one
//! `TranscriptItem` per visual block: user bubbles, markdown-rendered
//! assistant text, dim italic thinking blocks, collapsible tool-use
//! cards, and a footer cost meter.
//!
//! Per-turn behaviour:
//!   - `ClaudeCodeChatPlugin::send_rich(prompt, session, ct)` returns a
//!     stream of `ClaudeCodeEvent`s. The companion subscribes directly —
//!     no `AgentRuntime` adapter — so tool_use / tool_result / thinking /
//!     usage events all reach the UI verbatim.
//!   - `--resume <session_id>` is reused across turns inside one Operon
//!     session via the per-Uuid binding map in the plugin.
//!   - Switching sessions resets the in-memory transcript (persistent
//!     replay is task #12, deferred).
//!
//! Deferred to follow-ups: persistent transcript reload (M1.5b task #12),
//! composer affordances (M1.5c), model picker + plan mode (M1.5d),
//! permission prompts (M1.5e).

use dioxus::prelude::*;
use operon_core::traits::Usage;
use operon_core::agent_event::{AgentBackend, AgentEvent};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

use crate::agent::plugins::{ClaudeCodeChatPlugin, ClaudeCodeConfig};
use crate::agent::CancellationToken;
use crate::local_mode::desktop::{CurrentVaultRoot, LocalProjectRepo};
use crate::local_mode::explorer::LocalProjectVersion;
use crate::plugins::markdown::MarkdownView;
use crate::shell::companion_state::{
    take_permission_responder, ActiveChatScope, ActiveChatSession, ActiveRepoPath, ChatMessage,
    ChatMessageKind, ArtifactRunState, ChatMessageRepo, ChatScope, ChatSessionRepo,
    CompanionComposerInbox, PermissionStatus, ARTIFACT_RUN_STATE, CHAT_MESSAGE_VERSION,
    INPROGRESS_ASSISTANT, PERMISSION_DECISIONS, PERMISSION_PROMPTS,
};
use crate::shell::session_rail::SessionRail;
use crate::shell::splitter::RailSplitter;
use crate::shell::tool_card::{ToolCard, ToolResultBody};

/// One visible entry in the chat transcript. `AssistantText` holds an
/// accumulating markdown body that grows as text deltas arrive; tool
/// cards correlate `ToolResult` events back to their originating
/// `ToolUse` by id.
#[derive(Clone, Debug, PartialEq)]
pub enum TranscriptItem {
    UserText(String),
    AssistantText(String),
    Thinking(String),
    ToolCall {
        id: String,
        name: String,
        input: Value,
        result: Option<ToolResultBody>,
    },
    System(String),
    /// Inline permission prompt — the spawned claude asked to run a
    /// gated tool (typically Bash) and we surface Allow / Allow Always
    /// / Deny buttons. Status lives in the global
    /// `PERMISSION_DECISIONS` map keyed by `id`; the responder
    /// `oneshot::Sender` lives in `take_permission_responder` keyed
    /// the same way. The variant only stores display data so it
    /// derives Clone+PartialEq cleanly.
    PermissionRequest {
        id: String,
        tool_name: String,
        input: Value,
    },
}

/// Common slash commands surfaced in the composer's "/" popover. The
/// list is intentionally short — claude maintains its own dynamic
/// per-install set, but this v1 just gets the user to *the most
/// common ones* without typing. Selecting a command replaces the
/// composer text wholesale; the user clicks Send to dispatch it.
const SLASH_COMMANDS: &[&str] = &[
    "/help",
    "/clear",
    "/compact",
    "/cost",
    "/context",
    "/model",
    "/login",
];

/// Resolve the path of the `claude` binary at startup. Tries, in order:
/// 1. `OPERON_CLAUDE_BIN` env override.
/// 2. `~/.local/bin/claude` — the standalone installer's standard location.
///    Uses `is_file()` (not `exists()`-style) so a broken symlink falls
///    through to the next candidate instead of being returned as-is.
/// 3. Bare `"claude"` — relies on PATH, which Dioxus desktop spawns
///    inherit from the parent shell.
///
/// Public so `provide_local_app_signals` can construct the shared
/// `ClaudeCodeChatPlugin` instance once at App scope.
pub fn resolve_claude_bin() -> PathBuf {
    if let Ok(p) = std::env::var("OPERON_CLAUDE_BIN") {
        return PathBuf::from(p);
    }
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let local = home.join(".local/bin/claude");
    if local.is_file() {
        return local;
    }
    PathBuf::from("claude")
}

/// Locate the `operon-mcp-permission` shim that claude spawns to bridge
/// the inline-permission-prompt MCP server. Tries, in order:
/// 1. `OPERON_MCP_PERMISSION_BIN` env override.
/// 2. Sibling of `current_exe()` — the cargo `target/debug` layout puts
///    workspace bins next to each other, so `target/debug/operon-dioxus`
///    can find `target/debug/operon-mcp-permission` this way.
/// 3. `dx serve` layout: the running exe lives under
///    `<workspace>/target/dx/<…>/app/`, but workspace bins still build
///    into `<workspace>/target/debug/`. Walk up to the first ancestor
///    named `target` and probe `target/debug/operon-mcp-permission`.
///
/// Returns `None` if no candidate is a regular file; callers then skip
/// the inline-prompt wiring and fall back to the existing
/// `--permission-mode` behavior.
pub fn resolve_mcp_permission_shim() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("OPERON_MCP_PERMISSION_BIN") {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    let exe = std::env::current_exe().ok()?;
    resolve_shim_from_exe(&exe)
}

/// Path-walking half of `resolve_mcp_permission_shim`, extracted so
/// tests can drive it without overriding `current_exe()`.
fn resolve_shim_from_exe(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    let sibling = dir.join("operon-mcp-permission");
    if sibling.is_file() {
        return Some(sibling);
    }
    for ancestor in dir.ancestors() {
        if ancestor.file_name().and_then(|n| n.to_str()) == Some("target") {
            let probe = ancestor.join("debug").join("operon-mcp-permission");
            if probe.is_file() {
                return Some(probe);
            }
            break;
        }
    }
    None
}

#[component]
pub fn CompanionChat() -> Element {
    // Pull the shared plugin from context (provided in
    // `provide_local_app_signals`). Falls back to a fresh local instance
    // for robustness if the context is missing — but in practice the
    // companion always mounts under the local-mode app root.
    let plugin: Signal<Arc<ClaudeCodeChatPlugin>> = use_signal(|| {
        match try_consume_context::<crate::shell::companion_state::ClaudeCodePluginCtx>() {
            Some(crate::shell::companion_state::ClaudeCodePluginCtx(p)) => p,
            None => Arc::new(ClaudeCodeChatPlugin::new(ClaudeCodeConfig {
                claude_bin: resolve_claude_bin(),
                model: None,
                shim_bin: resolve_mcp_permission_shim(),
            })),
        }
    });

    // UI mirror of the plugin's current permission mode. The plugin's
    // internal `Mutex<PluginState>` is the source of truth for
    // `spawn_turn`, but a plain Mutex behind an `Arc` doesn't give
    // Dioxus anything to subscribe to, so the picker's onchange wouldn't
    // re-render the `shim_missing_notice` block (or any other reader)
    // when the user switches modes. Mirror the value into a Signal here
    // and update both in the onchange handler.
    let mut perm_mode: Signal<Option<String>> =
        use_signal(|| plugin.read().current_permission_mode());

    // Slice A14 cutover: route the chat send/bind path through the
    // backend-agnostic `AgentBackend` trait so the picker can swap
    // between claude-code and the in-process runtime. The Signal lives
    // in `AgentBackendCtx` (provided by `desktop.rs`) so flipping the
    // picker is reactive — the next bind / send routes to the new
    // backend. The concrete `plugin` signal above is retained for
    // `ensure_session_bridge`, which is claude-code-specific and only
    // fires when the active backend is claude-code.
    let active_backend: Signal<Arc<dyn AgentBackend>> =
        match try_consume_context::<crate::shell::companion_state::AgentBackendCtx>() {
            Some(crate::shell::companion_state::AgentBackendCtx(s)) => s,
            None => {
                // Fall back to the concrete claude-code plugin coerced to
                // the trait — same shape as production.
                let fallback: Arc<dyn AgentBackend> = plugin.read().clone();
                use_signal(|| fallback)
            }
        };
    // Available backends — populated when running under the desktop app
    // (where `desktop.rs` provides `BackendsCtx`). Tests/standalone usages
    // may not have it; in that case we hide the picker.
    let backends_ctx: Option<crate::shell::companion_state::BackendsCtx> =
        try_consume_context::<crate::shell::companion_state::BackendsCtx>();

    let transcript = use_signal::<Vec<TranscriptItem>>(Vec::new);
    let mut composer = use_signal(String::new);
    let mut slash_open = use_signal(|| false);
    let in_flight = use_signal(|| false);
    let active_ct = use_signal::<Option<CancellationToken>>(|| None);
    let usage_total = use_signal::<Usage>(Usage::default);
    // Tracks whether the tail-end AssistantText in `transcript` has been
    // persisted to chat_message yet. Set true on each Text delta; cleared
    // by `flush_pending_assistant` after writing to the repo. We persist
    // on Done (or before any non-text event) to coalesce streaming deltas
    // into one row per assistant block instead of a write per delta.
    let pending_assistant = use_signal(|| false);
    // Visibility of the MCP settings modal — opened by the wrench button
    // in the chat header, closed by clicking the scrim or pressing Esc.
    let mcp_panel_open: Signal<bool> = use_signal(|| false);

    // Hotfix: every context lookup uses `try_consume_context` so a missing
    // provider renders a degraded (but visible) chat surface instead of
    // panicking and bringing down sibling regions of the Shell tree.
    let scope_signal = match try_consume_context::<ActiveChatScope>() {
        Some(ActiveChatScope(s)) => s,
        None => {
            return rsx! {
                section { class: "operon-companion-chat",
                    div { class: "operon-companion-msg operon-companion-msg-system",
                        "Companion not available — chat scope context missing."
                    }
                }
            };
        }
    };
    let session_signal = match try_consume_context::<ActiveChatSession>() {
        Some(ActiveChatSession(s)) => s,
        None => {
            return rsx! {
                section { class: "operon-companion-chat",
                    div { class: "operon-companion-msg operon-companion-msg-system",
                        "Companion not available — chat session context missing."
                    }
                }
            };
        }
    };
    let active_repo = match try_consume_context::<ActiveRepoPath>() {
        Some(ActiveRepoPath(s)) => s,
        None => {
            return rsx! {
                section { class: "operon-companion-chat",
                    div { class: "operon-companion-msg operon-companion-msg-system",
                        "Companion not available — repo path context missing."
                    }
                }
            };
        }
    };
    let vault_root = match try_consume_context::<CurrentVaultRoot>() {
        Some(CurrentVaultRoot(s)) => s,
        None => {
            return rsx! {
                section { class: "operon-companion-chat",
                    div { class: "operon-companion-msg operon-companion-msg-system",
                        "Companion not available — vault context missing."
                    }
                }
            };
        }
    };
    let message_repo = match try_consume_context::<ChatMessageRepo>() {
        Some(ChatMessageRepo(r)) => r,
        None => {
            return rsx! {
                section { class: "operon-companion-chat",
                    div { class: "operon-companion-msg operon-companion-msg-system",
                        "Companion not available — message repo missing."
                    }
                }
            };
        }
    };
    // Phase D: live transcript re-load is handled via the
    // `CHAT_MESSAGE_VERSION` GlobalSignal — see the load-effect
    // below. No Signal hook needed here.
    let session_repo = match try_consume_context::<ChatSessionRepo>() {
        Some(ChatSessionRepo(r)) => r,
        None => {
            return rsx! {
                section { class: "operon-companion-chat",
                    div { class: "operon-companion-msg operon-companion-msg-system",
                        "Companion not available — session repo missing."
                    }
                }
            };
        }
    };
    let session_version = match try_consume_context::<crate::shell::companion_state::ChatSessionVersion>() {
        Some(crate::shell::companion_state::ChatSessionVersion(v)) => v,
        None => {
            return rsx! {
                section { class: "operon-companion-chat",
                    div { class: "operon-companion-msg operon-companion-msg-system",
                        "Companion not available — session version missing."
                    }
                }
            };
        }
    };
    // Optional inbox — remote callers (e.g., the skill plugin's Play
    // button) can drop a prompt here and the composer picks it up on the
    // next render. Render-body sync uses peek + clear so missing context
    // is harmless and the signal flips back to None after consumption.
    let composer_inbox = try_consume_context::<CompanionComposerInbox>().map(|c| c.0);
    if let Some(mut inbox) = composer_inbox {
        let pending = inbox.peek().clone();
        if let Some(text) = pending {
            composer.set(text);
            inbox.set(None);
        }
    }

    // Resolve cwd for the active scope. For Project scope, look the
    // repo_path up directly from `local_project` keyed by the scope's
    // project id — bypasses the broken `active_repo_path` use_effect
    // that wasn't firing when the user clicked a NOTE inside a project
    // (selected_project stays None; only selected_note flips). The
    // `_ = active_repo.read()` keeps backward subscribers happy without
    // letting the broken signal gate the actual cwd value.
    let project_repo_for_cwd = match try_consume_context::<LocalProjectRepo>() {
        Some(LocalProjectRepo(r)) => Some(r),
        None => None,
    };
    let project_version = try_consume_context::<LocalProjectVersion>().map(|c| c.0);
    let cwd_for_scope = use_memo(move || -> Option<PathBuf> {
        let _ = active_repo.read();
        if let Some(v) = project_version.as_ref() {
            let _ = v.read();
        }
        match *scope_signal.read() {
            ChatScope::Project(pid) => project_repo_for_cwd.as_ref().and_then(|repo| {
                repo.list()
                    .ok()
                    .and_then(|projects| projects.into_iter().find(|p| p.id == pid))
                    .and_then(|p| p.repo_path)
            }),
            ChatScope::Vault => vault_root.read().as_ref().map(|v| v.path.clone()),
        }
    });

    // Re-bind the active session whenever cwd or session changes.
    // After binding, kick off `ensure_session_bridge` so subsequent
    // turns spawn claude with the inline-permission-prompt MCP wired
    // up. The async work is fire-and-forget — the bridge is idempotent
    // per session, and writes to the plugin via `set_session_bridge`
    // (which atomically swaps the per-session field), so a slow bridge
    // bind won't block the chat from sending messages.
    {
        let plugin_for_effect = plugin;
        let backend_for_effect = active_backend;
        let session = session_signal;
        let cwd = cwd_for_scope;
        use_effect(move || {
            let backend = backend_for_effect.read().clone();
            let claude_plugin = plugin_for_effect.read().clone();
            let sid = *session.read();
            let cwd = cwd.read().clone();
            match (sid, cwd) {
                (Some(sid), Some(cwd)) => {
                    let cwd_for_bind = cwd.clone();
                    let backend_for_bind = backend.clone();
                    spawn(async move {
                        if let Err(e) = backend_for_bind.bind_session(sid, cwd_for_bind).await {
                            tracing::warn!(
                                target: "operon::companion",
                                "bind_session({sid}): {e}"
                            );
                        }
                    });
                    // The MCP-permission-shim bridge is claude-code only.
                    // Skip it when the active backend is the in-process
                    // runtime — the runtime emits permission requests via
                    // `AgentEvent::PermissionRequest` instead.
                    if backend.id() == "claude-code" {
                        let cwd_for_bridge = cwd;
                        spawn(async move {
                            if let Err(e) =
                                crate::shell::companion_state::ensure_session_bridge(
                                    &claude_plugin,
                                    sid,
                                    cwd_for_bridge,
                                )
                                .await
                            {
                                tracing::warn!(
                                    target: "operon::permission",
                                    "ensure_session_bridge({sid}): {e}"
                                );
                            }
                        });
                    }
                }
                (Some(sid), None) => {
                    let backend = backend.clone();
                    spawn(async move {
                        let _ = backend.unbind_session(sid).await;
                    });
                }
                _ => {}
            }
        });
    }

    // Reset transcript + cost on session switch, then replay any
    // persisted history for the newly-active session. Cost meter doesn't
    // restore from disk (deferred — needs per-session usage column).
    {
        let session = session_signal;
        let mut transcript_setter = transcript;
        let mut usage_setter = usage_total;
        let mut pending_setter = pending_assistant;
        let repo = message_repo.clone();
        use_effect(move || {
            let sid = *session.read();
            usage_setter.set(Usage::default());
            pending_setter.set(false);
            match sid {
                Some(id) => match repo.list(id) {
                    Ok(rows) => {
                        let restored: Vec<TranscriptItem> =
                            rows.iter().filter_map(transcript_item_from_message).collect();
                        transcript_setter.set(restored);
                    }
                    Err(e) => {
                        tracing::warn!(
                            target: "operon::companion",
                            "load chat history for {id}: {e}"
                        );
                        transcript_setter.set(Vec::new());
                    }
                },
                None => transcript_setter.set(Vec::new()),
            }
        });
    }

    // Phase D: live transcript updates via polling. We previously
    // tried `*CHAT_MESSAGE_VERSION.read()` inside the load effect,
    // but reading a `GlobalSignal` inside `use_effect` interacted
    // with the effect's own writes to create a tight infinite loop
    // (effect runs → writes transcript → component re-renders →
    // effect re-fires, ~200×/sec). Polling sidesteps the
    // subscription quirk entirely: every 500ms, if the active
    // session is set and `chat_message` rows differ from the
    // current transcript, push the new rows. The runner's
    // `bump_message_version()` calls become advisory — they hint
    // that a poll might find new rows but the poll is the
    // authoritative trigger. 500ms is below the noticeable-lag
    // threshold for typing UIs and the cost is one SQLite SELECT.
    {
        let session = session_signal;
        let mut transcript_setter = transcript;
        let repo = message_repo.clone();
        use_future(move || {
            let repo = repo.clone();
            async move {
                use std::time::Duration;
                let mut last_seen_version: u64 = 0;
                loop {
                    futures_timer::Delay::new(Duration::from_millis(500)).await;
                    let cur_version = *CHAT_MESSAGE_VERSION.peek();
                    if cur_version == last_seen_version {
                        continue;
                    }
                    last_seen_version = cur_version;
                    let Some(id) = *session.peek() else { continue };
                    let Ok(rows) = repo.list(id) else { continue };
                    let restored: Vec<TranscriptItem> = rows
                        .iter()
                        .filter_map(transcript_item_from_message)
                        .collect();
                    if restored != *transcript_setter.peek() {
                        transcript_setter.set(restored);
                    }
                }
            }
        });
    }

    // Auto-scroll the chat transcript to the bottom whenever new
    // items are appended (or the poll loop replaces the list).
    // Without this, runner-driven sessions look frozen at the user
    // prompt because the giant prompt fills the visible area and
    // tool_use cards / later assistant text live below the fold.
    // The eval is a tiny querySelector + scrollIntoView; the
    // selector is stable via `data-testid="companion-transcript"`
    // on the container (above).
    {
        let transcript_for_scroll = transcript;
        use_effect(move || {
            let _len = transcript_for_scroll.read().len();
            // Subscribing to len means this effect re-fires on every
            // transcript mutation. Skip the no-op zero-length case
            // on first mount.
            if _len == 0 {
                return;
            }
            let _ = dioxus::document::eval(
                "(function() { \
                  const root = document.querySelector('[data-testid=\"companion-transcript\"]'); \
                  if (!root) return; \
                  const last = root.lastElementChild; \
                  if (last && typeof last.scrollIntoView === 'function') { \
                    last.scrollIntoView({ behavior: 'smooth', block: 'end' }); \
                  } else { \
                    root.scrollTop = root.scrollHeight; \
                  } \
                })();",
            );
        });
    }

    let active_session = *session_signal.read();
    let has_session = active_session.is_some();
    let has_cwd = cwd_for_scope.read().is_some();
    let scope_now = *scope_signal.read();
    let scope_is_project = matches!(scope_now, ChatScope::Project(_));
    let banner = if !has_cwd {
        Some(if scope_is_project {
            "This project has no repository. Right-click the project → Set repository… to enable Claude."
        } else {
            "No vault is configured. Pick a vault in Settings → Vault to enable Claude here."
        })
    } else {
        None
    };
    // Surface a one-line notice when the MCP permission shim isn't on
    // disk. Without it, claude has no channel to ask for tool
    // permission in headless mode and auto-denies Bash/Edit/Write —
    // which the user reports as "npm i was rejected". Render only when
    // a cwd is set (so we don't pile up errors on the no-cwd banner)
    // and the picker would actually ask (i.e. not in
    // `bypassPermissions`).
    let shim_missing_notice = {
        let plugin_arc = plugin.read().clone();
        let mode = perm_mode.read().clone();
        if has_cwd
            && !plugin_arc.shim_available()
            && mode.as_deref() != Some("bypassPermissions")
        {
            Some(
                "Permission shim not built — Claude tool calls (Bash, Edit, Write) will be \
                 auto-denied. Build it with `cargo build` (it now ships with the workspace) or \
                 set OPERON_MCP_PERMISSION_BIN.",
            )
        } else {
            None
        }
    };

    rsx! {
        div { class: "operon-companion-chat-grid",
            SessionRail {}
            RailSplitter {}
            section { class: "operon-companion-chat",
                "data-region": "companion-chat",
                div { class: "operon-companion-chat-header",
                    span { class: "operon-companion-chat-title", "" }
                    if let Some(backends) = backends_ctx.clone() {
                        {
                            use crate::shell::agent_backend_picker::{
                                AgentBackendKind, AgentBackendPicker,
                            };
                            let current_id = active_backend.read().id().to_string();
                            let current = AgentBackendKind::parse(&current_id)
                                .unwrap_or(AgentBackendKind::ClaudeCode);
                            let mut active_backend_setter = active_backend;
                            let in_flight_read = *in_flight.read();
                            rsx! {
                                AgentBackendPicker {
                                    current,
                                    enabled: !in_flight_read,
                                    on_change: move |kind: AgentBackendKind| {
                                        active_backend_setter.set(backends.pick(kind));
                                    },
                                }
                            }
                        }
                    }
                    {
                        let plugin_arc = plugin.read().clone();
                        let current_model = plugin_arc.current_default_model();
                        let current_perm = perm_mode.read().clone();
                        let plugin_for_model = plugin_arc.clone();
                        let plugin_for_perm = plugin_arc.clone();
                        rsx! {
                            label { class: "operon-companion-toolbar-label",
                                title: "Model used for new turns",
                                span { class: "sr-only", "Model" }
                                select {
                                    class: "operon-companion-model-picker",
                                    "data-testid": "companion-model-picker",
                                    onchange: move |e| {
                                        let v = e.value();
                                        let next = if v == "default" { None } else { Some(v) };
                                        plugin_for_model.set_default_model(next);
                                    },
                                    option { value: "default",
                                        selected: current_model.is_none(),
                                        "Default"
                                    }
                                    option { value: "claude-opus-4-7",
                                        selected: current_model.as_deref() == Some("claude-opus-4-7"),
                                        "Opus 4.7"
                                    }
                                    option { value: "claude-opus-4-6",
                                        selected: current_model.as_deref() == Some("claude-opus-4-6"),
                                        "Opus 4.6"
                                    }
                                    option { value: "claude-sonnet-4-6",
                                        selected: current_model.as_deref() == Some("claude-sonnet-4-6"),
                                        "Sonnet 4.6"
                                    }
                                    option { value: "claude-sonnet-4-5",
                                        selected: current_model.as_deref() == Some("claude-sonnet-4-5"),
                                        "Sonnet 4.5"
                                    }
                                    option { value: "claude-haiku-4-5",
                                        selected: current_model.as_deref() == Some("claude-haiku-4-5"),
                                        "Haiku 4.5"
                                    }
                                    option { value: "claude-3-5-sonnet-20241022",
                                        selected: current_model.as_deref() == Some("claude-3-5-sonnet-20241022"),
                                        "Sonnet 3.5 (2024-10-22)"
                                    }
                                    option { value: "claude-3-5-haiku-20241022",
                                        selected: current_model.as_deref() == Some("claude-3-5-haiku-20241022"),
                                        "Haiku 3.5 (2024-10-22)"
                                    }
                                    option { value: "claude-3-opus-20240229",
                                        selected: current_model.as_deref() == Some("claude-3-opus-20240229"),
                                        "Opus 3 (2024-02-29)"
                                    }
                                }
                            }
                            label { class: "operon-companion-toolbar-label",
                                title: "claude --permission-mode",
                                span { class: "sr-only", "Permission mode" }
                                select {
                                    class: "operon-companion-model-picker",
                                    "data-testid": "companion-permission-picker",
                                    onchange: move |e| {
                                        let v = e.value();
                                        let next = if v == "(default)" { None } else { Some(v) };
                                        plugin_for_perm.set_permission_mode(next.clone());
                                        perm_mode.set(next);
                                    },
                                    option { value: "(default)",
                                        selected: current_perm.is_none(),
                                        "Permissions: default"
                                    }
                                    option { value: "acceptEdits",
                                        selected: current_perm.as_deref() == Some("acceptEdits"),
                                        "Accept edits"
                                    }
                                    option { value: "plan",
                                        selected: current_perm.as_deref() == Some("plan"),
                                        "Plan"
                                    }
                                    option { value: "bypassPermissions",
                                        selected: current_perm.as_deref() == Some("bypassPermissions"),
                                        "Bypass"
                                    }
                                }
                            }
                        }
                    }
                    button {
                        r#type: "button",
                        class: "operon-companion-mcp-button",
                        "data-testid": "companion-mcp-toggle",
                        title: "Manage MCP servers",
                        onclick: {
                            let mut mcp_panel_open = mcp_panel_open;
                            move |_| {
                                let cur = *mcp_panel_open.read();
                                mcp_panel_open.set(!cur);
                            }
                        },
                        "MCP"
                    }
                    if *in_flight.read() {
                        button {
                            class: "operon-companion-chat-stop",
                            "data-testid": "companion-stop",
                            onclick: move |_| {
                                if let Some(ct) = active_ct.read().as_ref() {
                                    ct.cancel();
                                }
                            },
                            "Stop"
                        }
                    }
                }
                div { class: "operon-companion-chat-transcript",
                    "data-testid": "companion-transcript",
                    if let Some(b) = banner {
                        div {
                            class: "operon-companion-msg operon-companion-msg-system",
                            "data-testid": "companion-no-cwd-banner",
                            "{b}"
                        }
                    }
                    if let Some(n) = shim_missing_notice {
                        div {
                            class: "operon-companion-msg operon-companion-msg-system",
                            "data-testid": "companion-shim-missing-banner",
                            "{n}"
                        }
                    }
                    if !has_session {
                        div {
                            class: "operon-companion-msg operon-companion-msg-system",
                            "data-testid": "companion-no-session",
                            "No chat selected. Click + to start one."
                        }
                    }
                    for (i, item) in transcript.read().iter().enumerate() {
                        {render_item(i, item)}
                    }
                    // Inline permission prompts. These come from the
                    // MCP bridge — `claude --print` can't show its own
                    // approval UI in headless mode, so when it asks
                    // for permission the bridge surfaces the request
                    // here instead of silently failing. Rendered after
                    // the transcript so a pending prompt always sits
                    // at the bottom where the user is looking.
                    for (j, entry) in PERMISSION_PROMPTS.read().iter().enumerate() {
                        {render_item(
                            transcript.read().len() + j,
                            &TranscriptItem::PermissionRequest {
                                id: entry.id.clone(),
                                tool_name: entry.tool_name.clone(),
                                input: entry.input.clone(),
                            },
                        )}
                    }
                    // Streaming surface (Phase G): live letter-by-
                    // letter Claude text + "Claude is thinking…"
                    // loader. Both subscribe to GlobalSignals so
                    // they re-render on every delta from the runner
                    // (or any background drainer).
                    {
                        let sid_now = *session_signal.read();
                        let inprogress: Option<String> = sid_now
                            .and_then(|id| {
                                INPROGRESS_ASSISTANT.read().get(&id).cloned()
                            })
                            .filter(|s| !s.is_empty());
                        let is_running = sid_now
                            .map(|id| {
                                matches!(
                                    ARTIFACT_RUN_STATE.read().get(&id),
                                    Some(ArtifactRunState::Running)
                                )
                            })
                            .unwrap_or(false);
                        // Cascade-level indicator: separate from the
                        // per-skill `ARTIFACT_RUN_STATE.Running`
                        // signal, this lights up while any cascade
                        // run started from the artifact toolbar's ▶
                        // Play button is in flight on this chat
                        // session. Sits BELOW the thinking /
                        // streaming row so the user sees
                        // letter-by-letter Claude output AND the
                        // overarching "the cascade is still working"
                        // banner together.
                        let cascade_working = sid_now
                            .map(|id| {
                                crate::shell::companion_state::CASCADE_RUNNING_SESSIONS
                                    .read()
                                    .contains(&id)
                            })
                            .unwrap_or(false);
                        rsx! {
                            {
                                match (inprogress, is_running) {
                                    (Some(text), _) => rsx! {
                                        div {
                                            class: "operon-companion-msg operon-companion-msg-assistant operon-companion-msg-assistant-streaming",
                                            "data-testid": "companion-streaming",
                                            "{text}"
                                            span {
                                                class: "operon-companion-streaming-cursor",
                                                "\u{258B}"
                                            }
                                        }
                                    },
                                    (None, true) => rsx! {
                                        div {
                                            class: "operon-companion-msg operon-companion-msg-thinking",
                                            "data-testid": "companion-thinking",
                                            span { class: "operon-companion-thinking-spinner" }
                                            span { class: "operon-companion-thinking-label",
                                                "Claude is thinking\u{2026}"
                                            }
                                        }
                                    },
                                    (None, false) => rsx! {},
                                }
                            }
                            if cascade_working {
                                div {
                                    class: "operon-companion-msg operon-companion-cascade-working",
                                    "data-testid": "companion-cascade-working",
                                    span { class: "operon-companion-thinking-spinner" }
                                    span { class: "operon-companion-thinking-label",
                                        "Claude is working\u{2026}"
                                    }
                                }
                            }
                        }
                    }
                }
                {
                    let u = usage_total.read();
                    rsx! { CostMeter {
                        prompt: u.prompt,
                        prompt_cached: u.prompt_cached,
                        completion: u.completion,
                    } }
                }
                div { class: "operon-companion-chat-composer",
                    "data-testid": "companion-composer-wrap",
                    div { class: "operon-companion-composer-toolbar",
                        button {
                            r#type: "button",
                            class: "operon-companion-slash-button",
                            "data-testid": "companion-slash-button",
                            title: "Slash commands",
                            onclick: move |_| {
                                let next = !*slash_open.read();
                                slash_open.set(next);
                            },
                            "/"
                        }
                        if *slash_open.read() {
                            ul {
                                class: "operon-companion-slash-popover",
                                "data-testid": "companion-slash-popover",
                                for cmd in SLASH_COMMANDS.iter() {
                                    {
                                        let cmd = *cmd;
                                        rsx! {
                                            li {
                                                key: "{cmd}",
                                                button {
                                                    r#type: "button",
                                                    class: "operon-companion-slash-item",
                                                    onclick: move |_| {
                                                        composer.set(cmd.into());
                                                        slash_open.set(false);
                                                    },
                                                    "{cmd}"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    textarea {
                        class: "operon-companion-chat-input",
                        "data-testid": "companion-input",
                        value: "{composer}",
                        placeholder: if has_cwd && has_session {
                            "Type a message... (Cmd/Ctrl+Enter to send)"
                        } else if !has_session {
                            "Pick or create a chat to start…"
                        } else {
                            "Bind a repository or pick a vault to start…"
                        },
                        disabled: !has_cwd || !has_session,
                        oninput: move |e| composer.set(e.value()),
                        onkeydown: {
                            let repo = message_repo.clone();
                            let srepo = session_repo.clone();
                            move |e: KeyboardEvent| {
                                if !has_cwd || !has_session { return; }
                                if e.key() == Key::Enter && (e.modifiers().ctrl() || e.modifiers().meta()) {
                                    if let Some(sid) = active_session {
                                        run_turn(active_backend, sid, transcript, composer, in_flight, active_ct, usage_total, pending_assistant, repo.clone(), srepo.clone(), session_version);
                                    }
                                }
                            }
                        },
                    }
                    button {
                        class: "operon-companion-chat-send",
                        "data-testid": "companion-send",
                        disabled: *in_flight.read() || !has_cwd || !has_session,
                        onclick: {
                            let repo = message_repo.clone();
                            let srepo = session_repo.clone();
                            move |_| {
                                if let Some(sid) = active_session {
                                    run_turn(active_backend, sid, transcript, composer, in_flight, active_ct, usage_total, pending_assistant, repo.clone(), srepo.clone(), session_version);
                                }
                            }
                        },
                        "Send"
                    }
                }
                crate::shell::mcp_settings::McpSettingsPanel {
                    open: mcp_panel_open,
                }
            }
        }
    }
}

fn render_item(i: usize, item: &TranscriptItem) -> Element {
    let key = format!("{i}");
    match item {
        TranscriptItem::UserText(t) => rsx! {
            div {
                key: "{key}",
                class: "operon-companion-msg operon-companion-msg-user",
                "data-role": "user",
                "{t}"
            }
        },
        TranscriptItem::AssistantText(body) => rsx! {
            div {
                key: "{key}",
                class: "operon-companion-msg operon-companion-msg-assistant",
                "data-role": "assistant",
                MarkdownView { content: body.clone() }
            }
        },
        TranscriptItem::Thinking(t) => rsx! {
            details {
                key: "{key}",
                class: "operon-companion-thinking",
                "data-role": "thinking",
                summary { "Thinking" }
                pre { class: "operon-companion-thinking-body", "{t}" }
            }
        },
        TranscriptItem::ToolCall { id, name, input, result } => rsx! {
            ToolCard {
                key: "{key}",
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
                result: result.clone(),
            }
        },
        TranscriptItem::System(t) => rsx! {
            div {
                key: "{key}",
                class: "operon-companion-msg operon-companion-msg-system",
                "data-role": "system",
                "{t}"
            }
        },
        TranscriptItem::PermissionRequest {
            id,
            tool_name,
            input,
        } => render_permission_request(key, id, tool_name, input),
    }
}

/// Render an inline permission prompt with Allow / Allow always / Deny
/// buttons. Mirrors the artifact view's approve/reject pattern
/// (`src/plugins/artifact/view.rs:174-199`): each button disables when
/// the prompt is already in that terminal state, and the click
/// handler updates `PERMISSION_DECISIONS` so the re-render disables
/// the row plus dispatches the bridge response.
fn render_permission_request(
    key: String,
    id: &str,
    tool_name: &str,
    input: &Value,
) -> Element {
    let status = PERMISSION_DECISIONS
        .read()
        .get(id)
        .cloned()
        .unwrap_or(PermissionStatus::Pending);
    let summary = render_permission_summary(tool_name, input);
    let allow_disabled = status != PermissionStatus::Pending;
    let id_owned = id.to_string();
    let id_for_allow = id_owned.clone();
    let id_for_allow_always = id_owned.clone();
    let id_for_deny = id_owned;
    let allow = move |_| resolve_permission(&id_for_allow, PermissionStatus::Allowed);
    let allow_always = move |_| {
        resolve_permission(&id_for_allow_always, PermissionStatus::AllowedAlways)
    };
    let deny = move |_| resolve_permission(&id_for_deny, PermissionStatus::Denied);
    let status_label = match status {
        PermissionStatus::Pending => "Awaiting decision",
        PermissionStatus::Allowed => "Allowed",
        PermissionStatus::AllowedAlways => "Allowed (always)",
        PermissionStatus::Denied => "Denied",
    };
    rsx! {
        div {
            key: "{key}",
            class: "operon-companion-permission-prompt",
            "data-testid": "companion-permission-prompt",
            "data-status": "{status_label}",
            div { class: "operon-companion-permission-head",
                strong { "{tool_name}" }
                span { class: "operon-companion-permission-status", " — {status_label}" }
            }
            pre { class: "operon-companion-permission-body", "{summary}" }
            div { class: "operon-companion-permission-actions",
                button {
                    r#type: "button",
                    class: "operon-companion-permission-allow",
                    "data-testid": "companion-permission-allow",
                    disabled: allow_disabled,
                    onclick: allow,
                    "Allow"
                }
                button {
                    r#type: "button",
                    class: "operon-companion-permission-allow-always",
                    "data-testid": "companion-permission-allow-always",
                    disabled: allow_disabled,
                    onclick: allow_always,
                    "Allow always"
                }
                button {
                    r#type: "button",
                    class: "operon-companion-permission-deny",
                    "data-testid": "companion-permission-deny",
                    disabled: allow_disabled,
                    onclick: deny,
                    "Deny"
                }
            }
        }
    }
}

/// Pull the most useful one-liner out of the proposed tool input. For
/// `Bash` that's the `command` field; for everything else, fall back
/// to the JSON itself so the user can at least see what's being
/// requested.
pub(crate) fn render_permission_summary(tool_name: &str, input: &Value) -> String {
    if tool_name == "Bash" {
        if let Some(cmd) = input.get("command").and_then(|v| v.as_str()) {
            return cmd.to_string();
        }
    }
    match input {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Click handler shared by all three buttons. Updates the reactive
/// status map (so the buttons disable on the next render) and forwards
/// the matching MCP response over the parked oneshot.
///
/// `AllowedAlways` additionally writes the derived rule into the child
/// project's `.claude/settings.local.json`, matching the persistent
/// allowlist format the harness already reads.
fn resolve_permission(id: &str, choice: PermissionStatus) {
    PERMISSION_DECISIONS
        .write()
        .insert(id.to_string(), choice.clone());

    // Look up the entry to recover tool_name + input + cwd before we
    // consume the responder. `find` clones a shallow copy so we drop
    // the read-guard before any further work.
    let entry = PERMISSION_PROMPTS
        .read()
        .iter()
        .find(|e| e.id == id)
        .cloned();

    if matches!(choice, PermissionStatus::AllowedAlways) {
        if let Some(entry) = &entry {
            if let Some(cwd) = entry.source_cwd.as_ref() {
                let rule = crate::shell::permission_persist::derive_rule(
                    &entry.tool_name,
                    &entry.input,
                );
                if let Err(e) =
                    crate::shell::permission_persist::append_allow_rule(cwd, &rule)
                {
                    tracing::warn!(
                        target: "operon::permission",
                        "persist allow rule {rule} for {}: {e}",
                        cwd.display()
                    );
                }
            } else {
                tracing::warn!(
                    target: "operon::permission",
                    "Allow-always for {id}: no cwd captured; rule not persisted"
                );
            }
        }
    }

    let Some(responder) = take_permission_responder(id) else {
        // Already resolved (double-click); UI re-render is the only
        // visible side-effect.
        return;
    };
    let decision = match choice {
        PermissionStatus::Pending => return, // unreachable from buttons
        PermissionStatus::Allowed | PermissionStatus::AllowedAlways => {
            operon_plugins_claude_code::PermissionDecision::Allow {
                updated_input: None,
            }
        }
        PermissionStatus::Denied => operon_plugins_claude_code::PermissionDecision::Deny {
            message: "Denied by user".into(),
        },
    };
    let _ = responder.send(decision);
}

#[derive(Props, Clone, PartialEq)]
struct CostMeterProps {
    prompt: u64,
    prompt_cached: u64,
    completion: u64,
}

#[component]
fn CostMeter(props: CostMeterProps) -> Element {
    if props.prompt == 0 && props.completion == 0 {
        return rsx! { div { class: "operon-companion-cost-meter operon-companion-cost-meter-empty" } };
    }
    let cache_pct = if props.prompt > 0 {
        (props.prompt_cached as f64 / props.prompt as f64) * 100.0
    } else {
        0.0
    };
    let cost = estimate_cost_usd(props.prompt, props.prompt_cached, props.completion);
    let prompt_total = props.prompt;
    let completion = props.completion;
    rsx! {
        div { class: "operon-companion-cost-meter",
            "data-testid": "companion-cost-meter",
            span { class: "operon-companion-cost-segment",
                "{prompt_total} in"
            }
            span { class: "operon-companion-cost-segment",
                "{completion} out"
            }
            span { class: "operon-companion-cost-segment",
                "{cache_pct:.0}% cached"
            }
            span { class: "operon-companion-cost-segment operon-companion-cost-cost",
                "${cost:.4}"
            }
        }
    }
}

/// Rough per-token cost estimate. USD per 1M tokens for the default Claude
/// model family (Opus-tier). Close enough to give the user a running "this
/// turn cost X" feel without claiming to be billing-accurate.
fn estimate_cost_usd(prompt: u64, prompt_cached: u64, completion: u64) -> f64 {
    let in_full_per_mtok = 15.0;
    let in_cache_per_mtok = 1.5;
    let out_per_mtok = 75.0;
    let uncached = prompt.saturating_sub(prompt_cached);
    let in_cost =
        (uncached as f64 / 1_000_000.0) * in_full_per_mtok
            + (prompt_cached as f64 / 1_000_000.0) * in_cache_per_mtok;
    let out_cost = (completion as f64 / 1_000_000.0) * out_per_mtok;
    in_cost + out_cost
}

/// Take the current composer text, append it to the transcript, persist
/// the user line, and stream the plugin's `ClaudeCodeEvent`s into the
/// transcript signal (also persisting each event). The Operon session
/// UUID is the active rail-selected one; the plugin reads its per-session
/// binding to spawn `claude` with the right cwd + `--resume`.
///
/// On the first user message of a session whose label is still "New chat",
/// derives a label from the message and renames the session — same pattern
/// the VS Code extension uses to keep the rail readable.
#[allow(clippy::too_many_arguments)]
fn run_turn(
    backend: Signal<Arc<dyn AgentBackend>>,
    chat_session: Uuid,
    mut transcript: Signal<Vec<TranscriptItem>>,
    mut composer: Signal<String>,
    mut in_flight: Signal<bool>,
    mut active_ct: Signal<Option<CancellationToken>>,
    mut usage_total: Signal<Usage>,
    mut pending_assistant: Signal<bool>,
    repo: Arc<dyn crate::shell::companion_state::ChatMessageRepository>,
    session_repo: Arc<dyn crate::shell::companion_state::ChatSessionRepository>,
    mut session_version: Signal<u64>,
) {
    if *in_flight.read() {
        return;
    }
    let text = composer.read().trim().to_string();
    if text.is_empty() {
        return;
    }
    composer.set(String::new());
    transcript
        .write()
        .push(TranscriptItem::UserText(text.clone()));
    if let Err(e) = repo.append(
        chat_session,
        ChatMessageKind::User,
        None,
        &serde_json::json!({ "text": text }),
    ) {
        tracing::warn!(target: "operon::companion", "persist user text: {e}");
    }
    // Auto-rename the session from the first user message if the label is
    // still the default "New chat". Manual renames in the rail set the
    // label to anything else and disable this path automatically.
    auto_rename_if_default(&session_repo, chat_session, &text, &mut session_version);
    in_flight.set(true);
    let ct = CancellationToken::new();
    active_ct.set(Some(ct.clone()));

    let backend_arc: Arc<dyn AgentBackend> = backend.read().clone();
    let repo_for_task = repo.clone();
    spawn(async move {
        let mut rx = match backend_arc.send_rich(text, chat_session, ct).await {
            Ok(rx) => rx,
            Err(e) => {
                let msg = format!("error: {e}");
                transcript
                    .write()
                    .push(TranscriptItem::System(msg.clone()));
                let _ = repo_for_task.append(
                    chat_session,
                    ChatMessageKind::System,
                    None,
                    &serde_json::json!({ "text": msg }),
                );
                in_flight.set(false);
                active_ct.set(None);
                return;
            }
        };
        while let Some(ev) = rx.recv().await {
            apply_event(
                &mut transcript,
                &mut usage_total,
                &mut pending_assistant,
                chat_session,
                &repo_for_task,
                ev,
            );
        }
        // Whatever the loop ended with, make sure any in-progress
        // assistant text gets a row before we go quiet.
        flush_pending_assistant(&mut transcript, &mut pending_assistant, chat_session, &repo_for_task);
        in_flight.set(false);
        active_ct.set(None);
    });
}

fn apply_event(
    transcript: &mut Signal<Vec<TranscriptItem>>,
    usage_total: &mut Signal<Usage>,
    pending_assistant: &mut Signal<bool>,
    chat_session: Uuid,
    repo: &Arc<dyn crate::shell::companion_state::ChatMessageRepository>,
    ev: AgentEvent,
) {
    match ev {
        AgentEvent::Text(t) => {
            let mut tx = transcript.write();
            if let Some(TranscriptItem::AssistantText(body)) = tx.last_mut() {
                body.push_str(&t);
            } else {
                tx.push(TranscriptItem::AssistantText(t));
            }
            pending_assistant.set(true);
        }
        AgentEvent::Thinking(t) => {
            // Any prior assistant block is now "complete" — flush before
            // we shift the tail of the transcript away from it.
            flush_pending_assistant(transcript, pending_assistant, chat_session, repo);
            transcript.write().push(TranscriptItem::Thinking(t.clone()));
            if let Err(e) = repo.append(
                chat_session,
                ChatMessageKind::Thinking,
                None,
                &serde_json::json!({ "text": t }),
            ) {
                tracing::warn!(target: "operon::companion", "persist thinking: {e}");
            }
        }
        AgentEvent::ToolUse { id, name, input } => {
            flush_pending_assistant(transcript, pending_assistant, chat_session, repo);
            transcript.write().push(TranscriptItem::ToolCall {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
                result: None,
            });
            let body = serde_json::json!({
                "id": id,
                "name": name,
                "input": input,
                "result": serde_json::Value::Null,
            });
            if let Err(e) =
                repo.append(chat_session, ChatMessageKind::ToolCall, Some(&id), &body)
            {
                tracing::warn!(target: "operon::companion", "persist tool_use: {e}");
            }
        }
        AgentEvent::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            // Patch in-memory.
            let mut tx = transcript.write();
            let mut patched: Option<(String, serde_json::Value)> = None;
            for item in tx.iter_mut() {
                if let TranscriptItem::ToolCall {
                    id,
                    name,
                    input,
                    result,
                } = item
                {
                    if *id == tool_use_id {
                        *result = Some(ToolResultBody {
                            content: content.clone(),
                            is_error,
                        });
                        patched = Some((
                            id.clone(),
                            serde_json::json!({
                                "id": id,
                                "name": name,
                                "input": input,
                                "result": {
                                    "content": content,
                                    "is_error": is_error,
                                },
                            }),
                        ));
                        break;
                    }
                }
            }
            drop(tx);
            if let Some((_, body)) = patched {
                if let Err(e) = repo.update_tool_result(chat_session, &tool_use_id, &body) {
                    tracing::warn!(target: "operon::companion", "patch tool_result: {e}");
                }
            }
        }
        AgentEvent::Done { stop_reason: _, usage } => {
            flush_pending_assistant(transcript, pending_assistant, chat_session, repo);
            if let Some(u) = usage {
                let mut total = usage_total.write();
                total.prompt += u.prompt;
                total.prompt_cached += u.prompt_cached;
                total.completion += u.completion;
            }
        }
        AgentEvent::Error(msg) => {
            flush_pending_assistant(transcript, pending_assistant, chat_session, repo);
            let line = format!("error: {msg}");
            transcript.write().push(TranscriptItem::System(line.clone()));
            if let Err(e) = repo.append(
                chat_session,
                ChatMessageKind::System,
                None,
                &serde_json::json!({ "text": line }),
            ) {
                tracing::warn!(target: "operon::companion", "persist error: {e}");
            }
        }
        AgentEvent::SessionInit { mcp_servers, tools } => {
            // Mirror claude's per-turn MCP roster + tool inventory into
            // the shared global signal so the MCP settings panel reflects
            // live status. The panel reads `MCP_LIVE_STATUS` and shows a
            // green dot for `connected`, red for `failed`/`needs-auth`,
            // grey when the snapshot is empty.
            *crate::shell::companion_state::MCP_LIVE_STATUS.write() =
                crate::shell::companion_state::McpLiveStatus {
                    mcp_servers,
                    tools,
                    session: Some(chat_session),
                };
        }
        AgentEvent::PermissionRequest {
            id,
            title,
            kind,
            locations: _,
            raw_input,
        } => {
            // Slice A14: runtime backends emit permission asks here
            // (claude-code routes through `ensure_session_bridge` and
            // `PERMISSION_PROMPTS` instead). Push onto the same global
            // signal so the inline prompt UI renders in either case.
            // Step 5 wires the inline `PermissionPrompt` component to
            // resolve this back through the runtime's `PermissionGate`.
            flush_pending_assistant(transcript, pending_assistant, chat_session, repo);
            let entry = crate::shell::companion_state::PermissionPromptEntry {
                id: id.clone(),
                tool_name: kind,
                input: raw_input,
                source_session: Some(chat_session),
                source_cwd: None,
            };
            crate::shell::companion_state::push_permission_prompt(entry);
            tracing::debug!(
                target: "operon::permission",
                "runtime permission request: id={id} title={title}"
            );
        }
    }
}

fn flush_pending_assistant(
    transcript: &mut Signal<Vec<TranscriptItem>>,
    pending: &mut Signal<bool>,
    chat_session: Uuid,
    repo: &Arc<dyn crate::shell::companion_state::ChatMessageRepository>,
) {
    if !*pending.read() {
        return;
    }
    let body_to_persist: Option<String> = {
        let tx = transcript.read();
        tx.iter().rev().find_map(|item| match item {
            TranscriptItem::AssistantText(b) => Some(b.clone()),
            // Only the latest contiguous run matters — once we hit a
            // non-text item from a prior block, stop searching.
            _ => None,
        })
    };
    if let Some(body) = body_to_persist {
        if let Err(e) = repo.append(
            chat_session,
            ChatMessageKind::Assistant,
            None,
            &serde_json::json!({ "body": body }),
        ) {
            tracing::warn!(target: "operon::companion", "persist assistant: {e}");
        }
    }
    pending.set(false);
}

/// If the session's current label is still the default "New chat",
/// generate a label from the user's first message text and rename. The
/// derived label is the first line of the message, trimmed and capped at
/// ~40 visible chars on a word boundary. No-op for sessions that have
/// already been auto- or manually-renamed.
fn auto_rename_if_default(
    session_repo: &Arc<dyn crate::shell::companion_state::ChatSessionRepository>,
    chat_session: Uuid,
    user_text: &str,
    session_version: &mut Signal<u64>,
) {
    let row = match session_repo.get(chat_session) {
        Ok(Some(r)) => r,
        _ => return,
    };
    if row.label != "New chat" {
        return;
    }
    let label = derive_session_label(user_text);
    if label.is_empty() {
        return;
    }
    if let Err(e) = session_repo.rename(chat_session, &label) {
        tracing::warn!(
            target: "operon::companion",
            "auto-rename session {chat_session}: {e}"
        );
        return;
    }
    session_version.with_mut(|v| *v += 1);
}

/// Squeeze a chat-session label out of a free-form user message. First
/// line, trim, collapse whitespace, cap at ~40 chars on a word boundary,
/// append `…` when truncated.
fn derive_session_label(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        return String::new();
    }
    let collapsed: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX: usize = 40;
    if collapsed.chars().count() <= MAX {
        return collapsed;
    }
    // Truncate to MAX chars on a word boundary, append ellipsis.
    let mut head: String = collapsed.chars().take(MAX).collect();
    if let Some(idx) = head.rfind(' ') {
        if idx > MAX / 2 {
            head.truncate(idx);
        }
    }
    head.push('\u{2026}');
    head
}

fn transcript_item_from_message(m: &ChatMessage) -> Option<TranscriptItem> {
    match m.kind {
        ChatMessageKind::User => m
            .body
            .get("text")
            .and_then(|v| v.as_str())
            .map(|s| TranscriptItem::UserText(s.to_string())),
        ChatMessageKind::Assistant => m
            .body
            .get("body")
            .and_then(|v| v.as_str())
            .map(|s| TranscriptItem::AssistantText(s.to_string())),
        ChatMessageKind::Thinking => m
            .body
            .get("text")
            .and_then(|v| v.as_str())
            .map(|s| TranscriptItem::Thinking(s.to_string())),
        ChatMessageKind::ToolCall => {
            let id = m.body.get("id")?.as_str()?.to_string();
            let name = m.body.get("name")?.as_str()?.to_string();
            let input = m
                .body
                .get("input")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let result = match m.body.get("result") {
                None | Some(serde_json::Value::Null) => None,
                Some(r) => {
                    let content = r
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let is_error =
                        r.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
                    Some(ToolResultBody { content, is_error })
                }
            };
            Some(TranscriptItem::ToolCall {
                id,
                name,
                input,
                result,
            })
        }
        ChatMessageKind::System => m
            .body
            .get("text")
            .and_then(|v| v.as_str())
            .map(|s| TranscriptItem::System(s.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_shim_from_exe;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn shim_resolver_finds_sibling_in_target_debug() {
        let tmp = tempdir().unwrap();
        let target_debug = tmp.path().join("target").join("debug");
        fs::create_dir_all(&target_debug).unwrap();
        let exe = target_debug.join("operon-dioxus");
        fs::write(&exe, b"").unwrap();
        let shim = target_debug.join("operon-mcp-permission");
        fs::write(&shim, b"").unwrap();

        assert_eq!(resolve_shim_from_exe(&exe).as_deref(), Some(shim.as_path()));
    }

    #[test]
    fn shim_resolver_finds_target_debug_via_dx_serve_layout() {
        // dx serve runs the app from `<workspace>/target/dx/<crate>/debug/<platform>/app/<exe>`,
        // but the shim is still built into `<workspace>/target/debug/`. The
        // sibling probe fails; the target ancestor walk has to pick it up.
        let tmp = tempdir().unwrap();
        let dx_app_dir = tmp
            .path()
            .join("target")
            .join("dx")
            .join("operon-dioxus")
            .join("debug")
            .join("linux")
            .join("app");
        fs::create_dir_all(&dx_app_dir).unwrap();
        let exe = dx_app_dir.join("operon-dioxus-abcdef0");
        fs::write(&exe, b"").unwrap();

        let target_debug = tmp.path().join("target").join("debug");
        fs::create_dir_all(&target_debug).unwrap();
        let shim = target_debug.join("operon-mcp-permission");
        fs::write(&shim, b"").unwrap();

        assert_eq!(resolve_shim_from_exe(&exe).as_deref(), Some(shim.as_path()));
    }

    #[test]
    fn shim_resolver_returns_none_when_nothing_exists() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("target").join("debug");
        fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("operon-dioxus");
        fs::write(&exe, b"").unwrap();
        // No `operon-mcp-permission` anywhere.
        assert!(resolve_shim_from_exe(&exe).is_none());
    }
}
