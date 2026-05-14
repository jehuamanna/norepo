//! Phase-tree resolution helpers used by the cascade orchestrator to
//! decide whether a given artifact should trigger phase-scoped skills
//! (e.g. the architecture skill, which only fires for the first
//! phase).
//!
//! Two operations:
//!   - `ancestor_phase_id`: walk a note's `parent_id` chain looking
//!     for the nearest `NoteKind::Phase`. Synchronous (only needs the
//!     note metadata, not bodies).
//!   - `first_phase_id`: enumerate every `NoteKind::Phase` in the
//!     project, load each body to read its `phase_order`, sort by
//!     `(phase_order.unwrap_or(MAX), created_at_ms)`, return the
//!     winner. Async because phase frontmatter lives in the note
//!     body.
//!
//! Legacy projects with no phase notes return `None` from
//! `first_phase_id`; callers treat that as "no gating, allow
//! everything" so existing cascades keep working.

use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use operon_store::repos::{LocalNote, LocalNoteRepository, NoteKind};

use crate::persistence::Persistence;
use crate::plugins::phase::frontmatter;

/// Walk `parent_id` from `start_id` until we hit a `NoteKind::Phase`
/// ancestor. Caps depth at 32 to avoid pathological cycles. Returns
/// the phase note's id, or `None` if no phase ancestor exists.
pub fn ancestor_phase_id(
    note_repo: &Arc<dyn LocalNoteRepository>,
    project_id: Uuid,
    start_id: Uuid,
) -> Option<Uuid> {
    let notes = note_repo.list_for_project(project_id).ok()?;
    let by_id: HashMap<Uuid, &LocalNote> = notes.iter().map(|n| (n.id, n)).collect();
    let mut cursor = by_id.get(&start_id).copied().and_then(|n| n.parent_id);
    let mut steps = 0;
    while let Some(id) = cursor {
        if steps > 32 {
            return None;
        }
        steps += 1;
        let node = by_id.get(&id).copied()?;
        if matches!(node.kind, NoteKind::Phase) {
            return Some(id);
        }
        cursor = node.parent_id;
    }
    None
}

/// Return the phase id that ranks first in `project_id`, by
/// `(phase_order, created_at_ms)` ascending. Phases with no explicit
/// `phase_order` rank after numbered ones via `i32::MAX`; among those,
/// `created_at_ms` decides. Returns `None` when the project has no
/// phase notes — caller treats that as "no gating."
pub async fn first_phase_id(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
) -> Option<Uuid> {
    let notes = note_repo.list_for_project(project_id).ok()?;
    let phase_notes: Vec<&LocalNote> = notes
        .iter()
        .filter(|n| matches!(n.kind, NoteKind::Phase))
        .collect();
    if phase_notes.is_empty() {
        return None;
    }
    let mut keyed: Vec<(Uuid, i32, i64)> = Vec::with_capacity(phase_notes.len());
    for n in phase_notes {
        let order = match persistence.load(&n.id.to_string()).await {
            Ok(bytes) => String::from_utf8(bytes)
                .ok()
                .and_then(|body| frontmatter::parse(&body).order)
                .unwrap_or(i32::MAX),
            Err(_) => i32::MAX,
        };
        keyed.push((n.id, order, n.created_at_ms));
    }
    keyed.sort_by_key(|(_, order, ts)| (*order, *ts));
    keyed.first().map(|(id, _, _)| *id)
}

/// Convenience: returns true when the artifact at `start_id` lives in
/// the first phase of its project, OR the project has no phase notes
/// at all. Used by the cascade to decide whether the architecture
/// skill should fire on a given master_requirement.
pub async fn is_in_first_phase(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
    start_id: Uuid,
) -> bool {
    let phase = ancestor_phase_id(note_repo, project_id, start_id);
    let first = first_phase_id(note_repo, persistence, project_id).await;
    match (phase, first) {
        // Legacy project: no phase ancestor + no phase notes → allow.
        (None, None) => true,
        // Project has phases but this artifact isn't inside one — also
        // legacy / mixed state; default to allow so the cascade
        // doesn't silently stall.
        (None, Some(_)) => true,
        // Phase ancestor found but project somehow has no phase notes
        // (shouldn't happen). Allow.
        (Some(_), None) => true,
        (Some(p), Some(f)) => p == f,
    }
}

/// Return the phase id immediately preceding `start_phase_id` in the
/// project's ordering. "Preceding" = highest `(phase_order,
/// created_at_ms)` that is strictly less than the start phase's key.
///
/// Returns `None` when:
/// - `start_phase_id` is the first phase (no previous), OR
/// - `start_phase_id` is not a known phase note, OR
/// - the project has no phase notes at all.
///
/// Used by the runner's architecture-skill inheritance: when running
/// `06-sa-draft-architecture` for Phase N, the prompt inlines the
/// architecture artifact from Phase N-1 as prior-art context. For
/// Phase 0 (`previous_phase_id` returns None), the runner falls back
/// to CE's subtree.
pub async fn previous_phase_id(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
    start_phase_id: Uuid,
) -> Option<Uuid> {
    let notes = note_repo.list_for_project(project_id).ok()?;
    let phase_notes: Vec<&LocalNote> = notes
        .iter()
        .filter(|n| matches!(n.kind, NoteKind::Phase))
        .collect();
    if phase_notes.is_empty() {
        return None;
    }
    // Build the sort keys for every phase so we can locate the start
    // phase's key and find the largest strictly-less neighbour in one
    // pass.
    let mut keyed: Vec<(Uuid, i32, i64)> = Vec::with_capacity(phase_notes.len());
    for n in phase_notes {
        let order = match persistence.load(&n.id.to_string()).await {
            Ok(bytes) => String::from_utf8(bytes)
                .ok()
                .and_then(|body| frontmatter::parse(&body).order)
                .unwrap_or(i32::MAX),
            Err(_) => i32::MAX,
        };
        keyed.push((n.id, order, n.created_at_ms));
    }
    let start_key = keyed
        .iter()
        .find(|(id, _, _)| *id == start_phase_id)
        .map(|(_, order, ts)| (*order, *ts))?;
    keyed
        .into_iter()
        .filter(|(id, order, ts)| {
            *id != start_phase_id && (*order, *ts) < start_key
        })
        .max_by_key(|(_, order, ts)| (*order, *ts))
        .map(|(id, _, _)| id)
}

/// Find the project-root note whose body declares
/// `artifact_kind: requirement` AND that has no master_requirement
/// ancestor. The "CE" customer-engagement bucket — a project-level
/// singleton sitting alongside the phases. Used by the runner as the
/// fallback inheritance source for Phase 0's architecture (when
/// `previous_phase_id` returns `None`).
///
/// Returns `None` if no such note exists. Convention: the user
/// authors one `Artifact` note at the project root with kind
/// `requirement`; everything under it (markdown, images, nested
/// requirements) is part of CE.
pub async fn find_ce_root(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
) -> Option<Uuid> {
    let notes = note_repo.list_for_project(project_id).ok()?;
    for n in &notes {
        // CE is at project root (no parent).
        if n.parent_id.is_some() {
            continue;
        }
        if !matches!(n.kind, NoteKind::Artifact) {
            continue;
        }
        let bytes = persistence.load(&n.id.to_string()).await.ok()?;
        let body = String::from_utf8(bytes).ok()?;
        let fm = crate::plugins::artifact::frontmatter::parse(&body);
        if fm
            .artifact_kind
            .as_ref()
            .map(|k| k.as_str() == "requirement" || k.as_str() == "requirements")
            .unwrap_or(false)
        {
            return Some(n.id);
        }
    }
    None
}

/// Find the architecture artifact (`artifact_kind: architecture`)
/// living directly under `phase_id`'s master_requirement. Walks
/// children of `phase_id` looking for a master_requirement, then
/// walks that master's children for an Architecture. Returns `None`
/// when either is missing.
pub async fn architecture_under_phase(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
    phase_id: Uuid,
) -> Option<Uuid> {
    let notes = note_repo.list_for_project(project_id).ok()?;
    // Step 1: find master_requirement child of phase_id.
    let master_id = {
        let mut found: Option<Uuid> = None;
        for n in &notes {
            if n.parent_id != Some(phase_id) {
                continue;
            }
            if !matches!(n.kind, NoteKind::Artifact) {
                continue;
            }
            let Ok(bytes) = persistence.load(&n.id.to_string()).await else {
                continue;
            };
            let Ok(body) = String::from_utf8(bytes) else {
                continue;
            };
            let fm = crate::plugins::artifact::frontmatter::parse(&body);
            if fm
                .artifact_kind
                .as_ref()
                .map(|k| k.as_str() == "master_requirement")
                .unwrap_or(false)
            {
                found = Some(n.id);
                break;
            }
        }
        found?
    };
    // Step 2: find architecture child of master_id.
    for n in &notes {
        if n.parent_id != Some(master_id) {
            continue;
        }
        if !matches!(n.kind, NoteKind::Artifact) {
            continue;
        }
        let Ok(bytes) = persistence.load(&n.id.to_string()).await else {
            continue;
        };
        let Ok(body) = String::from_utf8(bytes) else {
            continue;
        };
        let fm = crate::plugins::artifact::frontmatter::parse(&body);
        if fm
            .artifact_kind
            .as_ref()
            .map(|k| k.as_str() == "architecture")
            .unwrap_or(false)
        {
            return Some(n.id);
        }
    }
    None
}
