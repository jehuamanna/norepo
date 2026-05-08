//! Persisted transcript rows for the companion pane (M1.5b task #12).
//!
//! One row per visible block — user line, assistant text, thinking,
//! tool_call, system notice. Streaming text deltas are NOT persisted per
//! delta; the companion accumulates the consolidated assistant body and
//! calls `append` once per turn to write it. Tool calls are written when
//! the `tool_use` event arrives; the matching `tool_result` later patches
//! the same row via `update_tool_result` so a reload renders the full
//! round-trip card.

use crate::sql::params;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::StoreError;
use crate::store::Store;
use crate::time::now_ms;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatMessageKind {
    User,
    Assistant,
    Thinking,
    ToolCall,
    System,
}

impl ChatMessageKind {
    fn as_str(&self) -> &'static str {
        match self {
            ChatMessageKind::User => "user",
            ChatMessageKind::Assistant => "assistant",
            ChatMessageKind::Thinking => "thinking",
            ChatMessageKind::ToolCall => "tool_call",
            ChatMessageKind::System => "system",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "user" => Some(ChatMessageKind::User),
            "assistant" => Some(ChatMessageKind::Assistant),
            "thinking" => Some(ChatMessageKind::Thinking),
            "tool_call" => Some(ChatMessageKind::ToolCall),
            "system" => Some(ChatMessageKind::System),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub id: Uuid,
    pub chat_session_id: Uuid,
    pub sequence: i64,
    pub kind: ChatMessageKind,
    pub tool_use_id: Option<String>,
    /// Variant-specific payload. The companion knows its own shape per
    /// `kind`; the store layer treats it as opaque JSON.
    pub body: serde_json::Value,
    pub created_at_ms: i64,
}

pub trait ChatMessageRepository: Send + Sync {
    /// Ordered by sequence ASC. Empty when the session has no rows yet
    /// (e.g., a freshly-created chat).
    fn list(&self, chat_session_id: Uuid) -> Result<Vec<ChatMessage>, StoreError>;

    /// Append a new row. `tool_use_id` is required when `kind ==
    /// ToolCall` so the later `update_tool_result` can locate the row.
    fn append(
        &self,
        chat_session_id: Uuid,
        kind: ChatMessageKind,
        tool_use_id: Option<&str>,
        body: &serde_json::Value,
    ) -> Result<ChatMessage, StoreError>;

    /// Patch the body of an existing tool_call row identified by
    /// `(chat_session_id, tool_use_id)`. Returns `Err(NotFound)` if no
    /// matching row exists (caller should usually log + ignore).
    fn update_tool_result(
        &self,
        chat_session_id: Uuid,
        tool_use_id: &str,
        body: &serde_json::Value,
    ) -> Result<(), StoreError>;
}

pub struct SqliteChatMessageRepository {
    store: Store,
}

impl SqliteChatMessageRepository {
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

fn invalid_kind(s: String) -> crate::sql::Error {
    crate::sql::Error::FromSqlConversionFailure(
        0,
        crate::sql::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid kind: {s}"),
        )),
    )
}

fn invalid_json(e: serde_json::Error) -> crate::sql::Error {
    crate::sql::Error::FromSqlConversionFailure(
        0,
        crate::sql::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid body_json: {e}"),
        )),
    )
}

fn row_to_chat_message(row: &crate::sql::Row<'_>) -> crate::sql::Result<ChatMessage> {
    let id_text: String = row.get(0)?;
    let id = Uuid::parse_str(&id_text).map_err(|_| invalid_uuid(id_text))?;
    let session_text: String = row.get(1)?;
    let chat_session_id =
        Uuid::parse_str(&session_text).map_err(|_| invalid_uuid(session_text))?;
    let sequence: i64 = row.get(2)?;
    let kind_text: String = row.get(3)?;
    let kind =
        ChatMessageKind::from_str(&kind_text).ok_or_else(|| invalid_kind(kind_text))?;
    let tool_use_id: Option<String> = row.get(4)?;
    let body_text: String = row.get(5)?;
    let body: serde_json::Value =
        serde_json::from_str(&body_text).map_err(invalid_json)?;
    let created_at_ms: i64 = row.get(6)?;
    Ok(ChatMessage {
        id,
        chat_session_id,
        sequence,
        kind,
        tool_use_id,
        body,
        created_at_ms,
    })
}

impl ChatMessageRepository for SqliteChatMessageRepository {
    fn list(&self, chat_session_id: Uuid) -> Result<Vec<ChatMessage>, StoreError> {
        let conn = self.store.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, chat_session_id, sequence, kind, tool_use_id, body_json,
                    created_at_ms
             FROM chat_message
             WHERE chat_session_id = ?1
             ORDER BY sequence ASC",
        )?;
        let rows = stmt
            .query_map(params![chat_session_id.to_string()], row_to_chat_message)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    fn append(
        &self,
        chat_session_id: Uuid,
        kind: ChatMessageKind,
        tool_use_id: Option<&str>,
        body: &serde_json::Value,
    ) -> Result<ChatMessage, StoreError> {
        if matches!(kind, ChatMessageKind::ToolCall) && tool_use_id.is_none() {
            return Err(StoreError::InvalidArgument(
                "tool_call rows require a tool_use_id".into(),
            ));
        }
        let body_json = serde_json::to_string(body).map_err(|e| {
            StoreError::InvalidArgument(format!("serialize body: {e}"))
        })?;
        let id = Uuid::new_v4();
        let now = now_ms();
        let mut conn = self.store.conn()?;
        let tx = conn.transaction()?;
        let next_sequence: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(sequence), -1) + 1
                 FROM chat_message
                 WHERE chat_session_id = ?1",
                params![chat_session_id.to_string()],
                |row| row.get(0),
            )
            .unwrap_or(0);
        tx.execute(
            "INSERT INTO chat_message
                (id, chat_session_id, sequence, kind, tool_use_id, body_json, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id.to_string(),
                chat_session_id.to_string(),
                next_sequence,
                kind.as_str(),
                tool_use_id,
                body_json,
                now,
            ],
        )?;
        tx.commit()?;
        Ok(ChatMessage {
            id,
            chat_session_id,
            sequence: next_sequence,
            kind,
            tool_use_id: tool_use_id.map(|s| s.to_string()),
            body: body.clone(),
            created_at_ms: now,
        })
    }

    fn update_tool_result(
        &self,
        chat_session_id: Uuid,
        tool_use_id: &str,
        body: &serde_json::Value,
    ) -> Result<(), StoreError> {
        let body_json = serde_json::to_string(body).map_err(|e| {
            StoreError::InvalidArgument(format!("serialize body: {e}"))
        })?;
        let conn = self.store.conn()?;
        let n = conn.execute(
            "UPDATE chat_message
             SET body_json = ?3
             WHERE chat_session_id = ?1
               AND tool_use_id = ?2
               AND kind = 'tool_call'",
            params![chat_session_id.to_string(), tool_use_id, body_json],
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
    use crate::repos::{ChatScope, ChatSessionRepository, SqliteChatSessionRepository};
    use crate::test_support::open_in_memory;

    fn make_repos() -> (
        SqliteChatSessionRepository,
        SqliteChatMessageRepository,
        Uuid,
    ) {
        let store = open_in_memory().unwrap();
        let sess_repo = SqliteChatSessionRepository::new(store.clone());
        let msg_repo = SqliteChatMessageRepository::new(store);
        let session = sess_repo
            .create(ChatScope::Vault, "test session")
            .unwrap();
        (sess_repo, msg_repo, session.id)
    }

    fn body_text(text: &str) -> serde_json::Value {
        serde_json::json!({ "text": text })
    }

    #[test]
    fn append_assigns_dense_sequences() {
        let (_, repo, sid) = make_repos();
        let a = repo
            .append(sid, ChatMessageKind::User, None, &body_text("hi"))
            .unwrap();
        let b = repo
            .append(sid, ChatMessageKind::Assistant, None, &body_text("hello"))
            .unwrap();
        let c = repo
            .append(sid, ChatMessageKind::User, None, &body_text("again"))
            .unwrap();
        assert_eq!(a.sequence, 0);
        assert_eq!(b.sequence, 1);
        assert_eq!(c.sequence, 2);
    }

    #[test]
    fn list_returns_rows_in_sequence_order() {
        let (_, repo, sid) = make_repos();
        repo.append(sid, ChatMessageKind::User, None, &body_text("one"))
            .unwrap();
        repo.append(sid, ChatMessageKind::Assistant, None, &body_text("two"))
            .unwrap();
        repo.append(sid, ChatMessageKind::User, None, &body_text("three"))
            .unwrap();
        let rows = repo.list(sid).unwrap();
        let labels: Vec<_> = rows
            .iter()
            .map(|r| r.body["text"].as_str().unwrap())
            .collect();
        assert_eq!(labels, vec!["one", "two", "three"]);
    }

    #[test]
    fn list_for_unknown_session_returns_empty() {
        let (_, repo, _sid) = make_repos();
        let rows = repo.list(Uuid::new_v4()).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn append_tool_call_requires_tool_use_id() {
        let (_, repo, sid) = make_repos();
        let err = repo
            .append(sid, ChatMessageKind::ToolCall, None, &serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, StoreError::InvalidArgument(_)));
    }

    #[test]
    fn update_tool_result_patches_body() {
        let (_, repo, sid) = make_repos();
        let initial = serde_json::json!({
            "id": "tool-123",
            "name": "Read",
            "input": { "file_path": "/tmp/foo" },
            "result": null,
        });
        let _ = repo
            .append(sid, ChatMessageKind::ToolCall, Some("tool-123"), &initial)
            .unwrap();

        let patched = serde_json::json!({
            "id": "tool-123",
            "name": "Read",
            "input": { "file_path": "/tmp/foo" },
            "result": { "content": "file body", "is_error": false },
        });
        repo.update_tool_result(sid, "tool-123", &patched).unwrap();

        let rows = repo.list(sid).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].body["result"]["content"].as_str(),
            Some("file body")
        );
    }

    #[test]
    fn update_tool_result_unknown_id_errors() {
        let (_, repo, sid) = make_repos();
        let err = repo
            .update_tool_result(sid, "nonexistent", &serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, StoreError::NotFound));
    }

    #[test]
    fn delete_session_cascades_to_messages() {
        let (sess_repo, msg_repo, sid) = make_repos();
        msg_repo
            .append(sid, ChatMessageKind::User, None, &body_text("hi"))
            .unwrap();
        msg_repo
            .append(sid, ChatMessageKind::Assistant, None, &body_text("hello"))
            .unwrap();
        sess_repo.delete(sid).unwrap();
        let rows = msg_repo.list(sid).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn rows_are_isolated_per_session() {
        let store = open_in_memory().unwrap();
        let sess_repo = SqliteChatSessionRepository::new(store.clone());
        let msg_repo = SqliteChatMessageRepository::new(store);
        let s_a = sess_repo.create(ChatScope::Vault, "A").unwrap().id;
        let s_b = sess_repo.create(ChatScope::Vault, "B").unwrap().id;
        msg_repo
            .append(s_a, ChatMessageKind::User, None, &body_text("a"))
            .unwrap();
        msg_repo
            .append(s_b, ChatMessageKind::User, None, &body_text("b"))
            .unwrap();
        assert_eq!(msg_repo.list(s_a).unwrap().len(), 1);
        assert_eq!(msg_repo.list(s_b).unwrap().len(), 1);
        // Sequences restart per session.
        assert_eq!(msg_repo.list(s_a).unwrap()[0].sequence, 0);
        assert_eq!(msg_repo.list(s_b).unwrap()[0].sequence, 0);
    }
}
