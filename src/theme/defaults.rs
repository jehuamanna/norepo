//! Default light and dark themes.
//!
//! Values are close approximations of VS Code's "Default Light Modern" and "Default Dark Modern"
//! palettes — sufficient for Operon Shell's seed scope. The CSS custom property names emitted
//! by these themes are defined in [`super::tokens`].

use std::collections::HashMap;

use super::{Theme, ThemeMode, ThemeToken};

/// Construct the canonical dark theme.
pub fn dark() -> Theme {
    let mut c = HashMap::new();
    c.insert(ThemeToken::EditorBackground, "#1F1F1F".into());
    c.insert(ThemeToken::EditorForeground, "#CCCCCC".into());
    c.insert(ThemeToken::ActivityBarBackground, "#181818".into());
    c.insert(ThemeToken::ActivityBarForeground, "#FFFFFF".into());
    c.insert(ThemeToken::ActivityBarActiveBackground, "#1F1F1F".into());
    c.insert(ThemeToken::ActivityBarActiveForeground, "#FFFFFF".into());
    c.insert(ThemeToken::SideBarBackground, "#181818".into());
    c.insert(ThemeToken::SideBarForeground, "#CCCCCC".into());
    c.insert(ThemeToken::StatusBarBackground, "#181818".into());
    c.insert(ThemeToken::StatusBarForeground, "#CCCCCC".into());
    c.insert(ThemeToken::TabActiveBackground, "#1F1F1F".into());
    c.insert(ThemeToken::TabInactiveBackground, "#181818".into());
    c.insert(ThemeToken::TabActiveForeground, "#FFFFFF".into());
    c.insert(ThemeToken::TabInactiveForeground, "#9D9D9D".into());
    c.insert(ThemeToken::PanelBackground, "#181818".into());
    c.insert(ThemeToken::PanelBorder, "#2B2B2B".into());
    c.insert(ThemeToken::FocusBorder, "#0078D4".into());
    c.insert(ThemeToken::WidgetShadow, "rgba(0,0,0,0.36)".into());
    Theme { mode: ThemeMode::Dark, colors: c }
}

/// Construct the canonical light theme.
pub fn light() -> Theme {
    let mut c = HashMap::new();
    c.insert(ThemeToken::EditorBackground, "#FFFFFF".into());
    c.insert(ThemeToken::EditorForeground, "#3B3B3B".into());
    c.insert(ThemeToken::ActivityBarBackground, "#F8F8F8".into());
    c.insert(ThemeToken::ActivityBarForeground, "#1F1F1F".into());
    c.insert(ThemeToken::ActivityBarActiveBackground, "#FFFFFF".into());
    c.insert(ThemeToken::ActivityBarActiveForeground, "#1F1F1F".into());
    c.insert(ThemeToken::SideBarBackground, "#F8F8F8".into());
    c.insert(ThemeToken::SideBarForeground, "#3B3B3B".into());
    c.insert(ThemeToken::StatusBarBackground, "#F8F8F8".into());
    c.insert(ThemeToken::StatusBarForeground, "#3B3B3B".into());
    c.insert(ThemeToken::TabActiveBackground, "#FFFFFF".into());
    c.insert(ThemeToken::TabInactiveBackground, "#F8F8F8".into());
    c.insert(ThemeToken::TabActiveForeground, "#1F1F1F".into());
    c.insert(ThemeToken::TabInactiveForeground, "#6F6F6F".into());
    c.insert(ThemeToken::PanelBackground, "#F8F8F8".into());
    c.insert(ThemeToken::PanelBorder, "#E5E5E5".into());
    c.insert(ThemeToken::FocusBorder, "#005FB8".into());
    c.insert(ThemeToken::WidgetShadow, "rgba(0,0,0,0.16)".into());
    Theme { mode: ThemeMode::Light, colors: c }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_covers_every_token() {
        let t = dark();
        for token in ThemeToken::ALL {
            assert!(t.colors.contains_key(token), "missing {token:?}");
            assert!(!t.colors[token].is_empty());
        }
    }

    #[test]
    fn light_covers_every_token() {
        let t = light();
        for token in ThemeToken::ALL {
            assert!(t.colors.contains_key(token), "missing {token:?}");
            assert!(!t.colors[token].is_empty());
        }
    }

    #[test]
    fn dark_and_light_differ_on_editor_background() {
        assert_ne!(
            dark().colors[&ThemeToken::EditorBackground],
            light().colors[&ThemeToken::EditorBackground],
        );
    }
}
