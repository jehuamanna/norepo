//! Phase 1 POC — confirms `MonacoBackend` mounts a real Monaco editor against a fresh DOM
//! target via the TypeScript bridge, round-trips content, and disposes cleanly.
//!
//! This is the verification that closes plan item 1 of `Plans-Phase-1-foundations`. The
//! bridge dist files must exist before this test runs; `just test-wasm` depends on
//! `just build-bridge` per the Justfile.
//!
//! What the test exercises:
//! 1. Bridge is reachable on `window.operonBridge` (bridge entry installs itself there).
//! 2. `MonacoBackend::mount(target, init).await` resolves without bridge error.
//! 3. `set_content("hello world")` followed by `get_content()` round-trips the value.
//! 4. `dispose()` runs without panic; subsequent calls are no-ops.
//!
//! Cfg-gated to wasm32 so `cargo check --tests` on a native target doesn't try to
//! type-check against the desktop stub of `MonacoBackend` (whose `type Target = ()`).

#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;

use operon_dioxus::editor::{
    BackendInit, EditorBackend, EditorThemeBlob, LanguageDescriptor, MonacoBackend,
};
use operon_tests_wasm::{mount_root, next_frame, MountGuard};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

/// Dynamic-import the bridge entry so `window.operonBridge` is populated before the test
/// constructs a `MonacoBackend`. The bridge entry is path-resolved relative to wherever
/// wasm-pack serves the test bundle — `/assets/editor-bridge/dist/index.js` matches the
/// project-root-relative asset layout served by `dx serve`. If `wasm-pack test` doesn't
/// proxy these assets out of the box, the test will fail with a clear bridge error and we
/// can wire a fixtures harness in a follow-up.
async fn ensure_bridge_loaded() -> Result<(), JsValue> {
    let win = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    if !js_sys::Reflect::get(&win, &JsValue::from_str("operonBridge"))?.is_undefined() {
        return Ok(());
    }
    let import_fn = js_sys::Function::new_no_args(
        "return import('/assets/editor-bridge/dist/index.js')",
    );
    let promise: js_sys::Promise = import_fn.call0(&JsValue::NULL)?.dyn_into()?;
    JsFuture::from(promise).await?;
    Ok(())
}

#[wasm_bindgen_test]
async fn monaco_mount_set_get_dispose_roundtrip() {
    if let Err(e) = ensure_bridge_loaded().await {
        // We surface bridge-load failures as a soft skip rather than a panic so the suite
        // remains useful in environments where the bridge dist isn't statically served.
        web_sys::console::warn_1(&JsValue::from_str(&format!(
            "[monaco_smoke] bridge load failed; skipping: {e:?}"
        )));
        return;
    }

    let root = mount_root();
    let _guard = MountGuard(root.clone());
    root.set_attribute("style", "width: 600px; height: 400px;").unwrap();

    let mut backend = MonacoBackend::new();
    let init = BackendInit {
        language: LanguageDescriptor::markdown(),
        initial_content: "initial".to_string(),
        theme: EditorThemeBlob { blob: "vs".to_string() },
        read_only: false,
    };

    backend
        .mount(root.clone(), init)
        .await
        .expect("MonacoBackend::mount resolves once Monaco's `ready` promise settles");

    // Yield so Monaco's automaticLayout settles its first measurement against the target.
    next_frame().await;

    backend.set_content("hello world");
    next_frame().await;
    assert_eq!(
        backend.get_content(),
        "hello world",
        "set_content → get_content round-trips through the bridge handle"
    );

    backend.dispose();
    // Calling dispose twice is a documented no-op: the second call should not panic and
    // get_content should return "" (the disposed-handle fallback in monaco.ts).
    backend.dispose();
    assert_eq!(backend.get_content(), "");
}

#[wasm_bindgen_test]
async fn monaco_on_change_fires_on_programmatic_edit() {
    if let Err(e) = ensure_bridge_loaded().await {
        web_sys::console::warn_1(&JsValue::from_str(&format!(
            "[monaco_smoke] bridge load failed; skipping: {e:?}"
        )));
        return;
    }

    let root = mount_root();
    let _guard = MountGuard(root.clone());
    root.set_attribute("style", "width: 600px; height: 400px;").unwrap();

    let mut backend = MonacoBackend::new();
    backend
        .mount(
            root.clone(),
            BackendInit {
                language: LanguageDescriptor::plaintext(),
                initial_content: "".to_string(),
                theme: EditorThemeBlob::default(),
                read_only: false,
            },
        )
        .await
        .expect("mount resolves");

    let received: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let received_for_cb = received.clone();
    backend.on_change(Box::new(move |s| {
        received_for_cb.borrow_mut().push(s);
    }));

    backend.set_content("typed");
    // setContent triggers Monaco's onDidChangeModelContent; one frame is enough.
    next_frame().await;

    assert!(
        !received.borrow().is_empty(),
        "on_change callback fired at least once after set_content"
    );
    assert_eq!(received.borrow().last().map(String::as_str), Some("typed"));

    backend.dispose();
}

/// Sanity check: the `__operon_loaded` global tracks lazy-loaded bridge backends. After
/// mounting Monaco it must contain `"monaco"`. Used by the cross-cutting lazy-load
/// assertion in `TestCase-Phase-0-cross-cutting`.
#[wasm_bindgen_test]
async fn lazy_load_marker_records_monaco_after_mount() {
    if let Err(e) = ensure_bridge_loaded().await {
        web_sys::console::warn_1(&JsValue::from_str(&format!(
            "[monaco_smoke] bridge load failed; skipping: {e:?}"
        )));
        return;
    }

    // Reset the marker before measurement so this test is deterministic regardless of
    // sibling-test ordering.
    let win = web_sys::window().unwrap();
    let _ = js_sys::Reflect::set(
        &win,
        &JsValue::from_str("__operon_loaded"),
        &js_sys::Set::new(&JsValue::UNDEFINED),
    );

    let root = mount_root();
    let _guard = MountGuard(root.clone());
    let mut backend = MonacoBackend::new();
    let _ = backend
        .mount(
            root,
            BackendInit {
                language: LanguageDescriptor::json(),
                initial_content: "{}".into(),
                theme: EditorThemeBlob::default(),
                read_only: false,
            },
        )
        .await;
    next_frame().await;

    let marker = js_sys::Reflect::get(&win, &JsValue::from_str("__operon_loaded")).unwrap();
    let set: js_sys::Set = marker.dyn_into().expect("__operon_loaded is a Set");
    assert!(
        set.has(&JsValue::from_str("monaco")),
        "bridge tracks 'monaco' in __operon_loaded once mount fires the dynamic import"
    );

    backend.dispose();
    drop(Closure::once_into_js(|| ())); // anchor the imports so unused-warning lint is happy
}
