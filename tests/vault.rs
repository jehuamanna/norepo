//! Integration tests for the vault directory primitives in
//! `src/local_mode/vault.rs` exercised against the real
//! `SqliteLocalSettingsRepository`.
//!
//! Plans-Phase-1-vault-dir / TestCase-Phase-1 — I-1 (set then read roundtrip).

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use operon_store::repos::{LocalSettingsRepository, SqliteLocalSettingsRepository};
use operon_store::Store;

use operon_dioxus::local_mode::vault::{self, VaultRoot};

fn open_store() -> Store {
    Store::open_in_memory().expect("in-memory sqlite store opens")
}

#[test]
fn vault_set_then_read_roundtrip() {
    let store = open_store();
    let repo: Arc<dyn LocalSettingsRepository> =
        Arc::new(SqliteLocalSettingsRepository::new(store));
    assert!(vault::load(&repo).unwrap().is_none(), "no vault on first run");

    let tmp = tempfile::tempdir().expect("tempdir");
    let canonical = vault::validate(tmp.path()).expect("tempdir validates");
    let root = VaultRoot { path: canonical };

    vault::store(&repo, &root).expect("vault::store");
    let loaded = vault::load(&repo).expect("vault::load").expect("Some(root)");
    assert_eq!(loaded.path, root.path);
}

#[test]
fn vault_overwrites_existing_setting() {
    let store = open_store();
    let repo: Arc<dyn LocalSettingsRepository> =
        Arc::new(SqliteLocalSettingsRepository::new(store));

    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let root_a = VaultRoot {
        path: vault::validate(a.path()).unwrap(),
    };
    let root_b = VaultRoot {
        path: vault::validate(b.path()).unwrap(),
    };

    vault::store(&repo, &root_a).unwrap();
    vault::store(&repo, &root_b).unwrap();
    let loaded = vault::load(&repo).unwrap().unwrap();
    assert_eq!(loaded.path, root_b.path);
}
