//! Tools → Project Claude Defaults panel.
//!
//! Lists every project Operon knows about and lets the user set the
//! project-tier Claude model + permission-mode defaults
//! (`local_project.{default_model, default_permission_mode}`,
//! migration 019). Chats inside a project inherit these unless they
//! set their own override via the chat-header picker.
//!
//! Resolution order at spawn time is chat → project → global → omit
//! the flag. Each "Inherit (global)" option in this panel writes NULL
//! to the project column, which makes chats in that project fall back
//! to the global default.

#![cfg(not(target_arch = "wasm32"))]

use dioxus::prelude::*;

use crate::local_mode::desktop::{LocalProjectRepo, LocalSettingsRepo};

/// App-scope visibility signal for the panel. Provided in `App`,
/// flipped by the `tools.openProjectClaudeSettings` command. The panel
/// owns the close write (Esc / scrim click / Close button).
#[derive(Clone, Copy)]
pub struct ProjectClaudeSettingsOpen(pub Signal<bool>);

#[component]
pub fn ProjectClaudeSettingsPanel() -> Element {
    let ProjectClaudeSettingsOpen(mut open) = use_context();
    if !*open.read() {
        return rsx! {};
    }

    let LocalProjectRepo(project_repo) = use_context();
    let LocalSettingsRepo(settings_repo) = use_context();

    // Bumped on every set so the project list re-reads and the
    // dropdown's "Inherit (X)" label refreshes. We also bump
    // `PROJECT_SETTINGS_VERSION` so any open chat's `picker_persisted`
    // memo recomputes — the chat-header dropdown's inherited label
    // tracks the project change without needing a chat-switch.
    let mut refresh_token: Signal<u64> = use_signal(|| 0u64);

    let projects = {
        let _ = refresh_token.read();
        project_repo.list().unwrap_or_default()
    };

    // Resolve the global defaults once so each project row can show
    // "Inherit (Opus 4.7)" in its dropdown without re-querying.
    let global_model = settings_repo
        .get(crate::local_mode::SETTINGS_KEY_CLAUDE_DEFAULT_MODEL)
        .ok()
        .flatten()
        .filter(|s| !s.is_empty());
    let global_perm = settings_repo
        .get(crate::local_mode::SETTINGS_KEY_CLAUDE_DEFAULT_PERMISSION_MODE)
        .ok()
        .flatten()
        .filter(|s| !s.is_empty());

    let close = move |_| open.set(false);

    rsx! {
        div {
            class: "operon-modal-scrim",
            "data-testid": "project-claude-settings-panel",
            onclick: close,
            onkeydown: move |evt| {
                if evt.key().to_string() == "Escape" {
                    evt.prevent_default();
                    open.set(false);
                }
            },
            tabindex: "0",
            div {
                class: "operon-modal-card",
                style: "max-width: 720px; max-height: 80vh; display: flex; flex-direction: column;",
                onclick: move |evt| evt.stop_propagation(),
                h2 { class: "operon-modal-title", "Project Claude Defaults" }
                p {
                    class: "operon-modal-message",
                    "Set the Claude model and permission mode for every chat in a project. Chats inherit these unless they pick their own override. Picking ",
                    em { "Inherit (global)" }
                    " clears the project value and falls through to the app-wide default."
                }
                div {
                    style: "overflow-y: auto; flex: 1; margin-top: 12px;",
                    if projects.is_empty() {
                        p {
                            class: "operon-modal-message",
                            style: "font-style: italic; opacity: 0.7;",
                            "No projects yet."
                        }
                    }
                    for project in projects.into_iter() {
                        {
                            let pid = project.id;
                            let name = project.name.clone();
                            let cur_model = project.default_model.clone();
                            let cur_perm = project.default_permission_mode.clone();
                            let inherit_model_label = match global_model.as_deref() {
                                Some(id) => format!("Inherit ({})", crate::shell::companion_chat::model_display(id)),
                                None => "Inherit (Claude default)".to_string(),
                            };
                            let inherit_perm_label = match global_perm.as_deref() {
                                Some(id) => format!("Inherit ({})", crate::shell::companion_chat::perm_display(id)),
                                None => "Inherit (Claude default)".to_string(),
                            };
                            let project_repo_model = project_repo.clone();
                            let project_repo_perm = project_repo.clone();
                            rsx! {
                                div {
                                    style: "border: 1px solid var(--operon-border, #444); border-radius: 4px; padding: 10px 12px; margin-bottom: 8px;",
                                    "data-testid": "project-claude-settings-row",
                                    "data-project-id": "{pid}",
                                    div {
                                        style: "display: flex; align-items: center; gap: 12px; margin-bottom: 8px;",
                                        strong { "{name}" }
                                    }
                                    div {
                                        style: "display: flex; gap: 10px; align-items: center; margin-bottom: 6px;",
                                        label { style: "min-width: 110px; font-size: 0.9em;", "Model:" }
                                        select {
                                            class: "operon-companion-model-picker",
                                            "data-testid": "project-claude-model-picker",
                                            onchange: move |e| {
                                                let v = e.value();
                                                let next = if v == "inherit" { None } else { Some(v) };
                                                if let Err(e) = project_repo_model.set_default_model(pid, next.as_deref()) {
                                                    tracing::warn!(
                                                        target: "operon::project_settings",
                                                        "persist project default_model failed: {e}"
                                                    );
                                                }
                                                refresh_token.with_mut(|t| *t = t.wrapping_add(1));
                                                *crate::shell::companion_state::PROJECT_SETTINGS_VERSION.write() += 1;
                                            },
                                            option { value: "inherit",
                                                selected: cur_model.is_none(),
                                                "{inherit_model_label}"
                                            }
                                            option { value: "claude-opus-4-7",
                                                selected: cur_model.as_deref() == Some("claude-opus-4-7"),
                                                "Opus 4.7"
                                            }
                                            option { value: "claude-opus-4-6",
                                                selected: cur_model.as_deref() == Some("claude-opus-4-6"),
                                                "Opus 4.6"
                                            }
                                            option { value: "claude-sonnet-4-6",
                                                selected: cur_model.as_deref() == Some("claude-sonnet-4-6"),
                                                "Sonnet 4.6"
                                            }
                                            option { value: "claude-sonnet-4-5",
                                                selected: cur_model.as_deref() == Some("claude-sonnet-4-5"),
                                                "Sonnet 4.5"
                                            }
                                            option { value: "claude-haiku-4-5",
                                                selected: cur_model.as_deref() == Some("claude-haiku-4-5"),
                                                "Haiku 4.5"
                                            }
                                            option { value: "claude-3-5-sonnet-20241022",
                                                selected: cur_model.as_deref() == Some("claude-3-5-sonnet-20241022"),
                                                "Sonnet 3.5"
                                            }
                                            option { value: "claude-3-5-haiku-20241022",
                                                selected: cur_model.as_deref() == Some("claude-3-5-haiku-20241022"),
                                                "Haiku 3.5"
                                            }
                                            option { value: "claude-3-opus-20240229",
                                                selected: cur_model.as_deref() == Some("claude-3-opus-20240229"),
                                                "Opus 3"
                                            }
                                        }
                                    }
                                    div {
                                        style: "display: flex; gap: 10px; align-items: center;",
                                        label { style: "min-width: 110px; font-size: 0.9em;", "Permission mode:" }
                                        select {
                                            class: "operon-companion-model-picker",
                                            "data-testid": "project-claude-permission-picker",
                                            onchange: move |e| {
                                                let v = e.value();
                                                let next = if v == "inherit" { None } else { Some(v) };
                                                if let Err(e) = project_repo_perm.set_default_permission_mode(pid, next.as_deref()) {
                                                    tracing::warn!(
                                                        target: "operon::project_settings",
                                                        "persist project default_permission_mode failed: {e}"
                                                    );
                                                }
                                                refresh_token.with_mut(|t| *t = t.wrapping_add(1));
                                                *crate::shell::companion_state::PROJECT_SETTINGS_VERSION.write() += 1;
                                            },
                                            option { value: "inherit",
                                                selected: cur_perm.is_none(),
                                                "{inherit_perm_label}"
                                            }
                                            option { value: "acceptEdits",
                                                selected: cur_perm.as_deref() == Some("acceptEdits"),
                                                "Accept edits"
                                            }
                                            option { value: "plan",
                                                selected: cur_perm.as_deref() == Some("plan"),
                                                "Plan"
                                            }
                                            option { value: "bypassPermissions",
                                                selected: cur_perm.as_deref() == Some("bypassPermissions"),
                                                "Bypass"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                div {
                    class: "operon-modal-actions",
                    button {
                        r#type: "button",
                        class: "operon-modal-button operon-modal-button-primary",
                        "data-testid": "project-claude-settings-close",
                        onclick: close,
                        "Close"
                    }
                }
            }
        }
    }
}
