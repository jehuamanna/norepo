//! Filesystem side-effects for artifact tree mutations (migration 018).
//!
//! Wraps an inner [`LocalNoteRepository`] so renames, reparents, and deletes
//! that change an artifact's canonical on-disk path also move (or remove)
//! the corresponding directory under
//! `<vault>/.operon/<project-id>/artifacts/`. Non-artifact mutations pass
//! through unchanged.
//!
//! The wrapper is installed in `provide_local_state` / `LocalStateProvider`,
//! so every caller that uses the `LocalNoteRepo` context handle picks it up
//! automatically — bulk renames, the editor's undo/redo path, drag-drop in
//! the explorer, etc. all relocate without per-call-site changes.

use std::path::PathBuf;
use std::sync::Arc;

use operon_store::repos::{
    LocalNote, LocalNoteRepository, NoteKind, SubtreeSnapshot,
};
use operon_store::StoreError;
use uuid::Uuid;

use crate::local_mode::vault::VaultRoot;

use super::paths::ArtifactPathResolver;

pub struct RelocatingNoteRepo {
    inner: Arc<dyn LocalNoteRepository>,
    vault: Option<VaultRoot>,
}

impl RelocatingNoteRepo {
    pub fn new(
        inner: Arc<dyn LocalNoteRepository>,
        vault: Option<VaultRoot>,
    ) -> Self {
        Self { inner, vault }
    }

    /// Resolve `note_id`'s canonical artifact dir using the current DB
    /// state. Returns `None` for non-artifact notes, missing rows, when
    /// the vault isn't configured, or unset slugs (pre-migration).
    fn canonical_dir(&self, note_id: Uuid) -> Option<PathBuf> {
        let vault = self.vault.as_ref()?;
        let project_id = self.inner.find_project_for_note(note_id).ok().flatten()?;
        let notes = self.inner.list_for_project(project_id).ok()?;
        let artifacts_root = vault.project_artifacts_dir(project_id);
        ArtifactPathResolver::new(&artifacts_root, &notes).artifact_dir(note_id)
    }

    /// Best-effort move from `old` to `new`. Logs and swallows errors —
    /// we never want a UI rename to fail because the filesystem hiccupped;
    /// the DB row is the source of truth and a future save will recreate
    /// the file at the right location.
    fn try_relocate(&self, old: Option<PathBuf>, new: Option<PathBuf>) {
        let (Some(old), Some(new)) = (old, new) else { return };
        if old == new {
            return;
        }
        if !old.exists() {
            return;
        }
        if let Some(parent) = new.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!(
                    target: "operon::artifact::relocate",
                    "create_dir_all {parent:?} failed: {e}"
                );
                return;
            }
        }
        if let Err(e) = std::fs::rename(&old, &new) {
            tracing::warn!(
                target: "operon::artifact::relocate",
                "fs::rename {old:?} -> {new:?} failed: {e}"
            );
        }
    }

    fn try_remove_dir(&self, dir: Option<PathBuf>) {
        let Some(dir) = dir else { return };
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(
                target: "operon::artifact::relocate",
                "remove_dir_all {dir:?} failed: {e}"
            ),
        }
    }
}

impl LocalNoteRepository for RelocatingNoteRepo {
    fn list_for_project(&self, project_id: Uuid) -> Result<Vec<LocalNote>, StoreError> {
        self.inner.list_for_project(project_id)
    }

    fn find_project_for_note(&self, note_id: Uuid) -> Result<Option<Uuid>, StoreError> {
        self.inner.find_project_for_note(note_id)
    }

    fn create(
        &self,
        project_id: Uuid,
        parent_id: Option<Uuid>,
        title: &str,
    ) -> Result<LocalNote, StoreError> {
        self.inner.create(project_id, parent_id, title)
    }

    fn create_with_kind(
        &self,
        project_id: Uuid,
        parent_id: Option<Uuid>,
        title: &str,
        kind: NoteKind,
    ) -> Result<LocalNote, StoreError> {
        self.inner
            .create_with_kind(project_id, parent_id, title, kind)
    }

    fn set_kind(&self, id: Uuid, kind: NoteKind) -> Result<(), StoreError> {
        // Capture the pre-mutation path; if kind transitions out of Artifact,
        // the slug is cleared by the inner repo so we won't be able to derive
        // the path after the call. Drop the on-disk dir to avoid leaving an
        // orphan tree under `.operon/artifacts/`.
        let was_artifact_dir = self.canonical_dir(id);
        self.inner.set_kind(id, kind)?;
        if !matches!(kind, NoteKind::Artifact) {
            self.try_remove_dir(was_artifact_dir);
        }
        Ok(())
    }

    fn set_blob_path(&self, id: Uuid, path: Option<&str>) -> Result<(), StoreError> {
        self.inner.set_blob_path(id, path)
    }

    fn rename(&self, id: Uuid, title: &str) -> Result<(), StoreError> {
        let old_dir = self.canonical_dir(id);
        self.inner.rename(id, title)?;
        let new_dir = self.canonical_dir(id);
        self.try_relocate(old_dir, new_dir);
        Ok(())
    }

    fn delete(&self, id: Uuid) -> Result<(), StoreError> {
        let dir = self.canonical_dir(id);
        self.inner.delete(id)?;
        self.try_remove_dir(dir);
        Ok(())
    }

    fn touch_updated(&self, id: Uuid) -> Result<(), StoreError> {
        self.inner.touch_updated(id)
    }

    fn move_to(
        &self,
        id: Uuid,
        new_project_id: Uuid,
        new_parent: Option<Uuid>,
        new_sibling_index: i64,
    ) -> Result<(), StoreError> {
        let old_dir = self.canonical_dir(id);
        self.inner
            .move_to(id, new_project_id, new_parent, new_sibling_index)?;
        let new_dir = self.canonical_dir(id);
        self.try_relocate(old_dir, new_dir);
        Ok(())
    }

    fn duplicate_subtree(
        &self,
        id: Uuid,
        into_project: Uuid,
        into_parent: Option<Uuid>,
        new_sibling_index: i64,
    ) -> Result<Uuid, StoreError> {
        // Duplicates land as `NoteKind::Markdown` (legacy behavior — kind is
        // not copied by the SQLite repo); no slug, no FS work.
        self.inner
            .duplicate_subtree(id, into_project, into_parent, new_sibling_index)
    }

    fn indent(&self, id: Uuid) -> Result<(), StoreError> {
        let old_dir = self.canonical_dir(id);
        self.inner.indent(id)?;
        let new_dir = self.canonical_dir(id);
        self.try_relocate(old_dir, new_dir);
        Ok(())
    }

    fn outdent(&self, id: Uuid) -> Result<(), StoreError> {
        let old_dir = self.canonical_dir(id);
        self.inner.outdent(id)?;
        let new_dir = self.canonical_dir(id);
        self.try_relocate(old_dir, new_dir);
        Ok(())
    }

    fn move_up(&self, id: Uuid) -> Result<(), StoreError> {
        // Sibling-reordering only — slug stays put, no FS work.
        self.inner.move_up(id)
    }

    fn move_down(&self, id: Uuid) -> Result<(), StoreError> {
        self.inner.move_down(id)
    }

    fn snapshot_subtree(&self, id: Uuid) -> Result<SubtreeSnapshot, StoreError> {
        self.inner.snapshot_subtree(id)
    }

    fn restore_subtree(&self, snap: &SubtreeSnapshot) -> Result<(), StoreError> {
        // Restores re-insert rows with their captured slugs; the unique
        // index will reject a collision into a now-occupied bucket. The
        // body re-materializes when the caller calls `persistence.save`
        // afterwards, so no FS work is needed up-front.
        self.inner.restore_subtree(snap)
    }

    fn ensure_artifact_slug(&self, id: Uuid) -> Result<Option<String>, StoreError> {
        self.inner.ensure_artifact_slug(id)
    }
}
