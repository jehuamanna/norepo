//! High Contrast Dark palette (WCAG AAA-targeted).
//!
//! Source: VS Code bundled `hc_black-color-theme.json`. Pure-black background with
//! pure-white foreground for ≥ 7:1 contrast on editor text. Accents intentionally vivid.

use super::{build_theme, BasePalette};
use crate::theme::{Theme, ThemeId, ThemeKind};

pub fn theme() -> Theme {
    build_theme(
        ThemeId::HighContrastDark,
        ThemeKind::HighContrast,
        BasePalette {
            editor_bg: "#000000",
            editor_fg: "#FFFFFF",
            bar_bg: "#000000",
            elevated_bg: "#000000",
            inactive_fg: "#FFFFFF",
            border: "#6FC3DF",
            accent: "#F38518",
            selection_bg: "#0F4A85",
            shadow_alpha: 0.5,
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
        assert_eq!(t.id, ThemeId::HighContrastDark);
        assert_eq!(t.kind, ThemeKind::HighContrast);
    }

    #[test]
    fn editor_fg_bg_meets_aaa_contrast() {
        let t = theme();
        let bg = &t.colors[&ThemeToken::EditorBackground];
        let fg = &t.colors[&ThemeToken::EditorForeground];
        let ratio = crate::theme::contrast::contrast_ratio(bg, fg).expect("hex parses");
        assert!(ratio >= 7.0, "WCAG AAA needs ≥ 7.0, got {ratio}");
    }
}
