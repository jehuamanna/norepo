//! `McpSettingsPanel` — modal dialog listing MCP servers with add /
//! remove / details / live-status indicators.
//!
//! The panel runs in one of two scope-locked modes (see [`McpPanelMode`]):
//! - [`McpPanelMode::Global`] — only `User` scope entries; opened from
//!   the global Settings dialog.
//! - [`McpPanelMode::Project`] — only `Project` scope entries for a
//!   specific repo; opened from the explorer project row context menu.
//!
//! The chat header no longer mounts this — MCP config is split between
//! global Settings (user scope) and per-project context (project scope).

#![cfg(not(target_arch = "wasm32"))]

use std::path::PathBuf;

use dioxus::prelude::*;
use uuid::Uuid;

use crate::shell::companion_state::{
    ActiveChatSession, AgentBackendCtx, MCP_LIVE_STATUS,
};
use crate::shell::mcp_settings::add_form::AddForm;
use crate::shell::mcp_settings::server_card::ServerCard;
use crate::shell::mcp_settings::{McpEntry, McpServiceCtx, Scope};

#[derive(Clone, Debug, PartialEq)]
enum LoadState {
    Idle,
    Loading,
    Loaded(Vec<McpEntry>),
    Error(String),
}

/// Which scope tier this panel surfaces. Drives the title, the list
/// filter, the add-form's initial+locked scope, and the cwd used when
/// invoking `claude mcp …`.
#[derive(Clone, Debug, PartialEq)]
pub enum McpPanelMode {
    /// Only `User` scope entries — global, repo-agnostic. cwd is None
    /// so `claude mcp` doesn't try to read a project's `.mcp.json`.
    Global,
    /// Only `Project` scope entries, scoped to a given repo. The repo
    /// is used as cwd so `claude mcp add` writes to `<repo>/.mcp.json`.
    Project {
        repo: PathBuf,
        /// Display name (project name) for the modal title.
        name: String,
    },
}

impl McpPanelMode {
    fn restrict_scope(&self) -> Scope {
        match self {
            Self::Global => Scope::User,
            Self::Project { .. } => Scope::Project,
        }
    }

    fn cwd(&self) -> Option<PathBuf> {
        match self {
            Self::Global => None,
            Self::Project { repo, .. } => Some(repo.clone()),
        }
    }

    fn title(&self) -> String {
        match self {
            Self::Global => "Global MCP servers".to_string(),
            Self::Project { name, .. } => format!("Project MCP servers — {name}"),
        }
    }

    fn help(&self) -> &'static str {
        match self {
            Self::Global => {
                "User-scope MCP servers — available in every project. \
                 Project-scope servers live on the project row's context menu."
            }
            Self::Project { .. } => {
                "Project-scope MCP servers — written to this project's \
                 `.mcp.json` so the team shares the same config. \
                 Global servers live in Settings."
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct McpSettingsPanelProps {
    pub open: Signal<bool>,
    pub mode: McpPanelMode,
}

#[component]
pub fn McpSettingsPanel(props: McpSettingsPanelProps) -> Element {
    let McpSettingsPanelProps { mut open, mode } = props;
    let McpServiceCtx(service) = use_context();
    // `ActiveChatSession` is provided deeper in the Local-Mode shell
    // (`local_mode::desktop` around line 1342), but the panel is
    // mounted as a sibling of `Workspace` in `app.rs` so the
    // provider isn't visible here. The session id is only used to
    // restart the bound chat after a config edit — without it we
    // can still render the panel and let edits take effect on the
    // next session spawn. Degrade silently rather than panic on the
    // missing context.
    let active_session: Option<Signal<Option<Uuid>>> =
        try_consume_context::<ActiveChatSession>().map(|c| c.0);
    // `AgentBackendCtx` lives in the Local-Mode shell tree alongside
    // `ActiveChatSession` (see `local_mode::desktop` line 1416); same
    // reason it isn't visible at the cross-tree mount site. The
    // signal is only used by `restart_session` — without it we skip
    // the live restart and let the edit take effect on next spawn.
    let backend_signal = try_consume_context::<AgentBackendCtx>().map(|c| c.0);
    let mut servers: Signal<LoadState> = use_signal(|| LoadState::Idle);
    // Add form is expanded by default so adding a server is one of the
    // first affordances the user sees; collapse toggle is on the header.
    let mut show_add: Signal<bool> = use_signal(|| true);
    let mut info_msg: Signal<Option<String>> = use_signal(|| None);

    let restrict = mode.restrict_scope();
    let cwd = mode.cwd();

    {
        let service = service.clone();
        let cwd = cwd.clone();
        use_effect(move || {
            if !*open.read() {
                return;
            }
            let service = service.clone();
            let cwd = cwd.clone();
            servers.set(LoadState::Loading);
            spawn(async move {
                match service.list_enriched(cwd.as_deref()).await {
                    Ok(v) => servers.set(LoadState::Loaded(v)),
                    Err(e) => servers.set(LoadState::Error(e)),
                }
            });
        });
    }

    let mut close = move || {
        open.set(false);
        info_msg.set(None);
    };

    let restart_session = {
        let active_session = active_session;
        let backend_signal = backend_signal;
        // Restart the bound chat session only when the project the
        // panel manages matches the active chat's cwd. Global edits
        // don't trigger a restart — they take effect on the next
        // session spawn (claude reads user config at startup). If
        // either the session or backend context is absent (panel
        // opened outside the Local-Mode shell), skip the restart
        // entirely; the edit still persists.
        let cwd_for_restart = cwd.clone();
        move || {
            let Some(restart_cwd) = cwd_for_restart.clone() else {
                return;
            };
            let Some(active_session) = active_session.as_ref() else {
                return;
            };
            let Some(backend_signal) = backend_signal.as_ref() else {
                return;
            };
            let session = *active_session.read();
            let backend = backend_signal.read().clone();
            if let Some(session_id) = session {
                spawn(async move {
                    let _ = backend.unbind_session(session_id).await;
                    let _ = backend.bind_session(session_id, restart_cwd).await;
                });
            }
        }
    };

    let reload = {
        let service = service.clone();
        let restart_session = restart_session.clone();
        let cwd = cwd.clone();
        move || {
            let service = service.clone();
            let restart_session = restart_session.clone();
            let cwd = cwd.clone();
            servers.set(LoadState::Loading);
            spawn(async move {
                let next = match service.list_enriched(cwd.as_deref()).await {
                    Ok(v) => LoadState::Loaded(v),
                    Err(e) => LoadState::Error(e),
                };
                servers.set(next);
                restart_session();
            });
        }
    };

    if !*open.read() {
        return rsx! {};
    }

    let live = MCP_LIVE_STATUS.read().clone();
    let live_session = live.session;
    // Without an active session context (panel opened outside the
    // shell), there's no chat to compare against — treat as
    // not-current so the "live" indicator stays muted.
    let live_is_current = match (
        live_session,
        active_session.as_ref().and_then(|s| *s.read()),
    ) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    };
    let live_servers = live.mcp_servers.clone();
    let live_tools = live.tools.clone();

    let title = mode.title();
    let help_static = mode.help();
    let help_live = if live_is_current {
        "Live status from the current chat session."
    } else if live_session.is_some() {
        "Showing claude's static config. Send a chat turn in this session to refresh live status."
    } else {
        "No live session yet — start a chat to see per-server status & tools."
    };

    rsx! {
        div {
            class: "operon-modal-scrim",
            "data-testid": "mcp-settings-panel",
            onclick: move |_| close(),
            onkeydown: move |evt| {
                if evt.key().to_string() == "Escape" {
                    close();
                    evt.prevent_default();
                }
            },
            div {
                class: "operon-modal-card operon-mcp-card",
                onclick: move |evt| evt.stop_propagation(),
                div { class: "operon-mcp-header",
                    h2 { class: "operon-modal-title", "{title}" }
                    div { class: "operon-mcp-header-actions",
                        button {
                            r#type: "button",
                            class: "operon-modal-button",
                            "data-testid": "mcp-reload",
                            title: "Reload list & force the next chat turn to spawn fresh",
                            onclick: {
                                let mut reload = reload.clone();
                                move |_| reload()
                            },
                            "Reload"
                        }
                        button {
                            r#type: "button",
                            class: "operon-modal-button",
                            "data-testid": "mcp-close",
                            onclick: move |_| close(),
                            "Close"
                        }
                    }
                }
                p { class: "operon-modal-help", "{help_static}" }
                p { class: "operon-modal-help", "{help_live}" }
                if let Some(msg) = info_msg.read().clone() {
                    p { class: "operon-modal-info", "{msg}" }
                }
                // Add form on top — collapsible via the chevron toggle.
                div { class: "operon-mcp-add-section",
                    button {
                        r#type: "button",
                        class: "operon-mcp-add-toggle",
                        "data-testid": "mcp-add-toggle",
                        onclick: move |_| {
                            let cur = *show_add.read();
                            show_add.set(!cur);
                        },
                        {
                            let arrow = if *show_add.read() { "▾" } else { "▸" };
                            format!("{arrow} Add MCP server")
                        }
                    }
                    if *show_add.read() {
                        AddForm {
                            initial_scope: restrict,
                            lock_scope: true,
                            cwd_override: cwd.clone(),
                            on_done: {
                                let mut reload = reload.clone();
                                let mut info_msg = info_msg;
                                EventHandler::new(move |msg: Option<String>| {
                                    if let Some(m) = msg {
                                        info_msg.set(Some(m));
                                        reload();
                                    }
                                })
                            },
                        }
                    }
                }
                // Server listing below the form — scrollable region so a
                // long list (multiple figma sessions, etc.) doesn't push
                // the add form off-screen.
                h3 { class: "operon-modal-section operon-mcp-list-heading",
                    "Configured servers"
                }
                div { class: "operon-mcp-list-scroll",
                    div { class: "operon-mcp-list",
                        {
                            let live_servers = live_servers.clone();
                            let live_tools = live_tools.clone();
                            let live_is_current = live_is_current;
                            let cwd_card = cwd.clone();
                            match &*servers.read() {
                                LoadState::Idle => rsx! { p { class: "operon-modal-help", "" } },
                                LoadState::Loading => rsx! {
                                    p { class: "operon-modal-help", "Loading…" }
                                },
                                LoadState::Error(e) => rsx! {
                                    p { class: "operon-modal-error", "{e}" }
                                },
                                LoadState::Loaded(v) => {
                                    let filtered: Vec<McpEntry> = v
                                        .iter()
                                        .filter(|e| {
                                            // Entries without a classified
                                            // scope (rare — parse fall-through)
                                            // appear in every mode so the user
                                            // can still remove them.
                                            match e.scope {
                                                Some(s) => s == restrict,
                                                None => true,
                                            }
                                        })
                                        .cloned()
                                        .collect();
                                    if filtered.is_empty() {
                                        rsx! {
                                            p { class: "operon-modal-help",
                                                "No MCP servers configured for this scope. Add one above."
                                            }
                                        }
                                    } else {
                                        rsx! {
                                            for entry in filtered {
                                                ServerCard {
                                                    key: "{entry.name}",
                                                    entry: entry.clone(),
                                                    cwd_override: cwd_card.clone(),
                                                    live_servers: live_servers.clone(),
                                                    live_tools: live_tools.clone(),
                                                    live_is_current,
                                                    on_changed: {
                                                        let mut reload = reload.clone();
                                                        let mut info_msg = info_msg;
                                                        EventHandler::new(move |msg: String| {
                                                            info_msg.set(Some(msg));
                                                            reload();
                                                        })
                                                    },
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
