//! Integration coverage for the 9 shipped palettes — registry round-trip, token coverage,
//! valid CSS colour values, unique editor backgrounds, and HC contrast.

use operon_dioxus::theme::contrast::contrast_ratio;
use operon_dioxus::theme::{ThemeId, ThemeRegistry, ThemeToken};

fn is_valid_css_color(s: &str) -> bool {
    if let Some(rest) = s.strip_prefix('#') {
        return matches!(rest.len(), 3 | 6 | 8) && rest.chars().all(|c| c.is_ascii_hexdigit());
    }
    if let Some(inner) = s.strip_prefix("rgba(").and_then(|s| s.strip_suffix(')')) {
        return inner.split(',').count() == 4;
    }
    if let Some(inner) = s.strip_prefix("rgb(").and_then(|s| s.strip_suffix(')')) {
        return inner.split(',').count() == 3;
    }
    false
}

#[test]
fn registry_round_trips_every_id() {
    let reg = ThemeRegistry::new();
    for &id in ThemeId::ALL {
        assert_eq!(reg.get(id).id, id);
        assert_eq!(reg.get(id).kind, id.kind());
    }
}

#[test]
fn every_palette_covers_every_token() {
    let reg = ThemeRegistry::new();
    for &id in ThemeId::ALL {
        let t = reg.get(id);
        for tok in ThemeToken::ALL {
            assert!(t.colors.contains_key(tok), "{id:?} missing {tok:?}");
            let value = &t.colors[tok];
            assert!(!value.is_empty(), "{id:?} empty for {tok:?}");
        }
    }
}

#[test]
fn every_palette_value_is_valid_css_colour() {
    let reg = ThemeRegistry::new();
    for &id in ThemeId::ALL {
        let t = reg.get(id);
        for tok in ThemeToken::ALL {
            let v = &t.colors[tok];
            assert!(
                is_valid_css_color(v),
                "{id:?} {tok:?} = {v:?} is not a valid CSS colour"
            );
        }
    }
}

#[test]
fn all_palettes_have_unique_editor_backgrounds() {
    use std::collections::HashSet;
    let reg = ThemeRegistry::new();
    let bgs: HashSet<&String> = ThemeId::ALL
        .iter()
        .map(|&id| &reg.get(id).colors[&ThemeToken::EditorBackground])
        .collect();
    assert_eq!(bgs.len(), 9, "duplicate EditorBackground across palettes");
}

#[test]
fn high_contrast_dark_passes_aaa_for_editor() {
    let reg = ThemeRegistry::new();
    let t = reg.get(ThemeId::HighContrastDark);
    let ratio = contrast_ratio(
        &t.colors[&ThemeToken::EditorBackground],
        &t.colors[&ThemeToken::EditorForeground],
    )
    .expect("hex parses");
    assert!(ratio >= 7.0, "HC AAA needs >= 7.0, got {ratio}");
}
