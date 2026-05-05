//! Integration coverage for the theme registry + persistence wiring.
//!
//! `theme_palettes.rs` covers per-palette colour validity. This file focuses on the
//! cross-module behaviour: emitted CSS-var strings and the LRU bookkeeping that
//! `view.toggleTheme` relies on.

use operon_dioxus::theme::persistence::{
    self, MemoryStorage, ThemeStorage, KEY_LAST_DARK, KEY_LAST_LIGHT,
};
use operon_dioxus::theme::{ThemeId, ThemeRegistry, ThemeToken};

#[test]
fn css_variables_string_contains_every_token_for_every_theme() {
    let reg = ThemeRegistry::new();
    for &id in ThemeId::ALL {
        let css = reg.get(id).css_variables();
        for tok in ThemeToken::ALL {
            assert!(
                css.contains(tok.css_var()),
                "{id:?} css_variables() missing {}",
                tok.css_var()
            );
        }
    }
}

#[test]
fn lru_toggle_alternation_after_picker_sequence() {
    // Simulate: user picks SolarizedLight, then picks Abyss, then runs view.toggleTheme.
    // The LRU should land on SolarizedLight (last light), not the hard-coded default.
    let s = MemoryStorage::new();
    persistence::record_theme_change(&s, ThemeId::SolarizedLight);
    persistence::record_theme_change(&s, ThemeId::Abyss);

    // Active is now Abyss (Dark). Toggle expects last_light.
    assert_eq!(persistence::last_light(&s), ThemeId::SolarizedLight);

    // After committing the toggle, persistence should mark SolarizedLight as last_light.
    persistence::record_theme_change(&s, ThemeId::SolarizedLight);
    assert_eq!(s.get(KEY_LAST_LIGHT).as_deref(), Some("solarized-light"));
    assert_eq!(s.get(KEY_LAST_DARK).as_deref(), Some("abyss"));

    // Toggle again — last_dark should still be Abyss (the user's most recent dark choice).
    assert_eq!(persistence::last_dark(&s), ThemeId::Abyss);
}

#[test]
fn first_run_default_falls_back_when_storage_empty() {
    let s = MemoryStorage::new();
    assert_eq!(
        persistence::resolve_initial_id(&s, true),
        ThemeId::VscodeDarkPlus
    );
    assert_eq!(
        persistence::resolve_initial_id(&s, false),
        ThemeId::VscodeLightPlus
    );
}

#[test]
fn stored_id_overrides_prefers_color_scheme() {
    let s = MemoryStorage::new();
    persistence::save_id(&s, ThemeId::Nord);
    assert_eq!(persistence::resolve_initial_id(&s, true), ThemeId::Nord);
    assert_eq!(persistence::resolve_initial_id(&s, false), ThemeId::Nord);
}

#[test]
fn registry_descriptors_share_canonical_display_names() {
    let reg = ThemeRegistry::new();
    let descriptors = reg.available();
    let names: Vec<&str> = descriptors.iter().map(|d| d.display_name).collect();
    assert_eq!(
        names,
        vec![
            "VSCode Dark+",
            "VSCode Light+",
            "Nord",
            "Monokai Pro",
            "Solarized Dark",
            "Solarized Light",
            "Abyss",
            "Kimbie Dark",
            "High Contrast Dark",
        ]
    );
}
