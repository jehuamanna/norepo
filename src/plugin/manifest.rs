//! Plugin metadata.
//!
//! [`PluginManifest`] identifies a plugin and declares the surfaces it contributes to. Format
//! plugins additionally carry a `format_id` (an open-string identifier) and the file
//! `extensions` they handle by default. The registry uses these declarations to filter
//! plugins for surface contributions and per-format lookups.

/// Where a [`crate::plugin::UIPlugin`] can contribute.
#[derive(Clone, Copy, Eq, PartialEq, Debug, Hash)]
pub enum PluginSurface {
    ActivityBar,
    SideBarPanel,
    StatusBarItem,
    CommandPalette,
    MainAreaTabContent,
    /// Future plugin-supplied bottom-panel tabs. Not iterated this seed; the four built-in
    /// tabs are hard-coded in `crate::panel::PanelManager::default()`.
    PanelTabContent,
}

/// Static descriptor attached to every plugin. `id` must be unique across the whole registry.
///
/// `format_id` is `Some` only on format plugins; UI-only plugins set it to `None`. The string
/// is the open identifier the registry uses to resolve which plugin renders a tab's content
/// (e.g. `"markdown"`, `"plaintext"`, `"json"`, `"richtext-tiptap"`).
///
/// `extensions` is the file-extension list the plugin claims by default — `&["md", "markdown"]`
/// for the markdown plugin. Empty for UI-only plugins.
#[derive(Clone, Debug)]
pub struct PluginManifest {
    pub id: String,
    pub display_name: String,
    pub version: String,
    pub format_id: Option<&'static str>,
    pub extensions: &'static [&'static str],
    pub surfaces: Vec<PluginSurface>,
}
