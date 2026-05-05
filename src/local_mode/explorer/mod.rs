//! Local-Mode explorer panel: lists `local_project` rows with rename/delete
//! and a "+" button to create a new (default-named) project.

mod project_row;

pub use project_row::ProjectRow;

use dioxus::prelude::*;
use operon_store::repos::LocalProject;
use uuid::Uuid;

use crate::local_mode::desktop::LocalProjectRepo;
use crate::local_mode::ui::ConfirmDialog;

/// App-scope signal: bumped on every successful project mutation. The panel
/// re-fetches its row list whenever this changes.
#[derive(Clone, Copy)]
pub struct LocalProjectVersion(pub Signal<u64>);

/// App-scope signal: id of the currently selected project, if any.
#[derive(Clone, Copy)]
pub struct SelectedProject(pub Signal<Option<Uuid>>);

#[component]
pub fn ExplorerPanel() -> Element {
    let LocalProjectRepo(repo) = use_context();
    let LocalProjectVersion(mut version) = use_context();
    let SelectedProject(mut selected) = use_context();

    // Re-fetch when the version signal bumps.
    let projects: Signal<Vec<LocalProject>> = use_signal(Vec::new);
    let mut projects_setter = projects;
    {
        let repo = repo.clone();
        use_effect(move || {
            let _ = version.read();
            match repo.list() {
                Ok(rows) => projects_setter.set(rows),
                Err(e) => eprintln!("operon: list local_project failed: {e}"),
            }
        });
    }

    let renaming: Signal<Option<Uuid>> = use_signal(|| None);
    let pending_delete: Signal<Option<Uuid>> = use_signal(|| None);
    let mut renaming_setter = renaming;
    let mut pending_delete_setter = pending_delete;

    let on_select = use_callback(move |id: Uuid| {
        selected.set(Some(id));
    });

    let repo_for_create = repo.clone();
    let on_add = move |_| match repo_for_create.create("") {
        Ok(p) => {
            version.with_mut(|v| *v += 1);
            selected.set(Some(p.id));
            renaming_setter.set(Some(p.id));
        }
        Err(e) => eprintln!("operon: create local_project failed: {e}"),
    };

    let repo_for_rename = repo.clone();
    let on_rename = use_callback(move |(id, new_name): (Uuid, String)| {
        // Empty string is the inline-rename "cancel" sentinel — just exit rename
        // mode without touching the DB.
        if new_name.trim().is_empty() {
            renaming_setter.set(None);
            return;
        }
        match repo_for_rename.rename(id, &new_name) {
            Ok(()) => {
                version.with_mut(|v| *v += 1);
                renaming_setter.set(None);
            }
            Err(e) => {
                eprintln!("operon: rename local_project failed: {e}");
                renaming_setter.set(None);
            }
        }
    });

    let on_request_rename = use_callback(move |id: Uuid| {
        renaming_setter.set(Some(id));
    });

    let on_request_delete = use_callback(move |id: Uuid| {
        pending_delete_setter.set(Some(id));
    });

    // The actual delete happens from the confirm dialog below.
    let on_delete = use_callback(move |_id: Uuid| {});

    let projects_snapshot = projects.read().clone();
    let renaming_now = *renaming.read();
    let selected_now = *selected.read();

    let pending_delete_id = *pending_delete.read();
    let pending_delete_name = pending_delete_id.and_then(|did| {
        projects_snapshot
            .iter()
            .find(|p| p.id == did)
            .map(|p| p.name.clone())
    });

    let repo_for_delete = repo.clone();

    rsx! {
        div {
            class: "flex flex-col h-full w-full bg-[var(--operon-bg)] text-[var(--operon-fg)] border-r border-[var(--operon-border)]",
            "data-testid": "explorer-panel",
            // Header
            div {
                class: "flex items-center gap-2 px-2 py-2 border-b border-[var(--operon-border)]",
                input {
                    r#type: "search",
                    class: "flex-1 px-2 py-1 text-xs bg-[var(--operon-input-bg)] border border-[var(--operon-border)] rounded",
                    "data-testid": "explorer-search",
                    placeholder: "Search projects",
                    disabled: true,
                }
                button {
                    r#type: "button",
                    class: "w-7 h-7 inline-flex items-center justify-center rounded border border-[var(--operon-border)] hover:bg-[var(--operon-hover)] text-base leading-none",
                    "data-testid": "explorer-add-project",
                    "aria-label": "New project",
                    onclick: on_add,
                    "+"
                }
            }
            // Rows
            div {
                class: "flex-1 overflow-y-auto",
                if projects_snapshot.is_empty() {
                    div {
                        class: "px-3 py-6 text-xs opacity-60 text-center",
                        "data-testid": "explorer-empty",
                        "No projects yet. Click + to create one."
                    }
                } else {
                    for project in projects_snapshot.iter().cloned() {
                        ProjectRow {
                            key: "{project.id}",
                            project: project.clone(),
                            selected: selected_now == Some(project.id),
                            in_rename: renaming_now == Some(project.id),
                            on_select: on_select,
                            on_rename: on_rename,
                            on_delete: on_delete,
                            on_request_rename: on_request_rename,
                            on_request_delete: on_request_delete,
                        }
                    }
                }
            }
        }
        if let Some(did) = pending_delete_id {
            ConfirmDialog {
                title: "Delete project".to_string(),
                message: format!(
                    "Delete project \"{}\"?\nThis cannot be undone.",
                    pending_delete_name.clone().unwrap_or_default()
                ),
                confirm_label: "Delete".to_string(),
                on_confirm: Callback::new(move |_| {
                    match repo_for_delete.delete(did) {
                        Ok(()) => {
                            version.with_mut(|v| *v += 1);
                            if selected_now == Some(did) {
                                selected.set(None);
                            }
                        }
                        Err(e) => eprintln!("operon: delete local_project failed: {e}"),
                    }
                    pending_delete_setter.set(None);
                }),
                on_cancel: Callback::new(move |_| {
                    pending_delete_setter.set(None);
                }),
            }
        }
    }
}
