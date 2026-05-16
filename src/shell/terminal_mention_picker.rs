//! Floating note picker for terminal mode (M4d.4).
//!
//! Mounted inside `ClaudeRepoTerminal`'s outer container as an
//! absolute-positioned overlay docked at the top of the pane. Opens
//! when [`MENTION_PICKER_OPEN`] flips true — which happens when the
//! user types `@` at the claude prompt and the xterm bootstrap
//! reports `at_keypress` through the bridge.
//!
//! Picker behavior (kept minimal on purpose; iterate later):
//! - Substring match against note titles, case-insensitive, capped
//!   at [`MAX_RESULTS`] entries. No fuzzy ranking — first 20 matches
//!   in the order the repo returns them.
//! - Click a row to select. No keyboard nav for v1.
//! - Click outside / Escape closes the picker.
//! - On select: writes `[Title](note:<uuid>) ` to
//!   [`PENDING_TERMINAL_INJECTION`]. **No leading `@`** because the
//!   user already typed the `@` that triggered the open — the
//!   picker just completes the rest of the mention.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use dioxus::prelude::*;
use operon_store::repos::{LocalNoteRepository, LocalProjectRepository};
use uuid::Uuid;

use crate::local_mode::desktop::{LocalNoteRepo, LocalProjectRepo};
use crate::shell::companion_state::{
    MENTION_PICKER_OPEN, MENTION_PICKER_POS, PENDING_TERMINAL_INJECTION,
};

const MAX_RESULTS: usize = 20;

#[component]
pub fn TerminalMentionPicker() -> Element {
    let open = *MENTION_PICKER_OPEN.read();
    if !open {
        return rsx! {};
    }

    let note_repo: Option<Arc<dyn LocalNoteRepository>> =
        try_consume_context::<LocalNoteRepo>().map(|c| c.0);
    let project_repo: Option<Arc<dyn LocalProjectRepository>> =
        try_consume_context::<LocalProjectRepo>().map(|c| c.0);

    let mut query = use_signal(String::new);
    // Highlighted row index for keyboard nav. Reset to 0 whenever
    // the query changes so the first result is always pre-selected.
    let mut highlight = use_signal::<usize>(|| 0);

    // Enumerate matching notes. Cheap to recompute on each keystroke
    // for now — at the cap of MAX_RESULTS we're walking at most a
    // few thousand titles. If this becomes a hot path, memoize on
    // `query` and the note-version signal.
    let results: Vec<(Uuid, String)> = {
        let q = query.read().to_lowercase();
        let mut out: Vec<(Uuid, String)> = Vec::new();
        if let (Some(notes), Some(projects)) = (note_repo.as_ref(), project_repo.as_ref()) {
            if let Ok(projects) = projects.list() {
                'outer: for p in projects {
                    if let Ok(notes_in_project) = notes.list_for_project(p.id) {
                        for n in notes_in_project {
                            if q.is_empty() || n.title.to_lowercase().contains(&q) {
                                out.push((n.id, n.title));
                                if out.len() >= MAX_RESULTS {
                                    break 'outer;
                                }
                            }
                        }
                    }
                }
            }
        }
        out
    };

    // Keep the highlight in-bounds after the result set changes.
    // `with_mut` rather than two reads + a write to avoid a stale
    // intermediate signal value across renders.
    let result_len = results.len();
    if result_len == 0 {
        if *highlight.peek() != 0 {
            highlight.set(0);
        }
    } else if *highlight.peek() >= result_len {
        highlight.set(result_len - 1);
    }

    let close = move || {
        *MENTION_PICKER_OPEN.write() = false;
    };

    // Snapshot of (id, title) for the Enter handler's lookup. Built
    // before rendering so the closure captures a plain Vec rather
    // than a borrow into the live results.
    let results_for_enter = results.clone();

    rsx! {
        // Backdrop catches outside-clicks; transparent so it doesn't
        // obscure the terminal underneath, but its onclick closes the
        // picker. Using a sibling element (not a wrapper) so its
        // pointer events don't sit on top of the input/list.
        div {
            class: "operon-mention-picker-backdrop",
            "data-testid": "mention-picker-backdrop",
            onclick: move |_| close(),
        }
        div {
            class: "operon-mention-picker",
            "data-testid": "mention-picker",
            // Cursor anchoring: when the JS side reported pixel
            // coords (xterm could measure cell dims), emit them as
            // inline style — overrides the docked default in CSS.
            // Read once at render time; safe because the picker
            // re-renders when MENTION_PICKER_POS changes alongside
            // MENTION_PICKER_OPEN flipping true.
            style: match *MENTION_PICKER_POS.read() {
                Some((x, y)) => format!("top: {y:.1}px; left: {x:.1}px;"),
                None => String::new(),
            },
            input {
                class: "operon-mention-picker-input",
                r#type: "text",
                placeholder: "Filter notes…",
                "data-testid": "mention-picker-input",
                // Autofocus so the user can start typing immediately
                // after `@` — they don't have to click the input.
                autofocus: true,
                value: "{query.read()}",
                oninput: move |evt| {
                    query.set(evt.value());
                    // Reset the highlight whenever the result set
                    // potentially changes — first row is the
                    // natural target after a filter change.
                    highlight.set(0);
                },
                onkeydown: move |evt: KeyboardEvent| {
                    let key = evt.key();
                    match key {
                        Key::Escape => {
                            evt.prevent_default();
                            close();
                        }
                        Key::ArrowDown => {
                            evt.prevent_default();
                            if result_len > 0 {
                                let cur = *highlight.peek();
                                highlight.set((cur + 1) % result_len);
                            }
                        }
                        Key::ArrowUp => {
                            evt.prevent_default();
                            if result_len > 0 {
                                let cur = *highlight.peek();
                                highlight.set((cur + result_len - 1) % result_len);
                            }
                        }
                        Key::Enter => {
                            evt.prevent_default();
                            let idx = *highlight.peek();
                            if let Some((note_id, title)) = results_for_enter.get(idx) {
                                let token = format!("[{title}](note:{note_id}) ");
                                *PENDING_TERMINAL_INJECTION.write() = Some(token);
                                close();
                            }
                        }
                        _ => {}
                    }
                },
            }
            ul {
                class: "operon-mention-picker-list",
                role: "listbox",
                if results.is_empty() {
                    li {
                        class: "operon-mention-picker-empty",
                        "No matching notes"
                    }
                }
                for (i, (note_id, title)) in results.iter().cloned().enumerate() {
                    li {
                        key: "{note_id}",
                        class: "operon-mention-picker-item",
                        role: "option",
                        "data-note-id": "{note_id}",
                        "aria-selected": if i == *highlight.read() { "true" } else { "false" },
                        // Hovering the row also pre-selects it so a
                        // user mid-keyboard-nav who switches to the
                        // mouse doesn't have to click twice.
                        onmouseenter: move |_| highlight.set(i),
                        onclick: move |_| {
                            // The user already typed `@`. Complete
                            // the rest of the mention via the same
                            // PTY-injection signal the toolbar +
                            // drag/drop paths use.
                            let token = format!("[{title}](note:{note_id}) ");
                            *PENDING_TERMINAL_INJECTION.write() = Some(token);
                            close();
                        },
                        "{title}"
                    }
                }
            }
        }
    }
}
