//! Built-in commands shipped with the Shell.
//!
//! Adds: `view.toggleTheme`, `view.toggleSideBar`, `view.closeActiveTab`,
//! `notes.openSample`, `palette.show`, `palette.showCommands`.

use dioxus::prelude::*;

use crate::commands::{Command, CommandContext, CommandRegistry, PaletteMode, PaletteState};
use crate::plugin::PluginSurface;
use crate::shell::state::ActivityItemId;
use crate::theme::{self, ThemeMode};

pub fn register_builtin_commands(reg: &mut CommandRegistry) -> Result<(), String> {
    reg.register(Command {
        id: "view.toggleTheme".into(),
        title: "Toggle Theme".into(),
        category: "View".into(),
        handler: Box::new(|ctx: &CommandContext| {
            let mut theme_signal = ctx.theme;
            let next = match theme_signal.read().mode {
                ThemeMode::Dark => theme::defaults::light(),
                ThemeMode::Light => theme::defaults::dark(),
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
            let mut active = ctx.active_activity;
            let mut last = ctx.last_active_activity;
            let cur = active.read().clone();
            if cur.is_some() {
                last.set(cur);
                active.set(None);
            } else {
                let to_restore = last.read().clone();
                let next = to_restore.or_else(|| {
                    ctx.registry
                        .contributions(PluginSurface::ActivityBar)
                        .next()
                        .map(|p| ActivityItemId(format!("{}:default", p.manifest().id)))
                });
                active.set(next);
            }
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
    fn registers_six_built_in_ids() {
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
                "view.toggleSideBar".into(),
                "view.toggleTheme".into(),
            ]
        );
    }
}
