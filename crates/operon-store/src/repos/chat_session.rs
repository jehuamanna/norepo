//! Chat sessions persisted for the companion pane.
//!
//! Each row represents one open or historical chat. The companion's left
//! rail filters by scope (`Project(uuid)` or `Vault`) and renders rows by
//! `last_used_ms DESC`. Transcript bodies live in the operon-core
//! `messages` table keyed by the same session UUID — this table is
//! metadata only.

use crate::sql::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::StoreError;
use crate::store::Store;
use crate::time::now_ms;

/// Rolls the (`scope_kind`, `scope_id`) pair into a typed enum. The DB
/// CHECK constraint guarantees exactly one of the variants on every row.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(tag = "kind", content = "id", rename_all = "lowercase")]
pub enum ChatScope {
    /// Bound to a specific project; cwd resolves to that project's
    /// `repo_path`.
    Project(Uuid),
    /// Vault-wide; cwd resolves to the active `VaultRoot.path`.
    Vault,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatSession {
    pub id: Uuid,
    pub scope: ChatScope,
    pub label: String,
    /// Populated after the first turn finishes; used for `claude --resume`.
    pub claude_session_id: Option<String>,
    pub last_used_ms: i64,
    pub created_ms: i64,
}

pub trait ChatSessionRepository: Send + Sync {
    /// Most-recently-used first.
    fn list_in_scope(&self, scope: ChatScope) -> Result<Vec<ChatSession>, StoreError>;
    fn get(&self, id: Uuid) -> Result<Option<ChatSession>, StoreError>;
    fn create(&self, scope: ChatScope, label: &str) -> Result<ChatSession, StoreError>;
    fn rename(&self, id: Uuid, label: &str) -> Result<(), StoreError>;
    fn delete(&self, id: Uuid) -> Result<(), StoreError>;
    /// Bumps `last_used_ms` to now.
    fn touch(&self, id: Uuid) -> Result<(), StoreError>;
    fn set_claude_session_id(
        &self,
        id: Uuid,
        claude_session_id: Option<&str>,
    ) -> Result<(), StoreError>;
}

pub struct SqliteChatSessionRepository {
    store: Store,
}

impl SqliteChatSessionRepository {
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

fn row_to_chat_session(row: &crate::sql::Row<'_>) -> crate::sql::Result<ChatSession> {
    let id_text: String = row.get(0)?;
    let id = Uuid::parse_str(&id_text).map_err(|_| invalid_uuid(id_text))?;
    let kind: String = row.get(1)?;
    let scope_id_text: Option<String> = row.get(2)?;
    let scope = match (kind.as_str(), scope_id_text) {
        ("project", Some(s)) => {
            let scope_id =
                Uuid::parse_str(&s).map_err(|_| invalid_uuid(s))?;
            ChatScope::Project(scope_id)
        }
        ("vault", None) => ChatScope::Vault,
        other => {
            return Err(crate::sql::Error::FromSqlConversionFailure(
                0,
                crate::sql::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("invalid (scope_kind, scope_id) tuple: {other:?}"),
                )),
            ));
        }
    };
    Ok(ChatSession {
        id,
        scope,
        label: row.get(3)?,
        claude_session_id: row.get(4)?,
        last_used_ms: row.get(5)?,
        created_ms: row.get(6)?,
    })
}

fn scope_columns(scope: ChatScope) -> (&'static str, Option<String>) {
    match scope {
        ChatScope::Project(id) => ("project", Some(id.to_string())),
        ChatScope::Vault => ("vault", None),
    }
}

impl ChatSessionRepository for SqliteChatSessionRepository {
    fn list_in_scope(&self, scope: ChatScope) -> Result<Vec<ChatSession>, StoreError> {
        let conn = self.store.conn()?;
        let (kind, scope_id) = scope_columns(scope);
        // SQLite `IS` works for both NULL and value comparisons, so the
        // same query covers both vault (scope_id=NULL) and project rows.
        let mut stmt = conn.prepare(
            "SELECT id, scope_kind, scope_id, label, claude_session_id,
                    last_used_ms, created_ms
             FROM chat_session
             WHERE scope_kind = ?1 AND scope_id IS ?2
             ORDER BY last_used_ms DESC, created_ms DESC",
        )?;
        let rows = stmt.query_map(params![kind, scope_id], row_to_chat_session)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    fn get(&self, id: Uuid) -> Result<Option<ChatSession>, StoreError> {
        let conn = self.store.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, scope_kind, scope_id, label, claude_session_id,
                    last_used_ms, created_ms
             FROM chat_session
             WHERE id = ?1",
        )?;
        let row = stmt
            .query_row(params![id.to_string()], row_to_chat_session)
            .optional()?;
        Ok(row)
    }

    fn create(&self, scope: ChatScope, label: &str) -> Result<ChatSession, StoreError> {
        let trimmed = label.trim();
        let resolved_label = if trimmed.is_empty() { "New chat" } else { trimmed };
        let id = Uuid::new_v4();
        let now = now_ms();
        let (kind, scope_id) = scope_columns(scope);
        let conn = self.store.conn()?;
        conn.execute(
            "INSERT INTO chat_session
                (id, scope_kind, scope_id, label, claude_session_id,
                 last_used_ms, created_ms)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?5)",
            params![id.to_string(), kind, scope_id, resolved_label, now],
        )?;
        Ok(ChatSession {
            id,
            scope,
            label: resolved_label.to_string(),
            claude_session_id: None,
            last_used_ms: now,
            created_ms: now,
        })
    }

    fn rename(&self, id: Uuid, label: &str) -> Result<(), StoreError> {
        let trimmed = label.trim();
        if trimmed.is_empty() {
            return Err(StoreError::InvalidArgument(
                "session label must not be empty".into(),
            ));
        }
        let conn = self.store.conn()?;
        let n = conn.execute(
            "UPDATE chat_session SET label = ?2 WHERE id = ?1",
            params![id.to_string(), trimmed],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    fn delete(&self, id: Uuid) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute(
            "DELETE FROM chat_session WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(())
    }

    fn touch(&self, id: Uuid) -> Result<(), StoreError> {
        let now = now_ms();
        let conn = self.store.conn()?;
        let n = conn.execute(
            "UPDATE chat_session SET last_used_ms = ?2 WHERE id = ?1",
            params![id.to_string(), now],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    fn set_claude_session_id(
        &self,
        id: Uuid,
        claude_session_id: Option<&str>,
    ) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        let n = conn.execute(
            "UPDATE chat_session SET claude_session_id = ?2 WHERE id = ?1",
            params![id.to_string(), claude_session_id],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::open_in_memory;

    fn make_repo() -> SqliteChatSessionRepository {
        let store = open_in_memory().unwrap();
        SqliteChatSessionRepository::new(store)
    }

    #[test]
    fn create_and_list_in_scope() {
        let repo = make_repo();
        let proj = Uuid::new_v4();
        let other_proj = Uuid::new_v4();

        let a = repo
            .create(ChatScope::Project(proj), "alpha")
            .unwrap();
        // Sleep so last_used_ms differs deterministically — sub-ms precision
        // would otherwise tie the order of two same-scope inserts.
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = repo
            .create(ChatScope::Project(proj), "beta")
            .unwrap();
        // Different scopes should not bleed into each other.
        let _ = repo
            .create(ChatScope::Project(other_proj), "elsewhere")
            .unwrap();
        let _vault = repo.create(ChatScope::Vault, "vault chat").unwrap();

        let project_rows = repo
            .list_in_scope(ChatScope::Project(proj))
            .unwrap();
        assert_eq!(project_rows.len(), 2);
        // last_used_ms DESC: most recently created appears first.
        assert_eq!(project_rows[0].id, b.id);
        assert_eq!(project_rows[1].id, a.id);

        let vault_rows = repo.list_in_scope(ChatScope::Vault).unwrap();
        assert_eq!(vault_rows.len(), 1);
        assert!(matches!(vault_rows[0].scope, ChatScope::Vault));
    }

    #[test]
    fn vault_rows_have_null_scope_id() {
        let repo = make_repo();
        let s = repo.create(ChatScope::Vault, "g").unwrap();
        assert!(matches!(s.scope, ChatScope::Vault));
        let got = repo.get(s.id).unwrap().expect("get vault row");
        assert!(matches!(got.scope, ChatScope::Vault));
    }

    #[test]
    fn rename_round_trip() {
        let repo = make_repo();
        let s = repo.create(ChatScope::Vault, "first").unwrap();
        repo.rename(s.id, "renamed").unwrap();
        assert_eq!(repo.get(s.id).unwrap().unwrap().label, "renamed");
    }

    #[test]
    fn rename_rejects_empty_label() {
        let repo = make_repo();
        let s = repo.create(ChatScope::Vault, "first").unwrap();
        let err = repo.rename(s.id, "  ").unwrap_err();
        assert!(matches!(err, StoreError::InvalidArgument(_)));
    }

    #[test]
    fn create_default_label_when_empty() {
        let repo = make_repo();
        let s = repo.create(ChatScope::Vault, "  ").unwrap();
        assert_eq!(s.label, "New chat");
    }

    #[test]
    fn touch_updates_last_used_ms_and_reorders_listing() {
        let repo = make_repo();
        let proj = Uuid::new_v4();
        let a = repo.create(ChatScope::Project(proj), "alpha").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = repo.create(ChatScope::Project(proj), "beta").unwrap();
        // Initially: b is more recent.
        let listed = repo.list_in_scope(ChatScope::Project(proj)).unwrap();
        assert_eq!(listed[0].id, b.id);

        std::thread::sleep(std::time::Duration::from_millis(2));
        repo.touch(a.id).unwrap();
        let listed = repo.list_in_scope(ChatScope::Project(proj)).unwrap();
        assert_eq!(listed[0].id, a.id, "touched session bubbles to top");
    }

    #[test]
    fn delete_removes_row() {
        let repo = make_repo();
        let s = repo.create(ChatScope::Vault, "doomed").unwrap();
        repo.delete(s.id).unwrap();
        assert!(repo.get(s.id).unwrap().is_none());
    }

    #[test]
    fn set_claude_session_id_round_trip() {
        let repo = make_repo();
        let s = repo.create(ChatScope::Vault, "x").unwrap();
        repo.set_claude_session_id(s.id, Some("claude-abc-123")).unwrap();
        assert_eq!(
            repo.get(s.id).unwrap().unwrap().claude_session_id.as_deref(),
            Some("claude-abc-123")
        );
        repo.set_claude_session_id(s.id, None).unwrap();
        assert!(repo.get(s.id).unwrap().unwrap().claude_session_id.is_none());
    }

    #[test]
    fn get_unknown_id_returns_none() {
        let repo = make_repo();
        assert!(repo.get(Uuid::new_v4()).unwrap().is_none());
    }
}
