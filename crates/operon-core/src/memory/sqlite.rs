//! SqliteMemoryStore — feature-gated `sqlite-memory`. Desktop-only.
//!
//! Uses synchronous rusqlite wrapped in `tokio::task::spawn_blocking` for async API.

use crate::error::{OperonError, OperonResult};
use crate::traits::{
    Capabilities, ContentBlock, Hit, MemoryPlugin, Message, Plugin, Role, Scope,
};
use async_trait::async_trait;
use rusqlite::{params, Connection};
use rusqlite_migration::{Migrations, M};
use std::path::Path;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

pub struct SqliteMemoryStore {
    conn: Arc<Mutex<Connection>>,
}

const MIGRATION_001: &str = include_str!("../../migrations/sqlite/001_messages.sql");

impl SqliteMemoryStore {
    pub fn open(path: impl AsRef<Path>) -> OperonResult<Self> {
        let mut conn = Connection::open(path)
            .map_err(|e| OperonError::Config(format!("sqlite open: {e}")))?;
        Self::migrate(&mut conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn open_in_memory() -> OperonResult<Self> {
        let mut conn = Connection::open_in_memory()
            .map_err(|e| OperonError::Config(format!("sqlite open: {e}")))?;
        Self::migrate(&mut conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn migrate(conn: &mut Connection) -> OperonResult<()> {
        let migrations = Migrations::new(vec![M::up(MIGRATION_001)]);
        migrations
            .to_latest(conn)
            .map_err(|e| OperonError::Config(format!("sqlite migrate: {e}")))
    }

    fn scope_to_kind_and_id(scope: &Scope) -> (i64, Option<Vec<u8>>) {
        match scope {
            Scope::User => (0, None),
            Scope::Project(id) => (1, Some(id.as_bytes().to_vec())),
            Scope::Team(id) => (2, Some(id.as_bytes().to_vec())),
        }
    }
}

#[async_trait]
impl Plugin for SqliteMemoryStore {
    fn name(&self) -> &str {
        "sqlite_memory_store"
    }
    fn version(&self) -> &str {
        "0.1.0"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::MULTI_TENANT
    }
}

#[async_trait]
impl MemoryPlugin for SqliteMemoryStore {
    async fn write(&self, scope: Scope, msg: Message) -> OperonResult<Uuid> {
        let conn = self.conn.clone();
        let id = msg.id;
        let (kind, scope_id) = Self::scope_to_kind_and_id(&scope);
        let content_json = serde_json::to_string(&msg.content)
            .map_err(|e| OperonError::Config(format!("serialize content: {e}")))?;
        let metadata_json = serde_json::to_string(&msg.metadata)
            .map_err(|e| OperonError::Config(format!("serialize metadata: {e}")))?;
        let role = format!("{:?}", msg.role);
        let session = msg.session.as_bytes().to_vec();
        let id_bytes = id.as_bytes().to_vec();
        let created_at = msg.created_at_ms as i64;

        tokio::task::spawn_blocking(move || -> OperonResult<()> {
            let g = conn.lock().map_err(|_| {
                OperonError::Config("sqlite lock poisoned".into())
            })?;
            g.execute(
                "INSERT OR REPLACE INTO messages (id, scope_kind, scope_id, session, role, content_json, metadata_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![id_bytes, kind, scope_id, session, role, content_json, metadata_json, created_at],
            )
            .map_err(|e| OperonError::Config(format!("sqlite insert: {e}")))?;
            Ok(())
        })
        .await
        .map_err(|e| OperonError::Config(format!("spawn_blocking: {e}")))??;

        Ok(id)
    }

    async fn read(&self, scope: Scope, id: Uuid) -> OperonResult<Option<Message>> {
        let conn = self.conn.clone();
        let (kind, scope_id) = Self::scope_to_kind_and_id(&scope);
        let id_bytes = id.as_bytes().to_vec();

        tokio::task::spawn_blocking(move || -> OperonResult<Option<Message>> {
            let g = conn.lock().map_err(|_| {
                OperonError::Config("sqlite lock poisoned".into())
            })?;
            let mut stmt = g
                .prepare(
                    "SELECT id, role, content_json, metadata_json, created_at, session FROM messages WHERE id = ?1 AND scope_kind = ?2 AND ((scope_id IS NULL AND ?3 IS NULL) OR scope_id = ?3)",
                )
                .map_err(|e| OperonError::Config(format!("sqlite prepare: {e}")))?;
            let mut rows = stmt
                .query(params![id_bytes, kind, scope_id])
                .map_err(|e| OperonError::Config(format!("sqlite query: {e}")))?;
            if let Some(row) = rows
                .next()
                .map_err(|e| OperonError::Config(format!("sqlite row: {e}")))?
            {
                let id_b: Vec<u8> = row.get(0).unwrap();
                let role_s: String = row.get(1).unwrap();
                let content_s: String = row.get(2).unwrap();
                let meta_s: String = row.get(3).unwrap();
                let created: i64 = row.get(4).unwrap();
                let session_b: Vec<u8> = row.get(5).unwrap();
                let role = match role_s.as_str() {
                    "User" => Role::User,
                    "Assistant" => Role::Assistant,
                    "System" => Role::System,
                    "Tool" => Role::Tool,
                    other => return Err(OperonError::Config(format!("unknown role: {other}"))),
                };
                let content: Vec<ContentBlock> = serde_json::from_str(&content_s)
                    .map_err(|e| OperonError::Config(format!("deserialize content: {e}")))?;
                let metadata: std::collections::HashMap<String, String> =
                    serde_json::from_str(&meta_s).unwrap_or_default();
                Ok(Some(Message {
                    id: Uuid::from_slice(&id_b).unwrap(),
                    role,
                    content,
                    created_at_ms: created as u64,
                    session: Uuid::from_slice(&session_b).unwrap(),
                    metadata,
                }))
            } else {
                Ok(None)
            }
        })
        .await
        .map_err(|e| OperonError::Config(format!("spawn_blocking: {e}")))?
    }

    async fn search(&self, scope: Scope, query: &str, k: usize) -> OperonResult<Vec<Hit>> {
        let conn = self.conn.clone();
        let (kind, scope_id) = Self::scope_to_kind_and_id(&scope);
        let needle = query.to_string();

        tokio::task::spawn_blocking(move || -> OperonResult<Vec<Hit>> {
            let g = conn.lock().map_err(|_| {
                OperonError::Config("sqlite lock poisoned".into())
            })?;
            let mut stmt = g
                .prepare(
                    "SELECT id, role, content_json, metadata_json, created_at, session FROM messages WHERE scope_kind = ?1 AND ((scope_id IS NULL AND ?2 IS NULL) OR scope_id = ?2) AND content_json LIKE ?3 ORDER BY created_at DESC LIMIT ?4",
                )
                .map_err(|e| OperonError::Config(format!("sqlite prepare: {e}")))?;
            let like = format!("%{}%", needle);
            let mut rows = stmt
                .query(params![kind, scope_id, like, k as i64])
                .map_err(|e| OperonError::Config(format!("sqlite query: {e}")))?;
            let mut hits = Vec::new();
            while let Some(row) = rows
                .next()
                .map_err(|e| OperonError::Config(format!("sqlite row: {e}")))?
            {
                let id_b: Vec<u8> = row.get(0).unwrap();
                let role_s: String = row.get(1).unwrap();
                let content_s: String = row.get(2).unwrap();
                let meta_s: String = row.get(3).unwrap();
                let created: i64 = row.get(4).unwrap();
                let session_b: Vec<u8> = row.get(5).unwrap();
                let role = match role_s.as_str() {
                    "User" => Role::User,
                    "Assistant" => Role::Assistant,
                    "System" => Role::System,
                    "Tool" => Role::Tool,
                    other => return Err(OperonError::Config(format!("unknown role: {other}"))),
                };
                let content: Vec<ContentBlock> = serde_json::from_str(&content_s)
                    .map_err(|e| OperonError::Config(format!("deserialize content: {e}")))?;
                let metadata: std::collections::HashMap<String, String> =
                    serde_json::from_str(&meta_s).unwrap_or_default();
                hits.push(Hit {
                    message: Message {
                        id: Uuid::from_slice(&id_b).unwrap(),
                        role,
                        content,
                        created_at_ms: created as u64,
                        session: Uuid::from_slice(&session_b).unwrap(),
                        metadata,
                    },
                    score: 1.0,
                });
            }
            Ok(hits)
        })
        .await
        .map_err(|e| OperonError::Config(format!("spawn_blocking: {e}")))?
    }

    async fn delete(&self, scope: Scope, id: Uuid) -> OperonResult<()> {
        let conn = self.conn.clone();
        let (kind, scope_id) = Self::scope_to_kind_and_id(&scope);
        let id_bytes = id.as_bytes().to_vec();

        tokio::task::spawn_blocking(move || -> OperonResult<()> {
            let g = conn.lock().map_err(|_| {
                OperonError::Config("sqlite lock poisoned".into())
            })?;
            g.execute(
                "DELETE FROM messages WHERE id = ?1 AND scope_kind = ?2 AND ((scope_id IS NULL AND ?3 IS NULL) OR scope_id = ?3)",
                params![id_bytes, kind, scope_id],
            )
            .map_err(|e| OperonError::Config(format!("sqlite delete: {e}")))?;
            Ok(())
        })
        .await
        .map_err(|e| OperonError::Config(format!("spawn_blocking: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::run_conformance;

    #[tokio::test]
    async fn conformance_sqlite_in_memory() {
        let s = SqliteMemoryStore::open_in_memory().expect("open");
        run_conformance(&s).await;
    }

    #[tokio::test]
    async fn migration_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let _s1 = SqliteMemoryStore::open(&path).expect("first open");
        let _s2 = SqliteMemoryStore::open(&path).expect("second open");
    }
}
