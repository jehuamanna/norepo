//! `MdxFormatPlugin` — MDX without component evaluation.
//!
//! Reuses the markdown plugin's parser + renderer for everything except top-level JSX
//! blocks, which are detected and rendered as escaped code in View mode. Edit mode is
//! plain Monaco with the markdown language descriptor (a Phase-6.1 follow-up adds the
//! JSX-aware Monarch grammar). LIVE_PREVIEW capability is not claimed.
//!
//! Component evaluation is explicit non-goal. JSX text is shown as-is.

use dioxus::prelude::*;

use crate::editor::LanguageDescriptor;
use crate::plugin::{FormatCaps, FormatPlugin, PluginManifest, PluginSurface};

pub mod parser;
pub mod render;

pub use parser::{parse_mdx, MdxNode};

pub struct MdxFormatPlugin {
    manifest: PluginManifest,
}

impl MdxFormatPlugin {
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: "mdx-note".into(),
                display_name: "MDX".into(),
                version: "0.1.0".into(),
                format_id: Some("mdx"),
                extensions: &["mdx"],
                surfaces: vec![PluginSurface::MainAreaTabContent],
            },
        }
    }
}

impl Default for MdxFormatPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatPlugin for MdxFormatPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn capabilities(&self) -> FormatCaps {
        FormatCaps::VIEW | FormatCaps::EDIT
    }

    fn render(&self, _note_id: &str, content: &str) -> Element {
        let nodes = parse_mdx(content);
        rsx! { render::MdxView { nodes } }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_format_id_and_extensions() {
        let p = MdxFormatPlugin::new();
        assert_eq!(p.manifest().format_id, Some("mdx"));
        assert_eq!(p.manifest().extensions, &["mdx"]);
    }

    #[test]
    fn capabilities_are_view_and_edit_no_live_preview() {
        let p = MdxFormatPlugin::new();
        let caps = p.capabilities();
        assert!(caps.contains(FormatCaps::VIEW));
        assert!(caps.contains(FormatCaps::EDIT));
        assert!(!caps.contains(FormatCaps::LIVE_PREVIEW));
    }
}
