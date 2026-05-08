//! Companion-pane left rail: scope tabs + per-scope session list.
//!
//! The rail is the user's primary affordance for managing parallel chats.
//! It shows two scope tabs at the top — `Project: <name>` and `Global`
//! (vault) — and renders the rows from `chat_session` filtered by the
//! active scope, sorted most-recently-used first. A `+ New chat` row at
//! the top creates a fresh session in the active scope and selects it.

#![cfg(not(target_arch = "wasm32"))]

use dioxus::prelude::*;
use operon_store::repos::{ChatScope, ChatSession, LocalProject, LocalProjectRepository};
use std::sync::Arc;
use uuid::Uuid;

use crate::local_mode::desktop::{LocalNoteRepo, LocalProjectRepo};
use crate::local_mode::explorer::{LocalNoteVersion, SelectedNote, SelectedProject};
use crate::local_mode::ui::InlineRename;
use crate::shell::companion_state::{
    ActiveChatScope, ActiveChatSession, ChatSessionRepo, ChatSessionVersion,
};

#[component]
pub fn SessionRail() -> Element {
    // Hotfix: every context lookup uses `try_consume_context` so a missing
    // provider (e.g., the rail mounting during NonLocal mode, or before
    // `provide_local_app_signals` ran) renders an empty rail instead of
    // panicking and nuking the whole companion (which can cascade and
    // wipe sibling regions of the Shell tree).
    let project_repo = match try_consume_context::<LocalProjectRepo>() {
        Some(LocalProjectRepo(r)) => r,
        None => return rsx! { div { class: "operon-companion-rail" } },
    };
    let session_repo = match try_consume_context::<ChatSessionRepo>() {
        Some(ChatSessionRepo(r)) => r,
        None => return rsx! { div { class: "operon-companion-rail" } },
    };
    let active_scope = match try_consume_context::<ActiveChatScope>() {
        Some(ActiveChatScope(s)) => s,
        None => return rsx! { div { class: "operon-companion-rail" } },
    };
    let active_session = match try_consume_context::<ActiveChatSession>() {
        Some(ActiveChatSession(s)) => s,
        None => return rsx! { div { class: "operon-companion-rail" } },
    };
    let version = match try_consume_context::<ChatSessionVersion>() {
        Some(ChatSessionVersion(v)) => v,
        None => return rsx! { div { class: "operon-companion-rail" } },
    };
    let selected_project = match try_consume_context::<SelectedProject>() {
        Some(SelectedProject(s)) => s,
        None => return rsx! { div { class: "operon-companion-rail" } },
    };
    let selected_note = match try_consume_context::<SelectedNote>() {
        Some(SelectedNote(s)) => s,
        None => return rsx! { div { class: "operon-companion-rail" } },
    };
    let note_repo = match try_consume_context::<LocalNoteRepo>() {
        Some(LocalNoteRepo(r)) => r,
        None => return rsx! { div { class: "operon-companion-rail" } },
    };
    let project_version = match try_consume_context::<crate::local_mode::LocalProjectVersion>() {
        Some(crate::local_mode::LocalProjectVersion(v)) => v,
        None => return rsx! { div { class: "operon-companion-rail" } },
    };
    let note_version = match try_consume_context::<LocalNoteVersion>() {
        Some(LocalNoteVersion(v)) => v,
        None => return rsx! { div { class: "operon-companion-rail" } },
    };

    // Effective project for the rail's Project tab: the explorer's
    // `SelectedProject` if set, otherwise derived from the note the user
    // currently has selected (so opening a note flips the rail to its
    // project's chats — same intent as VS Code's per-workspace rail).
    let note_repo_for_effective = note_repo.clone();
    let effective_project_id: Memo<Option<Uuid>> = use_memo(move || {
        let _ = note_version.read();
        if let Some(pid) = *selected_project.read() {
            return Some(pid);
        }
        let nid = (*selected_note.read())?;
        note_repo_for_effective
            .find_project_for_note(nid)
            .ok()
            .flatten()
    });

    // Active project (for tab labels + Project tab enable). Re-fetches when
    // either selection or the project list changes.
    let project_repo_for_lookup = project_repo.clone();
    let active_project_lookup = use_memo(move || {
        let _ = project_version.read();
        let id = (*effective_project_id.read())?;
        project_repo_for_lookup
            .list()
            .ok()
            .and_then(|projects| projects.into_iter().find(|p| p.id == id))
    });

    // Auto-flip scope to Project whenever the effective project changes
    // value (explorer selects a project, OR opens / clicks a note from a
    // different project). Inline render-body sync (use_effect was
    // unreliable in this scope). The `prev_effective_project` peek-and-
    // update guard means the flip only fires on a true selection change,
    // so a user who manually clicks the Vault tab while a project is
    // active stays on Vault until they switch to a different project.
    let prev_effective_project: Signal<Option<Uuid>> = use_signal(|| None);
    {
        let mut prev_setter = prev_effective_project;
        let mut scope_setter = active_scope;
        let cur_effective = *effective_project_id.read();
        let prev = *prev_setter.peek();
        if cur_effective != prev {
            prev_setter.set(cur_effective);
            match cur_effective {
                Some(id) => {
                    let target = ChatScope::Project(id);
                    if *scope_setter.peek() != target {
                        scope_setter.set(target);
                    }
                }
                None => {
                    if matches!(*scope_setter.peek(), ChatScope::Project(_)) {
                        scope_setter.set(ChatScope::Vault);
                    }
                }
            }
        }
    }

    // Session list filtered by scope; refetched when scope or version
    // changes.
    let session_repo_for_list = session_repo.clone();
    let sessions = use_memo(move || -> Vec<ChatSession> {
        let _ = version.read();
        let scope = *active_scope.read();
        session_repo_for_list
            .list_in_scope(scope)
            .unwrap_or_else(|e| {
                tracing::warn!(
                    target: "operon::companion",
                    "list chat sessions in {scope:?} failed: {e}"
                );
                Vec::new()
            })
    });

    // Auto-select the most-recent session when the active scope has none
    // selected (or the selected one disappeared after a delete).
    {
        let sessions = sessions;
        let mut active_session_setter = active_session;
        use_effect(move || {
            let list = sessions.read();
            let cur = *active_session_setter.read();
            let still_present = cur
                .map(|id| list.iter().any(|s| s.id == id))
                .unwrap_or(false);
            if !still_present {
                active_session_setter.set(list.first().map(|s| s.id));
            }
        });
    }

    let project_label = active_project_lookup
        .read()
        .as_ref()
        .map(|p| p.name.clone());
    let project_id = active_project_lookup.read().as_ref().map(|p| p.id);
    let scope_now = *active_scope.read();
    let on_project_tab = matches!(scope_now, ChatScope::Project(_));
    let on_vault_tab = matches!(scope_now, ChatScope::Vault);

    let pick_project_tab = {
        let mut active_scope = active_scope;
        let pid = project_id;
        Callback::new(move |_| {
            if let Some(id) = pid {
                active_scope.set(ChatScope::Project(id));
            }
        })
    };
    let pick_vault_tab = {
        let mut active_scope = active_scope;
        Callback::new(move |_| active_scope.set(ChatScope::Vault))
    };

    let make_new_session = {
        let session_repo = session_repo.clone();
        let mut active_session = active_session;
        let mut version = version;
        Callback::new(move |_| {
            let scope = *active_scope.read();
            match session_repo.create(scope, "New chat") {
                Ok(s) => {
                    active_session.set(Some(s.id));
                    version.with_mut(|v| *v += 1);
                }
                Err(e) => tracing::warn!(target: "operon::companion", "create session: {e}"),
            }
        })
    };

    let session_rows = sessions.read().clone();
    let active_now = *active_session.read();
    // Tracks which row (if any) is currently being inline-renamed. Set by
    // double-click on a row's label; cleared by InlineRename's commit /
    // cancel callbacks.
    let renaming_session: Signal<Option<Uuid>> = use_signal(|| None);

    rsx! {
        nav { class: "operon-companion-rail",
            "data-testid": "companion-rail",
            div { class: "operon-companion-rail-tabs",
                button {
                    r#type: "button",
                    class: if on_project_tab {
                        "operon-companion-rail-tab operon-companion-rail-tab-active"
                    } else {
                        "operon-companion-rail-tab"
                    },
                    "data-testid": "companion-rail-tab-project",
                    disabled: project_id.is_none(),
                    onclick: pick_project_tab,
                    title: project_label.clone().unwrap_or_else(|| "no project selected".into()),
                    if let Some(name) = project_label.clone() {
                        "Project: "
                        span { class: "truncate", "{name}" }
                    } else {
                        "Project"
                    }
                }
                button {
                    r#type: "button",
                    class: if on_vault_tab {
                        "operon-companion-rail-tab operon-companion-rail-tab-active"
                    } else {
                        "operon-companion-rail-tab"
                    },
                    "data-testid": "companion-rail-tab-vault",
                    onclick: pick_vault_tab,
                    "Global"
                }
            }
            button {
                r#type: "button",
                class: "operon-companion-rail-new",
                "data-testid": "companion-rail-new",
                onclick: make_new_session,
                "+ New chat"
            }
            ul { class: "operon-companion-rail-list",
                "data-testid": "companion-rail-list",
                for s in session_rows.iter().cloned() {
                    {
                        let sid = s.id;
                        let is_active = active_now == Some(sid);
                        let in_rename = *renaming_session.read() == Some(sid);
                        let label = s.label.clone();
                        let session_repo_for_row = session_repo.clone();
                        let session_repo_for_rename = session_repo.clone();
                        let mut version_for_delete = version;
                        let mut active_for_delete = active_session;
                        let mut renaming_for_dblclick = renaming_session;
                        let mut renaming_for_commit = renaming_session;
                        let mut renaming_for_cancel = renaming_session;
                        let mut version_for_rename = version;
                        let on_select = {
                            let mut active = active_session;
                            let session_repo = session_repo.clone();
                            let mut version = version;
                            Callback::new(move |_| {
                                active.set(Some(sid));
                                if let Err(e) = session_repo.touch(sid) {
                                    tracing::warn!(target: "operon::companion", "touch session: {e}");
                                }
                                version.with_mut(|v| *v += 1);
                            })
                        };
                        let on_dblclick = Callback::new(move |evt: Event<MouseData>| {
                            evt.stop_propagation();
                            renaming_for_dblclick.set(Some(sid));
                        });
                        let on_rename_commit = Callback::new(move |new_label: String| {
                            let trimmed = new_label.trim();
                            if !trimmed.is_empty() {
                                if let Err(e) =
                                    session_repo_for_rename.rename(sid, trimmed)
                                {
                                    tracing::warn!(
                                        target: "operon::companion",
                                        "rename session: {e}"
                                    );
                                }
                                version_for_rename.with_mut(|v| *v += 1);
                            }
                            renaming_for_commit.set(None);
                        });
                        let on_rename_cancel = Callback::new(move |_| {
                            renaming_for_cancel.set(None);
                        });
                        let on_delete = Callback::new(move |evt: Event<MouseData>| {
                            evt.stop_propagation();
                            if let Err(e) = session_repo_for_row.delete(sid) {
                                tracing::warn!(target: "operon::companion", "delete session: {e}");
                                return;
                            }
                            if active_for_delete.read().as_ref() == Some(&sid) {
                                active_for_delete.set(None);
                            }
                            version_for_delete.with_mut(|v| *v += 1);
                        });
                        rsx! {
                            li {
                                key: "{sid}",
                                class: if is_active {
                                    "operon-companion-rail-item operon-companion-rail-item-active"
                                } else {
                                    "operon-companion-rail-item"
                                },
                                "data-testid": "companion-rail-item",
                                "data-session-id": "{sid}",
                                "data-active": if is_active { "true" } else { "false" },
                                onclick: on_select,
                                ondoubleclick: on_dblclick,
                                if in_rename {
                                    InlineRename {
                                        initial: label.clone(),
                                        on_commit: on_rename_commit,
                                        on_cancel: on_rename_cancel,
                                    }
                                } else {
                                    span {
                                        class: "operon-companion-rail-item-label truncate",
                                        title: "Double-click to rename",
                                        "{label}"
                                    }
                                }
                                button {
                                    r#type: "button",
                                    class: "operon-companion-rail-item-delete",
                                    "data-testid": "companion-rail-item-delete",
                                    title: "Delete chat",
                                    onclick: on_delete,
                                    "\u{2715}"
                                }
                            }
                        }
                    }
                }
                if session_rows.is_empty() {
                    li {
                        class: "operon-companion-rail-empty",
                        "data-testid": "companion-rail-empty",
                        "No chats yet — click + to start"
                    }
                }
            }
        }
    }
}

// Small helper indirection so `LocalProject` lookup compiles even with a
// thinner project_repo trait surface in the future.
#[allow(dead_code)]
fn lookup_project(
    project_repo: &Arc<dyn LocalProjectRepository>,
    id: Uuid,
) -> Option<LocalProject> {
    project_repo
        .list()
        .ok()
        .and_then(|ps| ps.into_iter().find(|p| p.id == id))
}
