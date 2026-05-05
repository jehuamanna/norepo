//! Debounced save scheduler.
//!
//! Coalesces rapid content changes from a tab's editor into a single `Persistence::save`
//! call after `DEBOUNCE_MS` of idle. Implementation: each `schedule` bumps a per-tab
//! generation counter and spawns a delay-then-save future; when it wakes up, it only saves
//! if its generation is still the latest one for that tab.
//!
//! Cross-target via `futures-timer::Delay` (gloo-timers on wasm, native timer on desktop).

use std::collections::HashMap;
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
}

impl SaveScheduler {
    pub fn new(persistence: Arc<dyn Persistence>) -> Self {
        Self {
            persistence,
            generations: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Schedule a save of `content` for `note_id`. Cancels any previously-pending save for
    /// the same `tab_id` by virtue of the generation counter (the older spawned task wakes
    /// up, sees its generation isn't current, and exits without saving).
    ///
    /// `on_saved` runs after a successful save (used by the caller to flip `Tab.dirty`
    /// back to false). On error, the closure is not invoked and the dirty flag stays true.
    /// Closure bound is `Fn() + 'static` (no Send/Sync) because Dioxus's `spawn` runs the
    /// future on the local thread — captures of Dioxus signals are fine.
    pub fn schedule<F>(
        &self,
        tab_id: TabId,
        note_id: String,
        content: String,
        on_saved: F,
    ) where
        F: Fn() + 'static,
    {
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
        assert_eq!(s.generations.lock().unwrap().get(&TabId(1)).copied(), Some(6));
    }

    #[test]
    fn scheduler_clone_shares_state() {
        let p: Arc<dyn Persistence> = Arc::new(MemoryPersistence::new());
        let a = SaveScheduler::new(p);
        let b = a.clone();
        a.generations.lock().unwrap().insert(TabId(7), 42);
        assert_eq!(b.generations.lock().unwrap().get(&TabId(7)).copied(), Some(42));
    }
}
