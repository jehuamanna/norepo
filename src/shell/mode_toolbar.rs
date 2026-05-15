//! Per-tab mode toolbar — View / Edit / Live Preview / Split.
//!
//! Renders one button per editor mode the active tab's `FormatPlugin` claims via its
//! `capabilities()`. Modes the plugin doesn't claim are hidden — never offered. Active mode
//! gets `aria-pressed="true"` plus an active CSS class.
//!
//! Lives directly above the `MainArea` body; clicking a button mutates the tab's
//! `mode: EditorMode` field which the dispatcher reads on the next render.
//!
//! **Revision flow integration.** For every EDIT-capable format
//! *except* skill (whose own toolbar embeds `RevisionFlowButtons`
//! next to its ▶ Run button), this toolbar mounts a
//! `RevisionFlowButtons` cluster. It supersedes the plain `Revise`
//! mode button — clicking the cluster's "Edit" snapshots the body,
//! flips to Edit, and reveals Cancel/Done. Done opens a required
//! manual-summary dialog and appends a `manual` row to the body's
//! `## Revision history` table.

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
        snapshot
            .active()
            .map(|t| (t.id, t.format_id.clone(), t.note_id.clone(), t.mode))
    };

    let Some((tab_id, format_id, note_id, mode)) = active else {
        return rsx! { div { class: "operon-mode-toolbar operon-mode-toolbar-empty" } };
    };
    // `note_id` is only consumed by the desktop `RevisionFlowButtons`
    // branch; suppress the unused-binding warning on wasm.
    #[cfg(target_arch = "wasm32")]
    let _ = &note_id;
    let caps = registry
        .format_plugin_for(&format_id)
        .map(|p| p.capabilities())
        .unwrap_or(FormatCaps::NONE);

    // Skill mounts the revise/done/cancel cluster inline alongside
    // its own ▶ Run button (see `plugins::skill::view::SkillToolbar`),
    // so the mode toolbar emits NO Revise button for that format —
    // otherwise we'd have two competing entry points into Edit, and
    // the mode-toolbar one wouldn't snapshot `prior_body` so Cancel
    // would have nothing to revert to.
    let edit_capable = caps.contains(FormatCaps::EDIT);
    let revise_cluster: Element = build_revise_cluster(
        edit_capable,
        format_id == "skill",
        note_id.clone(),
        tab_id,
        mode,
        tabs,
    );

    rsx! {
        div { class: "operon-mode-toolbar",
            "data-component": "mode-toolbar",
            "data-active-mode": mode_slug(mode),
            if caps.contains(FormatCaps::VIEW) {
                ModeButton { mode_label: "View", target: EditorMode::View, current: mode, tab_id, tabs }
            }
            {revise_cluster}
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

/// Build the Edit-mode button(s) the toolbar shows for an EDIT-capable
/// format. On desktop, non-skill formats get the rich
/// `RevisionFlowButtons` cluster (Revise + Cancel + Done dialog). Skill
/// + wasm fall back to a plain `Revise` mode button — skill because
/// its own toolbar mounts the cluster; wasm because `Persistence` save
/// isn't wired through the OPFS backend yet.
fn build_revise_cluster(
    edit_capable: bool,
    owns_inline_revise: bool,
    note_id: String,
    tab_id: crate::tabs::TabId,
    mode: EditorMode,
    tabs: Signal<TabManager>,
) -> Element {
    if !edit_capable {
        let _ = (note_id, tab_id, mode, tabs);
        return rsx! {};
    }
    if owns_inline_revise {
        // Skill: its own toolbar mounts the cluster. Emit nothing
        // here so the user has exactly one Edit entry point and so
        // `prior_body` is always snapshotted on the View→Edit flip.
        let _ = (note_id, tab_id, mode, tabs);
        return rsx! {};
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (tab_id, mode, tabs);
        rsx! {
            crate::plugins::revise_flow::RevisionFlowButtons {
                note_id,
                class_root: "operon-mode-button-revise".to_string(),
                testid_prefix: "mode-revise".to_string(),
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        // Web: `Persistence` save isn't wired through OPFS yet; fall
        // back to the plain mode button until that lands.
        let _ = note_id;
        rsx! {
            ModeButton { mode_label: "Revise", target: EditorMode::Edit, current: mode, tab_id, tabs }
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
