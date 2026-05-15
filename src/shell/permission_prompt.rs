//! Inline tool-permission card component.
//!
//! Replaces the earlier 3-button skeleton (Allow once / Always allow /
//! Reject) with the IDE-grade card: category badge, cwd display,
//! diff preview for Edit/Write, expandable raw-JSON textarea so the
//! user can rewrite the tool input before approving, elapsed-time
//! counter, plus four decision buttons (Allow / Allow always / Skip /
//! Deny). A fifth runtime-only `Cancel` button is only rendered when
//! the entry's `backend_id == "runtime"`.
//!
//! The component takes a [`PermissionPromptEntry`] reference and the
//! current [`PermissionStatus`] — both come from the global signals
//! (`PERMISSION_PROMPTS`, `PERMISSION_DECISIONS`). Decisions flow back
//! through the `on_decision` `EventHandler` which the caller wires to
//! `companion_chat::resolve_permission` (claude-code) or to the
//! runtime's `PermissionGate::reply` path.

#![cfg(not(target_arch = "wasm32"))]

use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use dioxus::prelude::*;
use serde_json::Value;

use crate::shell::companion_state::{PermissionPromptEntry, PermissionStatus};
use crate::shell::diff_preview;
use crate::shell::tool_category::ToolCategory;

/// User-visible decision routed back to the caller. The companion
/// chat maps these onto the bridge's `PermissionDecision` (Allow /
/// Allow + persist rule / Skip with synthetic result / Deny) before
/// resolving the parked oneshot.
#[derive(Clone, Debug, PartialEq)]
pub enum CardDecision {
    /// Approve this call; if `updated_input` is `Some`, the bridge
    /// rewrites the tool args before claude runs them.
    Allow { updated_input: Option<Value> },
    /// Approve + persist a derived allow-always rule via
    /// `permission_persist::append_allow_rule`.
    AllowAlways { updated_input: Option<Value> },
    /// Return a synthetic tool result body without running the tool;
    /// the model proceeds as if the call had failed gracefully.
    Skip { synthetic_result: String },
    /// Refuse the call. `message` is surfaced as the tool result so
    /// the model understands the rejection.
    Deny { message: String },
    /// Runtime-only: cancel the in-flight tool call entirely. Maps to
    /// firing the `TOOL_CANCEL_HANDLES[id]` cancellation token.
    Cancel,
}

#[derive(Clone, PartialEq, Props)]
pub struct PermissionPromptProps {
    pub entry: PermissionPromptEntry,
    pub status: PermissionStatus,
    pub on_decision: EventHandler<CardDecision>,
}

#[component]
pub fn PermissionPrompt(props: PermissionPromptProps) -> Element {
    let entry = props.entry;
    let status = props.status;
    let on_decision = props.on_decision;

    let pending = matches!(status, PermissionStatus::Pending);
    let buttons_disabled = !pending;

    // Editable JSON — pre-populated from the original input; the user
    // can tweak before clicking Allow.
    let pretty_initial = serde_json::to_string_pretty(&entry.input).unwrap_or_default();
    let mut input_json = use_signal(|| pretty_initial.clone());
    let mut parse_error = use_signal(|| Option::<String>::None);
    let mut json_expanded = use_signal(|| false);

    // Skip dialog: synthetic-result text the user can override before
    // sending. Default reads naturally as a tool failure message.
    let mut skip_open = use_signal(|| false);
    let mut skip_message = use_signal(|| {
        "User skipped this tool; proceed without it.".to_string()
    });

    // Tick the elapsed-time label every 500ms while pending. Drop the
    // future as soon as we leave Pending so the loop doesn't keep
    // re-rendering after the decision.
    let mut elapsed_ticker = use_signal(|| 0u64);
    use_future(move || async move {
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            elapsed_ticker.with_mut(|t| *t = t.wrapping_add(1));
        }
    });
    let _ = elapsed_ticker.read(); // subscribe — value itself is ignored
    let elapsed = SystemTime::now()
        .duration_since(entry.created_at)
        .unwrap_or_default();

    // Parse the textarea ahead of clicks so Allow can disable
    // immediately on bad JSON.
    let parsed_input: Option<Value> = if input_json.read().trim() == pretty_initial.trim() {
        // No user edits — preserve "use original verbatim" semantics
        // by sending None as updated_input.
        None
    } else {
        match serde_json::from_str::<Value>(input_json.read().as_str()) {
            Ok(v) => {
                if parse_error.read().is_some() {
                    parse_error.set(None);
                }
                Some(v)
            }
            Err(e) => {
                let msg = format!("Invalid JSON: {e}");
                if parse_error.read().as_deref() != Some(msg.as_str()) {
                    parse_error.set(Some(msg));
                }
                None
            }
        }
    };
    let allow_disabled = buttons_disabled || parse_error.read().is_some();

    // For convenience downstream — clone what the closures need.
    let parsed_for_allow = parsed_input.clone();
    let parsed_for_always = parsed_input.clone();

    let on_allow = {
        let h = on_decision;
        let p = parsed_for_allow.clone();
        move |_| {
            h.call(CardDecision::Allow {
                updated_input: p.clone(),
            })
        }
    };
    let on_allow_always = {
        let h = on_decision;
        let p = parsed_for_always.clone();
        move |_| {
            h.call(CardDecision::AllowAlways {
                updated_input: p.clone(),
            })
        }
    };
    let on_skip_send = {
        let h = on_decision;
        let mut skip_open = skip_open;
        let skip_msg = skip_message;
        move |_| {
            let msg = skip_msg.read().clone();
            skip_open.set(false);
            h.call(CardDecision::Skip {
                synthetic_result: msg,
            });
        }
    };
    let on_deny = {
        let h = on_decision;
        move |_| {
            h.call(CardDecision::Deny {
                message: "Denied by user".into(),
            })
        }
    };
    let on_cancel = {
        let h = on_decision;
        move |_| h.call(CardDecision::Cancel)
    };

    let cancel_supported = entry.backend_id == "runtime" && pending;

    let category_label = entry.category.label();
    let category_class = match entry.category {
        ToolCategory::ReadOnly => "operon-permission-cat-readonly",
        ToolCategory::FsWrite => "operon-permission-cat-fswrite",
        ToolCategory::Shell => "operon-permission-cat-shell",
        ToolCategory::Network => "operon-permission-cat-network",
        ToolCategory::Other => "operon-permission-cat-other",
    };

    let cwd_label = entry
        .source_cwd
        .as_ref()
        .map(|p| display_cwd(p))
        .unwrap_or_default();

    let diff_text =
        diff_preview::diff_source_from(&entry.tool_name, &entry.input).and_then(diff_preview::render);

    let summary = render_summary(&entry.tool_name, &entry.input);

    let status_label = match status {
        PermissionStatus::Pending => format!("Awaiting · {}", format_elapsed(elapsed)),
        PermissionStatus::Allowed => "Allowed".to_string(),
        PermissionStatus::AllowedAlways => "Allowed (always)".to_string(),
        PermissionStatus::AllowedAuto => "Auto-approved".to_string(),
        PermissionStatus::Skipped => "Skipped".to_string(),
        PermissionStatus::Denied => "Denied".to_string(),
    };

    rsx! {
        div {
            class: "operon-permission-prompt",
            "data-testid": "permission-prompt",
            "data-permission-id": entry.id.clone(),
            "data-status": status_label.clone(),
            div { class: "operon-permission-prompt-header",
                span { class: "operon-permission-prompt-tool", strong { "{entry.tool_name}" } }
                span { class: "operon-permission-prompt-category {category_class}",
                    "{category_label}"
                }
                if !cwd_label.is_empty() {
                    span { class: "operon-permission-prompt-cwd",
                        title: "{entry.source_cwd.as_ref().map(|p| p.display().to_string()).unwrap_or_default()}",
                        "cwd: {cwd_label}"
                    }
                }
                span { class: "operon-permission-prompt-status",
                    " — {status_label}"
                }
            }

            if !summary.is_empty() {
                pre { class: "operon-permission-prompt-summary",
                    "{summary}"
                }
            }

            if let Some(diff) = diff_text.as_ref() {
                details { class: "operon-permission-prompt-diff",
                    open: true,
                    summary { "Diff preview" }
                    pre { class: "operon-permission-prompt-diff-body",
                        "{diff}"
                    }
                }
            }

            details { class: "operon-permission-prompt-rawinput",
                open: *json_expanded.read(),
                ontoggle: move |_| {
                    // Flip the local state on every native toggle.
                    let cur = *json_expanded.read();
                    json_expanded.set(!cur);
                },
                summary { "Raw input (editable)" }
                textarea {
                    class: "operon-permission-prompt-jsoneditor",
                    "data-testid": "permission-json-editor",
                    disabled: buttons_disabled,
                    rows: 8,
                    value: "{input_json}",
                    oninput: move |evt| input_json.set(evt.value()),
                }
                if let Some(err) = parse_error.read().as_ref() {
                    div { class: "operon-permission-prompt-jsonerror",
                        "{err}"
                    }
                }
            }

            if *skip_open.read() {
                div { class: "operon-permission-prompt-skipform",
                    label { "Synthetic result claude will see:" }
                    textarea {
                        class: "operon-permission-prompt-skipinput",
                        rows: 2,
                        value: "{skip_message}",
                        oninput: move |evt| skip_message.set(evt.value()),
                    }
                    button {
                        class: "operon-permission-prompt-btn",
                        onclick: on_skip_send,
                        "Send skip"
                    }
                    button {
                        class: "operon-permission-prompt-btn",
                        onclick: move |_| skip_open.set(false),
                        "Cancel skip"
                    }
                }
            }

            div { class: "operon-permission-prompt-actions",
                button {
                    class: "operon-permission-prompt-btn operon-permission-prompt-btn-allow",
                    "data-testid": "permission-allow",
                    disabled: allow_disabled,
                    onclick: on_allow,
                    "Allow"
                }
                button {
                    class: "operon-permission-prompt-btn operon-permission-prompt-btn-always",
                    "data-testid": "permission-allow-always",
                    disabled: allow_disabled,
                    onclick: on_allow_always,
                    "Allow always"
                }
                button {
                    class: "operon-permission-prompt-btn operon-permission-prompt-btn-skip",
                    "data-testid": "permission-skip",
                    disabled: buttons_disabled,
                    onclick: move |_| skip_open.set(true),
                    "Skip"
                }
                button {
                    class: "operon-permission-prompt-btn operon-permission-prompt-btn-deny",
                    "data-testid": "permission-deny",
                    disabled: buttons_disabled,
                    onclick: on_deny,
                    "Deny"
                }
                if cancel_supported {
                    button {
                        class: "operon-permission-prompt-btn operon-permission-prompt-btn-cancel",
                        "data-testid": "permission-cancel",
                        title: "Cancel just this tool call (runtime backend)",
                        onclick: on_cancel,
                        "Cancel call"
                    }
                }
            }
        }
    }
}

/// One-line per-tool summary shown above the diff/JSON sections.
/// Mirrors the previous `render_permission_summary` in companion_chat.
fn render_summary(tool_name: &str, input: &Value) -> String {
    match tool_name {
        "Bash" => input
            .get("command")
            .and_then(|c| c.as_str())
            .map(|s| format!("$ {s}"))
            .unwrap_or_default(),
        "Read" | "Edit" | "Write" | "MultiEdit" => input
            .get("file_path")
            .and_then(|p| p.as_str())
            .map(|p| format!("file: {p}"))
            .unwrap_or_default(),
        "Glob" | "Grep" => input
            .get("pattern")
            .and_then(|p| p.as_str())
            .map(|s| format!("pattern: {s}"))
            .unwrap_or_default(),
        "WebFetch" | "WebSearch" => input
            .get("url")
            .or_else(|| input.get("query"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// Compact cwd label — last two path components, slash-separated.
/// Avoids overflowing the card header on a deep home path.
fn display_cwd(p: &Path) -> String {
    let comps: Vec<_> = p.components().collect();
    let n = comps.len();
    if n <= 2 {
        return p.display().to_string();
    }
    let last_two = &comps[n - 2..];
    let mut out = String::from("\u{2026}/");
    for (i, c) in last_two.iter().enumerate() {
        if i > 0 {
            out.push('/');
        }
        out.push_str(&c.as_os_str().to_string_lossy());
    }
    out
}

fn format_elapsed(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 60.0 {
        format!("{secs:.1}s")
    } else {
        let m = (secs / 60.0).floor() as u64;
        let s = (secs - (m as f64) * 60.0).round() as u64;
        format!("{m}m{s}s")
    }
}

// Suppress unused-import warning when the file is consumed only by
// builds that don't reach the use_future tick.
#[allow(dead_code)]
fn _force_link(_: Instant) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_cwd_shortens_deep_paths() {
        let p = Path::new("/home/jehu/srv/obsidian/operon-dioxus");
        let s = display_cwd(p);
        assert!(s.starts_with("\u{2026}/"));
        assert!(s.ends_with("obsidian/operon-dioxus"));
    }

    #[test]
    fn display_cwd_keeps_short_paths_verbatim() {
        let p = Path::new("/tmp");
        let s = display_cwd(p);
        assert_eq!(s, "/tmp");
    }

    #[test]
    fn format_elapsed_seconds_then_minutes() {
        assert!(format_elapsed(Duration::from_millis(500)).ends_with('s'));
        assert_eq!(format_elapsed(Duration::from_secs(75)), "1m15s");
    }

    #[test]
    fn summary_for_bash_prefixes_dollar() {
        let input = serde_json::json!({ "command": "ls -la" });
        assert_eq!(render_summary("Bash", &input), "$ ls -la");
    }

    #[test]
    fn summary_for_edit_shows_file_path() {
        let input = serde_json::json!({ "file_path": "src/foo.rs" });
        assert_eq!(render_summary("Edit", &input), "file: src/foo.rs");
    }

    #[test]
    fn summary_for_unknown_tool_is_empty() {
        let input = serde_json::json!({});
        assert_eq!(render_summary("SomeMcpTool", &input), "");
    }
}
