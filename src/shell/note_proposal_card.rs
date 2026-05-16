//! UI for `OperonReplaceNoteRangeTool` proposals (M4c.7).
//!
//! Rendered alongside `AskUserPromptCard` in the companion chat
//! surface when `NOTE_PROPOSALS` is non-empty. Shows the note
//! title, the unified-diff preview, and Accept / Reject buttons.
//! Buttons resolve the parked one-shot responder via
//! `accept_note_proposal` / `reject_note_proposal`, which wakes the
//! blocked tool call and either persists the staged body or returns
//! a "user rejected" error to Claude.
//!
//! Minimal styling on purpose — the diff is the content; we get out
//! of its way. Future iterations can split the `<pre>` into red/green
//! line spans for prettier rendering.

#![cfg(not(target_arch = "wasm32"))]

use dioxus::prelude::*;

use crate::shell::companion_state::{
    accept_note_proposal, reject_note_proposal, NoteProposalEntry, NoteProposalStatus,
};

#[derive(Props, Clone, PartialEq)]
pub struct NoteProposalCardProps {
    pub entry: NoteProposalEntry,
    pub status: NoteProposalStatus,
}

#[component]
pub fn NoteProposalCard(props: NoteProposalCardProps) -> Element {
    let entry = props.entry;
    let status = props.status;
    let pending = matches!(status, NoteProposalStatus::Pending);

    // Stable copies for each handler (Dioxus moves them into the
    // closure, so we clone per button).
    let id_for_accept = entry.id.clone();
    let id_for_reject = entry.id.clone();

    let badge = match status {
        NoteProposalStatus::Pending => ("pending", "Proposed edit"),
        NoteProposalStatus::Accepted => ("accepted", "Accepted"),
        NoteProposalStatus::Rejected => ("rejected", "Rejected"),
    };

    // Stat line: how many lines changed, roughly. We could parse
    // the diff for exact counts, but the relative size (new vs old)
    // is the cheaper, equally-useful signal.
    let old_lines = entry.old_body.lines().count();
    let new_lines = entry.new_body.lines().count();

    rsx! {
        div {
            class: "operon-note-proposal",
            "data-testid": "note-proposal-card",
            "data-status": badge.0,
            "data-proposal-id": "{entry.id}",
            header {
                class: "operon-note-proposal-header",
                span { class: "operon-note-proposal-badge", "{badge.1}" }
                span { class: "operon-note-proposal-title",
                    "{entry.note_title}"
                }
                span { class: "operon-note-proposal-stats",
                    "{old_lines} → {new_lines} lines"
                }
            }
            pre {
                class: "operon-note-proposal-diff",
                "{entry.diff_preview}"
            }
            if pending {
                div {
                    class: "operon-note-proposal-actions",
                    button {
                        class: "operon-note-proposal-reject",
                        r#type: "button",
                        "data-testid": "note-proposal-reject",
                        onclick: move |_| {
                            reject_note_proposal(&id_for_reject);
                        },
                        "Reject"
                    }
                    button {
                        class: "operon-note-proposal-accept",
                        r#type: "button",
                        "data-testid": "note-proposal-accept",
                        onclick: move |_| {
                            accept_note_proposal(&id_for_accept);
                        },
                        "Accept"
                    }
                }
            } else {
                div { class: "operon-note-proposal-resolved",
                    "Resolved."
                }
            }
        }
    }
}
