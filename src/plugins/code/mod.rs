//! `CodeFormatPlugin` — generic source-code editor backed by MonacoEditorHost.
//!
//! Edit mode mounts Monaco with `LanguageDescriptor::code()` (which defaults
//! to plaintext at mount time). A language picker lives in the editor toolbar
//! and re-mounts the host with the chosen Monaco language id. View mode is
//! a read-only `<pre>`.

use dioxus::prelude::*;

use crate::editor::LanguageDescriptor;
use crate::plugin::{FormatCaps, FormatPlugin, PluginManifest, PluginSurface};

mod view;

pub struct CodeFormatPlugin {
    manifest: PluginManifest,
}

impl CodeFormatPlugin {
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: "code-note".into(),
                display_name: "Code".into(),
                version: "0.1.0".into(),
                format_id: Some("code"),
                extensions: &[
                    "rs", "py", "js", "ts", "tsx", "jsx", "go", "java", "c", "cpp", "h", "hpp",
                    "rb", "sh", "yaml", "yml", "toml", "html", "css",
                ],
                surfaces: vec![PluginSurface::MainAreaTabContent],
            },
        }
    }
}

impl Default for CodeFormatPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatPlugin for CodeFormatPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn capabilities(&self) -> FormatCaps {
        FormatCaps::VIEW | FormatCaps::EDIT
    }

    fn render(&self, _note_id: &str, content: &str) -> Element {
        let content = content.to_string();
        rsx! { view::CodeView { content } }
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
            view::CodeEditor {
                note_id,
                content,
                language: LanguageDescriptor::code(),
                on_change,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_format_id() {
        let p = CodeFormatPlugin::new();
        assert_eq!(p.manifest().format_id, Some("code"));
    }

    #[test]
    fn capabilities_are_view_and_edit() {
        let p = CodeFormatPlugin::new();
        let caps = p.capabilities();
        assert!(caps.contains(FormatCaps::VIEW));
        assert!(caps.contains(FormatCaps::EDIT));
    }
}
