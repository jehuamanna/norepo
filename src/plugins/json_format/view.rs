//! View + Edit components for the JSON format plugin.

use dioxus::prelude::*;

use crate::editor::LanguageDescriptor;

#[component]
pub fn JsonView(content: String) -> Element {
    let pretty = super::pretty_print(&content);
    match pretty {
        Some(out) => rsx! {
            pre { class: "operon-json-view", "{out}" }
        },
        None => rsx! {
            pre {
                class: "operon-json-view operon-json-view-error",
                "data-error": "parse",
                "{content}"
            }
        },
    }
}

#[component]
pub fn JsonEditor(
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
