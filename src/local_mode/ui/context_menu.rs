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
    // Render initially hidden so the post-mount JS adjustment doesn't
    // produce a one-frame flash at the unadjusted position. The
    // `onmounted` handler runs `clamp_into_viewport_script` which
    // measures the rendered menu against `window.innerWidth/Height`,
    // moves it up / left whenever it would overflow, and finally
    // flips visibility back to visible.
    let style = format!(
        "position: fixed; left: {}px; top: {}px; visibility: hidden;",
        props.x, props.y
    );
    let items = props.items.clone();
    let on_dismiss = props.on_dismiss;
    // Tracks which row index in this menu currently has its submenu open.
    // Lifting this out of `ContextMenuRow` enforces mutual exclusion: hovering
    // a sibling row writes a different index here, which auto-closes the
    // previously open submenu. Without this, two submenus could be visible at
    // once because each row owned a private `bool` that no one cleared.
    let open_submenu: Signal<Option<usize>> = use_signal(|| None);

    rsx! {
        // Full-viewport scrim catches outside clicks. z-index sits above the
        // shell splitters (z=50 in shell.css) so hovering the splitter region
        // while the menu is open hits the scrim — preventing the splitter's
        // hover highlight from firing through. The menu itself (z=60) and
        // submenu (z=61) still paint above this scrim within its stacking
        // context.
        div {
            class: "fixed inset-0",
            style: "z-index: 55;",
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
                onmounted: move |_| {
                    let _ = dioxus::prelude::document::eval(
                        &clamp_into_viewport_script("[data-testid=\"context-menu\"]")
                    );
                },
                onclick: move |evt| evt.stop_propagation(),
                for (idx, item) in items.into_iter().enumerate() {
                    ContextMenuRow {
                        key: "{idx}",
                        idx: idx,
                        item: item,
                        on_dismiss: on_dismiss,
                        open_submenu: open_submenu,
                    }
                }
            }
        }
    }
}

/// JS one-shot that pins a fixed-positioned context menu inside the
/// viewport. Measures the matched element with
/// `getBoundingClientRect`, shifts it up / left whenever it would
/// overflow `window.innerWidth/Height`, leaves an 8px margin from the
/// edge, and unhides the element. Called from each menu component's
/// `onmounted` so the very first paint shows the menu in the final
/// (adjusted) position rather than flashing at the cursor coords
/// before the post-render correction lands.
///
/// Pure-function so we can unit-test the script template without a
/// Dioxus runtime; the actual execution happens via
/// `document::eval`.
pub fn clamp_into_viewport_script(selector: &str) -> String {
    // 8px is the breathing-room gap we want between the menu's edge
    // and the viewport edge. Tweaked alongside the menu's CSS shadow
    // — small enough that "near the edge" feels natural, large
    // enough that no shadow gets clipped.
    const EDGE_PAD: i32 = 8;
    format!(
        r#"(() => {{
            const el = document.querySelector('{selector}');
            if (!el) return;
            const rect = el.getBoundingClientRect();
            const vw = window.innerWidth;
            const vh = window.innerHeight;
            let top = rect.top;
            let left = rect.left;
            if (rect.bottom > vh) {{
                top = Math.max({pad}, vh - rect.height - {pad});
            }}
            if (top < {pad}) {{
                top = {pad};
            }}
            if (rect.right > vw) {{
                left = Math.max({pad}, vw - rect.width - {pad});
            }}
            if (left < {pad}) {{
                left = {pad};
            }}
            el.style.top = top + 'px';
            el.style.left = left + 'px';
            el.style.visibility = 'visible';
        }})();"#,
        selector = selector,
        pad = EDGE_PAD,
    )
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
    // Same hidden-until-clamped pattern as the parent ContextMenu so a
    // deep submenu near the screen edge doesn't flash at its anchor
    // point before flipping into the viewport.
    let style = format!(
        "position: fixed; left: {}px; top: {}px; visibility: hidden;",
        props.anchor_x, props.anchor_y
    );
    let items = props.items.clone();
    let on_dismiss = props.on_dismiss;
    // Each SubMenu owns its own active-child tracker so a deeper nested
    // submenu (if a leaf were ever promoted to `submenu(...)`) wouldn't
    // collide with the parent menu's tracker.
    let open_submenu: Signal<Option<usize>> = use_signal(|| None);
    rsx! {
        div {
            class: "operon-context-menu operon-context-submenu",
            style: "{style}",
            "data-testid": "context-submenu",
            tabindex: "-1",
            onmounted: move |_| {
                let _ = dioxus::prelude::document::eval(
                    &clamp_into_viewport_script("[data-testid=\"context-submenu\"]")
                );
            },
            onclick: move |evt| evt.stop_propagation(),
            for (idx, item) in items.into_iter().enumerate() {
                ContextMenuRow {
                    key: "{idx}",
                    idx: idx,
                    item: item,
                    on_dismiss: on_dismiss,
                    open_submenu: open_submenu,
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct ContextMenuRowProps {
    idx: usize,
    item: ContextMenuItem,
    on_dismiss: Callback<()>,
    /// Parent-owned signal: row index whose submenu is currently open within
    /// this menu, or `None`. The row is "active" iff this matches `idx`.
    open_submenu: Signal<Option<usize>>,
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
    let my_idx = props.idx;
    let mut open_submenu = props.open_submenu;
    let is_open = *open_submenu.read() == Some(my_idx);

    let mut class_attr = String::from("operon-context-menu-row");
    if !enabled {
        class_attr.push_str(" operon-context-menu-row-disabled");
    }
    if has_children {
        class_attr.push_str(" operon-context-menu-row-submenu");
    }

    // Anchor coordinates the SubMenu positions against — the row's
    // top-right corner (in viewport space). Captured in `onmounted` so
    // keyboard-only submenu reveal (`ArrowRight`) lands correctly.
    let mut sub_anchor: Signal<(i32, i32)> = use_signal(|| (0, 0));

    let testid_for_submenu = format!("{testid}-submenu");
    let chevron = if has_children { " \u{25B8}" } else { "" };

    // Capture the row's right-edge / top. On wasm we read the rect
    // synchronously off the DOM node; on desktop we fall back to the
    // async `get_client_rect()` so the anchor lands on the row regardless
    // of where the cursor entered. Without this, the desktop path used
    // raw cursor coords + 140px and the submenu drifted "far away" from
    // its parent menu when the user hovered the row's left edge.
    let capture_anchor = move |evt: Event<MountedData>| {
        #[cfg(target_arch = "wasm32")]
        {
            use dioxus::web::WebEventExt;
            if let Some(node) = evt.data().try_as_web_event() {
                let rect = node.get_bounding_client_rect();
                sub_anchor.set((rect.right() as i32, rect.top() as i32));
                return;
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mounted = evt.data();
            spawn(async move {
                if let Ok(rect) = mounted.get_client_rect().await {
                    sub_anchor.set((rect.max_x() as i32, rect.min_y() as i32));
                }
            });
        }
    };

    rsx! {
        button {
            r#type: "button",
            class: "{class_attr}",
            "data-testid": if has_children { testid_for_submenu.clone() } else { testid.clone() },
            "aria-haspopup": if has_children { "menu" } else { "false" },
            "aria-expanded": if has_children { if is_open { "true" } else { "false" } } else { "false" },
            disabled: !enabled,
            onmounted: capture_anchor,
            onmouseenter: move |_| {
                if !enabled { return; }
                if has_children {
                    open_submenu.set(Some(my_idx));
                } else if open_submenu.read().is_some() {
                    // Hovering a non-submenu sibling closes any open submenu.
                    open_submenu.set(None);
                }
            },
            onkeydown: move |evt| {
                let key = evt.key().to_string();
                if has_children && key == "ArrowRight" {
                    evt.prevent_default();
                    open_submenu.set(Some(my_idx));
                } else if key == "ArrowLeft" && is_open {
                    evt.prevent_default();
                    open_submenu.set(None);
                } else if key == "Escape" {
                    evt.prevent_default();
                    if is_open {
                        open_submenu.set(None);
                    } else {
                        on_dismiss.call(());
                    }
                }
            },
            onclick: move |evt| {
                evt.stop_propagation();
                if !enabled { return; }
                if has_children {
                    if is_open {
                        open_submenu.set(None);
                    } else {
                        open_submenu.set(Some(my_idx));
                    }
                } else {
                    on_click.call(());
                    on_dismiss.call(());
                }
            },
            "{label}{chevron}"
        }
        if has_children && is_open {
            SubMenu {
                items: children,
                anchor_x: sub_anchor.read().0,
                anchor_y: sub_anchor.read().1,
                on_dismiss: on_dismiss,
            }
        }
    }
}

// NB: Constructor unit tests for ContextMenuItem live in `tests-wasm/`
// because `Callback::new` requires a Dioxus runtime — Phase-1 TestCase U-1
// is exercised there alongside the Playwright submenu reveal spec.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_script_includes_selector_and_viewport_checks() {
        let script = clamp_into_viewport_script("[data-testid=\"ctx\"]");
        // Selector wired through verbatim.
        assert!(script.contains("[data-testid=\"ctx\"]"));
        // Reads viewport dimensions both ways.
        assert!(script.contains("window.innerWidth"));
        assert!(script.contains("window.innerHeight"));
        // Checks both overflow directions.
        assert!(script.contains("rect.bottom > vh"));
        assert!(script.contains("rect.right > vw"));
        // Always unhides at the end (so the visibility:hidden seed
        // style we set in the component doesn't leave the menu
        // permanently invisible if the clamp branches all skip).
        assert!(script.contains("visibility = 'visible'"));
    }

    #[test]
    fn clamp_script_pads_against_top_left_edge() {
        // Edge case: an open submenu near the top-left would have
        // its `top`/`left` driven negative by the overflow branches
        // without a lower-bound clamp. We guard against that with
        // explicit `< pad` checks; verify those are present.
        let script = clamp_into_viewport_script("anything");
        assert!(script.contains("top < 8"));
        assert!(script.contains("left < 8"));
    }
}
