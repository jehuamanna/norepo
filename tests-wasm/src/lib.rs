//! Browser-DOM testing helpers for operon-dioxus.
//!
//! Used by `tests-wasm/tests/*.rs` files via `use operon_tests_wasm::*;`.
//!
//! What this module provides:
//! - `document()`, `body()` — quick handles to the running browser's DOM.
//! - `mount_root()` / `MountGuard` — create + auto-clean a mount target.
//! - `query_selector*` — typed wrappers around `Element::query_selector`.
//! - `click`, `type_into`, `press_key` — synthetic event dispatch.
//! - `next_frame().await` — yields to the browser event loop so reconciliation
//!   completes before the next assertion.
//!
//! Note: `mount_component` (rendering a Dioxus component into a target DOM
//! node) is deliberately NOT exposed here yet — Dioxus 0.7's component-mount
//! API is in active flux and authoring tests against an unstable surface
//! creates churn. The first real component-DOM spec will lock that helper down.
//!
//! Authored under the "Playwright for testing" Archon seed
//! (84185cbf-0b4f-4211-bb33-145a9817ac0c, Plans-Phase-3-integration-test-scaffolding).

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Document, Element, Event, HtmlElement, KeyboardEvent, MouseEvent, Window};

pub fn window() -> Window {
    web_sys::window().expect("window in browser")
}

pub fn document() -> Document {
    window().document().expect("document in browser")
}

pub fn body() -> HtmlElement {
    document().body().expect("body in browser")
}

pub fn mount_root() -> Element {
    let root = document()
        .create_element("div")
        .expect("can create div for mount root");
    body()
        .append_child(&root)
        .expect("can attach mount root to body");
    root
}

/// RAII guard that removes the mount target on drop. Use as
/// `let _g = MountGuard(root.clone());` in every wasm-bindgen-test.
pub struct MountGuard(pub Element);
impl Drop for MountGuard {
    fn drop(&mut self) {
        if let Some(parent) = self.0.parent_node() {
            let _ = parent.remove_child(&self.0);
        }
    }
}

pub fn query_selector(root: &Element, sel: &str) -> Option<Element> {
    root.query_selector(sel).ok().flatten()
}

pub fn query_selector_or_panic(root: &Element, sel: &str) -> Element {
    query_selector(root, sel).unwrap_or_else(|| panic!("selector {sel:?} not found under root"))
}

pub fn query_selector_all(root: &Element, sel: &str) -> Vec<Element> {
    let nodes = root
        .query_selector_all(sel)
        .expect("query_selector_all does not throw");
    let mut out = Vec::with_capacity(nodes.length() as usize);
    for i in 0..nodes.length() {
        if let Some(n) = nodes.get(i) {
            if let Ok(el) = n.dyn_into::<Element>() {
                out.push(el);
            }
        }
    }
    out
}

pub fn click(elem: &Element) {
    let evt = MouseEvent::new("click").expect("MouseEvent");
    elem.dispatch_event(&evt).expect("dispatch click");
}

pub fn type_into(input: &Element, text: &str) {
    let html: &HtmlElement = input.dyn_ref().expect("input is HtmlElement");
    if let Ok(typed) = html.clone().dyn_into::<web_sys::HtmlInputElement>() {
        typed.set_value(text);
    } else {
        html.set_inner_text(text);
    }
    let evt = Event::new("input").expect("Event");
    input.dispatch_event(&evt).expect("dispatch input");
}

pub fn press_key(target: &Element, key: &str) {
    let init = web_sys::KeyboardEventInit::new();
    init.set_key(key);
    init.set_bubbles(true);
    init.set_cancelable(true);
    let evt = KeyboardEvent::new_with_keyboard_event_init_dict("keydown", &init)
        .expect("KeyboardEvent");
    target.dispatch_event(&evt).expect("dispatch keydown");
}

/// Resolves on the next animation frame. Use to wait for Dioxus reconciliation
/// after dispatching an event.
pub async fn next_frame() {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        let cb = Closure::once_into_js(move || {
            let _ = resolve.call0(&JsValue::NULL);
        });
        let _ = window()
            .request_animation_frame(cb.as_ref().unchecked_ref())
            .expect("request_animation_frame");
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}
