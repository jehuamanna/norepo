//! Plans-Phase-4-multiselect-aria: bulk rename modal.
//!
//! Renders a regex pattern + replacement input plus a live preview list
//! of `(old → new)` pairs. Apply iterates over the multi-selection set
//! and calls `LocalNoteRepository::rename` for each note whose title
//! matches the pattern. Project rows in the selection are skipped (the
//! existing rename flow remains the entry point for projects).

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use dioxus::prelude::*;
use operon_store::repos::LocalNoteRepository;
use uuid::Uuid;

use super::NodeKey;

#[derive(Clone, PartialEq)]
struct PreviewRow {
    id: Uuid,
    old: String,
    new: String,
}

#[component]
pub fn BulkRenameModal(
    open: Signal<bool>,
    on_applied: EventHandler<usize>,
) -> Element {
    let crate::local_mode::desktop::LocalNoteRepo(note_repo) = use_context();
    let crate::local_mode::desktop::LocalProjectRepo(project_repo) = use_context();
    let crate::local_mode::explorer::MultiSelected(multi_selected) = use_context();

    let mut pattern: Signal<String> = use_signal(String::new);
    let mut replacement: Signal<String> = use_signal(String::new);
    let error: Signal<Option<String>> = use_signal(|| None);
    let preview: Signal<Vec<PreviewRow>> = use_signal(Vec::new);

    let mut close = move || open.set(false);

    let recompute = {
        let note_repo = note_repo.clone();
        let project_repo = project_repo.clone();
        let mut preview = preview;
        let mut error = error;
        move || {
            let pat = pattern.read().clone();
            if pat.trim().is_empty() {
                preview.set(Vec::new());
                error.set(None);
                return;
            }
            let re = match regex::Regex::new(&pat) {
                Ok(r) => r,
                Err(e) => {
                    error.set(Some(e.to_string()));
                    preview.set(Vec::new());
                    return;
                }
            };
            error.set(None);
            let replacement_owned = replacement.read().clone();
            let snapshot = multi_selected.read().clone();
            let target_ids: std::collections::HashSet<Uuid> = snapshot
                .iter()
                .filter_map(|k| match k {
                    NodeKey::Note(id) => Some(*id),
                    NodeKey::Project(_) => None,
                })
                .collect();
            if target_ids.is_empty() {
                preview.set(Vec::new());
                return;
            }
            let projects = project_repo.list().unwrap_or_default();
            let mut rows = Vec::new();
            for p in projects {
                if let Ok(notes) = note_repo.list_for_project(p.id) {
                    for n in notes {
                        if !target_ids.contains(&n.id) {
                            continue;
                        }
                        let new = re.replace_all(&n.title, replacement_owned.as_str()).to_string();
                        if new != n.title {
                            rows.push(PreviewRow {
                                id: n.id,
                                old: n.title,
                                new,
                            });
                        }
                    }
                }
            }
            preview.set(rows);
        }
    };

    let mut recompute_pattern = recompute.clone();
    let mut recompute_replacement = recompute.clone();

    let apply = {
        let note_repo: Arc<dyn LocalNoteRepository> = note_repo.clone();
        let mut preview = preview;
        move |_| {
            let rows = preview.read().clone();
            let mut applied = 0;
            for row in rows.iter() {
                let trimmed = row.new.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if note_repo.rename(row.id, trimmed).is_ok() {
                    applied += 1;
                }
            }
            preview.set(Vec::new());
            on_applied.call(applied);
            open.set(false);
        }
    };

    rsx! {
        div {
            class: "operon-modal-scrim",
            "data-testid": "bulk-rename-modal",
            role: "dialog",
            "aria-modal": "true",
            "aria-labelledby": "bulk-rename-title",
            onclick: move |_| close(),
            onkeydown: move |evt| {
                if evt.key().to_string() == "Escape" {
                    evt.prevent_default();
                    close();
                }
            },
            div {
                class: "operon-modal-card",
                style: "min-width: 32rem; max-width: 90vw;",
                onclick: move |evt| evt.stop_propagation(),
                h2 {
                    id: "bulk-rename-title",
                    class: "operon-modal-title",
                    "Bulk rename"
                }
                label { class: "operon-modal-label", "Regex pattern" }
                input {
                    r#type: "text",
                    class: "operon-modal-input",
                    "data-testid": "bulk-rename-pattern",
                    placeholder: "(.*)",
                    value: "{pattern.read()}",
                    autofocus: true,
                    oninput: move |evt| {
                        pattern.set(evt.value());
                        recompute_pattern();
                    },
                }
                label { class: "operon-modal-label", "Replacement" }
                input {
                    r#type: "text",
                    class: "operon-modal-input",
                    "data-testid": "bulk-rename-replacement",
                    placeholder: "Note: $1",
                    value: "{replacement.read()}",
                    oninput: move |evt| {
                        replacement.set(evt.value());
                        recompute_replacement();
                    },
                }
                if let Some(msg) = error.read().clone() {
                    p { role: "alert", class: "operon-modal-error", "{msg}" }
                }
                {
                    let rows = preview.read().clone();
                    let count = rows.len();
                    rsx! {
                        p {
                            class: "operon-modal-help",
                            style: "font-size: 0.85em;",
                            "{count} note(s) will be renamed."
                        }
                        ul {
                            class: "operon-modal-results",
                            style: "list-style: none; padding: 0; margin: 0.5rem 0; max-height: 16rem; overflow-y: auto; font-size: 0.85em;",
                            for (i, row) in rows.iter().take(50).enumerate() {
                                li {
                                    key: "{i}-{row.id}",
                                    style: "padding: 0.15rem 0.25rem; display: flex; gap: 0.5rem;",
                                    "data-testid": "bulk-rename-preview-row",
                                    span { style: "opacity: 0.6; flex: 1; text-decoration: line-through;", "{row.old}" }
                                    span { style: "opacity: 0.4;", "→" }
                                    span { style: "flex: 1;", "{row.new}" }
                                }
                            }
                        }
                    }
                }
                div {
                    class: "operon-modal-actions",
                    button {
                        r#type: "button",
                        class: "operon-modal-button",
                        onclick: move |_| close(),
                        "Cancel"
                    }
                    button {
                        r#type: "button",
                        class: "operon-modal-button operon-modal-button-primary",
                        "data-testid": "bulk-rename-apply",
                        disabled: error.read().is_some() || preview.read().is_empty(),
                        onclick: apply,
                        "Apply"
                    }
                }
            }
        }
    }
}
