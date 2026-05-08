//! Write a skill note's body to `<repo>/.claude/skills/<slug>.md` so
//! Claude Code's native skill loader can resolve it on the next turn.
//! Operon owns the round-trip — the skill note in SQLite is the source
//! of truth; the materialized `.md` is a derived cache rewritten on
//! every Play.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum MaterializeError {
    Io(io::Error),
    EmptyBody,
}

impl std::fmt::Display for MaterializeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::EmptyBody => write!(f, "skill body is empty"),
        }
    }
}

impl std::error::Error for MaterializeError {}

impl From<io::Error> for MaterializeError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Returns the absolute path of the materialized file on success.
pub fn write_skill_to_repo(
    repo_path: &Path,
    slug: &str,
    body: &str,
) -> Result<PathBuf, MaterializeError> {
    if body.trim().is_empty() {
        return Err(MaterializeError::EmptyBody);
    }
    let dir = repo_path.join(".claude").join("skills");
    fs::create_dir_all(&dir)?;
    let target = dir.join(format!("{slug}.md"));
    fs::write(&target, body)?;
    Ok(target)
}

/// Remove a previously-materialized skill file. No-op if it didn't exist.
pub fn remove_skill_from_repo(repo_path: &Path, slug: &str) -> Result<(), MaterializeError> {
    let target = repo_path.join(".claude").join("skills").join(format!("{slug}.md"));
    if target.exists() {
        fs::remove_file(target)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn write_creates_dir_and_file() {
        let tmp = tempdir().unwrap();
        let path = write_skill_to_repo(tmp.path(), "ba-intake", "you are a BA").unwrap();
        assert!(path.is_file());
        let body = fs::read_to_string(&path).unwrap();
        assert_eq!(body, "you are a BA");
        let dir = tmp.path().join(".claude").join("skills");
        assert!(dir.is_dir());
    }

    #[test]
    fn write_overwrites_existing() {
        let tmp = tempdir().unwrap();
        write_skill_to_repo(tmp.path(), "x", "v1").unwrap();
        let path = write_skill_to_repo(tmp.path(), "x", "v2").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "v2");
    }

    #[test]
    fn write_rejects_empty_body() {
        let tmp = tempdir().unwrap();
        let err = write_skill_to_repo(tmp.path(), "x", "   \n").unwrap_err();
        assert!(matches!(err, MaterializeError::EmptyBody));
    }

    #[test]
    fn remove_is_noop_for_missing() {
        let tmp = tempdir().unwrap();
        // No file was ever written.
        remove_skill_from_repo(tmp.path(), "nonexistent").unwrap();
    }

    #[test]
    fn remove_deletes_existing() {
        let tmp = tempdir().unwrap();
        let path = write_skill_to_repo(tmp.path(), "doomed", "body").unwrap();
        remove_skill_from_repo(tmp.path(), "doomed").unwrap();
        assert!(!path.exists());
    }
}
