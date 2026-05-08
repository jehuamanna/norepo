//! Visualizes the autonomous SDLC cascade on the existing workflow
//! infinite canvas. As `cascade::run_cascade` produces artifacts, a
//! `CascadeGraphWriter` snapshots each one as a read-only Node into a
//! companion `NoteKind::Workflow` note titled `Cascade: <root title>`.
//! When the cascade finishes, a second pass parses `## Depends on`
//! sections from artifact bodies and adds cross-edges between
//! siblings.
//!
//! The writer is intentionally a thin façade over `WorkflowGraph`:
//! we reuse the existing serde, layout, and SVG renderer in
//! `src/plugins/workflow/`. The marker fields `is_artifact_snapshot`,
//! `artifact_ref`, `artifact_kind_label` (added in this phase) tell
//! the workflow view to render these as compact read-only cards
//! instead of editable skill nodes; the `edge_kind` field on Edge
//! distinguishes parent→child edges (black) from Depends-on cross-
//! edges (amber).

#![cfg(not(target_arch = "wasm32"))]

use operon_store::repos::{LocalNoteRepository, NoteKind};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::persistence::Persistence;
use crate::plugins::artifact::frontmatter::parse as parse_artifact_fm;
use crate::plugins::workflow::state::{Edge, Node, NodeStatus, WorkflowGraph};

/// Companion-graph writer driven by the cascade orchestrator. One
/// instance per cascade run. Methods are async (because they save
/// the underlying note's body to persistence on each commit), but
/// the in-memory graph is held in a regular field — single-threaded
/// access from the orchestrator's loop, so no Mutex needed.
pub struct CascadeGraphWriter {
    /// Note id of the `Cascade: <root>` workflow note we're updating.
    pub graph_note_id: Uuid,
    /// In-memory graph; persisted to `graph_note_id`'s body on each
    /// `flush()`. Loaded once from disk if the note already exists
    /// (re-run scenario).
    pub graph: WorkflowGraph,
    /// Map of artifact note id → workflow-graph node id, so we can
    /// look up the snapshot node for a given artifact when adding
    /// edges (parent→child or Depends-on).
    pub by_artifact: HashMap<Uuid, Uuid>,
    /// Cached body text per artifact id, populated as the writer
    /// records nodes. Used by the Depends-on second pass to find
    /// sibling references without re-loading from disk.
    pub bodies: HashMap<Uuid, String>,
}

impl CascadeGraphWriter {
    /// Allocate a new writer keyed on an existing or freshly-minted
    /// `Cascade:` workflow note. The caller is responsible for
    /// creating the note row (via `note_repo.create_with_kind`) and
    /// passing the resulting `graph_note_id`. If a graph already
    /// exists at that body (re-run), it's loaded so we extend
    /// rather than overwrite.
    pub async fn new_or_load(
        graph_note_id: Uuid,
        persistence: &Arc<dyn Persistence>,
    ) -> Self {
        let graph = match persistence.load(&graph_note_id.to_string()).await {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) => serde_json::from_str::<WorkflowGraph>(&text)
                    .unwrap_or_else(|_| WorkflowGraph::new()),
                Err(_) => WorkflowGraph::new(),
            },
            Err(_) => WorkflowGraph::new(),
        };
        // Index existing artifact-snapshot nodes so re-runs reuse
        // them instead of duplicating.
        let mut by_artifact = HashMap::new();
        for (nid, node) in graph.nodes.iter() {
            if let Some(art) = node.artifact_ref {
                by_artifact.insert(art, *nid);
            }
        }
        Self {
            graph_note_id,
            graph,
            by_artifact,
            bodies: HashMap::new(),
        }
    }

    /// Record one produced artifact. Adds (or refreshes) a snapshot
    /// node for `child_id` and an edge from the parent's snapshot
    /// node to the child's. The node's position is auto-laid out
    /// based on its parent + sibling ordering. The artifact's body
    /// text is cached so the Depends-on pass doesn't re-load it.
    pub fn on_artifact_produced(
        &mut self,
        parent_artifact_id: Uuid,
        child_artifact_id: Uuid,
        child_title: &str,
        child_body: String,
    ) {
        // Decode kind off the child's frontmatter for the badge.
        let fm = parse_artifact_fm(&child_body);
        let kind_label = fm
            .artifact_kind
            .as_ref()
            .map(|k| k.display_name())
            .unwrap_or_else(|| "Artifact".into());
        self.bodies.insert(child_artifact_id, child_body);

        // Ensure parent is represented as a snapshot too — the root
        // (a Requirements artifact, typically) lands as a node so
        // edges have somewhere to start. This is idempotent on
        // re-runs since `by_artifact` dedups.
        let parent_node_id = self.ensure_snapshot_for(parent_artifact_id, None);

        // Allocate / reuse a node id for the child.
        let child_node_id = self.ensure_snapshot_for(child_artifact_id, Some(kind_label.clone()));

        // Cache title + kind label on the child node so the canvas
        // tile renders without a per-paint LocalNoteRepository lookup.
        if let Some(n) = self.graph.nodes.get_mut(&child_node_id) {
            n.artifact_kind_label = Some(kind_label);
            n.artifact_title = Some(child_title.to_string());
        }

        // Auto-layout: walk the children of the parent so far, count
        // them, place this child to the right of the previous
        // sibling, one row below the parent.
        let level = self.level_of(parent_node_id) + 1;
        let sibling_index = self.count_children(parent_node_id);
        let pos = (
            f64::from(sibling_index as i32) * NODE_X_SPACING,
            f64::from(level as i32) * NODE_Y_SPACING,
        );
        if let Some(n) = self.graph.nodes.get_mut(&child_node_id) {
            n.position = pos;
        }

        // Add the parent→child edge if it doesn't already exist.
        let already = self
            .graph
            .edges
            .iter()
            .any(|e| e.from == parent_node_id && e.to == child_node_id);
        if !already {
            self.graph.edges.push(Edge {
                id: Uuid::new_v4(),
                from: parent_node_id,
                from_socket: "default".into(),
                to: child_node_id,
                to_socket: "default".into(),
                edge_kind: Some("parent_child".into()),
            });
        }
        self.graph.version = self.graph.version.saturating_add(1);
    }

    /// Allocate (or look up) the snapshot node for an artifact id.
    /// `kind_label_hint` lets the caller pre-populate the badge for
    /// freshly-allocated nodes.
    fn ensure_snapshot_for(
        &mut self,
        artifact_id: Uuid,
        kind_label_hint: Option<String>,
    ) -> Uuid {
        if let Some(nid) = self.by_artifact.get(&artifact_id) {
            return *nid;
        }
        let nid = Uuid::new_v4();
        self.graph.nodes.insert(
            nid,
            Node {
                id: nid,
                skill_note_id: Uuid::nil(),
                typed_fields: serde_json::Value::Null,
                extra_instructions: String::new(),
                position: (0.0, 0.0),
                cached_output_path: None,
                cached_input_hash: None,
                status: NodeStatus::Fresh,
                cached_output_note_id: None,
                is_artifact_snapshot: true,
                artifact_ref: Some(artifact_id),
                artifact_kind_label: kind_label_hint,
                artifact_title: None,
            },
        );
        self.by_artifact.insert(artifact_id, nid);
        nid
    }

    /// Count direct children of a node (via parent_child edges).
    fn count_children(&self, node_id: Uuid) -> usize {
        self.graph
            .edges
            .iter()
            .filter(|e| {
                e.from == node_id
                    && e.edge_kind.as_deref() == Some("parent_child")
            })
            .count()
    }

    /// BFS depth of a node from any root. Used for layout. Returns 0
    /// for nodes with no incoming parent_child edges.
    fn level_of(&self, node_id: Uuid) -> u32 {
        let mut level: u32 = 0;
        let mut current = node_id;
        let mut visited = std::collections::HashSet::new();
        loop {
            if !visited.insert(current) {
                break; // cycle guard
            }
            let parent = self
                .graph
                .edges
                .iter()
                .find(|e| {
                    e.to == current
                        && e.edge_kind.as_deref() == Some("parent_child")
                })
                .map(|e| e.from);
            match parent {
                Some(p) => {
                    current = p;
                    level += 1;
                }
                None => break,
            }
        }
        level
    }

    /// Second pass: walk every artifact body we've cached, parse its
    /// `## Depends on` section, and add an amber cross-edge from
    /// each referenced sibling's snapshot node to this artifact's.
    /// Tolerant — references that don't resolve to a known artifact
    /// are silently dropped.
    pub fn finalize_depends_on(&mut self, all_titles_by_artifact: &HashMap<Uuid, String>) {
        // Index titles → artifact_id so a "Depends on: T001" or
        // "Depends on: feature-01-foo" can resolve regardless of
        // whether the user wrote the file stem or a TaskID prefix.
        let mut by_title: HashMap<String, Uuid> = HashMap::new();
        for (art, title) in all_titles_by_artifact {
            by_title.insert(title.clone(), *art);
            // Also index just the leading TaskID token (T001 etc.)
            // by grabbing the first whitespace-delimited word in the
            // title — handles "T001 — Add user table" body titles.
            if let Some(first) = title.split_whitespace().next() {
                by_title.insert(first.to_string(), *art);
            }
        }

        let bodies: Vec<(Uuid, String)> = self
            .bodies
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        for (artifact_id, body) in bodies {
            for slug in parse_depends_on(&body) {
                let dep_artifact = match by_title.get(&slug) {
                    Some(a) => *a,
                    None => continue, // unresolved reference
                };
                let from = match self.by_artifact.get(&dep_artifact) {
                    Some(n) => *n,
                    None => continue,
                };
                let to = match self.by_artifact.get(&artifact_id) {
                    Some(n) => *n,
                    None => continue,
                };
                if from == to {
                    continue; // self-loop guard
                }
                let already = self.graph.edges.iter().any(|e| {
                    e.from == from
                        && e.to == to
                        && e.edge_kind.as_deref() == Some("depends_on")
                });
                if !already {
                    self.graph.edges.push(Edge {
                        id: Uuid::new_v4(),
                        from,
                        from_socket: "default".into(),
                        to,
                        to_socket: "default".into(),
                        edge_kind: Some("depends_on".into()),
                    });
                }
            }
        }
        self.graph.version = self.graph.version.saturating_add(1);
    }

    /// Serialize the in-memory graph to the workflow note's body.
    /// Called by the orchestrator at every level boundary so the
    /// canvas re-renders live.
    pub async fn flush(&self, persistence: &Arc<dyn Persistence>) -> std::io::Result<()> {
        let body = serde_json::to_string_pretty(&self.graph)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        persistence
            .save(&self.graph_note_id.to_string(), body.as_bytes())
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("{e}")))?;
        Ok(())
    }
}

/// Approximate per-node spacing used by the auto-layout. Picked so
/// the resulting tree is readable at the canvas's default zoom; not
/// a hard constraint — the user can drag nodes after the fact.
const NODE_X_SPACING: f64 = 180.0;
const NODE_Y_SPACING: f64 = 140.0;

/// Find or create the `Cascade: <root>` workflow note for a cascade
/// run. The note is parented under the project root (no
/// `parent_id`), matches by title (so re-runs reuse the same note).
/// Returns `(note_id, was_created)` so callers can seed a fresh
/// note's body without re-clobbering an existing user-edited graph.
pub fn ensure_cascade_workflow_note(
    note_repo: &Arc<dyn LocalNoteRepository>,
    project_id: Uuid,
    root_title: &str,
) -> Result<(Uuid, bool), String> {
    let title = format!("Cascade: {root_title}");
    let existing = note_repo
        .list_for_project(project_id)
        .map_err(|e| format!("list_for_project: {e}"))?
        .into_iter()
        .find(|n| matches!(n.kind, NoteKind::Workflow) && n.title == title)
        .map(|n| n.id);
    if let Some(id) = existing {
        return Ok((id, false));
    }
    let row = note_repo
        .create_with_kind(project_id, None, &title, NoteKind::Workflow)
        .map_err(|e| format!("create_with_kind: {e}"))?;
    Ok((row.id, true))
}

/// Strip the leading numeric prefix off a skill-note title (e.g.
/// `"01-ba-discover-epics"` → `Some(1)`). Returns `None` when the
/// title doesn't start with at least one ASCII digit — those skills
/// are excluded from the auto-seeded pipeline.
///
/// Accepts any number of leading digits, with or without a trailing
/// separator: `"7-foo"`, `"07-foo"`, `"10-foo"`, and `"3 foo"` all
/// parse. Non-numbered titles like `"my-custom-skill"` are skipped
/// so users can install ad-hoc skills without polluting the default
/// flow.
pub fn parse_pipeline_order(title: &str) -> Option<u32> {
    let digits: String = title.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<u32>().ok()
}

/// Seed a freshly-created `Cascade:` workflow note with the project's
/// natural pipeline of skills. Discovery rule: every project skill
/// whose title begins with a numeric prefix (e.g. `01-ba-discover-epics`,
/// `02-ba-decompose-features`, …) is included; ordering is by that
/// numeric prefix. Skills without a numeric prefix are ignored — the
/// user can still pick them via the `+ Add skill node` button.
///
/// Bails without writing when no numbered skills exist yet (fresh
/// project, user hasn't installed the seed skills). Partial seeds
/// are fine: if only `01-` and `02-` are installed, the seeded graph
/// is two nodes connected by one edge.
pub async fn seed_natural_pipeline(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
    graph_note_id: Uuid,
) -> Result<(), String> {
    let project_skills: Vec<_> = note_repo
        .list_for_project(project_id)
        .map_err(|e| format!("list_for_project: {e}"))?
        .into_iter()
        .filter(|n| matches!(n.kind, NoteKind::Skill))
        .collect();
    let graph = append_numbered_skill_chain(WorkflowGraph::new(), &project_skills);
    if graph.nodes.is_empty() {
        return Ok(());
    }
    let body = serde_json::to_string_pretty(&graph)
        .map_err(|e| format!("serialize seeded graph: {e}"))?;
    persistence
        .save(&graph_note_id.to_string(), body.as_bytes())
        .await
        .map_err(|e| format!("save seeded graph: {e}"))?;
    Ok(())
}

/// Append every numbered skill in `project_skills` as a Dirty node to
/// `graph`, ordered by the numeric prefix on the skill's title, and
/// connect them with `parent_child` edges so `Run all dirty` walks the
/// chain in order. The new chain is laid out in a fresh row below
/// any existing nodes so the user's hand-built work isn't disturbed.
///
/// Returns `graph` unchanged when no project skills carry a numeric
/// prefix (nothing to seed). Bumps `graph.version` exactly once when
/// any nodes are appended.
pub fn append_numbered_skill_chain(
    mut graph: WorkflowGraph,
    project_skills: &[operon_store::repos::LocalNote],
) -> WorkflowGraph {
    let mut numbered: Vec<(u32, &operon_store::repos::LocalNote)> = project_skills
        .iter()
        .filter_map(|n| parse_pipeline_order(&n.title).map(|order| (order, n)))
        .collect();
    if numbered.is_empty() {
        return graph;
    }
    numbered.sort_by_key(|(order, _)| *order);

    // Place the new chain one row below the deepest existing node so
    // re-clicking "Seed pipeline" with hand-edited content above just
    // appends fresh chains instead of overwriting existing work.
    let row_y = graph
        .nodes
        .values()
        .map(|n| n.position.1)
        .fold(0.0_f64, f64::max);
    let next_y = if graph.nodes.is_empty() {
        40.0
    } else {
        row_y + 160.0
    };

    let mut prev: Option<Uuid> = None;
    for (i, (_order, skill)) in numbered.iter().enumerate() {
        let nid = Uuid::new_v4();
        graph.nodes.insert(
            nid,
            Node {
                id: nid,
                skill_note_id: skill.id,
                typed_fields: serde_json::Value::Null,
                extra_instructions: String::new(),
                position: (40.0 + (i as f64) * 220.0, next_y),
                cached_output_path: None,
                cached_input_hash: None,
                status: NodeStatus::Dirty,
                cached_output_note_id: None,
                is_artifact_snapshot: false,
                artifact_ref: None,
                artifact_kind_label: None,
                artifact_title: None,
            },
        );
        if let Some(from) = prev {
            graph.edges.push(Edge {
                id: Uuid::new_v4(),
                from,
                from_socket: "default".into(),
                to: nid,
                to_socket: "default".into(),
                edge_kind: None,
            });
        }
        prev = Some(nid);
    }
    graph.version = graph.version.saturating_add(1);
    graph
}

/// Extract the slugs / TaskIDs listed under a `## Depends on`
/// heading in an artifact body. Tolerant: returns an empty Vec when
/// the section is absent, malformed, or the body has no headings.
///
/// Recognizes any of:
/// ```markdown
/// ## Depends on
/// - T001
/// - feature-01-account-creation
/// - story-02-handle-duplicate (some annotation)
/// ```
///
/// Each bullet's first token (whitespace-delimited, stripped of
/// trailing punctuation) becomes a slug. Annotations after the
/// first whitespace are dropped — it's the LOOKUP key, not the
/// human label.
pub fn parse_depends_on(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut in_section = false;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("##") {
            // New heading — toggle in_section based on whether it's
            // the depends-on heading.
            let heading = trimmed.trim_start_matches('#').trim().to_lowercase();
            in_section = heading == "depends on" || heading == "dependencies";
            continue;
        }
        if !in_section {
            continue;
        }
        // Bullet line: "- foo" or "* foo" or "+ foo"
        let bullet = trimmed.trim_start_matches(['-', '*', '+']).trim_start();
        if bullet == trimmed {
            continue; // not a bullet
        }
        // First whitespace-delimited token, stripped of trailing
        // punctuation that often follows the slug ("," or ".").
        let token = bullet
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_end_matches([',', '.', ':', ';']);
        if token.is_empty() || token.eq_ignore_ascii_case("none") {
            continue;
        }
        out.push(token.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_depends_on_finds_simple_bullets() {
        let body = "# Task: T003\n\n## Depends on\n- T001\n- T002\n\n## Other\n";
        let deps = parse_depends_on(body);
        assert_eq!(deps, vec!["T001", "T002"]);
    }

    #[test]
    fn parse_depends_on_ignores_none_marker() {
        let body = "## Depends on\n- None (parallel-safe)\n";
        let deps = parse_depends_on(body);
        assert!(deps.is_empty());
    }

    #[test]
    fn parse_depends_on_handles_dependencies_heading_alias() {
        let body = "## Dependencies\n- foo-01\n- bar-02\n";
        let deps = parse_depends_on(body);
        assert_eq!(deps, vec!["foo-01", "bar-02"]);
    }

    #[test]
    fn parse_depends_on_drops_annotations_after_slug() {
        let body = "## Depends on\n- T001 (added as a fixture)\n- T002, optional\n";
        let deps = parse_depends_on(body);
        assert_eq!(deps, vec!["T001", "T002"]);
    }

    #[test]
    fn parse_depends_on_empty_when_no_section() {
        let body = "# Task: T003\n\nNo section here.\n";
        assert!(parse_depends_on(body).is_empty());
    }

    #[test]
    fn parse_pipeline_order_extracts_zero_padded_prefix() {
        assert_eq!(parse_pipeline_order("01-ba-discover-epics"), Some(1));
        assert_eq!(parse_pipeline_order("07-sde-implement-task"), Some(7));
        assert_eq!(parse_pipeline_order("10-sum-summarize-task"), Some(10));
    }

    #[test]
    fn parse_pipeline_order_extracts_unpadded_prefix() {
        assert_eq!(parse_pipeline_order("3-foo"), Some(3));
        assert_eq!(parse_pipeline_order("100-bar"), Some(100));
    }

    #[test]
    fn parse_pipeline_order_returns_none_for_unprefixed_titles() {
        assert_eq!(parse_pipeline_order("ba-discover-epics"), None);
        assert_eq!(parse_pipeline_order("my-custom-skill"), None);
        assert_eq!(parse_pipeline_order("-leading-dash"), None);
        assert_eq!(parse_pipeline_order(""), None);
    }
}
