//! Per-tab mode toolbar — View / Edit / Live Preview / Split.
//!
//! Renders one button per editor mode the active tab's `FormatPlugin` claims via its
//! `capabilities()`. Modes the plugin doesn't claim are hidden — never offered. Active mode
//! gets `aria-pressed="true"` plus an active CSS class.
//!
//! Lives directly above the `MainArea` body; clicking a button mutates the tab's
//! `mode: EditorMode` field which the dispatcher reads on the next render.

use std::rc::Rc;

use dioxus::prelude::*;

use crate::editor::EditorMode;
use crate::plugin::{FormatCaps, PluginRegistry};
use crate::tabs::TabManager;

#[component]
pub fn ModeToolbar() -> Element {
    let tabs: Signal<TabManager> = use_context();
    let registry: Rc<PluginRegistry> = use_context();

    let active = {
        let snapshot = tabs.read();
        snapshot.active().map(|t| (t.id, t.format_id.clone(), t.mode))
    };

    let Some((tab_id, format_id, mode)) = active else {
        return rsx! { div { class: "operon-mode-toolbar operon-mode-toolbar-empty" } };
    };
    let caps = registry
        .format_plugin_for(&format_id)
        .map(|p| p.capabilities())
        .unwrap_or(FormatCaps::NONE);

    rsx! {
        div { class: "operon-mode-toolbar",
            "data-component": "mode-toolbar",
            "data-active-mode": mode_slug(mode),
            if caps.contains(FormatCaps::VIEW) {
                ModeButton { mode_label: "View", target: EditorMode::View, current: mode, tab_id, tabs }
            }
            if caps.contains(FormatCaps::EDIT) {
                ModeButton { mode_label: "Edit", target: EditorMode::Edit, current: mode, tab_id, tabs }
            }
            if caps.contains(FormatCaps::LIVE_PREVIEW) {
                ModeButton { mode_label: "Live Preview", target: EditorMode::LivePreview, current: mode, tab_id, tabs }
            }
            if caps.contains(FormatCaps::VIEW) && caps.contains(FormatCaps::EDIT) {
                // Split is a shell layout that pairs view+edit; only offered when both are
                // available. Phase 3 implements the actual paired-pane rendering.
                ModeButton { mode_label: "Split", target: EditorMode::Split, current: mode, tab_id, tabs }
            }
        }
    }
}

#[component]
fn ModeButton(
    mode_label: &'static str,
    target: EditorMode,
    current: EditorMode,
    tab_id: crate::tabs::TabId,
    tabs: Signal<TabManager>,
) -> Element {
    let active = current == target;
    let class = if active {
        "operon-mode-button operon-mode-button-active"
    } else {
        "operon-mode-button"
    };
    let mut tabs = tabs;
    rsx! {
        button {
            class: "{class}",
            r#type: "button",
            "data-mode": mode_slug(target),
            "aria-pressed": if active { "true" } else { "false" },
            onclick: move |_| {
                tabs.write().set_mode(tab_id, target);
            },
            "{mode_label}"
        }
    }
}

const fn mode_slug(m: EditorMode) -> &'static str {
    match m {
        EditorMode::View => "view",
        EditorMode::Edit => "edit",
        EditorMode::LivePreview => "live-preview",
        EditorMode::Split => "split",
    }
}
