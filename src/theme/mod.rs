//! Theme system for Operon Shell.
//!
//! Themes are runtime-toggleable. The active [`Theme`] is provided by the application root via
//! `use_context_provider` as a [`ThemeSignal`] (a `Signal<Theme>`); any descendant component can
//! read it or replace it. Theme values are emitted as CSS custom properties on a wrapper
//! element so switching modes is a single signal write — no recompile, no full re-render.

use dioxus::prelude::*;
use std::collections::HashMap;

pub mod defaults;
pub mod tokens;

pub use tokens::ThemeToken;

/// Which palette is active.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum ThemeMode {
    Light,
    Dark,
}

/// A complete theme: its [`ThemeMode`] and concrete color values per [`ThemeToken`].
#[derive(Clone, Debug)]
pub struct Theme {
    pub mode: ThemeMode,
    pub colors: HashMap<ThemeToken, String>,
}

impl Theme {
    /// Inline `style` value emitting every token as a CSS custom property declaration.
    pub fn css_variables(&self) -> String {
        let mut out = String::with_capacity(self.colors.len() * 48);
        for token in ThemeToken::ALL {
            if let Some(value) = self.colors.get(token) {
                out.push_str(token.css_var());
                out.push_str(": ");
                out.push_str(value);
                out.push_str("; ");
            }
        }
        out
    }

    /// String form of [`Self::mode`] suitable for the `data-theme` attribute.
    pub fn data_attr(&self) -> &'static str {
        match self.mode {
            ThemeMode::Light => "light",
            ThemeMode::Dark => "dark",
        }
    }
}

/// Convenience alias for the context-provided signal.
pub type ThemeSignal = Signal<Theme>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggling_mode_changes_a_sentinel_token() {
        let d = defaults::dark();
        let l = defaults::light();
        assert_ne!(
            d.colors[&ThemeToken::EditorBackground],
            l.colors[&ThemeToken::EditorBackground],
        );
        assert_ne!(d.data_attr(), l.data_attr());
    }
}
