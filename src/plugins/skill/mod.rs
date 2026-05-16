//! `SkillFormatPlugin` — `format_id = "skill"`.
//!
//! A Claude Code skill authored as a note: markdown body with optional
//! YAML frontmatter (skill_name, skill_version, inputs, output_frontmatter).
//! View renders the body via the existing markdown renderer; Edit mounts a
//! plain-text editor so the user can author the prompt + the frontmatter.
//! Both modes layer a ▶ Play toolbar on top — clicking Play materializes
//! the body to `<repo>/.claude/skills/<slug>.md` and pushes a "Use the
//! skill named <slug>" prompt into the active project's companion chat
//! session.

use dioxus::prelude::*;

use crate::plugin::{FormatCaps, FormatPlugin, PluginManifest, PluginSurface};

pub mod frontmatter;
pub mod install;
pub mod materialize;
pub mod seed;
mod view;

pub struct SkillFormatPlugin {
    manifest: PluginManifest,
}

impl SkillFormatPlugin {
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: "skill-note".into(),
                display_name: "Skill".into(),
                version: "0.1.0".into(),
                format_id: Some("skill"),
                extensions: &["skill", "md"],
                surfaces: vec![PluginSurface::MainAreaTabContent],
            },
        }
    }
}

impl Default for SkillFormatPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatPlugin for SkillFormatPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn capabilities(&self) -> FormatCaps {
        FormatCaps::VIEW | FormatCaps::EDIT
    }

    fn render(&self, note_id: &str, content: &str) -> Element {
        let note_id = note_id.to_string();
        let content = content.to_string();
        rsx! { view::SkillView { note_id, content, edit: false } }
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
            view::SkillEditor { note_id, content, on_change }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_format_id() {
        let p = SkillFormatPlugin::new();
        assert_eq!(p.manifest().format_id, Some("skill"));
    }

    #[test]
    fn capabilities_are_view_and_edit() {
        let p = SkillFormatPlugin::new();
        let caps = p.capabilities();
        assert!(caps.contains(FormatCaps::VIEW));
        assert!(caps.contains(FormatCaps::EDIT));
    }
}
