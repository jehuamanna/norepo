//! Side bar: panel adjacent to the activity bar.
//!
//! Phase 1 ships this as a static placeholder; Phase 4 wires it to render the active
//! activity item's contributed panel.

use dioxus::prelude::*;

#[component]
pub fn SideBar() -> Element {
    rsx! {
        section {
            "data-region": "side-bar",
            class: "operon-region operon-side-bar",
            div { class: "operon-side-bar-placeholder", "Side Bar" }
        }
    }
}
