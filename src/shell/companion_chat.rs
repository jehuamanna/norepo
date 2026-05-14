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
use regex::Regex;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
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

/// Snapshot of the three persistence tiers for the model/permission
/// pickers — output of the `picker_persisted` memo. Holds the raw
/// chat-tier override (what the dropdown shows as "selected"), the
/// resolved inherited value (for the "Inherit (X)" option label), and
/// the effective value the spawn path uses.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
struct PickerPersisted {
    chat_model: Option<String>,
    chat_perm: Option<String>,
    inherited_model: Option<String>,
    inherited_perm: Option<String>,
    effective_model: Option<String>,
    effective_perm: Option<String>,
}

/// Map a claude model id to its dropdown-friendly label. Used to build
/// the "Inherit (Opus 4.7)" option text without hard-coding the ladder
/// in two places.
pub fn model_display(id: &str) -> String {
    match id {
        "claude-opus-4-7" => "Opus 4.7".into(),
        "claude-opus-4-6" => "Opus 4.6".into(),
        "claude-sonnet-4-6" => "Sonnet 4.6".into(),
        "claude-sonnet-4-5" => "Sonnet 4.5".into(),
        "claude-haiku-4-5" => "Haiku 4.5".into(),
        "claude-3-5-sonnet-20241022" => "Sonnet 3.5".into(),
        "claude-3-5-haiku-20241022" => "Haiku 3.5".into(),
        "claude-3-opus-20240229" => "Opus 3".into(),
        other => other.into(),
    }
}

/// Mirror of `model_display` for `--permission-mode` values.
pub fn perm_display(id: &str) -> String {
    match id {
        "default" => "Default".into(),
        "acceptEdits" => "Accept edits".into(),
        "plan" => "Plan".into(),
        "bypassPermissions" => "Bypass".into(),
        other => other.into(),
    }
}

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
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("operon-mcp-permission");
            if sibling.is_file() {
                return Some(sibling);
            }
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
    let mut mention_picker = use_signal::<Option<MentionPickerState>>(|| None);
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
    let crate::local_mode::desktop::LocalSettingsRepo(settings_repo_for_prefs) =
        use_context();
    // Resolver wired up by `desktop.rs::Workspace` — opens the note in
    // a tab, sets `selected_note`, and asks the explorer to expand the
    // path so the note becomes visible. Optional so tests / sandboxed
    // previews without the resolver render mention chips as inert spans.
    let note_link_resolver =
        try_consume_context::<crate::plugins::markdown::render::NoteLinkResolver>();
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
    // Optional context bundle for resolving `@[<title>](note:<uuid>)`
    // mentions at send-time. Each turn's spawn task pulls bodies from
    // `Persistence` and titles from `LocalNoteRepository` to build the
    // `--- referenced note ---` blocks Claude sees. If either is
    // missing (e.g. tests without a wired repo), mentions stay literal
    // and the rewriter is a no-op.
    let note_repo_for_mentions: Option<Arc<dyn operon_store::repos::LocalNoteRepository>> =
        try_consume_context::<crate::local_mode::desktop::LocalNoteRepo>()
            .map(|c| c.0);
    let persistence_for_mentions: Option<Arc<dyn crate::persistence::Persistence>> =
        try_consume_context::<Arc<dyn crate::persistence::Persistence>>();
    // Project repo for cross-scope note-title resolution. Used by
    // drag-drop / right-click to map a `DragKind::Note(uuid)` to the
    // mention token's display title when the note's owning project
    // isn't otherwise in scope (vault-scope chats).
    let project_repo_for_mentions: Option<Arc<dyn operon_store::repos::LocalProjectRepository>> =
        try_consume_context::<LocalProjectRepo>().map(|c| c.0);
    // App-scope drag session populated by the explorer's
    // `ondragstart`. The chat textarea reads this on drop to
    // distinguish "note dragged from sidebar" from arbitrary OS
    // drops.
    let drag_session_for_drop = try_consume_context::<crate::local_mode::ui::DragSession>()
        .map(|s| s.0);

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
    // Append-semantics sibling — the side-bar's "Send to chat"
    // right-click writes a `@[<title>](note:<uuid>)` token here.
    // We append (with a leading space if non-empty) so the user's
    // current draft isn't clobbered.
    let composer_append =
        try_consume_context::<crate::shell::companion_state::CompanionComposerAppend>()
            .map(|c| c.0);
    if let Some(mut append) = composer_append {
        let pending = append.peek().clone();
        if let Some(text) = pending {
            let current = composer.read().clone();
            composer.set(append_to_composer(&current, &text));
            append.set(None);
        }
    }

    // Resolve cwd for the active scope. For Project scope, look the
    // repo_path up directly from `local_project` keyed by the scope's
    // project id — bypasses the broken `active_repo_path` use_effect
    // that wasn't firing when the user clicked a NOTE inside a project
    // (selected_project stays None; only selected_note flips). The
    // `_ = active_repo.read()` keeps backward subscribers happy without
    // letting the broken signal gate the actual cwd value.
    let project_repo_opt: Option<Arc<dyn operon_store::repos::LocalProjectRepository>> =
        match try_consume_context::<LocalProjectRepo>() {
            Some(LocalProjectRepo(r)) => Some(r),
            None => None,
        };
    let project_version = try_consume_context::<LocalProjectVersion>().map(|c| c.0);
    let cwd_for_scope = {
        let project_repo_for_cwd = project_repo_opt.clone();
        use_memo(move || -> Option<PathBuf> {
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
        })
    };

    // Source-of-truth read for the model/permission pickers, resolving
    // all three tiers (chat → project → global → omit-flag) in one
    // place. The plugin's per-session in-memory state is restored
    // asynchronously by the bind use_effect below, which races the
    // first render after a session switch — reading the plugin there
    // would show "Default" instead of the persisted value until the
    // user wiggled the picker. Reading the DB directly removes that
    // race and gives the picker the inherited-value label for free.
    //
    // Subscribes to:
    //  - `session_signal` / `scope_signal` (which chat / scope is active)
    //  - `session_version`  (chat-tier picker writes)
    //  - `PROJECT_SETTINGS_VERSION` (project-tier picker writes)
    //  - `GLOBAL_SETTINGS_VERSION` (app-settings picker writes)
    let picker_persisted = {
        let session_repo = session_repo.clone();
        let project_repo = project_repo_opt.clone();
        let prefs = settings_repo_for_prefs.clone();
        use_memo(move || -> PickerPersisted {
            let _ = session_version.read();
            let _ = crate::shell::companion_state::PROJECT_SETTINGS_VERSION.read();
            let _ = crate::shell::companion_state::GLOBAL_SETTINGS_VERSION.read();
            let scope = *scope_signal.read();
            let chat_row = session_signal
                .read()
                .and_then(|sid| session_repo.get(sid).ok().flatten());
            let chat_model = chat_row.as_ref().and_then(|c| c.model.clone());
            let chat_perm = chat_row.as_ref().and_then(|c| c.permission_mode.clone());
            let (project_model, project_perm) = match scope {
                ChatScope::Project(pid) => project_repo
                    .as_ref()
                    .and_then(|r| r.get(pid).ok().flatten())
                    .map(|p| (p.default_model, p.default_permission_mode))
                    .unwrap_or((None, None)),
                ChatScope::Vault => (None, None),
            };
            // The global picker writes empty-string when the user
            // selects "Default" (since `local_app_settings` rows store
            // TEXT NOT NULL). Filter so resolve_inherited sees None.
            let global_model = prefs
                .get(crate::local_mode::SETTINGS_KEY_CLAUDE_DEFAULT_MODEL)
                .ok()
                .flatten()
                .filter(|s| !s.is_empty());
            let global_perm = prefs
                .get(crate::local_mode::SETTINGS_KEY_CLAUDE_DEFAULT_PERMISSION_MODE)
                .ok()
                .flatten()
                .filter(|s| !s.is_empty());
            let inherited_model = crate::shell::companion_settings::resolve_inherited(
                project_model.as_deref(),
                global_model.as_deref(),
                scope,
            );
            let inherited_perm = crate::shell::companion_settings::resolve_inherited(
                project_perm.as_deref(),
                global_perm.as_deref(),
                scope,
            );
            let effective_model = chat_model.clone().or_else(|| inherited_model.clone());
            let effective_perm = chat_perm.clone().or_else(|| inherited_perm.clone());
            PickerPersisted {
                chat_model,
                chat_perm,
                inherited_model,
                inherited_perm,
                effective_model,
                effective_perm,
            }
        })
    };

    // Re-bind the active session whenever cwd or session changes, OR
    // when any of the three settings tiers (chat / project / global)
    // change underneath us — `picker_persisted.read()` below subscribes
    // to all three. After binding, kick off `ensure_session_bridge` so
    // subsequent turns spawn claude with the inline-permission-prompt
    // MCP wired up. The async work is fire-and-forget — bind_session
    // and the bridge are both idempotent.
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
            // Pull the layered effective values (chat → project →
            // global) so the spawn path sees the right `--model` /
            // `--permission-mode`. Subscribing here is what re-fires
            // this effect when the project- or global-tier picker
            // bumps the corresponding GlobalSignal.
            let pp = picker_persisted.read().clone();
            match (sid, cwd) {
                (Some(sid), Some(cwd)) => {
                    let cwd_for_bind = cwd.clone();
                    let backend_for_bind = backend.clone();
                    let claude_plugin_for_restore = claude_plugin.clone();
                    let effective_model = pp.effective_model.clone();
                    let effective_perm = pp.effective_perm.clone();
                    spawn(async move {
                        if let Err(e) = backend_for_bind.bind_session(sid, cwd_for_bind).await {
                            tracing::warn!(
                                target: "operon::companion",
                                "bind_session({sid}): {e}"
                            );
                            return;
                        }
                        // Push the layered effective value into the
                        // plugin so the next `spawn_turn` forwards the
                        // right `--model` / `--permission-mode`. Setting
                        // these after `bind_session` guarantees the
                        // binding exists (the setters are no-ops
                        // otherwise).
                        claude_plugin_for_restore.set_session_model(sid, effective_model);
                        claude_plugin_for_restore
                            .set_session_permission_mode(sid, effective_perm);
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
        let mode = plugin_arc.current_permission_mode();
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
                    if let Some(sid) = active_session {
                        {
                            // Chat header pickers write ONLY to the
                            // chat tier (`chat_session.{model,
                            // permission_mode}`). Picking "Inherit"
                            // stores NULL there and the spawn path
                            // falls through to project → global. The
                            // dropdown's first option is labelled with
                            // the live inherited value so the user can
                            // see what they'd inherit before
                            // committing.
                            let pp = picker_persisted.read().clone();
                            let session_repo_for_model = session_repo.clone();
                            let session_repo_for_perm = session_repo.clone();
                            let mut session_version_for_model = session_version;
                            let mut session_version_for_perm = session_version;
                            let inherit_model_label = match pp.inherited_model.as_deref() {
                                Some(id) => format!("Inherit ({})", model_display(id)),
                                None => "Inherit (Claude default)".to_string(),
                            };
                            let inherit_perm_label = match pp.inherited_perm.as_deref() {
                                Some(id) => format!("Inherit ({})", perm_display(id)),
                                None => "Inherit (Claude default)".to_string(),
                            };
                            let chat_model = pp.chat_model.clone();
                            let chat_perm = pp.chat_perm.clone();
                            rsx! {
                            label { class: "operon-companion-toolbar-label",
                                title: "Model used for new turns (chat-level override)",
                                span { class: "sr-only", "Model" }
                                select {
                                    class: "operon-companion-model-picker",
                                    "data-testid": "companion-model-picker",
                                    onchange: move |e| {
                                        let v = e.value();
                                        let next = if v == "inherit" { None } else { Some(v) };
                                        if let Err(e) = session_repo_for_model
                                            .set_model(sid, next.as_deref())
                                        {
                                            tracing::warn!(
                                                target: "operon::companion",
                                                "persist chat model failed: {e}"
                                            );
                                        }
                                        session_version_for_model.with_mut(|v| *v += 1);
                                    },
                                    option { value: "inherit",
                                        selected: chat_model.is_none(),
                                        "{inherit_model_label}"
                                    }
                                    option { value: "claude-opus-4-7",
                                        selected: chat_model.as_deref() == Some("claude-opus-4-7"),
                                        "Opus 4.7"
                                    }
                                    option { value: "claude-opus-4-6",
                                        selected: chat_model.as_deref() == Some("claude-opus-4-6"),
                                        "Opus 4.6"
                                    }
                                    option { value: "claude-sonnet-4-6",
                                        selected: chat_model.as_deref() == Some("claude-sonnet-4-6"),
                                        "Sonnet 4.6"
                                    }
                                    option { value: "claude-sonnet-4-5",
                                        selected: chat_model.as_deref() == Some("claude-sonnet-4-5"),
                                        "Sonnet 4.5"
                                    }
                                    option { value: "claude-haiku-4-5",
                                        selected: chat_model.as_deref() == Some("claude-haiku-4-5"),
                                        "Haiku 4.5"
                                    }
                                    option { value: "claude-3-5-sonnet-20241022",
                                        selected: chat_model.as_deref() == Some("claude-3-5-sonnet-20241022"),
                                        "Sonnet 3.5 (2024-10-22)"
                                    }
                                    option { value: "claude-3-5-haiku-20241022",
                                        selected: chat_model.as_deref() == Some("claude-3-5-haiku-20241022"),
                                        "Haiku 3.5 (2024-10-22)"
                                    }
                                    option { value: "claude-3-opus-20240229",
                                        selected: chat_model.as_deref() == Some("claude-3-opus-20240229"),
                                        "Opus 3 (2024-02-29)"
                                    }
                                }
                            }
                            label { class: "operon-companion-toolbar-label",
                                title: "claude --permission-mode (chat-level override)",
                                span { class: "sr-only", "Permission mode" }
                                select {
                                    class: "operon-companion-model-picker",
                                    "data-testid": "companion-permission-picker",
                                    onchange: move |e| {
                                        let v = e.value();
                                        let next = if v == "inherit" { None } else { Some(v) };
                                        if let Err(e) = session_repo_for_perm
                                            .set_permission_mode(sid, next.as_deref())
                                        {
                                            tracing::warn!(
                                                target: "operon::companion",
                                                "persist chat permission_mode failed: {e}"
                                            );
                                        }
                                        session_version_for_perm.with_mut(|v| *v += 1);
                                    },
                                    option { value: "inherit",
                                        selected: chat_perm.is_none(),
                                        "{inherit_perm_label}"
                                    }
                                    option { value: "acceptEdits",
                                        selected: chat_perm.as_deref() == Some("acceptEdits"),
                                        "Accept edits"
                                    }
                                    option { value: "plan",
                                        selected: chat_perm.as_deref() == Some("plan"),
                                        "Plan"
                                    }
                                    option { value: "bypassPermissions",
                                        selected: chat_perm.as_deref() == Some("bypassPermissions"),
                                        "Bypass"
                                    }
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
                        {render_item(i, item, note_link_resolver)}
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
                            note_link_resolver,
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
                        // Chat-turn-level in-flight signal. Fires the
                        // moment Send is clicked and stays true until
                        // `Done` (or error). Without this, there's a
                        // gap between Send-click and the first text
                        // delta where the user sees no feedback that
                        // Claude is actually working — confusing on
                        // slow first tokens or while the prompt is
                        // being uploaded.
                        let chat_turn_in_flight = *in_flight.read();
                        rsx! {
                            {
                                match (inprogress, is_running, chat_turn_in_flight) {
                                    (Some(text), _, _) => rsx! {
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
                                    (None, true, _) => rsx! {
                                        div {
                                            class: "operon-companion-msg operon-companion-msg-thinking",
                                            "data-testid": "companion-thinking",
                                            span { class: "operon-companion-thinking-spinner" }
                                            span { class: "operon-companion-thinking-label",
                                                "Claude is thinking\u{2026}"
                                            }
                                        }
                                    },
                                    (None, false, true) => rsx! {
                                        div {
                                            class: "operon-companion-msg operon-companion-msg-thinking",
                                            "data-testid": "companion-working",
                                            span { class: "operon-companion-thinking-spinner" }
                                            span { class: "operon-companion-thinking-label",
                                                "Claude is working\u{2026}"
                                            }
                                        }
                                    },
                                    (None, false, false) => rsx! {},
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
                        if let Some(picker) = mention_picker.read().clone() {
                            ul {
                                class: "operon-companion-slash-popover",
                                "data-testid": "companion-mention-popover",
                                for (idx, (uuid, title)) in picker.candidates.iter().enumerate() {
                                    {
                                        let uuid = *uuid;
                                        let title = title.clone();
                                        let at_off = picker.at_byte_offset;
                                        let is_selected = idx == picker.selected;
                                        rsx! {
                                            li {
                                                key: "{uuid}",
                                                button {
                                                    r#type: "button",
                                                    class: if is_selected {
                                                        "operon-companion-slash-item operon-companion-slash-item-selected"
                                                    } else {
                                                        "operon-companion-slash-item"
                                                    },
                                                    "data-testid": "companion-mention-candidate",
                                                    onmousedown: move |evt| {
                                                        // mousedown not click so we
                                                        // splice before the textarea
                                                        // loses focus (which would
                                                        // otherwise close the picker
                                                        // via the next render).
                                                        evt.prevent_default();
                                                        let token = format_mention_token(&title, uuid);
                                                        let cur = composer.read().clone();
                                                        let mut next = String::with_capacity(cur.len() + token.len());
                                                        next.push_str(&cur[..at_off]);
                                                        next.push_str(&token);
                                                        composer.set(next);
                                                        mention_picker.set(None);
                                                    },
                                                    "{title}"
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
                        oninput: {
                            let oninput_note_repo = note_repo_for_mentions.clone();
                            let oninput_project_repo = project_repo_for_mentions.clone();
                            move |e: Event<FormData>| {
                                composer.set(e.value());
                                // Drive the @ autocomplete picker off
                                // the trailing portion of the composer.
                                // Detection is purely textual — no
                                // caret-position dependency — so it
                                // works across browsers / textarea
                                // selection quirks.
                                let text = composer.read().clone();
                                let trigger = detect_trailing_mention(&text);
                                match (trigger, oninput_note_repo.as_ref()) {
                                    (Some((at_off, q)), Some(nrepo)) => {
                                        let scope_now = *scope_signal.read();
                                        let candidates = list_mention_candidates(
                                            &q,
                                            nrepo,
                                            oninput_project_repo.as_ref(),
                                            scope_now,
                                        );
                                        if candidates.is_empty() {
                                            mention_picker.set(None);
                                        } else {
                                            mention_picker.set(Some(MentionPickerState {
                                                query: q,
                                                candidates,
                                                selected: 0,
                                                at_byte_offset: at_off,
                                            }));
                                        }
                                    }
                                    _ => {
                                        mention_picker.set(None);
                                    }
                                }
                            }
                        },
                        ondragover: move |evt| evt.prevent_default(),
                        ondrop: {
                            let drop_note_repo = note_repo_for_mentions.clone();
                            let drop_project_repo = project_repo_for_mentions.clone();
                            let drag_session_opt = drag_session_for_drop;
                            move |evt: Event<DragData>| {
                                // Only handle in-app note drags from
                                // the explorer; ignore other drops
                                // (text, files) and let the textarea's
                                // native behavior take over.
                                let Some(mut drag_session) = drag_session_opt else {
                                    return;
                                };
                                let kind = *drag_session.peek();
                                let note_id = match kind {
                                    Some(crate::local_mode::ui::DragKind::Note(id)) => id,
                                    _ => return,
                                };
                                evt.prevent_default();
                                let Some(note_repo) = drop_note_repo.as_ref() else {
                                    drag_session.set(None);
                                    return;
                                };
                                let title = lookup_note_title(
                                    note_repo,
                                    drop_project_repo.as_ref(),
                                    note_id,
                                )
                                .unwrap_or_else(|| note_id.to_string());
                                let token = format_mention_token(&title, note_id);
                                let cur = composer.read().clone();
                                composer.set(append_to_composer(&cur, &token));
                                drag_session.set(None);
                            }
                        },
                        onkeydown: {
                            let repo = message_repo.clone();
                            let srepo = session_repo.clone();
                            let note_repo = note_repo_for_mentions.clone();
                            let persistence = persistence_for_mentions.clone();
                            move |e: KeyboardEvent| {
                                if !has_cwd || !has_session { return; }
                                // Mention-picker navigation takes
                                // precedence over plain typing /
                                // send. When the picker is open, the
                                // keys we own do NOT propagate to the
                                // textarea so e.g. ArrowDown doesn't
                                // move the caret to the next line.
                                let picker_open = mention_picker.read().is_some();
                                if picker_open {
                                    match e.key() {
                                        Key::Escape => {
                                            mention_picker.set(None);
                                            e.prevent_default();
                                            return;
                                        }
                                        Key::ArrowDown => {
                                            let mut st = mention_picker.read().clone();
                                            if let Some(s) = st.as_mut() {
                                                if !s.candidates.is_empty() {
                                                    s.selected =
                                                        (s.selected + 1) % s.candidates.len();
                                                }
                                            }
                                            mention_picker.set(st);
                                            e.prevent_default();
                                            return;
                                        }
                                        Key::ArrowUp => {
                                            let mut st = mention_picker.read().clone();
                                            if let Some(s) = st.as_mut() {
                                                let n = s.candidates.len();
                                                if n > 0 {
                                                    s.selected = (s.selected + n - 1) % n;
                                                }
                                            }
                                            mention_picker.set(st);
                                            e.prevent_default();
                                            return;
                                        }
                                        Key::Enter | Key::Tab => {
                                            // Don't accept on plain
                                            // Enter when the picker
                                            // is closed-but-stale; we
                                            // already checked
                                            // picker_open above.
                                            let st = mention_picker.read().clone();
                                            if let Some(s) = st {
                                                if let Some((uuid, title)) =
                                                    s.candidates.get(s.selected).cloned()
                                                {
                                                    let token =
                                                        format_mention_token(&title, uuid);
                                                    let cur = composer.read().clone();
                                                    let mut next = String::with_capacity(
                                                        cur.len() + token.len(),
                                                    );
                                                    next.push_str(&cur[..s.at_byte_offset]);
                                                    next.push_str(&token);
                                                    composer.set(next);
                                                    mention_picker.set(None);
                                                    e.prevent_default();
                                                    return;
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                if e.key() == Key::Enter && (e.modifiers().ctrl() || e.modifiers().meta()) {
                                    if let Some(sid) = active_session {
                                        let scope_now = *scope_signal.read();
                                        let vault_notes_now =
                                            vault_root.read().as_ref().map(|v| v.notes_dir());
                                        run_turn(
                                            active_backend, sid, transcript, composer,
                                            in_flight, active_ct, usage_total,
                                            pending_assistant, repo.clone(), srepo.clone(),
                                            session_version, note_repo.clone(),
                                            persistence.clone(), scope_now, vault_notes_now,
                                        );
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
                            let note_repo = note_repo_for_mentions.clone();
                            let persistence = persistence_for_mentions.clone();
                            move |_| {
                                if let Some(sid) = active_session {
                                    let scope_now = *scope_signal.read();
                                    let vault_notes_now =
                                        vault_root.read().as_ref().map(|v| v.notes_dir());
                                    run_turn(
                                        active_backend, sid, transcript, composer,
                                        in_flight, active_ct, usage_total,
                                        pending_assistant, repo.clone(), srepo.clone(),
                                        session_version, note_repo.clone(),
                                        persistence.clone(), scope_now, vault_notes_now,
                                    );
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

fn render_user_segments(
    text: &str,
    resolver: Option<crate::plugins::markdown::render::NoteLinkResolver>,
) -> Vec<Element> {
    let re = mention_link_regex();
    let mut out: Vec<Element> = Vec::new();
    let mut last = 0usize;
    let mut idx = 0usize;
    for cap in re.captures_iter(text) {
        let m = cap.get(0).unwrap();
        if m.start() > last {
            let s = text[last..m.start()].to_string();
            let k = format!("ut-{idx}");
            out.push(rsx! { span { key: "{k}", "{s}" } });
            idx += 1;
        }
        let title = cap.get(1).map(|x| x.as_str().to_string()).unwrap_or_default();
        let note_id_str = cap.get(2).map(|x| x.as_str().to_string()).unwrap_or_default();
        let parsed_uuid = Uuid::parse_str(&note_id_str).ok();
        let k = format!("um-{idx}");
        // When a `NoteLinkResolver` is available AND the uuid parses,
        // the chip becomes clickable: the resolver opens the note in a
        // tab and writes to `RevealNoteRequest`, which the explorer's
        // reveal effect picks up to expand the owning project + walk
        // ancestors. Tests / sandboxed previews without the resolver
        // get the plain-span fallback.
        match (resolver, parsed_uuid) {
            (Some(crate::plugins::markdown::render::NoteLinkResolver(cb)), Some(uuid)) => {
                out.push(rsx! {
                    span {
                        key: "{k}",
                        class: "operon-mention-chip operon-mention-chip-clickable",
                        "data-note-id": "{note_id_str}",
                        role: "button",
                        tabindex: "0",
                        onclick: move |_| { cb.call(uuid); },
                        onkeydown: move |evt| {
                            let key = evt.key().to_string();
                            if key == "Enter" || key == " " {
                                evt.prevent_default();
                                cb.call(uuid);
                            }
                        },
                        "{title}"
                    }
                });
            }
            _ => {
                out.push(rsx! {
                    span {
                        key: "{k}",
                        class: "operon-mention-chip",
                        "data-note-id": "{note_id_str}",
                        "{title}"
                    }
                });
            }
        }
        idx += 1;
        last = m.end();
    }
    if last < text.len() {
        let s = text[last..].to_string();
        let k = format!("ut-{idx}");
        out.push(rsx! { span { key: "{k}", "{s}" } });
    }
    out
}

fn render_item(
    i: usize,
    item: &TranscriptItem,
    resolver: Option<crate::plugins::markdown::render::NoteLinkResolver>,
) -> Element {
    let key = format!("{i}");
    match item {
        TranscriptItem::UserText(t) => rsx! {
            div {
                key: "{key}",
                class: "operon-companion-msg operon-companion-msg-user",
                "data-role": "user",
                {render_user_segments(t, resolver).into_iter()}
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

/// Result of resolving one referenced-note UUID at send-time.
///
/// Produced by the lookup closure passed to
/// [`build_mention_inlined_prompt`]. The rewriter doesn't care where
/// title/body/path come from — production wires this to the
/// `LocalNoteRepository` + `Persistence` context; tests use a static
/// in-memory map.
#[derive(Debug, Clone)]
pub struct ResolvedNote {
    /// Display title used in the inlined block header.
    pub title: String,
    /// Note body inlined verbatim under the block header.
    pub body: String,
    /// Path Claude should pass to `Write` / `Edit` to modify this
    /// note. Prefer absolute (`<vault>/notes/<uuid>`) so the same
    /// path works regardless of which `cwd` the chat session was
    /// spawned with. Falls back to `notes/<uuid>` when the vault
    /// root isn't available.
    pub path: String,
}

fn mention_link_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"@\[([^\]]*)\]\(note:([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})\)",
        )
        .expect("mention_link_regex compiles")
    })
}

fn mention_bare_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"\bnote:([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})\b",
        )
        .expect("mention_bare_regex compiles")
    })
}

const MENTION_SYSTEM_PROMPT_PREAMBLE: &str = "\
You are working in a project where the user can attach notes to the chat \
via `@[<title>](note:<uuid>)` mentions. When the user mentions a note, its \
full body is inlined below under `--- referenced note ---` blocks. If the \
user asks you to modify a mentioned note, edit the file at the `path` \
shown in that note's header (relative to the current working directory) \
using your `Write` or `Edit` tool. The app watches that directory and \
will pick up your changes automatically; you do not need a custom tool to \
\"save\" the note.";

/// Extract every unique referenced-note UUID from `user_text` in the
/// order it first appears. Both the structured form
/// `@[<title>](note:<uuid>)` and the bare form `note:<uuid>` are
/// scanned. UUID syntax that fails to parse is silently skipped.
pub fn extract_mention_uuids(user_text: &str) -> Vec<Uuid> {
    let mut seen: Vec<Uuid> = Vec::new();
    let mut push = |u: Uuid| {
        if !seen.contains(&u) {
            seen.push(u);
        }
    };
    for cap in mention_link_regex().captures_iter(user_text) {
        if let Ok(u) = Uuid::parse_str(&cap[2]) {
            push(u);
        }
    }
    for cap in mention_bare_regex().captures_iter(user_text) {
        if let Ok(u) = Uuid::parse_str(&cap[1]) {
            push(u);
        }
    }
    seen
}

/// Rewrite the user's composer text into the prompt actually sent to
/// Claude: inlines every referenced note's body under
/// `--- referenced note: <title> (id: <uuid>, path: <path>) ---`
/// blocks, prepends a short preamble teaching Claude how to edit the
/// notes via `Write`/`Edit`, and leaves the original
/// `@[..](note:..)` tokens in the user text intact so Claude can map
/// each mention back to its inlined block.
///
/// `lookup` returns `None` for any UUID that doesn't resolve in the
/// current scope (deleted, wrong project, etc.); the block for that
/// UUID becomes a one-line
/// `_(referenced note <uuid> not found in current scope)_`
/// placeholder so Claude knows the link broke without aborting.
///
/// When zero mentions are present, `user_text` is returned verbatim
/// (no preamble, no blocks) so plain-chat turns stay cache-friendly.
pub fn build_mention_inlined_prompt<F>(user_text: &str, lookup: F) -> String
where
    F: Fn(Uuid) -> Option<ResolvedNote>,
{
    let uuids = extract_mention_uuids(user_text);
    if uuids.is_empty() {
        return user_text.to_string();
    }

    let mut out = String::with_capacity(user_text.len() + 2048);
    out.push_str(MENTION_SYSTEM_PROMPT_PREAMBLE);
    out.push_str(
        "\n\n--- referenced notes (the user mentioned these; bodies inlined for context) ---\n\n",
    );

    for uuid in &uuids {
        match lookup(*uuid) {
            Some(note) => {
                out.push_str(&format!(
                    "--- referenced note: {} (id: {}, path: {}) ---\n",
                    note.title, uuid, note.path,
                ));
                out.push_str(&note.body);
                if !note.body.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(&format!("--- end note: {} ---\n\n", note.title));
            }
            None => {
                out.push_str(&format!(
                    "_(referenced note {} not found in current scope)_\n\n",
                    uuid,
                ));
            }
        }
    }

    out.push_str("--- end referenced notes ---\n\n");
    out.push_str(user_text);
    out
}

/// State for the composer's `@` autocomplete picker. Open when the
/// user is in the middle of typing a mention; cleared on selection,
/// dismissal, or when the trailing `@<query>` is invalidated by a
/// terminating character (space, bracket, etc.).
#[derive(Debug, Clone)]
struct MentionPickerState {
    /// Chars typed after the `@` so far. Filters the candidate list
    /// (case-insensitive substring match on note title).
    #[allow(dead_code)] // currently only used at construction; kept for
    // future "highlight matching chars in the candidate row" UX.
    query: String,
    /// Notes matching `query` in the current scope, capped at the top
    /// few. `selected` indexes into this list.
    candidates: Vec<(Uuid, String)>,
    /// Currently-highlighted candidate index. Wraps at the bounds.
    selected: usize,
    /// Byte offset of the triggering `@` within the composer text.
    /// Used when the user accepts a candidate so we can replace the
    /// `@<query>` span with the full mention token.
    at_byte_offset: usize,
}

/// Walk backward from end of `text` looking for an "open" `@`
/// mention — the last `@` whose chars-after are a valid in-progress
/// query (no whitespace, no bracket/paren that would close a token).
/// Returns `(byte offset of @, query chars after @)`.
///
/// The `@` must be at the start of `text` or preceded by whitespace;
/// `@` embedded in a word (e.g. `user@email`) is NOT a mention
/// trigger.
pub fn detect_trailing_mention(text: &str) -> Option<(usize, String)> {
    let mut at_idx: Option<usize> = None;
    for (i, ch) in text.char_indices().rev() {
        if ch == '@' {
            at_idx = Some(i);
            break;
        }
        if ch.is_whitespace() || matches!(ch, '[' | ']' | '(' | ')') {
            return None;
        }
    }
    let i = at_idx?;
    let prev_char = text[..i].chars().rev().next();
    match prev_char {
        None => {}
        Some(c) if c.is_whitespace() => {}
        _ => return None,
    }
    let query = text[i + 1..].to_string();
    Some((i, query))
}

/// Filter the notes visible in `scope` by case-insensitive title
/// substring match against `query`. Empty query lists the first few
/// notes alphabetically (driven by repo order; whatever the repo
/// returns). Caps at 8 results so the popover stays scannable.
fn list_mention_candidates(
    query: &str,
    note_repo: &Arc<dyn operon_store::repos::LocalNoteRepository>,
    project_repo: Option<&Arc<dyn operon_store::repos::LocalProjectRepository>>,
    scope: ChatScope,
) -> Vec<(Uuid, String)> {
    let query_lower = query.to_lowercase();
    let notes: Vec<operon_store::repos::LocalNote> = match scope {
        ChatScope::Project(pid) => note_repo.list_for_project(pid).unwrap_or_default(),
        ChatScope::Vault => {
            let mut all = Vec::new();
            if let Some(prepo) = project_repo {
                if let Ok(projects) = prepo.list() {
                    for p in projects {
                        if let Ok(notes) = note_repo.list_for_project(p.id) {
                            all.extend(notes);
                        }
                    }
                }
            }
            all
        }
    };
    notes
        .into_iter()
        .filter(|n| query.is_empty() || n.title.to_lowercase().contains(&query_lower))
        .take(8)
        .map(|n| (n.id, n.title))
        .collect()
}

/// Resolve a note UUID to its display title. Tries
/// `find_project_for_note` first (O(1) on the SQLite repo); falls
/// back to scanning every project's note list (works on repos that
/// don't override the default `find_project_for_note`). Returns
/// `None` when the note isn't found in any project — callers fall
/// back to displaying the bare UUID.
fn lookup_note_title(
    note_repo: &Arc<dyn operon_store::repos::LocalNoteRepository>,
    project_repo: Option<&Arc<dyn operon_store::repos::LocalProjectRepository>>,
    note_id: Uuid,
) -> Option<String> {
    if let Ok(Some(pid)) = note_repo.find_project_for_note(note_id) {
        if let Ok(notes) = note_repo.list_for_project(pid) {
            if let Some(n) = notes.into_iter().find(|n| n.id == note_id) {
                return Some(n.title);
            }
        }
    }
    let project_repo = project_repo?;
    let projects = project_repo.list().ok()?;
    for p in projects {
        if let Ok(notes) = note_repo.list_for_project(p.id) {
            if let Some(n) = notes.into_iter().find(|n| n.id == note_id) {
                return Some(n.title);
            }
        }
    }
    None
}

/// Format the standard mention token. Centralised so picker /
/// drag-drop / right-click all emit identical text.
fn format_mention_token(title: &str, note_id: Uuid) -> String {
    format!("@[{title}](note:{note_id})")
}

/// Append `addition` to `current`, prefixing a space when `current`
/// is non-empty and doesn't already end with whitespace. Used by the
/// drag-drop and right-click append paths so a mention token doesn't
/// run into the previous word.
fn append_to_composer(current: &str, addition: &str) -> String {
    if current.is_empty() {
        addition.to_string()
    } else if current.ends_with(' ') || current.ends_with('\n') {
        format!("{current}{addition}")
    } else {
        format!("{current} {addition}")
    }
}

/// Async wrapper that walks every referenced-note UUID in `user_text`,
/// resolves each to a [`ResolvedNote`] via the in-scope note repo +
/// persistence, and returns the prompt text Claude should actually
/// receive (mention bodies inlined under `--- referenced note ---`
/// blocks; original tokens left intact in the user line).
///
/// Returns `user_text` verbatim when:
/// - the chat has no `LocalNoteRepository` / `Persistence` context
///   (e.g. tests / standalone harness),
/// - or no mentions are present in the text.
///
/// UUIDs that resolve but whose body fails to load fall through to
/// the rewriter's "not found in current scope" placeholder so the
/// turn still goes through.
async fn resolve_mentions_for_prompt(
    user_text: &str,
    note_repo: Option<Arc<dyn operon_store::repos::LocalNoteRepository>>,
    persistence: Option<Arc<dyn crate::persistence::Persistence>>,
    scope: ChatScope,
    vault_notes_dir: Option<PathBuf>,
) -> String {
    let (Some(note_repo), Some(persistence)) = (note_repo, persistence) else {
        return user_text.to_string();
    };
    let uuids = extract_mention_uuids(user_text);
    if uuids.is_empty() {
        return user_text.to_string();
    }
    let mut resolved: std::collections::HashMap<Uuid, ResolvedNote> =
        std::collections::HashMap::new();
    for u in uuids {
        let project_id = match scope {
            ChatScope::Project(pid) => Some(pid),
            ChatScope::Vault => note_repo.find_project_for_note(u).ok().flatten(),
        };
        let title = project_id
            .and_then(|pid| note_repo.list_for_project(pid).ok())
            .and_then(|notes| notes.into_iter().find(|n| n.id == u).map(|n| n.title))
            .unwrap_or_else(|| u.to_string());
        let body = match persistence.load(&u.to_string()).await {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(s) => s,
                Err(_) => "(non-UTF-8 body — not inlined)".to_string(),
            },
            Err(_) => continue, // leave unresolved → rewriter emits placeholder
        };
        let path = match vault_notes_dir.as_ref() {
            Some(dir) => dir.join(u.to_string()).to_string_lossy().to_string(),
            None => format!("notes/{u}"),
        };
        resolved.insert(u, ResolvedNote { title, body, path });
    }
    build_mention_inlined_prompt(user_text, |u| resolved.get(&u).cloned())
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
    note_repo: Option<Arc<dyn operon_store::repos::LocalNoteRepository>>,
    persistence: Option<Arc<dyn crate::persistence::Persistence>>,
    scope: ChatScope,
    vault_notes_dir: Option<PathBuf>,
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
        // Resolve any `@[<title>](note:<uuid>)` mentions in `text`
        // into inlined-body blocks before send. Transcript + persisted
        // user line use the raw `text` (what the user sees); Claude
        // receives the rewritten version with bodies.
        let prompt_for_claude = resolve_mentions_for_prompt(
            &text,
            note_repo,
            persistence,
            scope,
            vault_notes_dir,
        )
        .await;
        let mut rx = match backend_arc.send_rich(prompt_for_claude, chat_session, ct).await {
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
mod mention_tests {
    use super::*;
    use std::collections::HashMap;

    fn uuid(s: &str) -> Uuid {
        Uuid::parse_str(s).unwrap()
    }

    fn make_lookup(
        entries: Vec<(Uuid, &str, &str, &str)>,
    ) -> impl Fn(Uuid) -> Option<ResolvedNote> {
        let map: HashMap<Uuid, ResolvedNote> = entries
            .into_iter()
            .map(|(u, title, body, path)| {
                (
                    u,
                    ResolvedNote {
                        title: title.into(),
                        body: body.into(),
                        path: path.into(),
                    },
                )
            })
            .collect();
        move |u: Uuid| map.get(&u).cloned()
    }

    #[test]
    fn resolves_structured_mention() {
        let u = uuid("550e8400-e29b-41d4-a716-446655440000");
        let text = format!("Summarize @[note-A](note:{u}) for me");
        let lookup = make_lookup(vec![(u, "note-A", "body-of-note-A", "notes/550e...")]);
        let out = build_mention_inlined_prompt(&text, lookup);
        assert!(out.contains(MENTION_SYSTEM_PROMPT_PREAMBLE));
        assert!(out.contains("--- referenced note: note-A (id:"));
        assert!(out.contains("body-of-note-A"));
        assert!(out.contains("--- end note: note-A ---"));
        assert!(
            out.ends_with(&text),
            "original user text should be preserved at the end"
        );
    }

    #[test]
    fn resolves_bare_uuid_mention() {
        let u = uuid("11111111-2222-3333-4444-555555555555");
        let text = format!("Look at note:{u} please");
        let lookup = make_lookup(vec![(u, "bare-note", "BARE-BODY", "notes/1111...")]);
        let out = build_mention_inlined_prompt(&text, lookup);
        assert!(out.contains("--- referenced note: bare-note"));
        assert!(out.contains("BARE-BODY"));
    }

    #[test]
    fn missing_uuid_emits_placeholder_without_aborting() {
        let u = uuid("00000000-0000-0000-0000-000000000000");
        let text = format!("Modify @[ghost](note:{u}) thanks");
        let lookup = make_lookup(vec![]);
        let out = build_mention_inlined_prompt(&text, lookup);
        assert!(out.contains(&format!(
            "_(referenced note {u} not found in current scope)_"
        )));
        // System-prompt preamble + the original text are still emitted.
        assert!(out.contains(MENTION_SYSTEM_PROMPT_PREAMBLE));
        assert!(out.ends_with(&text));
    }

    #[test]
    fn duplicate_mention_dedupes_to_one_block() {
        let u = uuid("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
        let text = format!("First @[x](note:{u}) and again note:{u} ok?");
        let lookup = make_lookup(vec![(u, "x", "ONE", "notes/aaaa...")]);
        let out = build_mention_inlined_prompt(&text, lookup);
        let block_count = out.matches("--- referenced note: x").count();
        assert_eq!(block_count, 1, "duplicates of the same UUID inline once");
    }

    #[test]
    fn zero_mentions_passes_text_through_unchanged() {
        let text = "just a normal message, no mentions here";
        let out = build_mention_inlined_prompt(text, |_| None);
        assert_eq!(out, text);
    }

    #[test]
    fn extract_mention_uuids_preserves_first_seen_order() {
        let a = uuid("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa");
        let b = uuid("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb");
        let c = uuid("cccccccc-cccc-cccc-cccc-cccccccccccc");
        let text = format!("@[B](note:{b}) then @[A](note:{a}) then note:{c} and note:{a}");
        let uuids = extract_mention_uuids(&text);
        assert_eq!(uuids, vec![b, a, c]);
    }

    #[test]
    fn detect_trailing_mention_opens_at_end() {
        let (at, query) = detect_trailing_mention("Look at @foo").unwrap();
        assert_eq!(at, 8);
        assert_eq!(query, "foo");
    }

    #[test]
    fn detect_trailing_mention_empty_query_just_after_at() {
        let (at, query) = detect_trailing_mention("Look at @").unwrap();
        assert_eq!(at, 8);
        assert_eq!(query, "");
    }

    #[test]
    fn detect_trailing_mention_at_start_of_text() {
        let (at, query) = detect_trailing_mention("@foo").unwrap();
        assert_eq!(at, 0);
        assert_eq!(query, "foo");
    }

    #[test]
    fn detect_trailing_mention_returns_none_after_space() {
        // Space after `@foo` terminates the open mention.
        assert!(detect_trailing_mention("Look at @foo bar").is_none());
    }

    #[test]
    fn detect_trailing_mention_returns_none_when_at_is_word_embedded() {
        // `user@email` is not a mention trigger.
        assert!(detect_trailing_mention("contact user@email").is_none());
    }

    #[test]
    fn detect_trailing_mention_returns_none_after_close_bracket() {
        // The mention token is already closed.
        assert!(detect_trailing_mention("Look at @[foo](note:x)").is_none());
    }

    #[test]
    fn extract_mention_uuids_handles_no_matches() {
        assert!(extract_mention_uuids("just text").is_empty());
        // Wrong format — bracket without `note:` prefix isn't a mention.
        assert!(extract_mention_uuids("@[foo](http://example.com)").is_empty());
        // UUID-like but missing `note:` prefix and word boundary fails.
        assert!(
            extract_mention_uuids("550e8400-e29b-41d4-a716-446655440000").is_empty()
        );
    }
}

