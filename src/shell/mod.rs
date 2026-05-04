//! VS Code-style Shell layout.
//!
//! [`Shell`] arranges the five canonical regions in a CSS Grid (activity bar, side bar,
//! main area, companion area, status bar). It owns Shell-level keyboard shortcuts:
//! `Ctrl+W` / `Cmd+W` closes the active tab; `Ctrl+B` / `Cmd+B` toggles the side bar
//! (collapses if open, restores last-active panel — or the first contributed activity
//! item if there was none — if closed).

use std::rc::Rc;

use dioxus::prelude::*;

use crate::plugin::{PluginRegistry, PluginSurface};
use crate::shell::state::{ActiveActivity, ActivityItemId, LastActiveActivity};
use crate::tabs::TabManager;

mod activity_bar;
mod companion_area;
mod main_area;
mod side_bar;
pub mod state;
mod status_bar;

pub use activity_bar::ActivityBar;
pub use companion_area::CompanionArea;
pub use main_area::MainArea;
pub use side_bar::SideBar;
pub use status_bar::StatusBar;

#[component]
pub fn Shell() -> Element {
    let mut tabs: Signal<TabManager> = use_context();
    let ActiveActivity(mut active) = use_context();
    let LastActiveActivity(mut last) = use_context();
    let registry: Rc<PluginRegistry> = use_context();

    let collapsed = active.read().is_none();
    let collapsed_attr = if collapsed { "true" } else { "false" };
    let extra_style = if collapsed {
        "--operon-side-bar-width: 0;"
    } else {
        ""
    };

    rsx! {
        div {
            id: "operon-shell",
            class: "operon-shell-grid",
            tabindex: "-1",
            "data-sidebar-collapsed": "{collapsed_attr}",
            style: "{extra_style}",
            onkeydown: move |event| {
                let mods = event.modifiers();
                let with_meta = mods.contains(Modifiers::META) || mods.contains(Modifiers::CONTROL);
                if !with_meta { return; }
                let key_str = event.key().to_string();
                if key_str.eq_ignore_ascii_case("w") {
                    let active_id = tabs.read().active_id();
                    if let Some(id) = active_id {
                        tabs.write().close(id);
                        event.prevent_default();
                    }
                } else if key_str.eq_ignore_ascii_case("b") {
                    let cur = active.read().clone();
                    if cur.is_some() {
                        last.set(cur);
                        active.set(None);
                    } else {
                        let to_restore = last.read().clone();
                        let next = to_restore.or_else(|| {
                            registry
                                .contributions(PluginSurface::ActivityBar)
                                .next()
                                .map(|p| ActivityItemId(format!("{}:default", p.manifest().id)))
                        });
                        active.set(next);
                    }
                    event.prevent_default();
                }
            },
            ActivityBar {}
            SideBar {}
            MainArea {}
            CompanionArea {}
            StatusBar {}
        }
    }
}
