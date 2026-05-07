//! `ImageFormatPlugin` — image notes (`format_id = "image"`).
//!
//! Image content lives on disk (content-addressed under
//! `<vault>/.operon/images/<sha>.<ext>`) and is referenced from the
//! `local_note.blob_path` column rather than the markdown body. The
//! plugin therefore ignores the `content` argument entirely and looks
//! the row up by `note_id` through the Local-Mode repos in context.
//!
//! View mode renders the blob as `<img>`. Edit mode falls back to a
//! drop / paste / picker empty-state when `blob_path` is `None` and
//! flips to the same `<img>` viewer once a blob has been attached.

use dioxus::prelude::*;

use crate::plugin::{FormatCaps, FormatPlugin, PluginManifest, PluginSurface};

#[cfg(not(target_arch = "wasm32"))]
mod view;

pub struct ImageFormatPlugin {
    manifest: PluginManifest,
}

impl ImageFormatPlugin {
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: "image-note".into(),
                display_name: "Image".into(),
                version: "0.1.0".into(),
                format_id: Some("image"),
                extensions: &["png", "jpg", "jpeg", "webp", "gif", "svg", "avif"],
                surfaces: vec![PluginSurface::MainAreaTabContent],
            },
        }
    }
}

impl Default for ImageFormatPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatPlugin for ImageFormatPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn capabilities(&self) -> FormatCaps {
        FormatCaps::VIEW | FormatCaps::EDIT
    }

    fn render(&self, note_id: &str, _content: &str) -> Element {
        let note_id = note_id.to_string();
        #[cfg(not(target_arch = "wasm32"))]
        {
            rsx! { view::ImageNotePane { note_id, editable: false } }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = note_id;
            rsx! { div { class: "operon-main-empty", "Image notes are desktop-only for now." } }
        }
    }

    fn render_edit(
        &self,
        note_id: &str,
        _content: &str,
        _on_change: EventHandler<String>,
    ) -> Element {
        let note_id = note_id.to_string();
        #[cfg(not(target_arch = "wasm32"))]
        {
            rsx! { view::ImageNotePane { note_id, editable: true } }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = note_id;
            rsx! { div { class: "operon-main-empty", "Image notes are desktop-only for now." } }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_format_id_and_extensions() {
        let p = ImageFormatPlugin::new();
        assert_eq!(p.manifest().format_id, Some("image"));
        assert!(p.manifest().extensions.contains(&"png"));
        assert!(p.manifest().extensions.contains(&"webp"));
    }

    #[test]
    fn capabilities_are_view_and_edit() {
        let p = ImageFormatPlugin::new();
        let caps = p.capabilities();
        assert!(caps.contains(FormatCaps::VIEW));
        assert!(caps.contains(FormatCaps::EDIT));
        assert!(!caps.contains(FormatCaps::LIVE_PREVIEW));
    }
}
