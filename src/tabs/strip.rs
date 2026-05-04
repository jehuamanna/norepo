//! Tab strip rendered atop the main area.
//!
//! Stub used by the C1 commit so the `tabs` module compiles cleanly. Real implementation
//! lands in the C2 commit alongside the `MainArea` body delegation.

use dioxus::prelude::*;

#[component]
pub fn TabStrip() -> Element {
    rsx! { div { class: "operon-tab-strip-stub" } }
}
