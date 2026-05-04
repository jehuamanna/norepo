//! Tests for the DOM helper layer in `operon_tests_wasm`.
//!
//! These exercise raw DOM construction (no Dioxus) so they isolate the
//! helpers from the Dioxus 0.7 mount-API uncertainty. When `mount_component`
//! is added later, additional tests will compose it with these helpers.

use operon_tests_wasm::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn mount_root_attaches_a_div_under_body() {
    let root = mount_root();
    let _g = MountGuard(root.clone());
    assert!(root.is_connected());
    assert_eq!(root.tag_name().to_lowercase(), "div");
}

#[wasm_bindgen_test]
fn cleanup_via_mount_guard_removes_root_from_dom() {
    let root = mount_root();
    {
        let _g = MountGuard(root.clone());
        assert!(root.is_connected());
    }
    assert!(!root.is_connected(), "MountGuard drop must remove the root node");
}

#[wasm_bindgen_test]
fn query_selector_finds_descendant_button() {
    let root = mount_root();
    let _g = MountGuard(root.clone());
    root.set_inner_html(r#"<button data-testid="ok">OK</button>"#);

    let btn = query_selector_or_panic(&root, "[data-testid='ok']");
    assert_eq!(btn.text_content().unwrap_or_default(), "OK");
}

#[wasm_bindgen_test]
fn query_selector_all_returns_every_match_in_document_order() {
    let root = mount_root();
    let _g = MountGuard(root.clone());
    root.set_inner_html("<span>a</span><span>b</span><span>c</span>");

    let spans = query_selector_all(&root, "span");
    let texts: Vec<_> = spans
        .iter()
        .map(|el| el.text_content().unwrap_or_default())
        .collect();
    assert_eq!(texts, vec!["a", "b", "c"]);
}

#[wasm_bindgen_test]
fn click_dispatches_a_click_event_to_handler() {
    let root = mount_root();
    let _g = MountGuard(root.clone());
    root.set_inner_html(r#"<button id="b">click me</button>"#);
    let btn = query_selector_or_panic(&root, "#b");

    let counter = std::rc::Rc::new(std::cell::Cell::new(0u32));
    let counter_for_handler = counter.clone();
    let handler = wasm_bindgen::closure::Closure::<dyn FnMut(_)>::new(
        move |_evt: web_sys::Event| {
            counter_for_handler.set(counter_for_handler.get() + 1);
        },
    );
    btn.add_event_listener_with_callback("click", handler.as_ref().unchecked_ref())
        .expect("attach listener");

    click(&btn);
    click(&btn);
    drop(handler);

    assert_eq!(counter.get(), 2);
}

#[wasm_bindgen_test]
async fn next_frame_yields_to_event_loop() {
    // Trivial liveness: next_frame() resolves; the test simply must not hang.
    next_frame().await;
    next_frame().await;
}
