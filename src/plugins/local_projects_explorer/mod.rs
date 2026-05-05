//! LocalProjectsExplorer — the ActivityBar + SideBarPanel plugin used in Local Mode.
//!
//! Mirrors `crate::plugins::notes_explorer::NotesExplorer` so Local Mode reuses the
//! same Cloud `Shell` chrome. The SideBarPanel render mounts the existing
//! `crate::local_mode::explorer::ExplorerPanel`, which carries every Phase 2–5
//! Local-Mode behavior (project + note CRUD, tree state, drag/drop, search, etc.).

use dioxus::prelude::*;

use crate::plugin::{PluginManifest, PluginSurface, UIPlugin};
use crate::ui::Icon;

pub mod view;

pub use view::LocalProjectsExplorerPanel;

pub struct LocalProjectsExplorer {
    manifest: PluginManifest,
}

impl LocalProjectsExplorer {
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: "local-projects-explorer".into(),
                display_name: "Local Projects".into(),
                version: "0.1.0".into(),
                format_id: None,
                extensions: &[],
                surfaces: vec![PluginSurface::ActivityBar, PluginSurface::SideBarPanel],
            },
        }
    }
}

impl Default for LocalProjectsExplorer {
    fn default() -> Self {
        Self::new()
    }
}

impl UIPlugin for LocalProjectsExplorer {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn render(&self, surface: PluginSurface) -> Element {
        match surface {
            PluginSurface::ActivityBar => rsx! {
                div {
                    class: "operon-activity-icon",
                    title: "Local Projects",
                    style: "display: flex; align-items: center; justify-content: center; width: 100%; height: 100%;",
                    Icon { name: "folder".to_string(), size: 20 }
                }
            },
            PluginSurface::SideBarPanel => rsx! { LocalProjectsExplorerPanel {} },
            _ => rsx! {},
        }
    }
}
