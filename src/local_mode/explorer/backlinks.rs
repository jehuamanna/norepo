//! Plans-Phase-5-vfs-wikilinks: backlinks pane.
//!
//! Side panel that lists every note whose body references the currently
//! selected note via `[[…]]` or `![[…]]`. Driven by
//! `LocalNoteLinkRepository::referrers_of`. Clicking an entry opens that
//! referring note in a tab.

#![cfg(not(target_arch = "wasm32"))]

use dioxus::prelude::*;
use uuid::Uuid;

use crate::local_mode::desktop::{LocalNoteLinkRepo, LocalNoteRepo, LocalProjectRepo};
use crate::local_mode::editor::open_local_note_tab;
use crate::local_mode::explorer::{LocalNoteVersion, SelectedNote};
use crate::tabs::{SaveScheduler, TabManager};

#[derive(Clone, PartialEq)]
struct Backref {
    id: Uuid,
    title: String,
    breadcrumb: String,
}

#[component]
pub fn BacklinksPanel() -> Element {
    let SelectedNote(selected_note) = use_context();
    let LocalNoteLinkRepo(link_repo) = use_context();
    let LocalNoteRepo(note_repo) = use_context();
    let LocalProjectRepo(project_repo) = use_context();
    let LocalNoteVersion(note_version) = use_context();
    let tabs: Signal<TabManager> = use_context();
    let scheduler: SaveScheduler = use_context();
    let SelectedNote(mut selected_setter) = use_context::<SelectedNote>();

    // Recompute on selection change or note version bump.
    let backrefs: Signal<Vec<Backref>> = use_signal(Vec::new);
    {
        let link_repo = link_repo.clone();
        let note_repo = note_repo.clone();
        let project_repo = project_repo.clone();
        let mut backrefs = backrefs;
        use_effect(move || {
            // Touch dependencies so the effect re-runs.
            let nv = *note_version.read();
            let _ = nv;
            let Some(active_id) = *selected_note.read() else {
                backrefs.set(Vec::new());
                return;
            };
            let referrers = link_repo.referrers_of(active_id).unwrap_or_default();
            if referrers.is_empty() {
                backrefs.set(Vec::new());
                return;
            }
            let projects = project_repo.list().unwrap_or_default();
            let mut by_id: std::collections::HashMap<Uuid, (String, String)> =
                std::collections::HashMap::new();
            for p in &projects {
                if let Ok(notes) = note_repo.list_for_project(p.id) {
                    for n in notes {
                        by_id.insert(n.id, (n.title, p.name.clone()));
                    }
                }
            }
            let mut rows: Vec<Backref> = Vec::new();
            for id in referrers {
                if let Some((title, proj)) = by_id.get(&id).cloned() {
                    rows.push(Backref {
                        id,
                        breadcrumb: format!("{proj} / {title}"),
                        title,
                    });
                }
            }
            rows.sort_by(|a, b| a.breadcrumb.cmp(&b.breadcrumb));
            backrefs.set(rows);
        });
    }

    let rows = backrefs.read().clone();
    if rows.is_empty() {
        return rsx! { Fragment {} };
    }

    rsx! {
        section {
            class: "operon-backlinks-panel",
            "data-testid": "backlinks-panel",
            "aria-label": "Linked mentions",
            style: "border-top: 1px solid var(--operon-border); padding: 0.5rem; font-size: 0.85em;",
            h3 {
                style: "margin: 0 0 0.25rem 0; font-size: 0.85em; opacity: 0.7;",
                "Linked from ({rows.len()})"
            }
            ul {
                style: "list-style: none; padding: 0; margin: 0; max-height: 12rem; overflow-y: auto;",
                for row in rows.iter().cloned() {
                    li {
                        key: "{row.id}",
                        "data-testid": "backlink-row",
                        "data-note-id": "{row.id}",
                        style: "padding: 0.15rem 0.25rem; cursor: pointer; border-radius: 0.25rem;",
                        onclick: {
                            let scheduler = scheduler.clone();
                            move |_| {
                                open_local_note_tab(
                                    tabs,
                                    scheduler.clone(),
                                    row.id,
                                    row.title.clone(),
                                    String::new(),
                                );
                                selected_setter.set(Some(row.id));
                            }
                        },
                        "{row.breadcrumb}"
                    }
                }
            }
        }
    }
}
