//! In-process buffer for surfaced problems (cascade errors, etc.).
//!
//! Backed by an `Arc<RwLock<VecDeque<Problem>>>` (same Send + Sync
//! shape as [`crate::log::LogBuffer`]) but wrapped in a
//! `GlobalSignal` because the cascade orchestrator runs in a
//! `spawn_forever`-detached scope and writes to a context-provided
//! `Signal<…>` from there would emit a `__copy_value_hoisted`
//! warning. Same pattern as [`crate::shell::companion_state::
//! CASCADE_STATE`] / `LOCAL_NOTE_VERSION`.
//!
//! Capacity is intentionally smaller than `LogBuffer`'s 1000 — a
//! Problems list is meant to be triaged quickly; older items roll
//! off the front when newer ones land.

use dioxus::signals::{GlobalSignal, Signal};
use std::collections::VecDeque;
use std::sync::{Arc, RwLock};
use uuid::Uuid;
use web_time::SystemTime;

pub const MAX_ENTRIES: usize = 200;

/// Where a problem originated. Currently only the cascade pushes
/// here, but the enum leaves room for the workflow executor, the
/// runner, plugin loaders, etc., to surface their own failures
/// without coupling the panel to a single subsystem.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum ProblemSource {
    Cascade,
}

impl ProblemSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cascade => "cascade",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Problem {
    pub ts: SystemTime,
    pub source: ProblemSource,
    /// Optional artifact link — set when the problem has a
    /// well-defined source artifact (e.g., a cascade skill failure
    /// on a specific Feature). Clicking it opens the artifact in a
    /// new tab via the panel's `NoteLinkResolver`.
    pub artifact_id: Option<Uuid>,
    /// Optional skill / context label shown next to the source
    /// badge ("04-decompose-stories", etc.).
    pub label: Option<String>,
    pub message: String,
}

impl Problem {
    pub fn new(
        source: ProblemSource,
        artifact_id: Option<Uuid>,
        label: Option<String>,
        message: String,
    ) -> Self {
        Self {
            ts: SystemTime::now(),
            source,
            artifact_id,
            label,
            message,
        }
    }
}

#[derive(Clone, Default)]
pub struct ProblemsBuffer {
    inner: Arc<RwLock<VecDeque<Problem>>>,
}

impl ProblemsBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, problem: Problem) {
        let mut q = match self.inner.write() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if q.len() >= MAX_ENTRIES {
            q.pop_front();
        }
        q.push_back(problem);
    }

    pub fn snapshot(&self) -> Vec<Problem> {
        match self.inner.read() {
            Ok(g) => g.iter().cloned().collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        match self.inner.read() {
            Ok(g) => g.len(),
            Err(_) => 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&self) {
        if let Ok(mut q) = self.inner.write() {
            q.clear();
        }
    }
}

/// Application-wide Problems buffer. Read by the
/// [`crate::panel::ProblemsView`] tab; written by the cascade
/// orchestrator on level-end and on terminal failures via
/// [`push_cascade_problem`] / [`push_cascade_failure`].
///
/// `GlobalSignal` (not a context-provided `Signal`) so writes from
/// `spawn_forever`-detached scopes don't emit the
/// `__copy_value_hoisted` warning the context API rejects. Mirrors
/// the [`crate::shell::companion_state::CASCADE_STATE`] pattern.
pub static PROBLEMS: GlobalSignal<ProblemsBuffer> =
    Signal::global(ProblemsBuffer::new);

/// Push one cascade-side problem into the global buffer. Bumps the
/// signal so the panel re-renders.
pub fn push_cascade_problem(
    artifact_id: Option<Uuid>,
    label: Option<String>,
    message: String,
) {
    PROBLEMS.with_mut(|b| {
        b.push(Problem::new(
            ProblemSource::Cascade,
            artifact_id,
            label,
            message,
        ))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_respects_cap() {
        let buf = ProblemsBuffer::new();
        for i in 0..(MAX_ENTRIES + 50) {
            buf.push(Problem::new(
                ProblemSource::Cascade,
                None,
                None,
                format!("msg {i}"),
            ));
        }
        let snap = buf.snapshot();
        assert_eq!(snap.len(), MAX_ENTRIES);
        // Oldest 50 should have rolled off the front.
        assert_eq!(snap.first().unwrap().message, format!("msg 50"));
        assert_eq!(
            snap.last().unwrap().message,
            format!("msg {}", MAX_ENTRIES + 49)
        );
    }

    #[test]
    fn clear_empties_the_buffer() {
        let buf = ProblemsBuffer::new();
        buf.push(Problem::new(
            ProblemSource::Cascade,
            None,
            None,
            "boom".into(),
        ));
        assert_eq!(buf.len(), 1);
        buf.clear();
        assert!(buf.is_empty());
    }

    #[test]
    fn source_labels_are_lowercase() {
        assert_eq!(ProblemSource::Cascade.label(), "cascade");
    }
}
