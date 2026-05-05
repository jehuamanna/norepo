//! Built-in commands shipped with the Shell.
//!
//! Adds: `view.toggleTheme`, `view.toggleSideBar`, `view.closeActiveTab`,
//! `notes.openSample`, `palette.show`, `palette.showCommands`.

use dioxus::prelude::*;

use crate::commands::{Command, CommandContext, CommandRegistry, PaletteMode, PaletteState};
use crate::plugin::PluginSurface;
use crate::shell::state::ActivityItemId;
use crate::theme::{self, ThemeKind};

pub fn register_builtin_commands(reg: &mut CommandRegistry) -> Result<(), String> {
    reg.register(Command {
        id: "view.toggleTheme".into(),
        title: "Toggle Theme".into(),
        category: "View".into(),
        handler: Box::new(|ctx: &CommandContext| {
            let mut theme_signal = ctx.theme;
            let next = match theme_signal.read().kind {
                ThemeKind::Dark | ThemeKind::HighContrast => theme::defaults::light(),
                ThemeKind::Light => theme::defaults::dark(),
            };
            theme_signal.set(next);
        }),
    })?;

    reg.register(Command {
        id: "view.closeActiveTab".into(),
        title: "Close Active Tab".into(),
        category: "View".into(),
        handler: Box::new(|ctx: &CommandContext| {
            let mut tabs = ctx.tabs;
            let active = tabs.read().active_id();
            if let Some(id) = active {
                tabs.write().close(id);
            }
        }),
    })?;

    reg.register(Command {
        id: "view.toggleSideBar".into(),
        title: "Toggle Side Bar".into(),
        category: "View".into(),
        handler: Box::new(|ctx: &CommandContext| {
            let mut layout = ctx.layout;
            layout.with_mut(|s| s.toggle_sidebar());
            // If the sidebar just expanded with no active panel, pick a sensible default.
            let mut active = ctx.active_activity;
            let last = ctx.last_active_activity;
            if active.read().is_none() {
                let next = last.read().clone().or_else(|| {
                    ctx.registry
                        .contributions(PluginSurface::ActivityBar)
                        .next()
                        .map(|p| ActivityItemId(format!("{}:default", p.manifest().id)))
                });
                if let Some(id) = next {
                    active.set(Some(id));
                }
            }
        }),
    })?;

    reg.register(Command {
        id: "view.toggleCompanion".into(),
        title: "Toggle Companion".into(),
        category: "View".into(),
        handler: Box::new(|ctx: &CommandContext| {
            let mut layout = ctx.layout;
            layout.with_mut(|s| s.toggle_companion());
        }),
    })?;

    reg.register(Command {
        id: "view.togglePanel".into(),
        title: "Toggle Panel".into(),
        category: "View".into(),
        handler: Box::new(|ctx: &CommandContext| {
            let mut layout = ctx.layout;
            layout.with_mut(|s| s.toggle_panel());
        }),
    })?;

    reg.register(Command {
        id: "notes.openSample".into(),
        title: "Open Sample Note...".into(),
        category: "Notes".into(),
        handler: Box::new(|ctx: &CommandContext| {
            let mut palette = ctx.palette;
            palette.set(PaletteState {
                open: true,
                mode: PaletteMode::Notes,
                query: String::new(),
                selection: 0,
            });
        }),
    })?;

    reg.register(Command {
        id: "palette.show".into(),
        title: "Show Palette (Notes)".into(),
        category: "Palette".into(),
        handler: Box::new(|ctx: &CommandContext| {
            let mut palette = ctx.palette;
            palette.set(PaletteState {
                open: true,
                mode: PaletteMode::Notes,
                query: String::new(),
                selection: 0,
            });
        }),
    })?;

    reg.register(Command {
        id: "palette.showCommands".into(),
        title: "Show Palette (Commands)".into(),
        category: "Palette".into(),
        handler: Box::new(|ctx: &CommandContext| {
            let mut palette = ctx.palette;
            palette.set(PaletteState {
                open: true,
                mode: PaletteMode::Commands,
                query: String::new(),
                selection: 0,
            });
        }),
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_expected_built_in_ids() {
        let mut r = CommandRegistry::new();
        register_builtin_commands(&mut r).unwrap();
        let mut ids: Vec<String> = r.iter().map(|c| c.id.clone()).collect();
        ids.sort();
        assert_eq!(
            ids,
            vec![
                "notes.openSample".to_string(),
                "palette.show".into(),
                "palette.showCommands".into(),
                "view.closeActiveTab".into(),
                "view.toggleCompanion".into(),
                "view.togglePanel".into(),
                "view.toggleSideBar".into(),
                "view.toggleTheme".into(),
            ]
        );
    }
}
