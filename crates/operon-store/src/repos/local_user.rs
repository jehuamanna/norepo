//! Local-mode single-row user identity. Backed by `local_user` (id always 1).

use crate::sql::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::StoreError;
use crate::sqlite::Store;
use crate::time::now_ms;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalUser {
    pub username: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

pub trait LocalUserRepository: Send + Sync {
    fn get(&self) -> Result<Option<LocalUser>, StoreError>;
    fn upsert(&self, username: &str) -> Result<LocalUser, StoreError>;
}

pub struct SqliteLocalUserRepository {
    store: Store,
}

impl SqliteLocalUserRepository {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

fn row_to_local_user(row: &crate::sql::Row<'_>) -> crate::sql::Result<LocalUser> {
    Ok(LocalUser {
        username: row.get(0)?,
        created_at_ms: row.get(1)?,
        updated_at_ms: row.get(2)?,
    })
}

impl LocalUserRepository for SqliteLocalUserRepository {
    fn get(&self) -> Result<Option<LocalUser>, StoreError> {
        let conn = self.store.conn()?;
        let mut stmt = conn.prepare(
            "SELECT username, created_at_ms, updated_at_ms FROM local_user WHERE id = 1",
        )?;
        Ok(stmt.query_row(params![], row_to_local_user).optional()?)
    }

    fn upsert(&self, username: &str) -> Result<LocalUser, StoreError> {
        let trimmed = username.trim();
        if trimmed.is_empty() {
            return Err(StoreError::InvalidArgument(
                "username must not be empty or whitespace-only".into(),
            ));
        }
        let now = now_ms();
        let conn = self.store.conn()?;
        // Insert-or-update on id=1; preserve created_at_ms on update.
        conn.execute(
            "INSERT INTO local_user (id, username, created_at_ms, updated_at_ms)
             VALUES (1, ?1, ?2, ?2)
             ON CONFLICT(id) DO UPDATE SET
                 username      = excluded.username,
                 updated_at_ms = excluded.updated_at_ms",
            params![trimmed, now],
        )?;
        let mut stmt = conn.prepare(
            "SELECT username, created_at_ms, updated_at_ms FROM local_user WHERE id = 1",
        )?;
        let user = stmt.query_row(params![], row_to_local_user)?;
        Ok(user)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::open_in_memory;

    #[test]
    fn local_user_repo_get_returns_none_when_empty() {
        let store = open_in_memory().unwrap();
        let repo = SqliteLocalUserRepository::new(store);
        assert!(repo.get().unwrap().is_none());
    }

    #[test]
    fn local_user_repo_upsert_rejects_empty_or_whitespace() {
        let store = open_in_memory().unwrap();
        let repo = SqliteLocalUserRepository::new(store);
        for bad in ["", "   ", "\t\n  "] {
            let err = repo.upsert(bad).unwrap_err();
            assert!(
                matches!(err, StoreError::InvalidArgument(_)),
                "expected InvalidArgument for {bad:?}, got {err:?}"
            );
        }
        assert!(repo.get().unwrap().is_none());
    }

    #[test]
    fn local_user_repo_upsert_creates_then_updates() {
        let store = open_in_memory().unwrap();
        let repo = SqliteLocalUserRepository::new(store);
        let first = repo.upsert("  alice  ").unwrap();
        assert_eq!(first.username, "alice");
        assert_eq!(first.created_at_ms, first.updated_at_ms);

        // Sleep at least 1 ms so updated_at_ms is observably different.
        std::thread::sleep(std::time::Duration::from_millis(2));

        let second = repo.upsert("bob").unwrap();
        assert_eq!(second.username, "bob");
        assert_eq!(second.created_at_ms, first.created_at_ms);
        assert!(second.updated_at_ms >= first.updated_at_ms);

        let got = repo.get().unwrap().unwrap();
        assert_eq!(got, second);
    }
}
