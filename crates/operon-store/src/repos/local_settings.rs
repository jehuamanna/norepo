//! Key/value store for local-mode app settings. Used by the startup chooser
//! to persist `mode_remembered` and similar lightweight preferences.

// Plans-Phase-2-saving: imports go through `crate::sql` so the same code
// builds on desktop (rusqlite) and wasm (the wasm-sqlite shim).
use crate::sql::{params, OptionalExtension};

use crate::error::StoreError;
use crate::store::Store;

pub trait LocalSettingsRepository: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<String>, StoreError>;
    fn set(&self, key: &str, value: &str) -> Result<(), StoreError>;
}

pub struct SqliteLocalSettingsRepository {
    store: Store,
}

impl SqliteLocalSettingsRepository {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

impl LocalSettingsRepository for SqliteLocalSettingsRepository {
    fn get(&self, key: &str) -> Result<Option<String>, StoreError> {
        let conn = self.store.conn()?;
        let mut stmt = conn.prepare("SELECT value FROM local_app_settings WHERE key = ?1")?;
        Ok(stmt
            .query_row(params![key], |row| row.get::<_, String>(0))
            .optional()?)
    }

    fn set(&self, key: &str, value: &str) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute(
            "INSERT INTO local_app_settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::open_in_memory;

    #[test]
    fn local_settings_repo_set_then_get() {
        let store = open_in_memory().unwrap();
        let repo = SqliteLocalSettingsRepository::new(store);
        repo.set("mode_remembered", "Local").unwrap();
        assert_eq!(
            repo.get("mode_remembered").unwrap(),
            Some("Local".to_string())
        );

        // Update overwrites.
        repo.set("mode_remembered", "Cloud").unwrap();
        assert_eq!(
            repo.get("mode_remembered").unwrap(),
            Some("Cloud".to_string())
        );
    }

    #[test]
    fn local_settings_repo_get_unknown_returns_none() {
        let store = open_in_memory().unwrap();
        let repo = SqliteLocalSettingsRepository::new(store);
        assert!(repo.get("nope").unwrap().is_none());
    }
}
