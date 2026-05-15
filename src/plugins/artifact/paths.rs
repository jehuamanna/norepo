//! Canonical on-disk paths for artifact notes (migration 018 / artifact-on-disk
//! 1:1 with the UI tree).
//!
//! An artifact note's body lives at
//! `<artifacts_root>/<root-slug>/<...>/<self-slug>/index.md`, where
//! `<artifacts_root>` is the project's `<vault>/.operon/<project-id>/artifacts/`
//! directory — produced by `VaultRoot::project_artifacts_dir(project_id)` at
//! every call site. The resolver itself is unaware of vault / project / repo
//! layout; it just walks slug chains against whatever root the caller supplies.
//!
//! The hierarchy is derived by walking the artifact's `parent_id` chain
//! through other `Artifact`-kind ancestors, stopping at the first non-artifact
//! parent (which is the cascade root's container, e.g. the project root) or
//! at `None`. Slugs come from `LocalNote.slug`, which the SQLite repo
//! assigns / refreshes whenever an artifact row is created, renamed, or
//! reparented (see `assign_artifact_slug` in `operon-store`).
//!
//! Non-artifact notes have no canonical path here — those continue to live
//! under the opaque UUID-indexed `Persistence` storage.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use operon_store::repos::{LocalNote, NoteKind};
use uuid::Uuid;

pub const ARTIFACT_INDEX_FILENAME: &str = "index.md";

/// Snapshot view over a project's notes that resolves artifact paths in
/// O(depth). Build it once per batch (e.g. once per cascade run, once per
/// migration sweep) when you need to look up paths for many notes.
pub struct ArtifactPathResolver<'a> {
    artifacts_root: &'a Path,
    notes_by_id: HashMap<Uuid, &'a LocalNote>,
}

impl<'a> ArtifactPathResolver<'a> {
    /// `artifacts_root` is the per-project artifacts directory — typically
    /// `<vault>/.operon/<project-id>/artifacts/`. Callers obtain it via
    /// `VaultRoot::project_artifacts_dir(project_id)`.
    pub fn new(artifacts_root: &'a Path, notes: &'a [LocalNote]) -> Self {
        let notes_by_id = notes.iter().map(|n| (n.id, n)).collect();
        Self { artifacts_root, notes_by_id }
    }

    /// Directory containing the artifact's `index.md`. Returns `None` when
    /// `note_id` is unknown, isn't an artifact, or is missing a slug
    /// (which means migration 018 hasn't backfilled it yet — callers
    /// should treat this as a transient condition).
    pub fn artifact_dir(&self, note_id: Uuid) -> Option<PathBuf> {
        let chain = self.collect_slug_chain(note_id)?;
        let mut path = self.artifacts_root.to_path_buf();
        for slug in chain {
            path.push(slug);
        }
        Some(path)
    }

    /// Full path to the artifact's body file.
    pub fn artifact_index_path(&self, note_id: Uuid) -> Option<PathBuf> {
        let mut p = self.artifact_dir(note_id)?;
        p.push(ARTIFACT_INDEX_FILENAME);
        Some(p)
    }

    /// Returns `true` iff `note_id` refers to an artifact note we have a
    /// resolvable path for. Used by `ArtifactPersistence` to decide whether
    /// to delegate to the inner opaque store.
    pub fn is_artifact(&self, note_id: Uuid) -> bool {
        self.notes_by_id
            .get(&note_id)
            .is_some_and(|n| matches!(n.kind, NoteKind::Artifact))
    }

    /// Root-to-leaf slug chain. Walks `parent_id` collecting each artifact
    /// ancestor's slug, stops at the first non-artifact or absent parent,
    /// then reverses so the result is `[root, …, self]`.
    fn collect_slug_chain(&self, note_id: Uuid) -> Option<Vec<&'a str>> {
        let mut leaf = *self.notes_by_id.get(&note_id)?;
        if !matches!(leaf.kind, NoteKind::Artifact) {
            return None;
        }
        let mut rev: Vec<&str> = Vec::new();
        loop {
            if !matches!(leaf.kind, NoteKind::Artifact) {
                break;
            }
            let slug = leaf.slug.as_deref()?;
            rev.push(slug);
            match leaf.parent_id {
                Some(pid) => match self.notes_by_id.get(&pid).copied() {
                    Some(parent) => leaf = parent,
                    None => break,
                },
                None => break,
            }
        }
        rev.reverse();
        Some(rev)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn art(id: Uuid, parent: Option<Uuid>, slug: &str) -> LocalNote {
        LocalNote {
            id,
            project_id: Uuid::nil(),
            parent_id: parent,
            sibling_index: 0,
            depth: 0,
            title: "ignored".into(),
            created_at_ms: 0,
            updated_at_ms: 0,
            kind: NoteKind::Artifact,
            blob_path: None,
            slug: Some(slug.into()),
        }
    }

    fn md(id: Uuid, parent: Option<Uuid>) -> LocalNote {
        LocalNote {
            id,
            project_id: Uuid::nil(),
            parent_id: parent,
            sibling_index: 0,
            depth: 0,
            title: "md".into(),
            created_at_ms: 0,
            updated_at_ms: 0,
            kind: NoteKind::Markdown,
            blob_path: None,
            slug: None,
        }
    }

    #[test]
    fn resolves_nested_artifact_chain() {
        let root = Uuid::new_v4();
        let mid = Uuid::new_v4();
        let leaf = Uuid::new_v4();
        let notes = vec![
            art(root, None, "ce-inputs"),
            art(mid, Some(root), "epic-01"),
            art(leaf, Some(mid), "feature-01"),
        ];
        let artifacts_root = Path::new("/vault/.operon/00000000-0000-0000-0000-000000000000/artifacts");
        let r = ArtifactPathResolver::new(artifacts_root, &notes);
        assert_eq!(
            r.artifact_index_path(leaf).unwrap(),
            artifacts_root.join("ce-inputs/epic-01/feature-01/index.md")
        );
        assert_eq!(
            r.artifact_dir(root).unwrap(),
            artifacts_root.join("ce-inputs")
        );
    }

    #[test]
    fn stops_at_non_artifact_parent() {
        let folder = Uuid::new_v4();
        let root = Uuid::new_v4();
        let leaf = Uuid::new_v4();
        let notes = vec![
            md(folder, None),
            art(root, Some(folder), "ce-inputs"),
            art(leaf, Some(root), "epic-01"),
        ];
        let artifacts_root = Path::new("/vault/.operon/00000000-0000-0000-0000-000000000000/artifacts");
        let r = ArtifactPathResolver::new(artifacts_root, &notes);
        assert_eq!(
            r.artifact_index_path(leaf).unwrap(),
            artifacts_root.join("ce-inputs/epic-01/index.md")
        );
    }

    #[test]
    fn non_artifact_target_returns_none() {
        let folder = Uuid::new_v4();
        let notes = vec![md(folder, None)];
        let artifacts_root = Path::new("/vault/.operon/00000000-0000-0000-0000-000000000000/artifacts");
        let r = ArtifactPathResolver::new(artifacts_root, &notes);
        assert!(r.artifact_index_path(folder).is_none());
        assert!(!r.is_artifact(folder));
    }

    #[test]
    fn missing_slug_returns_none() {
        let id = Uuid::new_v4();
        let mut n = art(id, None, "x");
        n.slug = None;
        let notes = vec![n];
        let artifacts_root = Path::new("/vault/.operon/00000000-0000-0000-0000-000000000000/artifacts");
        let r = ArtifactPathResolver::new(artifacts_root, &notes);
        assert!(r.artifact_index_path(id).is_none());
    }
}
