//! VS Code "Default Dark Modern" / "Dark+" palette.
//!
//! Source: VS Code default theme contributions (`extensions/theme-defaults/themes/dark_modern.json`
//! upstream). Hex values aligned with `theme::defaults::dark()` so existing screenshots stay
//! consistent.

use super::{build_theme, BasePalette};
use crate::theme::{Theme, ThemeId, ThemeKind};

pub fn theme() -> Theme {
    build_theme(
        ThemeId::VscodeDarkPlus,
        ThemeKind::Dark,
        BasePalette {
            editor_bg: "#1F1F1F",
            editor_fg: "#CCCCCC",
            bar_bg: "#181818",
            elevated_bg: "#252526",
            inactive_fg: "#9D9D9D",
            border: "#2B2B2B",
            accent: "#0078D4",
            selection_bg: "#04395E",
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
        assert_eq!(t.id, ThemeId::VscodeDarkPlus);
        assert_eq!(t.kind, ThemeKind::Dark);
    }

    #[test]
    fn editor_bg_matches_canonical() {
        assert_eq!(theme().colors[&ThemeToken::EditorBackground], "#1F1F1F");
    }
}
