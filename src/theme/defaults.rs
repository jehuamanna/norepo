//! Default light and dark themes (VSCode Dark+ / Light+ analogs).
//!
//! The shipped palettes live under [`crate::theme::palettes`]; this module keeps a thin facade
//! so older call sites (`theme::defaults::dark()`, `theme::defaults::light()`) continue to work.
//! Phase 2 fills the seven remaining palettes with canonical hex values.

use std::collections::HashMap;

use super::{Theme, ThemeId, ThemeKind, ThemeToken};

/// Construct the canonical dark theme (Dark+/Default Dark Modern analog).
pub fn dark() -> Theme {
    let mut c = HashMap::new();
    // Original 18 tokens — preserved hex values from the prior implementation.
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
    // Phase 1 expansion — derived from the originals.
    c.insert(ThemeToken::TitleBarActiveBackground, "#181818".into());
    c.insert(ThemeToken::TitleBarActiveForeground, "#CCCCCC".into());
    c.insert(ThemeToken::MenubarBackground, "#181818".into());
    c.insert(ThemeToken::MenubarForeground, "#CCCCCC".into());
    c.insert(ThemeToken::MenubarSelectionBackground, "#0078D433".into());
    c.insert(ThemeToken::ActivityBarHoverBackground, "#FFFFFF1A".into());
    c.insert(ThemeToken::SideBarSectionHeaderBackground, "#181818".into());
    c.insert(ThemeToken::SideBarBorder, "#2B2B2B".into());
    c.insert(ThemeToken::TabHoverBackground, "#1F1F1F".into());
    c.insert(ThemeToken::TabBorder, "#181818".into());
    c.insert(ThemeToken::TabActiveBorder, "#0078D4".into());
    c.insert(ThemeToken::PanelHeaderBackground, "#181818".into());
    c.insert(ThemeToken::CommandPaletteBackground, "#252526".into());
    c.insert(ThemeToken::CommandPaletteForeground, "#CCCCCC".into());
    c.insert(ThemeToken::CommandPaletteBorder, "#2B2B2B".into());
    c.insert(ThemeToken::CommandPaletteSelectionBackground, "#04395E".into());
    c.insert(ThemeToken::DropdownBackground, "#252526".into());
    c.insert(ThemeToken::ListActiveSelectionBackground, "#04395E".into());
    c.insert(ThemeToken::ListInactiveSelectionBackground, "#37373D".into());
    c.insert(ThemeToken::SplitterBackground, "#2B2B2B".into());
    c.insert(ThemeToken::SplitterHoverBackground, "#0078D4".into());
    c.insert(ThemeToken::ScrollbarSliderBackground, "#79797966".into());
    c.insert(ThemeToken::ScrollbarSliderHoverBackground, "#646464B3".into());
    c.insert(ThemeToken::ScrollbarSliderActiveBackground, "#BFBFBF66".into());
    Theme { id: ThemeId::VscodeDarkPlus, kind: ThemeKind::Dark, colors: c }
}

/// Construct the canonical light theme (Light+/Default Light Modern analog).
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
    c.insert(ThemeToken::TitleBarActiveBackground, "#F8F8F8".into());
    c.insert(ThemeToken::TitleBarActiveForeground, "#3B3B3B".into());
    c.insert(ThemeToken::MenubarBackground, "#F8F8F8".into());
    c.insert(ThemeToken::MenubarForeground, "#3B3B3B".into());
    c.insert(ThemeToken::MenubarSelectionBackground, "#005FB81F".into());
    c.insert(ThemeToken::ActivityBarHoverBackground, "#0000001A".into());
    c.insert(ThemeToken::SideBarSectionHeaderBackground, "#F8F8F8".into());
    c.insert(ThemeToken::SideBarBorder, "#E5E5E5".into());
    c.insert(ThemeToken::TabHoverBackground, "#FFFFFF".into());
    c.insert(ThemeToken::TabBorder, "#E5E5E5".into());
    c.insert(ThemeToken::TabActiveBorder, "#005FB8".into());
    c.insert(ThemeToken::PanelHeaderBackground, "#F8F8F8".into());
    c.insert(ThemeToken::CommandPaletteBackground, "#F8F8F8".into());
    c.insert(ThemeToken::CommandPaletteForeground, "#3B3B3B".into());
    c.insert(ThemeToken::CommandPaletteBorder, "#E5E5E5".into());
    c.insert(ThemeToken::CommandPaletteSelectionBackground, "#0060C040".into());
    c.insert(ThemeToken::DropdownBackground, "#FFFFFF".into());
    c.insert(ThemeToken::ListActiveSelectionBackground, "#0060C040".into());
    c.insert(ThemeToken::ListInactiveSelectionBackground, "#E4E6F1".into());
    c.insert(ThemeToken::SplitterBackground, "#E5E5E5".into());
    c.insert(ThemeToken::SplitterHoverBackground, "#005FB8".into());
    c.insert(ThemeToken::ScrollbarSliderBackground, "#64646466".into());
    c.insert(ThemeToken::ScrollbarSliderHoverBackground, "#646464B3".into());
    c.insert(ThemeToken::ScrollbarSliderActiveBackground, "#000000A6".into());
    Theme { id: ThemeId::VscodeLightPlus, kind: ThemeKind::Light, colors: c }
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
        assert_eq!(t.id, ThemeId::VscodeDarkPlus);
        assert_eq!(t.kind, ThemeKind::Dark);
    }

    #[test]
    fn light_covers_every_token() {
        let t = light();
        for token in ThemeToken::ALL {
            assert!(t.colors.contains_key(token), "missing {token:?}");
            assert!(!t.colors[token].is_empty());
        }
        assert_eq!(t.id, ThemeId::VscodeLightPlus);
        assert_eq!(t.kind, ThemeKind::Light);
    }

    #[test]
    fn dark_and_light_differ_on_editor_background() {
        assert_ne!(
            dark().colors[&ThemeToken::EditorBackground],
            light().colors[&ThemeToken::EditorBackground],
        );
    }
}
