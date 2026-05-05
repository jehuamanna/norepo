//! Abyss palette.
//!
//! Source: VS Code bundled theme `abyss-color-theme.json`. Deep midnight-blue background
//! (#000C18) with high-luminance blue accents.

use super::{build_theme, BasePalette};
use crate::theme::{Theme, ThemeId, ThemeKind};

pub fn theme() -> Theme {
    build_theme(
        ThemeId::Abyss,
        ThemeKind::Dark,
        BasePalette {
            editor_bg: "#000C18",
            editor_fg: "#6688CC",
            bar_bg: "#051336",
            elevated_bg: "#1B2845",
            inactive_fg: "#384357",
            border: "#10243E",
            accent: "#22AAFF",
            selection_bg: "#770811",
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
        assert_eq!(t.id, ThemeId::Abyss);
        assert_eq!(t.kind, ThemeKind::Dark);
    }

    #[test]
    fn deep_blue_editor_background() {
        assert_eq!(theme().colors[&ThemeToken::EditorBackground], "#000C18");
    }
}
