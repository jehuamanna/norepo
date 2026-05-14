//! Drag-and-drop primitives for the Local-Mode explorer.
//!
//! Dioxus 0.7 exposes the standard HTML drag events (`ondragstart`,
//! `ondragover`, `ondragleave`, `ondrop`, `ondragend`). The native browser
//! `dataTransfer` machinery is not the source of truth here — we keep the
//! payload in a single `Signal<DragSession>` provided at app scope and look it
//! up on drop. That keeps the abstraction Rust-typed and avoids round-tripping
//! UUIDs through string clipboards.
//!
//! `DropPosition` is computed from the cursor's offset Y vs the bounding rect
//! of the row being hovered — top 30% means insert before, bottom 30% after,
//! middle 40% means drop into. Each row is responsible for snapping its own
//! cursor math; this module just owns the type + signal vocabulary.

use std::collections::BTreeSet;

use dioxus::prelude::*;
use operon_store::repos::LocalNote;
use uuid::Uuid;

/// What the user is currently dragging. The signal is `None` outside of an
/// active drag.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DragKind {
    Project(Uuid),
    Note(Uuid),
}

/// Where the user wants to drop, relative to the hovered target row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DropPosition {
    Before,
    Into,
    After,
}

/// App-scope signal that tracks the currently dragged item. The explorer panel
/// and row components both read/write this — `ondragstart` sets it,
/// `ondragend` and `ondrop` clear it.
#[derive(Clone, Copy)]
pub struct DragSession(pub Signal<Option<DragKind>>);

/// Plans-Phase-3-explorer-drag-drop-feedback: while a note drag is active,
/// the source's full descendant set (excluding the source itself) lives
/// here so individual rows can answer "would dropping me on this target
/// create a cycle?" without re-traversing the tree on every `ondragover`.
/// `ondragstart` populates it; `ondragend`/`ondrop` clear it.
#[derive(Clone, Copy)]
pub struct DragDescendants(pub Signal<BTreeSet<Uuid>>);

/// Compute where, relative to a row's bounding rect of height `h`, a cursor
/// at offset y should drop. Top 30% → Before, bottom 30% → After, middle 40%
/// → Into. Falls back to `Into` for degenerate (zero-height) rows.
pub fn classify_drop_position(offset_y: f64, height: f64) -> DropPosition {
    if height <= 0.0 {
        return DropPosition::Into;
    }
    let frac = (offset_y / height).clamp(0.0, 1.0);
    if frac < 0.30 {
        DropPosition::Before
    } else if frac > 0.70 {
        DropPosition::After
    } else {
        DropPosition::Into
    }
}

/// Map a cursor's X offset (relative to the row's left edge) to a depth in
/// the explorer tree. `indent_px` is the per-level indent width — keep it in
/// sync with the `--depth` multiplier in `assets/shell.css` (currently 12px).
/// Result is clamped to `[min, max]`.
pub fn classify_target_depth(offset_x: f64, indent_px: f64, min: i64, max: i64) -> i64 {
    if indent_px <= 0.0 {
        return min.max(0).min(max);
    }
    let raw = (offset_x / indent_px).floor() as i64;
    raw.clamp(min, max)
}

/// Resolve a drop into a concrete `(new_parent_id, new_sibling_index)` pair
/// for `LocalNoteRepo::move_to`.
///
/// Rules:
/// - **Into** — child of `target`, appended after target's existing children.
///   `chosen_depth` is ignored.
/// - **Before** — sibling of `target` at `target.sibling_index`.
///   `chosen_depth` is ignored (Before is unambiguous).
/// - **After** — depth-aware:
///   - `chosen_depth >= target.depth + 1` and target has at least one child
///     → first child of target.
///   - `chosen_depth == target.depth` → sibling of target at
///     `target.sibling_index + 1`.
///   - `chosen_depth < target.depth` → walk target's ancestor chain by
///     `target.depth - chosen_depth` steps to ancestor `A`; result is
///     `(A.parent_id, A.sibling_index + 1)` (insert as A's next sibling).
///
/// Caller is still responsible for cycle/self-drop checks. Returns
/// `(parent_id, sibling_index)`. Walks at most `target.depth` rows of
/// `notes` so the cost is O(depth × project_size) — acceptable since the
/// caller already snapshots the full project list.
pub fn resolve_drop_parent(
    target: &LocalNote,
    pos: DropPosition,
    chosen_depth: i64,
    notes: &[LocalNote],
) -> (Option<Uuid>, i64) {
    match pos {
        DropPosition::Into => {
            let child_count = notes.iter().filter(|n| n.parent_id == Some(target.id)).count() as i64;
            (Some(target.id), child_count)
        }
        DropPosition::Before => (target.parent_id, target.sibling_index),
        DropPosition::After => {
            let target_has_children = notes.iter().any(|n| n.parent_id == Some(target.id));
            let max_depth = target.depth + if target_has_children { 1 } else { 0 };
            let depth = chosen_depth.clamp(0, max_depth);
            if depth > target.depth && target_has_children {
                // Indent into target as its first child.
                return (Some(target.id), 0);
            }
            if depth == target.depth {
                return (target.parent_id, target.sibling_index + 1);
            }
            // Outdent: walk up the ancestor chain by (target.depth - depth)
            // steps to reach the ancestor whose own depth equals `depth`.
            let mut steps = target.depth - depth;
            let mut current = target.clone();
            while steps > 0 {
                let parent_id = match current.parent_id {
                    Some(id) => id,
                    None => break,
                };
                match notes.iter().find(|n| n.id == parent_id) {
                    Some(p) => current = p.clone(),
                    None => break,
                }
                steps -= 1;
            }
            (current.parent_id, current.sibling_index + 1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use operon_store::repos::NoteKind;

    #[test]
    fn classify_drop_position_top_third_is_before() {
        assert_eq!(classify_drop_position(0.0, 100.0), DropPosition::Before);
        assert_eq!(classify_drop_position(20.0, 100.0), DropPosition::Before);
    }

    #[test]
    fn classify_drop_position_middle_is_into() {
        assert_eq!(classify_drop_position(40.0, 100.0), DropPosition::Into);
        assert_eq!(classify_drop_position(50.0, 100.0), DropPosition::Into);
        assert_eq!(classify_drop_position(70.0, 100.0), DropPosition::Into);
    }

    #[test]
    fn classify_drop_position_bottom_third_is_after() {
        assert_eq!(classify_drop_position(80.0, 100.0), DropPosition::After);
        assert_eq!(classify_drop_position(99.9, 100.0), DropPosition::After);
    }

    #[test]
    fn classify_drop_position_zero_height_is_into() {
        assert_eq!(classify_drop_position(0.0, 0.0), DropPosition::Into);
    }

    #[test]
    fn classify_target_depth_floors_and_clamps() {
        assert_eq!(classify_target_depth(0.0, 12.0, 0, 5), 0);
        assert_eq!(classify_target_depth(11.9, 12.0, 0, 5), 0);
        assert_eq!(classify_target_depth(12.0, 12.0, 0, 5), 1);
        assert_eq!(classify_target_depth(35.0, 12.0, 0, 5), 2);
        assert_eq!(classify_target_depth(-50.0, 12.0, 0, 5), 0);
        assert_eq!(classify_target_depth(9999.0, 12.0, 0, 5), 5);
        // Zero indent_px → degenerate; clamp to min.
        assert_eq!(classify_target_depth(100.0, 0.0, 0, 5), 0);
    }

    fn note(parent: Option<Uuid>, depth: i64, idx: i64) -> LocalNote {
        LocalNote {
            id: Uuid::new_v4(),
            project_id: Uuid::nil(),
            parent_id: parent,
            sibling_index: idx,
            depth,
            title: String::new(),
            created_at_ms: 0,
            updated_at_ms: 0,
            kind: NoteKind::Markdown,
            blob_path: None,
            slug: None,
        }
    }

    #[test]
    fn resolve_into_appends_as_last_child() {
        let target = note(None, 0, 0);
        let c1 = LocalNote { parent_id: Some(target.id), depth: 1, sibling_index: 0, ..note(None, 1, 0) };
        let c2 = LocalNote { parent_id: Some(target.id), depth: 1, sibling_index: 1, ..note(None, 1, 0) };
        let notes = vec![target.clone(), c1, c2];
        let (parent, idx) = resolve_drop_parent(&target, DropPosition::Into, 99, &notes);
        assert_eq!(parent, Some(target.id));
        assert_eq!(idx, 2);
    }

    #[test]
    fn resolve_before_uses_targets_own_position() {
        let target = LocalNote { sibling_index: 3, depth: 2, ..note(None, 2, 3) };
        let parent = Uuid::new_v4();
        let target = LocalNote { parent_id: Some(parent), ..target };
        let (resolved_parent, idx) = resolve_drop_parent(&target, DropPosition::Before, 0, &[target.clone()]);
        assert_eq!(resolved_parent, Some(parent));
        assert_eq!(idx, 3);
    }

    #[test]
    fn resolve_after_same_depth_inserts_next_to_target() {
        let parent = Uuid::new_v4();
        let target = LocalNote { parent_id: Some(parent), depth: 2, sibling_index: 5, ..note(Some(parent), 2, 5) };
        let (p, idx) = resolve_drop_parent(&target, DropPosition::After, 2, &[target.clone()]);
        assert_eq!(p, Some(parent));
        assert_eq!(idx, 6);
    }

    #[test]
    fn resolve_after_indents_into_target_when_target_has_children() {
        let target = note(None, 0, 0);
        let child = LocalNote { parent_id: Some(target.id), depth: 1, sibling_index: 0, ..note(None, 1, 0) };
        let notes = vec![target.clone(), child];
        let (p, idx) = resolve_drop_parent(&target, DropPosition::After, 1, &notes);
        assert_eq!(p, Some(target.id));
        assert_eq!(idx, 0);
    }

    #[test]
    fn resolve_after_outdents_one_level() {
        // grand <- p <- t. Drop after t at depth=1 → become next sibling of p
        // (so new parent = grand, sibling_index = p.sibling_index + 1).
        let grand = note(None, 0, 0);
        let p = LocalNote { parent_id: Some(grand.id), depth: 1, sibling_index: 4, ..note(Some(grand.id), 1, 4) };
        let t = LocalNote { parent_id: Some(p.id), depth: 2, sibling_index: 7, ..note(Some(p.id), 2, 7) };
        let notes = vec![grand.clone(), p.clone(), t.clone()];
        let (parent, idx) = resolve_drop_parent(&t, DropPosition::After, 1, &notes);
        assert_eq!(parent, Some(grand.id));
        assert_eq!(idx, 5);
    }

    #[test]
    fn resolve_after_outdents_to_root() {
        // grand <- p <- t. Drop after t at depth=0 → become next sibling of grand
        // (so new parent = None, sibling_index = grand.sibling_index + 1).
        let grand = LocalNote { sibling_index: 2, ..note(None, 0, 2) };
        let p = LocalNote { parent_id: Some(grand.id), depth: 1, sibling_index: 0, ..note(Some(grand.id), 1, 0) };
        let t = LocalNote { parent_id: Some(p.id), depth: 2, sibling_index: 0, ..note(Some(p.id), 2, 0) };
        let notes = vec![grand.clone(), p.clone(), t.clone()];
        let (parent, idx) = resolve_drop_parent(&t, DropPosition::After, 0, &notes);
        assert_eq!(parent, None);
        assert_eq!(idx, 3);
    }

    #[test]
    fn resolve_after_clamps_negative_chosen_depth() {
        let grand = LocalNote { sibling_index: 2, ..note(None, 0, 2) };
        let t = LocalNote { parent_id: Some(grand.id), depth: 1, sibling_index: 0, ..note(Some(grand.id), 1, 0) };
        let notes = vec![grand.clone(), t.clone()];
        let (parent, idx) = resolve_drop_parent(&t, DropPosition::After, -5, &notes);
        // Clamped to 0 → outdent to root.
        assert_eq!(parent, None);
        assert_eq!(idx, 3);
    }

    #[test]
    fn resolve_after_ignores_indent_when_target_has_no_children() {
        let target = LocalNote { sibling_index: 0, ..note(None, 0, 0) };
        let (p, idx) = resolve_drop_parent(&target, DropPosition::After, 5, &[target.clone()]);
        // depth>target.depth but no children → max_depth=0, clamped to depth=0
        // → same-depth After.
        assert_eq!(p, None);
        assert_eq!(idx, 1);
    }
}
