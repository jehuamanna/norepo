use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use operon_store::repos::note::NoteRepository;
use operon_store::repos::note_update::NoteUpdateRepository;
use operon_store::NoteId;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};

use crate::error::NotesError;
use crate::frame::{HubFrame, PresencePayload};
use crate::session::{LoroSession, SharedSession};

const CHANNEL_CAPACITY: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PresenceDelta {
    Joined(String),
    Left(String),
}

#[derive(Clone)]
struct PerNote {
    session: SharedSession,
    sender: broadcast::Sender<HubFrame>,
}

pub struct NoteHub {
    notes: DashMap<String, PerNote>,
    notes_repo: Arc<dyn NoteRepository + Send + Sync>,
    updates_repo: Arc<dyn NoteUpdateRepository + Send + Sync>,
    pub idle_timeout: Duration,
}

impl NoteHub {
    pub fn new(
        notes_repo: Arc<dyn NoteRepository + Send + Sync>,
        updates_repo: Arc<dyn NoteUpdateRepository + Send + Sync>,
    ) -> Self {
        Self {
            notes: DashMap::new(),
            notes_repo,
            updates_repo,
            idle_timeout: Duration::from_secs(5 * 60),
        }
    }

    /// Open or reuse a session for this note. Returns the shared session and
    /// a fresh broadcast receiver. Idempotent — concurrent calls converge.
    pub fn open(
        &self,
        note_id: &NoteId,
    ) -> Result<(SharedSession, broadcast::Sender<HubFrame>), NotesError> {
        let key = note_id.to_string();
        if let Some(entry) = self.notes.get(&key) {
            return Ok((entry.session.clone(), entry.sender.clone()));
        }
        let session = LoroSession::open(
            note_id.clone(),
            self.notes_repo.as_ref(),
            self.updates_repo.as_ref(),
        )?;
        let (tx, _rx) = broadcast::channel(CHANNEL_CAPACITY);
        let entry = PerNote {
            session: Arc::new(RwLock::new(session)),
            sender: tx.clone(),
        };
        self.notes.insert(key, entry.clone());
        Ok((entry.session, entry.sender))
    }

    /// Apply an incoming update on behalf of `client_id`, persist, and
    /// broadcast the resulting frame to subscribers.
    pub async fn apply_and_broadcast(
        &self,
        note_id: &NoteId,
        client_id: &str,
        blob: Vec<u8>,
    ) -> Result<(), NotesError> {
        let (session_arc, sender) = self.open(note_id)?;
        let echoed = {
            let mut session = session_arc.write().await;
            let echoed = session.apply_update(&blob, self.updates_repo.as_ref())?;
            // Periodic compaction.
            let _ = session.compact(self.notes_repo.as_ref(), self.updates_repo.as_ref());
            echoed
        };
        let _ = sender.send(HubFrame {
            kind: crate::frame::FrameKind::Update,
            client_id: client_id.to_string(),
            payload: echoed,
        });
        Ok(())
    }

    pub async fn export_snapshot(&self, note_id: &NoteId) -> Result<Vec<u8>, NotesError> {
        let (session_arc, _sender) = self.open(note_id)?;
        let session = session_arc.read().await;
        session.export_snapshot()
    }

    pub fn broadcast_presence(&self, note_id: &NoteId, delta: PresenceDelta) {
        if let Some(entry) = self.notes.get(&note_id.to_string()) {
            let payload = match delta {
                PresenceDelta::Joined(c) => PresencePayload {
                    joined: vec![c.clone()],
                    left: vec![],
                },
                PresenceDelta::Left(c) => PresencePayload {
                    joined: vec![],
                    left: vec![c.clone()],
                },
            };
            let body = serde_json::to_vec(&payload).unwrap_or_default();
            let _ = entry.sender.send(HubFrame {
                kind: crate::frame::FrameKind::Presence,
                client_id: String::new(),
                payload: body,
            });
        }
    }

    pub fn broadcast_awareness(&self, note_id: &NoteId, client_id: &str, payload: Vec<u8>) {
        if let Some(entry) = self.notes.get(&note_id.to_string()) {
            let _ = entry.sender.send(HubFrame {
                kind: crate::frame::FrameKind::Awareness,
                client_id: client_id.to_string(),
                payload,
            });
        }
    }

    /// Drop the per-note session and broadcast channel. Next `open` reloads
    /// from disk. Tests use this to flush state.
    pub fn evict(&self, note_id: &NoteId) {
        self.notes.remove(&note_id.to_string());
    }

    /// Apply an externally-prepared snapshot (e.g. from importer) into the
    /// live doc and broadcast a synthesised update so connected clients see
    /// it immediately.
    pub async fn import_into(
        &self,
        note_id: &NoteId,
        snapshot: &[u8],
    ) -> Result<(), NotesError> {
        let (session_arc, sender) = self.open(note_id)?;
        let echoed = {
            let mut session = session_arc.write().await;
            // Loro's import handles snapshots and updates uniformly.
            session
                .doc
                .import(snapshot)
                .map_err(|e| NotesError::Loro(e.to_string()))?;
            self.updates_repo.append(note_id, snapshot)?;
            session.updates_since_snapshot += 1;
            snapshot.to_vec()
        };
        let _ = sender.send(HubFrame {
            kind: crate::frame::FrameKind::Update,
            client_id: "importer".to_string(),
            payload: echoed,
        });
        Ok(())
    }
}
