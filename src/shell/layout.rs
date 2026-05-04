//! Layout state — region widths/heights and collapse flags.
//!
//! Replaces the prior seed's ad-hoc `--operon-side-bar-width: 0` override path. The Shell
//! root reads `LayoutState` and emits CSS variables consumed by the grid template; the
//! splitters in [`super::splitter`] mutate `LayoutState` while the user drags. Phase 4
//! adds explicit toggle buttons that flip the `*_collapsed` flags via the helpers below.

const SIDEBAR_DEFAULT: u32 = 280;
const COMPANION_DEFAULT: u32 = 320;
const PANEL_DEFAULT: u32 = 240;

const SIDEBAR_MIN: u32 = 160;
const SIDEBAR_MAX: u32 = 600;
const COMPANION_MIN: u32 = 160;
const COMPANION_MAX: u32 = 600;
const PANEL_MIN: u32 = 96;
const PANEL_MAX: u32 = 600;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayoutState {
    pub sidebar_width: u32,
    pub companion_width: u32,
    pub panel_height: u32,
    pub sidebar_collapsed: bool,
    pub companion_collapsed: bool,
    pub panel_collapsed: bool,
    pub last_sidebar_width: u32,
    pub last_companion_width: u32,
    pub last_panel_height: u32,
}

impl Default for LayoutState {
    fn default() -> Self {
        Self {
            sidebar_width: SIDEBAR_DEFAULT,
            companion_width: COMPANION_DEFAULT,
            panel_height: PANEL_DEFAULT,
            sidebar_collapsed: false,
            companion_collapsed: false,
            panel_collapsed: false,
            last_sidebar_width: SIDEBAR_DEFAULT,
            last_companion_width: COMPANION_DEFAULT,
            last_panel_height: PANEL_DEFAULT,
        }
    }
}

impl LayoutState {
    pub fn sidebar_track(&self) -> u32 {
        if self.sidebar_collapsed { 0 } else { self.sidebar_width }
    }
    pub fn companion_track(&self) -> u32 {
        if self.companion_collapsed { 0 } else { self.companion_width }
    }
    pub fn panel_track(&self) -> u32 {
        if self.panel_collapsed { 0 } else { self.panel_height }
    }

    pub fn set_sidebar_width(&mut self, px: u32) {
        self.sidebar_width = px.clamp(SIDEBAR_MIN, SIDEBAR_MAX);
    }
    pub fn set_companion_width(&mut self, px: u32) {
        self.companion_width = px.clamp(COMPANION_MIN, COMPANION_MAX);
    }
    pub fn set_panel_height(&mut self, px: u32) {
        self.panel_height = px.clamp(PANEL_MIN, PANEL_MAX);
    }

    pub fn toggle_sidebar(&mut self) {
        if self.sidebar_collapsed {
            self.sidebar_collapsed = false;
            if self.last_sidebar_width >= SIDEBAR_MIN {
                self.sidebar_width = self.last_sidebar_width;
            }
        } else {
            self.last_sidebar_width = self.sidebar_width;
            self.sidebar_collapsed = true;
        }
    }
    pub fn toggle_companion(&mut self) {
        if self.companion_collapsed {
            self.companion_collapsed = false;
            if self.last_companion_width >= COMPANION_MIN {
                self.companion_width = self.last_companion_width;
            }
        } else {
            self.last_companion_width = self.companion_width;
            self.companion_collapsed = true;
        }
    }
    pub fn toggle_panel(&mut self) {
        if self.panel_collapsed {
            self.panel_collapsed = false;
            if self.last_panel_height >= PANEL_MIN {
                self.panel_height = self.last_panel_height;
            }
        } else {
            self.last_panel_height = self.panel_height;
            self.panel_collapsed = true;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitterKind {
    Left,
    Right,
    Bottom,
}

#[derive(Clone, Copy, Debug)]
pub struct DragState {
    pub kind: SplitterKind,
    /// Pointer position at drag start: `client_coordinates().x` for Left/Right,
    /// `client_coordinates().y` for Bottom.
    pub start_pos: i32,
    pub start_size: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_seeded() {
        let s = LayoutState::default();
        assert_eq!(s.sidebar_width, SIDEBAR_DEFAULT);
        assert_eq!(s.companion_width, COMPANION_DEFAULT);
        assert_eq!(s.panel_height, PANEL_DEFAULT);
        assert!(!s.sidebar_collapsed);
        assert!(!s.companion_collapsed);
        assert!(!s.panel_collapsed);
        assert_eq!(s.last_sidebar_width, SIDEBAR_DEFAULT);
        assert_eq!(s.last_companion_width, COMPANION_DEFAULT);
        assert_eq!(s.last_panel_height, PANEL_DEFAULT);
    }

    #[test]
    fn set_sidebar_width_clamps() {
        let mut s = LayoutState::default();
        s.set_sidebar_width(50);
        assert_eq!(s.sidebar_width, SIDEBAR_MIN);
        s.set_sidebar_width(999);
        assert_eq!(s.sidebar_width, SIDEBAR_MAX);
        s.set_sidebar_width(300);
        assert_eq!(s.sidebar_width, 300);
    }

    #[test]
    fn set_companion_width_clamps() {
        let mut s = LayoutState::default();
        s.set_companion_width(50);
        assert_eq!(s.companion_width, COMPANION_MIN);
        s.set_companion_width(999);
        assert_eq!(s.companion_width, COMPANION_MAX);
    }

    #[test]
    fn set_panel_height_clamps() {
        let mut s = LayoutState::default();
        s.set_panel_height(50);
        assert_eq!(s.panel_height, PANEL_MIN);
        s.set_panel_height(999);
        assert_eq!(s.panel_height, PANEL_MAX);
    }

    #[test]
    fn toggle_sidebar_round_trip() {
        let mut s = LayoutState::default();
        s.toggle_sidebar();
        assert!(s.sidebar_collapsed);
        assert_eq!(s.last_sidebar_width, SIDEBAR_DEFAULT);
        s.toggle_sidebar();
        assert!(!s.sidebar_collapsed);
        assert_eq!(s.sidebar_width, SIDEBAR_DEFAULT);
    }

    #[test]
    fn toggle_companion_round_trip() {
        let mut s = LayoutState::default();
        s.set_companion_width(400);
        s.toggle_companion();
        assert!(s.companion_collapsed);
        assert_eq!(s.last_companion_width, 400);
        s.toggle_companion();
        assert!(!s.companion_collapsed);
        assert_eq!(s.companion_width, 400);
    }

    #[test]
    fn toggle_panel_round_trip() {
        let mut s = LayoutState::default();
        s.set_panel_height(150);
        s.toggle_panel();
        assert!(s.panel_collapsed);
        assert_eq!(s.last_panel_height, 150);
        s.toggle_panel();
        assert!(!s.panel_collapsed);
        assert_eq!(s.panel_height, 150);
    }

    #[test]
    fn track_returns_zero_when_collapsed() {
        let mut s = LayoutState::default();
        s.toggle_sidebar();
        assert_eq!(s.sidebar_track(), 0);
        s.toggle_sidebar();
        assert_eq!(s.sidebar_track(), SIDEBAR_DEFAULT);
    }

    #[test]
    fn three_way_independence() {
        let mut s = LayoutState::default();
        s.toggle_companion();
        assert!(s.companion_collapsed);
        assert!(!s.sidebar_collapsed);
        assert!(!s.panel_collapsed);
    }
}
