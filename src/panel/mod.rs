//! Bottom panel state.
//!
//! [`PanelManager`] tracks the four built-in tab labels (Terminal, Output, Problems, Logs) and
//! the active one. The panel surface is hard-coded this seed; future seeds may extend it via
//! [`crate::plugin::PluginSurface::PanelTabContent`] without touching `PanelStrip`.

use std::path::PathBuf;

mod logs;
mod problems;
mod strip;
#[cfg(not(target_arch = "wasm32"))]
mod terminal;

pub use logs::LogsView;
pub use problems::ProblemsView;
pub use strip::PanelStrip;
#[cfg(not(target_arch = "wasm32"))]
pub use terminal::TerminalsView;

#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
pub struct PanelTabId(pub &'static str);

#[derive(Clone, Debug)]
pub struct PanelTab {
    pub id: PanelTabId,
    pub title: &'static str,
}

#[derive(Clone, Debug)]
pub struct PanelManager {
    tabs: Vec<PanelTab>,
    active: PanelTabId,
}

impl Default for PanelManager {
    fn default() -> Self {
        let tabs = vec![
            PanelTab { id: PanelTabId("terminal"), title: "Terminal" },
            PanelTab { id: PanelTabId("output"), title: "Output" },
            PanelTab { id: PanelTabId("problems"), title: "Problems" },
            PanelTab { id: PanelTabId("logs"), title: "Logs" },
        ];
        Self {
            tabs,
            active: PanelTabId("logs"),
        }
    }
}

impl PanelManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn iter(&self) -> impl Iterator<Item = &PanelTab> {
        self.tabs.iter()
    }

    pub fn active(&self) -> PanelTabId {
        self.active
    }

    pub fn activate(&mut self, id: PanelTabId) {
        if self.tabs.iter().any(|t| t.id == id) {
            self.active = id;
        }
    }
}

/// Identity for a single terminal inside [`TerminalsManager`]. Wraps a
/// monotonically-increasing u64 minted by the manager so a closed +
/// reopened tab gets a fresh id (and therefore a fresh PTY session)
/// rather than rebinding to the corpse of the last shell.
#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
pub struct TerminalId(pub u64);

/// One tab in the Terminal panel: a label (the project name, or
/// "shell" when there's no project context) and the absolute cwd the
/// shell is rooted at. The PTY session itself lives in the
/// native-only `panel::terminal::SESSIONS` map keyed by [`TerminalId`].
#[derive(Clone, Debug)]
pub struct TerminalDescriptor {
    pub id: TerminalId,
    pub label: String,
    pub cwd: PathBuf,
}

/// Cross-platform descriptor store. The actual PTY plumbing is
/// native-only (see [`mod@terminal`]); this struct is just a list of
/// tabs + an active pointer, so it's safe to provide as a context
/// signal on both desktop and web builds. On web the Terminal panel
/// tab renders a "desktop-only" placeholder so the descriptors are
/// inert.
#[derive(Clone, Default, Debug)]
pub struct TerminalsManager {
    next_id: u64,
    terminals: Vec<TerminalDescriptor>,
    active: Option<TerminalId>,
}

impl TerminalsManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn iter(&self) -> impl Iterator<Item = &TerminalDescriptor> {
        self.terminals.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.terminals.is_empty()
    }

    pub fn len(&self) -> usize {
        self.terminals.len()
    }

    pub fn active(&self) -> Option<TerminalId> {
        self.active
    }

    pub fn descriptor(&self, id: TerminalId) -> Option<&TerminalDescriptor> {
        self.terminals.iter().find(|t| t.id == id)
    }

    pub fn activate(&mut self, id: TerminalId) {
        if self.terminals.iter().any(|t| t.id == id) {
            self.active = Some(id);
        }
    }

    /// Reuse an existing tab with the same (label, cwd) pair if one
    /// exists — picking "Open terminal" twice on the same project
    /// focuses the original tab instead of stacking duplicates.
    pub fn open_or_focus(&mut self, label: impl Into<String>, cwd: PathBuf) -> TerminalId {
        let label = label.into();
        if let Some(existing) = self
            .terminals
            .iter()
            .find(|t| t.label == label && t.cwd == cwd)
        {
            let id = existing.id;
            self.active = Some(id);
            return id;
        }
        self.next_id += 1;
        let id = TerminalId(self.next_id);
        self.terminals.push(TerminalDescriptor {
            id,
            label,
            cwd,
        });
        self.active = Some(id);
        id
    }

    /// Always creates a new tab — used by the "+" button so the user
    /// can keep two shells for the same cwd if they want.
    pub fn create(&mut self, label: impl Into<String>, cwd: PathBuf) -> TerminalId {
        self.next_id += 1;
        let id = TerminalId(self.next_id);
        self.terminals.push(TerminalDescriptor {
            id,
            label: label.into(),
            cwd,
        });
        self.active = Some(id);
        id
    }

    /// Removes the tab. If it was active, focus the previous sibling
    /// (or the new last tab if there was no previous). The caller is
    /// responsible for tearing down the PTY session
    /// (see `panel::terminal::kill_session`).
    pub fn close(&mut self, id: TerminalId) {
        let Some(idx) = self.terminals.iter().position(|t| t.id == id) else {
            return;
        };
        self.terminals.remove(idx);
        if self.active != Some(id) {
            return;
        }
        if self.terminals.is_empty() {
            self.active = None;
        } else {
            let new_idx = if idx == 0 { 0 } else { idx - 1 };
            self.active = self.terminals.get(new_idx).map(|t| t.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_active_is_logs() {
        assert_eq!(PanelManager::default().active(), PanelTabId("logs"));
    }

    #[test]
    fn default_lists_four_tabs_in_order() {
        let pm = PanelManager::default();
        let titles: Vec<_> = pm.iter().map(|t| t.title).collect();
        assert_eq!(titles, vec!["Terminal", "Output", "Problems", "Logs"]);
    }

    #[test]
    fn activate_switches_active() {
        let mut pm = PanelManager::default();
        pm.activate(PanelTabId("output"));
        assert_eq!(pm.active(), PanelTabId("output"));
    }

    #[test]
    fn activate_unknown_is_noop() {
        let mut pm = PanelManager::default();
        pm.activate(PanelTabId("nope"));
        assert_eq!(pm.active(), PanelTabId("logs"));
    }

    #[test]
    fn terminals_open_or_focus_dedupes_by_label_and_cwd() {
        let mut tm = TerminalsManager::new();
        let a = tm.open_or_focus("alpha", PathBuf::from("/x"));
        let b = tm.open_or_focus("alpha", PathBuf::from("/x"));
        let c = tm.open_or_focus("alpha", PathBuf::from("/y"));
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(tm.len(), 2);
        assert_eq!(tm.active(), Some(c));
    }

    #[test]
    fn terminals_create_always_makes_new() {
        let mut tm = TerminalsManager::new();
        let a = tm.create("shell", PathBuf::from("/x"));
        let b = tm.create("shell", PathBuf::from("/x"));
        assert_ne!(a, b);
        assert_eq!(tm.len(), 2);
        assert_eq!(tm.active(), Some(b));
    }

    #[test]
    fn terminals_close_active_focuses_previous() {
        let mut tm = TerminalsManager::new();
        let a = tm.create("a", PathBuf::from("/a"));
        let b = tm.create("b", PathBuf::from("/b"));
        let c = tm.create("c", PathBuf::from("/c"));
        tm.activate(b);
        tm.close(b);
        assert_eq!(tm.active(), Some(a));
        tm.close(a);
        assert_eq!(tm.active(), Some(c));
        tm.close(c);
        assert!(tm.is_empty());
        assert_eq!(tm.active(), None);
    }
}
