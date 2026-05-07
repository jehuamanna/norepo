//! `KanbanFormatPlugin` — column-based board persisted as JSON in the note's
//! body. The schema is intentionally minimal so a freshly-created Kanban note
//! starts at `{"columns":[]}` and grows as the user adds columns and cards.

use dioxus::prelude::*;

use crate::plugin::{FormatCaps, FormatPlugin, PluginManifest, PluginSurface};

mod model;
mod view;

pub use model::{KanbanBoard, KanbanCard, KanbanColumn};

pub struct KanbanFormatPlugin {
    manifest: PluginManifest,
}

impl KanbanFormatPlugin {
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: "kanban-note".into(),
                display_name: "Kanban".into(),
                version: "0.1.0".into(),
                format_id: Some("kanban"),
                extensions: &["kanban", "kanban.json"],
                surfaces: vec![PluginSurface::MainAreaTabContent],
            },
        }
    }
}

impl Default for KanbanFormatPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatPlugin for KanbanFormatPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn capabilities(&self) -> FormatCaps {
        FormatCaps::VIEW | FormatCaps::EDIT
    }

    fn render(&self, _note_id: &str, content: &str) -> Element {
        let board = KanbanBoard::parse(content);
        rsx! { view::KanbanView { board } }
    }

    fn render_edit(
        &self,
        _note_id: &str,
        content: &str,
        on_change: EventHandler<String>,
    ) -> Element {
        let initial = content.to_string();
        rsx! { view::KanbanEditor { initial, on_change } }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_format_id() {
        let p = KanbanFormatPlugin::new();
        assert_eq!(p.manifest().format_id, Some("kanban"));
    }

    #[test]
    fn capabilities_are_view_and_edit() {
        let p = KanbanFormatPlugin::new();
        let caps = p.capabilities();
        assert!(caps.contains(FormatCaps::VIEW));
        assert!(caps.contains(FormatCaps::EDIT));
    }
}
