//! Wire format for WebSocket frames between client and server.
//!
//! Each frame is `[type:u8][payload...]`. Types:
//!   0x01 snapshot — full LoroDoc snapshot (server → client)
//!   0x02 update   — incremental update blob (bidirectional)
//!   0x03 awareness — cursor / selection / display name (bidirectional)
//!   0x04 presence — `{joined: [...], left: [...]}` JSON (server → client)

use serde::{Deserialize, Serialize};

use crate::error::NotesError;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    Snapshot = 0x01,
    Update = 0x02,
    Awareness = 0x03,
    Presence = 0x04,
}

impl FrameKind {
    pub fn try_from(b: u8) -> Result<Self, NotesError> {
        match b {
            0x01 => Ok(FrameKind::Snapshot),
            0x02 => Ok(FrameKind::Update),
            0x03 => Ok(FrameKind::Awareness),
            0x04 => Ok(FrameKind::Presence),
            other => Err(NotesError::Frame(format!("unknown frame type {other}"))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HubFrame {
    pub kind: FrameKind,
    pub client_id: String,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresencePayload {
    pub joined: Vec<String>,
    pub left: Vec<String>,
}

pub fn encode(frame: &HubFrame) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + frame.payload.len());
    out.push(frame.kind as u8);
    out.extend_from_slice(&frame.payload);
    out
}

pub fn decode(bytes: &[u8]) -> Result<(FrameKind, Vec<u8>), NotesError> {
    if bytes.is_empty() {
        return Err(NotesError::Frame("empty".into()));
    }
    let kind = FrameKind::try_from(bytes[0])?;
    Ok((kind, bytes[1..].to_vec()))
}
