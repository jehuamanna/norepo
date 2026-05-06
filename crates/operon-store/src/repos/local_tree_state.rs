//! Persistent open/closed state for tree nodes (projects, notes, ...). Keyed by
//! `(scope, node_id)` so the same node can carry different state in different
//! UIs (e.g. workspace explorer vs. a future picker).

use std::collections::HashMap;

use crate::sql::params;

use crate::error::StoreError;
use crate::sqlite::Store;

pub trait LocalTreeStateRepository: Send + Sync {
    /// Returns `false` for nodes the user has never toggled.
    fn is_open(&self, scope: &str, node_id: &str) -> Result<bool, StoreError>;
    fn set(&self, scope: &str, node_id: &str, open: bool) -> Result<(), StoreError>;
    fn snapshot_for_scope(&self, scope: &str) -> Result<HashMap<String, bool>, StoreError>;
}

pub struct SqliteLocalTreeStateRepository {
    store: Store,
}

impl SqliteLocalTreeStateRepository {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

impl LocalTreeStateRepository for SqliteLocalTreeStateRepository {
    fn is_open(&self, scope: &str, node_id: &str) -> Result<bool, StoreError> {
        let conn = self.store.conn()?;
        let mut stmt =
            conn.prepare("SELECT is_open FROM local_tree_state WHERE scope = ?1 AND node_id = ?2")?;
        let mut rows = stmt.query(params![scope, node_id])?;
        if let Some(row) = rows.next()? {
            let v: i64 = row.get(0)?;
            Ok(v != 0)
        } else {
            Ok(false)
        }
    }

    fn set(&self, scope: &str, node_id: &str, open: bool) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        // We always upsert so the snapshot can distinguish "closed once" from
        // "never seen". Tests rely on `set(false)` being readable.
        conn.execute(
            "INSERT INTO local_tree_state (scope, node_id, is_open)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(scope, node_id) DO UPDATE SET is_open = excluded.is_open",
            params![scope, node_id, if open { 1_i64 } else { 0 }],
        )?;
        Ok(())
    }

    fn snapshot_for_scope(&self, scope: &str) -> Result<HashMap<String, bool>, StoreError> {
        let conn = self.store.conn()?;
        let mut stmt =
            conn.prepare("SELECT node_id, is_open FROM local_tree_state WHERE scope = ?1")?;
        let rows = stmt.query_map(params![scope], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? != 0))
        })?;
        let mut out = HashMap::new();
        for r in rows {
            let (k, v) = r?;
            out.insert(k, v);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::open_in_memory;

    fn make_repo() -> SqliteLocalTreeStateRepository {
        let store = open_in_memory().unwrap();
        SqliteLocalTreeStateRepository::new(store)
    }

    #[test]
    fn local_tree_state_default_is_closed() {
        let repo = make_repo();
        assert!(!repo.is_open("workspace", "project-a").unwrap());
        let snap = repo.snapshot_for_scope("workspace").unwrap();
        assert!(snap.is_empty());
    }

    #[test]
    fn local_tree_state_set_then_snapshot_returns_open_keys() {
        let repo = make_repo();
        repo.set("workspace", "project-a", true).unwrap();
        repo.set("workspace", "project-b", true).unwrap();
        repo.set("project:1", "note-x", true).unwrap();
        assert!(repo.is_open("workspace", "project-a").unwrap());
        assert!(repo.is_open("workspace", "project-b").unwrap());
        assert!(!repo.is_open("workspace", "project-c").unwrap());

        let workspace_snap = repo.snapshot_for_scope("workspace").unwrap();
        assert_eq!(workspace_snap.get("project-a"), Some(&true));
        assert_eq!(workspace_snap.get("project-b"), Some(&true));
        assert_eq!(workspace_snap.len(), 2);

        // Cross-scope isolation.
        let other = repo.snapshot_for_scope("project:1").unwrap();
        assert_eq!(other.get("note-x"), Some(&true));
        assert!(!other.contains_key("project-a"));
    }

    #[test]
    fn local_tree_state_set_to_false_removes_or_marks_closed() {
        let repo = make_repo();
        repo.set("workspace", "project-a", true).unwrap();
        assert!(repo.is_open("workspace", "project-a").unwrap());
        repo.set("workspace", "project-a", false).unwrap();
        assert!(!repo.is_open("workspace", "project-a").unwrap());
        // Snapshot reflects the closed state explicitly (we keep the row to
        // distinguish "user closed" from "never opened" when callers care).
        let snap = repo.snapshot_for_scope("workspace").unwrap();
        assert_eq!(snap.get("project-a"), Some(&false));
    }
}
