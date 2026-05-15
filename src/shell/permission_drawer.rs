//! Queued-permissions drawer + activity-bar badge (Phase 2).
//!
//! `PERMISSION_PROMPTS` is the system-wide audit log of every permission
//! ask Operon has surfaced — interactive companion chat, background
//! cascade, runtime backend, all share the same vec. The companion
//! chat inlines the rich `PermissionPrompt` card for entries belonging
//! to the visible chat session, but a cascade running in the
//! background may push entries while the user is elsewhere; without
//! the drawer those cards never render and the cascade hangs invisibly
//! (this was the actual symptom in the figma-MCP-check incident).
//!
//! Two components live here:
//! - [`PermissionBadge`] — a small icon-with-count chip mounted in
//!   the activity bar. Hidden when there are no pending non-auto-
//!   approved prompts; shows the count and acts as a button that
//!   toggles [`PermissionDrawerOpen`].
//! - [`PermissionDrawer`] — modal-style panel that lists every
//!   pending entry, grouped by source chat session, with `[focus]`
//!   buttons that switch the active chat to the entry's source so
//!   the user can act on the inline card. Mounted at the app shell
//!   level so it overlays whichever pane is currently visible.

#![cfg(not(target_arch = "wasm32"))]

use dioxus::prelude::*;
use uuid::Uuid;

use crate::shell::companion_state::{
    ActiveChatSession, PermissionPromptEntry, PermissionStatus, PERMISSION_DECISIONS,
    PERMISSION_PROMPTS,
};
use crate::shell::tool_category::ToolCategory;

/// App-scope signal toggling the drawer's visibility. Provided in
/// `app.rs` so both the activity-bar badge and the drawer panel
/// itself can read/write.
#[derive(Clone, Copy)]
pub struct PermissionDrawerOpen(pub Signal<bool>);

/// Count of pending non-auto-approved prompts visible in the drawer.
/// Subscribes to `PERMISSION_PROMPTS` + `PERMISSION_DECISIONS` so the
/// number updates live as prompts arrive and resolve.
fn pending_count() -> usize {
    let prompts = PERMISSION_PROMPTS.read();
    let decisions = PERMISSION_DECISIONS.read();
    prompts
        .iter()
        .filter(|e| {
            matches!(
                decisions.get(&e.id).cloned().unwrap_or(PermissionStatus::Pending),
                PermissionStatus::Pending
            )
        })
        .count()
}

/// Activity-bar chip. Hidden (renders nothing) when count == 0 so
/// the bar stays clean during the common case. Clicking it flips
/// `PermissionDrawerOpen`.
#[component]
pub fn PermissionBadge() -> Element {
    let drawer = try_consume_context::<PermissionDrawerOpen>();
    // Subscribe to both signals so the badge updates on push *and*
    // on resolve.
    let _ = PERMISSION_PROMPTS.read();
    let _ = PERMISSION_DECISIONS.read();
    let count = pending_count();
    if count == 0 || drawer.is_none() {
        return rsx! {};
    }
    let PermissionDrawerOpen(mut open) = drawer.unwrap();

    rsx! {
        button {
            r#type: "button",
            class: "operon-activity-toggle operon-permission-badge",
            "data-testid": "permission-badge",
            "data-count": "{count}",
            title: "Pending tool permissions ({count})",
            "aria-label": "Pending tool permissions",
            onclick: move |_| {
                let cur = *open.read();
                open.set(!cur);
            },
            span { class: "operon-permission-badge-glyph", "!" }
            span { class: "operon-permission-badge-count", "{count}" }
        }
    }
}

#[component]
pub fn PermissionDrawer() -> Element {
    let drawer = try_consume_context::<PermissionDrawerOpen>();
    let active_session_ctx = try_consume_context::<ActiveChatSession>();
    let Some(PermissionDrawerOpen(mut open)) = drawer else {
        return rsx! {};
    };
    if !*open.read() {
        return rsx! {};
    }

    let prompts = PERMISSION_PROMPTS.read().clone();
    let decisions = PERMISSION_DECISIONS.read().clone();

    // Newest-first; only entries still Pending are actionable so we
    // surface them at the top. Resolved entries appear below in a
    // collapsed History section purely for audit.
    let mut pending: Vec<PermissionPromptEntry> = prompts
        .iter()
        .filter(|e| {
            matches!(
                decisions.get(&e.id).cloned().unwrap_or(PermissionStatus::Pending),
                PermissionStatus::Pending
            )
        })
        .cloned()
        .collect();
    pending.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    // History: every entry that's not Pending. Capped to 200 in the
    // visible list for render speed; PERMISSION_PROMPTS already trims
    // itself at PERMISSION_PROMPTS_CAP (500) so this is a cheap
    // upper bound rather than a real eviction.
    let mut history: Vec<(PermissionPromptEntry, PermissionStatus)> = prompts
        .iter()
        .filter_map(|e| {
            let status = decisions
                .get(&e.id)
                .cloned()
                .unwrap_or(PermissionStatus::Pending);
            if matches!(status, PermissionStatus::Pending) {
                None
            } else {
                Some((e.clone(), status))
            }
        })
        .collect();
    history.sort_by(|a, b| b.0.created_at.cmp(&a.0.created_at));
    history.truncate(200);

    let close = move |_| open.set(false);

    rsx! {
        div {
            class: "operon-modal-scrim",
            "data-testid": "permission-drawer-scrim",
            onclick: close,
            onkeydown: move |evt| {
                if evt.key().to_string() == "Escape" {
                    evt.prevent_default();
                    open.set(false);
                }
            },
            tabindex: "0",
            div {
                class: "operon-modal-card operon-permission-drawer-card",
                style: "max-width: 640px; max-height: 80vh; display: flex; flex-direction: column;",
                onclick: move |evt| evt.stop_propagation(),
                div {
                    class: "operon-permission-drawer-header",
                    h2 { class: "operon-modal-title", "Pending tool permissions" }
                    button {
                        r#type: "button",
                        class: "operon-modal-button",
                        "data-testid": "permission-drawer-close",
                        onclick: move |_| open.set(false),
                        "Close"
                    }
                }
                if pending.is_empty() {
                    p {
                        class: "operon-modal-help",
                        style: "font-style: italic;",
                        "No pending approvals. Tool calls will appear here whenever a background cascade or chat asks for permission."
                    }
                } else {
                    div {
                        class: "operon-permission-drawer-list",
                        style: "overflow-y: auto; flex: 0 0 auto; max-height: 50vh;",
                        for entry in pending.iter() {
                            {render_row(entry.clone(), active_session_ctx, open)}
                        }
                    }
                }
                // Audit-log history. Collapsed by default so it doesn't
                // overwhelm the actionable pending list, but available
                // for users to scroll through past decisions.
                if !history.is_empty() {
                    details {
                        class: "operon-permission-drawer-history",
                        style: "margin-top: 0.75rem; padding-top: 0.5rem; border-top: 1px solid var(--operon-border, #ddd);",
                        summary {
                            style: "font-size: 0.9em; cursor: pointer;",
                            "History ({history.len()})"
                        }
                        div {
                            class: "operon-permission-drawer-history-list",
                            style: "overflow-y: auto; max-height: 40vh; margin-top: 0.5rem;",
                            for (entry, status) in history.iter() {
                                {render_history_row(entry.clone(), status.clone())}
                            }
                        }
                    }
                }
            }
        }
    }
}

fn render_history_row(entry: PermissionPromptEntry, status: PermissionStatus) -> Element {
    let category_class = match entry.category {
        ToolCategory::ReadOnly => "operon-permission-cat-readonly",
        ToolCategory::FsWrite => "operon-permission-cat-fswrite",
        ToolCategory::Shell => "operon-permission-cat-shell",
        ToolCategory::Network => "operon-permission-cat-network",
        ToolCategory::Other => "operon-permission-cat-other",
    };
    let summary = summary_for(&entry);
    let status_label = match status {
        PermissionStatus::Allowed => "Allowed",
        PermissionStatus::AllowedAlways => "Allowed (always)",
        PermissionStatus::AllowedAuto => "Auto-approved",
        PermissionStatus::Skipped => "Skipped",
        PermissionStatus::Denied => "Denied",
        PermissionStatus::Pending => "Pending", // unreachable; included for exhaustiveness
    };
    rsx! {
        div {
            class: "operon-permission-drawer-history-row",
            "data-testid": "permission-history-row",
            "data-prompt-id": entry.id.clone(),
            "data-status": status_label,
            style: "padding: 4px 0; border-bottom: 1px solid var(--operon-border-subtle, #eee); font-size: 0.85em;",
            div { style: "display: flex; gap: 8px; align-items: center;",
                strong { "{entry.tool_name}" }
                span { class: "operon-permission-prompt-category {category_class}",
                    "{entry.category.label()}"
                }
                span { style: "margin-left: auto; color: var(--operon-fg-muted, #666);",
                    "{status_label}"
                }
            }
            if !summary.is_empty() {
                div { style: "font-family: monospace; font-size: 0.85em; color: var(--operon-fg-muted, #666); margin-top: 2px;",
                    "{summary}"
                }
            }
        }
    }
}

fn render_row(
    entry: PermissionPromptEntry,
    active_session_ctx: Option<ActiveChatSession>,
    mut drawer_open: Signal<bool>,
) -> Element {
    let category_class = match entry.category {
        ToolCategory::ReadOnly => "operon-permission-cat-readonly",
        ToolCategory::FsWrite => "operon-permission-cat-fswrite",
        ToolCategory::Shell => "operon-permission-cat-shell",
        ToolCategory::Network => "operon-permission-cat-network",
        ToolCategory::Other => "operon-permission-cat-other",
    };
    let summary = summary_for(&entry);
    let session_label = entry
        .source_session
        .map(|s| format!("session {}", short_uuid(s)))
        .unwrap_or_else(|| "unknown session".to_string());
    let focus_session = entry.source_session;
    let can_focus = active_session_ctx.is_some() && focus_session.is_some();

    rsx! {
        div {
            class: "operon-permission-drawer-row",
            "data-testid": "permission-drawer-row",
            "data-prompt-id": entry.id.clone(),
            div { class: "operon-permission-drawer-row-head",
                strong { "{entry.tool_name}" }
                span { class: "operon-permission-prompt-category {category_class}",
                    "{entry.category.label()}"
                }
                span { class: "operon-permission-drawer-row-session",
                    "{session_label}"
                }
            }
            if !summary.is_empty() {
                pre { class: "operon-permission-drawer-row-summary",
                    "{summary}"
                }
            }
            div { class: "operon-permission-drawer-row-actions",
                button {
                    r#type: "button",
                    class: "operon-modal-button",
                    "data-testid": "permission-drawer-focus",
                    disabled: !can_focus,
                    onclick: move |_| {
                        if let (Some(ActiveChatSession(mut sess)), Some(sid)) =
                            (active_session_ctx, focus_session)
                        {
                            sess.set(Some(sid));
                            drawer_open.set(false);
                        }
                    },
                    "Focus chat"
                }
            }
        }
    }
}

fn short_uuid(id: Uuid) -> String {
    let s = id.to_string();
    if s.len() >= 8 {
        s[..8].to_string()
    } else {
        s
    }
}

fn summary_for(entry: &PermissionPromptEntry) -> String {
    match entry.tool_name.as_str() {
        "Bash" => entry
            .input
            .get("command")
            .and_then(|c| c.as_str())
            .map(|s| format!("$ {s}"))
            .unwrap_or_default(),
        "Read" | "Edit" | "Write" | "MultiEdit" => entry
            .input
            .get("file_path")
            .and_then(|p| p.as_str())
            .map(|p| format!("file: {p}"))
            .unwrap_or_default(),
        "Glob" | "Grep" => entry
            .input
            .get("pattern")
            .and_then(|p| p.as_str())
            .map(|s| format!("pattern: {s}"))
            .unwrap_or_default(),
        "WebFetch" | "WebSearch" => entry
            .input
            .get("url")
            .or_else(|| entry.input.get("query"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::SystemTime;

    fn entry(tool: &str, input: serde_json::Value) -> PermissionPromptEntry {
        PermissionPromptEntry {
            id: format!("test-{tool}"),
            tool_name: tool.to_string(),
            input,
            source_session: None,
            source_cwd: None,
            category: crate::shell::tool_category::of(tool),
            created_at: SystemTime::now(),
            backend_id: "claude-code".to_string(),
        }
    }

    #[test]
    fn summary_for_bash_prefixes_dollar() {
        let e = entry("Bash", json!({ "command": "ls -la" }));
        assert_eq!(summary_for(&e), "$ ls -la");
    }

    #[test]
    fn summary_for_edit_uses_file_path() {
        let e = entry("Edit", json!({ "file_path": "src/foo.rs" }));
        assert_eq!(summary_for(&e), "file: src/foo.rs");
    }

    #[test]
    fn short_uuid_truncates_to_eight() {
        let id = Uuid::parse_str("12345678-90ab-cdef-1234-567890abcdef").unwrap();
        assert_eq!(short_uuid(id), "12345678");
    }
}
