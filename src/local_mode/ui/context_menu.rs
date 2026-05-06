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
    /// When non-empty, this item is a submenu anchor: hovering or activating it
    /// reveals a nested [`ContextMenu`] containing these children. The item's
    /// own `on_click` is ignored while `children` is populated; activation goes
    /// through the leaf rows of the nested menu.
    pub children: Vec<ContextMenuItem>,
}

impl ContextMenuItem {
    pub fn new(label: impl Into<String>, on_click: Callback<()>) -> Self {
        Self {
            label: label.into(),
            on_click,
            enabled: true,
            children: Vec::new(),
        }
    }

    pub fn disabled(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            on_click: Callback::new(|_| {}),
            enabled: false,
            children: Vec::new(),
        }
    }

    /// Build a submenu anchor — its own `on_click` is a no-op; activation
    /// flows through the supplied `children` leaves. Caller passes a label
    /// (e.g. `"Add child note"`) and the leaves (e.g. `Markdown`, `Image`).
    pub fn submenu(label: impl Into<String>, children: Vec<ContextMenuItem>) -> Self {
        Self {
            label: label.into(),
            on_click: Callback::new(|_| {}),
            enabled: true,
            children,
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

/// Render a nested submenu floating at the right edge of the parent row.
/// The component owns nothing else — its children list is read from the
/// parent's `ContextMenuItem::children` and dismissal is propagated upward.
#[derive(Props, Clone, PartialEq)]
struct SubMenuProps {
    items: Vec<ContextMenuItem>,
    /// Pixel position of the parent row's right edge (clientX, clientY).
    anchor_x: i32,
    anchor_y: i32,
    on_dismiss: Callback<()>,
}

#[component]
fn SubMenu(props: SubMenuProps) -> Element {
    let style = format!(
        "position: fixed; left: {}px; top: {}px;",
        props.anchor_x, props.anchor_y
    );
    let items = props.items.clone();
    let on_dismiss = props.on_dismiss;
    rsx! {
        div {
            class: "operon-context-menu operon-context-submenu",
            style: "{style}",
            "data-testid": "context-submenu",
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
    let children = props.item.children.clone();
    let has_children = !children.is_empty();

    let mut class_attr = String::from("operon-context-menu-row");
    if !enabled {
        class_attr.push_str(" operon-context-menu-row-disabled");
    }
    if has_children {
        class_attr.push_str(" operon-context-menu-row-submenu");
    }

    // Submenu open state + the anchor coordinates we'll position against.
    let mut sub_open: Signal<bool> = use_signal(|| false);
    let mut sub_anchor: Signal<(i32, i32)> = use_signal(|| (0, 0));

    let testid_for_submenu = format!("{testid}-submenu");
    let chevron = if has_children { " \u{25B8}" } else { "" };

    rsx! {
        button {
            r#type: "button",
            class: "{class_attr}",
            "data-testid": if has_children { testid_for_submenu.clone() } else { testid.clone() },
            "aria-haspopup": if has_children { "menu" } else { "false" },
            "aria-expanded": if has_children { if *sub_open.read() { "true" } else { "false" } } else { "false" },
            disabled: !enabled,
            onmouseenter: move |evt| {
                if has_children && enabled {
                    let coords = evt.client_coordinates();
                    sub_anchor.set((coords.x as i32 + 140, coords.y as i32 - 4));
                    sub_open.set(true);
                }
            },
            onkeydown: move |evt| {
                let key = evt.key().to_string();
                if has_children && key == "ArrowRight" {
                    evt.prevent_default();
                    sub_open.set(true);
                } else if key == "ArrowLeft" && *sub_open.read() {
                    evt.prevent_default();
                    sub_open.set(false);
                } else if key == "Escape" {
                    evt.prevent_default();
                    if *sub_open.read() {
                        sub_open.set(false);
                    } else {
                        on_dismiss.call(());
                    }
                }
            },
            onclick: move |evt| {
                evt.stop_propagation();
                if !enabled { return; }
                if has_children {
                    let coords = evt.client_coordinates();
                    sub_anchor.set((coords.x as i32 + 140, coords.y as i32 - 4));
                    let was_open = *sub_open.read();
                    sub_open.set(!was_open);
                } else {
                    on_click.call(());
                    on_dismiss.call(());
                }
            },
            "{label}{chevron}"
        }
        if has_children && *sub_open.read() {
            SubMenu {
                items: children,
                anchor_x: sub_anchor.read().0,
                anchor_y: sub_anchor.read().1,
                on_dismiss: on_dismiss,
            }
        }
    }
}
