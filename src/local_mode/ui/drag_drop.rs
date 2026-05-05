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

use dioxus::prelude::*;
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
