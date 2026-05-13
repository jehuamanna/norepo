//! Drag splitters between regions.
//!
//! Each splitter is a 4 px hit zone. `onmousedown` records the pointer position + the relevant
//! `LayoutState` field's current size into a `Signal<Option<DragState>>`. The Shell root listens
//! for `onmousemove` / `onmouseup` and applies the delta via `LayoutState::set_*_*` (clamped).
//!
//! Accessibility: each splitter exposes `role="separator"` and is keyboard-actionable
//! (`tabindex=0`). ArrowLeft/ArrowRight resize vertical splitters, ArrowUp/ArrowDown resize
//! the horizontal one; Enter or Space toggle the adjacent region's collapse flag so a
//! keyboard user can hide / show the side bar, companion, or panel without grabbing the
//! mouse. Step is 16 px; Shift multiplies by 4 for coarse adjustments.

use dioxus::prelude::*;

use super::layout::{DragState, LayoutState, SplitterKind};

const STEP: i32 = 16;
const STEP_FAST: i32 = 64;

#[component]
pub fn LeftSplitter() -> Element {
    let mut layout: Signal<LayoutState> = use_context();
    let mut drag: Signal<Option<DragState>> = use_context();
    let cur = layout.read().sidebar_width as i32;
    rsx! {
        div {
            class: "operon-splitter operon-splitter-vertical",
            "data-edge": "sidebar",
            role: "separator",
            "aria-orientation": "vertical",
            "aria-label": "Resize side bar",
            "aria-valuenow": "{cur}",
            tabindex: "0",
            onmousedown: move |e| {
                let pos = e.client_coordinates().x as i32;
                let size = layout.read().sidebar_width;
                drag.set(Some(DragState { kind: SplitterKind::Left, start_pos: pos, start_size: size }));
                e.prevent_default();
            },
            onkeydown: move |e| {
                let key = e.key().to_string();
                let mods = e.modifiers();
                let step = if mods.contains(keyboard_types::Modifiers::SHIFT) { STEP_FAST } else { STEP };
                if key == "ArrowLeft" {
                    e.prevent_default();
                    let next = (layout.read().sidebar_width as i32 - step).max(0) as u32;
                    layout.with_mut(|s| s.drag_sidebar(next));
                } else if key == "ArrowRight" {
                    e.prevent_default();
                    let next = (layout.read().sidebar_width as i32 + step).max(0) as u32;
                    layout.with_mut(|s| s.drag_sidebar(next));
                } else if key == "Enter" || key == " " {
                    e.prevent_default();
                    layout.with_mut(|s| s.toggle_sidebar());
                }
            },
        }
    }
}

#[component]
pub fn RightSplitter() -> Element {
    let mut layout: Signal<LayoutState> = use_context();
    let mut drag: Signal<Option<DragState>> = use_context();
    let cur = layout.read().companion_width as i32;
    rsx! {
        div {
            class: "operon-splitter operon-splitter-vertical",
            "data-edge": "companion",
            role: "separator",
            "aria-orientation": "vertical",
            "aria-label": "Resize companion panel",
            "aria-valuenow": "{cur}",
            tabindex: "0",
            onmousedown: move |e| {
                let pos = e.client_coordinates().x as i32;
                let size = layout.read().companion_width;
                drag.set(Some(DragState { kind: SplitterKind::Right, start_pos: pos, start_size: size }));
                e.prevent_default();
            },
            onkeydown: move |e| {
                let key = e.key().to_string();
                let mods = e.modifiers();
                let step = if mods.contains(keyboard_types::Modifiers::SHIFT) { STEP_FAST } else { STEP };
                if key == "ArrowRight" {
                    e.prevent_default();
                    let next = (layout.read().companion_width as i32 - step).max(0) as u32;
                    layout.with_mut(|s| s.drag_companion(next));
                } else if key == "ArrowLeft" {
                    e.prevent_default();
                    let next = (layout.read().companion_width as i32 + step).max(0) as u32;
                    layout.with_mut(|s| s.drag_companion(next));
                } else if key == "Enter" || key == " " {
                    e.prevent_default();
                    layout.with_mut(|s| s.toggle_companion());
                }
            },
        }
    }
}

#[component]
pub fn RailSplitter() -> Element {
    let mut layout: Signal<LayoutState> = use_context();
    let mut drag: Signal<Option<DragState>> = use_context();
    let cur = layout.read().rail_width as i32;
    rsx! {
        div {
            class: "operon-splitter operon-splitter-vertical operon-splitter-inline",
            "data-edge": "rail",
            role: "separator",
            "aria-orientation": "vertical",
            "aria-label": "Resize chat session rail",
            "aria-valuenow": "{cur}",
            tabindex: "0",
            onmousedown: move |e| {
                let pos = e.client_coordinates().x as i32;
                let size = layout.read().rail_width;
                drag.set(Some(DragState { kind: SplitterKind::Rail, start_pos: pos, start_size: size }));
                e.prevent_default();
            },
            onkeydown: move |e| {
                let key = e.key().to_string();
                let mods = e.modifiers();
                let step = if mods.contains(keyboard_types::Modifiers::SHIFT) { STEP_FAST } else { STEP };
                if key == "ArrowLeft" {
                    e.prevent_default();
                    let next = (layout.read().rail_width as i32 - step).max(0) as u32;
                    layout.with_mut(|s| s.drag_rail(next));
                } else if key == "ArrowRight" {
                    e.prevent_default();
                    let next = (layout.read().rail_width as i32 + step).max(0) as u32;
                    layout.with_mut(|s| s.drag_rail(next));
                }
            },
        }
    }
}

#[component]
pub fn BottomSplitter() -> Element {
    let mut layout: Signal<LayoutState> = use_context();
    let mut drag: Signal<Option<DragState>> = use_context();
    let cur = layout.read().panel_height as i32;
    rsx! {
        div {
            class: "operon-splitter operon-splitter-horizontal",
            "data-edge": "panel",
            role: "separator",
            "aria-orientation": "horizontal",
            "aria-label": "Resize bottom panel",
            "aria-valuenow": "{cur}",
            tabindex: "0",
            onmousedown: move |e| {
                let pos = e.client_coordinates().y as i32;
                let size = layout.read().panel_height;
                drag.set(Some(DragState { kind: SplitterKind::Bottom, start_pos: pos, start_size: size }));
                e.prevent_default();
            },
            onkeydown: move |e| {
                let key = e.key().to_string();
                let mods = e.modifiers();
                let step = if mods.contains(keyboard_types::Modifiers::SHIFT) { STEP_FAST } else { STEP };
                if key == "ArrowDown" {
                    e.prevent_default();
                    let next = (layout.read().panel_height as i32 - step).max(0) as u32;
                    layout.with_mut(|s| s.drag_panel(next));
                } else if key == "ArrowUp" {
                    e.prevent_default();
                    let next = (layout.read().panel_height as i32 + step).max(0) as u32;
                    layout.with_mut(|s| s.drag_panel(next));
                } else if key == "Enter" || key == " " {
                    e.prevent_default();
                    layout.with_mut(|s| s.toggle_panel());
                }
            },
        }
    }
}
