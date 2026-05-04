//! Drag splitters between regions.
//!
//! Each splitter is a 4 px hit zone. `onmousedown` records the pointer position + the relevant
//! `LayoutState` field's current size into a `Signal<Option<DragState>>`. The Shell root listens
//! for `onmousemove` / `onmouseup` and applies the delta via `LayoutState::set_*_*` (clamped).

use dioxus::prelude::*;

use super::layout::{DragState, LayoutState, SplitterKind};

#[component]
pub fn LeftSplitter() -> Element {
    let layout: Signal<LayoutState> = use_context();
    let mut drag: Signal<Option<DragState>> = use_context();
    rsx! {
        div {
            class: "operon-splitter operon-splitter-vertical",
            "data-edge": "sidebar",
            onmousedown: move |e| {
                let pos = e.client_coordinates().x as i32;
                let size = layout.read().sidebar_width;
                drag.set(Some(DragState { kind: SplitterKind::Left, start_pos: pos, start_size: size }));
                e.prevent_default();
            },
        }
    }
}

#[component]
pub fn RightSplitter() -> Element {
    let layout: Signal<LayoutState> = use_context();
    let mut drag: Signal<Option<DragState>> = use_context();
    rsx! {
        div {
            class: "operon-splitter operon-splitter-vertical",
            "data-edge": "companion",
            onmousedown: move |e| {
                let pos = e.client_coordinates().x as i32;
                let size = layout.read().companion_width;
                drag.set(Some(DragState { kind: SplitterKind::Right, start_pos: pos, start_size: size }));
                e.prevent_default();
            },
        }
    }
}

#[component]
pub fn BottomSplitter() -> Element {
    let layout: Signal<LayoutState> = use_context();
    let mut drag: Signal<Option<DragState>> = use_context();
    rsx! {
        div {
            class: "operon-splitter operon-splitter-horizontal",
            "data-edge": "panel",
            onmousedown: move |e| {
                let pos = e.client_coordinates().y as i32;
                let size = layout.read().panel_height;
                drag.set(Some(DragState { kind: SplitterKind::Bottom, start_pos: pos, start_size: size }));
                e.prevent_default();
            },
        }
    }
}
