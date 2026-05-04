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

/// Register all compile-time built-in plugins.
pub fn register_builtins(
    registry: &mut PluginRegistry,
    _ctx: &PluginContext,
) -> Result<(), String> {
    use crate::plugins::markdown::MarkdownNotePlugin;
    use crate::plugins::notes_explorer::NotesExplorer;
    registry.add_ui_plugin(Box::new(NotesExplorer::new()))?;
    registry.add_note_plugin(Box::new(MarkdownNotePlugin::new()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dioxus::prelude::*;

    use crate::plugin::manifest::{NoteKind, PluginManifest, PluginSurface};
    use crate::plugin::traits::{NotePlugin, UIPlugin};

    struct StubNote {
        manifest: PluginManifest,
    }
    impl NotePlugin for StubNote {
        fn manifest(&self) -> &PluginManifest {
            &self.manifest
        }
        fn render(&self, _id: &str, _content: &str) -> Element {
            rsx! { div { "stub-note" } }
        }
    }

    struct StubUi {
        manifest: PluginManifest,
    }
    impl UIPlugin for StubUi {
        fn manifest(&self) -> &PluginManifest {
            &self.manifest
        }
        fn render(&self, _surface: PluginSurface) -> Element {
            rsx! { div { "data-stub": "yes", "stub-ui" } }
        }
    }

    fn note_stub(id: &str, kind: NoteKind) -> StubNote {
        StubNote {
            manifest: PluginManifest {
                id: id.into(),
                display_name: id.into(),
                version: "0.1.0".into(),
                note_kind: Some(kind),
                surfaces: Vec::new(),
            },
        }
    }

    fn ui_stub(id: &str, surfaces: Vec<PluginSurface>) -> StubUi {
        StubUi {
            manifest: PluginManifest {
                id: id.into(),
                display_name: id.into(),
                version: "0.1.0".into(),
                note_kind: None,
                surfaces,
            },
        }
    }

    #[test]
    fn empty_registry_has_no_plugins() {
        let r = PluginRegistry::new();
        assert_eq!(r.note_plugins().count(), 0);
        assert_eq!(r.contributions(PluginSurface::ActivityBar).count(), 0);
    }

    #[test]
    fn note_plugin_for_kind_lookup() {
        let mut r = PluginRegistry::new();
        r.add_note_plugin(Box::new(note_stub("md-stub", NoteKind::Markdown)))
            .unwrap();
        r.add_note_plugin(Box::new(note_stub("img-stub", NoteKind::Image)))
            .unwrap();
        assert_eq!(
            r.note_plugin_for(&NoteKind::Markdown).unwrap().manifest().id,
            "md-stub"
        );
        assert!(r.note_plugin_for(&NoteKind::Canvas).is_none());
    }

    #[test]
    fn contributions_filters_by_surface() {
        let mut r = PluginRegistry::new();
        r.add_ui_plugin(Box::new(ui_stub("a", vec![PluginSurface::ActivityBar])))
            .unwrap();
        r.add_ui_plugin(Box::new(ui_stub(
            "p",
            vec![PluginSurface::CommandPalette],
        )))
        .unwrap();
        assert_eq!(r.contributions(PluginSurface::ActivityBar).count(), 1);
        assert_eq!(r.contributions(PluginSurface::CommandPalette).count(), 1);
        assert_eq!(r.contributions(PluginSurface::SideBarPanel).count(), 0);
    }

    #[test]
    fn duplicate_id_returns_err() {
        let mut r = PluginRegistry::new();
        r.add_ui_plugin(Box::new(ui_stub("dup", vec![PluginSurface::ActivityBar])))
            .unwrap();
        let result = r.add_ui_plugin(Box::new(ui_stub(
            "dup",
            vec![PluginSurface::CommandPalette],
        )));
        assert!(result.is_err());
        assert_eq!(r.contributions(PluginSurface::CommandPalette).count(), 0);
    }

    #[test]
    fn note_id_collides_with_ui_id() {
        let mut r = PluginRegistry::new();
        r.add_ui_plugin(Box::new(ui_stub("shared", vec![PluginSurface::ActivityBar])))
            .unwrap();
        let res = r.add_note_plugin(Box::new(note_stub("shared", NoteKind::Markdown)));
        assert!(res.is_err());
    }
}
