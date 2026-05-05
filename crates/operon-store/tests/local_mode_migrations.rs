//! Migration #005 + local-mode repo persistence integration tests.

use operon_store::repos::{LocalUserRepository, SqliteLocalUserRepository};
use operon_store::test_support::fresh_store;
use operon_store::{Store, StoreConfig};

fn list_tables(store: &Store) -> Vec<String> {
    let conn = store.conn().unwrap();
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
        .unwrap();
    let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
    let mut out: Vec<String> = rows.map(|r| r.unwrap()).collect();
    out.sort();
    out
}

#[test]
fn migration_005_applies_idempotently() {
    let store = fresh_store().unwrap();
    // First migrate happens inside fresh_store(). Run again — must be a no-op.
    store.migrate().unwrap();

    let tables = list_tables(&store);
    assert!(tables.iter().any(|t| t == "local_user"));
    assert!(tables.iter().any(|t| t == "local_app_settings"));

    // Idempotency: only one row in _schema_migrations for version 5.
    let conn = store.conn().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM _schema_migrations WHERE version = 5",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn migrate_down_all_drops_local_tables() {
    let store = fresh_store().unwrap();
    let before = list_tables(&store);
    assert!(before.iter().any(|t| t == "local_user"));
    assert!(before.iter().any(|t| t == "local_app_settings"));

    store.migrate_down_test_only().unwrap();
    let after = list_tables(&store);
    assert!(after.is_empty(), "down left tables: {after:?}");
}

#[test]
fn local_user_persists_across_reopen() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();
    // Drop the temp file handle so the path is free for SQLite to own. The path itself
    // stays alive via `path` and is cleaned up at end-of-scope by `tmp` going out of scope
    // after we no longer need the DB.
    let _ = tmp.as_file();

    {
        let store = Store::open(StoreConfig::local(&path)).unwrap();
        let repo = SqliteLocalUserRepository::new(store);
        let user = repo.upsert("alice").unwrap();
        assert_eq!(user.username, "alice");
    }

    {
        let store = Store::open(StoreConfig::local(&path)).unwrap();
        let repo = SqliteLocalUserRepository::new(store);
        let got = repo.get().unwrap().expect("local_user persisted");
        assert_eq!(got.username, "alice");
    }
}
