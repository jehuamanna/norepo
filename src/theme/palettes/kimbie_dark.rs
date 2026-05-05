//! Kimbie Dark palette.
//!
//! Source: VS Code bundled theme `kimbie-dark-color-theme.json`. Warm-brown background
//! (#221A0F) with caramel and amber accents.

use super::{build_theme, BasePalette};
use crate::theme::{Theme, ThemeId, ThemeKind};

pub fn theme() -> Theme {
    build_theme(
        ThemeId::KimbieDark,
        ThemeKind::Dark,
        BasePalette {
            editor_bg: "#221A0F",
            editor_fg: "#D3AF86",
            bar_bg: "#362712",
            elevated_bg: "#423423",
            inactive_fg: "#A57E54",
            border: "#423423",
            accent: "#F79A32",
            selection_bg: "#7C5021",
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
        assert_eq!(t.id, ThemeId::KimbieDark);
        assert_eq!(t.kind, ThemeKind::Dark);
    }

    #[test]
    fn warm_brown_editor_background() {
        assert_eq!(theme().colors[&ThemeToken::EditorBackground], "#221A0F");
    }
}
