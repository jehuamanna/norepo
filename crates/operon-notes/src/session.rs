use std::sync::Arc;

use loro::{LoroDoc, ExportMode};
use operon_store::repos::note::NoteRepository;
use operon_store::repos::note_update::NoteUpdateRepository;
use operon_store::time::now_ms;
use operon_store::NoteId;

use crate::error::NotesError;

const COMPACT_UPDATE_THRESHOLD: u32 = 200;
const COMPACT_TIME_MS: i64 = 60_000;

pub struct LoroSession {
    pub note_id: NoteId,
    pub doc: LoroDoc,
    pub last_snapshot_at_ms: i64,
    pub updates_since_snapshot: u32,
}

impl LoroSession {
    /// Load a session from disk: import the persisted snapshot, then replay
    /// any `note_updates` rows that arrived after the snapshot's checkpoint.
    pub fn open(
        note_id: NoteId,
        notes: &dyn NoteRepository,
        updates: &dyn NoteUpdateRepository,
    ) -> Result<Self, NotesError> {
        let doc = LoroDoc::new();
        let mut last_snapshot_at_ms = 0;
        if let Some(note) = notes.get(&note_id)? {
            if let Some(snapshot) = note.loro_snapshot.as_ref() {
                doc.import(snapshot)
                    .map_err(|e| NotesError::Loro(e.to_string()))?;
                last_snapshot_at_ms = note.updated_at_ms;
            }
        }
        let pending = updates.since(&note_id, last_snapshot_at_ms)?;
        let updates_since_snapshot = pending.len() as u32;
        for blob in pending {
            doc.import(&blob)
                .map_err(|e| NotesError::Loro(e.to_string()))?;
        }
        Ok(Self {
            note_id,
            doc,
            last_snapshot_at_ms,
            updates_since_snapshot,
        })
    }

    /// Apply an incoming Loro update blob, persist it to `note_updates`, and
    /// return the same blob (callers broadcast it to other connected peers).
    pub fn apply_update(
        &mut self,
        blob: &[u8],
        updates: &dyn NoteUpdateRepository,
    ) -> Result<Vec<u8>, NotesError> {
        self.doc
            .import(blob)
            .map_err(|e| NotesError::Loro(e.to_string()))?;
        updates.append(&self.note_id, blob)?;
        self.updates_since_snapshot = self.updates_since_snapshot.saturating_add(1);
        Ok(blob.to_vec())
    }

    /// Export the doc as a full snapshot blob.
    pub fn export_snapshot(&self) -> Result<Vec<u8>, NotesError> {
        self.doc
            .export(ExportMode::Snapshot)
            .map_err(|e| NotesError::Loro(e.to_string()))
    }

    /// Compact the on-disk state: write a fresh snapshot to `notes` and trim
    /// `note_updates` rows older than the new checkpoint. No-op unless the
    /// 200-update or 60-second thresholds are met.
    pub fn compact(
        &mut self,
        notes: &dyn NoteRepository,
        updates: &dyn NoteUpdateRepository,
    ) -> Result<bool, NotesError> {
        let now = now_ms();
        let trigger_count = self.updates_since_snapshot >= COMPACT_UPDATE_THRESHOLD;
        let trigger_time =
            self.updates_since_snapshot > 0 && now.saturating_sub(self.last_snapshot_at_ms) >= COMPACT_TIME_MS;
        if !trigger_count && !trigger_time {
            return Ok(false);
        }
        let snapshot = self.export_snapshot()?;
        if let Some(mut note) = notes.get(&self.note_id)? {
            note.loro_snapshot = Some(snapshot);
            note.updated_at_ms = now;
            notes.update(&note)?;
            updates.delete_before(&self.note_id, now)?;
            self.last_snapshot_at_ms = now;
            self.updates_since_snapshot = 0;
        }
        Ok(true)
    }

    /// Best-effort projection of the doc to a plain markdown string. Looks for
    /// a Text container at well-known path "body"; falls back to an empty
    /// string. Used by Phase 5 import to project Loro -> plain markdown.
    pub fn project_to_markdown(&self) -> String {
        crate::projection::doc_to_markdown(&self.doc)
    }
}

/// Convenience wrapper for shared mutable access in a hub.
pub type SharedSession = Arc<tokio::sync::RwLock<LoroSession>>;
