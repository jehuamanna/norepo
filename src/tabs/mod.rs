//! Tab state for the main area.
//!
//! [`TabManager`] is provided to the tree as `Signal<TabManager>` from the application root.
//! The [`crate::shell::MainArea`] reads it to decide which format plugin renders the body, and
//! [`TabStrip`] renders the visible row of tab buttons. Phases 3 onwards mutate it via
//! `tabs.write()`.

mod save_loop;
mod strip;

pub use save_loop::{SaveScheduler, DEBOUNCE_MS};
pub use strip::TabStrip;

use crate::editor::{EditorMode, EditorState};

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
    /// Active editor mode for this tab — drives which `FormatPlugin` method `MainArea`
    /// dispatches to. Defaults to `View`.
    pub mode: EditorMode,
    /// Cached editor cursor / selection / scroll. Populated by `MainArea` snapshot on
    /// mode change so re-entering Edit / LivePreview restores the user's caret in
    /// backends that share the same offset domain.
    pub editor_state: Option<EditorState>,
    /// When true, the debounced [`SaveScheduler`] short-circuits — the tab uses
    /// explicit save (button + Ctrl+S) instead of autosave. Local-Mode note tabs
    /// set this to `true`; cloud-mode tabs leave it `false` for autosave.
    pub manual_save: bool,
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
    /// active. The new tab uses the autosave scheduler (`manual_save = false`); use
    /// [`Self::open_manual_save`] for tabs that opt into explicit save.
    pub fn open(
        &mut self,
        note_id: String,
        format_id: String,
        title: String,
        content: String,
    ) -> TabId {
        self.open_inner(note_id, format_id, title, content, false, false)
    }

    /// Open a tab whose saves are gated on the user pressing the Save button or
    /// `Ctrl+S` instead of running through the debounced [`SaveScheduler`].
    /// Used by Local-Mode note tabs.
    pub fn open_manual_save(
        &mut self,
        note_id: String,
        format_id: String,
        title: String,
        content: String,
    ) -> TabId {
        self.open_inner(note_id, format_id, title, content, true, false)
    }

    /// Plans-Phase-9-monaco-desktop (rev 14): always create a new tab,
    /// even if another tab already references the same `note_id`.
    /// Used by the Local-Mode click-on-explorer-row flow when the
    /// existing tab is in View / Split mode and the user wants a
    /// fresh Edit buffer alongside.
    pub fn open_manual_save_new(
        &mut self,
        note_id: String,
        format_id: String,
        title: String,
        content: String,
    ) -> TabId {
        self.open_inner(note_id, format_id, title, content, true, true)
    }

    /// Open a fresh tab (force_new=true) with auto-save behavior.
    /// Used for kinds that should never show "Unsaved" — e.g. the
    /// cascade workflow note, where every state mutation (node
    /// drag, auto-arrange, cascade-runner flush) should hit disk
    /// through the debounced [`SaveScheduler`] without the user
    /// reaching for Ctrl+S. Manual-save remains the right choice
    /// for free-form markdown bodies; this is the opt-out for
    /// structured-state notes.
    pub fn open_auto_save_new(
        &mut self,
        note_id: String,
        format_id: String,
        title: String,
        content: String,
    ) -> TabId {
        self.open_inner(note_id, format_id, title, content, false, true)
    }

    fn open_inner(
        &mut self,
        note_id: String,
        format_id: String,
        title: String,
        content: String,
        manual_save: bool,
        force_new: bool,
    ) -> TabId {
        if !force_new {
            if let Some(id) = self
                .tabs
                .iter()
                .find(|t| t.note_id == note_id)
                .map(|t| t.id)
            {
                self.active = Some(id);
                return id;
            }
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
            mode: EditorMode::default(),
            editor_state: None,
            manual_save,
        });
        self.active = Some(id);
        id
    }

    /// Look up a tab by id. Used by the SaveScheduler short-circuit and the
    /// LocalShell save handler.
    pub fn get(&self, id: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == id)
    }

    /// Set the editor mode for `id`. No-op if the tab doesn't exist.
    pub fn set_mode(&mut self, id: TabId, mode: EditorMode) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
            t.mode = mode;
        }
    }

    /// Snapshot the current editor cursor/selection/scroll for `id`. No-op if the tab
    /// doesn't exist.
    pub fn set_editor_state(&mut self, id: TabId, state: Option<EditorState>) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
            t.editor_state = state;
        }
    }

    /// Update the content for `id` and flip `dirty` to true. No-op if the tab doesn't exist.
    pub fn set_content(&mut self, id: TabId, content: String) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
            t.content = content;
            t.dirty = true;
        }
    }

    /// Replace the buffer for `id` with `content` and clear the dirty
    /// flag. Used by the filesystem watcher when an external write
    /// (typically Claude's `Write`/`Edit` tool against a referenced
    /// note) lands on disk: the buffer is now in sync with the file,
    /// so flipping `dirty` to false matches reality. Caller is
    /// expected to skip the call when `content` already matches the
    /// existing buffer to avoid spurious re-renders.
    pub fn reload_content(&mut self, id: TabId, content: String) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
            t.content = content;
            t.dirty = false;
        }
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

    /// Close every tab whose position is to the right of `id`. The
    /// pivot tab itself is preserved. No-op when `id` is not open or
    /// is already the rightmost tab. The active tab is reassigned to
    /// the pivot when it would otherwise be lost.
    pub fn close_to_right(&mut self, id: TabId) {
        let Some(pivot) = self.tabs.iter().position(|t| t.id == id) else {
            return;
        };
        // Truncate everything past the pivot. `truncate` preserves the
        // first `pivot + 1` elements (the pivot included).
        self.tabs.truncate(pivot + 1);
        // If the active tab was among the closed ones, fall back to
        // the pivot.
        if let Some(active) = self.active {
            if !self.tabs.iter().any(|t| t.id == active) {
                self.active = Some(id);
            }
        }
    }

    /// Close every tab. Active resets to None.
    pub fn close_all(&mut self) {
        self.tabs.clear();
        self.active = None;
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
        self.active
            .and_then(|id| self.tabs.iter().find(|t| t.id == id))
    }

    pub fn active_id(&self) -> Option<TabId> {
        self.active
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// Activate the tab whose 0-based index in the strip matches `index`.
    /// No-op when out of bounds.
    pub fn activate_index(&mut self, index: usize) {
        if let Some(t) = self.tabs.get(index) {
            self.active = Some(t.id);
        }
    }

    /// Activate the tab to the right of the active one. Wraps to the first
    /// tab when the active tab is the last one. No-op when empty or when
    /// no tab is active.
    pub fn activate_next(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        let cur = self
            .active
            .and_then(|id| self.tabs.iter().position(|t| t.id == id))
            .unwrap_or(0);
        let next = (cur + 1) % self.tabs.len();
        self.active = Some(self.tabs[next].id);
    }

    /// Reorder `from_id` next to `to_id`. `place_before == true` puts it
    /// at the slot occupied by `to_id` (pushing the target one slot to
    /// the right); `place_before == false` puts it after the target. The
    /// active tab id is preserved across the move. No-op if either id is
    /// missing or `from_id == to_id`.
    pub fn reorder(&mut self, from_id: TabId, to_id: TabId, place_before: bool) {
        if from_id == to_id {
            return;
        }
        let Some(from_idx) = self.tabs.iter().position(|t| t.id == from_id) else {
            return;
        };
        let tab = self.tabs.remove(from_idx);
        let Some(mut to_idx) = self.tabs.iter().position(|t| t.id == to_id) else {
            // Target disappeared between the drag and the drop — restore.
            self.tabs.insert(from_idx.min(self.tabs.len()), tab);
            return;
        };
        if !place_before {
            to_idx += 1;
        }
        let dest = to_idx.min(self.tabs.len());
        self.tabs.insert(dest, tab);
    }

    /// Activate the tab to the left of the active one. Wraps to the last
    /// tab when the active tab is the first one.
    pub fn activate_prev(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        let cur = self
            .active
            .and_then(|id| self.tabs.iter().position(|t| t.id == id))
            .unwrap_or(0);
        let prev = if cur == 0 { self.tabs.len() - 1 } else { cur - 1 };
        self.active = Some(self.tabs[prev].id);
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

    #[test]
    fn close_to_right_keeps_pivot_drops_rest() {
        let mut tm = TabManager::new();
        let n1 = open_md(&mut tm, "n1", "T1");
        let _n2 = open_md(&mut tm, "n2", "T2");
        let _n3 = open_md(&mut tm, "n3", "T3");
        tm.close_to_right(n1);
        assert_eq!(tm.iter().count(), 1);
        assert_eq!(tm.active_id(), Some(n1));
    }

    #[test]
    fn close_to_right_on_rightmost_tab_is_noop() {
        let mut tm = TabManager::new();
        let _n1 = open_md(&mut tm, "n1", "T1");
        let n2 = open_md(&mut tm, "n2", "T2");
        let len_before = tm.iter().count();
        tm.close_to_right(n2);
        assert_eq!(tm.iter().count(), len_before);
    }

    #[test]
    fn close_to_right_unknown_id_is_noop() {
        let mut tm = TabManager::new();
        let _n1 = open_md(&mut tm, "n1", "T1");
        let len_before = tm.iter().count();
        tm.close_to_right(TabId(9999));
        assert_eq!(tm.iter().count(), len_before);
    }

    #[test]
    fn close_all_empties_strip_and_clears_active() {
        let mut tm = TabManager::new();
        open_md(&mut tm, "n1", "T1");
        open_md(&mut tm, "n2", "T2");
        tm.close_all();
        assert_eq!(tm.iter().count(), 0);
        assert!(tm.active_id().is_none());
    }

    #[test]
    fn open_defaults_manual_save_to_false() {
        let mut tm = TabManager::new();
        let id = open_md(&mut tm, "n1", "T");
        let t = tm.get(id).unwrap();
        assert!(!t.manual_save);
    }

    #[test]
    fn activate_next_wraps_around() {
        let mut tm = TabManager::new();
        let n1 = open_md(&mut tm, "n1", "T1");
        let n2 = open_md(&mut tm, "n2", "T2");
        tm.activate(n1);
        tm.activate_next();
        assert_eq!(tm.active_id(), Some(n2));
        tm.activate_next();
        assert_eq!(tm.active_id(), Some(n1));
    }

    #[test]
    fn activate_prev_wraps_around() {
        let mut tm = TabManager::new();
        let n1 = open_md(&mut tm, "n1", "T1");
        let n2 = open_md(&mut tm, "n2", "T2");
        tm.activate(n1);
        tm.activate_prev();
        assert_eq!(tm.active_id(), Some(n2));
        tm.activate_prev();
        assert_eq!(tm.active_id(), Some(n1));
    }

    #[test]
    fn reorder_moves_before_target() {
        let mut tm = TabManager::new();
        let n1 = open_md(&mut tm, "n1", "T1");
        let n2 = open_md(&mut tm, "n2", "T2");
        let n3 = open_md(&mut tm, "n3", "T3");
        tm.reorder(n3, n1, true);
        let order: Vec<_> = tm.iter().map(|t| t.id).collect();
        assert_eq!(order, vec![n3, n1, n2]);
    }

    #[test]
    fn reorder_moves_after_target() {
        let mut tm = TabManager::new();
        let n1 = open_md(&mut tm, "n1", "T1");
        let n2 = open_md(&mut tm, "n2", "T2");
        let n3 = open_md(&mut tm, "n3", "T3");
        tm.reorder(n1, n3, false);
        let order: Vec<_> = tm.iter().map(|t| t.id).collect();
        assert_eq!(order, vec![n2, n3, n1]);
    }

    #[test]
    fn reorder_preserves_active_id() {
        let mut tm = TabManager::new();
        let n1 = open_md(&mut tm, "n1", "T1");
        let n2 = open_md(&mut tm, "n2", "T2");
        tm.activate(n1);
        tm.reorder(n1, n2, false);
        assert_eq!(tm.active_id(), Some(n1));
    }

    #[test]
    fn reorder_self_is_noop() {
        let mut tm = TabManager::new();
        let n1 = open_md(&mut tm, "n1", "T1");
        let n2 = open_md(&mut tm, "n2", "T2");
        tm.reorder(n1, n1, true);
        let order: Vec<_> = tm.iter().map(|t| t.id).collect();
        assert_eq!(order, vec![n1, n2]);
    }

    #[test]
    fn reorder_unknown_target_keeps_order() {
        let mut tm = TabManager::new();
        let n1 = open_md(&mut tm, "n1", "T1");
        let n2 = open_md(&mut tm, "n2", "T2");
        tm.reorder(n1, TabId(9999), false);
        let order: Vec<_> = tm.iter().map(|t| t.id).collect();
        assert_eq!(order, vec![n1, n2]);
    }

    #[test]
    fn activate_index_clamps_silently() {
        let mut tm = TabManager::new();
        let n1 = open_md(&mut tm, "n1", "T1");
        tm.activate_index(99);
        assert_eq!(tm.active_id(), Some(n1));
    }

    #[test]
    fn open_manual_save_sets_flag_true() {
        let mut tm = TabManager::new();
        let id = tm.open_manual_save("n1".into(), "markdown".into(), "T".into(), String::new());
        let t = tm.get(id).unwrap();
        assert!(t.manual_save);
    }
}
