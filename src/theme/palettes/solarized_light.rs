//! Solarized Light palette (Ethan Schoonover).
//!
//! Source: <https://github.com/altercation/solarized>. Inverts the base ramp from
//! Solarized Dark: base3 #FDF6E3 (background), base2 #EEE8D5, etc.

use super::{build_theme, BasePalette};
use crate::theme::{Theme, ThemeId, ThemeKind};

pub fn theme() -> Theme {
    build_theme(
        ThemeId::SolarizedLight,
        ThemeKind::Light,
        BasePalette {
            editor_bg: "#FDF6E3",
            editor_fg: "#586E75",
            bar_bg: "#EEE8D5",
            elevated_bg: "#FDF6E3",
            inactive_fg: "#93A1A1",
            border: "#EEE8D5",
            accent: "#268BD2",
            selection_bg: "#268BD230",
            shadow_alpha: 0.16,
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
        assert_eq!(t.id, ThemeId::SolarizedLight);
        assert_eq!(t.kind, ThemeKind::Light);
    }

    #[test]
    fn base3_editor_background() {
        assert_eq!(theme().colors[&ThemeToken::EditorBackground], "#FDF6E3");
    }
}
