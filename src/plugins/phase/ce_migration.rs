//! One-shot migration: flip legacy CE artifacts to `NoteKind::Ce`.
//!
//! Before migration 021, the CE (Customer Engineering) root was modelled
//! as an `Artifact` at the project root with `artifact_kind: requirement`
//! in YAML frontmatter, and discovered by parsing each root note's body.
//! Migration 021 added `NoteKind::Ce` as a first-class kind; this module
//! is the data-side companion that updates existing rows so the new
//! kind-based discovery in `find_ce_root` returns the same id it used
//! to return.
//!
//! Wired into the project-open code path. Idempotent: once a project
//! has no qualifying Artifact roots, the scan is a no-op.
//!
//! Artifact frontmatter is left intact (no body rewrite); downstream
//! code reads kind from SQL, not frontmatter, so the `artifact_kind`
//! line on a flipped row is harmless leftover.

use std::sync::Arc;

use uuid::Uuid;

use operon_store::repos::{LocalNoteRepository, NoteKind};

use crate::persistence::Persistence;

/// Scan `project_id` for any root-level Artifact note whose body
/// declares `artifact_kind: requirement` (or `requirements`), and
/// flip its row to `NoteKind::Ce`. Returns the number of rows
/// converted (zero on idempotent re-runs).
pub async fn migrate_legacy_ce(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
) -> usize {
    let Ok(notes) = note_repo.list_for_project(project_id) else {
        return 0;
    };
    let mut converted = 0usize;
    for n in notes {
        if n.parent_id.is_some() || !matches!(n.kind, NoteKind::Artifact) {
            continue;
        }
        let Ok(bytes) = persistence.load(&n.id.to_string()).await else {
            continue;
        };
        let Ok(body) = String::from_utf8(bytes) else {
            continue;
        };
        let fm = crate::plugins::artifact::frontmatter::parse(&body);
        let is_legacy_ce = fm
            .artifact_kind
            .as_ref()
            .map(|k| k.as_str() == "requirement" || k.as_str() == "requirements")
            .unwrap_or(false);
        if !is_legacy_ce {
            continue;
        }
        if note_repo.set_kind(n.id, NoteKind::Ce).is_ok() {
            converted += 1;
        }
    }
    converted
}
