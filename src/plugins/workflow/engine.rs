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
    Cycle(CycleError),
    /// Catch-all for run-time failures inside `run_dirty_levels_parallel` —
    /// node closures returning errors, tokio join failures, etc.
    NodeRun(String),
    Other(String),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownSkill(id) => write!(f, "no SkillSnapshot for skill_note_id {id}"),
            Self::UnknownNode(id) => write!(f, "no node with id {id} in graph"),
            Self::Cycle(c) => write!(f, "{c}"),
            Self::NodeRun(msg) => write!(f, "node run failed: {msg}"),
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for EngineError {}

impl From<CycleError> for EngineError {
    fn from(c: CycleError) -> Self {
        Self::Cycle(c)
    }
}

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

/// Topologically group the currently-dirty nodes into depth-levels
/// (a.k.a. "waves"). Every node in level `i` has all its dirty
/// predecessors in levels `< i`, so all nodes inside one level can run
/// concurrently. Returns `CycleError` on any cycle through the dirty subset.
///
/// Slice B6 — used by the cascade runner to fan out within a level via
/// `tokio::spawn` (bounded by a `Semaphore`) and `join_all` before
/// advancing to the next level.
///
/// Mirrors `topo_order_dirty`'s edge-restriction policy: only edges
/// between two dirty endpoints contribute to the indegree count, so a
/// node whose only predecessors are clean lands at level 0.
pub fn topo_levels_dirty(graph: &WorkflowGraph) -> Result<Vec<Vec<NodeId>>, CycleError> {
    let dirty: BTreeSet<NodeId> = graph
        .nodes
        .iter()
        .filter(|(_, n)| matches!(n.status, NodeStatus::Dirty))
        .map(|(id, _)| *id)
        .collect();
    if dirty.is_empty() {
        return Ok(Vec::new());
    }

    let mut indeg: BTreeMap<NodeId, usize> = dirty.iter().map(|id| (*id, 0usize)).collect();
    let mut adj: BTreeMap<NodeId, Vec<NodeId>> = BTreeMap::new();
    for e in &graph.edges {
        if dirty.contains(&e.from) && dirty.contains(&e.to) {
            adj.entry(e.from).or_default().push(e.to);
            *indeg.entry(e.to).or_default() += 1;
        }
    }

    // Stable level construction: snapshot the current zero-indegree set,
    // emit it as one level, then decrement neighbours and repeat.
    let mut levels: Vec<Vec<NodeId>> = Vec::new();
    let mut placed: BTreeSet<NodeId> = BTreeSet::new();
    loop {
        // Zero-indeg nodes that haven't been placed yet — sorted so output
        // is deterministic across runs.
        let mut current: Vec<NodeId> = indeg
            .iter()
            .filter(|(id, d)| **d == 0 && !placed.contains(id))
            .map(|(id, _)| *id)
            .collect();
        if current.is_empty() {
            break;
        }
        current.sort();
        for n in &current {
            placed.insert(*n);
            if let Some(nexts) = adj.get(n) {
                for next in nexts {
                    if let Some(d) = indeg.get_mut(next) {
                        *d -= 1;
                    }
                }
            }
        }
        levels.push(current);
    }

    if placed.len() != dirty.len() {
        let residual: BTreeSet<NodeId> = dirty.difference(&placed).copied().collect();
        return Err(CycleError { residual });
    }
    Ok(levels)
}

/// Drive a workflow's dirty nodes through `topo_levels_dirty`, fanning out
/// each level's nodes concurrently up to `max_concurrent` at a time, then
/// `join_all` before advancing.
///
/// The caller supplies `run_node` — a closure that knows how to run a
/// single node's skill against the agent runtime. We don't reach into the
/// cascade orchestrator to keep the engine layer dependency-free.
///
/// **Failure semantics**: by default, one node failure aborts the rest of
/// the *current* level (other in-flight nodes are awaited but their errors
/// are surfaced via the next return) and no further levels are scheduled.
/// Set `continue_on_error: true` to keep going past failures within a level.
///
/// Returns the list of node ids that completed successfully, in the order
/// they finished.
///
/// **Slice B6** primitive — dead code until the cascade runner adopts it
/// (Slice A14 follow-up).
#[cfg(not(target_arch = "wasm32"))]
pub async fn run_dirty_levels_parallel<F>(
    graph: &WorkflowGraph,
    max_concurrent: usize,
    continue_on_error: bool,
    run_node: F,
) -> Result<Vec<NodeId>, EngineError>
where
    F: Fn(
            NodeId,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), EngineError>> + Send>,
        > + Send
        + Sync
        + 'static,
{
    let levels = topo_levels_dirty(graph)?;
    if levels.is_empty() {
        return Ok(Vec::new());
    }
    let max_concurrent = max_concurrent.max(1);
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(max_concurrent));
    let run_node = std::sync::Arc::new(run_node);

    let mut completed: Vec<NodeId> = Vec::new();

    for level in levels {
        let mut handles = Vec::with_capacity(level.len());
        for id in level {
            let permit = semaphore
                .clone()
                .acquire_owned()
                .await
                .expect("semaphore closed unexpectedly");
            let run_node = run_node.clone();
            handles.push((
                id,
                tokio::spawn(async move {
                    let _permit = permit;
                    run_node(id).await
                }),
            ));
        }
        let mut level_failed: Option<EngineError> = None;
        for (id, h) in handles {
            match h.await {
                Ok(Ok(())) => completed.push(id),
                Ok(Err(e)) => {
                    if level_failed.is_none() {
                        level_failed = Some(e);
                    }
                }
                Err(join_err) => {
                    if level_failed.is_none() {
                        level_failed = Some(EngineError::NodeRun(format!(
                            "join error on node {id}: {join_err}"
                        )));
                    }
                }
            }
        }
        if let Some(err) = level_failed {
            if !continue_on_error {
                return Err(err);
            }
        }
    }
    Ok(completed)
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

    // ---- topo_levels_dirty (Slice B6) ----

    #[test]
    fn topo_levels_dirty_groups_concurrent_siblings() {
        // a → c, b → c. a and b share level 0; c is level 1.
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let s = Uuid::new_v4();
        let mut g = graph_with(&[(a, s), (b, s), (c, s)], &[(a, c), (b, c)]);
        for id in [a, b, c] {
            g.nodes.get_mut(&id).unwrap().status = NodeStatus::Dirty;
        }
        let levels = topo_levels_dirty(&g).unwrap();
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[0].len(), 2, "a and b run concurrently at depth 0");
        assert!(levels[0].contains(&a));
        assert!(levels[0].contains(&b));
        assert_eq!(levels[1], vec![c]);
    }

    #[test]
    fn topo_levels_dirty_diamond_has_three_levels() {
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
        let levels = topo_levels_dirty(&g).unwrap();
        assert_eq!(levels.len(), 3);
        assert_eq!(levels[0], vec![a]);
        assert_eq!(levels[1].len(), 2);
        assert!(levels[1].contains(&b) && levels[1].contains(&c));
        assert_eq!(levels[2], vec![d]);
    }

    #[test]
    fn topo_levels_dirty_chain_each_node_its_own_level() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let s = Uuid::new_v4();
        let mut g = graph_with(&[(a, s), (b, s), (c, s)], &[(a, b), (b, c)]);
        for id in [a, b, c] {
            g.nodes.get_mut(&id).unwrap().status = NodeStatus::Dirty;
        }
        let levels = topo_levels_dirty(&g).unwrap();
        assert_eq!(levels, vec![vec![a], vec![b], vec![c]]);
    }

    #[test]
    fn topo_levels_dirty_empty_when_nothing_dirty() {
        let a = Uuid::new_v4();
        let s = Uuid::new_v4();
        let mut g = graph_with(&[(a, s)], &[]);
        g.nodes.get_mut(&a).unwrap().status = NodeStatus::Fresh;
        assert!(topo_levels_dirty(&g).unwrap().is_empty());
    }

    #[test]
    fn topo_levels_dirty_detects_cycle() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let s = Uuid::new_v4();
        let mut g = graph_with(&[(a, s), (b, s)], &[(a, b), (b, a)]);
        for id in [a, b] {
            g.nodes.get_mut(&id).unwrap().status = NodeStatus::Dirty;
        }
        let err = topo_levels_dirty(&g).unwrap_err();
        assert_eq!(err.residual.len(), 2);
    }

    #[test]
    fn topo_levels_dirty_clean_predecessor_doesnt_delay_dirty_node() {
        // a (fresh) → b (dirty) → c (dirty). b should land at level 0
        // because the dirty-restricted indegree is zero.
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let s = Uuid::new_v4();
        let mut g = graph_with(&[(a, s), (b, s), (c, s)], &[(a, b), (b, c)]);
        g.nodes.get_mut(&a).unwrap().status = NodeStatus::Fresh;
        g.nodes.get_mut(&b).unwrap().status = NodeStatus::Dirty;
        g.nodes.get_mut(&c).unwrap().status = NodeStatus::Dirty;
        let levels = topo_levels_dirty(&g).unwrap();
        assert_eq!(levels, vec![vec![b], vec![c]]);
    }

    #[test]
    fn topo_levels_dirty_total_node_count_matches_topo_order() {
        // Both functions should agree on which nodes are emitted.
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
        let levels = topo_levels_dirty(&g).unwrap();
        let flat: Vec<NodeId> = levels.into_iter().flatten().collect();
        assert_eq!(flat.len(), order.len());
        assert_eq!(flat.iter().collect::<BTreeSet<_>>(), order.iter().collect::<BTreeSet<_>>());
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

    // ---- run_dirty_levels_parallel (Slice B6) ----

    #[tokio::test]
    async fn run_levels_parallel_visits_every_dirty_node() {
        // Diamond a → b, a → c, b → d, c → d. Each node calls its closure;
        // we assert all four ran exactly once.
        use std::sync::atomic::{AtomicUsize, Ordering};
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
        let counter = std::sync::Arc::new(AtomicUsize::new(0));
        let counter_for_closure = counter.clone();
        let completed = run_dirty_levels_parallel(&g, 4, false, move |_id| {
            let counter_for_closure = counter_for_closure.clone();
            Box::pin(async move {
                counter_for_closure.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        })
        .await
        .unwrap();
        assert_eq!(completed.len(), 4);
        assert_eq!(counter.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn run_levels_parallel_aborts_on_failure_by_default() {
        // Two nodes at level 0 (a, b), one at level 1 (c, depending on a).
        // Make `a` fail; `c` should never run.
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let s = Uuid::new_v4();
        let mut g = graph_with(&[(a, s), (b, s), (c, s)], &[(a, c)]);
        for id in [a, b, c] {
            g.nodes.get_mut(&id).unwrap().status = NodeStatus::Dirty;
        }
        let a_id = a;
        let c_ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let c_ran_for_closure = c_ran.clone();
        let res = run_dirty_levels_parallel(&g, 2, false, move |id| {
            let c_ran = c_ran_for_closure.clone();
            Box::pin(async move {
                if id == a_id {
                    Err(EngineError::NodeRun("boom".into()))
                } else if id == c {
                    c_ran.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                } else {
                    Ok(())
                }
            })
        })
        .await;
        assert!(matches!(res, Err(EngineError::NodeRun(_))));
        assert!(!c_ran.load(std::sync::atomic::Ordering::SeqCst), "c must not run after a's failure");
    }

    #[tokio::test]
    async fn run_levels_parallel_continue_on_error_skips_failed_node() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let s = Uuid::new_v4();
        let mut g = graph_with(&[(a, s), (b, s)], &[]);
        for id in [a, b] {
            g.nodes.get_mut(&id).unwrap().status = NodeStatus::Dirty;
        }
        let a_id = a;
        let completed = run_dirty_levels_parallel(&g, 2, true, move |id| {
            Box::pin(async move {
                if id == a_id {
                    Err(EngineError::NodeRun("fail".into()))
                } else {
                    Ok(())
                }
            })
        })
        .await
        .unwrap();
        // b succeeds; a fails — completed has only b.
        assert_eq!(completed.len(), 1);
    }

    #[tokio::test]
    async fn run_levels_parallel_respects_concurrency_cap() {
        // 6 nodes at level 0; with max_concurrent = 2, no more than 2 run
        // simultaneously. We track current vs peak.
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;
        let s = Uuid::new_v4();
        let ids: Vec<Uuid> = (0..6).map(|_| Uuid::new_v4()).collect();
        let mut entries: Vec<(Uuid, Uuid)> = ids.iter().map(|i| (*i, s)).collect();
        // No edges → all at level 0.
        let mut g = graph_with(&entries.as_slice(), &[]);
        for id in &ids {
            g.nodes.get_mut(id).unwrap().status = NodeStatus::Dirty;
        }
        let _ = entries.drain(..);

        let current = std::sync::Arc::new(AtomicUsize::new(0));
        let peak = std::sync::Arc::new(AtomicUsize::new(0));
        let current_c = current.clone();
        let peak_c = peak.clone();
        let _ = run_dirty_levels_parallel(&g, 2, false, move |_id| {
            let current = current_c.clone();
            let peak = peak_c.clone();
            Box::pin(async move {
                let n = current.fetch_add(1, Ordering::SeqCst) + 1;
                let mut p = peak.load(Ordering::SeqCst);
                while p < n {
                    match peak.compare_exchange(p, n, Ordering::SeqCst, Ordering::SeqCst) {
                        Ok(_) => break,
                        Err(actual) => p = actual,
                    }
                }
                tokio::time::sleep(Duration::from_millis(40)).await;
                current.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            })
        })
        .await
        .unwrap();
        assert!(
            peak.load(Ordering::SeqCst) <= 2,
            "expected peak ≤ 2 with max_concurrent=2, got {}",
            peak.load(Ordering::SeqCst)
        );
    }

    #[tokio::test]
    async fn run_levels_parallel_empty_graph_returns_empty() {
        let a = Uuid::new_v4();
        let s = Uuid::new_v4();
        let mut g = graph_with(&[(a, s)], &[]);
        g.nodes.get_mut(&a).unwrap().status = NodeStatus::Fresh;
        let completed = run_dirty_levels_parallel(&g, 2, false, |_id| Box::pin(async { Ok(()) }))
            .await
            .unwrap();
        assert!(completed.is_empty());
    }

    #[tokio::test]
    async fn run_levels_parallel_propagates_cycle_error() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let s = Uuid::new_v4();
        let mut g = graph_with(&[(a, s), (b, s)], &[(a, b), (b, a)]);
        for id in [a, b] {
            g.nodes.get_mut(&id).unwrap().status = NodeStatus::Dirty;
        }
        let res = run_dirty_levels_parallel(&g, 2, false, |_id| Box::pin(async { Ok(()) })).await;
        assert!(matches!(res, Err(EngineError::Cycle(_))));
    }
}
