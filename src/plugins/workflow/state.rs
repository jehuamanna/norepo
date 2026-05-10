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
    /// Persisted view-only state (expand/collapse, pan, zoom). Lives
    /// on the graph itself so closing and reopening a workflow note
    /// restores the exact viewport the user was looking at. Skipped
    /// at serialize time when default so existing workflow files stay
    /// minimal until the user actually customizes the view.
    #[serde(default, skip_serializing_if = "WorkflowViewState::is_default")]
    pub view_state: WorkflowViewState,
}

/// Persisted UI state for the canvas: which nodes are expanded, plus
/// the last viewport pan / zoom. All fields default; an absent
/// `view_state` block deserializes to "everything collapsed,
/// viewport reset" — matching the first-open experience.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct WorkflowViewState {
    /// Node ids whose children are currently visible. The set lives
    /// on the graph (not a sidecar file) so it round-trips with the
    /// note body — copy/paste / sync works for free.
    #[serde(default)]
    pub expanded_nodes: Vec<NodeId>,
    /// Last viewport translation in canvas client px.
    #[serde(default)]
    pub pan_x: f64,
    #[serde(default)]
    pub pan_y: f64,
    /// Last viewport zoom factor (1.0 = 100%). 0.0 / missing → reset
    /// to 1.0 at hydrate time.
    #[serde(default)]
    pub zoom: f64,
    /// "Step mode" — when set, the cascade pauses after every skill
    /// that produced an artifact (not just `cascade_stop` checkpoint
    /// skills) and outputs stay `Pending` instead of being
    /// auto-approved. Lets the user check + edit + approve each
    /// stage independently before continuing. `None` means "use the
    /// heuristic default" (see `effective_step_mode`); `Some(true)`
    /// / `Some(false)` is the user's explicit choice persisted from
    /// the toolbar toggle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_mode: Option<bool>,
}

impl WorkflowViewState {
    /// `true` when nothing about the view differs from a fresh first
    /// open — used by the `skip_serializing_if` on `view_state` so a
    /// brand-new workflow's JSON stays compact.
    fn is_default(&self) -> bool {
        self.expanded_nodes.is_empty()
            && self.pan_x == 0.0
            && self.pan_y == 0.0
            && (self.zoom == 0.0 || self.zoom == 1.0)
            && self.step_mode.is_none()
    }
}

/// Resolve a graph's effective step-mode flag. Returns the persisted
/// `view_state.step_mode` when the user has explicitly chosen, or the
/// heuristic default when they haven't.
///
/// Heuristic: "is there at least one skill node?".
/// - Cascade-driven `Cascade: <root>` notes are seeded with a single
///   artifact-snapshot tile and grow more snapshots as the cascade
///   runs — they have zero skill nodes. Default → `false` (continuous,
///   matches existing ▶ Play UX).
/// - Hand-built workflows have at least one skill node wired in
///   (otherwise there's nothing to run). Default → `true` so the user
///   can step through and validate each skill in isolation while
///   designing the chain.
/// - Empty graphs default to `false` (no skills to step through).
pub fn effective_step_mode(graph: &WorkflowGraph) -> bool {
    if let Some(explicit) = graph.view_state.step_mode {
        return explicit;
    }
    graph.nodes.values().any(|n| !n.is_artifact_snapshot)
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
    /// The source `NoteKind::Artifact` this skill node consumes when
    /// the workflow-canvas executor invokes it. Set automatically when
    /// the user wires an edge from an artifact-snapshot tile (the
    /// edge-creation handler copies the upstream tile's `artifact_ref`
    /// here) or set explicitly via the inspector. Empty for hand-built
    /// "free-form" workflow nodes that have no artifact context — in
    /// that case the executor falls back to the legacy upstream-outputs
    /// prompt instead of routing through `runner::run_skill_on_source`.
    /// Populating this is what unlocks `aggregate` / `inherit` /
    /// `cascade_stop` / artifact-frontmatter behavior in workflow runs.
    #[serde(default)]
    pub source_artifact_id: Option<Uuid>,
    /// Artifact note ids produced by the most recent successful run
    /// of this node. Populated after `runner::run_skill_on_source`
    /// returns. Used by downstream nodes for fan-out (Phase 6): when
    /// node B's source isn't explicitly set, the executor walks B's
    /// incoming edges and visits each upstream node's
    /// `cached_produced_artifact_ids` as a separate source — i.e.
    /// node B fires once per artifact the upstream produced.
    #[serde(default)]
    pub cached_produced_artifact_ids: Vec<Uuid>,
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
            source_artifact_id: None,
            cached_produced_artifact_ids: Vec::new(),
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

    fn artifact_tile_node(id: NodeId) -> Node {
        Node {
            id,
            skill_note_id: Uuid::nil(),
            typed_fields: serde_json::Value::Null,
            extra_instructions: String::new(),
            position: (0.0, 0.0),
            cached_output_path: None,
            cached_input_hash: None,
            status: NodeStatus::Fresh,
            cached_output_note_id: None,
            is_artifact_snapshot: true,
            artifact_ref: Some(Uuid::new_v4()),
            artifact_kind_label: None,
            artifact_title: None,
            source_artifact_id: None,
            cached_produced_artifact_ids: Vec::new(),
        }
    }

    #[test]
    fn effective_step_mode_explicit_choice_wins() {
        let mut g = WorkflowGraph::new();
        g.view_state.step_mode = Some(true);
        assert!(effective_step_mode(&g));
        g.view_state.step_mode = Some(false);
        assert!(!effective_step_mode(&g));
    }

    #[test]
    fn effective_step_mode_defaults_false_for_artifact_only_graph() {
        // Cascade-driven `Cascade: <root>` notes start with just a
        // single artifact-snapshot tile and zero skill nodes.
        let mut g = WorkflowGraph::new();
        let id = Uuid::new_v4();
        g.nodes.insert(id, artifact_tile_node(id));
        assert!(!effective_step_mode(&g));
    }

    #[test]
    fn effective_step_mode_defaults_true_when_skill_node_present() {
        // Hand-built workflow: at least one skill node wired in.
        let mut g = WorkflowGraph::new();
        let id = Uuid::new_v4();
        g.nodes.insert(id, node(id));
        assert!(effective_step_mode(&g));
    }

    #[test]
    fn effective_step_mode_defaults_false_for_empty_graph() {
        let g = WorkflowGraph::new();
        assert!(!effective_step_mode(&g));
    }

    #[test]
    fn view_state_step_mode_round_trips_through_serde() {
        let mut vs = WorkflowViewState::default();
        vs.step_mode = Some(true);
        let json = serde_json::to_string(&vs).unwrap();
        let back: WorkflowViewState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.step_mode, Some(true));
    }

    #[test]
    fn view_state_default_skips_step_mode_in_serialized_form() {
        // None should be skipped to keep brand-new workflow JSON
        // compact (matches the other view_state fields).
        let vs = WorkflowViewState::default();
        let json = serde_json::to_string(&vs).unwrap();
        assert!(!json.contains("step_mode"));
    }
}
