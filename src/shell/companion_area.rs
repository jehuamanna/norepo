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
    let mut layout: Signal<LayoutState> = use_context();

    // Auto-uncollapse when a cascade run requests it (Play on an
    // artifact bumps `EXPAND_COMPANION_TICK` so the user sees live
    // thinking / tool_use rows instead of an empty chrome). Tracking
    // the previously-seen tick locally — initialised from the current
    // global value — keeps initial mount from clobbering a
    // deliberately-collapsed panel.
    let mut seen_expand_tick = use_signal(|| {
        *crate::shell::companion_state::EXPAND_COMPANION_TICK.peek()
    });
    use_effect(move || {
        let cur = *crate::shell::companion_state::EXPAND_COMPANION_TICK.read();
        if cur != *seen_expand_tick.peek() {
            seen_expand_tick.set(cur);
            layout.with_mut(|s| {
                if s.companion_collapsed {
                    s.companion_collapsed = false;
                }
            });
        }
    });

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
