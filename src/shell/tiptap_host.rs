//! `TiptapEditorHost` — shared component the richtext-tiptap plugin's render +
//! render_edit delegate to. Mirrors MonacoEditorHost / CodeMirrorEditorHost.

use dioxus::prelude::*;

use crate::editor::LanguageDescriptor;

#[component]
pub fn TiptapEditorHost(
    note_id: String,
    content: String,
    language: LanguageDescriptor,
    read_only: bool,
    on_change: EventHandler<String>,
) -> Element {
    let note_id_attr = note_id.clone();

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (&content, &on_change, &language);
        return rsx! {
            div {
                class: "operon-tiptap operon-tiptap-stub",
                "data-tiptap-host": "{note_id_attr}",
                "data-read-only": "{read_only}",
                "data-stub": "true",
                div { class: "operon-tiptap-placeholder",
                    "Tiptap richtext mounts in the web build."
                }
            }
        };
    }

    #[cfg(target_arch = "wasm32")]
    {
        use std::cell::RefCell;
        use std::rc::Rc;

        use crate::editor::{
            BackendInit, EditorBackend, EditorThemeBlob, TiptapBackend,
        };

        let backend: Signal<Rc<RefCell<TiptapBackend>>> =
            use_signal(|| Rc::new(RefCell::new(TiptapBackend::new())));

        let host_id = format!("operon-tiptap-host-{note_id}");
        let host_id_for_effect = host_id.clone();
        let initial_content = content.clone();
        let language_for_effect = language.clone();

        use_effect(move || {
            let host_id = host_id_for_effect.clone();
            let initial_content = initial_content.clone();
            let language = language_for_effect.clone();
            let backend = backend.clone();
            let on_change = on_change;
            spawn(async move {
                let Some(window) = web_sys::window() else { return };
                let Some(doc) = window.document() else { return };
                let Some(target) = doc.get_element_by_id(&host_id) else { return };
                let init = BackendInit {
                    language,
                    initial_content,
                    theme: EditorThemeBlob::default(),
                    read_only,
                };
                let bk = backend.read().clone();
                let mount_res = bk.borrow_mut().mount(target, init).await;
                if mount_res.is_ok() {
                    bk.borrow().on_change(Box::new(move |new_content| {
                        on_change.call(new_content);
                    }));
                }
            });
        });

        let _drop_guard = use_drop(move || {
            let bk = backend.read().clone();
            bk.borrow_mut().dispose();
        });

        return rsx! {
            div {
                id: "{host_id}",
                class: "operon-tiptap",
                "data-tiptap-host": "{note_id_attr}",
                "data-read-only": "{read_only}",
                style: "width: 100%; height: 100%; min-height: 300px;",
            }
        };
    }
}
