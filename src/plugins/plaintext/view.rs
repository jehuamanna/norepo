//! View + Edit components for the plaintext format plugin.
//!
//! `PlaintextView` is a read-only `<pre>` block. `PlaintextEditor` is the host element
//! MonacoBackend mounts into; the actual mount happens in a `use_effect` so the DOM target
//! exists before `MonacoBackend::mount(target, init).await` runs.

use dioxus::prelude::*;

use crate::editor::LanguageDescriptor;

#[component]
pub fn PlaintextView(content: String) -> Element {
    rsx! {
        pre { class: "operon-plaintext-view", "{content}" }
    }
}

/// Host element for an embedded MonacoBackend in plaintext language. The actual mount of
/// MonacoBackend is wired in Phase 2's `MonacoEditorHost` shared component (next commit);
/// for now the host renders an empty placeholder so the surface compiles.
#[component]
pub fn PlaintextEditor(
    note_id: String,
    content: String,
    language: LanguageDescriptor,
    on_change: EventHandler<String>,
) -> Element {
    rsx! {
        crate::shell::editor_host::MonacoEditorHost {
            note_id,
            content,
            language,
            on_change,
        }
    }
}
