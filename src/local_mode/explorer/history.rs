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

use uuid::Uuid;

/// Inverse of a single user action. The variant carries the *previous*
/// state, not the action's parameters — so undo is "restore the captured
/// snapshot" rather than "compute the inverse of an op".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExplorerAction {
    Rename {
        id: Uuid,
        prev_title: String,
    },
    /// Indent / outdent / move-up / move-down all collapse to the same
    /// shape: restore (parent, sibling_index) for `id` in `project_id`.
    MoveWithin {
        id: Uuid,
        project_id: Uuid,
        prev_parent: Option<Uuid>,
        prev_index: i64,
    },
}

/// Bounded ring buffer of undo entries. The newest entry is at the back;
/// `pop()` returns the most recent, oldest dropped on overflow.
#[derive(Debug)]
pub struct ExplorerHistory {
    stack: VecDeque<ExplorerAction>,
    cap: usize,
}

impl ExplorerHistory {
    pub fn new(cap: usize) -> Self {
        Self {
            stack: VecDeque::new(),
            cap,
        }
    }

    pub fn push(&mut self, action: ExplorerAction) {
        if self.stack.len() == self.cap {
            self.stack.pop_front();
        }
        self.stack.push_back(action);
    }

    pub fn pop(&mut self) -> Option<ExplorerAction> {
        self.stack.pop_back()
    }

    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    pub fn len(&self) -> usize {
        self.stack.len()
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
    }
}
