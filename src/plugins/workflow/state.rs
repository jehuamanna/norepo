//! Workflow graph data model — serde-friendly types stored as JSON in
//! the workflow note's body. The engine in `engine.rs` operates over
//! these types; the React Flow canvas (M3b) reads + writes the same
//! shapes through the webview bridge.
//!
//! Position uses `f64` so the JSON round-trip is exact (canvas
//! libraries produce float coords). NodeStatus is in-memory state —
//! recomputed via the engine on graph load — but persisting it in the
//! body lets us show the last-known state immediately on reopen
//! without recomputing every node's hash.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use uuid::Uuid;

pub type NodeId = Uuid;
pub type EdgeId = Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct WorkflowGraph {
    pub nodes: BTreeMap<NodeId, Node>,
    pub edges: Vec<Edge>,
    /// Bumped on every committed mutation. Lets the canvas detect
    /// out-of-band edits (e.g., workflow file rewritten on disk) and
    /// reload without diffing every field.
    #[serde(default)]
    pub version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Node {
    pub id: NodeId,
    /// Points to a `NoteKind::Skill` note in `local_note`. The engine
    /// reads the skill's body + version through `SkillSnapshot` (see
    /// engine.rs) to compute input hashes.
    pub skill_note_id: Uuid,
    /// Form data the BA edits in the inspector — schema declared per
    /// skill via its frontmatter. Engine treats this as opaque and
    /// canonical-json's it into the input hash.
    #[serde(default)]
    pub typed_fields: serde_json::Value,
    /// Free-form markdown the BA appends to the skill prompt at
    /// runtime.
    #[serde(default)]
    pub extra_instructions: String,
    /// Canvas coordinates for the node in the React Flow editor.
    pub position: (f64, f64),
    /// Absolute path to the cached output (markdown body + YAML
    /// frontmatter) produced by the last successful run.
    #[serde(default)]
    pub cached_output_path: Option<PathBuf>,
    /// `compute_input_hash` value at the time `cached_output_path` was
    /// written. Compared against the current input hash to decide
    /// Fresh vs Dirty.
    #[serde(default)]
    pub cached_input_hash: Option<String>,
    /// Last-known render state. Recomputed by the engine on load /
    /// after every mutation; persisted so reopens render immediately.
    #[serde(default)]
    pub status: NodeStatus,
    /// Phase-2 output surfacing: the explorer note row that mirrors
    /// this node's last run output. Stamped on first successful run,
    /// reused across re-runs (the body gets overwritten via
    /// `Persistence::save`). `None` when the node hasn't run yet, or
    /// when the auto-created note was deleted by the user — in which
    /// case the cascade re-creates it on the next run.
    #[serde(default)]
    pub cached_output_note_id: Option<Uuid>,
    /// Cascade-visualization marker: when `true`, this node is a
    /// READ-ONLY snapshot of an Artifact note produced by the
    /// autonomous SDLC cascade (`crate::plugins::artifact::cascade`),
    /// not a user-editable skill invocation in a hand-crafted DAG.
    /// The view renders these with a compact "kind badge + title"
    /// card; the engine/executor skip them during dirty-propagation
    /// and run since they have nothing to execute. Defaults to
    /// `false`, so pre-existing graphs round-trip unchanged.
    #[serde(default)]
    pub is_artifact_snapshot: bool,
    /// When `is_artifact_snapshot` is true, this is the Artifact note
    /// id the snapshot represents. Lets the view link from the
    /// canvas tile to the artifact's editor tab. None on regular
    /// skill nodes.
    #[serde(default)]
    pub artifact_ref: Option<Uuid>,
    /// When `is_artifact_snapshot` is true, this is the artifact's
    /// kind (e.g. "epic") cached for fast badge rendering without
    /// reloading the artifact's frontmatter on every paint. None on
    /// regular skill nodes.
    #[serde(default)]
    pub artifact_kind_label: Option<String>,
    /// When `is_artifact_snapshot` is true, this is the artifact
    /// note's title at snapshot time — what the user sees on the
    /// canvas tile (e.g. `epic-01-realtime-collaboration`). Cached
    /// so the canvas renderer doesn't need a `LocalNoteRepository`
    /// lookup on every paint.
    #[serde(default)]
    pub artifact_title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum NodeStatus {
    /// Fresh: cached_input_hash == current input hash; output is
    /// up-to-date.
    #[default]
    Fresh,
    /// Dirty: cached_input_hash differs (or is absent). Needs a re-run.
    Dirty,
    /// Currently executing in the companion. UI shows a spinner.
    Running,
    /// Last run failed. `detail` carries the error message for the
    /// inspector / tooltip.
    Error(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Edge {
    pub id: EdgeId,
    pub from: NodeId,
    /// Output socket on the source node. M3a supports a single default
    /// socket per node ("out"); future skills with multi-output schemas
    /// will dispatch on this name.
    #[serde(default = "default_socket")]
    pub from_socket: String,
    pub to: NodeId,
    #[serde(default = "default_socket")]
    pub to_socket: String,
    /// Cascade-visualization marker: distinguishes parent/child edges
    /// from "Depends on" cross-edges parsed out of artifact bodies.
    /// `None` (or the default empty string) is the existing skill-DAG
    /// edge — black. `Some("parent_child")` is the implicit fan-out
    /// from a parent artifact to its produced children; `Some("depends_on")`
    /// is a cross-edge between siblings derived from the
    /// `## Depends on` section in their markdown bodies (rendered amber
    /// in the canvas).
    #[serde(default)]
    pub edge_kind: Option<String>,
}

fn default_socket() -> String {
    "default".into()
}

impl WorkflowGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Convenience: nodes whose status is `Dirty`, in graph-insertion
    /// order. The topo-order helper in engine.rs takes a fresh slice.
    pub fn dirty_nodes(&self) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter(|(_, n)| matches!(n.status, NodeStatus::Dirty))
            .map(|(id, _)| *id)
            .collect()
    }

    pub fn upstream_of(&self, node: NodeId) -> Vec<NodeId> {
        self.edges
            .iter()
            .filter(|e| e.to == node)
            .map(|e| e.from)
            .collect()
    }

    pub fn downstream_of(&self, node: NodeId) -> Vec<NodeId> {
        self.edges
            .iter()
            .filter(|e| e.from == node)
            .map(|e| e.to)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_graph() -> WorkflowGraph {
        WorkflowGraph::new()
    }

    fn node(id: NodeId) -> Node {
        Node {
            id,
            skill_note_id: Uuid::new_v4(),
            typed_fields: serde_json::Value::Null,
            extra_instructions: String::new(),
            position: (0.0, 0.0),
            cached_output_path: None,
            cached_input_hash: None,
            status: NodeStatus::Fresh,
            cached_output_note_id: None,
            is_artifact_snapshot: false,
            artifact_ref: None,
            artifact_kind_label: None,
            artifact_title: None,
        }
    }

    fn edge(from: NodeId, to: NodeId) -> Edge {
        Edge {
            id: Uuid::new_v4(),
            from,
            from_socket: "default".into(),
            to,
            to_socket: "default".into(),
            edge_kind: None,
        }
    }

    #[test]
    fn graph_roundtrips_via_serde_json() {
        let mut g = empty_graph();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        g.nodes.insert(a, node(a));
        g.nodes.insert(b, node(b));
        g.edges.push(edge(a, b));
        let json = serde_json::to_string(&g).unwrap();
        let back: WorkflowGraph = serde_json::from_str(&json).unwrap();
        assert_eq!(g, back);
    }

    #[test]
    fn dirty_nodes_filters_correctly() {
        let mut g = empty_graph();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        g.nodes.insert(a, node(a));
        let mut nb = node(b);
        nb.status = NodeStatus::Dirty;
        g.nodes.insert(b, nb);
        let mut nc = node(c);
        nc.status = NodeStatus::Running;
        g.nodes.insert(c, nc);
        assert_eq!(g.dirty_nodes(), vec![b]);
    }

    #[test]
    fn upstream_downstream_lookup() {
        let mut g = empty_graph();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        for id in [a, b, c] {
            g.nodes.insert(id, node(id));
        }
        g.edges.push(edge(a, b));
        g.edges.push(edge(b, c));
        g.edges.push(edge(a, c));
        assert_eq!(g.upstream_of(c), vec![b, a]);
        assert_eq!(g.downstream_of(a), vec![b, c]);
        assert!(g.upstream_of(a).is_empty());
        assert!(g.downstream_of(c).is_empty());
    }

    #[test]
    fn node_status_serde_tagged() {
        let s = NodeStatus::Error("boom".into());
        let j = serde_json::to_string(&s).unwrap();
        assert_eq!(j, r#"{"kind":"error","detail":"boom"}"#);
        let back: NodeStatus = serde_json::from_str(&j).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn edge_socket_defaults() {
        let json = r#"{"id":"00000000-0000-0000-0000-000000000000","from":"00000000-0000-0000-0000-000000000001","to":"00000000-0000-0000-0000-000000000002"}"#;
        let e: Edge = serde_json::from_str(json).unwrap();
        assert_eq!(e.from_socket, "default");
        assert_eq!(e.to_socket, "default");
    }
}
