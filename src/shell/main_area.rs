//! Main area: hosts the tab strip and renders the active tab via its `FormatPlugin`.

use std::rc::Rc;

use dioxus::prelude::*;

use crate::plugin::PluginRegistry;
use crate::tabs::{TabManager, TabStrip};

#[component]
pub fn MainArea() -> Element {
    let tabs: Signal<TabManager> = use_context();
    let registry: Rc<PluginRegistry> = use_context();

    let active_info: Option<(String, String, String)> = {
        let snapshot = tabs.read();
        snapshot
            .active()
            .map(|tab| (tab.format_id.clone(), tab.note_id.clone(), tab.content.clone()))
    };

    let body: Element = match active_info {
        None => rsx! {
            div { class: "operon-main-empty",
                "No notes open — open one from the side bar or via the command palette."
            }
        },
        Some((format_id, id, content)) => match registry.format_plugin_for(&format_id) {
            Some(plugin) => plugin.render(&id, &content),
            None => rsx! {
                div { class: "operon-main-empty",
                    "No plugin registered for format {format_id:?}"
                }
            },
        },
    };

    rsx! {
        section {
            "data-region": "main-area",
            class: "operon-region operon-main-area",
            TabStrip {}
            div { class: "operon-main-body", {body} }
        }
    }
}
