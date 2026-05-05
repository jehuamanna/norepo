//! VS Code "Default Light Modern" / "Light+" palette.
//!
//! Source: VS Code default theme contributions (`extensions/theme-defaults/themes/light_modern.json`
//! upstream).

use super::{build_theme, BasePalette};
use crate::theme::{Theme, ThemeId, ThemeKind};

pub fn theme() -> Theme {
    build_theme(
        ThemeId::VscodeLightPlus,
        ThemeKind::Light,
        BasePalette {
            editor_bg: "#FFFFFF",
            editor_fg: "#3B3B3B",
            bar_bg: "#F8F8F8",
            elevated_bg: "#F8F8F8",
            inactive_fg: "#6F6F6F",
            border: "#E5E5E5",
            accent: "#005FB8",
            selection_bg: "#0060C040",
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
        assert_eq!(t.id, ThemeId::VscodeLightPlus);
        assert_eq!(t.kind, ThemeKind::Light);
    }

    #[test]
    fn editor_bg_matches_canonical() {
        assert_eq!(theme().colors[&ThemeToken::EditorBackground], "#FFFFFF");
    }
}
