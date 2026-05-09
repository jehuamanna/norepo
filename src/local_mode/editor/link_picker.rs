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
use operon_store::vfs;
use uuid::Uuid;

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
    /// Resolved note UUID for note hits — `None` for project hits.
    /// Lets callers that prefer markdown-link syntax build
    /// `[Title](operon://note/<uuid>)` instead of the wikilink form
    /// `target` carries.
    pub note_id: Option<Uuid>,
    /// Display title for the picked entry — note title for notes,
    /// project name for projects. Used as the visible link text in
    /// markdown-link callers.
    pub title: String,
}

#[component]
pub fn LinkPicker(open: Signal<bool>, on_pick: EventHandler<PickedLink>) -> Element {
    let search_repo: ExplorerSearchRepo = use_context();
    let mut query: Signal<String> = use_signal(String::new);
    // (target, breadcrumb-for-display, kind: None=project, Some(NoteKind)=note,
    //  resolved note id, display title)
    let mut results: Signal<Vec<(String, String, Option<NoteKind>, Option<Uuid>, String)>> =
        use_signal(Vec::new);
    // Plans-Phase-9-wikilink-picker (rev 2): keyboard-driven highlight
    // index. Arrow up/down moves; Enter picks the highlighted row. Reset
    // to 0 every time the result list changes so a stale index can never
    // outlive its row.
    let mut highlight: Signal<usize> = use_signal(|| 0);

    let close = use_callback(move |_: ()| {
        open.set(false);
        query.set(String::new());
        results.set(Vec::new());
        highlight.set(0);
    });

    // Helper invoked by Enter / row-click. `idx` is the row index in
    // `results.read()`. Builds the PickedLink and closes.
    let pick_at = use_callback(move |idx: usize| {
        let snap = results.read().clone();
        if let Some((target, _, note_kind, note_id, title)) = snap.get(idx).cloned() {
            let embed = matches!(note_kind, Some(NoteKind::Image));
            on_pick.call(PickedLink {
                target,
                embed,
                note_id,
                title,
            });
            close.call(());
        }
    });

    // Recompute results on every query change. Title-only search (in_content=false)
    // is enough for the picker — we just need to find candidates by name.
    {
        let search_repo = search_repo.clone();
        let mut results_setter = results;
        let mut highlight_setter = highlight;
        use_effect(move || {
            let needle = query.read().clone();
            // Reset the keyboard-highlight to the first row of every new
            // result list. Without this the index could outlive its row
            // and Enter would either pick nothing or the wrong note.
            highlight_setter.set(0);
            if needle.trim().is_empty() {
                results_setter.set(Vec::new());
                return;
            }
            let loader = |_id: uuid::Uuid| -> Option<String> { None };
            match search_repo.0.search(needle.trim(), false, 50, &loader) {
                Ok(hits) => {
                    let rows: Vec<(String, String, Option<NoteKind>, Option<Uuid>, String)> =
                        hits
                            .into_iter()
                            .map(|h| match h.kind {
                                SearchKind::Project => (
                                    h.title.clone(),
                                    h.breadcrumb,
                                    None,
                                    None,
                                    h.title,
                                ),
                                SearchKind::Note => {
                                    // Plans-Phase-9-wikilink-picker (rev 2):
                                    // emit the FULL parent-path target (the
                                    // search-repo breadcrumb walks parent_id
                                    // and produces "Project / Folder / .../
                                    // Title") AND append `^short` so duplicate
                                    // titles still resolve uniquely. The vfs
                                    // Nested form parses both the path and
                                    // the short id.
                                    let path = h.breadcrumb.replace(" / ", "/");
                                    let target =
                                        format!("{}^{}", path, vfs::short_id(h.id));
                                    (target, h.breadcrumb, h.note_kind, Some(h.id), h.title)
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
            onclick: move |_| close.call(()),
            // Plans-Phase-9-wikilink-picker (rev 2): keyboard nav.
            // Arrow up/down moves the highlight; Enter picks; Escape
            // closes. Native textarea-style key repeat works because
            // Dioxus delivers each repeat as a separate event.
            onkeydown: move |evt| {
                let key = evt.key().to_string();
                let len = results.read().len();
                match key.as_str() {
                    "Escape" => {
                        evt.prevent_default();
                        close.call(());
                    }
                    "ArrowDown" if len > 0 => {
                        evt.prevent_default();
                        let cur = *highlight.read();
                        highlight.set((cur + 1).min(len - 1));
                    }
                    "ArrowUp" if len > 0 => {
                        evt.prevent_default();
                        let cur = *highlight.read();
                        highlight.set(cur.saturating_sub(1));
                    }
                    "Enter" if len > 0 => {
                        evt.prevent_default();
                        let idx = (*highlight.read()).min(len - 1);
                        pick_at.call(idx);
                    }
                    _ => {}
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
                    role: "listbox",
                    style: "list-style: none; padding: 0; margin: 0.5rem 0; max-height: 16rem; overflow-y: auto;",
                    for (i, (target, breadcrumb, note_kind, _note_id, _title)) in results.read().iter().cloned().enumerate() {
                        li {
                            key: "{i}-{target}",
                            // Plans-Phase-9-wikilink-picker (rev 2):
                            // visual highlight tracks the keyboard
                            // selection. `aria-selected` exposes it to
                            // assistive tech; the data-testid attr is
                            // keyed for the e2e selector.
                            class: if i == *highlight.read() {
                                "operon-modal-result operon-modal-result-selected"
                            } else {
                                "operon-modal-result"
                            },
                            role: "option",
                            "aria-selected": if i == *highlight.read() { "true" } else { "false" },
                            style: "padding: 0.25rem 0.5rem; cursor: pointer; border-radius: 0.25rem;",
                            "data-testid": "link-picker-result",
                            "data-highlighted": if i == *highlight.read() { "true" } else { "false" },
                            "data-note-kind": match note_kind {
                                Some(k) => k.as_str(),
                                None => "project",
                            },
                            onmouseenter: move |_| highlight.set(i),
                            onclick: move |evt| {
                                evt.stop_propagation();
                                pick_at.call(i);
                            },
                            // Plans-Phase-9-wikilink-picker (rev 1): kind
                            // badge mirrors the explorer-row convention
                            // (note_row.rs:651-663) so the user can see at a
                            // glance whether picking will insert `[[…]]` or
                            // `![[…]]`. Project hits get no badge — they
                            // can't be embedded.
                            if let Some(kind) = note_kind {
                                {
                                    let icon = kind.icon();
                                    let kind_str = kind.as_str();
                                    rsx! {
                                        span {
                                            class: "kind-badge kind-{kind_str} text-[0.65rem] mr-1 px-1 rounded select-none opacity-60",
                                            "data-testid": "kind-badge",
                                            "data-note-kind": "{kind_str}",
                                            "[{icon}]"
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
                        onclick: move |_| close.call(()),
                        "Cancel"
                    }
                }
            }
        }
    }
}
