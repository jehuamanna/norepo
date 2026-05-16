//! `ServerCard` — one row in the MCP settings panel.

#![cfg(not(target_arch = "wasm32"))]

use dioxus::prelude::*;
use operon_core::agent_event::McpServerStatus;

use crate::shell::companion_state::ActiveRepoPath;
use crate::shell::mcp_settings::{McpDetails, McpEntry, McpServiceCtx, Scope};

#[derive(Props, Clone, PartialEq)]
pub struct ServerCardProps {
    pub entry: McpEntry,
    /// Working directory for `claude mcp get`/`remove`. When `None`,
    /// falls back to the active repo from context (legacy callers).
    /// Project-scope panels MUST set this to the project's repo so
    /// `mcp get` resolves the right `.mcp.json`.
    #[props(default)]
    pub cwd_override: Option<std::path::PathBuf>,
    /// Live MCP server roster reported by the most recent
    /// `system/init`. Empty if the chat session hasn't started yet.
    pub live_servers: Vec<McpServerStatus>,
    /// All tool names (built-in + `mcp__*`) reported by the most
    /// recent `system/init`. Used to derive per-server tools.
    pub live_tools: Vec<String>,
    /// Whether the live snapshot belongs to the currently-active chat
    /// session. When false, "Tools" defers to a stale message rather
    /// than (mis)reporting tools from a different session.
    pub live_is_current: bool,
    pub on_changed: EventHandler<String>,
}

#[component]
pub fn ServerCard(props: ServerCardProps) -> Element {
    let McpServiceCtx(service) = use_context();
    let ActiveRepoPath(active_repo) = use_context();
    let cwd_override = props.cwd_override.clone();
    let resolve_cwd = {
        let cwd_override = cwd_override.clone();
        let active_repo = active_repo;
        move || -> Option<std::path::PathBuf> {
            cwd_override
                .clone()
                .or_else(|| active_repo.read().clone())
        }
    };
    let mut details: Signal<DetailState> = use_signal(|| DetailState::Hidden);
    let mut tools_open: Signal<bool> = use_signal(|| false);
    let confirm_remove: Signal<bool> = use_signal(|| false);
    let mut removing: Signal<bool> = use_signal(|| false);
    let mut remove_err: Signal<Option<String>> = use_signal(|| None);

    let entry = props.entry.clone();
    let entry_for_load = entry.clone();
    let entry_for_remove = entry.clone();

    let live_status = props
        .live_servers
        .iter()
        .find(|s| s.name == entry.name)
        .cloned();

    let server_tools: Vec<String> = if props.live_is_current {
        let prefix = format!("mcp__{}__", entry.name);
        props
            .live_tools
            .iter()
            .filter(|t| t.starts_with(&prefix))
            .map(|t| t[prefix.len()..].to_string())
            .collect()
    } else {
        Vec::new()
    };

    let (dot_class, status_label) = compute_indicator(&entry, live_status.as_ref());

    // Scope chip + directory hint. For Local/Project the directory is
    // the cwd that owns the config; User is global.
    let cwd_for_scope = resolve_cwd();
    let (scope_chip_class, scope_chip_label, scope_dir): (&'static str, String, Option<String>) =
        match entry.scope {
            Some(Scope::User) => (
                "operon-mcp-scope-chip operon-mcp-scope-user",
                "User · global".to_string(),
                None,
            ),
            Some(Scope::Project) => (
                "operon-mcp-scope-chip operon-mcp-scope-project",
                "Project".to_string(),
                cwd_for_scope.as_ref().map(|p| p.display().to_string()),
            ),
            Some(Scope::Local) => (
                "operon-mcp-scope-chip operon-mcp-scope-local",
                "Local".to_string(),
                cwd_for_scope.as_ref().map(|p| p.display().to_string()),
            ),
            None => (
                "operon-mcp-scope-chip operon-mcp-scope-unknown",
                entry
                    .scope_label
                    .clone()
                    .unwrap_or_else(|| "Scope ?".to_string()),
                None,
            ),
        };
    let scope_dir_basename = scope_dir.as_ref().map(|p| {
        std::path::Path::new(p)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(p.as_str())
            .to_string()
    });

    let toggle_details = {
        let service = service.clone();
        let name = entry_for_load.name.clone();
        let resolve_cwd = resolve_cwd.clone();
        move |_| {
            let cur = details.read().clone();
            match cur {
                DetailState::Hidden => {
                    details.set(DetailState::Loading);
                    let service = service.clone();
                    let name = name.clone();
                    let cwd = resolve_cwd();
                    spawn(async move {
                        match service.get(&name, cwd.as_deref()).await {
                            Ok(d) => details.set(DetailState::Shown(d)),
                            Err(e) => details.set(DetailState::Error(e)),
                        }
                    });
                }
                _ => details.set(DetailState::Hidden),
            }
        }
    };

    let on_remove_click = {
        let mut confirm_remove = confirm_remove;
        move |_| confirm_remove.set(true)
    };

    let cancel_remove = {
        let mut confirm_remove = confirm_remove;
        let mut remove_err = remove_err;
        move |_| {
            confirm_remove.set(false);
            remove_err.set(None);
        }
    };

    let confirm_remove_click = {
        let service = service.clone();
        let name = entry_for_remove.name.clone();
        let on_changed = props.on_changed;
        let resolve_cwd = resolve_cwd.clone();
        move |_| {
            let service = service.clone();
            let name = name.clone();
            let cwd = resolve_cwd();
            removing.set(true);
            remove_err.set(None);
            spawn(async move {
                match service.remove(&name, None, cwd.as_deref()).await {
                    Ok(()) => {
                        on_changed.call(format!("Removed `{name}`."));
                    }
                    Err(e) => {
                        remove_err.set(Some(e));
                        removing.set(false);
                    }
                }
            });
        }
    };

    let tools_label = if !props.live_is_current {
        "Tools (start a chat to see)".to_string()
    } else if server_tools.is_empty() {
        "Tools (none from live session)".to_string()
    } else {
        format!("Tools ({})", server_tools.len())
    };

    rsx! {
        div { class: "operon-mcp-card-row", "data-testid": "mcp-server-card",
            div { class: "operon-mcp-card-head",
                span { class: "operon-mcp-status-dot {dot_class}",
                    title: "{status_label}",
                    ""
                }
                span { class: "operon-mcp-server-name", "{entry.name}" }
                span {
                    class: "{scope_chip_class}",
                    title: {
                        match scope_dir.as_ref() {
                            Some(p) => format!("Scope: {scope_chip_label}\nDirectory: {p}"),
                            None => format!("Scope: {scope_chip_label}"),
                        }
                    },
                    "{scope_chip_label}"
                }
                span { class: "operon-mcp-status-text", "{status_label}" }
                div { class: "operon-mcp-card-actions",
                    button {
                        r#type: "button",
                        class: "operon-modal-button",
                        "data-testid": "mcp-server-details",
                        onclick: toggle_details,
                        {
                            match &*details.read() {
                                DetailState::Hidden => "Details",
                                _ => "Hide details",
                            }
                        }
                    }
                    if *confirm_remove.read() {
                        button {
                            r#type: "button",
                            class: "operon-modal-button",
                            disabled: *removing.read(),
                            onclick: cancel_remove,
                            "Cancel"
                        }
                        button {
                            r#type: "button",
                            class: "operon-modal-button operon-modal-button-danger",
                            "data-testid": "mcp-server-remove-confirm",
                            disabled: *removing.read(),
                            onclick: confirm_remove_click,
                            { if *removing.read() { "Removing…" } else { "Confirm remove" } }
                        }
                    } else {
                        button {
                            r#type: "button",
                            class: "operon-modal-button",
                            "data-testid": "mcp-server-remove",
                            onclick: on_remove_click,
                            "Remove"
                        }
                    }
                }
            }
            p { class: "operon-mcp-server-meta",
                "{entry.command_or_url}"
            }
            if let Some(dir_path) = scope_dir.as_ref() {
                p { class: "operon-mcp-server-dir", title: "{dir_path}",
                    span { class: "operon-mcp-server-dir-icon", "📁 " }
                    span { class: "operon-mcp-server-dir-name",
                        "{scope_dir_basename.clone().unwrap_or_else(|| dir_path.clone())}"
                    }
                    span { class: "operon-mcp-server-dir-path", " — {dir_path}" }
                }
            }
            if let Some(err) = remove_err.read().clone() {
                p { class: "operon-modal-error", "{err}" }
            }
            div { class: "operon-mcp-tools-block",
                button {
                    r#type: "button",
                    class: "operon-mcp-tools-toggle",
                    onclick: move |_| {
                        let cur = *tools_open.read();
                        tools_open.set(!cur);
                    },
                    {
                        let arrow = if *tools_open.read() { "▾" } else { "▸" };
                        format!("{arrow} {tools_label}")
                    }
                }
                if *tools_open.read() && props.live_is_current && !server_tools.is_empty() {
                    ul { class: "operon-mcp-tools-list",
                        for t in server_tools.iter() {
                            li { key: "{t}", "{t}" }
                        }
                    }
                }
            }
            {
                match &*details.read() {
                    DetailState::Hidden => rsx! {},
                    DetailState::Loading => rsx! {
                        p { class: "operon-modal-help", "Loading details…" }
                    },
                    DetailState::Error(e) => rsx! {
                        p { class: "operon-modal-error", "{e}" }
                    },
                    DetailState::Shown(d) => render_details(d),
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum DetailState {
    Hidden,
    Loading,
    Shown(McpDetails),
    Error(String),
}

fn render_details(d: &McpDetails) -> Element {
    let env = d.env.clone();
    let headers = d.headers.clone();
    rsx! {
        div { class: "operon-mcp-details",
            dl { class: "operon-mcp-details-grid",
                dt { "Scope" } dd { "{d.scope_label}" }
                dt { "Type" } dd { "{d.transport}" }
                if let Some(c) = &d.command {
                    dt { "Command" } dd { "{c}" }
                }
                if let Some(a) = &d.args {
                    dt { "Args" } dd { "{a}" }
                }
                if let Some(u) = &d.url {
                    dt { "URL" } dd { "{u}" }
                }
                if !env.is_empty() {
                    dt { "Environment" }
                    dd {
                        ul { class: "operon-mcp-kv-list",
                            for (k, v) in env {
                                li { key: "{k}", "{k}={v}" }
                            }
                        }
                    }
                }
                if !headers.is_empty() {
                    dt { "Headers" }
                    dd {
                        ul { class: "operon-mcp-kv-list",
                            for (k, v) in headers {
                                li { key: "{k}", "{k}: {v}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Pick the status dot class + human label, preferring the live session
/// snapshot when available and falling back to the static `mcp list`
/// status string.
fn compute_indicator(entry: &McpEntry, live: Option<&McpServerStatus>) -> (&'static str, String) {
    if let Some(s) = live {
        let status = s.status.to_ascii_lowercase();
        if status.contains("connected") {
            return ("operon-mcp-dot-connected", "Connected".to_string());
        }
        if status.contains("failed") || status.contains("not connected") {
            return ("operon-mcp-dot-failed", s.status.clone());
        }
        if status.contains("auth") {
            return ("operon-mcp-dot-auth", s.status.clone());
        }
        return ("operon-mcp-dot-idle", s.status.clone());
    }
    if entry.connected {
        ("operon-mcp-dot-connected", entry.status.clone())
    } else if entry.status.is_empty() {
        ("operon-mcp-dot-idle", "(unknown)".to_string())
    } else {
        ("operon-mcp-dot-failed", entry.status.clone())
    }
}
