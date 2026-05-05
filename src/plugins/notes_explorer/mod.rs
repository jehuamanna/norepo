//! NotesExplorer — the first built-in `UIPlugin`.
//!
//! Contributes both an [`PluginSurface::ActivityBar`] icon and a [`PluginSurface::SideBarPanel`]
//! list of in-memory sample notes. Clicking a row opens the note as a tab via the
//! context-provided [`crate::tabs::TabManager`].

use dioxus::prelude::*;

use crate::plugin::{PluginManifest, PluginSurface, UIPlugin};
use crate::ui::Icon;

pub mod samples;
pub mod view;

pub use view::NotesExplorerPanel;

pub struct NotesExplorer {
    manifest: PluginManifest,
}

impl NotesExplorer {
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: "notes-explorer".into(),
                display_name: "Notes Explorer".into(),
                version: "0.1.0".into(),
                note_kind: None,
                surfaces: vec![
                    PluginSurface::ActivityBar,
                    PluginSurface::SideBarPanel,
                ],
            },
        }
    }
}

impl Default for NotesExplorer {
    fn default() -> Self {
        Self::new()
    }
}

impl UIPlugin for NotesExplorer {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn render(&self, surface: PluginSurface) -> Element {
        match surface {
            PluginSurface::ActivityBar => rsx! {
                div {
                    class: "operon-activity-icon",
                    title: "Notes Explorer",
                    style: "display: flex; align-items: center; justify-content: center; width: 100%; height: 100%;",
                    Icon { name: "book".to_string(), size: 20, title: "Notes Explorer".to_string() }
                }
            },
            PluginSurface::SideBarPanel => rsx! { NotesExplorerPanel {} },
            _ => rsx! {},
        }
    }
}
