//! Artifact-aware [`Persistence`] wrapper (migration 018 / artifact-on-disk
//! 1:1 with the UI tree).
//!
//! Routes reads/writes for `NoteKind::Artifact` rows to
//! `<vault>/.operon/<project-id>/artifacts/<root-slug>/.../<self-slug>/index.md`.
//! Everything else (markdown, skills, workflows, images) passes through to an
//! inner `Persistence`, unchanged.
//!
//! The artifact path is rooted in the **vault**, not in the user's git
//! repository, so the repo Claude Code edits stays free of operon
//! sidecars. Vault is captured once at `ArtifactPersistence` construction
//! (in `app.rs`) — if the user re-picks the vault at runtime, the
//! persistence chain rebuilds with the new path.
//!
//! Per-call overhead: one `find_project_for_note` + one `list_for_project`
//! query. Acceptable for save/load latency at expected vault sizes; if a
//! future profile shows this is hot, add a project-scoped notes cache here.
//!
//! Failure semantics: if any lookup fails (note not found, target isn't an
//! artifact, vault not set, missing slug), the wrapper delegates to the
//! inner persistence — so non-artifact notes always work and never see
//! the artifact pathway.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use operon_store::repos::LocalNoteRepository;
use uuid::Uuid;

use crate::local_mode::vault::VaultRoot;
use crate::plugins::artifact::paths::ArtifactPathResolver;

use super::{NoteRef, PersistError, Persistence};

pub struct ArtifactPersistence {
    inner: Arc<dyn Persistence>,
    note_repo: Arc<dyn LocalNoteRepository>,
    vault: Option<VaultRoot>,
}

impl ArtifactPersistence {
    pub fn new(
        inner: Arc<dyn Persistence>,
        note_repo: Arc<dyn LocalNoteRepository>,
        vault: Option<VaultRoot>,
    ) -> Self {
        Self { inner, note_repo, vault }
    }

    /// Resolve a note_id to its canonical artifact path, returning `None`
    /// for anything that should fall through to inner persistence (non-UUID
    /// ids, non-artifact notes, missing slugs, vault not configured).
    fn resolve_artifact_path(&self, note_id: &str) -> Option<PathBuf> {
        let vault = self.vault.as_ref()?;
        let id = Uuid::parse_str(note_id).ok()?;
        let project_id = self.note_repo.find_project_for_note(id).ok().flatten()?;
        let notes = self.note_repo.list_for_project(project_id).ok()?;
        let artifacts_root = vault.project_artifacts_dir(project_id);
        let resolver = ArtifactPathResolver::new(&artifacts_root, &notes);
        resolver.artifact_index_path(id)
    }
}

impl Persistence for ArtifactPersistence {
    fn load<'a>(
        &'a self,
        note_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, PersistError>> + 'a>> {
        Box::pin(async move {
            if let Some(path) = self.resolve_artifact_path(note_id) {
                match std::fs::read(&path) {
                    Ok(bytes) => return Ok(bytes),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        // Migration not yet complete or fresh artifact —
                        // fall back to opaque store so legacy bytes still
                        // resolve.
                    }
                    Err(e) => return Err(PersistError::Io(e.to_string())),
                }
            }
            self.inner.load(note_id).await
        })
    }

    fn save<'a>(
        &'a self,
        note_id: &'a str,
        bytes: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = Result<(), PersistError>> + 'a>> {
        Box::pin(async move {
            if let Some(path) = self.resolve_artifact_path(note_id) {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)
                        .map_err(|e| PersistError::Io(e.to_string()))?;
                }
                let temp_dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
                let temp = tempfile::NamedTempFile::new_in(temp_dir)
                    .map_err(|e| PersistError::Io(e.to_string()))?;
                std::fs::write(temp.path(), bytes)
                    .map_err(|e| PersistError::Io(e.to_string()))?;
                temp.persist(&path)
                    .map_err(|e| PersistError::Io(e.error.to_string()))?;
                return Ok(());
            }
            self.inner.save(note_id, bytes).await
        })
    }

    fn list<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<NoteRef>, PersistError>> + 'a>> {
        // Artifact files don't appear in `list()` results — they live under
        // per-project `<vault>/.operon/<project-id>/artifacts/` and the
        // abstract `Persistence::list` doesn't know which project to scan.
        // Callers (search, hydration) query the SQLite note tree directly
        // for artifact discovery.
        self.inner.list()
    }

    fn delete<'a>(
        &'a self,
        note_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), PersistError>> + 'a>> {
        Box::pin(async move {
            if let Some(path) = self.resolve_artifact_path(note_id) {
                if let Some(dir) = path.parent() {
                    match std::fs::remove_dir_all(dir) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(PersistError::Io(e.to_string())),
                    }
                }
            }
            // Also clear any legacy opaque-store bytes for this id; tolerate
            // NotFound either way.
            match self.inner.delete(note_id).await {
                Ok(()) => Ok(()),
                Err(PersistError::NotFound) => Ok(()),
                Err(e) => Err(e),
            }
        })
    }

    fn rename<'a>(
        &'a self,
        from: &'a str,
        to: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), PersistError>> + 'a>> {
        // Artifact title renames are handled by the title-rename hook in
        // `src/plugins/artifact/relocate.rs`, which moves the folder on disk.
        // The Persistence trait's `rename` operates on note_ids (UUIDs),
        // which artifacts never change — so this just delegates.
        self.inner.rename(from, to)
    }

    fn resolved_path(&self, note_id: &str) -> Option<PathBuf> {
        // Artifact notes resolve to
        // `<vault>/.operon/<project-id>/artifacts/.../index.md`; everything
        // else falls through to the inner store's path
        // (`<vault>/notes/<uuid>` for the filesystem backend).
        self.resolve_artifact_path(note_id)
            .or_else(|| self.inner.resolved_path(note_id))
    }
}
