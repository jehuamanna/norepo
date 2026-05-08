//! Artifact note plugin: SDLC pipeline output (Epics, Features,
//! Stories, Tasks, Plans, TestCases, Summaries). Each artifact is a
//! `NoteKind::Artifact` markdown note whose YAML frontmatter declares
//! its kind / status / parent / source-skill linkage. The artifact
//! tree under a project is the canonical workflow surface — running a
//! skill against an artifact produces N child artifacts under it.

use dioxus::prelude::*;

use crate::plugin::{FormatCaps, FormatPlugin, PluginManifest, PluginSurface};

pub mod frontmatter;
#[cfg(not(target_arch = "wasm32"))]
pub mod runner;
mod view;

pub use frontmatter::{
    parse, rewrite, ArtifactFrontmatter, ArtifactKind, ArtifactStatus,
};
#[cfg(not(target_arch = "wasm32"))]
pub use runner::{run_skill_on_source, RunOutcome, RunnerError};

pub struct ArtifactFormatPlugin {
    manifest: PluginManifest,
}

impl ArtifactFormatPlugin {
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: "artifact-note".into(),
                display_name: "Artifact".into(),
                version: "0.1.0".into(),
                format_id: Some("artifact"),
                extensions: &["artifact", "md"],
                surfaces: vec![PluginSurface::MainAreaTabContent],
            },
        }
    }
}

impl Default for ArtifactFormatPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatPlugin for ArtifactFormatPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn capabilities(&self) -> FormatCaps {
        FormatCaps::VIEW | FormatCaps::EDIT
    }

    fn render(&self, note_id: &str, content: &str) -> Element {
        let note_id = note_id.to_string();
        let content = content.to_string();
        rsx! { view::ArtifactView { note_id, content, edit: false } }
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
            view::ArtifactView {
                note_id,
                content,
                edit: true,
                on_change: on_change,
            }
        }
    }
}

#[cfg(test)]
mod plugin_tests {
    use super::*;

    #[test]
    fn manifest_format_id() {
        let p = ArtifactFormatPlugin::new();
        assert_eq!(p.manifest().format_id, Some("artifact"));
    }

    #[test]
    fn capabilities_are_view_and_edit() {
        let p = ArtifactFormatPlugin::new();
        let caps = p.capabilities();
        assert!(caps.contains(FormatCaps::VIEW));
        assert!(caps.contains(FormatCaps::EDIT));
    }
}
