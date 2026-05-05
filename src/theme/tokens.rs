//! Subset of VS Code's theme color contributions used by Operon Shell.
//!
//! See <https://code.visualstudio.com/api/references/theme-color>. Each variant maps to a CSS
//! custom property name via [`ThemeToken::css_var`]; values are populated by
//! [`crate::theme::defaults`] and [`crate::theme::palettes`].

#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
pub enum ThemeToken {
    EditorBackground,
    EditorForeground,
    ActivityBarBackground,
    ActivityBarForeground,
    ActivityBarActiveBackground,
    ActivityBarActiveForeground,
    SideBarBackground,
    SideBarForeground,
    StatusBarBackground,
    StatusBarForeground,
    TabActiveBackground,
    TabInactiveBackground,
    TabActiveForeground,
    TabInactiveForeground,
    PanelBackground,
    PanelBorder,
    FocusBorder,
    WidgetShadow,
    // Phase 1 expansion — surface tokens the shell actually renders today.
    TitleBarActiveBackground,
    TitleBarActiveForeground,
    MenubarBackground,
    MenubarForeground,
    MenubarSelectionBackground,
    ActivityBarHoverBackground,
    SideBarSectionHeaderBackground,
    SideBarBorder,
    TabHoverBackground,
    TabBorder,
    TabActiveBorder,
    PanelHeaderBackground,
    CommandPaletteBackground,
    CommandPaletteForeground,
    CommandPaletteBorder,
    CommandPaletteSelectionBackground,
    DropdownBackground,
    ListActiveSelectionBackground,
    ListInactiveSelectionBackground,
    SplitterBackground,
    SplitterHoverBackground,
    ScrollbarSliderBackground,
    ScrollbarSliderHoverBackground,
    ScrollbarSliderActiveBackground,
}

impl ThemeToken {
    /// All variants in declaration order. Used for default-theme coverage and tests.
    pub const ALL: &'static [Self] = &[
        Self::EditorBackground,
        Self::EditorForeground,
        Self::ActivityBarBackground,
        Self::ActivityBarForeground,
        Self::ActivityBarActiveBackground,
        Self::ActivityBarActiveForeground,
        Self::SideBarBackground,
        Self::SideBarForeground,
        Self::StatusBarBackground,
        Self::StatusBarForeground,
        Self::TabActiveBackground,
        Self::TabInactiveBackground,
        Self::TabActiveForeground,
        Self::TabInactiveForeground,
        Self::PanelBackground,
        Self::PanelBorder,
        Self::FocusBorder,
        Self::WidgetShadow,
        Self::TitleBarActiveBackground,
        Self::TitleBarActiveForeground,
        Self::MenubarBackground,
        Self::MenubarForeground,
        Self::MenubarSelectionBackground,
        Self::ActivityBarHoverBackground,
        Self::SideBarSectionHeaderBackground,
        Self::SideBarBorder,
        Self::TabHoverBackground,
        Self::TabBorder,
        Self::TabActiveBorder,
        Self::PanelHeaderBackground,
        Self::CommandPaletteBackground,
        Self::CommandPaletteForeground,
        Self::CommandPaletteBorder,
        Self::CommandPaletteSelectionBackground,
        Self::DropdownBackground,
        Self::ListActiveSelectionBackground,
        Self::ListInactiveSelectionBackground,
        Self::SplitterBackground,
        Self::SplitterHoverBackground,
        Self::ScrollbarSliderBackground,
        Self::ScrollbarSliderHoverBackground,
        Self::ScrollbarSliderActiveBackground,
    ];

    /// CSS custom property name (including the leading `--`) for this token.
    pub const fn css_var(self) -> &'static str {
        match self {
            Self::EditorBackground => "--vscode-editor-background",
            Self::EditorForeground => "--vscode-editor-foreground",
            Self::ActivityBarBackground => "--vscode-activitybar-background",
            Self::ActivityBarForeground => "--vscode-activitybar-foreground",
            Self::ActivityBarActiveBackground => "--vscode-activitybar-activebackground",
            Self::ActivityBarActiveForeground => "--vscode-activitybar-activeforeground",
            Self::SideBarBackground => "--vscode-sidebar-background",
            Self::SideBarForeground => "--vscode-sidebar-foreground",
            Self::StatusBarBackground => "--vscode-statusbar-background",
            Self::StatusBarForeground => "--vscode-statusbar-foreground",
            Self::TabActiveBackground => "--vscode-tab-activebackground",
            Self::TabInactiveBackground => "--vscode-tab-inactivebackground",
            Self::TabActiveForeground => "--vscode-tab-activeforeground",
            Self::TabInactiveForeground => "--vscode-tab-inactiveforeground",
            Self::PanelBackground => "--vscode-panel-background",
            Self::PanelBorder => "--vscode-panel-border",
            Self::FocusBorder => "--vscode-focusborder",
            Self::WidgetShadow => "--vscode-widget-shadow",
            Self::TitleBarActiveBackground => "--vscode-titlebar-activebackground",
            Self::TitleBarActiveForeground => "--vscode-titlebar-activeforeground",
            Self::MenubarBackground => "--vscode-menubar-background",
            Self::MenubarForeground => "--vscode-menubar-foreground",
            Self::MenubarSelectionBackground => "--vscode-menubar-selectionbackground",
            Self::ActivityBarHoverBackground => "--vscode-activitybar-hoverbackground",
            Self::SideBarSectionHeaderBackground => "--vscode-sidebarsectionheader-background",
            Self::SideBarBorder => "--vscode-sidebar-border",
            Self::TabHoverBackground => "--vscode-tab-hoverbackground",
            Self::TabBorder => "--vscode-tab-border",
            Self::TabActiveBorder => "--vscode-tab-activeborder",
            Self::PanelHeaderBackground => "--vscode-panelheader-background",
            Self::CommandPaletteBackground => "--vscode-quickinput-background",
            Self::CommandPaletteForeground => "--vscode-quickinput-foreground",
            Self::CommandPaletteBorder => "--vscode-quickinput-border",
            Self::CommandPaletteSelectionBackground => {
                "--vscode-quickinputlist-focusbackground"
            }
            Self::DropdownBackground => "--vscode-dropdown-background",
            Self::ListActiveSelectionBackground => "--vscode-list-activeselectionbackground",
            Self::ListInactiveSelectionBackground => {
                "--vscode-list-inactiveselectionbackground"
            }
            Self::SplitterBackground => "--vscode-editorgroup-border",
            Self::SplitterHoverBackground => "--vscode-sash-hoverborder",
            Self::ScrollbarSliderBackground => "--vscode-scrollbarslider-background",
            Self::ScrollbarSliderHoverBackground => "--vscode-scrollbarslider-hoverbackground",
            Self::ScrollbarSliderActiveBackground => {
                "--vscode-scrollbarslider-activebackground"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn css_vars_are_unique_and_prefixed() {
        let set: HashSet<_> = ThemeToken::ALL.iter().map(|t| t.css_var()).collect();
        assert_eq!(set.len(), ThemeToken::ALL.len());
        for var in &set {
            assert!(var.starts_with("--vscode-"), "{var}");
        }
    }

    #[test]
    fn all_array_lists_every_variant_exactly_once() {
        let set: HashSet<_> = ThemeToken::ALL.iter().copied().collect();
        assert_eq!(
            set.len(),
            ThemeToken::ALL.len(),
            "ThemeToken::ALL must contain each variant exactly once"
        );
    }

    #[test]
    fn all_array_has_at_least_42_tokens() {
        // Phase 1 expanded the original 18 tokens to cover the rendered shell surface.
        // Lower bound guards against accidental removal during refactors.
        assert!(ThemeToken::ALL.len() >= 42, "got {}", ThemeToken::ALL.len());
    }
}
