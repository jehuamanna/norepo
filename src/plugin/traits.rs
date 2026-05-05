//! Plugin contracts.
//!
//! Both traits are object-safe and used as `Box<dyn ...>` inside [`crate::plugin::PluginRegistry`].
//! `on_register` is a default-noop hook later phases use to wire context-provided handles
//! (e.g. registering palette commands).

use dioxus::prelude::*;

use super::context::PluginContext;
use super::manifest::{PluginManifest, PluginSurface};

/// Bitflag set declaring which editor modes a [`FormatPlugin`] supports. The shell consults
/// this to decide which mode buttons to render in the tab toolbar; modes whose flag is unset
/// are hidden, never offered.
///
/// Hand-rolled bitflag (no `bitflags!` crate dep) — three flags, never grows.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Default)]
pub struct FormatCaps(u8);

impl FormatCaps {
    pub const NONE: Self = Self(0);
    pub const VIEW: Self = Self(0b001);
    pub const EDIT: Self = Self(0b010);
    pub const LIVE_PREVIEW: Self = Self(0b100);

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

impl std::ops::BitOr for FormatCaps {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for FormatCaps {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// Renders a note of a particular format inside the main area's active tab. The format the
/// plugin claims is declared via `manifest().format_id` (open string identifier).
///
/// `render` is the View-mode entry point and must always be implemented. `render_edit` and
/// `render_live_preview` get default impls that return an "unsupported" placeholder; plugins
/// override them and bump their `capabilities()` to claim those modes.
pub trait FormatPlugin {
    fn manifest(&self) -> &PluginManifest;

    /// Capability bitflags. Defaults to `VIEW` only. Override to claim `EDIT` and / or
    /// `LIVE_PREVIEW` once the corresponding render method is implemented — the shell
    /// hides any toolbar button whose capability isn't claimed.
    fn capabilities(&self) -> FormatCaps {
        FormatCaps::VIEW
    }

    /// Render in View mode (read-only).
    fn render(&self, note_id: &str, content: &str) -> Element;

    /// Render in Edit mode. The returned element is mounted into a tab; `on_change` is
    /// invoked by the plugin's editor backend on every content mutation. Default impl
    /// renders an "unsupported" message — implementing this method without bumping
    /// `capabilities()` is harmless but useless (the toolbar won't expose Edit).
    fn render_edit(
        &self,
        _note_id: &str,
        _content: &str,
        _on_change: EventHandler<String>,
    ) -> Element {
        unsupported_mode("Edit")
    }

    /// Render in LivePreview mode (Obsidian-style inline-replace). Same on_change
    /// contract as `render_edit`. Default impl is the unsupported placeholder.
    fn render_live_preview(
        &self,
        _note_id: &str,
        _content: &str,
        _on_change: EventHandler<String>,
    ) -> Element {
        unsupported_mode("Live Preview")
    }

    fn on_register(&mut self, _ctx: &PluginContext) {}
}

/// Placeholder element returned by the default `render_edit` / `render_live_preview` impls.
/// The shell will only ever render this if a plugin's `capabilities()` claims a mode it
/// hasn't actually implemented — a contract bug we surface visibly rather than silently.
fn unsupported_mode(mode: &str) -> Element {
    let label = format!("This format does not support {mode} mode.");
    rsx! {
        div { class: "operon-main-empty operon-mode-unsupported",
            "{label}"
        }
    }
}

/// Contributes UI to one or more [`PluginSurface`]s of the Shell.
pub trait UIPlugin {
    fn manifest(&self) -> &PluginManifest;
    /// Render the contribution for `surface`. Plugins contributing to multiple surfaces
    /// dispatch internally on the argument.
    fn render(&self, surface: PluginSurface) -> Element;
    fn on_register(&mut self, _ctx: &PluginContext) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_default_is_none() {
        assert_eq!(FormatCaps::default(), FormatCaps::NONE);
    }

    #[test]
    fn caps_or_combines() {
        let c = FormatCaps::VIEW | FormatCaps::EDIT;
        assert!(c.contains(FormatCaps::VIEW));
        assert!(c.contains(FormatCaps::EDIT));
        assert!(!c.contains(FormatCaps::LIVE_PREVIEW));
    }

    #[test]
    fn caps_or_assign() {
        let mut c = FormatCaps::VIEW;
        c |= FormatCaps::LIVE_PREVIEW;
        assert!(c.contains(FormatCaps::VIEW));
        assert!(c.contains(FormatCaps::LIVE_PREVIEW));
        assert!(!c.contains(FormatCaps::EDIT));
    }

    #[test]
    fn contains_self_is_always_true() {
        assert!(FormatCaps::VIEW.contains(FormatCaps::VIEW));
        assert!(FormatCaps::NONE.contains(FormatCaps::NONE));
    }
}
