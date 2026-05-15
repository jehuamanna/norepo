//! One-shot per-project sweep that ensures every artifact note has a
//! slug and a body file at its canonical on-disk location.
//!
//! Canonical location is `<vault>/.operon/<project-id>/artifacts/<slug>/
//! .../index.md` — anchored at the **vault**, not at the user's git
//! repository. This keeps operon's own data out of the user's source
//! tree.
//!
//! Sentinel: `<vault>/.operon/<project-id>/.artifact-layout-v1`.
//! Idempotent — the sentinel is checked up front and only touched on
//! successful completion. Failures during a project's migration leave
//! the sentinel absent, so a later run retries.
//!
//! Body source priority for each artifact note, in order:
//!   1. `<notes_dir>/<UUID>` (opaque persistence — latest UI edits).
//!   2. empty stub (last resort so the file exists for the reader).
//!
//! Legacy `<repo>/.operon/artifacts/<UUID>/<title>.md` staging paths
//! (from before the vault relocation) are NOT swept by this code path
//! anymore — users move them manually if they want their old artifacts
//! preserved.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use operon_store::repos::{
    LocalNote, LocalNoteRepository, LocalProjectRepository, NoteKind,
};
use uuid::Uuid;

use crate::local_mode::vault::VaultRoot;

use super::paths::ArtifactPathResolver;

/// Run the migration for every project. Safe to call on every boot —
/// finished projects are skipped via their per-project sentinel.
/// Errors during a single project are logged and don't abort the sweep.
pub fn migrate_all_projects(
    project_repo: &Arc<dyn LocalProjectRepository>,
    note_repo: &Arc<dyn LocalNoteRepository>,
    notes_dir: &Path,
    vault: &VaultRoot,
) {
    let projects = match project_repo.list() {
        Ok(ps) => ps,
        Err(e) => {
            tracing::warn!(
                target: "operon::artifact::migrate_v018",
                "list projects failed: {e}"
            );
            return;
        }
    };
    for p in projects {
        if let Err(e) = migrate_project(vault, p.id, note_repo, notes_dir) {
            tracing::warn!(
                target: "operon::artifact::migrate_v018",
                "project {} migration failed: {e}",
                p.id
            );
        }
    }
}

fn migrate_project(
    vault: &VaultRoot,
    project_id: Uuid,
    note_repo: &Arc<dyn LocalNoteRepository>,
    notes_dir: &Path,
) -> std::io::Result<()> {
    let sentinel = vault.project_artifact_layout_sentinel(project_id);
    if sentinel.exists() {
        return Ok(());
    }

    let notes = match note_repo.list_for_project(project_id) {
        Ok(ns) => ns,
        Err(e) => {
            tracing::warn!(
                target: "operon::artifact::migrate_v018",
                "list_for_project {project_id} failed: {e}"
            );
            return Ok(());
        }
    };

    // Backfill slugs root-first. Each ensure call computes a slug against
    // siblings already in the DB; processing root-first guarantees the
    // ancestor chain is fully resolvable when we compute the path below.
    let mut artifacts: Vec<&LocalNote> = notes
        .iter()
        .filter(|n| matches!(n.kind, NoteKind::Artifact))
        .collect();
    artifacts.sort_by_key(|n| n.depth);
    for art in &artifacts {
        if art.slug.is_some() {
            continue;
        }
        if let Err(e) = note_repo.ensure_artifact_slug(art.id) {
            tracing::warn!(
                target: "operon::artifact::migrate_v018",
                "ensure_artifact_slug {} failed: {e}",
                art.id
            );
        }
    }

    // Re-read with slugs populated so the resolver can derive canonical
    // paths for every artifact.
    let notes = match note_repo.list_for_project(project_id) {
        Ok(ns) => ns,
        Err(_) => return Ok(()),
    };
    let artifacts_root = vault.project_artifacts_dir(project_id);
    let resolver = ArtifactPathResolver::new(&artifacts_root, &notes);

    let mut written: Vec<PathBuf> = Vec::new();
    for art in notes.iter().filter(|n| matches!(n.kind, NoteKind::Artifact)) {
        let Some(target) = resolver.artifact_index_path(art.id) else {
            continue;
        };
        if target.exists() {
            written.push(target);
            continue;
        }
        let body = locate_body(notes_dir, art);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, body.as_deref().unwrap_or(b""))?;
        written.push(target);
    }

    // Touch sentinel last so a crash mid-migration retries on next boot.
    if let Some(parent) = sentinel.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&sentinel, b"")?;
    tracing::info!(
        target: "operon::artifact::migrate_v018",
        "project {project_id}: migrated {} artifact bodies",
        written.len()
    );
    Ok(())
}

/// Body source: opaque store → empty.
fn locate_body(notes_dir: &Path, art: &LocalNote) -> Option<Vec<u8>> {
    let opaque = notes_dir.join(art.id.to_string());
    std::fs::read(&opaque).ok()
}
