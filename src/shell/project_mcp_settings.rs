//! Per-project MCP servers modal.
//!
//! Opened from the explorer project row context menu. Wraps
//! [`crate::shell::mcp_settings::McpSettingsPanel`] in
//! [`crate::shell::mcp_settings::McpPanelMode::Project`] mode so only
//! the project's `.mcp.json` is exposed; the scope picker on the add
//! form is hidden, and `claude mcp add` writes into the project's bound
//! repo.
//!
//! When the targeted project has no `repo_path` bound yet, the modal
//! renders a "Bind repository…" prompt that opens the OS folder picker
//! and persists the choice via
//! [`operon_store::repos::LocalProjectRepository::set_repo_path`] —
//! mirroring the per-project Tool Permissions modal.

#![cfg(not(target_arch = "wasm32"))]

use std::path::PathBuf;
use std::sync::Arc;

use dioxus::prelude::*;
use operon_store::repos::LocalProjectRepository;
use uuid::Uuid;

use crate::local_mode::desktop::LocalProjectRepo;
use crate::shell::mcp_settings::{McpPanelMode, McpSettingsPanel};

/// App-scope handle. `Some(project_id)` opens the modal scoped to that
/// project; `None` keeps it closed. Provided in `App`; written by the
/// project row's context-menu entry; cleared by the modal's own close
/// paths (Esc / scrim / Close button).
#[derive(Clone, Copy)]
pub struct ProjectMcpSettingsTarget(pub Signal<Option<Uuid>>);

#[component]
pub fn ProjectMcpSettingsPanel() -> Element {
    let ProjectMcpSettingsTarget(mut target) = use_context();
    let pid_opt: Option<Uuid> = *target.read();
    let Some(pid) = pid_opt else {
        return rsx! {};
    };

    let LocalProjectRepo(project_repo) = use_context();

    // Re-read after a repo bind so the panel switches from the "Bind
    // repository…" prompt to the live list without a re-mount.
    let mut refresh_token: Signal<u64> = use_signal(|| 0u64);

    let project = {
        let _ = refresh_token.read();
        project_repo
            .list()
            .ok()
            .and_then(|projects| projects.into_iter().find(|p| p.id == pid))
    };
    let Some(project) = project else {
        // Project deleted out from under the modal — close it.
        target.set(None);
        return rsx! {};
    };

    let project_name = project.name.clone();
    let repo_path: Option<PathBuf> = project.repo_path.clone();

    // Drive the inner panel with a constant-true open signal — we
    // close by clearing the target signal instead.
    let mut open_relay: Signal<bool> = use_signal(|| true);

    // When the inner panel writes `false` to its `open` signal (Close
    // button / Esc), propagate that to the target so the host
    // un-mounts and the next open starts fresh.
    use_effect(move || {
        if !*open_relay.read() {
            target.set(None);
            open_relay.set(true);
        }
    });

    if let Some(repo) = repo_path {
        rsx! {
            McpSettingsPanel {
                open: open_relay,
                mode: McpPanelMode::Project {
                    repo,
                    name: project_name,
                },
            }
        }
    } else {
        rsx! {
            BindRepoPrompt {
                project_id: pid,
                project_name: project_name.clone(),
                project_repo: project_repo.clone(),
                on_bound: move |_: ()| {
                    refresh_token.with_mut(|t| *t = t.wrapping_add(1));
                },
                on_close: move |_: ()| target.set(None),
            }
        }
    }
}

#[derive(Props, Clone)]
struct BindRepoPromptProps {
    project_id: Uuid,
    project_name: String,
    project_repo: Arc<dyn LocalProjectRepository>,
    on_bound: EventHandler<()>,
    on_close: EventHandler<()>,
}

impl PartialEq for BindRepoPromptProps {
    fn eq(&self, other: &Self) -> bool {
        self.project_id == other.project_id
            && self.project_name == other.project_name
    }
}

#[component]
fn BindRepoPrompt(props: BindRepoPromptProps) -> Element {
    let mut err: Signal<Option<String>> = use_signal(|| None);

    let pick = {
        let project_repo = props.project_repo.clone();
        let project_id = props.project_id;
        let on_bound = props.on_bound;
        move |_| {
            let project_repo = project_repo.clone();
            let on_bound = on_bound;
            spawn(async move {
                let Some(handle) = rfd::AsyncFileDialog::new()
                    .set_title("Bind project to repository")
                    .pick_folder()
                    .await
                else {
                    return;
                };
                let path = handle.path().to_path_buf();
                match project_repo.set_repo_path(project_id, Some(&path)) {
                    Ok(_) => on_bound.call(()),
                    Err(e) => err.set(Some(format!("Failed to bind: {e}"))),
                }
            });
        }
    };

    let on_close = props.on_close;
    let project_name = props.project_name.clone();

    rsx! {
        div {
            class: "operon-modal-scrim",
            "data-testid": "project-mcp-bind-prompt",
            onclick: move |_| on_close.call(()),
            div {
                class: "operon-modal-card",
                onclick: move |evt| evt.stop_propagation(),
                h2 { class: "operon-modal-title",
                    "Project MCP servers — {project_name}"
                }
                p { class: "operon-modal-help",
                    "Project-scope MCP servers are stored in the project's repository (`.mcp.json`). Bind this project to a repository to configure them."
                }
                if let Some(msg) = err.read().clone() {
                    p { class: "operon-modal-error", "{msg}" }
                }
                div { class: "operon-modal-actions",
                    button {
                        r#type: "button",
                        class: "operon-modal-button",
                        onclick: move |_| on_close.call(()),
                        "Cancel"
                    }
                    button {
                        r#type: "button",
                        class: "operon-modal-button operon-modal-button-primary",
                        "data-testid": "project-mcp-bind-pick",
                        onclick: pick,
                        "Bind repository…"
                    }
                }
            }
        }
    }
}
