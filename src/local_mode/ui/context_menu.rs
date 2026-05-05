//! Right-click context menu primitive used by the Local-Mode explorer rows.
//!
//! The caller wires up the `oncontextmenu` handler on the target row, computes
//! a screen position, and renders a [`ContextMenu`] whose `items` it controls.
//! The menu dismisses itself on outside click, Escape, or after an enabled
//! item fires.

use dioxus::prelude::*;

#[derive(Clone, PartialEq)]
pub struct ContextMenuItem {
    pub label: String,
    pub on_click: Callback<()>,
    pub enabled: bool,
}

impl ContextMenuItem {
    pub fn new(label: impl Into<String>, on_click: Callback<()>) -> Self {
        Self {
            label: label.into(),
            on_click,
            enabled: true,
        }
    }

    pub fn disabled(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            on_click: Callback::new(|_| {}),
            enabled: false,
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct ContextMenuProps {
    /// Pixel x position relative to the viewport (clientX).
    pub x: i32,
    /// Pixel y position relative to the viewport (clientY).
    pub y: i32,
    pub items: Vec<ContextMenuItem>,
    pub on_dismiss: Callback<()>,
}

#[component]
pub fn ContextMenu(props: ContextMenuProps) -> Element {
    let style = format!("position: fixed; left: {}px; top: {}px;", props.x, props.y);
    let items = props.items.clone();
    let on_dismiss = props.on_dismiss;

    rsx! {
        // Full-viewport scrim catches outside clicks.
        div {
            class: "fixed inset-0 z-50",
            "data-testid": "context-menu-scrim",
            onclick: move |evt| {
                evt.stop_propagation();
                on_dismiss.call(());
            },
            oncontextmenu: move |evt| {
                evt.prevent_default();
                evt.stop_propagation();
                on_dismiss.call(());
            },
            onkeydown: move |evt| {
                if evt.key().to_string() == "Escape" {
                    evt.prevent_default();
                    on_dismiss.call(());
                }
            },
            div {
                class: "operon-context-menu",
                style: "{style}",
                "data-testid": "context-menu",
                tabindex: "-1",
                onclick: move |evt| evt.stop_propagation(),
                for (idx, item) in items.into_iter().enumerate() {
                    ContextMenuRow {
                        key: "{idx}",
                        item: item,
                        on_dismiss: on_dismiss,
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct ContextMenuRowProps {
    item: ContextMenuItem,
    on_dismiss: Callback<()>,
}

#[component]
fn ContextMenuRow(props: ContextMenuRowProps) -> Element {
    let label = props.item.label.clone();
    let testid = format!(
        "context-menu-item-{}",
        label.to_lowercase().replace(' ', "-")
    );
    let enabled = props.item.enabled;
    let on_click = props.item.on_click;
    let on_dismiss = props.on_dismiss;

    let class_attr = if enabled {
        "operon-context-menu-row"
    } else {
        "operon-context-menu-row operon-context-menu-row-disabled"
    };

    rsx! {
        button {
            r#type: "button",
            class: "{class_attr}",
            "data-testid": "{testid}",
            disabled: !enabled,
            onclick: move |evt| {
                evt.stop_propagation();
                if enabled {
                    on_click.call(());
                    on_dismiss.call(());
                }
            },
            "{label}"
        }
    }
}
