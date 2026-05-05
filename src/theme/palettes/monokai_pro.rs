//! Monokai Pro palette (Original / Classic filter).
//!
//! Source: <https://monokai.pro/>. We use the "Pro" filter (the namesake palette):
//! background #2D2A2E, foreground #FCFCFA, border/inactive #5B595C, accent #FFD866.

use super::{build_theme, BasePalette};
use crate::theme::{Theme, ThemeId, ThemeKind};

pub fn theme() -> Theme {
    build_theme(
        ThemeId::MonokaiPro,
        ThemeKind::Dark,
        BasePalette {
            editor_bg: "#2D2A2E",
            editor_fg: "#FCFCFA",
            bar_bg: "#221F22",
            elevated_bg: "#403E41",
            inactive_fg: "#727072",
            border: "#403E41",
            accent: "#FFD866",
            selection_bg: "#5B595C",
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
        assert_eq!(t.id, ThemeId::MonokaiPro);
        assert_eq!(t.kind, ThemeKind::Dark);
    }

    #[test]
    fn pro_filter_editor_background() {
        assert_eq!(theme().colors[&ThemeToken::EditorBackground], "#2D2A2E");
    }
}
