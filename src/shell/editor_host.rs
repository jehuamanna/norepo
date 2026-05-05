//! Shared Monaco-mounting component used by every source-text format plugin's `render_edit`.
//!
//! Owns a `MonacoBackend` for the lifetime of its mounted DOM node: spawns the async mount
//! once the target `<div>` is in the tree, registers `on_change` so the plugin's
//! `EventHandler<String>` fires on every content mutation, and disposes the backend when the
//! component unmounts (Dioxus's `use_drop` runs the cleanup).
//!
//! A single component shared across markdown / plaintext / json / mdx avoids four copies of
//! the wasm-bindgen wiring — Plans-Phase-0's "shared bridge layer" rationale.

use dioxus::prelude::*;

use crate::editor::LanguageDescriptor;

#[component]
pub fn MonacoEditorHost(
    note_id: String,
    content: String,
    language: LanguageDescriptor,
    on_change: EventHandler<String>,
) -> Element {
    // The `data-*` attributes let Playwright + wasm-bindgen tests assert the mount fired.
    // Capability-honest: this surface only renders when the active plugin claims `EDIT`,
    // so reaching here means a Monaco mount is desired.
    let note_id_attr = note_id.clone();
    let language_attr = language.id;

    // Native build: Dioxus desktop runs Rust natively; DOM is reachable inside the webview
    // via Dioxus's own JS layer rather than via wasm-bindgen + web_sys::Element. Mounting a
    // real Monaco editor on desktop requires the Phase-2 follow-up of routing through
    // `dioxus::eval`; for now the desktop build renders a clear placeholder so the surface
    // is visible during dev.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (&content, &on_change, &language); // silence unused
        return rsx! {
            div {
                class: "operon-monaco-host operon-monaco-host-stub",
                "data-monaco-host": "{note_id_attr}",
                "data-monaco-language": "{language_attr}",
                "data-stub": "true",
                div { class: "operon-monaco-host-placeholder",
                    "Monaco editor mounts in the web build (desktop wiring lands in a follow-up). Active language: {language_attr}"
                }
            }
        };
    }

    #[cfg(target_arch = "wasm32")]
    {
        use std::cell::RefCell;
        use std::rc::Rc;

        use crate::editor::{
            BackendInit, EditorBackend, EditorThemeBlob, MonacoBackend,
        };
        use crate::theme::{editor_theme, Theme};

        let theme: Signal<Theme> = use_context();
        let backend: Signal<Rc<RefCell<MonacoBackend>>> =
            use_signal(|| Rc::new(RefCell::new(MonacoBackend::new())));

        // Mount once on first render with the host element. Re-runs are safe — MonacoBackend
        // tracks `disposed` internally and stale calls are no-ops.
        let host_id = format!("operon-monaco-host-{note_id}");
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
                // Dioxus may not have flushed the host element yet; one rAF is sufficient.
                let Some(target) = doc.get_element_by_id(&host_id) else { return };
                let init = BackendInit {
                    language,
                    initial_content,
                    theme: theme_blob,
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
                class: "operon-monaco-host",
                "data-monaco-host": "{note_id_attr}",
                "data-monaco-language": "{language_attr}",
                style: "width: 100%; height: 100%; min-height: 300px;",
            }
        };
    }
}
