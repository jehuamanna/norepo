//! Collapsible card for a single Claude Code tool invocation.
//!
//! Each card shows a one-line summary closed (icon + tool + key argument)
//! and the full input + result on expand. Result rendering is per-tool —
//! `Read`/`Write` show the file body, `Edit` shows the unified diff,
//! `Bash` shows stdout/stderr, `Glob`/`Grep` show match listings, anything
//! else falls back to pretty JSON.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use dioxus::prelude::*;
use operon_core::agent_event::AgentBackend;
use serde_json::Value;

use crate::shell::companion_state::{
    ActiveChatSession, AgentBackendCtx, MCP_LIVE_STATUS, TOOL_STREAM_OUTPUT,
};

/// Elapsed wall-clock threshold at which a pending tool gets a
/// "Cancel turn" affordance. Short enough that a stalled MCP call
/// (figma, etc.) gives the user an escape hatch quickly; long enough
/// that healthy multi-second tool calls don't surface the button.
const STUCK_TOOL_THRESHOLD: Duration = Duration::from_secs(30);

/// Parse `mcp__<server>__<tool>` and return `<server>`. Used by the
/// pending tool card to surface the MCP server's live health when a
/// call hangs ("figma: needs-auth") — answers "is this server even
/// authenticated?" without leaving the chat.
fn mcp_server_name(tool: &str) -> Option<&str> {
    let rest = tool.strip_prefix("mcp__")?;
    rest.split_once("__").map(|(server, _)| server)
}

#[derive(Clone, PartialEq, Debug)]
pub struct ToolResultBody {
    pub content: String,
    pub is_error: bool,
}

#[derive(Props, Clone, PartialEq)]
pub struct ToolCardProps {
    /// claude's tool_use id (matches a later tool_result.tool_use_id)
    pub id: String,
    pub name: String,
    pub input: Value,
    /// `None` means the tool is still running; `Some(_)` means complete.
    pub result: Option<ToolResultBody>,
}

#[component]
pub fn ToolCard(props: ToolCardProps) -> Element {
    let summary = summarize_tool(&props.name, &props.input);
    let icon = tool_icon(&props.name);
    let pending = props.result.is_none();
    let is_error = props.result.as_ref().map(|r| r.is_error).unwrap_or(false);

    let status_class = if pending {
        "operon-tool-card-status-pending"
    } else if is_error {
        "operon-tool-card-status-error"
    } else {
        "operon-tool-card-status-ok"
    };

    // For pending MCP tool calls, surface the *current* server health
    // next to the running badge — only when the server is in a non-OK
    // state, so healthy calls aren't cluttered. Read inside `pending`
    // so completed cards don't drag along a now-stale status.
    let mcp_pill: Option<(String, String)> = if pending {
        mcp_server_name(&props.name).and_then(|server| {
            let status = MCP_LIVE_STATUS
                .read()
                .mcp_servers
                .iter()
                .find(|s| s.name == server)
                .map(|s| s.status.clone());
            match status {
                Some(s) if s != "connected" => Some((server.to_string(), s)),
                _ => None,
            }
        })
    } else {
        None
    };

    rsx! {
        details {
            class: "operon-tool-card",
            "data-testid": "tool-card",
            "data-tool-name": "{props.name}",
            "data-tool-id": "{props.id}",
            "data-pending": if pending { "true" } else { "false" },
            "data-error": if is_error { "true" } else { "false" },
            summary { class: "operon-tool-card-summary",
                span { class: "operon-tool-card-icon", "{icon}" }
                span { class: "operon-tool-card-name", "{props.name}" }
                span { class: "operon-tool-card-arg truncate", "{summary}" }
                if let Some((server, status)) = mcp_pill.as_ref() {
                    span {
                        class: "operon-tool-card-mcp-status",
                        "data-mcp-status": "{status}",
                        "data-testid": "tool-card-mcp-status",
                        "{server}: {status}"
                    }
                }
                span { class: "operon-tool-card-status {status_class}",
                    if pending {
                        "running\u{2026}"
                    } else if is_error {
                        "error"
                    } else {
                        "done"
                    }
                }
            }
            div { class: "operon-tool-card-body",
                ToolBody {
                    name: props.name.clone(),
                    input: props.input.clone(),
                    result: props.result.clone(),
                }
                if pending {
                    PendingToolFooter { id: props.id.clone() }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct PendingToolFooterProps {
    id: String,
}

/// Live region rendered below a running tool's body: streaming output
/// from `TOOL_STREAM_OUTPUT` (runtime backend only — claude-code
/// never writes there), elapsed timer, and a runtime-only Cancel
/// button that calls `backend.cancel_tool(session, id)`.
#[component]
fn PendingToolFooter(props: PendingToolFooterProps) -> Element {
    let active_backend = try_consume_context::<AgentBackendCtx>().map(|c| c.0);
    let active_session_ctx = try_consume_context::<ActiveChatSession>();

    // Tick every 500ms so the elapsed timer keeps refreshing without
    // requiring an explicit signal-bump from upstream.
    let mut tick = use_signal(|| 0u64);
    use_future(move || async move {
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            tick.with_mut(|t| *t = t.wrapping_add(1));
        }
    });
    let _ = tick.read(); // subscribe

    // Read stream + start time. Both are populated by
    // AgentEvent::ToolChunk; if claude-code is the backend they stay
    // empty and we just render the elapsed-time hint.
    let stream = {
        let map = TOOL_STREAM_OUTPUT.read();
        map.get(&props.id).cloned()
    };
    let stdout = stream.as_ref().map(|s| s.stdout.clone()).unwrap_or_default();
    let stderr = stream.as_ref().map(|s| s.stderr.clone()).unwrap_or_default();
    let started_at = stream.as_ref().and_then(|s| s.started_at);
    let elapsed = started_at
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .unwrap_or_default();
    // `started_at` is now populated when the `ToolUse` event arrives
    // (via `mark_tool_started`), regardless of backend. claude-code
    // tools — which never emit `ToolChunk` — still get a timer.
    let elapsed_label = format_elapsed(elapsed);
    let stuck = elapsed >= STUCK_TOOL_THRESHOLD;

    let backend_id = active_backend
        .as_ref()
        .map(|b| b.read().id().to_string())
        .unwrap_or_default();
    let runtime_backend = backend_id == "runtime";

    let id_for_cancel = props.id.clone();
    let cancel_click = move |_| {
        let Some(backend_sig) = active_backend.as_ref() else {
            return;
        };
        let Some(ActiveChatSession(sess_sig)) = active_session_ctx else {
            return;
        };
        let backend: Arc<dyn AgentBackend> = backend_sig.read().clone();
        let Some(session) = *sess_sig.read() else {
            return;
        };
        let id = id_for_cancel.clone();
        spawn(async move {
            let ok = backend.cancel_tool(session, &id).await;
            tracing::info!(
                target: "operon::tool",
                "cancel_tool({id}) on session {session} -> {ok}"
            );
        });
    };

    // Cancel turn: only path to recovery for claude-code (which can't
    // kill an individual MCP request) and a useful escape on runtime
    // too. Wired to the existing `cancel_session_run` helper — the
    // stream drainer's cancellation arm then kills the subprocess
    // and the `Error`-driven `flush_orphan_tools` stamps this card
    // with an error result so it stops spinning.
    let cancel_turn_click = move |_| {
        let Some(ActiveChatSession(sess_sig)) = active_session_ctx else {
            return;
        };
        let Some(session) = *sess_sig.read() else {
            return;
        };
        let ok = crate::shell::companion_state::cancel_session_run(session);
        tracing::info!(
            target: "operon::tool",
            "cancel_session_run({session}) -> {ok}"
        );
    };

    rsx! {
        div {
            class: "operon-tool-card-pending-footer",
            "data-testid": "tool-pending-footer",
            if !stdout.is_empty() {
                pre { class: "operon-tool-card-pre operon-tool-card-stream-stdout",
                    code { class: "md-code-block", "{stdout}" }
                }
            }
            if !stderr.is_empty() {
                pre { class: "operon-tool-card-pre operon-tool-card-stream-stderr",
                    code { class: "md-code-block", "{stderr}" }
                }
            }
            div { class: "operon-tool-card-pending-row",
                span { class: "operon-tool-card-elapsed",
                    "{elapsed_label}"
                }
                if runtime_backend {
                    button {
                        r#type: "button",
                        class: "operon-tool-card-cancel-btn",
                        "data-testid": "tool-cancel-btn",
                        title: "Cancel this tool call (runtime backend)",
                        onclick: cancel_click,
                        "Cancel"
                    }
                }
                if stuck {
                    button {
                        r#type: "button",
                        class: "operon-tool-card-cancel-turn",
                        "data-testid": "tool-cancel-turn-btn",
                        title: "Cancel the entire turn (kills the in-flight subprocess)",
                        onclick: cancel_turn_click,
                        "Cancel turn"
                    }
                }
            }
        }
    }
}

fn format_elapsed(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 1.0 {
        return "starting\u{2026}".to_string();
    }
    if secs < 60.0 {
        format!("{secs:.1}s")
    } else {
        let m = (secs / 60.0).floor() as u64;
        let s = (secs - (m as f64) * 60.0).round() as u64;
        format!("{m}m{s}s")
    }
}

#[derive(Props, Clone, PartialEq)]
struct ToolBodyProps {
    name: String,
    input: Value,
    result: Option<ToolResultBody>,
}

#[component]
fn ToolBody(props: ToolBodyProps) -> Element {
    match props.name.as_str() {
        "Read" => render_read(&props.input, props.result.as_ref()),
        "Write" => render_write(&props.input, props.result.as_ref()),
        "Edit" => render_edit(&props.input, props.result.as_ref()),
        "Bash" => render_bash(&props.input, props.result.as_ref()),
        "Glob" => render_glob(&props.input, props.result.as_ref()),
        "Grep" => render_grep(&props.input, props.result.as_ref()),
        _ => render_generic(&props.input, props.result.as_ref()),
    }
}

fn tool_icon(name: &str) -> &'static str {
    match name {
        "Read" => "\u{1F4C4}",       // 📄
        "Write" => "\u{1F4DD}",      // 📝
        "Edit" => "\u{270F}\u{FE0F}", // ✏️
        "Bash" => "\u{25B6}",        // ▶
        "Glob" => "\u{1F50D}",       // 🔍
        "Grep" => "\u{1F50E}",       // 🔎
        "TodoWrite" => "\u{2705}",   // ✅
        "Task" => "\u{1F916}",       // 🤖
        "WebFetch" => "\u{1F310}",   // 🌐
        "WebSearch" => "\u{1F50E}",  // 🔎
        _ => "\u{2699}\u{FE0F}",     // ⚙️
    }
}

fn summarize_tool(name: &str, input: &Value) -> String {
    match name {
        "Read" | "Write" | "Edit" => input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "Bash" => {
            let desc = input.get("description").and_then(|v| v.as_str());
            let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
            desc.map(|d| d.to_string())
                .unwrap_or_else(|| cmd.lines().next().unwrap_or("").to_string())
        }
        "Glob" | "Grep" => {
            let pat = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            let path = input
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if path.is_empty() {
                pat.to_string()
            } else {
                format!("{pat} in {path}")
            }
        }
        "TodoWrite" => "todos".to_string(),
        "Task" => input
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("subagent")
            .to_string(),
        _ => input
            .as_object()
            .and_then(|o| o.iter().next())
            .map(|(k, v)| format!("{k}={}", short_value(v)))
            .unwrap_or_default(),
    }
}

fn short_value(v: &Value) -> String {
    let s = match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if s.chars().count() > 60 {
        let mut head: String = s.chars().take(60).collect();
        head.push('\u{2026}');
        head
    } else {
        s
    }
}

fn render_read(input: &Value, result: Option<&ToolResultBody>) -> Element {
    let offset = input.get("offset").and_then(|v| v.as_u64());
    let limit = input.get("limit").and_then(|v| v.as_u64());
    rsx! {
        if offset.is_some() || limit.is_some() {
            div { class: "operon-tool-card-meta",
                if let Some(o) = offset { span { "offset: " code { class: "md-inline-code", "{o}" } } }
                if let Some(l) = limit { span { "limit: " code { class: "md-inline-code", "{l}" } } }
            }
        }
        ResultPre { result: result.cloned() }
    }
}

fn render_write(input: &Value, result: Option<&ToolResultBody>) -> Element {
    let content = input
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let preview: String = content.lines().take(20).collect::<Vec<_>>().join("\n");
    rsx! {
        pre { class: "operon-tool-card-pre", code { class: "md-code-block", "{preview}" } }
        ResultPre { result: result.cloned() }
    }
}

fn render_edit(input: &Value, result: Option<&ToolResultBody>) -> Element {
    let old = input.get("old_string").and_then(|v| v.as_str()).unwrap_or("");
    let new_s = input.get("new_string").and_then(|v| v.as_str()).unwrap_or("");
    rsx! {
        div { class: "operon-tool-card-diff",
            pre { class: "operon-tool-card-pre operon-tool-card-diff-old",
                code { class: "md-code-block", "{old}" }
            }
            pre { class: "operon-tool-card-pre operon-tool-card-diff-new",
                code { class: "md-code-block", "{new_s}" }
            }
        }
        ResultPre { result: result.cloned() }
    }
}

fn render_bash(input: &Value, result: Option<&ToolResultBody>) -> Element {
    let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
    rsx! {
        div { class: "operon-tool-card-meta",
            span { "$ " code { class: "md-inline-code", "{cmd}" } }
        }
        ResultPre { result: result.cloned() }
    }
}

fn render_glob(input: &Value, result: Option<&ToolResultBody>) -> Element {
    let pat = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
    let path = input.get("path").and_then(|v| v.as_str()).unwrap_or("");
    rsx! {
        div { class: "operon-tool-card-meta",
            span { "glob: " code { class: "md-inline-code", "{pat}" } }
            if !path.is_empty() { span { " in " code { class: "md-inline-code", "{path}" } } }
        }
        ResultPre { result: result.cloned() }
    }
}

fn render_grep(input: &Value, result: Option<&ToolResultBody>) -> Element {
    let pat = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
    let path = input.get("path").and_then(|v| v.as_str()).unwrap_or("");
    rsx! {
        div { class: "operon-tool-card-meta",
            span { "grep: " code { class: "md-inline-code", "{pat}" } }
            if !path.is_empty() { span { " in " code { class: "md-inline-code", "{path}" } } }
        }
        ResultPre { result: result.cloned() }
    }
}

fn render_generic(input: &Value, result: Option<&ToolResultBody>) -> Element {
    let pretty = serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string());
    rsx! {
        pre { class: "operon-tool-card-pre",
            code { class: "md-code-block", "{pretty}" }
        }
        ResultPre { result: result.cloned() }
    }
}

#[derive(Props, Clone, PartialEq)]
struct ResultPreProps {
    result: Option<ToolResultBody>,
}

#[component]
fn ResultPre(props: ResultPreProps) -> Element {
    match props.result {
        None => rsx! {
            div { class: "operon-tool-card-result-pending", "(running)" }
        },
        Some(body) => {
            if !body.is_error && body.content.trim().is_empty() {
                return rsx! {};
            }
            let class = if body.is_error {
                "operon-tool-card-pre operon-tool-card-result-error"
            } else {
                "operon-tool-card-pre operon-tool-card-result-ok"
            };
            rsx! {
                pre { class: "{class}",
                    code { class: "md-code-block", "{body.content}" }
                }
            }
        }
    }
}
