//! Toolchain smoke test: confirms `wasm-pack test --headless --chrome` boots
//! a browser, executes a wasm test, and exits cleanly. Phase 3 adds real DOM
//! tests on top of this scaffolding.

use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn arithmetic_smoke() {
    assert_eq!(1 + 1, 2);
}

#[wasm_bindgen_test]
fn document_is_reachable() {
    let win = web_sys::window().expect("window exists in browser");
    let doc = win.document().expect("document exists in browser");
    assert!(doc.body().is_some(), "body element exists in browser test");
}
