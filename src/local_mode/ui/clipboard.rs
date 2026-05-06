//! Local-Mode in-app clipboard for cut/copy/paste of projects and notes.
//!
//! The clipboard never leaves Operon — it's an app-scope `Signal` carrying a
//! payload + intent (`Cut` or `Copy`). The explorer panel and row components
//! observe it to render a "ghost" effect on the cut source row and to
//! enable/disable the Paste menu item.

use dioxus::prelude::*;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipKind {
    Cut,
    Copy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipPayload {
    Project(Uuid),
    Note(Uuid),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clipboard {
    pub kind: ClipKind,
    pub payload: ClipPayload,
}

impl Clipboard {
    pub fn cut_note(id: Uuid) -> Self {
        Self {
            kind: ClipKind::Cut,
            payload: ClipPayload::Note(id),
        }
    }

    pub fn copy_note(id: Uuid) -> Self {
        Self {
            kind: ClipKind::Copy,
            payload: ClipPayload::Note(id),
        }
    }

    pub fn cut_project(id: Uuid) -> Self {
        Self {
            kind: ClipKind::Cut,
            payload: ClipPayload::Project(id),
        }
    }

    pub fn copy_project(id: Uuid) -> Self {
        Self {
            kind: ClipKind::Copy,
            payload: ClipPayload::Project(id),
        }
    }

    /// True when this clipboard would let a Paste action affect the given note id.
    pub fn is_cut_note(&self, id: Uuid) -> bool {
        self.kind == ClipKind::Cut && self.payload == ClipPayload::Note(id)
    }

    pub fn is_cut_project(&self, id: Uuid) -> bool {
        self.kind == ClipKind::Cut && self.payload == ClipPayload::Project(id)
    }
}

/// App-scope signal: the current clipboard, if any.
#[derive(Clone, Copy)]
pub struct LocalClipboard(pub Signal<Option<Clipboard>>);

/// Plans-Phase-4-multiselect-aria: bulk clipboard. Populated when the user
/// runs Cut/Copy with the multi-selection set holding 2+ items. Coexists
/// with [`LocalClipboard`]: the keyboard handler clears the single-item
/// clipboard when it writes a multi clipboard, and Paste prefers the multi
/// when present.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BulkClipboard {
    pub kind: ClipKind,
    pub items: Vec<ClipPayload>,
}

#[derive(Clone, Copy)]
pub struct LocalBulkClipboard(pub Signal<Option<BulkClipboard>>);
