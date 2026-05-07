//! Built-in commands shipped with the Shell.
//!
//! Adds: `view.toggleTheme`, `view.toggleSideBar`, `view.closeActiveTab`,
//! `notes.openSample`, `palette.show`, `palette.showCommands`.

use dioxus::prelude::*;

use crate::commands::{Command, CommandContext, CommandRegistry, PaletteMode, PaletteState};
use crate::plugin::PluginSurface;
use crate::shell::state::ActivityItemId;
use crate::theme::persistence::{self, WebLocalStorage};
use crate::theme::ThemeKind;

pub fn register_builtin_commands(reg: &mut CommandRegistry) -> Result<(), String> {
    reg.register(Command {
        id: "file.saveNote".into(),
        title: "Save Note".into(),
        category: "File".into(),
        handler: Box::new(|ctx: &CommandContext| {
            // Local-Mode-only: Cloud has its own debounced autosave path and
            // leaves `local_save` as None, so this is a no-op there.
            if let Some(action) = &ctx.local_save {
                action.callback.call(());
            }
        }),
    })?;

    reg.register(Command {
        id: "file.exit".into(),
        title: "Exit".into(),
        category: "File".into(),
        handler: Box::new(|_ctx: &CommandContext| {
            // On desktop, close the active window. With the default
            // `exit_on_last_window_close` config (true), closing the only
            // window terminates the process. On web/wasm builds the
            // command is a harmless no-op — browsers can't close a tab
            // they didn't open from a user gesture, and there is no
            // window to dismiss.
            #[cfg(feature = "desktop")]
            {
                dioxus::desktop::window().close();
            }
        }),
    })?;

    reg.register(Command {
        id: "view.toggleTheme".into(),
        title: "Toggle Theme".into(),
        category: "View".into(),
        handler: Box::new(|ctx: &CommandContext| {
            let mut theme_signal = ctx.theme;
            let storage = WebLocalStorage;
            let next_id = match theme_signal.read().kind {
                ThemeKind::Dark | ThemeKind::HighContrast => persistence::last_light(&storage),
                ThemeKind::Light => persistence::last_dark(&storage),
            };
            let next = ctx.theme_registry.get(next_id).clone();
            theme_signal.set(next);
            persistence::record_theme_change(&storage, next_id);
        }),
    })?;

    reg.register(Command {
        id: "workbench.action.selectTheme".into(),
        title: "Color Theme...".into(),
        category: "View".into(),
        handler: Box::new(|ctx: &CommandContext| {
            // Capture the currently active theme so Escape can revert.
            let original = ctx.theme.read().id;
            let active_idx = crate::theme::ThemeId::ALL
                .iter()
                .position(|&id| id == original)
                .unwrap_or(0);
            let mut palette = ctx.palette;
            palette.set(PaletteState {
                open: true,
                mode: PaletteMode::Themes,
                query: String::new(),
                selection: active_idx,
                themes_original: Some(original),
                themes_focus_cache: Some(original),
            });
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
                themes_original: None,
                themes_focus_cache: None,
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
                themes_original: None,
                themes_focus_cache: None,
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
                themes_original: None,
                themes_focus_cache: None,
            });
        }),
    })?;

    // `Help` menu maps to the `Palette` category (see `MenuId::category_label`),
    // so registering About here surfaces it in Help → About and in the
    // command palette under "About".
    reg.register(Command {
        id: "help.about".into(),
        title: "About".into(),
        category: "Palette".into(),
        handler: Box::new(|ctx: &CommandContext| {
            let mut about_open = ctx.about_open;
            about_open.set(true);
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
                "file.exit".to_string(),
                "file.saveNote".into(),
                "help.about".into(),
                "notes.openSample".into(),
                "palette.show".into(),
                "palette.showCommands".into(),
                "view.closeActiveTab".into(),
                "view.toggleCompanion".into(),
                "view.togglePanel".into(),
                "view.toggleSideBar".into(),
                "view.toggleTheme".into(),
                "workbench.action.selectTheme".into(),
            ]
        );
    }
}
