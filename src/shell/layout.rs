//! Layout state — region widths/heights and collapse flags.
//!
//! Replaces the prior seed's ad-hoc `--operon-side-bar-width: 0` override path. The Shell
//! root reads `LayoutState` and emits CSS variables consumed by the grid template; the
//! splitters in [`super::splitter`] mutate `LayoutState` while the user drags. Phase 4
//! adds explicit toggle buttons that flip the `*_collapsed` flags via the helpers below.

use serde::{Deserialize, Serialize};

const SIDEBAR_DEFAULT: u32 = 280;
const COMPANION_DEFAULT: u32 = 320;
const PANEL_DEFAULT: u32 = 240;

const SIDEBAR_MIN: u32 = 160;
const SIDEBAR_MAX: u32 = 600;
const COMPANION_MIN: u32 = 160;
const COMPANION_MAX: u32 = u32::MAX;
const PANEL_MIN: u32 = 96;
const PANEL_MAX: u32 = 600;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
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
            // Bottom panel hidden by default — most users only need it
            // for terminal/logs investigation. The toggle button on the
            // menubar (and the splitter's snap-to-edge) brings it back
            // when needed; `last_panel_height` retains the default
            // height so the first un-collapse opens at PANEL_DEFAULT.
            panel_collapsed: true,
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

    /// Apply a target width from a live drag of the left splitter.
    ///
    /// Crossing below `SIDEBAR_MIN` snaps the side bar to collapsed; the live
    /// width is stashed in `last_sidebar_width` once per snap (further frames
    /// below MIN do not clobber it). Crossing back above MIN uncollapses and
    /// sets the new width, clamped to MAX.
    pub fn drag_sidebar(&mut self, target_px: u32) {
        if target_px < SIDEBAR_MIN {
            if !self.sidebar_collapsed {
                self.last_sidebar_width = self.sidebar_width;
                self.sidebar_collapsed = true;
            }
        } else {
            self.sidebar_width = target_px.min(SIDEBAR_MAX);
            self.sidebar_collapsed = false;
        }
    }

    pub fn drag_companion(&mut self, target_px: u32) {
        if target_px < COMPANION_MIN {
            if !self.companion_collapsed {
                self.last_companion_width = self.companion_width;
                self.companion_collapsed = true;
            }
        } else {
            self.companion_width = target_px.min(COMPANION_MAX);
            self.companion_collapsed = false;
        }
    }

    pub fn drag_panel(&mut self, target_px: u32) {
        if target_px < PANEL_MIN {
            if !self.panel_collapsed {
                self.last_panel_height = self.panel_height;
                self.panel_collapsed = true;
            }
        } else {
            self.panel_height = target_px.min(PANEL_MAX);
            self.panel_collapsed = false;
        }
    }

    /// Load persisted layout from `~/.local/share/operon/layout.json`,
    /// falling back to `Default` if the file is missing or unreadable.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load_or_default() -> Self {
        let Some(path) = persisted_path() else { return Self::default() };
        let Ok(raw) = std::fs::read_to_string(&path) else { return Self::default() };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    #[cfg(target_arch = "wasm32")]
    pub fn load_or_default() -> Self {
        Self::default()
    }

    /// Persist this layout to `~/.local/share/operon/layout.json`. Errors
    /// are swallowed — a write failure shouldn't break the UI.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn save(&self) {
        let Some(path) = persisted_path() else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn save(&self) {}
}

#[cfg(not(target_arch = "wasm32"))]
fn persisted_path() -> Option<std::path::PathBuf> {
    if let Ok(home) = std::env::var("HOME") {
        return Some(std::path::PathBuf::from(home).join(".local/share/operon/layout.json"));
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        return Some(std::path::PathBuf::from(home).join("AppData/Local/operon/layout.json"));
    }
    None
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
        // Bottom panel hidden by default.
        assert!(s.panel_collapsed);
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
        assert_eq!(s.companion_width, 999);
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
        // Default is collapsed now — first toggle expands, second
        // re-collapses. Exercise both transitions starting from the
        // default state.
        let mut s = LayoutState::default();
        assert!(s.panel_collapsed);
        s.toggle_panel();
        assert!(!s.panel_collapsed);
        // Set a new height while open, then collapse and re-open;
        // re-open should restore that height.
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
        // Start from a fully-open layout (panel default is now
        // collapsed; expand it explicitly so the test asserts that
        // toggling the companion doesn't affect the other two).
        let mut s = LayoutState::default();
        s.toggle_panel();
        assert!(!s.panel_collapsed);
        s.toggle_companion();
        assert!(s.companion_collapsed);
        assert!(!s.sidebar_collapsed);
        assert!(!s.panel_collapsed);
    }

    // --- snap-to-edge / drag-from-edge tests (TestCase-Phase-1-snap-collapsing) ---

    #[test]
    fn drag_sidebar_below_min_snaps_to_collapsed() {
        let mut s = LayoutState::default();
        s.set_sidebar_width(300);
        s.drag_sidebar(SIDEBAR_MIN.saturating_sub(1));
        assert!(s.sidebar_collapsed, "below MIN must snap to collapsed");
        assert_eq!(s.last_sidebar_width, 300, "stash live width into last_*");
        assert_eq!(s.sidebar_track(), 0, "track is zero while collapsed");
    }

    #[test]
    fn drag_sidebar_at_min_does_not_collapse() {
        let mut s = LayoutState::default();
        s.drag_sidebar(SIDEBAR_MIN);
        assert!(!s.sidebar_collapsed);
        assert_eq!(s.sidebar_width, SIDEBAR_MIN);
    }

    #[test]
    fn drag_sidebar_above_min_uncollapses_and_resizes() {
        let mut s = LayoutState::default();
        s.toggle_sidebar(); // start collapsed
        assert!(s.sidebar_collapsed);
        s.drag_sidebar(220);
        assert!(!s.sidebar_collapsed, "drag above MIN restores from collapsed");
        assert_eq!(s.sidebar_width, 220);
    }

    #[test]
    fn drag_sidebar_above_max_clamps_to_max() {
        let mut s = LayoutState::default();
        s.drag_sidebar(SIDEBAR_MAX + 500);
        assert!(!s.sidebar_collapsed);
        assert_eq!(s.sidebar_width, SIDEBAR_MAX);
    }

    #[test]
    fn drag_sidebar_while_already_collapsed_preserves_last_width() {
        let mut s = LayoutState::default();
        s.set_sidebar_width(300);
        s.drag_sidebar(0);
        assert_eq!(s.last_sidebar_width, 300);
        // Subsequent drag frames at zero must NOT overwrite last_sidebar_width to 0,
        // otherwise re-opening would lose the previous size.
        s.drag_sidebar(0);
        s.drag_sidebar(20);
        assert_eq!(s.last_sidebar_width, 300);
        assert!(s.sidebar_collapsed);
    }

    #[test]
    fn drag_companion_below_min_snaps_to_collapsed() {
        let mut s = LayoutState::default();
        s.set_companion_width(380);
        s.drag_companion(COMPANION_MIN.saturating_sub(1));
        assert!(s.companion_collapsed);
        assert_eq!(s.last_companion_width, 380);
        assert_eq!(s.companion_track(), 0);
    }

    #[test]
    fn drag_companion_above_min_uncollapses_and_resizes() {
        let mut s = LayoutState::default();
        s.toggle_companion();
        s.drag_companion(250);
        assert!(!s.companion_collapsed);
        assert_eq!(s.companion_width, 250);
    }

    #[test]
    fn drag_panel_below_min_snaps_to_collapsed() {
        let mut s = LayoutState::default();
        // Default is collapsed; expand first so the snap-to-collapse
        // path actually has work to do.
        s.toggle_panel();
        s.set_panel_height(180);
        s.drag_panel(PANEL_MIN.saturating_sub(1));
        assert!(s.panel_collapsed);
        assert_eq!(s.last_panel_height, 180);
        assert_eq!(s.panel_track(), 0);
    }

    #[test]
    fn drag_panel_above_min_uncollapses_and_resizes() {
        let mut s = LayoutState::default();
        s.toggle_panel();
        s.drag_panel(150);
        assert!(!s.panel_collapsed);
        assert_eq!(s.panel_height, 150);
    }

    #[test]
    fn drag_sidebar_round_trip_via_threshold() {
        let mut s = LayoutState::default();
        s.set_sidebar_width(320);
        // Snap to collapsed.
        s.drag_sidebar(40);
        assert!(s.sidebar_collapsed);
        assert_eq!(s.last_sidebar_width, 320);
        // Drag back across threshold from a collapsed state.
        s.drag_sidebar(260);
        assert!(!s.sidebar_collapsed);
        assert_eq!(s.sidebar_width, 260);
    }
}
