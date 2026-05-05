//! `TiptapBackend` — Rust-side EditorBackend impl for the Tiptap bridge.
//!
//! Mirrors `MonacoBackend` / `CodeMirror6Backend` shape. Note: ProseMirror offsets do not
//! match Monaco / CM6 character offsets, so EditorState round-trips cleanly only between
//! Tiptap-and-Tiptap. Mode switches across backends (markdown View ↔ richtext-tiptap Edit)
//! drop the cursor — documented behaviour.

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

    pub struct TiptapBackend {
        handle: Rc<RefCell<Option<JsValue>>>,
        closures: Rc<RefCell<Vec<Closure<dyn FnMut(JsValue)>>>>,
    }

    impl TiptapBackend {
        pub fn new() -> Self {
            Self {
                handle: Rc::new(RefCell::new(None)),
                closures: Rc::new(RefCell::new(Vec::new())),
            }
        }

        fn invoke(&self, name: &str, args: &[JsValue]) -> Option<JsValue> {
            let h = self.handle.borrow();
            let this = h.as_ref()?;
            let f = Reflect::get(this, &JsValue::from_str(name)).ok()?;
            let f = f.dyn_into::<Function>().ok()?;
            let arr = js_sys::Array::new();
            for a in args {
                arr.push(a);
            }
            f.apply(this, &arr).ok()
        }
    }

    impl Default for TiptapBackend {
        fn default() -> Self {
            Self::new()
        }
    }

    impl EditorBackend for TiptapBackend {
        type Target = web_sys::Element;

        fn mount<'a>(
            &'a mut self,
            target: web_sys::Element,
            init: BackendInit,
        ) -> Pin<Box<dyn Future<Output = Result<(), EditorError>> + 'a>> {
            Box::pin(async move {
                let window = web_sys::window().ok_or(EditorError::Bridge("no window".into()))?;
                let bridge = Reflect::get(&window, &JsValue::from_str("operonBridge"))
                    .map_err(|_| EditorError::Bridge("operonBridge missing".into()))?;
                let mount_fn = Reflect::get(&bridge, &JsValue::from_str("mount"))
                    .ok()
                    .and_then(|v| v.dyn_into::<Function>().ok())
                    .ok_or(EditorError::Bridge("operonBridge.mount missing".into()))?;

                let init_obj = Object::new();
                let _ = Reflect::set(&init_obj, &"kind".into(), &"tiptap".into());
                let _ = Reflect::set(
                    &init_obj,
                    &"languageId".into(),
                    &init.language.id.into(),
                );
                let _ = Reflect::set(
                    &init_obj,
                    &"content".into(),
                    &init.initial_content.as_str().into(),
                );
                let _ = Reflect::set(&init_obj, &"theme".into(), &init.theme.blob.as_str().into());
                let _ = Reflect::set(&init_obj, &"readOnly".into(), &JsValue::from_bool(init.read_only));

                let promise: Promise = mount_fn
                    .call2(&JsValue::NULL, &target, &init_obj)
                    .map_err(|e| EditorError::Bridge(format!("{e:?}")))?
                    .dyn_into()
                    .map_err(|_| EditorError::Bridge("mount returned non-Promise".into()))?;
                let handle = JsFuture::from(promise)
                    .await
                    .map_err(|e| EditorError::Bridge(format!("mount rejected: {e:?}")))?;
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
            let selection = Reflect::get(&raw, &JsValue::from_str("selection"))
                .ok()
                .and_then(|v| {
                    if v.is_null() || v.is_undefined() {
                        return None;
                    }
                    let arr: js_sys::Array = v.dyn_into().ok()?;
                    let s = arr.get(0).as_f64()? as u32;
                    let e = arr.get(1).as_f64()? as u32;
                    Some((s, e))
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

        fn dispatch(&self, _cmd: EditorCommand) {}

        fn dispose(&mut self) {
            self.invoke("dispose", &[]);
            *self.handle.borrow_mut() = None;
            self.closures.borrow_mut().clear();
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use imp::TiptapBackend;

#[cfg(not(target_arch = "wasm32"))]
mod stub {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;

    pub struct TiptapBackend;

    impl TiptapBackend {
        pub fn new() -> Self {
            Self
        }
    }

    impl Default for TiptapBackend {
        fn default() -> Self {
            Self::new()
        }
    }

    impl EditorBackend for TiptapBackend {
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
pub use stub::TiptapBackend;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn stub_get_content_is_empty() {
        let b = TiptapBackend::new();
        assert_eq!(b.get_content(), "");
    }

    #[test]
    fn stub_dispose_is_idempotent() {
        let mut b = TiptapBackend::new();
        b.dispose();
        b.dispose();
    }
}
