//! Plans-Phase-13 (TestCase-Phase-4 / Phase-8 / Phase-11 unit specs):
//! end-to-end undo + redo round-trips for the explorer history stack.
//!
//! Status: scaffolded. The deque mechanics (push, pop, push_redo,
//! pop_redo, fresh-push-clears-redo, capacity) are tested *inline* in
//! `src/local_mode/explorer/history.rs`'s `#[cfg(test)] mod tests`
//! because they're pure-Rust and don't need the wasm harness. What
//! remains for this file are the *integration* round-trips that go
//! through the real explorer panel + repo: simulate a user gesture,
//! apply on_undo via the keybinding, observe the DB, then on_redo, etc.
//!
//! Those need a Dioxus component-mount helper that `tests-wasm/src/lib.rs`
//! deliberately does not yet expose (see the head comment on that
//! file). When that helper lands the bodies below get filled in;
//! today they're `#[ignore]`d so the file compiles and the names show
//! up in `cargo test --list`.
//!
//! When the helper lands:
//! 1. Remove `#[ignore]`.
//! 2. Replace `unimplemented!()` with: spawn a `VirtualDom`, mount the
//!    panel, dispatch synthetic events for the gesture, await
//!    `next_frame()`, simulate `Cmd+Z`, assert the DB state via the
//!    repo handle the panel exposes.
//!
//! Authored under seed `be14fe84` (Bugs in Notes), Round 3.

use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
#[ignore]
fn rename_undo_redo_round_trip() {
    // Plan: create a note titled "Original"; rename to "New"; press Cmd+Z;
    // assert title = "Original"; press Cmd+Shift+Z; assert title = "New".
    // Also verify any wikilink referrers are rewritten in both directions.
    unimplemented!("requires Dioxus component-mount helper");
}

#[wasm_bindgen_test]
#[ignore]
fn delete_undo_redo_round_trip() {
    // Plan: create a 3-level subtree; delete the root; press Cmd+Z;
    // assert subtree restored with identical structure (parent_id /
    // sibling_index / title / kind / blob_path); press Cmd+Shift+Z;
    // assert subtree gone.
    unimplemented!("requires Dioxus component-mount helper");
}

#[wasm_bindgen_test]
#[ignore]
fn create_undo_no_redo() {
    // Plan: Add note (markdown); type a title; press Cmd+Z; assert
    // row gone. Press Cmd+Shift+Z; assert row STILL gone (Create is
    // not redoable in v1).
    unimplemented!("requires Dioxus component-mount helper");
}

#[wasm_bindgen_test]
#[ignore]
fn paste_undo_no_redo() {
    // Plan: Copy A; Paste under B; press Cmd+Z; assert pasted subtree
    // gone. Press Cmd+Shift+Z; assert pasted subtree STILL gone (Paste
    // is not redoable in v1 — the forward direction needs the original
    // clipboard payload).
    unimplemented!("requires Dioxus component-mount helper");
}

#[wasm_bindgen_test]
#[ignore]
fn move_within_undo_no_redo() {
    // Plan: Indent note B (under prior sibling A); press Cmd+Z; assert
    // B back at its original position. Press Cmd+Shift+Z; assert B
    // STILL at the post-undo position (MoveWithin not redoable in v1).
    unimplemented!("requires Dioxus component-mount helper");
}

#[wasm_bindgen_test]
#[ignore]
fn fresh_gesture_clears_redo() {
    // Plan: Rename A; press Cmd+Z (redo deque now has 1); rename B;
    // assert redo deque has 0 (push cleared it); press Cmd+Shift+Z;
    // assert nothing happens.
    unimplemented!("requires Dioxus component-mount helper");
}

#[wasm_bindgen_test]
#[ignore]
fn cmd_z_in_editor_does_not_pop_explorer_stack() {
    // Plan: Rename A (explorer stack now has 1 entry); click A to focus
    // Monaco; type some text; press Cmd+Z; assert editor content
    // reverted (Monaco intrinsic) but A's title remains the new value.
    unimplemented!("requires Dioxus component-mount helper");
}

#[wasm_bindgen_test]
#[ignore]
fn undo_keybinding_skips_while_renaming() {
    // Plan: Add note → rename input opens; type a partial title; press
    // Cmd+Z; assert rename input still focused with partial text-undo
    // applied (NOT the explorer's row deletion).
    unimplemented!("requires Dioxus component-mount helper");
}

#[wasm_bindgen_test]
#[ignore]
fn failed_undo_emits_toast() {
    // Plan: Push a synthetic Move action whose target parent has been
    // deleted; press Cmd+Z; assert the entry is consumed AND a toast
    // with kind=Error appears.
    unimplemented!("requires Dioxus component-mount helper");
}
