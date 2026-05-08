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
use futures::StreamExt;
use operon_core::traits::Usage;
use operon_plugins_claude_code::ClaudeCodeEvent;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

use crate::agent::plugins::{ClaudeCodeChatPlugin, ClaudeCodeConfig};
use crate::agent::CancellationToken;
use crate::local_mode::desktop::{CurrentVaultRoot, LocalProjectRepo};
use crate::local_mode::explorer::LocalProjectVersion;
use crate::plugins::markdown::MarkdownView;
use crate::shell::companion_state::{
    ActiveChatScope, ActiveChatSession, ActiveRepoPath, ChatMessage, ChatMessageKind,
    ChatMessageRepo, ChatScope, ChatSessionRepo, CompanionComposerInbox, CHAT_MESSAGE_VERSION,
};
use crate::shell::session_rail::SessionRail;
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
            })),
        }
    });

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
    {
        let plugin_for_effect = plugin;
        let session = session_signal;
        let cwd = cwd_for_scope;
        use_effect(move || {
            let plugin = plugin_for_effect.read();
            let sid = *session.read();
            let cwd = cwd.read().clone();
            match (sid, cwd) {
                (Some(sid), Some(cwd)) => plugin.bind_session(sid, cwd),
                (Some(sid), None) => plugin.unbind_session(sid),
                _ => {}
            }
        });
    }

    // Reset transcript + cost on session switch, then replay any
    // persisted history for the newly-active session. Cost meter doesn't
    // restore from disk (deferred — needs per-session usage column).
    //
    // Phase D: also re-fire when `CHAT_MESSAGE_VERSION` bumps, which
    // a background drainer (the artifact runner) does after each
    // `chat_message` append. That re-reads the row list and reflects
    // streaming events in the transcript even though we aren't the
    // ones draining the claude stream. Regular companion chats don't
    // bump the version (they update the in-memory transcript directly
    // via `apply_event`), so this watcher is a no-op for them.
    {
        let session = session_signal;
        let mut transcript_setter = transcript;
        let mut usage_setter = usage_total;
        let mut pending_setter = pending_assistant;
        let repo = message_repo.clone();
        use_effect(move || {
            let sid = *session.read();
            // Subscribe to GlobalSignal bumps so this effect re-runs
            // on background appends from the artifact runner.
            let _ = *CHAT_MESSAGE_VERSION.read();
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

    rsx! {
        div { class: "operon-companion-chat-grid",
            SessionRail {}
            section { class: "operon-companion-chat",
                "data-region": "companion-chat",
                div { class: "operon-companion-chat-header",
                    span { class: "operon-companion-chat-title", "" }
                    {
                        let plugin_arc = plugin.read().clone();
                        let current_model = plugin_arc.current_default_model();
                        let current_perm = plugin_arc.current_permission_mode();
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
                                    option { value: "claude-sonnet-4-6",
                                        selected: current_model.as_deref() == Some("claude-sonnet-4-6"),
                                        "Sonnet 4.6"
                                    }
                                    option { value: "claude-haiku-4-5",
                                        selected: current_model.as_deref() == Some("claude-haiku-4-5"),
                                        "Haiku 4.5"
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
                                        plugin_for_perm.set_permission_mode(next);
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
                                        run_turn(plugin, sid, transcript, composer, in_flight, active_ct, usage_total, pending_assistant, repo.clone(), srepo.clone(), session_version);
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
                                    run_turn(plugin, sid, transcript, composer, in_flight, active_ct, usage_total, pending_assistant, repo.clone(), srepo.clone(), session_version);
                                }
                            }
                        },
                        "Send"
                    }
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
    }
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
    plugin: Signal<Arc<ClaudeCodeChatPlugin>>,
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

    let plugin_arc: Arc<ClaudeCodeChatPlugin> = plugin.read().clone();
    let repo_for_task = repo.clone();
    spawn(async move {
        let mut rx = match plugin_arc.send_rich(text, chat_session, ct).await {
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
        while let Some(ev) = rx.next().await {
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
    ev: ClaudeCodeEvent,
) {
    match ev {
        ClaudeCodeEvent::Text(t) => {
            let mut tx = transcript.write();
            if let Some(TranscriptItem::AssistantText(body)) = tx.last_mut() {
                body.push_str(&t);
            } else {
                tx.push(TranscriptItem::AssistantText(t));
            }
            pending_assistant.set(true);
        }
        ClaudeCodeEvent::Thinking(t) => {
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
        ClaudeCodeEvent::ToolUse { id, name, input } => {
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
        ClaudeCodeEvent::ToolResult {
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
        ClaudeCodeEvent::Done { stop_reason: _, usage } => {
            flush_pending_assistant(transcript, pending_assistant, chat_session, repo);
            if let Some(u) = usage {
                let mut total = usage_total.write();
                total.prompt += u.prompt;
                total.prompt_cached += u.prompt_cached;
                total.completion += u.completion;
            }
        }
        ClaudeCodeEvent::Error(msg) => {
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
