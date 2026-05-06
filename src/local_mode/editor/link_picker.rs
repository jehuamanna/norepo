//! Plans-Phase-5-vfs-wikilinks: Insert-link picker.
//!
//! Mounts a focus-trapped popover with a search input over
//! `LocalSearchRepository`. Picking a result calls `on_pick` with the
//! wikilink target string (for "Project / Note" hits the picker emits
//! `Project/Note`; for project-only hits it emits the project name as a
//! relative form). The caller wraps the chosen target with `[[…]]` and
//! inserts it into the editor body.

#![cfg(not(target_arch = "wasm32"))]

use dioxus::prelude::*;
use operon_store::repos::{NoteKind, SearchKind};

use crate::local_mode::explorer::ExplorerSearchRepo;

/// What the user picked from `LinkPicker`. The caller decides whether to
/// insert the markdown link form `[[target]]` or the embed form
/// `![[target]]` — the picker just reports the chosen target plus the
/// kind hint so the caller can branch.
///
/// Plans-Phase-9-wikilink-picker (rev 1): `embed = true` when the picked
/// hit is a `NoteKind::Image` row. Project hits and markdown notes are
/// `embed = false`.
#[derive(Clone, Debug, PartialEq)]
pub struct PickedLink {
    pub target: String,
    pub embed: bool,
}

#[component]
pub fn LinkPicker(open: Signal<bool>, on_pick: EventHandler<PickedLink>) -> Element {
    let search_repo: ExplorerSearchRepo = use_context();
    let mut query: Signal<String> = use_signal(String::new);
    // (target, breadcrumb-for-display, kind: None=project, Some(NoteKind)=note)
    let mut results: Signal<Vec<(String, String, Option<NoteKind>)>> = use_signal(Vec::new);

    let mut close = move || {
        open.set(false);
        query.set(String::new());
        results.set(Vec::new());
    };

    // Recompute results on every query change. Title-only search (in_content=false)
    // is enough for the picker — we just need to find candidates by name.
    {
        let search_repo = search_repo.clone();
        let mut results_setter = results;
        use_effect(move || {
            let needle = query.read().clone();
            if needle.trim().is_empty() {
                results_setter.set(Vec::new());
                return;
            }
            let loader = |_id: uuid::Uuid| -> Option<String> { None };
            match search_repo.0.search(needle.trim(), false, 50, &loader) {
                Ok(hits) => {
                    let rows: Vec<(String, String, Option<NoteKind>)> = hits
                        .into_iter()
                        .map(|h| match h.kind {
                            SearchKind::Project => (h.title.clone(), h.breadcrumb, None),
                            SearchKind::Note => {
                                // Compose "Project/Title" so vfs::resolve_link
                                // treats it as Absolute and resolves uniquely.
                                let target = h.breadcrumb.replace(" / ", "/");
                                (target, h.breadcrumb, h.note_kind)
                            }
                        })
                        .collect();
                    results_setter.set(rows);
                }
                Err(e) => {
                    eprintln!("operon: link picker search failed: {e}");
                    results_setter.set(Vec::new());
                }
            }
        });
    }

    rsx! {
        div {
            class: "operon-modal-scrim",
            "data-testid": "link-picker",
            role: "dialog",
            "aria-modal": "true",
            "aria-labelledby": "link-picker-title",
            onclick: move |_| close(),
            onkeydown: move |evt| {
                if evt.key().to_string() == "Escape" {
                    evt.prevent_default();
                    close();
                }
            },
            div {
                class: "operon-modal-card",
                style: "min-width: 28rem;",
                onclick: move |evt| evt.stop_propagation(),
                h2 {
                    id: "link-picker-title",
                    class: "operon-modal-title",
                    "Insert link"
                }
                input {
                    r#type: "text",
                    class: "operon-modal-input",
                    "data-testid": "link-picker-query",
                    placeholder: "Search notes by title…",
                    value: "{query.read()}",
                    autofocus: true,
                    oninput: move |evt| query.set(evt.value()),
                }
                ul {
                    class: "operon-modal-results",
                    style: "list-style: none; padding: 0; margin: 0.5rem 0; max-height: 16rem; overflow-y: auto;",
                    for (i, (target, breadcrumb, note_kind)) in results.read().iter().cloned().enumerate() {
                        li {
                            key: "{i}-{target}",
                            class: "operon-modal-result",
                            style: "padding: 0.25rem 0.5rem; cursor: pointer; border-radius: 0.25rem;",
                            "data-testid": "link-picker-result",
                            "data-note-kind": match note_kind {
                                Some(NoteKind::Markdown) => "markdown",
                                Some(NoteKind::Image) => "image",
                                None => "project",
                            },
                            onclick: move |evt| {
                                evt.stop_propagation();
                                let embed = matches!(note_kind, Some(NoteKind::Image));
                                on_pick.call(PickedLink { target: target.clone(), embed });
                                close();
                            },
                            // Plans-Phase-9-wikilink-picker (rev 1): kind
                            // badge mirrors the explorer-row convention
                            // (note_row.rs:651-663) so the user can see at a
                            // glance whether picking will insert `[[…]]` or
                            // `![[…]]`. Project hits get no badge — they
                            // can't be embedded.
                            if let Some(kind) = note_kind {
                                {
                                    let (label, css) = match kind {
                                        NoteKind::Markdown => ("[md]", "kind-badge kind-md"),
                                        NoteKind::Image => ("[im]", "kind-badge kind-im"),
                                    };
                                    rsx! {
                                        span {
                                            class: "{css} text-[0.65rem] mr-1 px-1 rounded select-none opacity-60",
                                            "data-testid": "kind-badge",
                                            "data-note-kind": match kind {
                                                NoteKind::Markdown => "markdown",
                                                NoteKind::Image => "image",
                                            },
                                            "{label}"
                                        }
                                    }
                                }
                            }
                            "{breadcrumb}"
                        }
                    }
                }
                if !query.read().trim().is_empty() && results.read().is_empty() {
                    p {
                        class: "operon-modal-help",
                        style: "font-size: 0.85em; opacity: 0.7;",
                        "No matches."
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
                }
            }
        }
    }
}
