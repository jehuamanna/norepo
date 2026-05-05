//! Main area: hosts the tab strip, mode toolbar, and renders the active tab via its
//! `FormatPlugin` according to the tab's current `EditorMode`.

use std::rc::Rc;

use dioxus::prelude::*;

use crate::editor::EditorMode;
use crate::plugin::PluginRegistry;
use crate::shell::mode_toolbar::ModeToolbar;
use crate::tabs::{TabId, TabManager, TabStrip};

#[component]
pub fn MainArea() -> Element {
    let mut tabs: Signal<TabManager> = use_context();
    let registry: Rc<PluginRegistry> = use_context();

    let active_info: Option<(TabId, String, String, String, EditorMode)> = {
        let snapshot = tabs.read();
        snapshot.active().map(|tab| {
            (
                tab.id,
                tab.format_id.clone(),
                tab.note_id.clone(),
                tab.content.clone(),
                tab.mode,
            )
        })
    };

    let body: Element = match active_info {
        None => rsx! {
            div { class: "operon-main-empty",
                "No notes open — open one from the side bar or via the command palette."
            }
        },
        Some((tab_id, format_id, note_id, content, mode)) => {
            match registry.format_plugin_for(&format_id) {
                Some(plugin) => {
                    // on_change writes the new content back through the TabManager. Plugins
                    // that don't need to mutate (View, plain rendering) don't invoke it.
                    let on_change = EventHandler::new(move |new_content: String| {
                        tabs.write().set_content(tab_id, new_content);
                    });
                    match mode {
                        EditorMode::View => plugin.render(&note_id, &content),
                        EditorMode::Edit => plugin.render_edit(&note_id, &content, on_change),
                        EditorMode::LivePreview => {
                            plugin.render_live_preview(&note_id, &content, on_change)
                        }
                        EditorMode::Split => {
                            // Phase 3 ships the dedicated SplitView shell layout. Until
                            // then, render side-by-side via a simple flex container so the
                            // capability surface is honest.
                            let view_content = content.clone();
                            let view_note_id = note_id.clone();
                            let edit_note_id = note_id.clone();
                            rsx! {
                                div { class: "operon-split-host",
                                    div { class: "operon-split-pane operon-split-view",
                                        {plugin.render(&view_note_id, &view_content)}
                                    }
                                    div { class: "operon-split-divider" }
                                    div { class: "operon-split-pane operon-split-edit",
                                        {plugin.render_edit(&edit_note_id, &content, on_change)}
                                    }
                                }
                            }
                        }
                    }
                }
                None => rsx! {
                    div { class: "operon-main-empty",
                        "No plugin registered for format {format_id:?}"
                    }
                },
            }
        }
    };

    rsx! {
        section {
            "data-region": "main-area",
            class: "operon-region operon-main-area",
            TabStrip {}
            ModeToolbar {}
            div { class: "operon-main-body", {body} }
        }
    }
}
