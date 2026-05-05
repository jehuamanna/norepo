//! `MarkdownFormatPlugin` — the first concrete `FormatPlugin`.
//!
//! Parses CommonMark via `pulldown-cmark`, walks the events into an [`MdNode`] tree, and
//! renders themed Dioxus RSX in View mode. Edit mode mounts MonacoBackend with the
//! markdown language descriptor. LivePreview is deferred to Phase 4 (CodeMirror 6).

use dioxus::prelude::*;

use crate::editor::LanguageDescriptor;
use crate::plugin::{FormatCaps, FormatPlugin, PluginManifest, PluginSurface};

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

    fn capabilities(&self) -> FormatCaps {
        // LIVE_PREVIEW capability flips on once Phase 4 lands the CodeMirror 6 backend.
        FormatCaps::VIEW | FormatCaps::EDIT
    }

    fn render(&self, _note_id: &str, content: &str) -> Element {
        let content = content.to_string();
        rsx! { MarkdownView { content } }
    }

    fn render_edit(
        &self,
        note_id: &str,
        content: &str,
        on_change: EventHandler<String>,
    ) -> Element {
        let note_id = note_id.to_string();
        let content = content.to_string();
        rsx! {
            crate::shell::editor_host::MonacoEditorHost {
                note_id,
                content,
                language: LanguageDescriptor::markdown(),
                on_change,
            }
        }
    }
}
