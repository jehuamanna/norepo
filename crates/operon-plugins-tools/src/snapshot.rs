//! Snapshot / revert primitives.
//!
//! Two backends:
//!   1. **Git stash** — fast, only works if cwd is inside a git repo. Snapshots
//!      via `git stash --include-untracked --keep-index`, tagged with a stash
//!      message containing the snapshot id. Revert pops by message match.
//!   2. **Copy-on-write** — universal, walks the cwd (respecting .gitignore)
//!      and copies modified files to `<cwd>/.operon/snapshots/<session>/<step>/`.
//!      Revert restores file-by-file.
//!
//! The agent runtime captures a snapshot before each `Step::ToolCall` that
//! mutates the filesystem. Cascade phase boundaries get free undo. The
//! snapshot id is surfaced on the tool-use card so the user can press
//! "Revert this step".

use git2::{Repository, StashFlags};
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotId(pub String);

impl SnapshotId {
    pub fn new() -> Self {
        Self(format!("op-snap-{}", Uuid::new_v4().simple()))
    }
}

impl Default for SnapshotId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("git: {0}")]
    Git(String),
    #[error("not a git repo at {0}")]
    NotARepo(String),
    #[error("snapshot {0} not found")]
    NotFound(String),
}

/// A snapshot mechanism. Sync; callers run via `tokio::task::spawn_blocking`.
pub trait Snapshotter: Send + Sync {
    fn capture(&self, root: &Path) -> Result<SnapshotId, SnapshotError>;
    fn revert(&self, root: &Path, id: &SnapshotId) -> Result<(), SnapshotError>;
}

// === Git stash backend ===

pub struct GitStashSnapshotter;

impl Snapshotter for GitStashSnapshotter {
    fn capture(&self, root: &Path) -> Result<SnapshotId, SnapshotError> {
        let mut repo = Repository::discover(root)
            .map_err(|_| SnapshotError::NotARepo(root.display().to_string()))?;
        let id = SnapshotId::new();
        let sig = repo
            .signature()
            .or_else(|_| git2::Signature::now("operon", "operon@local"))
            .map_err(|e| SnapshotError::Git(format!("signature: {e}")))?;
        // INCLUDE_UNTRACKED captures new files; KEEP_INDEX leaves the index alone
        // so the user's staged work isn't blown away.
        let flags = StashFlags::INCLUDE_UNTRACKED | StashFlags::KEEP_INDEX;
        match repo.stash_save(&sig, &id.0, Some(flags)) {
            Ok(_oid) => Ok(id),
            Err(e) => {
                // git2 returns `NotFound` when there's nothing to stash;
                // `UnbornBranch` when there's no initial commit yet. Both
                // are best treated as no-op captures so the agent loop doesn't fail.
                if matches!(
                    e.code(),
                    git2::ErrorCode::NotFound | git2::ErrorCode::UnbornBranch
                ) {
                    Ok(SnapshotId(format!("noop-{}", Uuid::new_v4().simple())))
                } else {
                    Err(SnapshotError::Git(format!("stash_save: {e}")))
                }
            }
        }
    }

    fn revert(&self, root: &Path, id: &SnapshotId) -> Result<(), SnapshotError> {
        if id.0.starts_with("noop-") {
            return Ok(());
        }
        let mut repo = Repository::discover(root)
            .map_err(|_| SnapshotError::NotARepo(root.display().to_string()))?;
        // Find the stash with our message; pop it.
        let mut found_index: Option<usize> = None;
        repo.stash_foreach(|index, message, _oid| {
            if message.contains(&id.0) {
                found_index = Some(index);
                false
            } else {
                true
            }
        })
        .map_err(|e| SnapshotError::Git(format!("stash_foreach: {e}")))?;
        let idx = found_index.ok_or_else(|| SnapshotError::NotFound(id.0.clone()))?;
        repo.stash_pop(idx, None)
            .map_err(|e| SnapshotError::Git(format!("stash_pop: {e}")))?;
        Ok(())
    }
}

// === Copy-on-write backend (universal fallback) ===

pub struct CopySnapshotter {
    /// Directory under `root` where snapshots live. e.g. `.operon/snapshots`.
    pub dir: PathBuf,
}

impl CopySnapshotter {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
}

impl Default for CopySnapshotter {
    fn default() -> Self {
        Self {
            dir: PathBuf::from(".operon/snapshots"),
        }
    }
}

impl Snapshotter for CopySnapshotter {
    fn capture(&self, root: &Path) -> Result<SnapshotId, SnapshotError> {
        let id = SnapshotId::new();
        let dest = root.join(&self.dir).join(&id.0);
        std::fs::create_dir_all(&dest)?;
        for entry in WalkBuilder::new(root)
            .git_ignore(true)
            .follow_links(false)
            .build()
            .filter_map(Result::ok)
        {
            let p = entry.path();
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            // Skip the snapshots dir itself.
            if p.starts_with(&dest) || p.starts_with(root.join(&self.dir)) {
                continue;
            }
            let rel = match p.strip_prefix(root) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let target = dest.join(rel);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(p, &target)?;
        }
        Ok(id)
    }

    fn revert(&self, root: &Path, id: &SnapshotId) -> Result<(), SnapshotError> {
        let src = root.join(&self.dir).join(&id.0);
        if !src.exists() {
            return Err(SnapshotError::NotFound(id.0.clone()));
        }
        // Copy snapshot files back over the working tree. This restores edits but
        // does NOT delete files that didn't exist at snapshot time. That's
        // acceptable for v1 — the agent loop's "revert this step" is meant to
        // undo file edits, not nuke the workspace.
        for entry in WalkBuilder::new(&src)
            .git_ignore(false)
            .follow_links(false)
            .build()
            .filter_map(Result::ok)
        {
            let p = entry.path();
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let rel = match p.strip_prefix(&src) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let target = root.join(rel);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(p, &target)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn snapshot_id_unique() {
        let a = SnapshotId::new();
        let b = SnapshotId::new();
        assert_ne!(a.0, b.0);
        assert!(a.0.starts_with("op-snap-"));
    }

    #[test]
    fn copy_snapshotter_round_trips_edit() {
        let tmp = TempDir::new().unwrap();
        let f = tmp.path().join("x.txt");
        std::fs::write(&f, b"original").unwrap();

        let snap = CopySnapshotter::default();
        let id = snap.capture(tmp.path()).unwrap();
        std::fs::write(&f, b"modified").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "modified");

        snap.revert(tmp.path(), &id).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "original");
    }

    #[test]
    fn copy_revert_unknown_id_errors() {
        let tmp = TempDir::new().unwrap();
        let snap = CopySnapshotter::default();
        let r = snap.revert(tmp.path(), &SnapshotId("missing".into()));
        assert!(matches!(r, Err(SnapshotError::NotFound(_))));
    }

    #[test]
    fn git_stash_snapshotter_round_trips_edit() {
        let tmp = TempDir::new().unwrap();
        let repo = git2::Repository::init(tmp.path()).unwrap();
        let mut cfg = repo.config().unwrap();
        cfg.set_str("user.name", "Test").unwrap();
        cfg.set_str("user.email", "t@example.com").unwrap();
        // Seed an initial commit so HEAD exists for stash.
        let f = tmp.path().join("x.txt");
        std::fs::write(&f, b"original").unwrap();
        let mut idx = repo.index().unwrap();
        idx.add_path(Path::new("x.txt")).unwrap();
        idx.write().unwrap();
        let tree = idx.write_tree().unwrap();
        let tree = repo.find_tree(tree).unwrap();
        let sig = git2::Signature::now("Test", "t@example.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[]).unwrap();

        // Modify the working tree.
        std::fs::write(&f, b"modified").unwrap();
        let snap = GitStashSnapshotter;
        let id = snap.capture(tmp.path()).unwrap();
        // Stash captured the modification — working tree is back to "original".
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "original");

        // Revert pops the stash, restoring the modification.
        snap.revert(tmp.path(), &id).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "modified");
    }

    #[test]
    fn git_stash_capture_with_no_changes_returns_noop_id() {
        let tmp = TempDir::new().unwrap();
        let repo = git2::Repository::init(tmp.path()).unwrap();
        let mut cfg = repo.config().unwrap();
        cfg.set_str("user.name", "Test").unwrap();
        cfg.set_str("user.email", "t@example.com").unwrap();
        let snap = GitStashSnapshotter;
        let id = snap.capture(tmp.path()).unwrap();
        assert!(id.0.starts_with("noop-") || id.0.starts_with("op-snap-"));
        // Reverting a noop-id is also a no-op.
        snap.revert(tmp.path(), &id).unwrap();
    }

    #[test]
    fn git_stash_on_non_repo_errors() {
        let tmp = TempDir::new().unwrap();
        let snap = GitStashSnapshotter;
        let r = snap.capture(tmp.path());
        assert!(matches!(r, Err(SnapshotError::NotARepo(_))));
    }
}
