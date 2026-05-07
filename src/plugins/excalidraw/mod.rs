//! `ExcalidrawFormatPlugin` — freeform pen-and-shape sketching, persisted as
//! JSON. v1 ships a Rust/SVG drawing surface (rectangles + freehand strokes)
//! so users get a working sketcher without vendoring the full Excalidraw JS
//! bundle. The on-disk schema is forward-compatible with a future bundle
//! swap: a top-level `"version": "operon-1"` tag identifies our v1 docs so
//! the upgrade path can transparently convert.

use dioxus::prelude::*;

use crate::plugin::{FormatCaps, FormatPlugin, PluginManifest, PluginSurface};

mod model;
mod view;

pub use model::{ExcalidrawDoc, ExcalidrawElement};

pub struct ExcalidrawFormatPlugin {
    manifest: PluginManifest,
}

impl ExcalidrawFormatPlugin {
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: "excalidraw-note".into(),
                display_name: "Excalidraw".into(),
                version: "0.1.0".into(),
                format_id: Some("excalidraw"),
                extensions: &["excalidraw"],
                surfaces: vec![PluginSurface::MainAreaTabContent],
            },
        }
    }
}

impl Default for ExcalidrawFormatPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatPlugin for ExcalidrawFormatPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn capabilities(&self) -> FormatCaps {
        FormatCaps::VIEW | FormatCaps::EDIT
    }

    fn render(&self, _note_id: &str, content: &str) -> Element {
        let doc = ExcalidrawDoc::parse(content);
        rsx! { view::ExcalidrawView { doc } }
    }

    fn render_edit(
        &self,
        _note_id: &str,
        content: &str,
        on_change: EventHandler<String>,
    ) -> Element {
        let initial = content.to_string();
        rsx! { view::ExcalidrawEditor { initial, on_change } }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_format_id() {
        let p = ExcalidrawFormatPlugin::new();
        assert_eq!(p.manifest().format_id, Some("excalidraw"));
    }
}
