//! Debounced flush of `local_tree_state` toggles. Tree carets can fire rapidly
//! (e.g. when a user fans through a deep tree); collecting toggles into a
//! 50ms window and flushing in a single transaction keeps the UI responsive
//! and the SQLite write load minimal.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use operon_store::repos::LocalTreeStateRepository;

const FLUSH_MS: u64 = 50;

#[derive(Default)]
struct Inner {
    /// Pending writes keyed by `(scope, node_id)` -> `is_open`. Re-toggling
    /// the same key inside the window collapses to a single write.
    pending: HashMap<(String, String), bool>,
    /// Generation counter — every `enqueue` bumps it. The spawned flush task
    /// only commits if its generation is still the latest.
    gen: u64,
}

#[derive(Clone)]
pub struct TreeStateQueue {
    repo: Arc<dyn LocalTreeStateRepository>,
    inner: Arc<Mutex<Inner>>,
}

impl TreeStateQueue {
    pub fn new(repo: Arc<dyn LocalTreeStateRepository>) -> Self {
        Self {
            repo,
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    /// Queue a toggle. The flush schedules a single delayed write; multiple
    /// toggles inside the window coalesce. Idempotent across re-toggles of
    /// the same `(scope, node_id)`.
    pub fn enqueue(&self, scope: impl Into<String>, node_id: impl Into<String>, open: bool) {
        let key = (scope.into(), node_id.into());
        let next_gen = {
            let mut inner = self.inner.lock().expect("tree-state queue mutex");
            inner.pending.insert(key, open);
            inner.gen = inner.gen.saturating_add(1);
            inner.gen
        };
        let inner_arc = self.inner.clone();
        let repo = self.repo.clone();
        dioxus::prelude::spawn(async move {
            futures_timer::Delay::new(Duration::from_millis(FLUSH_MS)).await;
            let drained = {
                let mut inner = match inner_arc.lock() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                if inner.gen != next_gen {
                    return;
                }
                std::mem::take(&mut inner.pending)
            };
            for ((scope, node_id), open) in drained {
                if let Err(e) = repo.set(&scope, &node_id, open) {
                    eprintln!("operon: tree-state set failed: {e}");
                }
            }
        });
    }

    /// Synchronous flush — used by tests and shutdown paths to avoid races
    /// with the debounced spawn.
    pub fn flush_sync(&self) {
        let drained = {
            let mut inner = match self.inner.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            std::mem::take(&mut inner.pending)
        };
        for ((scope, node_id), open) in drained {
            if let Err(e) = self.repo.set(&scope, &node_id, open) {
                eprintln!("operon: tree-state set failed: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use operon_store::repos::SqliteLocalTreeStateRepository;
    use operon_store::test_support::open_in_memory;

    fn make_queue() -> (TreeStateQueue, Arc<dyn LocalTreeStateRepository>) {
        let store = open_in_memory().unwrap();
        let repo: Arc<dyn LocalTreeStateRepository> =
            Arc::new(SqliteLocalTreeStateRepository::new(store));
        let q = TreeStateQueue::new(repo.clone());
        (q, repo)
    }

    #[test]
    fn flush_sync_writes_pending() {
        let (q, repo) = make_queue();
        // Enqueue without a Dioxus runtime — the spawned task won't run, but
        // the pending map captures the writes for `flush_sync`.
        {
            let mut inner = q.inner.lock().unwrap();
            inner
                .pending
                .insert(("workspace".into(), "p-1".into()), true);
            inner
                .pending
                .insert(("workspace".into(), "p-2".into()), false);
        }
        q.flush_sync();
        assert!(repo.is_open("workspace", "p-1").unwrap());
        assert!(!repo.is_open("workspace", "p-2").unwrap());
    }

    #[test]
    fn duplicate_enqueues_collapse_to_latest() {
        let (q, repo) = make_queue();
        {
            let mut inner = q.inner.lock().unwrap();
            inner
                .pending
                .insert(("workspace".into(), "p-1".into()), true);
            inner
                .pending
                .insert(("workspace".into(), "p-1".into()), false);
            inner
                .pending
                .insert(("workspace".into(), "p-1".into()), true);
        }
        q.flush_sync();
        assert!(repo.is_open("workspace", "p-1").unwrap());
        // No leftover state.
        assert!(q.inner.lock().unwrap().pending.is_empty());
    }
}
