//! Side-bar panel rendered by [`super::NotesExplorer`] when its activity item is active.

use dioxus::prelude::*;

use super::samples::SAMPLES;
use crate::tabs::TabManager;

#[component]
pub fn NotesExplorerPanel() -> Element {
    let mut tabs: Signal<TabManager> = use_context();
    let samples: Vec<(&'static str, &'static str, &'static str)> =
        SAMPLES.iter().copied().collect();

    rsx! {
        div { class: "notes-explorer-panel",
            div {
                class: "notes-explorer-heading",
                style: "font-size: 11px; text-transform: uppercase; letter-spacing: 0.05em; padding: 0 0 6px 0; opacity: 0.7;",
                "Notes Explorer"
            }
            ul {
                class: "notes-explorer-list",
                style: "list-style: none; padding: 0; margin: 0;",
                for (id, title, content) in samples {
                    li {
                        class: "notes-explorer-row",
                        style: "padding: 4px 6px; cursor: pointer; border-radius: 3px;",
                        onclick: move |_| {
                            tabs.write().open(
                                id.to_string(),
                                "markdown".to_string(),
                                title.to_string(),
                                content.to_string(),
                            );
                        },
                        "{title}"
                    }
                }
            }
        }
    }
}
