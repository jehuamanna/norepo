//! `CanvasFormatPlugin` — spatial pinboard. Adopts the Obsidian Canvas JSON
//! schema (`{"nodes":[…], "edges":[…]}`) for interop. v1 supports text nodes
//! with pan / drag and persists edges round-trip but doesn't render them as
//! arrows yet — a Phase-6 follow-up.

use dioxus::prelude::*;

use crate::plugin::{FormatCaps, FormatPlugin, PluginManifest, PluginSurface};

mod model;
mod view;

pub use model::{CanvasDoc, CanvasEdge, CanvasNode};

pub struct CanvasFormatPlugin {
    manifest: PluginManifest,
}

impl CanvasFormatPlugin {
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: "canvas-note".into(),
                display_name: "Canvas".into(),
                version: "0.1.0".into(),
                format_id: Some("canvas"),
                extensions: &["canvas"],
                surfaces: vec![PluginSurface::MainAreaTabContent],
            },
        }
    }
}

impl Default for CanvasFormatPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatPlugin for CanvasFormatPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn capabilities(&self) -> FormatCaps {
        FormatCaps::VIEW | FormatCaps::EDIT
    }

    fn render(&self, _note_id: &str, content: &str) -> Element {
        let doc = CanvasDoc::parse(content);
        rsx! { view::CanvasView { doc } }
    }

    fn render_edit(
        &self,
        _note_id: &str,
        content: &str,
        on_change: EventHandler<String>,
    ) -> Element {
        let initial = content.to_string();
        rsx! { view::CanvasEditor { initial, on_change } }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_format_id() {
        let p = CanvasFormatPlugin::new();
        assert_eq!(p.manifest().format_id, Some("canvas"));
    }
}
