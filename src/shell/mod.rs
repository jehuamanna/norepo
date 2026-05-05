//! VS Code-style Shell layout.
//!
//! [`Shell`] arranges the five canonical regions in a CSS Grid (activity bar, side bar,
//! main area, companion area, status bar) and mounts the [`CommandPalette`] as the last
//! child (modal overlay above all regions). Owns Shell-level keyboard shortcuts:
//! `Ctrl+W` / `Cmd+W` closes the active tab; `Ctrl+B` / `Cmd+B` toggles the side bar;
//! `Ctrl+Shift+P` / `Cmd+Shift+P` opens the palette in commands mode; `Ctrl+P` / `Cmd+P`
//! opens it in notes mode. Tab/SideBar shortcuts are skipped while the palette is open
//! (the palette captures and stops keystroke propagation from its own input).

use std::rc::Rc;

use dioxus::prelude::*;

use crate::commands::{CommandPalette, PaletteMode, PaletteState};
use crate::panel::PanelStrip;
use crate::plugin::{PluginRegistry, PluginSurface};
use crate::shell::layout::{DragState, LayoutState, SplitterKind};
use crate::shell::splitter::{BottomSplitter, LeftSplitter, RightSplitter};
use crate::shell::state::{ActiveActivity, ActivityItemId, LastActiveActivity};
use crate::tabs::TabManager;

mod activity_bar;
mod companion_area;
pub mod dropdown;
pub mod layout;
mod main_area;
pub mod menubar;
mod side_bar;
pub mod splitter;
pub mod state;
mod status_bar;

pub use activity_bar::ActivityBar;
pub use companion_area::CompanionArea;
pub use main_area::MainArea;
pub use menubar::{MenuId, Menubar};
pub use side_bar::SideBar;
pub use status_bar::StatusBar;

#[component]
pub fn Shell() -> Element {
    let mut tabs: Signal<TabManager> = use_context();
    let ActiveActivity(mut active) = use_context();
    let LastActiveActivity(last) = use_context();
    let registry: Rc<PluginRegistry> = use_context();
    let mut palette: Signal<PaletteState> = use_context();
    let mut open_menu: Signal<Option<menubar::MenuId>> = use_context();
    let mut layout: Signal<LayoutState> = use_context();
    let mut drag: Signal<Option<DragState>> = use_context();

    let layout_read = layout.read();
    let layout_style = format!(
        "--operon-side-bar-width: {}px; --operon-companion-width: {}px; --operon-panel-height: {}px;",
        layout_read.sidebar_track(),
        layout_read.companion_track(),
        layout_read.panel_track(),
    );
    let collapsed_attr = if layout_read.sidebar_collapsed { "true" } else { "false" };
    drop(layout_read);

    rsx! {
        div {
            id: "operon-shell",
            class: "operon-shell-grid",
            tabindex: "-1",
            "data-sidebar-collapsed": "{collapsed_attr}",
            style: "{layout_style}",
            onmousemove: move |e| {
                let cur = *drag.read();
                if let Some(d) = cur {
                    let new_size = match d.kind {
                        SplitterKind::Left => {
                            let dx = e.client_coordinates().x as i32 - d.start_pos;
                            (d.start_size as i32 + dx).max(0) as u32
                        }
                        SplitterKind::Right => {
                            let dx = e.client_coordinates().x as i32 - d.start_pos;
                            (d.start_size as i32 - dx).max(0) as u32
                        }
                        SplitterKind::Bottom => {
                            let dy = e.client_coordinates().y as i32 - d.start_pos;
                            (d.start_size as i32 - dy).max(0) as u32
                        }
                    };
                    layout.with_mut(|s| match d.kind {
                        SplitterKind::Left => s.drag_sidebar(new_size),
                        SplitterKind::Right => s.drag_companion(new_size),
                        SplitterKind::Bottom => s.drag_panel(new_size),
                    });
                }
            },
            onmouseup: move |_| {
                if drag.read().is_some() {
                    drag.set(None);
                }
            },
            onkeydown: move |event| {
                let key_str = event.key().to_string();

                // Escape closes any open menubar dropdown — works without modifiers.
                if key_str == "Escape" {
                    if open_menu.read().is_some() {
                        open_menu.set(None);
                        event.prevent_default();
                        return;
                    }
                }

                let mods = event.modifiers();
                let with_meta = mods.contains(Modifiers::META) || mods.contains(Modifiers::CONTROL);
                if !with_meta { return; }
                let palette_open = palette.read().open;

                if key_str.eq_ignore_ascii_case("p") {
                    let mode = if mods.contains(Modifiers::SHIFT) {
                        PaletteMode::Commands
                    } else {
                        PaletteMode::Notes
                    };
                    palette.set(PaletteState {
                        open: true,
                        mode,
                        query: String::new(),
                        selection: 0,
                        themes_original: None,
                        themes_focus_cache: None,
                    });
                    event.prevent_default();
                    return;
                }

                if palette_open { return; }

                if key_str.eq_ignore_ascii_case("w") {
                    let active_id = tabs.read().active_id();
                    if let Some(id) = active_id {
                        tabs.write().close(id);
                        event.prevent_default();
                    }
                } else if key_str.eq_ignore_ascii_case("b") {
                    layout.with_mut(|s| s.toggle_sidebar());
                    if active.read().is_none() {
                        let next = last.read().clone().or_else(|| {
                            registry
                                .contributions(PluginSurface::ActivityBar)
                                .next()
                                .map(|p| ActivityItemId(format!("{}:default", p.manifest().id)))
                        });
                        if let Some(id) = next {
                            active.set(Some(id));
                        }
                    }
                    event.prevent_default();
                }
            },
            Menubar {}
            ActivityBar {}
            SideBar {}
            MainArea {}
            PanelStrip {}
            CompanionArea {}
            StatusBar {}
            LeftSplitter {}
            RightSplitter {}
            BottomSplitter {}
            CommandPalette {}
        }
    }
}
