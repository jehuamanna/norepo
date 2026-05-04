//! Main area: the center region that hosts the tab strip and the active note's content.
//!
//! Phase 1 ships only a placeholder body. Phase 3 mounts the [`crate::tabs::TabStrip`] and the
//! plugin-driven body delegation.

use dioxus::prelude::*;

#[component]
pub fn MainArea() -> Element {
    rsx! {
        section {
            "data-region": "main-area",
            class: "operon-region operon-main-area",
            div { class: "operon-main-area-placeholder", "Main Area" }
        }
    }
}
