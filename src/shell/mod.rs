//! VS Code-style Shell layout.
//!
//! [`Shell`] arranges the five canonical regions in a CSS Grid: activity bar, side bar,
//! main area, companion area, and the bottom-spanning status bar. The theme is provided
//! upstream via [`crate::theme::ThemeSignal`] context; regions consume token values via
//! CSS custom properties rendered by the application root.

use dioxus::prelude::*;

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
    rsx! {
        div { id: "operon-shell", class: "operon-shell-grid",
            ActivityBar {}
            SideBar {}
            MainArea {}
            CompanionArea {}
            StatusBar {}
        }
    }
}
