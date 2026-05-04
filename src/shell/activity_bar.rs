//! Activity bar: pinned vertical icon column on the left edge.
//!
//! Phase 1 ships this as an empty placeholder; Phase 4 populates it from `UIPlugin`
//! contributions registered against [`crate::plugin::PluginSurface::ActivityBar`].

use dioxus::prelude::*;

#[component]
pub fn ActivityBar() -> Element {
    rsx! {
        section {
            "data-region": "activity-bar",
            class: "operon-region operon-activity-bar",
            div { class: "operon-activity-bar-placeholder", "AB" }
        }
    }
}
