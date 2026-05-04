//! VS Code-style Shell layout.
//!
//! [`Shell`] arranges the five canonical regions in a CSS Grid (activity bar, side bar,
//! main area, companion area, status bar). It also owns the global keyboard shortcuts
//! relevant to the Shell-level state (e.g. `Ctrl+W` / `Cmd+W` to close the active tab).

use dioxus::prelude::*;

use crate::tabs::TabManager;

mod activity_bar;
mod companion_area;
mod main_area;
mod side_bar;
mod status_bar;

pub use activity_bar::ActivityBar;
pub use companion_area::CompanionArea;
pub use main_area::MainArea;
pub use side_bar::SideBar;
pub use status_bar::StatusBar;

#[component]
pub fn Shell() -> Element {
    let mut tabs: Signal<TabManager> = use_context();

    rsx! {
        div {
            id: "operon-shell",
            class: "operon-shell-grid",
            tabindex: "-1",
            onkeydown: move |event| {
                let mods = event.modifiers();
                let with_meta = mods.contains(Modifiers::META) || mods.contains(Modifiers::CONTROL);
                if !with_meta { return; }
                let key_str = event.key().to_string();
                if key_str.eq_ignore_ascii_case("w") {
                    let active = tabs.read().active_id();
                    if let Some(id) = active {
                        tabs.write().close(id);
                        event.prevent_default();
                    }
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
