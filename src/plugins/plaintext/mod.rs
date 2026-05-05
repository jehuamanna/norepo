//! `PlaintextFormatPlugin` — `format_id = "plaintext"`.
//!
//! Read-only View renders content in a `<pre>` block with whitespace preserved. Edit mode
//! mounts MonacoBackend with the `plaintext` language descriptor (no syntax highlighting,
//! no folding, no autocomplete — just text).

use dioxus::prelude::*;

use crate::editor::LanguageDescriptor;
use crate::plugin::{FormatCaps, FormatPlugin, PluginManifest, PluginSurface};

mod view;

pub struct PlaintextFormatPlugin {
    manifest: PluginManifest,
}

impl PlaintextFormatPlugin {
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: "plaintext-note".into(),
                display_name: "Plain Text".into(),
                version: "0.1.0".into(),
                format_id: Some("plaintext"),
                extensions: &["txt"],
                surfaces: vec![PluginSurface::MainAreaTabContent],
            },
        }
    }
}

impl Default for PlaintextFormatPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatPlugin for PlaintextFormatPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn capabilities(&self) -> FormatCaps {
        FormatCaps::VIEW | FormatCaps::EDIT
    }

    fn render(&self, _note_id: &str, content: &str) -> Element {
        let content = content.to_string();
        rsx! { view::PlaintextView { content } }
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
            view::PlaintextEditor {
                note_id,
                content,
                language: LanguageDescriptor::plaintext(),
                on_change,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_format_id_and_extensions() {
        let p = PlaintextFormatPlugin::new();
        assert_eq!(p.manifest().format_id, Some("plaintext"));
        assert_eq!(p.manifest().extensions, &["txt"]);
    }

    #[test]
    fn capabilities_are_view_and_edit() {
        let p = PlaintextFormatPlugin::new();
        let caps = p.capabilities();
        assert!(caps.contains(FormatCaps::VIEW));
        assert!(caps.contains(FormatCaps::EDIT));
        assert!(!caps.contains(FormatCaps::LIVE_PREVIEW));
    }
}
