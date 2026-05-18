//! Artifact-tree walkers shared by the cascade orchestrator and the
//! workflow-canvas executor.
//!
//! The cascade orchestrator (`crate::plugins::artifact::cascade`) runs
//! skills tree-first: starting from one Artifact note, walking children
//! BFS, and firing every `input_kind`-matching skill on each visited
//! node. The workflow-canvas executor (`crate::plugins::workflow`)
//! runs skills graph-first: a hand-built DAG of skill nodes connected
//! by edges. Both code paths need to honor the same SkillContract
//! fields — in particular `aggregate: <kind>` (descend the artifact
//! tree under the source seed and inline every artifact of that kind)
//! and `inherit: <kind>` (walk the artifact tree's ancestor chain
//! upward and inline every sibling artifact of that kind).
//!
//! Keeping the helpers here means there's exactly one implementation
//! of each tree walk; both executors call into this module.
//!
//! Both helpers are read-only against `LocalNoteRepository` +
//! `Persistence` — no mutations, no plugin invocations.

#![cfg(not(target_arch = "wasm32"))]

use operon_store::repos::{LocalNote, LocalNoteRepository, NoteKind};
use std::collections::HashSet;
use std::sync::Arc;
use uuid::Uuid;

use crate::persistence::Persistence;
use crate::plugins::artifact::frontmatter::parse as parse_artifact_fm;

/// Aggregator helper: walk the descendants of `seed_id` under
/// `project_id` and return `(title, body)` for every Artifact note
/// whose `artifact_kind` matches `wanted_kind`. BFS, ordered by note
/// title so the prompt is deterministic across runs. Skips the seed
/// itself even if it happens to be the same kind (the seed body is
/// already inlined separately).
pub async fn collect_descendant_artifacts(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
    seed_id: Uuid,
    wanted_kind: &str,
) -> Vec<(String, String)> {
    let all = match note_repo.list_for_project(project_id) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut by_parent: std::collections::HashMap<Uuid, Vec<&LocalNote>> =
        std::collections::HashMap::new();
    for n in &all {
        if let Some(p) = n.parent_id {
            by_parent.entry(p).or_default().push(n);
        }
    }
    let mut visited = HashSet::new();
    let mut queue: std::collections::VecDeque<Uuid> = std::collections::VecDeque::new();
    queue.push_back(seed_id);
    visited.insert(seed_id);
    let mut matched: Vec<&LocalNote> = Vec::new();
    while let Some(id) = queue.pop_front() {
        if let Some(children) = by_parent.get(&id) {
            for child in children {
                if !visited.insert(child.id) {
                    continue;
                }
                if matches!(child.kind, NoteKind::Artifact) {
                    matched.push(child);
                }
                queue.push_back(child.id);
            }
        }
    }
    matched.sort_by(|a, b| a.title.cmp(&b.title));
    let mut out: Vec<(String, String)> = Vec::with_capacity(matched.len());
    for n in matched {
        let bytes = match persistence.load(&n.id.to_string()).await {
            Ok(b) => b,
            Err(_) => continue,
        };
        let body = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let fm = parse_artifact_fm(&body);
        let matches_kind = fm
            .artifact_kind
            .as_ref()
            .map(|k| k.as_str() == wanted_kind)
            .unwrap_or(false);
        if !matches_kind {
            continue;
        }
        out.push((n.title.clone(), body));
    }
    out
}

/// Inheritance helper: walk the **ancestor chain** from `source_id`
/// upward through `parent_id` links and collect `(title, body)` for
/// every Artifact note that is a child of one of those ancestors AND
/// whose `artifact_kind` matches `wanted_kind`. Excludes `source_id`
/// itself and any node already on the ancestor path. Stops at the
/// project root or after 32 hops (defensive cap against pathological
/// cycles). Used by skills that declare `inherit:` — e.g. an SDE skill
/// on a Task pulls the parent Story's LLD plan and the grandparent
/// Epic's HLD plan into its prompt context.
pub async fn collect_ancestor_sibling_artifacts(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
    source_id: Uuid,
    wanted_kind: &str,
) -> Vec<(String, String)> {
    let all = match note_repo.list_for_project(project_id) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let by_id: std::collections::HashMap<Uuid, &LocalNote> =
        all.iter().map(|n| (n.id, n)).collect();
    let mut by_parent: std::collections::HashMap<Uuid, Vec<&LocalNote>> =
        std::collections::HashMap::new();
    for n in &all {
        if let Some(p) = n.parent_id {
            by_parent.entry(p).or_default().push(n);
        }
    }

    // Walk parent_id chain from source upward.
    let mut ancestors: Vec<Uuid> = Vec::new();
    let mut current = by_id.get(&source_id).copied().and_then(|n| n.parent_id);
    let mut steps = 0;
    while let Some(p) = current {
        if !ancestors.contains(&p) {
            ancestors.push(p);
        }
        if steps > 32 {
            break;
        }
        current = by_id.get(&p).copied().and_then(|n| n.parent_id);
        steps += 1;
    }

    // For each ancestor, collect its Artifact-kind children whose
    // `artifact_kind` matches `wanted_kind`. Exclude the source and
    // anything else already on the ancestor path so the source's
    // direct lineage doesn't echo itself into the prompt.
    let mut visited: HashSet<Uuid> = HashSet::new();
    visited.insert(source_id);
    for a in &ancestors {
        visited.insert(*a);
    }
    let mut matched: Vec<&LocalNote> = Vec::new();
    for a in &ancestors {
        if let Some(children) = by_parent.get(a) {
            for c in children {
                if !visited.insert(c.id) {
                    continue;
                }
                if !matches!(c.kind, NoteKind::Artifact) {
                    continue;
                }
                matched.push(c);
            }
        }
    }
    matched.sort_by(|a, b| a.title.cmp(&b.title));

    let mut out: Vec<(String, String)> = Vec::with_capacity(matched.len());
    for n in matched {
        let bytes = match persistence.load(&n.id.to_string()).await {
            Ok(b) => b,
            Err(_) => continue,
        };
        let body = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let fm = parse_artifact_fm(&body);
        let matches_kind = fm
            .artifact_kind
            .as_ref()
            .map(|k| k.as_str() == wanted_kind)
            .unwrap_or(false);
        if !matches_kind {
            continue;
        }
        out.push((n.title.clone(), body));
    }
    out
}

/// One entry in a master-requirement subtree bundle: either a piece
/// of text the skill prompt should consume, or an image path the
/// model can `Read` for vision context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleEntry {
    /// Markdown body or nested master_requirement artifact body. Title
    /// is the note title.
    Text { title: String, body: String },
    /// Image attachment. `path` is whatever was stored on the note's
    /// `blob_path` column — typically vault-relative; callers join
    /// against the vault root if they need an absolute path.
    Image { title: String, path: std::path::PathBuf },
}

/// Walk the master-requirement subtree under `seed_id` depth-first
/// and collect a flat bundle of context. Children considered:
///
/// - `NoteKind::Markdown` → `Text`.
/// - `NoteKind::Artifact` whose frontmatter declares
///   `artifact_kind: master_requirement` → `Text`. Other artifact
///   kinds (epic / story / task / …) are skipped — those are
///   downstream outputs, not source material.
/// - `NoteKind::Image` → `Image` (`blob_path` as stored on the note
///   row).
/// - Every other kind is skipped.
///
/// Order is depth-first by `sibling_index`, mirroring the explorer
/// tree the user authored. Empty when the seed has no qualifying
/// descendants; safe to call on any note id (returns empty for
/// missing nodes / project mismatches).
pub async fn collect_master_req_subtree(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
    seed_id: Uuid,
) -> Vec<BundleEntry> {
    let all = match note_repo.list_for_project(project_id) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut by_parent: std::collections::HashMap<Uuid, Vec<&LocalNote>> =
        std::collections::HashMap::new();
    for n in &all {
        if let Some(p) = n.parent_id {
            by_parent.entry(p).or_default().push(n);
        }
    }
    // DFS pre-order. Children sorted by sibling_index so the user's
    // authored ordering shows up in the prompt. The stack stores
    // pending nodes in reverse so popping yields sibling_index order.
    let mut stack: Vec<&LocalNote> = Vec::new();
    if let Some(direct) = by_parent.get(&seed_id) {
        let mut direct_sorted = direct.clone();
        direct_sorted.sort_by_key(|n| n.sibling_index);
        for n in direct_sorted.into_iter().rev() {
            stack.push(n);
        }
    }
    let mut visited: HashSet<Uuid> = HashSet::new();
    let mut visit_order: Vec<&LocalNote> = Vec::new();
    while let Some(n) = stack.pop() {
        if !visited.insert(n.id) {
            continue;
        }
        visit_order.push(n);
        if let Some(children) = by_parent.get(&n.id) {
            let mut sorted = children.clone();
            sorted.sort_by_key(|c| c.sibling_index);
            for c in sorted.into_iter().rev() {
                stack.push(c);
            }
        }
    }
    let mut out: Vec<BundleEntry> = Vec::with_capacity(visit_order.len());
    for n in visit_order {
        match n.kind {
            NoteKind::Markdown => {
                let bytes = match persistence.load(&n.id.to_string()).await {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                let body = match String::from_utf8(bytes) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                out.push(BundleEntry::Text {
                    title: n.title.clone(),
                    body,
                });
            }
            NoteKind::Artifact => {
                // Only nested master_requirement artifacts qualify as
                // bundle text; epic / story / task / etc. are output
                // products, not source.
                let bytes = match persistence.load(&n.id.to_string()).await {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                let body = match String::from_utf8(bytes) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let fm = parse_artifact_fm(&body);
                let is_master_req = fm
                    .artifact_kind
                    .as_ref()
                    .map(|k| k.as_str() == "master_requirement")
                    .unwrap_or(false);
                if !is_master_req {
                    continue;
                }
                out.push(BundleEntry::Text {
                    title: n.title.clone(),
                    body,
                });
            }
            NoteKind::Image => {
                if let Some(path) = &n.blob_path {
                    out.push(BundleEntry::Image {
                        title: n.title.clone(),
                        path: std::path::PathBuf::from(path),
                    });
                }
            }
            _ => continue,
        }
    }
    out
}
