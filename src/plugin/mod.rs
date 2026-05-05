//! Plugin contracts and the registry that holds them.
//!
//! Plugins are compile-time Rust types. Two trait objects exist: [`FormatPlugin`] for note-content
//! renderers and [`UIPlugin`] for surface contributions (activity bar, side bar, status bar,
//! command palette, main-area tab content).
//!
//! The Shell builds a single [`PluginRegistry`] at startup via [`register_builtins`] and provides
//! it through Dioxus context as `Rc<PluginRegistry>`. Read-only consumers downstream call
//! [`PluginRegistry::format_plugin_for`] and [`PluginRegistry::contributions`] to render their UIs.

pub mod context;
pub mod manifest;
pub mod registry;
pub mod traits;

pub use context::PluginContext;
pub use manifest::{PluginManifest, PluginSurface};
pub use registry::{register_builtins, PluginRegistry};
pub use traits::{FormatCaps, FormatPlugin, UIPlugin};
