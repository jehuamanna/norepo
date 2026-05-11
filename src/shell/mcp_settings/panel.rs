//! `McpSettingsPanel` — modal dialog listing MCP servers with add /
//! remove / details / live-status indicators.

#![cfg(not(target_arch = "wasm32"))]

use dioxus::prelude::*;

use crate::shell::companion_state::{
    ActiveChatSession, ActiveRepoPath, AgentBackendCtx, MCP_LIVE_STATUS,
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

#[component]
pub fn McpSettingsPanel(open: Signal<bool>) -> Element {
    let McpServiceCtx(service) = use_context();
    let ActiveChatSession(active_session) = use_context();
    let ActiveRepoPath(active_repo) = use_context();
    let AgentBackendCtx(backend_signal) = use_context();
    let mut servers: Signal<LoadState> = use_signal(|| LoadState::Idle);
    let mut show_add: Signal<bool> = use_signal(|| false);
    let mut info_msg: Signal<Option<String>> = use_signal(|| None);
    let default_scope_signal: Signal<Scope> =
        use_signal(|| pick_default_scope(active_repo.read().clone()));

    {
        let service = service.clone();
        use_effect(move || {
            if !*open.read() {
                return;
            }
            let service = service.clone();
            let cwd = active_repo.read().clone();
            servers.set(LoadState::Loading);
            spawn(async move {
                match service.list(cwd.as_deref()).await {
                    Ok(v) => servers.set(LoadState::Loaded(v)),
                    Err(e) => servers.set(LoadState::Error(e)),
                }
            });
        });
    }

    // Keep the default-scope signal aligned with the active project root
    // so opening the panel after switching projects picks the right
    // initial scope.
    {
        let mut default_scope_signal = default_scope_signal;
        use_effect(move || {
            default_scope_signal.set(pick_default_scope(active_repo.read().clone()));
        });
    }

    let mut close = move || {
        open.set(false);
        show_add.set(false);
        info_msg.set(None);
    };

    let restart_session = {
        let active_session = active_session;
        let active_repo = active_repo;
        let backend_signal = backend_signal;
        move || {
            let session = *active_session.read();
            let cwd = active_repo.read().clone();
            let backend = backend_signal.read().clone();
            if let (Some(session_id), Some(cwd)) = (session, cwd) {
                spawn(async move {
                    let _ = backend.unbind_session(session_id).await;
                    let _ = backend.bind_session(session_id, cwd).await;
                });
            }
        }
    };

    let reload = {
        let service = service.clone();
        let restart_session = restart_session.clone();
        move || {
            let service = service.clone();
            let restart_session = restart_session.clone();
            let cwd = active_repo.read().clone();
            servers.set(LoadState::Loading);
            spawn(async move {
                let next = match service.list(cwd.as_deref()).await {
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
    let live_is_current = match (live_session, *active_session.read()) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    };
    let live_servers = live.mcp_servers.clone();
    let live_tools = live.tools.clone();

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
                    h2 { class: "operon-modal-title", "MCP servers" }
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
                p { class: "operon-modal-help",
                    {
                        if live_is_current {
                            "Live status from the current chat session."
                        } else if live_session.is_some() {
                            "Showing claude's static config. Send a chat turn in this session to refresh live status."
                        } else {
                            "No live session yet — start a chat to see per-server status & tools."
                        }
                    }
                }
                if let Some(msg) = info_msg.read().clone() {
                    p { class: "operon-modal-info", "{msg}" }
                }
                div { class: "operon-mcp-list",
                    {
                        let live_servers = live_servers.clone();
                        let live_tools = live_tools.clone();
                        let live_is_current = live_is_current;
                        match &*servers.read() {
                            LoadState::Idle => rsx! { p { class: "operon-modal-help", "" } },
                            LoadState::Loading => rsx! {
                                p { class: "operon-modal-help", "Loading…" }
                            },
                            LoadState::Error(e) => rsx! {
                                p { class: "operon-modal-error", "{e}" }
                            },
                            LoadState::Loaded(v) if v.is_empty() => rsx! {
                                p { class: "operon-modal-help",
                                    "No MCP servers configured. Add one below."
                                }
                            },
                            LoadState::Loaded(v) => {
                                let entries = v.clone();
                                rsx! {
                                    for entry in entries {
                                        ServerCard {
                                            key: "{entry.name}",
                                            entry: entry.clone(),
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
                div { class: "operon-mcp-footer",
                    if *show_add.read() {
                        AddForm {
                            initial_scope: *default_scope_signal.read(),
                            on_done: {
                                let mut reload = reload.clone();
                                let mut info_msg = info_msg;
                                let mut show_add = show_add;
                                EventHandler::new(move |msg: Option<String>| {
                                    show_add.set(false);
                                    if let Some(m) = msg {
                                        info_msg.set(Some(m));
                                        reload();
                                    }
                                })
                            },
                        }
                    } else {
                        button {
                            r#type: "button",
                            class: "operon-modal-button operon-modal-button-primary",
                            "data-testid": "mcp-add-toggle",
                            onclick: move |_| show_add.set(true),
                            "Add MCP server"
                        }
                    }
                }
            }
        }
    }
}

/// Pick a sensible default scope for the add form: project when a repo
/// is bound, user otherwise. The user can still override per-add.
fn pick_default_scope(repo: Option<std::path::PathBuf>) -> Scope {
    match repo {
        Some(_) => Scope::Project,
        None => Scope::User,
    }
}
