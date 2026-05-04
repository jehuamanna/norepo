//! Panel tab strip + body delegation.

use dioxus::prelude::*;

use super::{LogsView, PanelManager, PanelTabId};

#[component]
pub fn PanelStrip() -> Element {
    let mut panel: Signal<PanelManager> = use_context();
    let snapshot = panel.read();
    let active = snapshot.active();
    let view: Vec<(PanelTabId, &'static str, bool)> = snapshot
        .iter()
        .map(|t| (t.id, t.title, t.id == active))
        .collect();
    drop(snapshot);

    let body: Element = match active.0 {
        "logs" => rsx! { LogsView {} },
        _ => rsx! {
            div { class: "operon-panel-empty",
                "No content yet for this tab."
            }
        },
    };

    rsx! {
        section {
            "data-region": "panel",
            class: "operon-region operon-panel",
            div { class: "operon-panel-strip",
                for (id, title, is_active) in view {
                    div {
                        class: if is_active { "operon-panel-tab operon-panel-tab-active" } else { "operon-panel-tab" },
                        "data-id": "{id.0}",
                        "data-active": if is_active { "true" } else { "false" },
                        onclick: move |_| { panel.write().activate(id); },
                        "{title}"
                    }
                }
            }
            div { class: "operon-panel-body",
                {body}
            }
        }
    }
}
