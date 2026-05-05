//! Theme system for Operon Shell.
//!
//! Themes are runtime-toggleable. The active [`Theme`] is provided by the application root via
//! `use_context_provider` as a [`ThemeSignal`] (a `Signal<Theme>`); any descendant component can
//! read it or replace it. Theme values are emitted as CSS custom properties on a wrapper
//! element so switching themes is a single signal write — no recompile, no full re-render.

use dioxus::prelude::*;
use std::collections::HashMap;

pub mod contrast;
pub mod defaults;
pub mod id;
pub mod palettes;
pub mod persistence;
pub mod registry;
pub mod tokens;

pub use id::{ThemeId, ThemeKind};
pub use registry::{ThemeDescriptor, ThemeRegistry};
pub use tokens::ThemeToken;

/// Back-compat alias retained for crates outside this module that still refer to `ThemeMode`.
/// Prefer [`ThemeKind`] directly.
pub type ThemeMode = ThemeKind;

/// A complete theme: its identity, kind, and concrete color values per [`ThemeToken`].
#[derive(Clone, Debug)]
pub struct Theme {
    pub id: ThemeId,
    pub kind: ThemeKind,
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

    /// String form of [`Self::kind`] suitable for the `data-theme` attribute.
    pub fn data_attr(&self) -> &'static str {
        self.kind.data_attr()
    }

    /// Stable slug for [`Self::id`] suitable for the `data-theme-id` attribute.
    pub fn data_id_attr(&self) -> &'static str {
        self.id.slug()
    }
}

/// Convenience alias for the context-provided signal.
pub type ThemeSignal = Signal<Theme>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggling_kind_changes_a_sentinel_token() {
        let d = defaults::dark();
        let l = defaults::light();
        assert_ne!(
            d.colors[&ThemeToken::EditorBackground],
            l.colors[&ThemeToken::EditorBackground],
        );
        assert_ne!(d.data_attr(), l.data_attr());
    }

    #[test]
    fn data_attr_distinguishes_kinds() {
        assert_eq!(ThemeKind::Dark.data_attr(), "dark");
        assert_eq!(ThemeKind::Light.data_attr(), "light");
        assert_eq!(ThemeKind::HighContrast.data_attr(), "hc-dark");
    }

    #[test]
    fn data_id_attr_returns_slug() {
        let t = defaults::dark();
        assert_eq!(t.data_id_attr(), "vscode-dark-plus");
    }
}
