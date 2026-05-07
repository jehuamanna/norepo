//! Command registry + palette state.
//!
//! Phase 5 introduces a [`CommandRegistry`] of `id`-keyed [`Command`]s. Built-ins are
//! registered at app start; future plugin contributions can extend the registry too. The
//! palette UI lives in [`palette`]. The fuzzy matcher driving result ranking is in [`fuzzy`].

use std::collections::HashMap;
use std::rc::Rc;

use dioxus::prelude::*;

use crate::plugin::PluginRegistry;
use crate::shell::layout::LayoutState;
use crate::shell::state::ActivityItemId;
use crate::tabs::TabManager;
use crate::theme::{Theme, ThemeId, ThemeRegistry};

pub mod builtins;
pub mod fuzzy;
pub mod palette;

pub use builtins::register_builtin_commands;
pub use palette::CommandPalette;

/// What the palette is currently filtering against.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum PaletteMode {
    Commands,
    Notes,
    Themes,
}

/// Reactive palette state, provided as `Signal<PaletteState>`.
#[derive(Clone, Debug)]
pub struct PaletteState {
    pub open: bool,
    pub mode: PaletteMode,
    pub query: String,
    pub selection: usize,
    /// Theme picker only: the theme id active at the moment the picker was opened. Set when
    /// entering [`PaletteMode::Themes`] and consulted on Escape / backdrop close to restore
    /// the prior theme. Always cleared when the palette closes.
    pub themes_original: Option<ThemeId>,
    /// Theme picker only: the id most recently applied as a live preview. Used to de-bounce
    /// repeated `theme_signal.set` calls when the focused row hasn't changed.
    pub themes_focus_cache: Option<ThemeId>,
}

impl Default for PaletteState {
    fn default() -> Self {
        Self {
            open: false,
            mode: PaletteMode::Commands,
            query: String::new(),
            selection: 0,
            themes_original: None,
            themes_focus_cache: None,
        }
    }
}

/// All handles a command handler may want at execute time.
#[derive(Clone)]
pub struct CommandContext {
    pub theme: Signal<Theme>,
    pub tabs: Signal<TabManager>,
    pub active_activity: Signal<Option<ActivityItemId>>,
    pub last_active_activity: Signal<Option<ActivityItemId>>,
    pub registry: Rc<PluginRegistry>,
    pub palette: Signal<PaletteState>,
    pub layout: Signal<LayoutState>,
    pub theme_registry: Rc<ThemeRegistry>,
    /// Visibility of the About dialog. The `help.about` command flips this
    /// to `true`; the dialog itself owns the close path.
    pub about_open: Signal<bool>,
    /// Available only when Local Mode is active. Cloud Mode leaves this `None`,
    /// so commands gated behind it (e.g. `file.saveNote`) become no-ops there.
    pub local_save: Option<crate::local_mode::LocalSaveAction>,
}

pub type CommandHandler = Box<dyn Fn(&CommandContext)>;

pub struct Command {
    pub id: String,
    pub title: String,
    pub category: String,
    pub handler: CommandHandler,
}

#[derive(Default)]
pub struct CommandRegistry {
    by_id: HashMap<String, Command>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a command. Errors if `cmd.id` already exists.
    pub fn register(&mut self, cmd: Command) -> Result<(), String> {
        if self.by_id.contains_key(&cmd.id) {
            return Err(format!("command id collision: {}", cmd.id));
        }
        self.by_id.insert(cmd.id.clone(), cmd);
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&Command> {
        self.by_id.get(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Command> {
        self.by_id.values()
    }

    /// Execute a command by id. Returns Err if it is missing.
    pub fn execute(&self, id: &str, ctx: &CommandContext) -> Result<(), String> {
        let cmd = self
            .by_id
            .get(id)
            .ok_or_else(|| format!("command not found: {id}"))?;
        (cmd.handler)(ctx);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_command(id: &str) -> Command {
        Command {
            id: id.into(),
            title: id.into(),
            category: "Test".into(),
            handler: Box::new(|_ctx| {}),
        }
    }

    #[test]
    fn register_and_iter() {
        let mut r = CommandRegistry::default();
        r.register(dummy_command("a.b")).unwrap();
        r.register(dummy_command("c.d")).unwrap();
        assert_eq!(r.iter().count(), 2);
    }

    #[test]
    fn duplicate_id_returns_err() {
        let mut r = CommandRegistry::default();
        r.register(dummy_command("a.b")).unwrap();
        let res = r.register(dummy_command("a.b"));
        assert!(res.is_err());
        assert_eq!(r.iter().count(), 1);
    }

    #[test]
    fn get_unknown_returns_none() {
        let r = CommandRegistry::default();
        assert!(r.get("nope").is_none());
    }

    #[test]
    fn get_known_returns_command() {
        let mut r = CommandRegistry::default();
        r.register(dummy_command("known.id")).unwrap();
        assert!(r.get("known.id").is_some());
    }
}
