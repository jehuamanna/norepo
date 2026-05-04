//! Status bar: bottom-spanning row.
//!
//! Phase 1 ships only a static label; the temporary theme-toggle button is wired in the
//! next commit.

use dioxus::prelude::*;

#[component]
pub fn StatusBar() -> Element {
    rsx! {
        section {
            "data-region": "status-bar",
            class: "operon-region operon-status-bar",
            span { class: "operon-status-bar-label", "Operon" }
        }
    }
}
