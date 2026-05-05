//! `RichTextTiptapFormatPlugin` — `format_id = "richtext-tiptap"`, extension `.note`.
//!
//! Per locked decision D2: distinct format with its own file type. Persisted as Tiptap
//! JSON (the `editor.getJSON()` shape). View renders Tiptap with `editable: false`; Edit
//! is the live editor. LIVE_PREVIEW capability not claimed (Tiptap is already WYSIWYG).
//! No round-trip through markdown source.

use dioxus::prelude::*;

use crate::editor::LanguageDescriptor;
use crate::plugin::{FormatCaps, FormatPlugin, PluginManifest, PluginSurface};

pub struct RichTextTiptapFormatPlugin {
    manifest: PluginManifest,
}

impl RichTextTiptapFormatPlugin {
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: "richtext-tiptap-note".into(),
                display_name: "Rich Text".into(),
                version: "0.1.0".into(),
                format_id: Some("richtext-tiptap"),
                extensions: &["note"],
                surfaces: vec![PluginSurface::MainAreaTabContent],
            },
        }
    }
}

impl Default for RichTextTiptapFormatPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatPlugin for RichTextTiptapFormatPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn capabilities(&self) -> FormatCaps {
        FormatCaps::VIEW | FormatCaps::EDIT
    }

    fn render(&self, note_id: &str, content: &str) -> Element {
        let note_id = note_id.to_string();
        let content = content.to_string();
        rsx! {
            crate::shell::tiptap_host::TiptapEditorHost {
                note_id,
                content,
                language: LanguageDescriptor::richtext_tiptap(),
                read_only: true,
                on_change: EventHandler::new(|_: String| {}),
            }
        }
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
            crate::shell::tiptap_host::TiptapEditorHost {
                note_id,
                content,
                language: LanguageDescriptor::richtext_tiptap(),
                read_only: false,
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
        let p = RichTextTiptapFormatPlugin::new();
        assert_eq!(p.manifest().format_id, Some("richtext-tiptap"));
        assert_eq!(p.manifest().extensions, &["note"]);
    }

    #[test]
    fn capabilities_are_view_and_edit_no_live_preview() {
        let p = RichTextTiptapFormatPlugin::new();
        let caps = p.capabilities();
        assert!(caps.contains(FormatCaps::VIEW));
        assert!(caps.contains(FormatCaps::EDIT));
        assert!(!caps.contains(FormatCaps::LIVE_PREVIEW));
    }
}
