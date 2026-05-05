use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::StoreError;
use crate::ids::UserId;
use crate::sqlite::Store;
use crate::time::now_ms;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct User {
    pub id: UserId,
    pub email: String,
    pub display_name: Option<String>,
    pub password_hash: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl User {
    pub fn new_with_email(email: impl Into<String>) -> Self {
        let now = now_ms();
        Self {
            id: UserId::new(),
            email: email.into(),
            display_name: None,
            password_hash: None,
            created_at_ms: now,
            updated_at_ms: now,
        }
    }
}

pub trait UserRepository: Send + Sync {
    fn create(&self, user: &User) -> Result<(), StoreError>;
    fn get(&self, id: &UserId) -> Result<Option<User>, StoreError>;
    fn by_email(&self, email: &str) -> Result<Option<User>, StoreError>;
    fn update(&self, user: &User) -> Result<(), StoreError>;
    fn delete(&self, id: &UserId) -> Result<(), StoreError>;
    fn list(&self, limit: usize, after: Option<UserId>) -> Result<Vec<User>, StoreError>;
}

pub struct SqliteUserRepository {
    store: Store,
}

impl SqliteUserRepository {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

fn row_to_user(row: &rusqlite::Row<'_>) -> rusqlite::Result<User> {
    let id_str: String = row.get(0)?;
    let id = UserId::from_str_strict(&id_str)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))))?;
    Ok(User {
        id,
        email: row.get(1)?,
        display_name: row.get(2)?,
        password_hash: row.get(3)?,
        created_at_ms: row.get(4)?,
        updated_at_ms: row.get(5)?,
    })
}

impl UserRepository for SqliteUserRepository {
    fn create(&self, user: &User) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute(
            "INSERT INTO users (id, email, display_name, password_hash, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                user.id.as_str(),
                user.email,
                user.display_name,
                user.password_hash,
                user.created_at_ms,
                user.updated_at_ms,
            ],
        )?;
        Ok(())
    }

    fn get(&self, id: &UserId) -> Result<Option<User>, StoreError> {
        let conn = self.store.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, email, display_name, password_hash, created_at_ms, updated_at_ms
             FROM users WHERE id = ?1",
        )?;
        let user = stmt
            .query_row(params![id.as_str()], row_to_user)
            .optional()?;
        Ok(user)
    }

    fn by_email(&self, email: &str) -> Result<Option<User>, StoreError> {
        let conn = self.store.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, email, display_name, password_hash, created_at_ms, updated_at_ms
             FROM users WHERE email = ?1 COLLATE NOCASE",
        )?;
        let user = stmt
            .query_row(params![email.trim()], row_to_user)
            .optional()?;
        Ok(user)
    }

    fn update(&self, user: &User) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        let n = conn.execute(
            "UPDATE users SET email = ?2, display_name = ?3, password_hash = ?4,
                              updated_at_ms = ?5
             WHERE id = ?1",
            params![
                user.id.as_str(),
                user.email,
                user.display_name,
                user.password_hash,
                user.updated_at_ms,
            ],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    fn delete(&self, id: &UserId) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute("DELETE FROM users WHERE id = ?1", params![id.as_str()])?;
        Ok(())
    }

    fn list(&self, limit: usize, after: Option<UserId>) -> Result<Vec<User>, StoreError> {
        let conn = self.store.conn()?;
        let after_str = after.as_ref().map(|i| i.as_str());
        let mut stmt = conn.prepare(
            "SELECT id, email, display_name, password_hash, created_at_ms, updated_at_ms
             FROM users
             WHERE (?1 IS NULL OR id > ?1)
             ORDER BY id
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(
            params![after_str, limit as i64],
            row_to_user,
        )?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}
