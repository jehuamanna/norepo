//! Plans-Phase-4-explorer-undo-stack: in-memory undo history for explorer
//! tree mutations. Each user action records an inverse `ExplorerAction`
//! before its repo call commits; Cmd/Ctrl+Z while the explorer panel has
//! focus pops and applies the inverse.
//!
//! This first cut wraps the structural ops that always have a small
//! reversible inverse: rename, indent, outdent, move-up, move-down. Delete
//! and paste are deliberately deferred — they need full subtree snapshots
//! and a `restore_subtree` repo method that doesn't yet exist.
//!
//! Capacity is bounded (`cap`); pushing past capacity drops the oldest
//! entry. Failed undo (e.g. the target's parent has since been deleted)
//! logs but does not panic; the entry is still consumed.

use std::collections::VecDeque;

use operon_store::repos::SubtreeSnapshot;
use uuid::Uuid;

use crate::plugins::cleanup::trash::TrashRecord;

/// Inverse of a single user action. The variant carries the *previous*
/// state, not the action's parameters — so undo is "restore the captured
/// snapshot" rather than "compute the inverse of an op".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExplorerAction {
    Rename {
        id: Uuid,
        prev_title: String,
        /// Plans-Phase-8: needed by undo to run wikilink rewrite in the
        /// reverse direction (`new_title` → `prev_title`) across every
        /// referrer body. None when no project context was available
        /// (rare — usually means the note was orphaned mid-rename).
        project_name: Option<String>,
        /// The title we renamed *to*. Together with `prev_title` this is
        /// the substitution pair the undo path applies to every referrer.
        new_title: String,
    },
    /// Indent / outdent / move-up / move-down all collapse to the same
    /// shape: restore (parent, sibling_index) for `id` in `project_id`.
    /// Plans-Phase-11 design note: capturing the post-mutation position
    /// to support redo of MoveWithin would require either an extra
    /// repo read after each move or duplicating the repo's index-shift
    /// logic. Both are higher-cost than the redo affordance is worth in
    /// v1. MoveWithin is therefore **not redoable** — undoing one
    /// drops it from the redo deque rather than re-pushing.
    MoveWithin {
        id: Uuid,
        project_id: Uuid,
        prev_parent: Option<Uuid>,
        prev_index: i64,
    },
    /// Plans-Phase-8: full subtree captured before delete; undo re-inserts.
    /// `trash` carries the on-disk side-effects the delete moved aside
    /// (artifact dirs, materialized skills, orphaned blobs) so undo can
    /// restore them in lockstep with the SQLite rows.
    Delete {
        snapshot: SubtreeSnapshot,
        trash: TrashRecord,
    },
    /// Plans-Phase-8: paste of a copied subtree. Undo deletes the pasted
    /// subtree by id (cascade kills its descendants automatically).
    Paste {
        pasted_root_id: Uuid,
    },
    /// Plans-Phase-10: a freshly created note. Undo deletes the row;
    /// `blob_path` (when present, e.g. for image-picker creates) is the
    /// vault-relative path so the on-disk blob can be GC'd in lockstep
    /// with the row.
    Create {
        id: Uuid,
        blob_path: Option<String>,
    },
}

/// Bounded paired ring buffers of explorer mutations.
///
/// Plans-Phase-11: tracks both an undo deque and a redo deque, each
/// independently bounded to `cap`. `push` adds a fresh user gesture to
/// the undo deque and clears the redo deque (canonical text-editor
/// invariant: any new gesture invalidates the redo path).
///
/// `pop()` returns the latest undo entry; the caller (`on_undo`) is
/// responsible for re-pushing it onto `redo` after applying the inverse.
/// `pop_redo()` does the symmetric thing on the redo side.
#[derive(Debug)]
pub struct ExplorerHistory {
    undo: VecDeque<ExplorerAction>,
    redo: VecDeque<ExplorerAction>,
    cap: usize,
}

impl ExplorerHistory {
    pub fn new(cap: usize) -> Self {
        Self {
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            cap,
        }
    }

    /// Record a fresh user gesture. Drops the oldest undo entry when the
    /// cap is hit. Always clears the redo deque — any new gesture makes
    /// the redo path stale.
    pub fn push(&mut self, action: ExplorerAction) {
        if self.undo.len() == self.cap {
            self.undo.pop_front();
        }
        self.undo.push_back(action);
        self.redo.clear();
    }

    /// Pop the latest undo entry. Caller applies its inverse and (if
    /// the variant supports redo) re-pushes via `push_redo`.
    pub fn pop(&mut self) -> Option<ExplorerAction> {
        self.undo.pop_back()
    }

    /// Pop the latest redo entry. Caller applies its forward direction
    /// and re-pushes to undo via `push_undo`.
    pub fn pop_redo(&mut self) -> Option<ExplorerAction> {
        self.redo.pop_back()
    }

    /// Insert into the redo deque without clearing it. Used by `on_undo`
    /// after applying an inverse — never by user-initiated gestures.
    pub fn push_redo(&mut self, action: ExplorerAction) {
        if self.redo.len() == self.cap {
            self.redo.pop_front();
        }
        self.redo.push_back(action);
    }

    /// Insert into the undo deque without clearing redo. Used by
    /// `on_redo` after applying a forward direction — never by
    /// user-initiated gestures.
    pub fn push_undo(&mut self, action: ExplorerAction) {
        if self.undo.len() == self.cap {
            self.undo.pop_front();
        }
        self.undo.push_back(action);
    }

    pub fn is_empty(&self) -> bool {
        self.undo.is_empty()
    }

    pub fn redo_is_empty(&self) -> bool {
        self.redo.is_empty()
    }

    pub fn len(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }
}

impl Default for ExplorerHistory {
    fn default() -> Self {
        Self::new(100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rename(id: u128, title: &str) -> ExplorerAction {
        ExplorerAction::Rename {
            id: Uuid::from_u128(id),
            prev_title: title.into(),
            project_name: None,
            new_title: String::new(),
        }
    }

    #[test]
    fn push_pop_lifo_order() {
        // U-1 (TestCase-Phase-4) baseline — newest entry comes back first.
        let mut h = ExplorerHistory::new(100);
        h.push(rename(1, "a"));
        h.push(rename(2, "b"));
        assert_eq!(h.pop(), Some(rename(2, "b")));
        assert_eq!(h.pop(), Some(rename(1, "a")));
        assert!(h.is_empty());
    }

    #[test]
    fn capacity_drops_oldest() {
        // U-1 — pushing past `cap` drops the oldest entry.
        let mut h = ExplorerHistory::new(2);
        h.push(rename(1, "a"));
        h.push(rename(2, "b"));
        h.push(rename(3, "c"));
        assert_eq!(h.len(), 2);
        // Newest two survive; "a" should be gone.
        let last = h.pop();
        let mid = h.pop();
        assert_eq!(last, Some(rename(3, "c")));
        assert_eq!(mid, Some(rename(2, "b")));
        assert!(h.is_empty());
    }

    #[test]
    fn pop_empty_returns_none() {
        let mut h = ExplorerHistory::new(10);
        assert_eq!(h.pop(), None);
        assert_eq!(h.pop_redo(), None);
    }

    // ===== Plans-Phase-11: redo deque mechanics =====

    #[test]
    fn fresh_push_clears_redo() {
        let mut h = ExplorerHistory::new(10);
        // Simulate "undo applied" by directly pushing onto redo.
        h.push_redo(rename(1, "a"));
        assert_eq!(h.redo_len(), 1);
        // A fresh user gesture (push) must clear the redo deque.
        h.push(rename(2, "b"));
        assert!(h.redo_is_empty());
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn push_redo_then_pop_redo_round_trip() {
        let mut h = ExplorerHistory::new(10);
        h.push_redo(rename(1, "a"));
        h.push_redo(rename(2, "b"));
        // LIFO on the redo side — newest comes back first.
        assert_eq!(h.pop_redo(), Some(rename(2, "b")));
        assert_eq!(h.pop_redo(), Some(rename(1, "a")));
        assert!(h.redo_is_empty());
    }

    #[test]
    fn push_undo_does_not_clear_redo() {
        // push_undo (used by on_redo after applying forward) should NOT
        // clear the redo deque — only fresh user gestures (push) do.
        let mut h = ExplorerHistory::new(10);
        h.push_redo(rename(1, "a"));
        h.push_undo(rename(2, "b"));
        assert_eq!(h.redo_len(), 1);
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn redo_capacity_drops_oldest() {
        let mut h = ExplorerHistory::new(2);
        h.push_redo(rename(1, "a"));
        h.push_redo(rename(2, "b"));
        h.push_redo(rename(3, "c"));
        assert_eq!(h.redo_len(), 2);
        assert_eq!(h.pop_redo(), Some(rename(3, "c")));
        assert_eq!(h.pop_redo(), Some(rename(2, "b")));
        assert!(h.redo_is_empty());
    }
}
