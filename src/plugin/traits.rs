//! Plugin contracts.
//!
//! Both traits are object-safe and used as `Box<dyn ...>` inside [`crate::plugin::PluginRegistry`].
//! `on_register` is a default-noop hook later phases use to wire context-provided handles
//! (e.g. registering palette commands).

use dioxus::prelude::*;

use super::context::PluginContext;
use super::manifest::{PluginManifest, PluginSurface};

/// Renders a note of a particular format inside the main area's active tab. The format the
/// plugin claims is declared via `manifest().format_id` (open string identifier).
pub trait FormatPlugin {
    fn manifest(&self) -> &PluginManifest;
    /// Render the note content in read-only View mode. Edit / LivePreview methods land in
    /// later phases.
    fn render(&self, note_id: &str, content: &str) -> Element;
    fn on_register(&mut self, _ctx: &PluginContext) {}
}

/// Contributes UI to one or more [`PluginSurface`]s of the Shell.
pub trait UIPlugin {
    fn manifest(&self) -> &PluginManifest;
    /// Render the contribution for `surface`. Plugins contributing to multiple surfaces
    /// dispatch internally on the argument.
    fn render(&self, surface: PluginSurface) -> Element;
    fn on_register(&mut self, _ctx: &PluginContext) {}
}
