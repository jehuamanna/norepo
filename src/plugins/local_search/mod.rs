//! LocalSearch — dedicated full-text search activity panel for Local Mode.
//!
//! Mirrors VS Code's separate "Search" view: an activity-bar magnifier swaps the
//! sidebar to a search input + results list grouped by note (file → matched
//! lines). Body content is cached lazily on first mount so the cost is paid
//! once per session rather than on every keystroke.

use dioxus::prelude::*;

use crate::plugin::{PluginManifest, PluginSurface, UIPlugin};
use crate::ui::Icon;

pub mod view;

pub use view::{LocalSearchFocus, LocalSearchPanel};

pub struct LocalSearch {
    manifest: PluginManifest,
}

impl LocalSearch {
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: "local-search".into(),
                display_name: "Search".into(),
                version: "0.1.0".into(),
                format_id: None,
                extensions: &[],
                surfaces: vec![PluginSurface::ActivityBar, PluginSurface::SideBarPanel],
            },
        }
    }
}

impl Default for LocalSearch {
    fn default() -> Self {
        Self::new()
    }
}

impl UIPlugin for LocalSearch {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn render(&self, surface: PluginSurface) -> Element {
        match surface {
            PluginSurface::ActivityBar => rsx! {
                div {
                    class: "operon-activity-icon",
                    title: "Search",
                    style: "display: flex; align-items: center; justify-content: center; width: 100%; height: 100%;",
                    Icon { name: "search".to_string(), size: 20 }
                }
            },
            PluginSurface::SideBarPanel => rsx! { LocalSearchPanel {} },
            _ => rsx! {},
        }
    }
}
