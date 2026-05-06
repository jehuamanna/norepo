//! `MonacoBackend` — wasm-side `EditorBackend` impl that delegates to the TypeScript bridge.
//!
//! Compiled only for `target_arch = "wasm32"`; the desktop / native build provides a stub so
//! the trait is namable everywhere but mount() returns `NotMounted` outside wasm.
//!
//! The `Handle` JsValue holds a reference to a Monaco editor instance owned by the JS bridge
//! (`assets/editor-bridge/dist/index.js`). All methods round-trip through `js_sys::Reflect` to
//! call the corresponding fn on that handle. Closures crossing the wasm-bindgen boundary
//! during `on_change` registration are tracked on the Rust side so `dispose` frees them
//! deterministically — see Plans-Phase-0 R2.

use super::{BackendInit, EditorBackend, EditorCommand, EditorError, EditorState, EditorThemeBlob};

#[cfg(target_arch = "wasm32")]
mod imp {
    use super::*;
    use std::cell::RefCell;
    use std::future::Future;
    use std::pin::Pin;
    use std::rc::Rc;

    use js_sys::{Function, Object, Promise, Reflect};
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;

    /// Handle to a Monaco editor instance owned by the JS bridge. Stored as `Option<JsValue>`
    /// because `mount` populates it asynchronously; methods called before `mount` return
    /// safely without panicking.
    pub struct MonacoBackend {
        handle: Rc<RefCell<Option<JsValue>>>,
        /// Tracks every `Closure` that has crossed the wasm-bindgen boundary so `dispose`
        /// can free them deterministically.
        closures: Rc<RefCell<Vec<Closure<dyn FnMut(JsValue)>>>>,
    }

    impl MonacoBackend {
        pub fn new() -> Self {
            Self {
                handle: Rc::new(RefCell::new(None)),
                closures: Rc::new(RefCell::new(Vec::new())),
            }
        }

        fn handle_fn(&self, name: &str) -> Option<Function> {
            let handle = self.handle.borrow();
            let h = handle.as_ref()?;
            Reflect::get(h, &JsValue::from_str(name))
                .ok()
                .and_then(|v| v.dyn_into::<Function>().ok())
        }

        fn invoke(&self, name: &str, args: &[JsValue]) -> Option<JsValue> {
            let f = self.handle_fn(name)?;
            let handle = self.handle.borrow();
            let this = handle.as_ref()?;
            let arr = js_sys::Array::new();
            for a in args {
                arr.push(a);
            }
            f.apply(this, &arr).ok()
        }

        fn build_init_object(init: &BackendInit) -> Object {
            let obj = Object::new();
            let _ = Reflect::set(&obj, &"kind".into(), &"monaco".into());
            let _ = Reflect::set(&obj, &"languageId".into(), &init.language.monaco_language.into());
            let _ = Reflect::set(&obj, &"content".into(), &init.initial_content.as_str().into());
            let _ = Reflect::set(&obj, &"theme".into(), &init.theme.blob.as_str().into());
            let _ = Reflect::set(&obj, &"readOnly".into(), &JsValue::from_bool(init.read_only));
            obj
        }

        /// Resolve the bridge's `mount` function from the `window.operonBridge` global. The
        /// bridge entry script attaches itself there on first import so wasm-bindgen can call
        /// it without a static module reference.
        fn bridge_mount() -> Result<Function, EditorError> {
            let window = web_sys::window().ok_or(EditorError::Bridge("no window".into()))?;
            let bridge = Reflect::get(&window, &JsValue::from_str("operonBridge"))
                .map_err(|_| EditorError::Bridge("operonBridge global missing".into()))?;
            if bridge.is_undefined() {
                return Err(EditorError::Bridge(
                    "operonBridge global missing — call (await import('/assets/editor-bridge/dist/index.js')) and set window.operonBridge".into(),
                ));
            }
            Reflect::get(&bridge, &JsValue::from_str("mount"))
                .ok()
                .and_then(|v| v.dyn_into::<Function>().ok())
                .ok_or(EditorError::Bridge("operonBridge.mount not callable".into()))
        }
    }

    impl Default for MonacoBackend {
        fn default() -> Self {
            Self::new()
        }
    }

    impl EditorBackend for MonacoBackend {
        type Target = web_sys::Element;

        fn mount<'a>(
            &'a mut self,
            target: web_sys::Element,
            init: BackendInit,
        ) -> Pin<Box<dyn Future<Output = Result<(), EditorError>> + 'a>> {
            Box::pin(async move {
                let mount_fn = Self::bridge_mount()?;
                let init_obj = Self::build_init_object(&init);
                let promise: Promise = mount_fn
                    .call2(&JsValue::NULL, &target, &init_obj)
                    .map_err(|e| EditorError::Bridge(format!("{e:?}")))?
                    .dyn_into()
                    .map_err(|_| EditorError::Bridge("mount didn't return a Promise".into()))?;
                let handle = JsFuture::from(promise)
                    .await
                    .map_err(|e| EditorError::Bridge(format!("mount rejected: {e:?}")))?;
                // Await the handle's `ready` promise — gates on Monaco finishing its
                // construction. Once resolved, the editor is interactive.
                if let Some(ready) = Reflect::get(&handle, &JsValue::from_str("ready"))
                    .ok()
                    .and_then(|v| v.dyn_into::<Promise>().ok())
                {
                    let _ = JsFuture::from(ready).await;
                }
                *self.handle.borrow_mut() = Some(handle);
                Ok(())
            })
        }

        fn set_content(&self, content: &str) {
            self.invoke("setContent", &[content.into()]);
        }

        fn get_content(&self) -> String {
            self.invoke("getContent", &[])
                .and_then(|v| v.as_string())
                .unwrap_or_default()
        }

        fn on_change(&self, cb: Box<dyn Fn(String) + 'static>) {
            // Wrap the Rust callback as a JS Closure. Track it on `self.closures` so dispose
            // can free it (avoids the leak class described in R2). The bridge returns its own
            // unsubscribe function; we drop it because dispose handles all cleanup at once.
            let closure: Closure<dyn FnMut(JsValue)> = Closure::new(move |val: JsValue| {
                if let Some(s) = val.as_string() {
                    cb(s);
                }
            });
            let js_fn = closure.as_ref().clone();
            self.closures.borrow_mut().push(closure);
            self.invoke("onChange", &[js_fn]);
        }

        fn snapshot(&self) -> EditorState {
            let raw = match self.invoke("snapshot", &[]) {
                Some(v) => v,
                None => return EditorState::default(),
            };
            let cursor = Reflect::get(&raw, &JsValue::from_str("cursor"))
                .ok()
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as u32;
            let scroll = Reflect::get(&raw, &JsValue::from_str("scroll"))
                .ok()
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as u32;
            let selection = Reflect::get(&raw, &JsValue::from_str("selection")).ok().and_then(|v| {
                if v.is_null() || v.is_undefined() {
                    return None;
                }
                let arr: js_sys::Array = v.dyn_into().ok()?;
                let start = arr.get(0).as_f64()? as u32;
                let end = arr.get(1).as_f64()? as u32;
                Some((start, end))
            });
            EditorState { cursor, selection, scroll }
        }

        fn restore(&self, state: EditorState) {
            let obj = Object::new();
            let _ = Reflect::set(&obj, &"cursor".into(), &JsValue::from_f64(state.cursor as f64));
            let _ = Reflect::set(&obj, &"scroll".into(), &JsValue::from_f64(state.scroll as f64));
            match state.selection {
                Some((a, b)) => {
                    let arr = js_sys::Array::new();
                    arr.push(&JsValue::from_f64(a as f64));
                    arr.push(&JsValue::from_f64(b as f64));
                    let _ = Reflect::set(&obj, &"selection".into(), &arr);
                }
                None => {
                    let _ = Reflect::set(&obj, &"selection".into(), &JsValue::NULL);
                }
            }
            self.invoke("restore", &[obj.into()]);
        }

        fn set_read_only(&self, ro: bool) {
            self.invoke("setReadOnly", &[JsValue::from_bool(ro)]);
        }

        fn set_theme(&self, theme: EditorThemeBlob) {
            self.invoke("setTheme", &[theme.blob.into()]);
        }

        fn dispatch(&self, cmd: EditorCommand) {
            let s = match cmd {
                EditorCommand::Undo => "Undo",
                EditorCommand::Redo => "Redo",
                EditorCommand::FormatDocument => "FormatDocument",
                EditorCommand::FindReplace => "FindReplace",
                EditorCommand::ToggleComment => "ToggleComment",
                // Plans-Phase-2-editor-auto-focus: route to the JS shim's
                // dispatch handler, which calls `editor.focus()` on the
                // underlying Monaco instance.
                EditorCommand::Focus => "Focus",
            };
            self.invoke("dispatch", &[s.into()]);
        }

        fn dispose(&mut self) {
            self.invoke("dispose", &[]);
            *self.handle.borrow_mut() = None;
            // Dropping the closures cancels their FnMut bodies and frees the JS-side function
            // refs. Without this `Vec`, every `on_change` registration would leak its Closure.
            self.closures.borrow_mut().clear();
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use imp::MonacoBackend;

#[cfg(not(target_arch = "wasm32"))]
mod stub {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;

    /// Native (desktop / unit-test) stub. Constructed and named identically to the wasm impl
    /// so generic code compiles, but every method either no-ops or returns `NotMounted` — DOM
    /// is unreachable from native Rust.
    pub struct MonacoBackend;

    impl MonacoBackend {
        pub fn new() -> Self {
            Self
        }
    }

    impl Default for MonacoBackend {
        fn default() -> Self {
            Self::new()
        }
    }

    impl EditorBackend for MonacoBackend {
        type Target = ();

        fn mount<'a>(
            &'a mut self,
            _target: (),
            _init: BackendInit,
        ) -> Pin<Box<dyn Future<Output = Result<(), EditorError>> + 'a>> {
            Box::pin(async { Err(EditorError::NotMounted) })
        }

        fn set_content(&self, _content: &str) {}
        fn get_content(&self) -> String {
            String::new()
        }
        fn on_change(&self, _cb: Box<dyn Fn(String) + 'static>) {}
        fn snapshot(&self) -> EditorState {
            EditorState::default()
        }
        fn restore(&self, _state: EditorState) {}
        fn set_read_only(&self, _ro: bool) {}
        fn set_theme(&self, _theme: EditorThemeBlob) {}
        fn dispatch(&self, _cmd: EditorCommand) {}
        fn dispose(&mut self) {}
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use stub::MonacoBackend;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn stub_get_content_is_empty() {
        let b = MonacoBackend::new();
        assert_eq!(b.get_content(), "");
    }

    #[test]
    fn stub_snapshot_is_default() {
        let b = MonacoBackend::new();
        assert_eq!(b.snapshot(), EditorState::default());
    }

    #[test]
    fn stub_dispose_is_idempotent() {
        let mut b = MonacoBackend::new();
        b.dispose();
        b.dispose();
    }
}
