//! Workflow notes — visual DAG of skill-node references with stale-
//! tracking for cascade re-execution.
//!
//! M3a (this milestone): pure Rust engine + data model. Hashes skill +
//! node config + upstream outputs, marks downstream nodes Dirty when
//! anything drifts, topo-sorts the dirty subset for cascade execution.
//! No UI; the React Flow canvas + executor wiring land in M3b/M3c.

pub mod engine;
pub mod state;
mod view;

pub use engine::{
    compute_input_hash, hash_body, propagate_dirty, topo_order_dirty, CycleError, EngineError,
    SkillBag, SkillSnapshot,
};
pub use state::{Edge, EdgeId, Node, NodeId, NodeStatus, WorkflowGraph};

use dioxus::prelude::*;

use crate::plugin::{FormatCaps, FormatPlugin, PluginManifest, PluginSurface};

pub struct WorkflowFormatPlugin {
    manifest: PluginManifest,
}

impl WorkflowFormatPlugin {
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: "workflow-note".into(),
                display_name: "Workflow".into(),
                version: "0.1.0".into(),
                format_id: Some("workflow"),
                extensions: &["workflow"],
                surfaces: vec![PluginSurface::MainAreaTabContent],
            },
        }
    }
}

impl Default for WorkflowFormatPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatPlugin for WorkflowFormatPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn capabilities(&self) -> FormatCaps {
        FormatCaps::VIEW | FormatCaps::EDIT
    }

    fn render(&self, _note_id: &str, content: &str) -> Element {
        let content = content.to_string();
        rsx! { view::WorkflowView { content } }
    }

    fn render_edit(
        &self,
        note_id: &str,
        content: &str,
        on_change: EventHandler<String>,
    ) -> Element {
        let note_id = note_id.to_string();
        let content = content.to_string();
        rsx! { view::WorkflowEditor { note_id, content, on_change } }
    }
}

#[cfg(test)]
mod plugin_tests {
    use super::*;

    #[test]
    fn manifest_format_id() {
        let p = WorkflowFormatPlugin::new();
        assert_eq!(p.manifest().format_id, Some("workflow"));
    }

    #[test]
    fn capabilities_are_view_and_edit() {
        let p = WorkflowFormatPlugin::new();
        let caps = p.capabilities();
        assert!(caps.contains(FormatCaps::VIEW));
        assert!(caps.contains(FormatCaps::EDIT));
    }
}