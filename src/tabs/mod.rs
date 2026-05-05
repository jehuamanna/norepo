//! Tab state for the main area.
//!
//! [`TabManager`] is provided to the tree as `Signal<TabManager>` from the application root.
//! The [`crate::shell::MainArea`] reads it to decide which format plugin renders the body, and
//! [`TabStrip`] renders the visible row of tab buttons. Phases 3 onwards mutate it via
//! `tabs.write()`.

mod strip;

pub use strip::TabStrip;

/// Monotonic tab identifier. Newtype around `u64`; never reused.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct TabId(pub u64);

#[derive(Clone, Debug)]
pub struct Tab {
    pub id: TabId,
    pub note_id: String,
    /// Open-string format identifier — resolves to a `FormatPlugin` via the registry.
    pub format_id: String,
    pub title: String,
    pub content: String,
    pub dirty: bool,
}

#[derive(Default, Clone, Debug)]
pub struct TabManager {
    next_id: u64,
    tabs: Vec<Tab>,
    active: Option<TabId>,
}

impl TabManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a tab. If a tab with the same `note_id` already exists, it is activated and its
    /// id is returned (no second tab is created). Otherwise a fresh tab is appended and made
    /// active.
    pub fn open(
        &mut self,
        note_id: String,
        format_id: String,
        title: String,
        content: String,
    ) -> TabId {
        if let Some(id) = self
            .tabs
            .iter()
            .find(|t| t.note_id == note_id)
            .map(|t| t.id)
        {
            self.active = Some(id);
            return id;
        }
        self.next_id += 1;
        let id = TabId(self.next_id);
        self.tabs.push(Tab {
            id,
            note_id,
            format_id,
            title,
            content,
            dirty: false,
        });
        self.active = Some(id);
        id
    }

    /// Close the tab with `id`. No-op if it doesn't exist. If the closed tab was active,
    /// activates the right neighbor; if none, the left; if none, sets active to `None`.
    pub fn close(&mut self, id: TabId) {
        let Some(idx) = self.tabs.iter().position(|t| t.id == id) else {
            return;
        };
        let was_active = self.active == Some(id);
        self.tabs.remove(idx);
        if was_active {
            self.active = self.tabs.get(idx).map(|t| t.id).or_else(|| {
                if idx > 0 {
                    self.tabs.get(idx - 1).map(|t| t.id)
                } else {
                    None
                }
            });
        }
    }

    /// Make the given tab active. No-op if it doesn't exist.
    pub fn activate(&mut self, id: TabId) {
        if self.tabs.iter().any(|t| t.id == id) {
            self.active = Some(id);
        }
    }

    pub fn set_dirty(&mut self, id: TabId, dirty: bool) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
            t.dirty = dirty;
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Tab> {
        self.tabs.iter()
    }

    pub fn active(&self) -> Option<&Tab> {
        self.active.and_then(|id| self.tabs.iter().find(|t| t.id == id))
    }

    pub fn active_id(&self) -> Option<TabId> {
        self.active
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_md(tm: &mut TabManager, id: &str, title: &str) -> TabId {
        tm.open(id.into(), "markdown".into(), title.into(), String::new())
    }

    #[test]
    fn open_creates_active_tab() {
        let mut tm = TabManager::new();
        let id = open_md(&mut tm, "n1", "T");
        assert_eq!(tm.iter().count(), 1);
        assert_eq!(tm.active().map(|t| t.id), Some(id));
    }

    #[test]
    fn activate_does_not_reorder() {
        let mut tm = TabManager::new();
        let n1 = open_md(&mut tm, "n1", "T1");
        let _n2 = open_md(&mut tm, "n2", "T2");
        tm.activate(n1);
        assert_eq!(tm.active_id(), Some(n1));
        let order: Vec<_> = tm.iter().map(|t| t.note_id.clone()).collect();
        assert_eq!(order, vec!["n1", "n2"]);
    }

    #[test]
    fn close_active_focuses_right_then_left_then_none() {
        let mut tm = TabManager::new();
        let n1 = open_md(&mut tm, "n1", "T1");
        let n2 = open_md(&mut tm, "n2", "T2");
        let n3 = open_md(&mut tm, "n3", "T3");
        tm.close(n3);
        assert_eq!(tm.active_id(), Some(n2));
        tm.close(n2);
        assert_eq!(tm.active_id(), Some(n1));
        tm.close(n1);
        assert_eq!(tm.active_id(), None);
    }

    #[test]
    fn close_active_in_middle_focuses_right_neighbor() {
        let mut tm = TabManager::new();
        let _n1 = open_md(&mut tm, "n1", "T1");
        let n2 = open_md(&mut tm, "n2", "T2");
        let n3 = open_md(&mut tm, "n3", "T3");
        tm.activate(n2);
        tm.close(n2);
        assert_eq!(tm.active_id(), Some(n3));
    }

    #[test]
    fn set_dirty_flips_flag() {
        let mut tm = TabManager::new();
        let id = open_md(&mut tm, "n1", "T");
        assert!(!tm.iter().next().unwrap().dirty);
        tm.set_dirty(id, true);
        assert!(tm.iter().next().unwrap().dirty);
        tm.set_dirty(id, false);
        assert!(!tm.iter().next().unwrap().dirty);
    }

    #[test]
    fn reopen_same_note_id_activates_existing_tab() {
        let mut tm = TabManager::new();
        let n1 = open_md(&mut tm, "n1", "T1");
        let _n2 = open_md(&mut tm, "n2", "T2");
        let returned = open_md(&mut tm, "n1", "different title");
        assert_eq!(returned, n1);
        assert_eq!(tm.iter().count(), 2);
        assert_eq!(tm.active_id(), Some(n1));
    }

    #[test]
    fn tab_id_is_monotonic_and_not_reused() {
        let mut tm = TabManager::new();
        let n1 = open_md(&mut tm, "a", "A");
        let n2 = open_md(&mut tm, "b", "B");
        tm.close(n1);
        let n3 = open_md(&mut tm, "c", "C");
        assert!(n3.0 > n2.0 && n2.0 > n1.0);
    }

    #[test]
    fn close_on_unknown_id_is_noop() {
        let mut tm = TabManager::new();
        open_md(&mut tm, "a", "A");
        let len_before = tm.iter().count();
        tm.close(TabId(9999));
        assert_eq!(tm.iter().count(), len_before);
    }
}
