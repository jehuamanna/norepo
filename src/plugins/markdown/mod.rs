//! `MarkdownFormatPlugin` — the first concrete `FormatPlugin`.
//!
//! Parses CommonMark via `pulldown-cmark`, walks the events into an [`MdNode`] tree, and
//! renders themed Dioxus RSX. No editing in this seed; raw HTML is dropped; YAML-style
//! frontmatter is hidden.

use dioxus::prelude::*;

use crate::plugin::{FormatPlugin, PluginManifest, PluginSurface};

pub mod nodes;
pub mod parser;
pub mod render;

pub use nodes::MdNode;
pub use render::MarkdownView;

pub struct MarkdownFormatPlugin {
    manifest: PluginManifest,
}

impl MarkdownFormatPlugin {
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: "markdown-note".into(),
                display_name: "Markdown Note".into(),
                version: "0.1.0".into(),
                format_id: Some("markdown"),
                extensions: &["md", "markdown"],
                surfaces: vec![PluginSurface::MainAreaTabContent],
            },
        }
    }
}

impl Default for MarkdownFormatPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatPlugin for MarkdownFormatPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn render(&self, _note_id: &str, content: &str) -> Element {
        let content = content.to_string();
        rsx! { MarkdownView { content } }
    }
}
