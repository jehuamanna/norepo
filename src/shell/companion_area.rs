//! Companion area: the right-edge region reserved for assistant / companion surfaces.
//!
//! Phase 1 keeps this as an empty placeholder; later seeds may host an LLM chat or
//! collaboration panel here.

use dioxus::prelude::*;

use crate::shell::layout::LayoutState;

#[component]
pub fn CompanionArea() -> Element {
    let layout: Signal<LayoutState> = use_context();
    let collapsed = layout.read().companion_collapsed;

    if collapsed {
        rsx! {
            section {
                "data-region": "companion-area",
                class: "operon-region operon-companion-area",
                "data-collapsed": "true",
                style: "display: none;",
            }
        }
    } else {
        rsx! {
            section {
                "data-region": "companion-area",
                class: "operon-region operon-companion-area",
                "data-collapsed": "false",
                div { class: "operon-companion-placeholder", "Companion" }
            }
        }
    }
}
