//! Plugin metadata.
//!
//! [`PluginManifest`] identifies a plugin and declares the surfaces it contributes to and (for
//! note plugins) the [`NoteKind`] it renders. The registry uses these declarations to filter
//! plugins for surface contributions and per-kind lookups.

/// Note kinds the application can host. Phase 6 ships the markdown renderer; later seeds add
/// the rest.
#[derive(Clone, Eq, PartialEq, Debug, Hash)]
pub enum NoteKind {
    Markdown,
    Mdx,
    Image,
    Canvas,
    Excalidraw,
}

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
#[derive(Clone, Debug)]
pub struct PluginManifest {
    pub id: String,
    pub display_name: String,
    pub version: String,
    pub note_kind: Option<NoteKind>,
    pub surfaces: Vec<PluginSurface>,
}
