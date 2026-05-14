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
