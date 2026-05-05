//! Concrete theme palettes shipped with the app.
//!
//! Each palette file fills a [`BasePalette`] from canonical upstream sources (cited at the
//! top of the file) and forwards to [`build_theme`] which expands the small base set into a
//! complete map over [`crate::theme::ThemeToken`]. The expansion derivation rules are
//! documented inline below — they're the same across palettes so a refactor of derivation
//! only needs to land in one place.

pub mod derivation;

pub mod abyss;
pub mod high_contrast_dark;
pub mod kimbie_dark;
pub mod monokai_pro;
pub mod nord;
pub mod solarized_dark;
pub mod solarized_light;
pub mod vscode_dark_plus;
pub mod vscode_light_plus;

use std::collections::HashMap;

use super::{Theme, ThemeId, ThemeKind, ThemeToken};

/// Small set of canonical colours per palette. `build_theme` expands this into the full
/// 42-token Theme using the derivation rules documented below.
#[derive(Clone, Copy)]
pub struct BasePalette {
    pub editor_bg: &'static str,
    pub editor_fg: &'static str,
    /// Activity bar / sidebar / status bar / panel background — usually slightly darker than
    /// `editor_bg` for dark themes, slightly off-white for light themes.
    pub bar_bg: &'static str,
    /// Command palette / dropdown background — usually one step elevated from `bar_bg`.
    pub elevated_bg: &'static str,
    /// Tab inactive / muted-text foreground.
    pub inactive_fg: &'static str,
    /// Border / divider — panels, sidebar edge, tab borders, splitter.
    pub border: &'static str,
    /// Accent — focus border, active tab border, splitter hover, status bar.
    pub accent: &'static str,
    /// List active-selection background (a tint of `accent`).
    pub selection_bg: &'static str,
    /// Widget shadow alpha (0.36 for dark themes, 0.16 for light).
    pub shadow_alpha: f64,
}

/// Build a complete Theme by deriving the remaining tokens from a small set of canonical
/// colours. Derivation rules:
///
/// - `tab.activeBackground` = `editor_bg`
/// - `tab.inactiveBackground` = `bar_bg`
/// - `tab.activeForeground` = `editor_fg`
/// - `tab.inactiveForeground` = `inactive_fg`
/// - `tab.hoverBackground` = `editor_bg`
/// - `tab.activeBorder` = `accent`
/// - `tab.border` = `border`
/// - `panel.background` = `bar_bg`
/// - `panel.border` = `border`
/// - `panelHeader.background` = `bar_bg`
/// - `sidebar.background` / `sidebar.foreground` = `bar_bg` / `editor_fg`
/// - `sidebarSectionHeader.background` = `bar_bg`
/// - `sidebar.border` = `border`
/// - `splitter.background` = `border`
/// - `splitter.hoverBackground` = `accent`
/// - `quickInput.*` (command palette) inherits from `elevated_bg` / `editor_fg` / `border`
/// - `list.activeSelectionBackground` = `selection_bg`
/// - `list.inactiveSelectionBackground` = `derivation::lighten(border, 0.2)` for dark,
///   `darken(border, 0.05)` for light
/// - `widget.shadow` = `rgba(0,0,0,shadow_alpha)`
pub fn build_theme(id: ThemeId, kind: ThemeKind, base: BasePalette) -> Theme {
    let mut c = HashMap::new();
    let BasePalette {
        editor_bg,
        editor_fg,
        bar_bg,
        elevated_bg,
        inactive_fg,
        border,
        accent,
        selection_bg,
        shadow_alpha,
    } = base;

    c.insert(ThemeToken::EditorBackground, editor_bg.to_string());
    c.insert(ThemeToken::EditorForeground, editor_fg.to_string());

    c.insert(ThemeToken::ActivityBarBackground, bar_bg.to_string());
    c.insert(ThemeToken::ActivityBarForeground, editor_fg.to_string());
    c.insert(ThemeToken::ActivityBarActiveBackground, editor_bg.to_string());
    c.insert(ThemeToken::ActivityBarActiveForeground, editor_fg.to_string());
    let hover_alpha = if matches!(kind, ThemeKind::Light) { 0.10 } else { 0.10 };
    c.insert(
        ThemeToken::ActivityBarHoverBackground,
        derivation::with_alpha_hex(editor_fg, hover_alpha),
    );

    c.insert(ThemeToken::SideBarBackground, bar_bg.to_string());
    c.insert(ThemeToken::SideBarForeground, editor_fg.to_string());
    c.insert(ThemeToken::SideBarSectionHeaderBackground, bar_bg.to_string());
    c.insert(ThemeToken::SideBarBorder, border.to_string());

    c.insert(ThemeToken::StatusBarBackground, bar_bg.to_string());
    c.insert(ThemeToken::StatusBarForeground, editor_fg.to_string());

    c.insert(ThemeToken::TabActiveBackground, editor_bg.to_string());
    c.insert(ThemeToken::TabInactiveBackground, bar_bg.to_string());
    c.insert(ThemeToken::TabActiveForeground, editor_fg.to_string());
    c.insert(ThemeToken::TabInactiveForeground, inactive_fg.to_string());
    c.insert(ThemeToken::TabHoverBackground, editor_bg.to_string());
    c.insert(ThemeToken::TabBorder, border.to_string());
    c.insert(ThemeToken::TabActiveBorder, accent.to_string());

    c.insert(ThemeToken::PanelBackground, bar_bg.to_string());
    c.insert(ThemeToken::PanelBorder, border.to_string());
    c.insert(ThemeToken::PanelHeaderBackground, bar_bg.to_string());

    c.insert(ThemeToken::TitleBarActiveBackground, bar_bg.to_string());
    c.insert(ThemeToken::TitleBarActiveForeground, editor_fg.to_string());

    c.insert(ThemeToken::MenubarBackground, bar_bg.to_string());
    c.insert(ThemeToken::MenubarForeground, editor_fg.to_string());
    c.insert(
        ThemeToken::MenubarSelectionBackground,
        derivation::with_alpha_hex(accent, 0.20),
    );

    c.insert(ThemeToken::DropdownBackground, elevated_bg.to_string());

    c.insert(ThemeToken::CommandPaletteBackground, elevated_bg.to_string());
    c.insert(ThemeToken::CommandPaletteForeground, editor_fg.to_string());
    c.insert(ThemeToken::CommandPaletteBorder, border.to_string());
    c.insert(ThemeToken::CommandPaletteSelectionBackground, selection_bg.to_string());

    c.insert(ThemeToken::ListActiveSelectionBackground, selection_bg.to_string());
    let inactive_selection = match kind {
        ThemeKind::Light => derivation::darken(border, 0.05),
        _ => derivation::lighten(border, 0.20),
    };
    c.insert(ThemeToken::ListInactiveSelectionBackground, inactive_selection);

    c.insert(ThemeToken::SplitterBackground, border.to_string());
    c.insert(ThemeToken::SplitterHoverBackground, accent.to_string());

    c.insert(ThemeToken::FocusBorder, accent.to_string());
    c.insert(ThemeToken::WidgetShadow, derivation::rgba("#000000", shadow_alpha));

    c.insert(
        ThemeToken::ScrollbarSliderBackground,
        derivation::with_alpha_hex(editor_fg, 0.20),
    );
    c.insert(
        ThemeToken::ScrollbarSliderHoverBackground,
        derivation::with_alpha_hex(editor_fg, 0.40),
    );
    c.insert(
        ThemeToken::ScrollbarSliderActiveBackground,
        derivation::with_alpha_hex(editor_fg, 0.60),
    );

    Theme { id, kind, colors: c }
}

// Re-export each palette's `theme()` constructor at the module root so the registry can call
// `palettes::nord()` etc. uniformly.
pub use abyss::theme as abyss;
pub use high_contrast_dark::theme as high_contrast_dark;
pub use kimbie_dark::theme as kimbie_dark;
pub use monokai_pro::theme as monokai_pro;
pub use nord::theme as nord;
pub use solarized_dark::theme as solarized_dark;
pub use solarized_light::theme as solarized_light;
pub use vscode_dark_plus::theme as vscode_dark_plus;
pub use vscode_light_plus::theme as vscode_light_plus;
