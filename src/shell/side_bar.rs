//! Side bar: panel adjacent to the activity bar.
//!
//! Renders the contribution from the `UIPlugin` whose id matches the active activity-item.
//! Collapses (zero width via grid var) when no item is active.

use std::rc::Rc;

use dioxus::prelude::*;

use crate::plugin::{PluginRegistry, PluginSurface};
use crate::shell::state::ActiveActivity;

#[component]
pub fn SideBar() -> Element {
    let registry: Rc<PluginRegistry> = use_context();
    let ActiveActivity(active) = use_context();

    let panel: Option<Element> = active.read().as_ref().and_then(|aid| {
        let plugin_id = aid.0.split(':').next().unwrap_or("").to_string();
        registry
            .contributions(PluginSurface::SideBarPanel)
            .find(|p| p.manifest().id == plugin_id)
            .map(|p| p.render(PluginSurface::SideBarPanel))
    });

    match panel {
        Some(content) => rsx! {
            section {
                "data-region": "side-bar",
                class: "operon-region operon-side-bar",
                "data-collapsed": "false",
                {content}
            }
        },
        None => rsx! {
            section {
                "data-region": "side-bar",
                class: "operon-region operon-side-bar",
                "data-collapsed": "true",
                style: "display: none;",
            }
        },
    }
}
