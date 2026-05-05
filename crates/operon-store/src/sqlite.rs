use std::path::PathBuf;
use std::sync::Arc;

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::Connection;

use crate::error::StoreError;
use crate::migrations;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreMode {
    Local,
    NonLocal,
}

#[derive(Debug, Clone)]
pub struct StoreConfig {
    pub path: StorePath,
    pub mode: StoreMode,
}

#[derive(Debug, Clone)]
pub enum StorePath {
    File(PathBuf),
    Memory,
}

impl StoreConfig {
    pub fn local(path: impl Into<PathBuf>) -> Self {
        Self {
            path: StorePath::File(path.into()),
            mode: StoreMode::Local,
        }
    }

    pub fn non_local(path: impl Into<PathBuf>) -> Self {
        Self {
            path: StorePath::File(path.into()),
            mode: StoreMode::NonLocal,
        }
    }

    pub fn memory(mode: StoreMode) -> Self {
        Self {
            path: StorePath::Memory,
            mode,
        }
    }
}

#[derive(Clone)]
pub struct Store {
    pool: Arc<Pool<SqliteConnectionManager>>,
    mode: StoreMode,
}

fn pragmas(conn: &mut Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(())
}

impl Store {
    /// Open the store, run any pending migrations, and return a handle.
    pub fn open(cfg: StoreConfig) -> Result<Self, StoreError> {
        let manager = match &cfg.path {
            StorePath::File(p) => SqliteConnectionManager::file(p),
            StorePath::Memory => SqliteConnectionManager::memory(),
        }
        .with_init(|c| pragmas(c));

        let pool_size = match (cfg.mode, &cfg.path) {
            (StoreMode::Local, _) => 1,
            (_, StorePath::Memory) => 1,
            (StoreMode::NonLocal, _) => 8,
        };

        let pool = Pool::builder()
            .max_size(pool_size)
            .build(manager)
            .map_err(|e| StoreError::Open(e.to_string()))?;

        let store = Self {
            pool: Arc::new(pool),
            mode: cfg.mode,
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn for_test() -> Result<Self, StoreError> {
        Self::open(StoreConfig::memory(StoreMode::NonLocal))
    }

    pub fn mode(&self) -> StoreMode {
        self.mode
    }

    pub fn pool(&self) -> &Pool<SqliteConnectionManager> {
        &self.pool
    }

    pub fn conn(&self) -> Result<r2d2::PooledConnection<SqliteConnectionManager>, StoreError> {
        self.pool.get().map_err(StoreError::from)
    }

    pub fn migrate(&self) -> Result<(), StoreError> {
        let mut conn = self.conn()?;
        migrations::migrate_up(&mut conn)
    }

    /// Roll back the most recent migration. Test-only. Drops every user table.
    #[doc(hidden)]
    pub fn migrate_down_test_only(&self) -> Result<(), StoreError> {
        let mut conn = self.conn()?;
        migrations::migrate_down_all(&mut conn)
    }
}
