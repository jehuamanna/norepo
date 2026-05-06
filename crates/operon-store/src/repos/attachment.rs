use crate::sql::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::StoreError;
use crate::ids::{AttachmentId, NoteId};
use crate::store::Store;
use crate::time::now_ms;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Attachment {
    pub id: AttachmentId,
    pub note_id: NoteId,
    pub filename: String,
    pub mime_type: Option<String>,
    pub sha256_hex: String,
    pub size_bytes: i64,
    pub blob_path: String,
    pub created_at_ms: i64,
}

impl Attachment {
    pub fn new(
        note_id: NoteId,
        filename: impl Into<String>,
        sha256_hex: impl Into<String>,
        size_bytes: i64,
        blob_path: impl Into<String>,
    ) -> Self {
        Self {
            id: AttachmentId::new(),
            note_id,
            filename: filename.into(),
            mime_type: None,
            sha256_hex: sha256_hex.into(),
            size_bytes,
            blob_path: blob_path.into(),
            created_at_ms: now_ms(),
        }
    }
}

pub trait AttachmentRepository: Send + Sync {
    fn create(&self, a: &Attachment) -> Result<(), StoreError>;
    fn get(&self, id: &AttachmentId) -> Result<Option<Attachment>, StoreError>;
    fn delete(&self, id: &AttachmentId) -> Result<(), StoreError>;
    fn list_by_note(&self, note: &NoteId) -> Result<Vec<Attachment>, StoreError>;
}

pub struct SqliteAttachmentRepository {
    store: Store,
}

impl SqliteAttachmentRepository {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

fn invalid(e: StoreError) -> crate::sql::Error {
    crate::sql::Error::FromSqlConversionFailure(
        0,
        crate::sql::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            e.to_string(),
        )),
    )
}

fn row_to_attachment(row: &crate::sql::Row<'_>) -> crate::sql::Result<Attachment> {
    Ok(Attachment {
        id: AttachmentId::from_str_strict(&row.get::<_, String>(0)?).map_err(invalid)?,
        note_id: NoteId::from_str_strict(&row.get::<_, String>(1)?).map_err(invalid)?,
        filename: row.get(2)?,
        mime_type: row.get(3)?,
        sha256_hex: row.get(4)?,
        size_bytes: row.get(5)?,
        blob_path: row.get(6)?,
        created_at_ms: row.get(7)?,
    })
}

const SELECT_COLS: &str =
    "id, note_id, filename, mime_type, sha256_hex, size_bytes, blob_path, created_at_ms";

impl AttachmentRepository for SqliteAttachmentRepository {
    fn create(&self, a: &Attachment) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute(
            "INSERT INTO attachments (id, note_id, filename, mime_type, sha256_hex, size_bytes, blob_path, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                a.id.as_str(),
                a.note_id.as_str(),
                a.filename,
                a.mime_type,
                a.sha256_hex,
                a.size_bytes,
                a.blob_path,
                a.created_at_ms,
            ],
        )?;
        Ok(())
    }

    fn get(&self, id: &AttachmentId) -> Result<Option<Attachment>, StoreError> {
        let conn = self.store.conn()?;
        let sql = format!("SELECT {SELECT_COLS} FROM attachments WHERE id = ?1");
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt
            .query_row(params![id.as_str()], row_to_attachment)
            .optional()?)
    }

    fn delete(&self, id: &AttachmentId) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute(
            "DELETE FROM attachments WHERE id = ?1",
            params![id.as_str()],
        )?;
        Ok(())
    }

    fn list_by_note(&self, note: &NoteId) -> Result<Vec<Attachment>, StoreError> {
        let conn = self.store.conn()?;
        let sql = format!("SELECT {SELECT_COLS} FROM attachments WHERE note_id = ?1");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![note.as_str()], row_to_attachment)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}
