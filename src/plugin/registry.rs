//! Plugin registry — owns boxed plugin trait objects and answers lookup queries.
//!
//! The registry is built once at app startup, populated by [`register_builtins`], and
//! provided to the rest of the tree via Dioxus context as `Rc<PluginRegistry>`.

use super::context::PluginContext;
use super::manifest::{NoteKind, PluginSurface};
use super::traits::{NotePlugin, UIPlugin};

pub struct PluginRegistry {
    note_plugins: Vec<Box<dyn NotePlugin>>,
    ui_plugins: Vec<Box<dyn UIPlugin>>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            note_plugins: Vec::new(),
            ui_plugins: Vec::new(),
        }
    }

    /// Register a note plugin. Errors if its `manifest.id` collides with any prior entry.
    pub fn add_note_plugin(&mut self, p: Box<dyn NotePlugin>) -> Result<(), String> {
        if self.has_id(&p.manifest().id) {
            return Err(format!("plugin id collision: {}", p.manifest().id));
        }
        self.note_plugins.push(p);
        Ok(())
    }

    /// Register a UI plugin. Errors on `manifest.id` collision.
    pub fn add_ui_plugin(&mut self, p: Box<dyn UIPlugin>) -> Result<(), String> {
        if self.has_id(&p.manifest().id) {
            return Err(format!("plugin id collision: {}", p.manifest().id));
        }
        self.ui_plugins.push(p);
        Ok(())
    }

    fn has_id(&self, id: &str) -> bool {
        self.note_plugins.iter().any(|p| p.manifest().id == id)
            || self.ui_plugins.iter().any(|p| p.manifest().id == id)
    }

    /// Iterate every registered note plugin.
    pub fn note_plugins(&self) -> impl Iterator<Item = &dyn NotePlugin> {
        self.note_plugins.iter().map(|b| b.as_ref())
    }

    /// Find the note plugin claiming the given [`NoteKind`], if any.
    pub fn note_plugin_for(&self, kind: &NoteKind) -> Option<&dyn NotePlugin> {
        self.note_plugins
            .iter()
            .map(|b| b.as_ref())
            .find(|p| p.manifest().note_kind.as_ref() == Some(kind))
    }

    /// Iterate every UI plugin contributing to `surface`.
    pub fn contributions(
        &self,
        surface: PluginSurface,
    ) -> impl Iterator<Item = &dyn UIPlugin> + '_ {
        self.ui_plugins
            .iter()
            .map(|b| b.as_ref())
            .filter(move |p| p.manifest().surfaces.contains(&surface))
    }

    /// Lookup any plugin (note or UI) by manifest id.
    pub fn ui_by_id(&self, id: &str) -> Option<&dyn UIPlugin> {
        self.ui_plugins
            .iter()
            .map(|b| b.as_ref())
            .find(|p| p.manifest().id == id)
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Register all compile-time built-in plugins. Empty for Phase 2; later phases populate this.
pub fn register_builtins(
    _registry: &mut PluginRegistry,
    _ctx: &PluginContext,
) -> Result<(), String> {
    Ok(())
}
