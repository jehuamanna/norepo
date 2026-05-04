//! Bottom panel state.
//!
//! [`PanelManager`] tracks the four built-in tab labels (Terminal, Output, Problems, Logs) and
//! the active one. The panel surface is hard-coded this seed; future seeds may extend it via
//! [`crate::plugin::PluginSurface::PanelTabContent`] without touching `PanelStrip`.

mod logs;
mod strip;

pub use logs::LogsView;
pub use strip::PanelStrip;

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
}
