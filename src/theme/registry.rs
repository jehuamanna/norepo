//! Theme registry — the single lookup from [`ThemeId`] to a fully populated [`Theme`].
//!
//! Built once at app startup and provided as `Rc<ThemeRegistry>` via Dioxus context. The
//! command palette enumerates `available()` for the picker; persistence reads the active id
//! and resolves through `get()`.
//!
//! Phase 1 ships placeholder palettes for the seven non-default ids (cloned from the dark
//! default); Phase 2 replaces them with canonical hex values per upstream sources.

use super::{defaults, Theme, ThemeId, ThemeKind};

/// Picker-row metadata. Lightweight (no colour map) — used for menu / palette enumeration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThemeDescriptor {
    pub id: ThemeId,
    pub display_name: &'static str,
    pub kind: ThemeKind,
}

/// Holds every shipped theme indexed by [`ThemeId`].
pub struct ThemeRegistry {
    themes: Vec<Theme>,
}

impl ThemeRegistry {
    /// Build the registry. One entry per [`ThemeId::ALL`] variant, in canonical order.
    pub fn new() -> Self {
        let themes = ThemeId::ALL.iter().map(|&id| build(id)).collect();
        Self { themes }
    }

    /// Picker-friendly descriptor list in `ThemeId::ALL` order.
    pub fn available(&self) -> Vec<ThemeDescriptor> {
        self.themes
            .iter()
            .map(|t| ThemeDescriptor {
                id: t.id,
                display_name: t.id.display_name(),
                kind: t.kind,
            })
            .collect()
    }

    /// Total: every `ThemeId` variant has a registered theme. Panics if invariant is broken
    /// (caught by `tests::registry_get_resolves_every_id`).
    pub fn get(&self, id: ThemeId) -> &Theme {
        self.themes
            .iter()
            .find(|t| t.id == id)
            .expect("ThemeRegistry::new() must populate every ThemeId variant")
    }
}

impl Default for ThemeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve a placeholder `Theme` for `id`. Phase 2 replaces this with palette-specific calls.
fn build(id: ThemeId) -> Theme {
    match id {
        ThemeId::VscodeDarkPlus => defaults::dark(),
        ThemeId::VscodeLightPlus => defaults::light(),
        // Placeholder palettes — Phase 2 replaces each with its canonical hex set.
        // We restamp `id` and `kind` so identity is correct even before colours are.
        other => {
            let mut base = match other.kind() {
                ThemeKind::Light => defaults::light(),
                _ => defaults::dark(),
            };
            base.id = other;
            base.kind = other.kind();
            base
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeToken;

    #[test]
    fn available_returns_nine_in_canonical_order() {
        let reg = ThemeRegistry::new();
        let available = reg.available();
        assert_eq!(available.len(), 9);
        for (i, &expected) in ThemeId::ALL.iter().enumerate() {
            assert_eq!(available[i].id, expected);
        }
    }

    #[test]
    fn descriptor_display_names_match_id_names() {
        let reg = ThemeRegistry::new();
        for d in reg.available() {
            assert_eq!(d.display_name, d.id.display_name());
        }
    }

    #[test]
    fn registry_get_resolves_every_id() {
        let reg = ThemeRegistry::new();
        for &id in ThemeId::ALL {
            let t = reg.get(id);
            assert_eq!(t.id, id);
            assert_eq!(t.kind, id.kind());
        }
    }

    #[test]
    fn every_theme_covers_every_token() {
        let reg = ThemeRegistry::new();
        for &id in ThemeId::ALL {
            let t = reg.get(id);
            for tok in ThemeToken::ALL {
                assert!(t.colors.contains_key(tok), "{id:?} missing {tok:?}");
                assert!(!t.colors[tok].is_empty(), "{id:?} empty {tok:?}");
            }
        }
    }
}
