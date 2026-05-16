//! Local-mode attachments — files / images pinned to a `local_note`.
//!
//! Mirrors `attachment.rs` (cloud) but FKs to `local_note(id)` so
//! chat-mode "attach this screenshot to my Features note" works
//! inside an Operon vault. The two tables are intentionally parallel
//! — same column shape, just a different FK target — because the
//! local / cloud split shows up everywhere else in the schema too
//! (`local_note` vs `notes`, `local_note_link` vs `note_link`).
//!
//! Uses plain `uuid::Uuid` for `id` / `note_id` to match the rest of
//! the local-mode repos (`LocalNote.id: Uuid`, etc.) rather than the
//! `AttachmentId` / `NoteId` newtypes the cloud side carries. This
//! keeps bridge-tool callsites free of `NoteId(uuid)` ceremony.

use crate::sql::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::StoreError;
use crate::store::Store;
use crate::time::now_ms;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalAttachment {
    pub id: Uuid,
    pub note_id: Uuid,
    pub filename: String,
    pub mime_type: Option<String>,
    pub sha256_hex: String,
    pub size_bytes: i64,
    pub blob_path: String,
    pub created_at_ms: i64,
}

impl LocalAttachment {
    /// Helper for the common create path: stamps a fresh UUID + `now_ms`.
    /// Callers can override either by mutating the returned struct
    /// before persisting (the bridge `attach_image_to_note` tool
    /// uses this then sets `mime_type` from the request).
    pub fn new(
        note_id: Uuid,
        filename: impl Into<String>,
        sha256_hex: impl Into<String>,
        size_bytes: i64,
        blob_path: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
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

pub trait LocalAttachmentRepository: Send + Sync {
    fn create(&self, a: &LocalAttachment) -> Result<(), StoreError>;
    fn get(&self, id: Uuid) -> Result<Option<LocalAttachment>, StoreError>;
    fn delete(&self, id: Uuid) -> Result<(), StoreError>;
    fn list_by_note(&self, note_id: Uuid) -> Result<Vec<LocalAttachment>, StoreError>;
    /// Count rows whose `blob_path` exactly equals `path`. Used by
    /// the bridge's blob-GC pass after `delete_attachment` /
    /// `delete_note` cascades — when the count drops to zero across
    /// both `local_attachments` and `local_note.blob_path`, the
    /// content-addressed blob under `<vault>/.operon/images/` is
    /// safe to unlink.
    fn count_by_blob_path(&self, path: &str) -> Result<i64, StoreError>;
}

pub struct SqliteLocalAttachmentRepository {
    store: Store,
}

impl SqliteLocalAttachmentRepository {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

fn invalid(msg: String) -> crate::sql::Error {
    crate::sql::Error::FromSqlConversionFailure(
        0,
        crate::sql::types::Type::Text,
        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, msg)),
    )
}

fn row_to_attachment(row: &crate::sql::Row<'_>) -> crate::sql::Result<LocalAttachment> {
    let id_str: String = row.get(0)?;
    let note_id_str: String = row.get(1)?;
    Ok(LocalAttachment {
        id: Uuid::parse_str(&id_str).map_err(|e| invalid(format!("id: {e}")))?,
        note_id: Uuid::parse_str(&note_id_str)
            .map_err(|e| invalid(format!("note_id: {e}")))?,
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

impl LocalAttachmentRepository for SqliteLocalAttachmentRepository {
    fn create(&self, a: &LocalAttachment) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute(
            "INSERT INTO local_attachments
                (id, note_id, filename, mime_type, sha256_hex, size_bytes, blob_path, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                a.id.to_string(),
                a.note_id.to_string(),
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

    fn get(&self, id: Uuid) -> Result<Option<LocalAttachment>, StoreError> {
        let conn = self.store.conn()?;
        let sql = format!("SELECT {SELECT_COLS} FROM local_attachments WHERE id = ?1");
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt
            .query_row(params![id.to_string()], row_to_attachment)
            .optional()?)
    }

    fn delete(&self, id: Uuid) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute(
            "DELETE FROM local_attachments WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(())
    }

    fn list_by_note(&self, note_id: Uuid) -> Result<Vec<LocalAttachment>, StoreError> {
        let conn = self.store.conn()?;
        let sql = format!(
            "SELECT {SELECT_COLS} FROM local_attachments \
             WHERE note_id = ?1 ORDER BY created_at_ms"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![note_id.to_string()], row_to_attachment)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    fn count_by_blob_path(&self, path: &str) -> Result<i64, StoreError> {
        let conn = self.store.conn()?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM local_attachments WHERE blob_path = ?1",
            params![path],
            |row| row.get(0),
        )?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repos::{
        LocalNoteRepository, LocalProjectRepository, SqliteLocalNoteRepository,
        SqliteLocalProjectRepository,
    };
    use crate::test_support::open_in_memory;

    fn fixture() -> (
        SqliteLocalAttachmentRepository,
        SqliteLocalNoteRepository,
        SqliteLocalProjectRepository,
    ) {
        let store = open_in_memory().unwrap();
        let attachments = SqliteLocalAttachmentRepository::new(store.clone());
        let notes = SqliteLocalNoteRepository::new(store.clone());
        let projects = SqliteLocalProjectRepository::new(store);
        (attachments, notes, projects)
    }

    fn seed_note(
        notes: &SqliteLocalNoteRepository,
        projects: &SqliteLocalProjectRepository,
    ) -> Uuid {
        let p = projects.create("P").unwrap();
        let n = notes.create(p.id, None, "A").unwrap();
        n.id
    }

    #[test]
    fn create_then_get_round_trip() {
        let (attachments, notes, projects) = fixture();
        let note_id = seed_note(&notes, &projects);
        let mut a = LocalAttachment::new(note_id, "x.png", "sha", 10, ".operon/images/sha.png");
        a.mime_type = Some("image/png".into());
        attachments.create(&a).unwrap();

        let got = attachments.get(a.id).unwrap().expect("row");
        assert_eq!(got.id, a.id);
        assert_eq!(got.note_id, note_id);
        assert_eq!(got.filename, "x.png");
        assert_eq!(got.mime_type.as_deref(), Some("image/png"));
        assert_eq!(got.sha256_hex, "sha");
        assert_eq!(got.size_bytes, 10);
        assert_eq!(got.blob_path, ".operon/images/sha.png");
    }

    #[test]
    fn list_by_note_returns_all_pinned_attachments() {
        let (attachments, notes, projects) = fixture();
        let note_id = seed_note(&notes, &projects);
        let a = LocalAttachment::new(note_id, "a.png", "sha-a", 1, "p/a.png");
        let b = LocalAttachment::new(note_id, "b.png", "sha-b", 2, "p/b.png");
        attachments.create(&a).unwrap();
        attachments.create(&b).unwrap();

        let listed = attachments.list_by_note(note_id).unwrap();
        let mut names: Vec<String> = listed.into_iter().map(|x| x.filename).collect();
        names.sort();
        assert_eq!(names, vec!["a.png".to_string(), "b.png".to_string()]);
    }

    #[test]
    fn delete_removes_row() {
        let (attachments, notes, projects) = fixture();
        let note_id = seed_note(&notes, &projects);
        let a = LocalAttachment::new(note_id, "z.png", "sha-z", 1, "p/z.png");
        attachments.create(&a).unwrap();
        attachments.delete(a.id).unwrap();
        assert!(attachments.get(a.id).unwrap().is_none());
    }

    #[test]
    fn cascade_on_local_note_delete() {
        // Foreign-key constraint: deleting the host note removes
        // every attachment row pinned to it. Without this the
        // bridge's `delete_note` confirm flow would leak attachment
        // rows whose blob_path references a now-orphaned blob.
        let (attachments, notes, projects) = fixture();
        let note_id = seed_note(&notes, &projects);
        let a = LocalAttachment::new(note_id, "cascade.png", "sha-c", 1, "p/c.png");
        attachments.create(&a).unwrap();

        notes.delete(note_id).unwrap();
        assert!(attachments.list_by_note(note_id).unwrap().is_empty());
        assert!(attachments.get(a.id).unwrap().is_none());
    }

    #[test]
    fn unique_constraint_blocks_duplicate_sha_per_note() {
        // `UNIQUE (note_id, sha256_hex)` means a second insert of
        // the same blob to the same note errors out — the GUI / tool
        // surface treats that as "already attached" and skips.
        let (attachments, notes, projects) = fixture();
        let note_id = seed_note(&notes, &projects);
        let a = LocalAttachment::new(note_id, "first.png", "sha-dup", 1, "p/dup.png");
        attachments.create(&a).unwrap();
        let mut b = LocalAttachment::new(note_id, "second.png", "sha-dup", 1, "p/dup.png");
        // Distinct row id, same (note_id, sha) pair → constraint trips.
        b.id = Uuid::new_v4();
        let err = attachments.create(&b).unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("constraint"),
            "expected constraint error, got {err}"
        );
    }
}
