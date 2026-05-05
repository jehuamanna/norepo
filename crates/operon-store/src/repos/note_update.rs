use rusqlite::params;
use uuid::Uuid;

use crate::error::StoreError;
use crate::ids::NoteId;
use crate::sqlite::Store;
use crate::time::now_ms;

pub trait NoteUpdateRepository: Send + Sync {
    fn append(&self, note_id: &NoteId, update_blob: &[u8]) -> Result<(), StoreError>;
    fn since(&self, note_id: &NoteId, after_ms: i64) -> Result<Vec<Vec<u8>>, StoreError>;
    fn count_for(&self, note_id: &NoteId) -> Result<u32, StoreError>;
    fn delete_before(&self, note_id: &NoteId, before_ms: i64) -> Result<(), StoreError>;
}

pub struct SqliteNoteUpdateRepository {
    store: Store,
}

impl SqliteNoteUpdateRepository {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

impl NoteUpdateRepository for SqliteNoteUpdateRepository {
    fn append(&self, note_id: &NoteId, update_blob: &[u8]) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute(
            "INSERT INTO note_updates (id, note_id, update_blob, applied_at_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                Uuid::new_v4().to_string(),
                note_id.as_str(),
                update_blob,
                now_ms(),
            ],
        )?;
        Ok(())
    }

    fn since(&self, note_id: &NoteId, after_ms: i64) -> Result<Vec<Vec<u8>>, StoreError> {
        let conn = self.store.conn()?;
        let mut stmt = conn.prepare(
            "SELECT update_blob FROM note_updates WHERE note_id = ?1 AND applied_at_ms > ?2 ORDER BY applied_at_ms",
        )?;
        let rows = stmt.query_map(params![note_id.as_str(), after_ms], |r| r.get::<_, Vec<u8>>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    fn count_for(&self, note_id: &NoteId) -> Result<u32, StoreError> {
        let conn = self.store.conn()?;
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM note_updates WHERE note_id = ?1",
            params![note_id.as_str()],
            |r| r.get(0),
        )?;
        Ok(n as u32)
    }

    fn delete_before(&self, note_id: &NoteId, before_ms: i64) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute(
            "DELETE FROM note_updates WHERE note_id = ?1 AND applied_at_ms <= ?2",
            params![note_id.as_str(), before_ms],
        )?;
        Ok(())
    }
}
