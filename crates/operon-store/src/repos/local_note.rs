//! Local-mode note metadata. Backed by `local_note` (a sidecar to `local_project`).
//! Note content is stored in the Loro engine via `operon-notes`; this table only
//! holds the tree shape (parent/sibling/depth) and rename/timestamps.

use std::collections::HashMap;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::StoreError;
use crate::sqlite::Store;
use crate::time::now_ms;

const DEFAULT_NOTE_TITLE: &str = "Untitled";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalNote {
    pub id: Uuid,
    pub project_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub sibling_index: i64,
    pub depth: i64,
    pub title: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

pub trait LocalNoteRepository: Send + Sync {
    fn list_for_project(&self, project_id: Uuid) -> Result<Vec<LocalNote>, StoreError>;
    fn create(
        &self,
        project_id: Uuid,
        parent_id: Option<Uuid>,
        title: &str,
    ) -> Result<LocalNote, StoreError>;
    fn rename(&self, id: Uuid, title: &str) -> Result<(), StoreError>;
    fn delete(&self, id: Uuid) -> Result<(), StoreError>;
    fn touch_updated(&self, id: Uuid) -> Result<(), StoreError>;
}

pub struct SqliteLocalNoteRepository {
    store: Store,
}

impl SqliteLocalNoteRepository {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

fn invalid_uuid(s: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid uuid: {s}"),
        )),
    )
}

fn row_to_local_note(row: &rusqlite::Row<'_>) -> rusqlite::Result<LocalNote> {
    let id_text: String = row.get(0)?;
    let id = Uuid::parse_str(&id_text).map_err(|_| invalid_uuid(id_text))?;
    let project_text: String = row.get(1)?;
    let project_id = Uuid::parse_str(&project_text).map_err(|_| invalid_uuid(project_text))?;
    let parent_opt: Option<String> = row.get(2)?;
    let parent_id = match parent_opt {
        Some(s) => Some(Uuid::parse_str(&s).map_err(|_| invalid_uuid(s))?),
        None => None,
    };
    Ok(LocalNote {
        id,
        project_id,
        parent_id,
        sibling_index: row.get(3)?,
        depth: row.get(4)?,
        title: row.get(5)?,
        created_at_ms: row.get(6)?,
        updated_at_ms: row.get(7)?,
    })
}

const SELECT_COLS: &str =
    "id, project_id, parent_id, sibling_index, depth, title, created_at_ms, updated_at_ms";

impl LocalNoteRepository for SqliteLocalNoteRepository {
    fn list_for_project(&self, project_id: Uuid) -> Result<Vec<LocalNote>, StoreError> {
        let conn = self.store.conn()?;
        let sql = format!(
            "SELECT {SELECT_COLS} FROM local_note
             WHERE project_id = ?1
             ORDER BY parent_id IS NULL DESC, parent_id, sibling_index, created_at_ms"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![project_id.to_string()], row_to_local_note)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    fn create(
        &self,
        project_id: Uuid,
        parent_id: Option<Uuid>,
        title: &str,
    ) -> Result<LocalNote, StoreError> {
        let trimmed = title.trim();
        let resolved_title = if trimmed.is_empty() {
            DEFAULT_NOTE_TITLE
        } else {
            trimmed
        };
        let id = Uuid::new_v4();
        let now = now_ms();
        let mut conn = self.store.conn()?;
        let tx = conn.transaction()?;

        // Validate parent (when given) belongs to the same project, and derive depth.
        let depth = match parent_id {
            Some(pid) => {
                let parent_row: Option<(String, i64)> = tx
                    .query_row(
                        "SELECT project_id, depth FROM local_note WHERE id = ?1",
                        params![pid.to_string()],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                    )
                    .optional()?;
                let (parent_project, parent_depth) = parent_row.ok_or_else(|| {
                    StoreError::InvalidArgument(format!("parent note {pid} not found"))
                })?;
                let parent_project_uuid = Uuid::parse_str(&parent_project).map_err(|_| {
                    StoreError::InvalidArgument(format!(
                        "stored parent project_id is not a uuid: {parent_project}"
                    ))
                })?;
                if parent_project_uuid != project_id {
                    return Err(StoreError::InvalidArgument(
                        "parent note belongs to a different project".into(),
                    ));
                }
                parent_depth + 1
            }
            None => 0,
        };

        let next_index: i64 = match parent_id {
            Some(pid) => tx.query_row(
                "SELECT COALESCE(MAX(sibling_index), -1) + 1 FROM local_note
                 WHERE project_id = ?1 AND parent_id = ?2",
                params![project_id.to_string(), pid.to_string()],
                |row| row.get(0),
            )?,
            None => tx.query_row(
                "SELECT COALESCE(MAX(sibling_index), -1) + 1 FROM local_note
                 WHERE project_id = ?1 AND parent_id IS NULL",
                params![project_id.to_string()],
                |row| row.get(0),
            )?,
        };

        tx.execute(
            "INSERT INTO local_note (id, project_id, parent_id, sibling_index, depth,
                                     title, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![
                id.to_string(),
                project_id.to_string(),
                parent_id.map(|p| p.to_string()),
                next_index,
                depth,
                resolved_title,
                now,
            ],
        )?;
        tx.commit()?;
        Ok(LocalNote {
            id,
            project_id,
            parent_id,
            sibling_index: next_index,
            depth,
            title: resolved_title.to_string(),
            created_at_ms: now,
            updated_at_ms: now,
        })
    }

    fn rename(&self, id: Uuid, title: &str) -> Result<(), StoreError> {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return Err(StoreError::InvalidArgument(
                "note title must not be empty or whitespace-only".into(),
            ));
        }
        let now = now_ms();
        let conn = self.store.conn()?;
        let n = conn.execute(
            "UPDATE local_note SET title = ?2, updated_at_ms = ?3 WHERE id = ?1",
            params![id.to_string(), trimmed, now],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    fn delete(&self, id: Uuid) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        // ON DELETE CASCADE handles descendants.
        conn.execute(
            "DELETE FROM local_note WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(())
    }

    fn touch_updated(&self, id: Uuid) -> Result<(), StoreError> {
        let now = now_ms();
        let conn = self.store.conn()?;
        let n = conn.execute(
            "UPDATE local_note SET updated_at_ms = ?2 WHERE id = ?1",
            params![id.to_string(), now],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }
}

/// Same key surface as the `LocalNoteRepository` for callers that want to read
/// the open/closed state of a tree node within a scope.
pub type LocalTreeStateMap = HashMap<String, bool>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repos::{LocalProjectRepository, SqliteLocalProjectRepository};
    use crate::test_support::open_in_memory;

    fn make_pair() -> (
        SqliteLocalProjectRepository,
        SqliteLocalNoteRepository,
        Uuid,
    ) {
        let store = open_in_memory().unwrap();
        let project_repo = SqliteLocalProjectRepository::new(store.clone());
        let note_repo = SqliteLocalNoteRepository::new(store);
        let project = project_repo.create("alpha").unwrap();
        (project_repo, note_repo, project.id)
    }

    #[test]
    fn local_note_repo_create_under_project_returns_uuid_and_depth_zero() {
        let (_p, n, project_id) = make_pair();
        let note = n.create(project_id, None, "first").unwrap();
        assert_eq!(note.depth, 0);
        assert!(note.parent_id.is_none());
        assert_eq!(note.sibling_index, 0);
        assert_eq!(note.title, "first");
        assert_eq!(note.project_id, project_id);
    }

    #[test]
    fn local_note_repo_create_under_parent_increments_depth() {
        let (_p, n, project_id) = make_pair();
        let root = n.create(project_id, None, "root").unwrap();
        let child = n.create(project_id, Some(root.id), "child").unwrap();
        let grand = n.create(project_id, Some(child.id), "grand").unwrap();
        assert_eq!(root.depth, 0);
        assert_eq!(child.depth, 1);
        assert_eq!(grand.depth, 2);
        assert_eq!(child.parent_id, Some(root.id));
        assert_eq!(grand.parent_id, Some(child.id));
        // First child under each parent gets sibling_index 0.
        assert_eq!(child.sibling_index, 0);
        assert_eq!(grand.sibling_index, 0);
    }

    #[test]
    fn local_note_repo_rename_persists() {
        let (_p, n, project_id) = make_pair();
        let note = n.create(project_id, None, "original").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        n.rename(note.id, "  Updated  ").unwrap();
        let listed = n.list_for_project(project_id).unwrap();
        let got = listed.iter().find(|x| x.id == note.id).unwrap();
        assert_eq!(got.title, "Updated");
        assert!(got.updated_at_ms > note.updated_at_ms);
    }

    #[test]
    fn local_note_repo_rename_rejects_empty_or_whitespace() {
        let (_p, n, project_id) = make_pair();
        let note = n.create(project_id, None, "keep").unwrap();
        for bad in ["", "   ", "\t\n  "] {
            let err = n.rename(note.id, bad).unwrap_err();
            assert!(
                matches!(err, StoreError::InvalidArgument(_)),
                "expected InvalidArgument for {bad:?}, got {err:?}"
            );
        }
        let listed = n.list_for_project(project_id).unwrap();
        assert_eq!(listed[0].title, "keep");
    }

    #[test]
    fn local_note_repo_create_with_empty_title_uses_default() {
        let (_p, n, project_id) = make_pair();
        let a = n.create(project_id, None, "").unwrap();
        let b = n.create(project_id, None, "   \t\n").unwrap();
        assert_eq!(a.title, DEFAULT_NOTE_TITLE);
        assert_eq!(b.title, DEFAULT_NOTE_TITLE);
    }

    #[test]
    fn local_note_repo_delete_cascades_children() {
        let (_p, n, project_id) = make_pair();
        let root = n.create(project_id, None, "root").unwrap();
        let _c1 = n.create(project_id, Some(root.id), "c1").unwrap();
        let c2 = n.create(project_id, Some(root.id), "c2").unwrap();
        let _g = n.create(project_id, Some(c2.id), "g").unwrap();

        n.delete(root.id).unwrap();

        let listed = n.list_for_project(project_id).unwrap();
        assert!(
            listed.is_empty(),
            "all descendants should cascade: {listed:?}"
        );
    }

    #[test]
    fn local_note_repo_list_orders_by_sibling_index() {
        let (_p, n, project_id) = make_pair();
        let a = n.create(project_id, None, "a").unwrap();
        let b = n.create(project_id, None, "b").unwrap();
        let c = n.create(project_id, None, "c").unwrap();
        let listed = n.list_for_project(project_id).unwrap();
        let roots: Vec<&LocalNote> = listed.iter().filter(|x| x.parent_id.is_none()).collect();
        assert_eq!(roots.len(), 3);
        assert_eq!(roots[0].id, a.id);
        assert_eq!(roots[1].id, b.id);
        assert_eq!(roots[2].id, c.id);
        assert_eq!(roots[0].sibling_index, 0);
        assert_eq!(roots[1].sibling_index, 1);
        assert_eq!(roots[2].sibling_index, 2);
    }

    #[test]
    fn local_note_repo_create_with_invalid_parent_returns_error() {
        let (_p, n, project_id) = make_pair();
        let phantom = Uuid::new_v4();
        let err = n.create(project_id, Some(phantom), "orphan").unwrap_err();
        assert!(
            matches!(err, StoreError::InvalidArgument(_)),
            "expected InvalidArgument, got {err:?}"
        );
    }

    #[test]
    fn local_note_repo_touch_updated_bumps_timestamp() {
        let (_p, n, project_id) = make_pair();
        let note = n.create(project_id, None, "t").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        n.touch_updated(note.id).unwrap();
        let listed = n.list_for_project(project_id).unwrap();
        let got = listed.iter().find(|x| x.id == note.id).unwrap();
        assert!(got.updated_at_ms > note.updated_at_ms);
    }

    #[test]
    fn local_note_repo_delete_unknown_id_is_noop() {
        let (_p, n, project_id) = make_pair();
        let _root = n.create(project_id, None, "root").unwrap();
        n.delete(Uuid::new_v4()).unwrap();
        let listed = n.list_for_project(project_id).unwrap();
        assert_eq!(listed.len(), 1);
    }
}
