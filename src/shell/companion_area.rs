//! Companion area: the right-edge region reserved for assistant / companion surfaces.
//!
//! Phase 1 keeps this as an empty placeholder; later seeds may host an LLM chat or
//! collaboration panel here.

use dioxus::prelude::*;

#[component]
pub fn CompanionArea() -> Element {
    rsx! {
        section {
            "data-region": "companion-area",
            class: "operon-region operon-companion-area",
            div { class: "operon-companion-placeholder", "Companion" }
        }
    }
}
