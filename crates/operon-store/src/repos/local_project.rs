//! Local-mode project entity. Backed by `local_project`. Sibling indices are
//! kept dense — `reorder` shifts neighbours inside a single transaction so the
//! list never has gaps.

use crate::sql::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

use crate::error::StoreError;
use crate::store::Store;
use crate::time::now_ms;

const DEFAULT_PROJECT_NAME: &str = "Untitled project";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalProject {
    pub id: Uuid,
    pub name: String,
    pub sibling_index: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    /// Absolute path to the git repository this project operates on. The
    /// companion-pane Claude Code subprocess uses this as cwd. `None` means
    /// the user hasn't bound a repo yet — chat is disabled until they do.
    #[serde(default)]
    pub repo_path: Option<PathBuf>,
}

pub trait LocalProjectRepository: Send + Sync {
    fn list(&self) -> Result<Vec<LocalProject>, StoreError>;
    fn create(&self, name: &str) -> Result<LocalProject, StoreError>;
    fn rename(&self, id: Uuid, name: &str) -> Result<(), StoreError>;
    fn delete(&self, id: Uuid) -> Result<(), StoreError>;
    fn reorder(&self, id: Uuid, new_sibling_index: i64) -> Result<(), StoreError>;
    /// Bind/unbind the project's git repository. Pass `None` to clear.
    fn set_repo_path(&self, id: Uuid, repo_path: Option<&std::path::Path>)
        -> Result<(), StoreError>;
}

pub struct SqliteLocalProjectRepository {
    store: Store,
}

impl SqliteLocalProjectRepository {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

fn invalid_uuid(s: String) -> crate::sql::Error {
    crate::sql::Error::FromSqlConversionFailure(
        0,
        crate::sql::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid uuid: {s}"),
        )),
    )
}

fn row_to_local_project(row: &crate::sql::Row<'_>) -> crate::sql::Result<LocalProject> {
    let id_text: String = row.get(0)?;
    let id = Uuid::parse_str(&id_text).map_err(|_| invalid_uuid(id_text))?;
    let repo_path: Option<String> = row.get(5).ok();
    Ok(LocalProject {
        id,
        name: row.get(1)?,
        sibling_index: row.get(2)?,
        created_at_ms: row.get(3)?,
        updated_at_ms: row.get(4)?,
        repo_path: repo_path.map(PathBuf::from),
    })
}

impl LocalProjectRepository for SqliteLocalProjectRepository {
    fn list(&self) -> Result<Vec<LocalProject>, StoreError> {
        let conn = self.store.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, sibling_index, created_at_ms, updated_at_ms, repo_path
             FROM local_project
             ORDER BY sibling_index ASC, created_at_ms ASC",
        )?;
        let rows = stmt.query_map(params![], row_to_local_project)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    fn create(&self, name: &str) -> Result<LocalProject, StoreError> {
        let trimmed = name.trim();
        let resolved_name = if trimmed.is_empty() {
            DEFAULT_PROJECT_NAME
        } else {
            trimmed
        };
        let id = Uuid::new_v4();
        let now = now_ms();
        let mut conn = self.store.conn()?;
        let tx = conn.transaction()?;
        let next_index: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(sibling_index), -1) + 1 FROM local_project",
                params![],
                |row| row.get(0),
            )
            .unwrap_or(0);
        tx.execute(
            "INSERT INTO local_project (id, name, sibling_index, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            params![id.to_string(), resolved_name, next_index, now],
        )?;
        tx.commit()?;
        Ok(LocalProject {
            id,
            name: resolved_name.to_string(),
            sibling_index: next_index,
            created_at_ms: now,
            updated_at_ms: now,
            repo_path: None,
        })
    }

    fn set_repo_path(
        &self,
        id: Uuid,
        repo_path: Option<&std::path::Path>,
    ) -> Result<(), StoreError> {
        let now = now_ms();
        let conn = self.store.conn()?;
        let path_str = repo_path.map(|p| p.to_string_lossy().into_owned());
        let n = conn.execute(
            "UPDATE local_project SET repo_path = ?2, updated_at_ms = ?3 WHERE id = ?1",
            params![id.to_string(), path_str, now],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    fn rename(&self, id: Uuid, name: &str) -> Result<(), StoreError> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(StoreError::InvalidArgument(
                "project name must not be empty or whitespace-only".into(),
            ));
        }
        let now = now_ms();
        let conn = self.store.conn()?;
        let n = conn.execute(
            "UPDATE local_project SET name = ?2, updated_at_ms = ?3 WHERE id = ?1",
            params![id.to_string(), trimmed, now],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    fn delete(&self, id: Uuid) -> Result<(), StoreError> {
        let mut conn = self.store.conn()?;
        let tx = conn.transaction()?;
        let removed_index: Option<i64> = tx
            .query_row(
                "SELECT sibling_index FROM local_project WHERE id = ?1",
                params![id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        let Some(removed_index) = removed_index else {
            tx.commit()?;
            return Ok(());
        };
        tx.execute(
            "DELETE FROM local_project WHERE id = ?1",
            params![id.to_string()],
        )?;
        // Keep indices dense: shift everything above the removed slot down by one.
        tx.execute(
            "UPDATE local_project SET sibling_index = sibling_index - 1
             WHERE sibling_index > ?1",
            params![removed_index],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn reorder(&self, id: Uuid, new_sibling_index: i64) -> Result<(), StoreError> {
        let mut conn = self.store.conn()?;
        let tx = conn.transaction()?;
        let current_index: Option<i64> = tx
            .query_row(
                "SELECT sibling_index FROM local_project WHERE id = ?1",
                params![id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        let Some(current_index) = current_index else {
            return Err(StoreError::NotFound);
        };
        let count: i64 = tx.query_row("SELECT COUNT(*) FROM local_project", params![], |row| {
            row.get(0)
        })?;
        // Clamp the requested target into [0, count - 1] so callers can pass
        // arbitrary indices without overshooting the dense range.
        let target = new_sibling_index.clamp(0, count.saturating_sub(1));
        if target == current_index {
            tx.commit()?;
            return Ok(());
        }
        let now = now_ms();
        if target > current_index {
            // Moving down: shift the range (current, target] up by one.
            tx.execute(
                "UPDATE local_project SET sibling_index = sibling_index - 1
                 WHERE sibling_index > ?1 AND sibling_index <= ?2",
                params![current_index, target],
            )?;
        } else {
            // Moving up: shift the range [target, current) down by one.
            tx.execute(
                "UPDATE local_project SET sibling_index = sibling_index + 1
                 WHERE sibling_index >= ?1 AND sibling_index < ?2",
                params![target, current_index],
            )?;
        }
        tx.execute(
            "UPDATE local_project SET sibling_index = ?2, updated_at_ms = ?3 WHERE id = ?1",
            params![id.to_string(), target, now],
        )?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::open_in_memory;

    fn make_repo() -> SqliteLocalProjectRepository {
        let store = open_in_memory().unwrap();
        SqliteLocalProjectRepository::new(store)
    }

    #[test]
    fn local_project_repo_create_assigns_monotonic_sibling_index() {
        let repo = make_repo();
        let a = repo.create("alpha").unwrap();
        let b = repo.create("beta").unwrap();
        let c = repo.create("gamma").unwrap();
        assert_eq!(a.sibling_index, 0);
        assert_eq!(b.sibling_index, 1);
        assert_eq!(c.sibling_index, 2);

        let listed = repo.list().unwrap();
        assert_eq!(
            listed.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["alpha", "beta", "gamma"]
        );
    }

    #[test]
    fn local_project_repo_rename_updates_name_and_timestamp() {
        let repo = make_repo();
        let a = repo.create("alpha").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        repo.rename(a.id, "  Renamed  ").unwrap();
        let listed = repo.list().unwrap();
        let got = listed.iter().find(|p| p.id == a.id).unwrap();
        assert_eq!(got.name, "Renamed");
        assert!(got.updated_at_ms > a.updated_at_ms);
        assert_eq!(got.created_at_ms, a.created_at_ms);
    }

    #[test]
    fn local_project_repo_rename_rejects_empty_or_whitespace() {
        let repo = make_repo();
        let a = repo.create("alpha").unwrap();
        for bad in ["", "   ", "\t\n  "] {
            let err = repo.rename(a.id, bad).unwrap_err();
            assert!(
                matches!(err, StoreError::InvalidArgument(_)),
                "expected InvalidArgument for {bad:?}, got {err:?}"
            );
        }
        // Name unchanged.
        let listed = repo.list().unwrap();
        assert_eq!(listed[0].name, "alpha");
    }

    #[test]
    fn local_project_repo_create_with_empty_name_uses_default() {
        let repo = make_repo();
        let a = repo.create("").unwrap();
        let b = repo.create("   \t\n").unwrap();
        assert_eq!(a.name, DEFAULT_PROJECT_NAME);
        assert_eq!(b.name, DEFAULT_PROJECT_NAME);
    }

    #[test]
    fn local_project_repo_delete_removes_row() {
        let repo = make_repo();
        let a = repo.create("alpha").unwrap();
        let b = repo.create("beta").unwrap();
        let c = repo.create("gamma").unwrap();
        repo.delete(b.id).unwrap();
        let listed = repo.list().unwrap();
        assert_eq!(listed.len(), 2);
        let names: Vec<&str> = listed.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "gamma"]);
        // Indices stay dense.
        assert_eq!(listed[0].id, a.id);
        assert_eq!(listed[0].sibling_index, 0);
        assert_eq!(listed[1].id, c.id);
        assert_eq!(listed[1].sibling_index, 1);
    }

    #[test]
    fn local_project_repo_delete_unknown_id_is_noop() {
        let repo = make_repo();
        let _a = repo.create("alpha").unwrap();
        repo.delete(Uuid::new_v4()).unwrap();
        let listed = repo.list().unwrap();
        assert_eq!(listed.len(), 1);
    }

    #[test]
    fn local_project_set_repo_path_round_trips() {
        let repo = make_repo();
        let a = repo.create("alpha").unwrap();
        // Defaults to None.
        assert_eq!(repo.list().unwrap()[0].repo_path, None);

        let path: std::path::PathBuf = "/tmp/some/repo".into();
        repo.set_repo_path(a.id, Some(&path)).unwrap();
        let listed = repo.list().unwrap();
        assert_eq!(listed[0].repo_path.as_ref(), Some(&path));

        // Clearing back to None.
        repo.set_repo_path(a.id, None).unwrap();
        assert_eq!(repo.list().unwrap()[0].repo_path, None);
    }

    #[test]
    fn local_project_set_repo_path_on_unknown_id_errors() {
        let repo = make_repo();
        let _ = repo.create("alpha").unwrap();
        let err = repo
            .set_repo_path(Uuid::new_v4(), Some(std::path::Path::new("/x")))
            .unwrap_err();
        assert!(matches!(err, StoreError::NotFound));
    }

    #[test]
    fn local_project_repo_reorder_swaps_indices_atomically() {
        let repo = make_repo();
        let a = repo.create("alpha").unwrap();
        let b = repo.create("beta").unwrap();
        let c = repo.create("gamma").unwrap();

        // Move alpha (0) to position 2 -> order should become beta, gamma, alpha.
        repo.reorder(a.id, 2).unwrap();
        let listed = repo.list().unwrap();
        let names: Vec<&str> = listed.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["beta", "gamma", "alpha"]);
        let indices: Vec<i64> = listed.iter().map(|p| p.sibling_index).collect();
        assert_eq!(indices, vec![0, 1, 2]);

        // Move gamma (currently at index 1) up to 0 -> gamma, beta, alpha.
        repo.reorder(c.id, 0).unwrap();
        let listed = repo.list().unwrap();
        let names: Vec<&str> = listed.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["gamma", "beta", "alpha"]);
        let indices: Vec<i64> = listed.iter().map(|p| p.sibling_index).collect();
        assert_eq!(indices, vec![0, 1, 2]);

        // Out-of-range target clamps to last slot — beta moves to the end.
        repo.reorder(b.id, 999).unwrap();
        let listed = repo.list().unwrap();
        let names: Vec<&str> = listed.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["gamma", "alpha", "beta"]);
    }
}
