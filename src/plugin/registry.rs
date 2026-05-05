//! Plugin registry — owns boxed plugin trait objects and answers lookup queries.
//!
//! The registry is built once at app startup, populated by [`register_builtins`], and
//! provided to the rest of the tree via Dioxus context as `Rc<PluginRegistry>`. Format
//! plugins are indexed by their `format_id` string (e.g. `"markdown"`).

use super::context::PluginContext;
use super::manifest::PluginSurface;
use super::traits::{FormatPlugin, UIPlugin};

pub struct PluginRegistry {
    format_plugins: Vec<Box<dyn FormatPlugin>>,
    ui_plugins: Vec<Box<dyn UIPlugin>>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            format_plugins: Vec::new(),
            ui_plugins: Vec::new(),
        }
    }

    /// Register a format plugin. Errors if its `manifest.id` collides with any prior entry.
    pub fn add_format_plugin(&mut self, p: Box<dyn FormatPlugin>) -> Result<(), String> {
        if self.has_id(&p.manifest().id) {
            return Err(format!("plugin id collision: {}", p.manifest().id));
        }
        self.format_plugins.push(p);
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
        self.format_plugins.iter().any(|p| p.manifest().id == id)
            || self.ui_plugins.iter().any(|p| p.manifest().id == id)
    }

    /// Iterate every registered format plugin.
    pub fn format_plugins(&self) -> impl Iterator<Item = &dyn FormatPlugin> {
        self.format_plugins.iter().map(|b| b.as_ref())
    }

    /// Find the format plugin claiming the given `format_id`, if any.
    pub fn format_plugin_for(&self, format_id: &str) -> Option<&dyn FormatPlugin> {
        self.format_plugins
            .iter()
            .map(|b| b.as_ref())
            .find(|p| p.manifest().format_id == Some(format_id))
    }

    /// Find the format plugin whose `extensions` list contains the given `ext` (case-sensitive
    /// lowercase). Returns the first match.
    pub fn format_plugin_by_extension(&self, ext: &str) -> Option<&dyn FormatPlugin> {
        self.format_plugins
            .iter()
            .map(|b| b.as_ref())
            .find(|p| p.manifest().extensions.iter().any(|e| *e == ext))
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
    use crate::plugins::markdown::MarkdownFormatPlugin;
    use crate::plugins::notes_explorer::NotesExplorer;
    registry.add_ui_plugin(Box::new(NotesExplorer::new()))?;
    registry.add_format_plugin(Box::new(MarkdownFormatPlugin::new()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dioxus::prelude::*;

    use crate::plugin::manifest::{PluginManifest, PluginSurface};
    use crate::plugin::traits::{FormatPlugin, UIPlugin};

    struct StubFormat {
        manifest: PluginManifest,
    }
    impl FormatPlugin for StubFormat {
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

    fn format_stub(
        id: &str,
        format_id: &'static str,
        extensions: &'static [&'static str],
    ) -> StubFormat {
        StubFormat {
            manifest: PluginManifest {
                id: id.into(),
                display_name: id.into(),
                version: "0.1.0".into(),
                format_id: Some(format_id),
                extensions,
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
                format_id: None,
                extensions: &[],
                surfaces,
            },
        }
    }

    #[test]
    fn empty_registry_has_no_plugins() {
        let r = PluginRegistry::new();
        assert_eq!(r.format_plugins().count(), 0);
        assert_eq!(r.contributions(PluginSurface::ActivityBar).count(), 0);
    }

    #[test]
    fn format_plugin_for_lookup() {
        let mut r = PluginRegistry::new();
        r.add_format_plugin(Box::new(format_stub("md-stub", "markdown", &["md"])))
            .unwrap();
        r.add_format_plugin(Box::new(format_stub("img-stub", "image", &["png"])))
            .unwrap();
        assert_eq!(
            r.format_plugin_for("markdown").unwrap().manifest().id,
            "md-stub"
        );
        assert!(r.format_plugin_for("canvas").is_none());
    }

    #[test]
    fn format_plugin_by_extension_lookup() {
        let mut r = PluginRegistry::new();
        r.add_format_plugin(Box::new(format_stub(
            "md-stub",
            "markdown",
            &["md", "markdown"],
        )))
        .unwrap();
        assert_eq!(
            r.format_plugin_by_extension("md").unwrap().manifest().id,
            "md-stub"
        );
        assert_eq!(
            r.format_plugin_by_extension("markdown").unwrap().manifest().id,
            "md-stub"
        );
        assert!(r.format_plugin_by_extension("txt").is_none());
        assert!(r.format_plugin_by_extension("").is_none());
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
    fn format_id_collides_with_ui_id() {
        let mut r = PluginRegistry::new();
        r.add_ui_plugin(Box::new(ui_stub("shared", vec![PluginSurface::ActivityBar])))
            .unwrap();
        let res = r.add_format_plugin(Box::new(format_stub("shared", "markdown", &["md"])));
        assert!(res.is_err());
    }
}
