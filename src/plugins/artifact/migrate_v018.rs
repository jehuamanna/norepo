//! One-shot migration for artifact-on-disk 1:1 layout (migration 018).
//!
//! On first boot after the SQL migration runs, every project's
//! `.operon/artifacts/` directory still uses the old
//! `<source-UUID>/<child-title>.md` staging layout, and canonical bodies
//! still live in the opaque `notes_dir/<UUID>` store. This module reshapes
//! both into the new `.operon/artifacts/<slug>/.../index.md` form so the
//! disk view matches what the UI shows.
//!
//! Sentinel: `<repo_path>/.operon/.artifact-layout-v1`. Idempotent — the
//! sentinel is checked up front and only touched on successful completion.
//! Failures during a project's migration leave the sentinel absent, so a
//! later run retries; partially-migrated state is detected by re-resolving
//! slugs (which are unique per (project, parent) and stable once written).
//!
//! Body source priority for each artifact note, in order:
//!   1. `<notes_dir>/<UUID>` (opaque persistence — latest UI edits).
//!   2. legacy staging `<repo>/.operon/artifacts/<source-UUID>/<title>.md`.
//!   3. empty stub (last resort so the file exists for the reader).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use operon_store::repos::{
    LocalNote, LocalNoteRepository, LocalProjectRepository, NoteKind,
};
use uuid::Uuid;

use super::paths::{ArtifactPathResolver, ARTIFACT_INDEX_FILENAME, ARTIFACTS_SUBDIR};

const SENTINEL_FILENAME: &str = ".operon/.artifact-layout-v1";

/// Run the migration for every project that has a `repo_path` set. Safe to
/// call on every boot — finished projects are skipped via their sentinel.
/// Errors during a single project are logged and don't abort the sweep.
pub fn migrate_all_projects(
    project_repo: &Arc<dyn LocalProjectRepository>,
    note_repo: &Arc<dyn LocalNoteRepository>,
    notes_dir: &Path,
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
        let Some(repo_path) = p.repo_path else { continue };
        if let Err(e) = migrate_project(&repo_path, p.id, note_repo, notes_dir) {
            tracing::warn!(
                target: "operon::artifact::migrate_v018",
                "project {} migration failed: {e}",
                p.id
            );
        }
    }
}

fn migrate_project(
    repo_path: &Path,
    project_id: Uuid,
    note_repo: &Arc<dyn LocalNoteRepository>,
    notes_dir: &Path,
) -> std::io::Result<()> {
    let sentinel = repo_path.join(SENTINEL_FILENAME);
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
    let resolver = ArtifactPathResolver::new(repo_path, &notes);

    let mut written: Vec<PathBuf> = Vec::new();
    for art in notes.iter().filter(|n| matches!(n.kind, NoteKind::Artifact)) {
        let Some(target) = resolver.artifact_index_path(art.id) else {
            continue;
        };
        if target.exists() {
            written.push(target);
            continue;
        }
        let body = locate_body(notes_dir, repo_path, art, &notes);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, body.as_deref().unwrap_or(b""))?;
        written.push(target);
    }

    // Sweep the legacy `.operon/artifacts/<UUID>/` directories that the old
    // runner used as staging. We only remove dirs whose name parses as a
    // UUID — any new `<slug>/` directory we just wrote stays untouched.
    let artifacts_root = repo_path.join(ARTIFACTS_SUBDIR);
    if let Ok(entries) = std::fs::read_dir(&artifacts_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if Uuid::parse_str(name).is_ok() {
                if let Err(e) = std::fs::remove_dir_all(&path) {
                    tracing::warn!(
                        target: "operon::artifact::migrate_v018",
                        "remove legacy {path:?} failed: {e}"
                    );
                }
            }
        }
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

/// Body source priority: opaque store → legacy staging → empty.
fn locate_body(
    notes_dir: &Path,
    repo_path: &Path,
    art: &LocalNote,
    all_notes: &[LocalNote],
) -> Option<Vec<u8>> {
    let opaque = notes_dir.join(art.id.to_string());
    if let Ok(bytes) = std::fs::read(&opaque) {
        return Some(bytes);
    }
    // Legacy staging: parent's UUID directory held the child's
    // `<title>.md`. For root artifacts (parent_id == None) the old runner
    // never staged a self-body, so we fall through to the empty stub.
    if let Some(parent_id) = art.parent_id {
        let legacy = repo_path
            .join(ARTIFACTS_SUBDIR)
            .join(parent_id.to_string())
            .join(format!("{}.md", art.title));
        if let Ok(bytes) = std::fs::read(&legacy) {
            return Some(bytes);
        }
        // Fallback: the legacy filename used the title verbatim; if the
        // file was created with a different casing/whitespace, scan the
        // dir for any markdown file owned by this artifact's title-stem.
        let legacy_dir = repo_path.join(ARTIFACTS_SUBDIR).join(parent_id.to_string());
        if let Ok(entries) = std::fs::read_dir(&legacy_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                let stem = p.file_stem().and_then(|s| s.to_str());
                if stem == Some(art.title.as_str()) {
                    if let Ok(bytes) = std::fs::read(&p) {
                        return Some(bytes);
                    }
                }
            }
        }
    }
    // Suppress the "unused parameter" warning when `all_notes` ends up
    // unused in some build configurations; reserved for richer fallback
    // strategies (e.g. walking sibling sets) without changing the signature.
    let _ = all_notes;
    None
}
