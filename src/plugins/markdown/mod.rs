//! `MarkdownNotePlugin` — the first concrete `NotePlugin`.
//!
//! Parses CommonMark via `pulldown-cmark`, walks the events into an [`MdNode`] tree, and
//! renders themed Dioxus RSX. No editing in this seed; raw HTML is dropped; YAML-style
//! frontmatter is hidden.

use dioxus::prelude::*;

use crate::plugin::manifest::NoteKind;
use crate::plugin::{NotePlugin, PluginManifest, PluginSurface};

pub mod nodes;
pub mod parser;
pub mod render;

pub use nodes::MdNode;
pub use render::MarkdownView;

pub struct MarkdownNotePlugin {
    manifest: PluginManifest,
}

impl MarkdownNotePlugin {
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: "markdown-note".into(),
                display_name: "Markdown Note".into(),
                version: "0.1.0".into(),
                note_kind: Some(NoteKind::Markdown),
                surfaces: vec![PluginSurface::MainAreaTabContent],
            },
        }
    }
}

impl Default for MarkdownNotePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl NotePlugin for MarkdownNotePlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn render(&self, _note_id: &str, content: &str) -> Element {
        let content = content.to_string();
        rsx! { MarkdownView { content } }
    }
}
