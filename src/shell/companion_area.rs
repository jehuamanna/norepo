//! Companion area: the right-edge region reserved for assistant / companion surfaces.
//!
//! Plans-Phase-4 hosts the Companion chat surface here. The actual chat UI lives in
//! `companion_chat.rs`; this file owns layout/collapse semantics only.

use dioxus::prelude::*;

#[cfg(not(target_arch = "wasm32"))]
use crate::shell::companion_chat::CompanionChat;
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
        #[cfg(not(target_arch = "wasm32"))]
        {
            rsx! {
                section {
                    "data-region": "companion-area",
                    class: "operon-region operon-companion-area",
                    "data-collapsed": "false",
                    CompanionChat {}
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            rsx! {
                section {
                    "data-region": "companion-area",
                    class: "operon-region operon-companion-area",
                    "data-collapsed": "false",
                    div { class: "operon-companion-placeholder", "Companion (web build: chat unavailable until wasm runtime lands)" }
                }
            }
        }
    }
}
