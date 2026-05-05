//! Tab strip rendered atop the main area.
//!
//! Reads `Signal<TabManager>` from context. Click activates a tab; clicking the close icon
//! closes it. A `●` marker replaces the close `×` while a tab is dirty.

use dioxus::prelude::*;

use super::{Tab, TabId, TabManager};
use crate::ui::Icon;

#[component]
pub fn TabStrip() -> Element {
    let mut tabs: Signal<TabManager> = use_context();
    let snapshot = tabs.read();
    let active_id = snapshot.active_id();
    let view: Vec<(TabId, String, bool)> = snapshot
        .iter()
        .map(|t: &Tab| (t.id, t.title.clone(), t.dirty))
        .collect();
    drop(snapshot);

    rsx! {
        div { class: "operon-tab-strip",
            for (id, title, dirty) in view {
                div {
                    class: "operon-tab",
                    "data-active": if active_id == Some(id) { "true" } else { "false" },
                    onclick: move |_| { tabs.write().activate(id); },
                    span { class: "operon-tab-title", "{title}" }
                    span {
                        class: if dirty { "operon-tab-marker" } else { "operon-tab-close" },
                        onclick: move |evt| {
                            evt.stop_propagation();
                            tabs.write().close(id);
                        },
                        if dirty {
                            Icon { name: "circle-dot".to_string(), size: 12 }
                        } else {
                            Icon { name: "x".to_string(), size: 12, title: "Close tab".to_string() }
                        }
                    }
                }
            }
        }
    }
}
