//! Panel tab strip + body delegation.

use dioxus::prelude::*;

use super::{LogsView, PanelManager, PanelTabId, ProblemsView};
#[cfg(not(target_arch = "wasm32"))]
use super::TerminalsView;
use crate::shell::layout::LayoutState;

#[component]
pub fn PanelStrip() -> Element {
    let mut panel: Signal<PanelManager> = use_context();
    let layout: Signal<LayoutState> = use_context();
    let snapshot = panel.read();
    let active = snapshot.active();
    let view: Vec<(PanelTabId, &'static str, bool)> = snapshot
        .iter()
        .map(|t| (t.id, t.title, t.id == active))
        .collect();
    drop(snapshot);

    if layout.read().panel_collapsed {
        return rsx! {
            section {
                "data-region": "panel",
                class: "operon-region operon-panel",
                "data-collapsed": "true",
                style: "display: none;",
            }
        };
    }

    let body: Element = match active.0 {
        "logs" => rsx! { LogsView {} },
        "problems" => rsx! { ProblemsView {} },
        #[cfg(not(target_arch = "wasm32"))]
        "terminal" => rsx! { TerminalsView {} },
        #[cfg(target_arch = "wasm32")]
        "terminal" => rsx! {
            div { class: "operon-panel-empty",
                "Terminal is only available on the desktop build."
            }
        },
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
            "data-collapsed": "false",
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
