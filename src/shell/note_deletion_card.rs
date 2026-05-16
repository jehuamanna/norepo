//! UI for `OperonDeleteNoteTool` confirmation cards.
//!
//! Rendered alongside `NoteProposalCard` in the companion chat
//! surface when `NOTE_DELETION_PROPOSALS` is non-empty. Shows the
//! note title, the number of descendants that would be deleted along
//! with it (cascading via the FK constraint), and Accept / Reject
//! buttons. Buttons resolve the parked oneshot responder via
//! `accept_note_deletion` / `reject_note_deletion`, waking the
//! blocked `delete_note` tool call so it either commits the delete
//! or returns a "user rejected" error to Claude.
//!
//! Visually distinct from the edit-proposal card on purpose — copy
//! is destructive and short ("Delete '<title>'? N descendants will
//! be removed."), no diff body to read.

#![cfg(not(target_arch = "wasm32"))]

use dioxus::prelude::*;

use crate::shell::companion_state::{
    accept_note_deletion, reject_note_deletion, NoteDeletionProposalEntry, NoteProposalStatus,
};

#[derive(Props, Clone, PartialEq)]
pub struct NoteDeletionCardProps {
    pub entry: NoteDeletionProposalEntry,
    pub status: NoteProposalStatus,
}

#[component]
pub fn NoteDeletionCard(props: NoteDeletionCardProps) -> Element {
    let entry = props.entry;
    let status = props.status;
    let pending = matches!(status, NoteProposalStatus::Pending);

    let id_for_accept = entry.id.clone();
    let id_for_reject = entry.id.clone();

    let badge = match status {
        NoteProposalStatus::Pending => ("pending", "Confirm deletion"),
        NoteProposalStatus::Accepted => ("accepted", "Deleted"),
        NoteProposalStatus::Rejected => ("rejected", "Kept"),
    };

    // Pluralization keeps the copy from rendering "1 descendants".
    // Zero descendants is also possible (leaf note) and gets a
    // dedicated short form so the user doesn't read "0 descendants".
    let descendants_line = match entry.descendant_count {
        0 => "Only this note will be removed.".to_string(),
        1 => "1 child note will also be removed.".to_string(),
        n => format!("{n} descendant notes will also be removed."),
    };

    rsx! {
        div {
            class: "operon-note-deletion",
            "data-testid": "note-deletion-card",
            "data-status": badge.0,
            "data-proposal-id": "{entry.id}",
            header {
                class: "operon-note-deletion-header",
                span { class: "operon-note-deletion-badge", "{badge.1}" }
                span { class: "operon-note-deletion-title",
                    "{entry.note_title}"
                }
            }
            p {
                class: "operon-note-deletion-body",
                "{descendants_line}"
            }
            if pending {
                div {
                    class: "operon-note-deletion-actions",
                    button {
                        class: "operon-note-deletion-reject",
                        r#type: "button",
                        "data-testid": "note-deletion-reject",
                        onclick: move |_| {
                            reject_note_deletion(&id_for_reject);
                        },
                        "Keep"
                    }
                    button {
                        class: "operon-note-deletion-accept",
                        r#type: "button",
                        "data-testid": "note-deletion-accept",
                        onclick: move |_| {
                            accept_note_deletion(&id_for_accept);
                        },
                        "Delete"
                    }
                }
            } else {
                div { class: "operon-note-deletion-resolved",
                    "Resolved."
                }
            }
        }
    }
}
