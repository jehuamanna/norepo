//! Plans-Phase-6 (TestCase-Phase-1 U-4 — relocated): regression guard for
//! the select-on-mount behaviour `InlineRename` (`src/local_mode/ui/inline_rename.rs`)
//! relies on. We can't yet mount a real Dioxus component into the test
//! harness — the helper is deliberately not exposed (see
//! `tests-wasm/src/lib.rs` head comment). So this test exercises the
//! underlying browser primitive `HTMLInputElement::select()` directly:
//! an input with non-empty `value` whose `select()` is called must report
//! `selectionStart == 0` and `selectionEnd == value.len()`.
//!
//! If a future browser regresses this contract, our `InlineRename::onmounted`
//! select call would silently no-op and Bug 2 from seed `be14fe84` would
//! resurface.

use operon_tests_wasm::{document, MountGuard};
use wasm_bindgen::JsCast;
use wasm_bindgen_test::*;
use web_sys::HtmlInputElement;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn select_on_input_selects_full_value() {
    let doc = document();
    let input: HtmlInputElement = doc
        .create_element("input")
        .expect("create input")
        .dyn_into()
        .expect("input cast");
    input.set_type("text");
    input.set_value("Untitled");
    let body = doc.body().expect("body");
    body.append_child(&input).expect("attach input");
    let _g = MountGuard(input.clone().unchecked_into());

    input.focus().expect("focus the input");
    input.select();

    assert_eq!(input.selection_start().ok().flatten(), Some(0));
    assert_eq!(
        input.selection_end().ok().flatten(),
        Some(input.value().chars().count() as u32),
    );
}

#[wasm_bindgen_test]
fn select_on_empty_input_is_no_op_safe() {
    // Newly-created notes start with an empty placeholder. select() on an
    // empty value should still be safe — selection bounds collapse to 0.
    let doc = document();
    let input: HtmlInputElement = doc
        .create_element("input")
        .expect("create input")
        .dyn_into()
        .expect("input cast");
    input.set_type("text");
    input.set_value("");
    let body = doc.body().expect("body");
    body.append_child(&input).expect("attach input");
    let _g = MountGuard(input.clone().unchecked_into());

    input.focus().expect("focus the input");
    input.select();

    assert_eq!(input.selection_start().ok().flatten(), Some(0));
    assert_eq!(input.selection_end().ok().flatten(), Some(0));
}
