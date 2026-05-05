//! Nord palette.
//!
//! Source: <https://www.nordtheme.com/docs/colors-and-palettes>.
//! Polar Night: nord0 #2E3440, nord1 #3B4252, nord2 #434C5E, nord3 #4C566A.
//! Snow Storm: nord4 #D8DEE9, nord5 #E5E9F0, nord6 #ECEFF4.
//! Frost: nord7 #8FBCBB, nord8 #88C0D0, nord9 #81A1C1, nord10 #5E81AC.

use super::{build_theme, BasePalette};
use crate::theme::{Theme, ThemeId, ThemeKind};

pub fn theme() -> Theme {
    build_theme(
        ThemeId::Nord,
        ThemeKind::Dark,
        BasePalette {
            editor_bg: "#2E3440",
            editor_fg: "#D8DEE9",
            bar_bg: "#3B4252",
            elevated_bg: "#434C5E",
            inactive_fg: "#4C566A",
            border: "#3B4252",
            accent: "#88C0D0",
            selection_bg: "#434C5E",
            shadow_alpha: 0.36,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::theme;
    use crate::theme::{ThemeId, ThemeKind, ThemeToken};

    #[test]
    fn identity_and_kind() {
        let t = theme();
        assert_eq!(t.id, ThemeId::Nord);
        assert_eq!(t.kind, ThemeKind::Dark);
    }

    #[test]
    fn polar_night_editor_background() {
        assert_eq!(theme().colors[&ThemeToken::EditorBackground], "#2E3440");
    }
}
