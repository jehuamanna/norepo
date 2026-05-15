//! Notes vault directory: load/store/validate/lock.
//!
//! Desktop-side implementation for Plans-Phase-1-vault-dir. The vault root is
//! the absolute path the user picks on first run; markdown bodies live under
//! `<vault>/notes/` and image blobs under `<vault>/.operon/images/`. The path
//! is persisted in `local_app_settings` under `SETTINGS_KEY_VAULT_ROOT`.
//!
//! Per-project operon metadata (artifacts, outputs, trash, cascade state)
//! lives at `<vault>/.operon/<project-id>/…`, NOT inside the user's git
//! repository. Keeping it out of `<repo>` means Claude Code (whose `cwd`
//! is the repo) sees only business source code.

#![cfg(not(target_arch = "wasm32"))]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use operon_store::repos::LocalSettingsRepository;
use uuid::Uuid;

use super::SETTINGS_KEY_VAULT_ROOT;

/// Resolved vault root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VaultRoot {
    pub path: PathBuf,
}

impl VaultRoot {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn notes_dir(&self) -> PathBuf {
        self.path.join("notes")
    }

    pub fn images_dir(&self) -> PathBuf {
        self.path.join(".operon/images")
    }

    fn lock_path(&self) -> PathBuf {
        self.path.join(".operon/lock")
    }

    /// Per-project operon root: `<vault>/.operon/<project-id>/`. Parent
    /// of every per-project sidecar (artifacts, outputs, trash, etc.).
    pub fn project_operon_dir(&self, project_id: Uuid) -> PathBuf {
        self.path.join(".operon").join(project_id.to_string())
    }

    pub fn project_artifacts_dir(&self, project_id: Uuid) -> PathBuf {
        self.project_operon_dir(project_id).join("artifacts")
    }

    pub fn project_outputs_dir(&self, project_id: Uuid) -> PathBuf {
        self.project_operon_dir(project_id).join("outputs")
    }

    pub fn project_trash_dir(&self, project_id: Uuid) -> PathBuf {
        self.project_operon_dir(project_id).join("trash")
    }

    pub fn project_cascade_stages_path(&self, project_id: Uuid) -> PathBuf {
        self.project_operon_dir(project_id)
            .join("cascade-stages.json")
    }

    pub fn project_artifact_layout_sentinel(&self, project_id: Uuid) -> PathBuf {
        self.project_operon_dir(project_id)
            .join(".artifact-layout-v1")
    }
}

#[derive(Debug)]
pub enum VaultErr {
    NotSet,
    NotFound(PathBuf),
    NotWritable(PathBuf),
    Locked,
    Settings(String),
    Io(std::io::Error),
}

impl std::fmt::Display for VaultErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotSet => f.write_str("vault path is not set"),
            Self::NotFound(p) => write!(f, "vault path does not exist or is not a directory: {}", p.display()),
            Self::NotWritable(p) => write!(f, "vault path is not writable: {}", p.display()),
            Self::Locked => f.write_str("vault is already locked by another running instance"),
            Self::Settings(s) => write!(f, "vault settings I/O failed: {s}"),
            Self::Io(e) => write!(f, "vault filesystem I/O failed: {e}"),
        }
    }
}

impl std::error::Error for VaultErr {}

impl From<std::io::Error> for VaultErr {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Read the persisted vault root from settings.
///
/// Returns `Ok(None)` when no vault has been picked yet (first run). Returns
/// `Err(VaultErr::Settings)` if the underlying KV store fails.
pub fn load(
    settings: &Arc<dyn LocalSettingsRepository>,
) -> Result<Option<VaultRoot>, VaultErr> {
    let raw = settings
        .get(SETTINGS_KEY_VAULT_ROOT)
        .map_err(|e| VaultErr::Settings(e.to_string()))?;
    Ok(raw
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .map(|path| VaultRoot { path }))
}

/// Persist the vault root path in settings.
pub fn store(
    settings: &Arc<dyn LocalSettingsRepository>,
    root: &VaultRoot,
) -> Result<(), VaultErr> {
    let s = root
        .path
        .to_str()
        .ok_or_else(|| VaultErr::Settings("vault path is not valid UTF-8".into()))?;
    settings
        .set(SETTINGS_KEY_VAULT_ROOT, s)
        .map_err(|e| VaultErr::Settings(e.to_string()))
}

/// Reject paths that don't exist, aren't directories, or are read-only.
///
/// Path-traversal segments (`..`) are eliminated by canonicalizing first; if
/// canonicalization fails, the path is treated as not-found.
pub fn validate(path: &Path) -> Result<PathBuf, VaultErr> {
    let canonical = fs::canonicalize(path).map_err(|_| VaultErr::NotFound(path.into()))?;
    let meta = fs::metadata(&canonical).map_err(|_| VaultErr::NotFound(canonical.clone()))?;
    if !meta.is_dir() {
        return Err(VaultErr::NotFound(canonical));
    }
    let probe_dir = canonical.join(".operon");
    fs::create_dir_all(&probe_dir).map_err(|_| VaultErr::NotWritable(canonical.clone()))?;
    let probe_file = probe_dir.join(".write_probe");
    let mut file =
        fs::File::create(&probe_file).map_err(|_| VaultErr::NotWritable(canonical.clone()))?;
    file.write_all(b"ok")
        .map_err(|_| VaultErr::NotWritable(canonical.clone()))?;
    drop(file);
    let _ = fs::remove_file(&probe_file);
    Ok(canonical)
}

/// Best-effort lock against concurrent app instances writing to the same
/// vault. Writes `pid=<pid>` to `<vault>/.operon/lock`. The returned
/// [`LockGuard`] removes the file on drop.
pub fn acquire_lock(root: &VaultRoot) -> Result<LockGuard, VaultErr> {
    let lock = root.lock_path();
    if let Some(parent) = lock.parent() {
        fs::create_dir_all(parent)?;
    }
    if lock.exists() {
        return Err(VaultErr::Locked);
    }
    let mut file = fs::File::create(&lock)?;
    let pid = std::process::id();
    let started = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    writeln!(file, "pid={pid}")?;
    writeln!(file, "start_ms={started}")?;
    Ok(LockGuard { path: lock })
}

/// RAII guard that removes the lock file when dropped.
#[derive(Debug)]
#[must_use = "drop the LockGuard explicitly to release the vault lock"]
pub struct LockGuard {
    path: PathBuf,
}

impl LockGuard {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn validate_rejects_missing_path() {
        let tmp = tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");
        let err = validate(&missing).unwrap_err();
        assert!(matches!(err, VaultErr::NotFound(_)), "got {err:?}");
    }

    #[test]
    fn validate_rejects_file_path() {
        let tmp = tempdir().unwrap();
        let f = tmp.path().join("a-file");
        std::fs::write(&f, b"x").unwrap();
        let err = validate(&f).unwrap_err();
        assert!(matches!(err, VaultErr::NotFound(_)), "got {err:?}");
    }

    #[test]
    fn validate_accepts_writable_dir_and_creates_operon() {
        let tmp = tempdir().unwrap();
        let canonical = validate(tmp.path()).unwrap();
        assert!(canonical.join(".operon").is_dir());
    }

    #[test]
    fn acquire_lock_creates_then_releases_lock_file() {
        let tmp = tempdir().unwrap();
        let root = VaultRoot {
            path: tmp.path().to_path_buf(),
        };
        let guard = acquire_lock(&root).unwrap();
        assert!(guard.path().exists());
        drop(guard);
        assert!(!root.lock_path().exists());
    }

    #[test]
    fn acquire_lock_fails_when_lock_held() {
        let tmp = tempdir().unwrap();
        let root = VaultRoot {
            path: tmp.path().to_path_buf(),
        };
        let _g = acquire_lock(&root).unwrap();
        let err = acquire_lock(&root).unwrap_err();
        assert!(matches!(err, VaultErr::Locked), "got {err:?}");
    }
}
