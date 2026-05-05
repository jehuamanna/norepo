//! `SplitView` — paired view + edit pane with a draggable splitter.
//!
//! Mounted by `MainArea` when a tab's `EditorMode` is `Split`. Both panes share the tab's
//! content; right-pane edits flow through the same `on_change` EventHandler the dispatcher
//! provides, so the view pane re-renders reactively as the user types.

use std::rc::Rc;

use dioxus::prelude::*;

use crate::plugin::{FormatPlugin, PluginRegistry};

/// Splitter ratio (left-pane fraction of total width). 0.5 = 50/50.
const DEFAULT_SPLIT_RATIO: f64 = 0.5;
const MIN_RATIO: f64 = 0.15;
const MAX_RATIO: f64 = 0.85;

#[component]
pub fn SplitView(
    format_id: String,
    note_id: String,
    content: String,
    on_change: EventHandler<String>,
) -> Element {
    let registry: Rc<PluginRegistry> = use_context();
    let mut ratio = use_signal(|| DEFAULT_SPLIT_RATIO);
    let mut dragging = use_signal(|| false);

    let plugin_box = registry.format_plugin_for(&format_id);
    let Some(plugin) = plugin_box else {
        return rsx! {
            div { class: "operon-main-empty", "No plugin for format {format_id:?}" }
        };
    };

    let r = *ratio.read();
    let left_pct = (r * 100.0).clamp(MIN_RATIO * 100.0, MAX_RATIO * 100.0);
    let right_pct = 100.0 - left_pct;
    let style = format!(
        "display: grid; grid-template-columns: {left_pct:.2}% 4px {right_pct:.2}%; height: 100%; min-height: 0;"
    );

    let view_el = render_view(plugin, &note_id, &content);
    let edit_el = render_edit(plugin, &note_id, &content, on_change);

    let pointer_active = if *dragging.read() { "true" } else { "false" };

    rsx! {
        div {
            class: "operon-split-host",
            "data-component": "split-view",
            "data-pointer-active": "{pointer_active}",
            style: "{style}",
            onmousemove: move |e| {
                if !*dragging.read() { return; }
                if let Some(host) = e.data().client_coordinates().x.into() {
                    let _ = host;
                }
                let x = e.data().client_coordinates().x as f64;
                // Use the document's body width as a reasonable host width estimate. A
                // refined version would measure the actual host element via web_sys, but
                // body width is good enough for v1 ergonomics.
                let total = body_width().unwrap_or(1000.0);
                if total > 0.0 {
                    let new_ratio = (x / total).clamp(MIN_RATIO, MAX_RATIO);
                    ratio.set(new_ratio);
                }
            },
            onmouseup: move |_| { dragging.set(false); },
            onmouseleave: move |_| { dragging.set(false); },
            div { class: "operon-split-pane operon-split-view-pane",
                style: "overflow: auto; min-width: 0;",
                {view_el}
            }
            div {
                class: "operon-split-divider",
                style: "cursor: col-resize; background: var(--vscode-editor-background); border-left: 1px solid var(--vscode-panel-border, rgba(128,128,128,0.3));",
                onmousedown: move |_| { dragging.set(true); },
            }
            div { class: "operon-split-pane operon-split-edit-pane",
                style: "overflow: hidden; min-width: 0;",
                {edit_el}
            }
        }
    }
}

fn render_view(plugin: &dyn FormatPlugin, note_id: &str, content: &str) -> Element {
    plugin.render(note_id, content)
}

fn render_edit(
    plugin: &dyn FormatPlugin,
    note_id: &str,
    content: &str,
    on_change: EventHandler<String>,
) -> Element {
    plugin.render_edit(note_id, content, on_change)
}

#[cfg(target_arch = "wasm32")]
fn body_width() -> Option<f64> {
    let win = web_sys::window()?;
    let body = win.document()?.body()?;
    Some(body.client_width() as f64)
}

#[cfg(not(target_arch = "wasm32"))]
fn body_width() -> Option<f64> {
    // Desktop webview: would route through dioxus::eval. v1 desktop falls back to a
    // reasonable default; the splitter is mostly exercised via the web build.
    Some(1200.0)
}
