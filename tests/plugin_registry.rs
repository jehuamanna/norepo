//! Integration tests for the plugin registry.
//!
//! Proves that trait-object plugins compose with Dioxus's `Element` return type and that
//! a single struct can register against both [`NotePlugin`] and [`UIPlugin`] surfaces. This
//! is the Phase-2 mitigation for risk R-2 in `Plans-Phase-0-architecture`.

use dioxus::prelude::*;

use operon_dioxus::plugin::{
    NoteKind, NotePlugin, PluginManifest, PluginRegistry, PluginSurface, UIPlugin,
};

/// A plugin that implements both traits — separate boxes share the same id.
struct DualPlugin {
    manifest: PluginManifest,
}

impl NotePlugin for DualPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }
    fn render(&self, _id: &str, _content: &str) -> Element {
        rsx! { div { "data-dual": "note", "dual-note" } }
    }
}

impl UIPlugin for DualPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }
    fn render(&self, surface: PluginSurface) -> Element {
        let label = match surface {
            PluginSurface::ActivityBar => "AB",
            PluginSurface::CommandPalette => "CP",
            _ => "??",
        };
        rsx! { div { "data-dual": "ui", "{label}" } }
    }
}

fn manifest() -> PluginManifest {
    PluginManifest {
        id: "dual".into(),
        display_name: "Dual".into(),
        version: "0.1.0".into(),
        note_kind: Some(NoteKind::Markdown),
        surfaces: vec![PluginSurface::ActivityBar, PluginSurface::CommandPalette],
    }
}

#[test]
fn registry_round_trip_with_mixed_plugins() {
    let mut registry = PluginRegistry::new();
    let np: Box<dyn NotePlugin> = Box::new(DualPlugin { manifest: manifest() });
    let up: Box<dyn UIPlugin> = Box::new(DualPlugin {
        manifest: PluginManifest { id: "dual-ui".into(), ..manifest() },
    });
    registry.add_note_plugin(np).unwrap();
    registry.add_ui_plugin(up).unwrap();

    assert_eq!(
        registry.note_plugin_for(&NoteKind::Markdown).unwrap().manifest().id,
        "dual"
    );
    assert_eq!(registry.contributions(PluginSurface::ActivityBar).count(), 1);
    assert_eq!(registry.contributions(PluginSurface::CommandPalette).count(), 1);
    assert!(registry.ui_by_id("dual-ui").is_some());
}

#[test]
fn trait_object_render_returns_element_for_each_surface() {
    let plugin: Box<dyn UIPlugin> = Box::new(DualPlugin { manifest: manifest() });
    // Returning Element across a trait-object boundary is the R-2 risk mitigation. If this
    // test compiles and runs, the plugin/component composition path is sound.
    let _ab: Element = plugin.render(PluginSurface::ActivityBar);
    let _cp: Element = plugin.render(PluginSurface::CommandPalette);
}

#[test]
fn note_plugin_render_returns_element() {
    let plugin: Box<dyn NotePlugin> = Box::new(DualPlugin { manifest: manifest() });
    let _e: Element = plugin.render("note-1", "# hi");
}
