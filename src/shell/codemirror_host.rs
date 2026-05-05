//! `CodeMirrorEditorHost` — shared component every plugin's `render_live_preview`
//! delegates to. Mirrors `MonacoEditorHost` lifecycle: spawn async mount, register
//! on_change, dispose on unmount. Wasm32 only; native build renders a stub.

use dioxus::prelude::*;

use crate::editor::LanguageDescriptor;

#[component]
pub fn CodeMirrorEditorHost(
    note_id: String,
    content: String,
    language: LanguageDescriptor,
    on_change: EventHandler<String>,
) -> Element {
    let note_id_attr = note_id.clone();
    let language_attr = language.id;

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (&content, &on_change, &language);
        return rsx! {
            div {
                class: "operon-cm-host operon-cm-host-stub",
                "data-cm-host": "{note_id_attr}",
                "data-cm-language": "{language_attr}",
                "data-stub": "true",
                div { class: "operon-cm-host-placeholder",
                    "CodeMirror Live Preview mounts in the web build. Active language: {language_attr}"
                }
            }
        };
    }

    #[cfg(target_arch = "wasm32")]
    {
        use std::cell::RefCell;
        use std::rc::Rc;

        use crate::editor::{
            BackendInit, CodeMirror6Backend, EditorBackend, EditorThemeBlob,
        };
        use crate::theme::{editor_theme, Theme};

        let theme: Signal<Theme> = use_context();
        let backend: Signal<Rc<RefCell<CodeMirror6Backend>>> =
            use_signal(|| Rc::new(RefCell::new(CodeMirror6Backend::new())));

        let host_id = format!("operon-cm-host-{note_id}");
        let host_id_for_effect = host_id.clone();
        let initial_content = content.clone();
        let language_for_effect = language.clone();
        let theme_blob = editor_theme::monaco_blob(&theme.read());

        use_effect(move || {
            let host_id = host_id_for_effect.clone();
            let initial_content = initial_content.clone();
            let language = language_for_effect.clone();
            let theme_blob = theme_blob.clone();
            let backend = backend.clone();
            let on_change = on_change;
            spawn(async move {
                let Some(window) = web_sys::window() else { return };
                let Some(doc) = window.document() else { return };
                let Some(target) = doc.get_element_by_id(&host_id) else { return };
                let init = BackendInit {
                    language,
                    initial_content,
                    theme: EditorThemeBlob { blob: theme_blob.blob },
                    read_only: false,
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
                class: "operon-cm-host",
                "data-cm-host": "{note_id_attr}",
                "data-cm-language": "{language_attr}",
                style: "width: 100%; height: 100%; min-height: 300px;",
            }
        };
    }
}
