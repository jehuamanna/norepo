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
use operon_store::repos::SearchKind;

use crate::local_mode::explorer::ExplorerSearchRepo;

#[component]
pub fn LinkPicker(open: Signal<bool>, on_pick: EventHandler<String>) -> Element {
    let search_repo: ExplorerSearchRepo = use_context();
    let mut query: Signal<String> = use_signal(String::new);
    let mut results: Signal<Vec<(String, String)>> = use_signal(Vec::new);
    // (target, breadcrumb-for-display)

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
                    let rows: Vec<(String, String)> = hits
                        .into_iter()
                        .map(|h| match h.kind {
                            SearchKind::Project => (h.title.clone(), h.breadcrumb),
                            SearchKind::Note => {
                                // Compose "Project/Title" so vfs::resolve_link
                                // treats it as Absolute and resolves uniquely.
                                let target = h.breadcrumb.replace(" / ", "/");
                                (target, h.breadcrumb)
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
                    for (i, (target, breadcrumb)) in results.read().iter().cloned().enumerate() {
                        li {
                            key: "{i}-{target}",
                            class: "operon-modal-result",
                            style: "padding: 0.25rem 0.5rem; cursor: pointer; border-radius: 0.25rem;",
                            "data-testid": "link-picker-result",
                            onclick: move |evt| {
                                evt.stop_propagation();
                                on_pick.call(target.clone());
                                close();
                            },
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
