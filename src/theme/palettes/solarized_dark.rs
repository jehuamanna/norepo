//! Solarized Dark palette (Ethan Schoonover).
//!
//! Source: <https://github.com/altercation/solarized> base16 palette.
//! base03 #002B36 (background), base02 #073642, base01 #586E75, base00 #657B83,
//! base0 #839496, base1 #93A1A1, base2 #EEE8D5, base3 #FDF6E3.
//! Accents: yellow #B58900, blue #268BD2.

use super::{build_theme, BasePalette};
use crate::theme::{Theme, ThemeId, ThemeKind};

pub fn theme() -> Theme {
    build_theme(
        ThemeId::SolarizedDark,
        ThemeKind::Dark,
        BasePalette {
            editor_bg: "#002B36",
            editor_fg: "#93A1A1",
            bar_bg: "#073642",
            elevated_bg: "#073642",
            inactive_fg: "#586E75",
            border: "#073642",
            accent: "#268BD2",
            selection_bg: "#586E75",
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
        assert_eq!(t.id, ThemeId::SolarizedDark);
        assert_eq!(t.kind, ThemeKind::Dark);
    }

    #[test]
    fn base03_editor_background() {
        assert_eq!(theme().colors[&ThemeToken::EditorBackground], "#002B36");
    }
}
