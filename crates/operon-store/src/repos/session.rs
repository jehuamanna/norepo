use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::StoreError;
use crate::ids::{OrgId, SessionId, UserId};
use crate::sqlite::Store;
use crate::time::now_ms;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Session {
    pub id: SessionId,
    pub user_id: UserId,
    pub active_org_id: Option<OrgId>,
    pub token_hash: String,
    pub expires_at_ms: i64,
    pub created_at_ms: i64,
    pub last_seen_at_ms: i64,
}

pub trait SessionRepository: Send + Sync {
    fn create(&self, s: &Session) -> Result<(), StoreError>;
    fn by_token_hash(&self, token_hash: &str) -> Result<Option<Session>, StoreError>;
    fn touch_last_seen(&self, id: &SessionId) -> Result<(), StoreError>;
    fn set_active_org(&self, id: &SessionId, org: Option<&OrgId>) -> Result<(), StoreError>;
    fn delete(&self, id: &SessionId) -> Result<(), StoreError>;
    fn delete_for_user(&self, user: &UserId) -> Result<(), StoreError>;
}

pub struct SqliteSessionRepository {
    store: Store,
}

impl SqliteSessionRepository {
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

fn row_to_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<Session> {
    let active: Option<String> = row.get(2)?;
    let active_org_id = match active {
        Some(s) => Some(OrgId::from_str_strict(&s).map_err(invalid)?),
        None => None,
    };
    Ok(Session {
        id: SessionId::from_str_strict(&row.get::<_, String>(0)?).map_err(invalid)?,
        user_id: UserId::from_str_strict(&row.get::<_, String>(1)?).map_err(invalid)?,
        active_org_id,
        token_hash: row.get(3)?,
        expires_at_ms: row.get(4)?,
        created_at_ms: row.get(5)?,
        last_seen_at_ms: row.get(6)?,
    })
}

const SELECT_COLS: &str =
    "id, user_id, active_org_id, token_hash, expires_at_ms, created_at_ms, last_seen_at_ms";

impl SessionRepository for SqliteSessionRepository {
    fn create(&self, s: &Session) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute(
            "INSERT INTO sessions (id, user_id, active_org_id, token_hash, expires_at_ms, created_at_ms, last_seen_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                s.id.as_str(),
                s.user_id.as_str(),
                s.active_org_id.as_ref().map(|o| o.as_str()),
                s.token_hash,
                s.expires_at_ms,
                s.created_at_ms,
                s.last_seen_at_ms,
            ],
        )?;
        Ok(())
    }

    fn by_token_hash(&self, token_hash: &str) -> Result<Option<Session>, StoreError> {
        let conn = self.store.conn()?;
        let sql = format!("SELECT {SELECT_COLS} FROM sessions WHERE token_hash = ?1");
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt
            .query_row(params![token_hash], row_to_session)
            .optional()?)
    }

    fn touch_last_seen(&self, id: &SessionId) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute(
            "UPDATE sessions SET last_seen_at_ms = ?2 WHERE id = ?1",
            params![id.as_str(), now_ms()],
        )?;
        Ok(())
    }

    fn set_active_org(
        &self,
        id: &SessionId,
        org: Option<&OrgId>,
    ) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        let n = conn.execute(
            "UPDATE sessions SET active_org_id = ?2, last_seen_at_ms = ?3 WHERE id = ?1",
            params![id.as_str(), org.map(|o| o.as_str()), now_ms()],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    fn delete(&self, id: &SessionId) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute("DELETE FROM sessions WHERE id = ?1", params![id.as_str()])?;
        Ok(())
    }

    fn delete_for_user(&self, user: &UserId) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute(
            "DELETE FROM sessions WHERE user_id = ?1",
            params![user.as_str()],
        )?;
        Ok(())
    }
}
