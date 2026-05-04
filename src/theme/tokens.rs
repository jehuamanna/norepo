//! Subset of VS Code's theme color contributions used by Operon Shell.
//!
//! See <https://code.visualstudio.com/api/references/theme-color>. Each variant maps to a CSS
//! custom property name via [`ThemeToken::css_var`]; values are populated by
//! [`crate::theme::defaults`].

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
}
