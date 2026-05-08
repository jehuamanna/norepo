//! Reactive engine for workflow graphs.
//!
//! Pure functions over `WorkflowGraph`:
//! - `compute_input_hash`: sha256 of (skill identity + node config +
//!   sorted upstream cached output hashes). Deterministic; identical
//!   inputs always produce identical hashes regardless of edge insertion
//!   order.
//! - `propagate_dirty`: walk down the edge graph from a seed and mark
//!   any node whose recomputed input hash differs from its cached one
//!   as `Dirty`. Idempotent.
//! - `topo_order_dirty`: Kahn's algorithm restricted to the dirty
//!   subset. Returns `CycleError` if the graph has a cycle through any
//!   currently-dirty node.
//!
//! Skill metadata is supplied via `SkillSnapshot` so unit tests can
//! drive the engine without standing up the SQLite store. In
//! production (M3b), the workflow plugin builds these snapshots from
//! the referenced `NoteKind::Skill` notes.

use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use uuid::Uuid;

use crate::plugins::workflow::state::{NodeId, NodeStatus, WorkflowGraph};

/// Frozen view of a skill the engine needs to hash. The version + body
/// hash are consumed verbatim into the node's input hash so editing
/// the skill body invalidates downstream nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSnapshot {
    /// The skill's `skill_version` frontmatter field (or any other
    /// identity stamp the caller has). Empty string is fine when not
    /// declared.
    pub version: String,
    /// sha256 hex of the skill note's body. Helper `hash_body` below
    /// computes this.
    pub body_hash: String,
}

pub type SkillBag = HashMap<Uuid, SkillSnapshot>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineError {
    UnknownSkill(Uuid),
    UnknownNode(NodeId),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownSkill(id) => write!(f, "no SkillSnapshot for skill_note_id {id}"),
            Self::UnknownNode(id) => write!(f, "no node with id {id} in graph"),
        }
    }
}

impl std::error::Error for EngineError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleError {
    pub residual: BTreeSet<NodeId>,
}

impl std::fmt::Display for CycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "cycle detected; {} dirty node(s) could not be ordered",
            self.residual.len()
        )
    }
}

impl std::error::Error for CycleError {}

/// Compute a node's input hash. Mixes:
/// - skill_note_id, skill_version, skill_body_hash
/// - canonical-json(typed_fields)
/// - extra_instructions
/// - sorted (by NodeId) upstream `cached_input_hash`es (or empty when
///   an upstream has no output yet)
pub fn compute_input_hash(
    node_id: NodeId,
    graph: &WorkflowGraph,
    skills: &SkillBag,
) -> Result<String, EngineError> {
    let node = graph
        .nodes
        .get(&node_id)
        .ok_or(EngineError::UnknownNode(node_id))?;
    let skill = skills
        .get(&node.skill_note_id)
        .ok_or(EngineError::UnknownSkill(node.skill_note_id))?;

    let mut hasher = Sha256::new();
    hasher.update(b"skill_note_id\0");
    hasher.update(node.skill_note_id.as_bytes());
    hasher.update(b"\0skill_version\0");
    hasher.update(skill.version.as_bytes());
    hasher.update(b"\0skill_body_hash\0");
    hasher.update(skill.body_hash.as_bytes());
    hasher.update(b"\0typed_fields\0");
    hasher.update(canonical_json(&node.typed_fields).as_bytes());
    hasher.update(b"\0extra_instructions\0");
    hasher.update(node.extra_instructions.as_bytes());

    hasher.update(b"\0upstream\0");
    let mut upstream: Vec<NodeId> = graph.upstream_of(node_id);
    upstream.sort();
    for up_id in upstream {
        hasher.update(up_id.as_bytes());
        hasher.update(b":");
        let up = graph
            .nodes
            .get(&up_id)
            .ok_or(EngineError::UnknownNode(up_id))?;
        let up_hash = up.cached_input_hash.as_deref().unwrap_or("");
        hasher.update(up_hash.as_bytes());
        hasher.update(b"\0");
    }

    Ok(hex(&hasher.finalize()))
}

/// Propagate dirty downstream from `seed`. The seed itself is also
/// re-hashed and marked dirty if its hash drifted. Nodes already
/// `Running` or `Error` keep their status — re-running them is a
/// caller decision, and the dirty mark would be stale by definition.
pub fn propagate_dirty(
    seed: NodeId,
    graph: &mut WorkflowGraph,
    skills: &SkillBag,
) -> Result<(), EngineError> {
    let mut visited: HashSet<NodeId> = HashSet::new();
    let mut queue: VecDeque<NodeId> = VecDeque::new();
    queue.push_back(seed);
    while let Some(id) = queue.pop_front() {
        if !visited.insert(id) {
            continue;
        }
        // We need a stable downstream snapshot before mutating the
        // node's status (mutating mid-borrow gets messy with BTreeMap).
        let downstream = graph.downstream_of(id);
        let new_hash = compute_input_hash(id, graph, skills)?;
        if let Some(node) = graph.nodes.get_mut(&id) {
            let cached = node.cached_input_hash.as_deref();
            let hash_drifted = cached != Some(new_hash.as_str());
            if hash_drifted && !matches!(node.status, NodeStatus::Running | NodeStatus::Error(_))
            {
                node.status = NodeStatus::Dirty;
            }
        }
        for d in downstream {
            queue.push_back(d);
        }
    }
    Ok(())
}

/// Topologically order the currently-dirty nodes such that for every
/// edge `u → v` where both are dirty, `u` precedes `v`. Returns
/// `CycleError` if any cycle exists through the dirty subset.
pub fn topo_order_dirty(graph: &WorkflowGraph) -> Result<Vec<NodeId>, CycleError> {
    let dirty: BTreeSet<NodeId> = graph
        .nodes
        .iter()
        .filter(|(_, n)| matches!(n.status, NodeStatus::Dirty))
        .map(|(id, _)| *id)
        .collect();
    if dirty.is_empty() {
        return Ok(Vec::new());
    }

    // Restrict edges to those between dirty nodes.
    let mut indeg: BTreeMap<NodeId, usize> = dirty.iter().map(|id| (*id, 0usize)).collect();
    let mut adj: BTreeMap<NodeId, Vec<NodeId>> = BTreeMap::new();
    for e in &graph.edges {
        if dirty.contains(&e.from) && dirty.contains(&e.to) {
            adj.entry(e.from).or_default().push(e.to);
            *indeg.entry(e.to).or_default() += 1;
        }
    }

    let mut zero: VecDeque<NodeId> = indeg
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(id, _)| *id)
        .collect();
    let mut out: Vec<NodeId> = Vec::with_capacity(dirty.len());
    while let Some(n) = zero.pop_front() {
        out.push(n);
        if let Some(nexts) = adj.get(&n) {
            for next in nexts {
                if let Some(d) = indeg.get_mut(next) {
                    *d -= 1;
                    if *d == 0 {
                        zero.push_back(*next);
                    }
                }
            }
        }
    }

    if out.len() != dirty.len() {
        let placed: BTreeSet<NodeId> = out.iter().copied().collect();
        let residual: BTreeSet<NodeId> = dirty.difference(&placed).copied().collect();
        return Err(CycleError { residual });
    }
    Ok(out)
}

/// Compute the body hash a `SkillSnapshot` should carry. Provided here
/// so the workflow plugin and tests use the same algorithm.
pub fn hash_body(body: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(body.as_bytes());
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(s, "{:02x}", b);
    }
    s
}

/// Stable JSON serialization with object keys sorted. `serde_json`
/// already preserves insertion order; we walk the value and rebuild
/// with sorted keys so two semantically equivalent objects hash the
/// same regardless of how the canvas emitted them.
fn canonical_json(value: &serde_json::Value) -> String {
    fn walk(v: &serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Object(map) => {
                let mut sorted: Vec<(String, serde_json::Value)> = map
                    .iter()
                    .map(|(k, v)| (k.clone(), walk(v)))
                    .collect();
                sorted.sort_by(|a, b| a.0.cmp(&b.0));
                let mut new_map = serde_json::Map::new();
                for (k, v) in sorted {
                    new_map.insert(k, v);
                }
                serde_json::Value::Object(new_map)
            }
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(walk).collect())
            }
            other => other.clone(),
        }
    }
    serde_json::to_string(&walk(value)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::workflow::state::{Edge, Node, NodeStatus};

    fn skill(version: &str, body: &str) -> SkillSnapshot {
        SkillSnapshot {
            version: version.into(),
            body_hash: hash_body(body),
        }
    }

    fn node(id: NodeId, skill_note_id: Uuid) -> Node {
        Node {
            id,
            skill_note_id,
            typed_fields: serde_json::Value::Null,
            extra_instructions: String::new(),
            position: (0.0, 0.0),
            cached_output_path: None,
            cached_input_hash: None,
            cached_output_note_id: None,
            status: NodeStatus::Dirty,
        }
    }

    fn edge(from: NodeId, to: NodeId) -> Edge {
        Edge {
            id: Uuid::new_v4(),
            from,
            from_socket: "default".into(),
            to,
            to_socket: "default".into(),
        }
    }

    fn graph_with(nodes: &[(NodeId, Uuid)], edges: &[(NodeId, NodeId)]) -> WorkflowGraph {
        let mut g = WorkflowGraph::new();
        for (id, skill_id) in nodes {
            g.nodes.insert(*id, node(*id, *skill_id));
        }
        for (f, t) in edges {
            g.edges.push(edge(*f, *t));
        }
        g
    }

    fn skills(snapshots: &[(Uuid, &str, &str)]) -> SkillBag {
        snapshots
            .iter()
            .map(|(id, v, body)| (*id, skill(v, body)))
            .collect()
    }

    #[test]
    fn compute_input_hash_is_deterministic() {
        let n = Uuid::new_v4();
        let s = Uuid::new_v4();
        let g = graph_with(&[(n, s)], &[]);
        let bag = skills(&[(s, "1", "you are a BA")]);
        let h1 = compute_input_hash(n, &g, &bag).unwrap();
        let h2 = compute_input_hash(n, &g, &bag).unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn compute_input_hash_changes_with_typed_fields() {
        let n = Uuid::new_v4();
        let s = Uuid::new_v4();
        let mut g = graph_with(&[(n, s)], &[]);
        let bag = skills(&[(s, "1", "body")]);
        let h0 = compute_input_hash(n, &g, &bag).unwrap();
        g.nodes.get_mut(&n).unwrap().typed_fields = serde_json::json!({"a": 1});
        let h1 = compute_input_hash(n, &g, &bag).unwrap();
        assert_ne!(h0, h1);
    }

    #[test]
    fn compute_input_hash_changes_with_extra_instructions() {
        let n = Uuid::new_v4();
        let s = Uuid::new_v4();
        let mut g = graph_with(&[(n, s)], &[]);
        let bag = skills(&[(s, "1", "body")]);
        let h0 = compute_input_hash(n, &g, &bag).unwrap();
        g.nodes.get_mut(&n).unwrap().extra_instructions = "tweak".into();
        let h1 = compute_input_hash(n, &g, &bag).unwrap();
        assert_ne!(h0, h1);
    }

    #[test]
    fn compute_input_hash_changes_with_skill_body() {
        let n = Uuid::new_v4();
        let s = Uuid::new_v4();
        let g = graph_with(&[(n, s)], &[]);
        let bag1 = skills(&[(s, "1", "body v1")]);
        let bag2 = skills(&[(s, "1", "body v2")]);
        let h1 = compute_input_hash(n, &g, &bag1).unwrap();
        let h2 = compute_input_hash(n, &g, &bag2).unwrap();
        assert_ne!(h1, h2);
    }

    #[test]
    fn compute_input_hash_changes_with_skill_version() {
        let n = Uuid::new_v4();
        let s = Uuid::new_v4();
        let g = graph_with(&[(n, s)], &[]);
        let bag1 = skills(&[(s, "1", "body")]);
        let bag2 = skills(&[(s, "2", "body")]);
        let h1 = compute_input_hash(n, &g, &bag1).unwrap();
        let h2 = compute_input_hash(n, &g, &bag2).unwrap();
        assert_ne!(h1, h2);
    }

    #[test]
    fn compute_input_hash_includes_upstream_output_hashes() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let s = Uuid::new_v4();
        let mut g = graph_with(&[(a, s), (b, s)], &[(a, b)]);
        let bag = skills(&[(s, "1", "body")]);
        // First, b's hash with no upstream output:
        let h0 = compute_input_hash(b, &g, &bag).unwrap();
        // Now mark A's cached output hash:
        g.nodes.get_mut(&a).unwrap().cached_input_hash = Some("a-output-x".into());
        let h1 = compute_input_hash(b, &g, &bag).unwrap();
        assert_ne!(h0, h1);
        // Different a output → different b hash:
        g.nodes.get_mut(&a).unwrap().cached_input_hash = Some("a-output-y".into());
        let h2 = compute_input_hash(b, &g, &bag).unwrap();
        assert_ne!(h1, h2);
    }

    #[test]
    fn compute_input_hash_is_independent_of_edge_insertion_order() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let s = Uuid::new_v4();
        let mut g1 = graph_with(&[(a, s), (b, s), (c, s)], &[(a, c), (b, c)]);
        let mut g2 = graph_with(&[(a, s), (b, s), (c, s)], &[(b, c), (a, c)]);
        // Same upstream cache states.
        for g in [&mut g1, &mut g2] {
            g.nodes.get_mut(&a).unwrap().cached_input_hash = Some("ah".into());
            g.nodes.get_mut(&b).unwrap().cached_input_hash = Some("bh".into());
        }
        let bag = skills(&[(s, "1", "body")]);
        let h1 = compute_input_hash(c, &g1, &bag).unwrap();
        let h2 = compute_input_hash(c, &g2, &bag).unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn propagate_dirty_marks_seed_when_hash_drifts() {
        let a = Uuid::new_v4();
        let s = Uuid::new_v4();
        let mut g = graph_with(&[(a, s)], &[]);
        // Seed status starts Dirty (default in test factory). Set Fresh
        // with a non-matching cached hash so propagate flips it back.
        let na = g.nodes.get_mut(&a).unwrap();
        na.status = NodeStatus::Fresh;
        na.cached_input_hash = Some("stale".into());
        let bag = skills(&[(s, "1", "body")]);
        propagate_dirty(a, &mut g, &bag).unwrap();
        assert!(matches!(g.nodes[&a].status, NodeStatus::Dirty));
    }

    #[test]
    fn propagate_dirty_keeps_fresh_when_hash_matches() {
        let a = Uuid::new_v4();
        let s = Uuid::new_v4();
        let mut g = graph_with(&[(a, s)], &[]);
        let bag = skills(&[(s, "1", "body")]);
        let h = compute_input_hash(a, &g, &bag).unwrap();
        let na = g.nodes.get_mut(&a).unwrap();
        na.cached_input_hash = Some(h);
        na.status = NodeStatus::Fresh;
        propagate_dirty(a, &mut g, &bag).unwrap();
        assert!(matches!(g.nodes[&a].status, NodeStatus::Fresh));
    }

    #[test]
    fn propagate_dirty_walks_downstream() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let s = Uuid::new_v4();
        let mut g = graph_with(&[(a, s), (b, s), (c, s)], &[(a, b), (b, c)]);
        let bag = skills(&[(s, "1", "body")]);
        // All start Fresh with stale hashes.
        for id in [a, b, c] {
            let n = g.nodes.get_mut(&id).unwrap();
            n.status = NodeStatus::Fresh;
            n.cached_input_hash = Some(format!("stale-{id}"));
        }
        propagate_dirty(a, &mut g, &bag).unwrap();
        assert!(matches!(g.nodes[&a].status, NodeStatus::Dirty));
        assert!(matches!(g.nodes[&b].status, NodeStatus::Dirty));
        assert!(matches!(g.nodes[&c].status, NodeStatus::Dirty));
    }

    #[test]
    fn propagate_dirty_skips_running_and_error_nodes() {
        let a = Uuid::new_v4();
        let s = Uuid::new_v4();
        let mut g = graph_with(&[(a, s)], &[]);
        let bag = skills(&[(s, "1", "body")]);
        g.nodes.get_mut(&a).unwrap().status = NodeStatus::Running;
        propagate_dirty(a, &mut g, &bag).unwrap();
        assert!(matches!(g.nodes[&a].status, NodeStatus::Running));
    }

    #[test]
    fn topo_order_dirty_returns_kahn_order() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let s = Uuid::new_v4();
        let mut g = graph_with(&[(a, s), (b, s), (c, s)], &[(a, b), (b, c)]);
        for id in [a, b, c] {
            g.nodes.get_mut(&id).unwrap().status = NodeStatus::Dirty;
        }
        let order = topo_order_dirty(&g).unwrap();
        let pos = |x: &NodeId| order.iter().position(|y| y == x).unwrap();
        assert!(pos(&a) < pos(&b));
        assert!(pos(&b) < pos(&c));
    }

    #[test]
    fn topo_order_dirty_handles_diamond() {
        // a → b, a → c, b → d, c → d
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let d = Uuid::new_v4();
        let s = Uuid::new_v4();
        let mut g = graph_with(
            &[(a, s), (b, s), (c, s), (d, s)],
            &[(a, b), (a, c), (b, d), (c, d)],
        );
        for id in [a, b, c, d] {
            g.nodes.get_mut(&id).unwrap().status = NodeStatus::Dirty;
        }
        let order = topo_order_dirty(&g).unwrap();
        let pos = |x: &NodeId| order.iter().position(|y| y == x).unwrap();
        assert!(pos(&a) < pos(&b));
        assert!(pos(&a) < pos(&c));
        assert!(pos(&b) < pos(&d));
        assert!(pos(&c) < pos(&d));
    }

    #[test]
    fn topo_order_dirty_detects_cycle() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let s = Uuid::new_v4();
        let mut g = graph_with(&[(a, s), (b, s)], &[(a, b), (b, a)]);
        for id in [a, b] {
            g.nodes.get_mut(&id).unwrap().status = NodeStatus::Dirty;
        }
        let err = topo_order_dirty(&g).unwrap_err();
        assert_eq!(err.residual.len(), 2);
    }

    #[test]
    fn topo_order_dirty_skips_fresh_nodes_in_path() {
        // a (fresh) → b (dirty) → c (dirty)
        // b should come before c; a is excluded entirely.
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let s = Uuid::new_v4();
        let mut g = graph_with(&[(a, s), (b, s), (c, s)], &[(a, b), (b, c)]);
        g.nodes.get_mut(&a).unwrap().status = NodeStatus::Fresh;
        g.nodes.get_mut(&b).unwrap().status = NodeStatus::Dirty;
        g.nodes.get_mut(&c).unwrap().status = NodeStatus::Dirty;
        let order = topo_order_dirty(&g).unwrap();
        assert_eq!(order, vec![b, c]);
    }

    #[test]
    fn topo_order_dirty_empty_when_nothing_dirty() {
        let a = Uuid::new_v4();
        let s = Uuid::new_v4();
        let g = {
            let mut g = graph_with(&[(a, s)], &[]);
            g.nodes.get_mut(&a).unwrap().status = NodeStatus::Fresh;
            g
        };
        assert!(topo_order_dirty(&g).unwrap().is_empty());
    }

    #[test]
    fn canonical_json_sorts_object_keys() {
        let v1: serde_json::Value = serde_json::from_str(r#"{"b": 1, "a": 2}"#).unwrap();
        let v2: serde_json::Value = serde_json::from_str(r#"{"a": 2, "b": 1}"#).unwrap();
        assert_eq!(canonical_json(&v1), canonical_json(&v2));
        assert_eq!(canonical_json(&v1), r#"{"a":2,"b":1}"#);
    }

    #[test]
    fn hash_body_is_deterministic_and_distinct() {
        assert_eq!(hash_body("x"), hash_body("x"));
        assert_ne!(hash_body("x"), hash_body("y"));
        // sha256("") prefix sanity check:
        assert!(hash_body("").starts_with("e3b0c44"));
    }
}
