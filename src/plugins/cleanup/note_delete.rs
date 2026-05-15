//! Unified disk cleanup for note deletion, with **session-scoped trash**
//! for Ctrl-Z recovery.
//!
//! `delete_note_with_disk_cleanup` is the single entry point every UI/
//! cascade caller funnels through. Rather than deleting on-disk
//! side-effects outright, it **moves them into a per-delete trash
//! folder**. The returned [`TrashRecord`] is attached to the explorer's
//! undo entry so the user's next Ctrl-Z can restore everything together
//! with the SQLite rows.
//!
//! Side-effects handled:
//!
//! 1. **Artifact dirs** under
//!    `<vault>/.operon/<project-id>/artifacts/<...>/`. The
//!    `ArtifactPathResolver` walks ancestor slugs, so paths must be
//!    resolved **before** SQLite cascade fires (which is why we
//!    snapshot the subtree up front).
//! 2. **Materialized skill files** at `<repo>/.claude/skills/<slug>.md`.
//!    The slug is the same one [`crate::plugins::skill::view`] uses on
//!    Play — derived from the body's `skill_name:` frontmatter, with
//!    the slugified note id as fallback. Slug derivation needs the
//!    body, so this is async.
//! 3. **Image blobs** under the vault. Blobs are refcounted: only
//!    blobs no remaining note references are moved into the trash.
//!    Restoring the note restores the blob too.
//!
//! Errors at the FS layer are logged and swallowed. The SQLite row is
//! the source of truth.
//!
//! Trash is **session-scoped**: see [`crate::plugins::cleanup::trash`].

use std::path::PathBuf;
use std::sync::Arc;

use operon_store::repos::{
    LocalNote, LocalNoteRepository, LocalProjectRepository, NoteKind, SubtreeSnapshot,
};
use operon_store::StoreError;
use uuid::Uuid;

use crate::local_mode::vault::VaultRoot;
use crate::persistence::Persistence;
use crate::plugins::artifact::paths::ArtifactPathResolver;
use crate::plugins::cleanup::trash::{
    project_trash_root, repo_skill_trash_root, vault_trash_root, TrashRecord,
};
use crate::plugins::skill::frontmatter as skill_fm;

/// Outcome of a delete: the snapshot needed to drive
/// [`LocalNoteRepository::restore_subtree`] on undo, plus the
/// `TrashRecord` needed to put the on-disk side-effects back where they
/// were. Both arrive at the explorer's undo history together.
#[derive(Debug, Clone, Default)]
pub struct DeleteOutcome {
    pub snapshot: Option<SubtreeSnapshot>,
    pub trash: TrashRecord,
}

/// Delete a note (and its subtree, via SQL cascade) and trash every
/// on-disk side-effect produced by any deleted node. The returned
/// outcome carries everything undo needs to restore the original
/// state.
///
/// `vault_root` is `None` on platforms where blob trash isn't
/// applicable (e.g. wasm OPFS); artifact dirs and skills are still
/// trashed.
pub async fn delete_note_with_disk_cleanup(
    note_id: Uuid,
    note_repo: &Arc<dyn LocalNoteRepository>,
    project_repo: &Arc<dyn LocalProjectRepository>,
    persistence: &Arc<dyn Persistence>,
    vault_root: Option<&VaultRoot>,
) -> Result<DeleteOutcome, StoreError> {
    let snapshot = note_repo.snapshot_subtree(note_id).ok();

    let plan = build_cleanup_plan(
        snapshot.as_ref(),
        note_repo,
        project_repo,
        persistence,
        vault_root,
    )
    .await;

    // Trash artifact dirs and skill files **before** the SQLite cascade
    // so the `RelocatingNoteRepo.delete` wrapper's own `remove_dir_all`
    // call finds the dir already moved out and becomes a no-op.
    let mut trash = TrashRecord::new();
    apply_artifact_trash(&plan.artifact_dirs, vault_root, &mut trash);
    apply_skill_trash(&plan.skill_targets, &mut trash);

    note_repo.delete(note_id)?;

    // Blobs are refcounted: a blob is only orphaned (and therefore
    // trashable) once the deleted notes have been removed from the
    // SQLite tree. Run this **after** delete.
    if let Some(vault) = vault_root {
        trash_unreferenced_blobs(&plan.blob_paths, vault, project_repo, note_repo, &mut trash);
    }

    Ok(DeleteOutcome { snapshot, trash })
}

/// What we need to move aside after the SQLite cascade fires.
#[derive(Default)]
struct CleanupPlan {
    /// `(project_id, artifact_dir)` pairs. `project_id` keys the
    /// per-project trash root under the vault; `artifact_dir` is the
    /// absolute path being moved aside.
    artifact_dirs: Vec<(Uuid, PathBuf)>,
    /// `(repo_path, slug)` for each materialized skill file. Skill
    /// files live under `<repo>/.claude/skills/` (Claude Code's
    /// convention), so their trash root is co-located with the repo.
    skill_targets: Vec<(PathBuf, String)>,
    /// Vault-relative blob paths to refcount-check after delete. Only
    /// trashed if no remaining note still references them.
    blob_paths: Vec<String>,
}

async fn build_cleanup_plan(
    snapshot: Option<&SubtreeSnapshot>,
    note_repo: &Arc<dyn LocalNoteRepository>,
    project_repo: &Arc<dyn LocalProjectRepository>,
    persistence: &Arc<dyn Persistence>,
    vault: Option<&VaultRoot>,
) -> CleanupPlan {
    let Some(snap) = snapshot else {
        return CleanupPlan::default();
    };
    if snap.notes.is_empty() {
        return CleanupPlan::default();
    }

    let mut plan = CleanupPlan::default();

    let mut by_project: std::collections::HashMap<Uuid, Vec<&LocalNote>> =
        std::collections::HashMap::new();
    for n in &snap.notes {
        by_project.entry(n.project_id).or_default().push(n);
    }

    for (project_id, notes_in_project) in &by_project {
        let repo_path = repo_path_for_project(project_repo, *project_id);
        let project_notes = note_repo.list_for_project(*project_id).ok();

        if let (Some(v), Some(all_notes)) = (vault, project_notes.as_ref()) {
            let artifacts_root = v.project_artifacts_dir(*project_id);
            let resolver = ArtifactPathResolver::new(&artifacts_root, all_notes);
            for n in notes_in_project {
                if matches!(n.kind, NoteKind::Artifact) {
                    if let Some(dir) = resolver.artifact_dir(n.id) {
                        plan.artifact_dirs.push((*project_id, dir));
                    }
                }
            }
        }

        if let Some(repo) = repo_path {
            for n in notes_in_project {
                if !matches!(n.kind, NoteKind::Skill) {
                    continue;
                }
                let slug = derive_skill_slug(persistence, n.id).await;
                plan.skill_targets.push((repo.clone(), slug));
            }
        }

        for n in notes_in_project {
            if let Some(bp) = n.blob_path.clone() {
                plan.blob_paths.push(bp);
            }
        }
    }

    plan
}

fn apply_artifact_trash(
    dirs: &[(Uuid, PathBuf)],
    vault: Option<&VaultRoot>,
    trash: &mut TrashRecord,
) {
    let Some(vault) = vault else {
        // No vault → nothing to trash to; cleanup is a no-op. The
        // SQLite row deletion is still the source of truth.
        return;
    };
    for (project_id, dir) in dirs {
        let trash_root = project_trash_root(vault, *project_id);
        let artifacts_root = vault.project_artifacts_dir(*project_id);
        // Trash layout mirrors the source's relative path under the
        // artifacts root so multiple sibling artifacts don't collide.
        let rel = match dir.strip_prefix(&artifacts_root) {
            Ok(p) => p.to_path_buf(),
            Err(_) => PathBuf::from(dir.file_name().unwrap_or_default()),
        };
        if let Err(e) = trash.move_into_trash(dir, &trash_root, &rel) {
            tracing::warn!(
                target: "operon::cleanup::note_delete",
                "trash artifact dir {dir:?} failed: {e}"
            );
        }
    }
}

fn apply_skill_trash(targets: &[(PathBuf, String)], trash: &mut TrashRecord) {
    for (repo, slug) in targets {
        let source = repo
            .join(".claude")
            .join("skills")
            .join(format!("{slug}.md"));
        let trash_root = repo_skill_trash_root(repo);
        let rel = PathBuf::from("skills").join(format!("{slug}.md"));
        if let Err(e) = trash.move_into_trash(&source, &trash_root, &rel) {
            tracing::warn!(
                target: "operon::cleanup::note_delete",
                "trash skill {source:?} failed: {e}"
            );
        }
    }
}

fn trash_unreferenced_blobs(
    blobs: &[String],
    vault: &VaultRoot,
    project_repo: &Arc<dyn LocalProjectRepository>,
    note_repo: &Arc<dyn LocalNoteRepository>,
    trash: &mut TrashRecord,
) {
    if blobs.is_empty() {
        return;
    }
    let projects = match project_repo.list() {
        Ok(ps) => ps,
        Err(_) => return,
    };
    let trash_root = vault_trash_root(vault.path());
    for blob in blobs {
        let mut still_referenced = false;
        'outer: for p in &projects {
            if let Ok(notes) = note_repo.list_for_project(p.id) {
                for n in notes {
                    if n.blob_path.as_deref() == Some(blob.as_str()) {
                        still_referenced = true;
                        break 'outer;
                    }
                }
            }
        }
        if still_referenced {
            continue;
        }
        let source = vault.path().join(blob);
        let rel = PathBuf::from("blobs").join(blob);
        if let Err(e) = trash.move_into_trash(&source, &trash_root, &rel) {
            tracing::warn!(
                target: "operon::cleanup::note_delete",
                "trash blob {source:?} failed: {e}"
            );
        }
    }
}

fn repo_path_for_project(
    project_repo: &Arc<dyn LocalProjectRepository>,
    project_id: Uuid,
) -> Option<PathBuf> {
    project_repo
        .list()
        .ok()?
        .into_iter()
        .find(|p| p.id == project_id)
        .and_then(|p| p.repo_path)
}

/// Derive the slug used at materialize time. Mirrors
/// `src/plugins/skill/view.rs` so the same skill note resolves to the
/// same filename whether we're writing it on Play or trashing it on
/// Delete. Falls back to the note-id slug when the body is missing,
/// unreadable, or lacks `skill_name:`.
async fn derive_skill_slug(persistence: &Arc<dyn Persistence>, note_id: Uuid) -> String {
    let id_str = note_id.to_string();
    let fallback = || skill_fm::slugify(&id_str);
    let bytes = match persistence.load(&id_str).await {
        Ok(b) => b,
        Err(_) => return fallback(),
    };
    let body = match std::str::from_utf8(&bytes) {
        Ok(s) => s,
        Err(_) => return fallback(),
    };
    let (fm_opt, _rest) = skill_fm::split(body);
    let Some(fm_lines) = fm_opt else {
        return fallback();
    };
    match skill_fm::field(&fm_lines, "skill_name") {
        Some(name) if !name.is_empty() => skill_fm::slugify(name),
        _ => fallback(),
    }
}

// Integration tests live at `tests/note_delete_cleanup.rs` because
// they need the real SQLite-backed `LocalNoteRepository` and a tempdir
// for `repo_path`.
