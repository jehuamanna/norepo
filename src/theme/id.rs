//! Named identity for shipped themes plus the kind partition used by `data-theme`.
//!
//! Every shipped palette has a stable [`ThemeId`] (used for persistence + picker selection)
//! and a [`ThemeKind`] (Dark / Light / HighContrast — drives the `data-theme` attribute).

use std::fmt;
use std::str::FromStr;

/// Stable identity for every theme the app ships.
#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
pub enum ThemeId {
    VscodeDarkPlus,
    VscodeLightPlus,
    Nord,
    MonokaiPro,
    SolarizedDark,
    SolarizedLight,
    Abyss,
    KimbieDark,
    HighContrastDark,
}

/// Coarse classification used by CSS to switch behaviour (`[data-theme="dark"]`, etc.).
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum ThemeKind {
    Dark,
    Light,
    HighContrast,
}

impl ThemeId {
    /// All variants in canonical picker order. Stable across releases.
    pub const ALL: &'static [Self] = &[
        Self::VscodeDarkPlus,
        Self::VscodeLightPlus,
        Self::Nord,
        Self::MonokaiPro,
        Self::SolarizedDark,
        Self::SolarizedLight,
        Self::Abyss,
        Self::KimbieDark,
        Self::HighContrastDark,
    ];

    /// Stable, persistence-safe slug.
    pub const fn slug(self) -> &'static str {
        match self {
            Self::VscodeDarkPlus => "vscode-dark-plus",
            Self::VscodeLightPlus => "vscode-light-plus",
            Self::Nord => "nord",
            Self::MonokaiPro => "monokai-pro",
            Self::SolarizedDark => "solarized-dark",
            Self::SolarizedLight => "solarized-light",
            Self::Abyss => "abyss",
            Self::KimbieDark => "kimbie-dark",
            Self::HighContrastDark => "high-contrast-dark",
        }
    }

    /// Human-facing label shown in the picker and the View menu.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::VscodeDarkPlus => "VSCode Dark+",
            Self::VscodeLightPlus => "VSCode Light+",
            Self::Nord => "Nord",
            Self::MonokaiPro => "Monokai Pro",
            Self::SolarizedDark => "Solarized Dark",
            Self::SolarizedLight => "Solarized Light",
            Self::Abyss => "Abyss",
            Self::KimbieDark => "Kimbie Dark",
            Self::HighContrastDark => "High Contrast Dark",
        }
    }

    /// Coarse classification used by CSS / `data-theme` switching.
    pub const fn kind(self) -> ThemeKind {
        match self {
            Self::VscodeLightPlus | Self::SolarizedLight => ThemeKind::Light,
            Self::HighContrastDark => ThemeKind::HighContrast,
            _ => ThemeKind::Dark,
        }
    }
}

impl ThemeKind {
    /// `data-theme` attribute value. Stable on the wire; CSS rules key off it.
    pub const fn data_attr(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
            Self::HighContrast => "hc-dark",
        }
    }
}

impl fmt::Display for ThemeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

impl FromStr for ThemeId {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        for &id in Self::ALL {
            if id.slug() == s {
                return Ok(id);
            }
        }
        Err(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn slug_round_trips_for_every_variant() {
        for &id in ThemeId::ALL {
            assert_eq!(ThemeId::from_str(id.slug()), Ok(id));
        }
    }

    #[test]
    fn from_str_unknown_slug_errors() {
        assert!(ThemeId::from_str("nope").is_err());
        assert!(ThemeId::from_str("").is_err());
    }

    #[test]
    fn display_uses_slug() {
        assert_eq!(format!("{}", ThemeId::Nord), "nord");
        assert_eq!(format!("{}", ThemeId::HighContrastDark), "high-contrast-dark");
    }

    #[test]
    fn display_names_are_unique() {
        let set: HashSet<&str> = ThemeId::ALL.iter().map(|i| i.display_name()).collect();
        assert_eq!(set.len(), ThemeId::ALL.len());
    }

    #[test]
    fn slugs_are_unique() {
        let set: HashSet<&str> = ThemeId::ALL.iter().map(|i| i.slug()).collect();
        assert_eq!(set.len(), ThemeId::ALL.len());
    }

    #[test]
    fn kind_partitions_correctly() {
        let dark: Vec<_> = ThemeId::ALL.iter().filter(|i| i.kind() == ThemeKind::Dark).collect();
        let light: Vec<_> = ThemeId::ALL.iter().filter(|i| i.kind() == ThemeKind::Light).collect();
        let hc: Vec<_> = ThemeId::ALL
            .iter()
            .filter(|i| i.kind() == ThemeKind::HighContrast)
            .collect();
        assert_eq!(dark.len(), 6);
        assert_eq!(light.len(), 2);
        assert_eq!(hc.len(), 1);
    }

    #[test]
    fn kind_data_attr_values() {
        assert_eq!(ThemeKind::Dark.data_attr(), "dark");
        assert_eq!(ThemeKind::Light.data_attr(), "light");
        assert_eq!(ThemeKind::HighContrast.data_attr(), "hc-dark");
    }
}
