use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::StoreError;
use crate::ids::{NoteId, ProjectId};
use crate::sqlite::Store;
use crate::time::now_ms;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Note {
    pub id: NoteId,
    pub project_id: ProjectId,
    pub parent_id: Option<NoteId>,
    pub title: String,
    pub body_markdown: Option<String>,
    pub loro_snapshot: Option<Vec<u8>>,
    pub sibling_index: i64,
    pub kind: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl Note {
    pub fn new_root(project_id: ProjectId, title: impl Into<String>) -> Self {
        let now = now_ms();
        Self {
            id: NoteId::new(),
            project_id,
            parent_id: None,
            title: title.into(),
            body_markdown: None,
            loro_snapshot: None,
            sibling_index: 0,
            kind: "markdown".into(),
            created_at_ms: now,
            updated_at_ms: now,
        }
    }
}

pub trait NoteRepository: Send + Sync {
    fn create(&self, n: &Note) -> Result<(), StoreError>;
    fn get(&self, id: &NoteId) -> Result<Option<Note>, StoreError>;
    fn update(&self, n: &Note) -> Result<(), StoreError>;
    fn delete(&self, id: &NoteId) -> Result<(), StoreError>;
    fn list_by_project(&self, project_id: &ProjectId) -> Result<Vec<Note>, StoreError>;
    fn children_of(&self, parent: &NoteId) -> Result<Vec<Note>, StoreError>;
    fn top_level(&self, project_id: &ProjectId) -> Result<Vec<Note>, StoreError>;
}

pub struct SqliteNoteRepository {
    store: Store,
}

impl SqliteNoteRepository {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

fn invalid(e: StoreError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            e.to_string(),
        )),
    )
}

fn row_to_note(row: &rusqlite::Row<'_>) -> rusqlite::Result<Note> {
    let id = NoteId::from_str_strict(&row.get::<_, String>(0)?).map_err(invalid)?;
    let project_id = ProjectId::from_str_strict(&row.get::<_, String>(1)?).map_err(invalid)?;
    let parent_id_opt: Option<String> = row.get(2)?;
    let parent_id = match parent_id_opt {
        Some(s) => Some(NoteId::from_str_strict(&s).map_err(invalid)?),
        None => None,
    };
    Ok(Note {
        id,
        project_id,
        parent_id,
        title: row.get(3)?,
        body_markdown: row.get(4)?,
        loro_snapshot: row.get(5)?,
        sibling_index: row.get(6)?,
        kind: row.get(7)?,
        created_at_ms: row.get(8)?,
        updated_at_ms: row.get(9)?,
    })
}

const SELECT_COLS: &str = "id, project_id, parent_id, title, body_markdown, loro_snapshot, sibling_index, type, created_at_ms, updated_at_ms";

impl NoteRepository for SqliteNoteRepository {
    fn create(&self, n: &Note) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute(
            "INSERT INTO notes (id, project_id, parent_id, title, body_markdown, loro_snapshot,
                                sibling_index, type, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                n.id.as_str(),
                n.project_id.as_str(),
                n.parent_id.as_ref().map(|p| p.as_str()),
                n.title,
                n.body_markdown,
                n.loro_snapshot,
                n.sibling_index,
                n.kind,
                n.created_at_ms,
                n.updated_at_ms,
            ],
        )?;
        Ok(())
    }

    fn get(&self, id: &NoteId) -> Result<Option<Note>, StoreError> {
        let conn = self.store.conn()?;
        let sql = format!("SELECT {SELECT_COLS} FROM notes WHERE id = ?1");
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt.query_row(params![id.as_str()], row_to_note).optional()?)
    }

    fn update(&self, n: &Note) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        let updated = conn.execute(
            "UPDATE notes SET project_id = ?2, parent_id = ?3, title = ?4,
                              body_markdown = ?5, loro_snapshot = ?6,
                              sibling_index = ?7, type = ?8, updated_at_ms = ?9
             WHERE id = ?1",
            params![
                n.id.as_str(),
                n.project_id.as_str(),
                n.parent_id.as_ref().map(|p| p.as_str()),
                n.title,
                n.body_markdown,
                n.loro_snapshot,
                n.sibling_index,
                n.kind,
                n.updated_at_ms,
            ],
        )?;
        if updated == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    fn delete(&self, id: &NoteId) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute("DELETE FROM notes WHERE id = ?1", params![id.as_str()])?;
        Ok(())
    }

    fn list_by_project(&self, project_id: &ProjectId) -> Result<Vec<Note>, StoreError> {
        let conn = self.store.conn()?;
        let sql = format!(
            "SELECT {SELECT_COLS} FROM notes WHERE project_id = ?1 ORDER BY parent_id, sibling_index"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![project_id.as_str()], row_to_note)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    fn children_of(&self, parent: &NoteId) -> Result<Vec<Note>, StoreError> {
        let conn = self.store.conn()?;
        let sql =
            format!("SELECT {SELECT_COLS} FROM notes WHERE parent_id = ?1 ORDER BY sibling_index");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![parent.as_str()], row_to_note)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    fn top_level(&self, project_id: &ProjectId) -> Result<Vec<Note>, StoreError> {
        let conn = self.store.conn()?;
        let sql = format!(
            "SELECT {SELECT_COLS} FROM notes WHERE project_id = ?1 AND parent_id IS NULL ORDER BY sibling_index"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![project_id.as_str()], row_to_note)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}
