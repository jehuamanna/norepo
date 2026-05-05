//! Side-bar panel for [`super::LocalProjectsExplorer`]. Wraps the Phase-5 Local
//! `ExplorerPanel` in the same heading + container chrome as
//! `crate::plugins::notes_explorer::view::NotesExplorerPanel` so both look
//! identical inside the Cloud `Shell` SideBar.

use dioxus::prelude::*;

use crate::local_mode::ExplorerPanel;

#[component]
pub fn LocalProjectsExplorerPanel() -> Element {
    rsx! {
        div {
            class: "notes-explorer-panel",
            "data-testid": "local-projects-explorer-panel",
            div {
                class: "notes-explorer-heading",
                style: "font-size: 11px; text-transform: uppercase; letter-spacing: 0.05em; padding: 0 0 6px 0; opacity: 0.7;",
                "Local Projects"
            }
            ExplorerPanel {}
        }
    }
}
