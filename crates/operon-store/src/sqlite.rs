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
    // With pool size > 1 in local mode, two transactions can race for
    // the writer lock. Without busy_timeout, the loser gets SQLITE_BUSY
    // immediately and the caller has to retry. 5 s is enough to ride
    // out the normal cases (a tab autosave + a bridge tool overlap, an
    // FTS rebuild) without papering over a real deadlock.
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
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
            // Local mode used to be 1 conn — fine when only the GUI
            // thread reached for the DB. The in-tree MCP bridge runs on
            // its own OS thread (and dispatches each tool call onto
            // tokio's blocking pool), so a single-conn pool now serializes
            // every bridge call behind whatever the GUI is doing and vice
            // versa. WAL allows N readers + 1 writer concurrently, so
            // bumping to 4 lets the bridge and the GUI make progress in
            // parallel without changing the writer-exclusivity contract.
            (StoreMode::Local, _) => 4,
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

    /// Convenience: open a fresh `:memory:` store in `Local` mode. Used by Phase-1
    /// local-mode unit tests and by app-level fallbacks where an in-memory store is
    /// acceptable (e.g. wasm builds).
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::open(StoreConfig::memory(StoreMode::Local))
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
