//! Session-scoped trash for note-delete recovery via Ctrl-Z.
//!
//! When a note is deleted, every on-disk side-effect it produced
//! (artifact dirs, materialized skills, orphaned image blobs) is
//! **moved** into a per-delete sub-folder of the trash root rather
//! than removed. The returned [`TrashRecord`] is attached to the
//! explorer's undo entry so [`TrashRecord::restore`] can move every
//! file back to its original location if the user hits Cmd/Ctrl+Z.
//!
//! Trash is **session-scoped**: it lives under `.operon/trash/` and
//! is wiped at app startup. We do not expose a trash UI in v1; the
//! only consumer is the undo path.
//!
//! Trash roots are per-filesystem because `std::fs::rename` cannot
//! cross filesystems. Two roots are typical:
//! - `<vault>/.operon/<project-id>/trash/` — for artifact dirs.
//!   Co-located with the artifacts they trash so renames don't
//!   cross filesystems.
//! - `<repo>/.claude/trash/` — for materialized skill files (those
//!   live under the project's repo at `<repo>/.claude/skills/…`,
//!   so their trash mirror needs to stay on the same filesystem).
//! - `<vault>/.operon/trash/` — for vault-rooted image blobs.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(not(target_arch = "wasm32"))]
use crate::local_mode::vault::VaultRoot;

/// Where in the trash a given file landed and what it was before.
/// Restoration is `rename(trashed, original)`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrashMove {
    pub original: PathBuf,
    pub trashed: PathBuf,
}

/// All moves recorded for a single delete operation. Lives inside
/// the explorer's undo entry; restored as a unit on Ctrl+Z.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrashRecord {
    pub trash_id: Uuid,
    pub moves: Vec<TrashMove>,
}

impl TrashRecord {
    pub fn new() -> Self {
        Self {
            trash_id: Uuid::new_v4(),
            moves: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.moves.is_empty()
    }

    /// Move `source` aside into the trash root. The directory tree
    /// under the trash root mirrors the source's path layout so that
    /// [`restore`](Self::restore) can simply walk back. Returns
    /// `Ok(false)` when the source doesn't exist (e.g. an artifact
    /// that was never written to disk), `Ok(true)` on a successful
    /// move, and `Err` on any other I/O failure.
    pub fn move_into_trash(
        &mut self,
        source: &Path,
        trash_root: &Path,
        relative_under_trash: &Path,
    ) -> io::Result<bool> {
        if !source.exists() {
            return Ok(false);
        }
        let trashed = trash_root
            .join(self.trash_id.to_string())
            .join(relative_under_trash);
        if let Some(parent) = trashed.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(source, &trashed)?;
        self.moves.push(TrashMove {
            original: source.to_path_buf(),
            trashed,
        });
        Ok(true)
    }

    /// Move every file back to its original location. Errors are
    /// logged via `tracing` and swallowed — a partial restore is
    /// preferable to aborting the undo entirely.
    pub fn restore(&self) {
        for mv in &self.moves {
            if let Some(parent) = mv.original.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    tracing::warn!(
                        target: "operon::cleanup::trash",
                        "trash restore create_dir_all {parent:?} failed: {e}"
                    );
                    continue;
                }
            }
            match std::fs::rename(&mv.trashed, &mv.original) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!(
                    target: "operon::cleanup::trash",
                    "trash restore rename {:?} -> {:?} failed: {e}",
                    mv.trashed,
                    mv.original,
                ),
            }
        }
    }

    /// Remove the per-delete trash dir for real. Called when an undo
    /// entry is no longer reachable (history evicted, app shutdown).
    /// Safe to call multiple times; subsequent calls are no-ops.
    pub fn purge(&self, repo_trash_root: &Path, vault_trash_root: Option<&Path>) {
        let repo_dir = repo_trash_root.join(self.trash_id.to_string());
        try_remove_dir(&repo_dir);
        if let Some(vroot) = vault_trash_root {
            let vault_dir = vroot.join(self.trash_id.to_string());
            try_remove_dir(&vault_dir);
        }
    }
}

impl Default for TrashRecord {
    fn default() -> Self {
        Self::new()
    }
}

/// Wipe the entire trash root. Call once at session start so leftover
/// trash from a previous run doesn't accumulate.
pub fn wipe_trash_root(root: &Path) {
    try_remove_dir(root);
}

/// Per-project trash root, anchored at the vault. Used for artifact
/// dirs (which now live at `<vault>/.operon/<project-id>/artifacts/`).
#[cfg(not(target_arch = "wasm32"))]
pub fn project_trash_root(vault: &VaultRoot, project_id: Uuid) -> PathBuf {
    vault.project_trash_dir(project_id)
}

/// Trash root for materialized skill files that still live under the
/// user's repo at `<repo>/.claude/skills/…`. Co-located with the
/// `.claude/` source dir so `std::fs::rename` never crosses
/// filesystems. NOT under `.operon/` — `.claude/` is the only operon-
/// managed folder we leave inside the repo.
pub fn repo_skill_trash_root(repo_path: &Path) -> PathBuf {
    repo_path.join(".claude").join("trash")
}

/// Standard sub-path for a vault-rooted trash root.
pub fn vault_trash_root(vault_path: &Path) -> PathBuf {
    vault_path.join(".operon").join("trash")
}

fn try_remove_dir(dir: &Path) {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(
            target: "operon::cleanup::trash",
            "remove_dir_all {dir:?} failed: {e}"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_then_restore_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let original = tmp.path().join("subdir").join("note.md");
        std::fs::create_dir_all(original.parent().unwrap()).unwrap();
        std::fs::write(&original, b"body").unwrap();

        let trash_root = tmp.path().join("trash");
        let mut rec = TrashRecord::new();
        let moved = rec
            .move_into_trash(&original, &trash_root, Path::new("note.md"))
            .unwrap();
        assert!(moved);
        assert!(!original.exists(), "file moved away");
        assert_eq!(rec.moves.len(), 1);

        rec.restore();
        assert!(original.is_file(), "file restored");
        assert_eq!(std::fs::read(&original).unwrap(), b"body");
    }

    #[test]
    fn move_skips_missing_source() {
        let tmp = tempfile::tempdir().unwrap();
        let mut rec = TrashRecord::new();
        let moved = rec
            .move_into_trash(
                &tmp.path().join("ghost"),
                &tmp.path().join("trash"),
                Path::new("ghost"),
            )
            .unwrap();
        assert!(!moved);
        assert!(rec.moves.is_empty());
    }

    #[test]
    fn purge_removes_per_delete_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let original = tmp.path().join("blob.png");
        std::fs::write(&original, b"img").unwrap();
        let trash_root = tmp.path().join("trash");
        let mut rec = TrashRecord::new();
        rec.move_into_trash(&original, &trash_root, Path::new("blob.png"))
            .unwrap();
        let trashed_dir = trash_root.join(rec.trash_id.to_string());
        assert!(trashed_dir.is_dir());

        rec.purge(&trash_root, None);
        assert!(!trashed_dir.exists());
    }
}
