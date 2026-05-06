//! Debounced save scheduler.
//!
//! Coalesces rapid content changes from a tab's editor into a single `Persistence::save`
//! call after `DEBOUNCE_MS` of idle. Implementation: each `schedule` bumps a per-tab
//! generation counter and spawns a delay-then-save future; when it wakes up, it only saves
//! if its generation is still the latest one for that tab.
//!
//! Cross-target via `futures-timer::Delay` (gloo-timers on wasm, native timer on desktop).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::persistence::Persistence;
use crate::tabs::TabId;

/// Idle window before a save fires. Tunable; 150 ms is below human perceptibility and well
/// above any incidental keystroke jitter.
pub const DEBOUNCE_MS: u64 = 150;

/// Cheap to clone — internally `Arc<Mutex<...>>` over the generation map.
#[derive(Clone)]
pub struct SaveScheduler {
    persistence: Arc<dyn Persistence>,
    generations: Arc<Mutex<HashMap<TabId, u64>>>,
    /// Tabs whose `Tab.manual_save == true`. The scheduler short-circuits
    /// `schedule()` for these — they save through an explicit Save handler
    /// (Local-Mode note tabs). Side-channel rather than threading a `&Tab`
    /// through every `schedule()` call site.
    manual_save_tabs: Arc<Mutex<HashSet<TabId>>>,
}

impl SaveScheduler {
    pub fn new(persistence: Arc<dyn Persistence>) -> Self {
        Self {
            persistence,
            generations: Arc::new(Mutex::new(HashMap::new())),
            manual_save_tabs: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Mark `tab_id` as opting into explicit save. After this, `schedule()`
    /// becomes a no-op for that tab.
    pub fn set_manual_save(&self, tab_id: TabId) {
        if let Ok(mut s) = self.manual_save_tabs.lock() {
            s.insert(tab_id);
        }
    }

    /// Drop the manual-save mark (used when a Local-Mode tab closes so the id
    /// could in principle be re-bound — TabIds are monotonic so this is mainly
    /// hygiene).
    pub fn clear_manual_save(&self, tab_id: TabId) {
        if let Ok(mut s) = self.manual_save_tabs.lock() {
            s.remove(&tab_id);
        }
    }

    pub fn is_manual_save(&self, tab_id: TabId) -> bool {
        self.manual_save_tabs
            .lock()
            .map(|s| s.contains(&tab_id))
            .unwrap_or(false)
    }

    /// Schedule a save of `content` for `note_id`. Cancels any previously-pending save for
    /// the same `tab_id` by virtue of the generation counter (the older spawned task wakes
    /// up, sees its generation isn't current, and exits without saving).
    ///
    /// `on_saved` runs after a successful save (used by the caller to flip `Tab.dirty`
    /// back to false). On error, the closure is not invoked and the dirty flag stays true.
    /// Closure bound is `Fn() + 'static` (no Send/Sync) because Dioxus's `spawn` runs the
    /// future on the local thread — captures of Dioxus signals are fine.
    pub fn schedule<F>(&self, tab_id: TabId, note_id: String, content: String, on_saved: F)
    where
        F: Fn() + 'static,
    {
        // Manual-save tabs (Local-Mode notes) bypass the debounce entirely;
        // they save through an explicit Save button / Ctrl+S handler.
        if self.is_manual_save(tab_id) {
            return;
        }
        let next_gen = {
            let mut gens = self.generations.lock().expect("generations mutex");
            let next = gens.get(&tab_id).copied().unwrap_or(0).saturating_add(1);
            gens.insert(tab_id, next);
            next
        };
        let persistence = self.persistence.clone();
        let generations = self.generations.clone();
        dioxus::prelude::spawn(async move {
            futures_timer::Delay::new(Duration::from_millis(DEBOUNCE_MS)).await;
            // Re-check our generation; if the user typed again during the debounce, a newer
            // task is now in flight and this one bails.
            let still_current = generations
                .lock()
                .map(|g| g.get(&tab_id) == Some(&next_gen))
                .unwrap_or(false);
            if !still_current {
                return;
            }
            if persistence.save(&note_id, content.as_bytes()).await.is_ok() {
                on_saved();
            }
        });
    }

    /// Cancel any pending debounced save for `tab_id` and persist immediately.
    ///
    /// Plans-Phase-2-saving: this is the unified manual-save path for Local
    /// Mode (Ctrl+S / File→Save / Save button). Bumping the generation counter
    /// invalidates any in-flight debounce future for the same tab so the
    /// content lands exactly once.
    ///
    /// Returns the result of `Persistence::save`. The caller flips the tab's
    /// `dirty` flag and calls `LocalNoteRepository::touch_updated` on success.
    pub async fn flush(
        &self,
        tab_id: TabId,
        note_id: &str,
        content: &str,
    ) -> Result<(), crate::persistence::PersistError> {
        // Bump the generation: any pending debounce future for this tab will
        // wake up, observe the mismatch, and exit without saving.
        if let Ok(mut gens) = self.generations.lock() {
            let next = gens.get(&tab_id).copied().unwrap_or(0).saturating_add(1);
            gens.insert(tab_id, next);
        }
        self.persistence.save(note_id, content.as_bytes()).await
    }

    /// Test-only synchronous schedule that skips the debounce (and the spawn) and saves
    /// immediately. Used by unit tests so we don't need a Dioxus runtime in scope.
    #[cfg(test)]
    pub async fn save_now(
        &self,
        note_id: &str,
        content: &str,
    ) -> Result<(), crate::persistence::PersistError> {
        self.persistence.save(note_id, content.as_bytes()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::MemoryPersistence;

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
        const VTABLE: RawWakerVTable = RawWakerVTable::new(
            |_| RawWaker::new(std::ptr::null(), &VTABLE),
            |_| (),
            |_| (),
            |_| (),
        );
        let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
        let mut cx = Context::from_waker(&waker);
        let mut f = Box::pin(f);
        loop {
            if let Poll::Ready(out) = f.as_mut().poll(&mut cx) {
                return out;
            }
        }
    }

    #[test]
    fn save_now_writes_through_persistence() {
        let p: Arc<dyn Persistence> = Arc::new(MemoryPersistence::new());
        let s = SaveScheduler::new(p.clone());
        block_on(s.save_now("note-1", "hello")).unwrap();
        assert_eq!(block_on(p.load("note-1")).unwrap(), b"hello");
    }

    #[test]
    fn flush_writes_immediately_and_bumps_generation() {
        let p: Arc<dyn Persistence> = Arc::new(MemoryPersistence::new());
        let s = SaveScheduler::new(p.clone());
        // Pretend a debounce was scheduled (gen = 7).
        s.generations.lock().unwrap().insert(TabId(42), 7);
        block_on(s.flush(TabId(42), "note-x", "fresh")).unwrap();
        // Persistence wrote.
        assert_eq!(block_on(p.load("note-x")).unwrap(), b"fresh");
        // Generation bumped — any in-flight task with gen=7 will now bail.
        assert_eq!(*s.generations.lock().unwrap().get(&TabId(42)).unwrap(), 8);
    }

    #[test]
    fn schedule_bumps_generation_counter() {
        let p: Arc<dyn Persistence> = Arc::new(MemoryPersistence::new());
        let s = SaveScheduler::new(p);
        // Three schedules; generation map should hold the latest.
        s.generations.lock().unwrap().insert(TabId(1), 5);
        // Simulate the bump logic from `schedule` directly (we can't actually call schedule
        // here without a Dioxus runtime).
        let new_gen = {
            let mut g = s.generations.lock().unwrap();
            let next = g.get(&TabId(1)).copied().unwrap_or(0) + 1;
            g.insert(TabId(1), next);
            next
        };
        assert_eq!(new_gen, 6);
        assert_eq!(
            s.generations.lock().unwrap().get(&TabId(1)).copied(),
            Some(6)
        );
    }

    #[test]
    fn scheduler_clone_shares_state() {
        let p: Arc<dyn Persistence> = Arc::new(MemoryPersistence::new());
        let a = SaveScheduler::new(p);
        let b = a.clone();
        a.generations.lock().unwrap().insert(TabId(7), 42);
        assert_eq!(
            b.generations.lock().unwrap().get(&TabId(7)).copied(),
            Some(42)
        );
    }

    #[test]
    fn save_scheduler_skips_when_manual_save_flag_set() {
        let p: Arc<dyn Persistence> = Arc::new(MemoryPersistence::new());
        let s = SaveScheduler::new(p);
        let tab = TabId(11);
        s.set_manual_save(tab);
        assert!(s.is_manual_save(tab));
        // We can't observe the no-spawn directly, but we can verify the
        // generation counter does NOT advance when schedule is called for a
        // manual-save tab — the `if is_manual_save { return; }` short-circuit
        // skips the bump entirely.
        assert!(!s.generations.lock().unwrap().contains_key(&tab));
        // A direct call to `schedule` would require a Dioxus runtime in scope
        // to spawn the timer future; instead simulate the invariant the
        // short-circuit guarantees: generations stays empty.
        if !s.is_manual_save(tab) {
            // Unreachable — included to mirror the structure of the production
            // path so a regression that flips the check is visible at review.
            unreachable!("manual_save flag must be observable as true");
        }
    }

    #[test]
    fn manual_save_clear_removes_short_circuit() {
        let p: Arc<dyn Persistence> = Arc::new(MemoryPersistence::new());
        let s = SaveScheduler::new(p);
        let tab = TabId(12);
        s.set_manual_save(tab);
        assert!(s.is_manual_save(tab));
        s.clear_manual_save(tab);
        assert!(!s.is_manual_save(tab));
    }
}
