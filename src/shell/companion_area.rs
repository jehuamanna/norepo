//! Companion area: the right-edge region reserved for assistant / companion surfaces.
//!
//! Plans-Phase-4 hosts the Companion chat surface here. The actual chat UI lives in
//! `companion_chat.rs`; this file owns layout/collapse semantics only.
//!
//! The user can swap the chat for a raw Claude Code terminal via
//! Settings → Companion pane. We resolve the persisted choice on every
//! render (cheap key/value read) and subscribe to
//! [`COMPANION_MODE_VERSION`] so a toggle in Settings swaps the surface
//! in-place without a restart.

use dioxus::prelude::*;

#[cfg(not(target_arch = "wasm32"))]
use crate::shell::companion_chat::CompanionChat;
#[cfg(not(target_arch = "wasm32"))]
use crate::shell::companion_terminal::CompanionClaudeTerminal;
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
            // Subscribe so a Settings toggle re-renders this scope; the
            // returned tick value is unused beyond establishing the
            // dependency.
            let _ = crate::shell::companion_state::COMPANION_MODE_VERSION.read();
            let crate::local_mode::desktop::LocalSettingsRepo(settings_repo) =
                use_context();
            let mode = settings_repo
                .get(crate::local_mode::SETTINGS_KEY_COMPANION_MODE)
                .ok()
                .flatten()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    crate::local_mode::COMPANION_MODE_CHAT.to_string()
                });
            let is_terminal = mode == crate::local_mode::COMPANION_MODE_CLAUDE_CODE;
            rsx! {
                section {
                    "data-region": "companion-area",
                    class: "operon-region operon-companion-area",
                    "data-collapsed": "false",
                    "data-companion-mode": if is_terminal { "claude_code" } else { "chat" },
                    if is_terminal {
                        CompanionClaudeTerminal {}
                    } else {
                        CompanionChat {}
                    }
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
