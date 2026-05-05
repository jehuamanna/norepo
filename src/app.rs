//! Application root: provides theme, tab manager, plugin registry, activity-state, command
//! registry, and palette-state contexts; loads stylesheets; mounts the [`Shell`].

use std::rc::Rc;

use dioxus::prelude::*;

use std::sync::Arc;

use crate::commands::{register_builtin_commands, CommandRegistry, PaletteState};
use crate::local_mode::{LocalShell, StartupChooser};
use crate::log::LogBuffer;
use crate::log_info;
use crate::panel::PanelManager;
use crate::persistence::{MemoryPersistence, Persistence};
use crate::plugin::{register_builtins, PluginContext, PluginRegistry};
use crate::rbag::state::{AppState, Mode};
use crate::shell::layout::{DragState, LayoutState};
use crate::shell::menubar::MenuId;
use crate::shell::state::{ActiveActivity, ActivityItemId, LastActiveActivity};
use crate::shell::Shell;
use crate::tabs::{SaveScheduler, TabManager};
use crate::theme::persistence::{self as theme_persistence, WebLocalStorage};
use crate::theme::{Theme, ThemeRegistry};

const FAVICON: Asset = asset!("/assets/favicon.ico");
const MAIN_CSS: Asset = asset!("/assets/main.css");
const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");
const THEME_CSS: Asset = asset!("/assets/theme.css");
const SHELL_CSS: Asset = asset!("/assets/shell.css");
const MARKDOWN_CSS: Asset = asset!("/assets/markdown.css");

#[component]
pub fn App() -> Element {
    let theme_registry = Rc::new(ThemeRegistry::new());
    let storage = WebLocalStorage;
    let initial_id =
        theme_persistence::resolve_initial_id(&storage, theme_persistence::prefers_dark());
    let initial = theme_registry.get(initial_id).clone();
    let theme: Signal<Theme> = use_signal(|| initial);
    use_context_provider(|| theme);
    use_context_provider(|| theme_registry.clone());

    let tabs: Signal<TabManager> = use_signal(TabManager::new);
    use_context_provider(|| tabs);

    let active: Signal<Option<ActivityItemId>> =
        use_signal(|| Some(ActivityItemId("notes-explorer:default".to_string())));
    use_context_provider(|| ActiveActivity(active));

    let last_active: Signal<Option<ActivityItemId>> = use_signal(|| None);
    use_context_provider(|| LastActiveActivity(last_active));

    let palette: Signal<PaletteState> = use_signal(PaletteState::default);
    use_context_provider(|| palette);

    let open_menu: Signal<Option<MenuId>> = use_signal(|| None);
    use_context_provider(|| open_menu);

    let panel: Signal<PanelManager> = use_signal(PanelManager::new);
    use_context_provider(|| panel);

    let layout: Signal<LayoutState> = use_signal(LayoutState::default);
    use_context_provider(|| layout);

    let drag: Signal<Option<DragState>> = use_signal(|| None);
    use_context_provider(|| drag);

    let mut log_buffer: Signal<LogBuffer> = use_signal(LogBuffer::new);
    use_context_provider(|| log_buffer);

    // Local Mode wiring: install the LocalUserRepo / LocalSettingsRepo before any
    // component (e.g. StartupChooser, LocalShell) reads them. Then resolve the
    // remembered mode from local_app_settings; if absent, AppState defaults to
    // NonLocal but we render the chooser instead of mounting a shell.
    crate::local_mode::provide_local_state();

    let mut app_state: Signal<AppState> = use_signal(AppState::default);
    use_context_provider(|| app_state);

    #[cfg(not(target_arch = "wasm32"))]
    let initial_mode_remembered: Option<Mode> = {
        let crate::local_mode::LocalSettingsRepo(settings) = use_context();
        use_hook(|| crate::local_mode::read_remembered_mode(&settings))
    };
    #[cfg(target_arch = "wasm32")]
    let initial_mode_remembered: Option<Mode> = Some(Mode::NonLocal);

    use_hook(|| {
        if let Some(m) = initial_mode_remembered {
            app_state.with_mut(|s| s.mode = m);
        }
    });

    let resolved_mode = app_state.read().mode;
    let persistence = provide_persistence(resolved_mode);
    let scheduler = SaveScheduler::new(persistence.clone());
    use_context_provider(|| persistence);
    use_context_provider(|| scheduler);

    use_context_provider(|| {
        let mut registry = PluginRegistry::new();
        let ctx = PluginContext {
            theme,
            tabs: Some(tabs),
        };
        if let Err(err) = register_builtins(&mut registry, &ctx) {
            eprintln!("operon: register_builtins failed: {err}");
        }
        Rc::new(registry)
    });

    use_context_provider(|| {
        let mut reg = CommandRegistry::new();
        if let Err(err) = register_builtin_commands(&mut reg) {
            eprintln!("operon: register_builtin_commands failed: {err}");
        }
        Rc::new(reg)
    });

    use_hook(|| {
        log_info!(log_buffer, "Operon: ready");
    });

    use_effect(move || {
        let snapshot = theme.read();
        let data = snapshot.data_attr();
        let data_id = snapshot.data_id_attr();
        let style = snapshot.css_variables();
        drop(snapshot);
        let script = format!(
            "document.documentElement.setAttribute('data-theme', '{data}');\
             document.documentElement.setAttribute('data-theme-id', '{data_id}');\
             document.documentElement.setAttribute('style', '{style}');"
        );
        document::eval(&script);
    });

    let mode_known = initial_mode_remembered.is_some();
    let current_mode = app_state.read().mode;

    rsx! {
        document::Link { rel: "icon", href: FAVICON }
        document::Stylesheet { href: MAIN_CSS }
        document::Stylesheet { href: TAILWIND_CSS }
        document::Stylesheet { href: THEME_CSS }
        document::Stylesheet { href: SHELL_CSS }
        document::Stylesheet { href: MARKDOWN_CSS }
        div {
            id: "operon-root",
            if !mode_known {
                StartupChooser {}
            } else if current_mode == Mode::Local {
                LocalShell {}
            } else {
                Shell {}
            }
        }
    }
}

/// Construct the per-platform `Persistence` for the running app. On desktop, attempts to use
/// `~/.local/share/operon/notes` (or the OS-equivalent) and falls back to `MemoryPersistence`
/// if directory creation fails. Local Mode roots persistence under a `local/` subdir so it
/// stays separate from cloud-mode synced state. On wasm, returns `MemoryPersistence` until
/// Phase 3 lands the real `WebPersistence` (OPFS first, IndexedDB fallback).
fn provide_persistence(mode: Mode) -> Arc<dyn Persistence> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use crate::persistence::FilesystemPersistence;
        let dir = match mode {
            Mode::Local => default_notes_dir().join("local"),
            Mode::NonLocal => default_notes_dir(),
        };
        match FilesystemPersistence::new(&dir) {
            Ok(p) => Arc::new(p),
            Err(e) => {
                eprintln!(
                    "operon: filesystem persistence init failed for {dir:?} ({e}); \
                     falling back to in-memory storage"
                );
                Arc::new(MemoryPersistence::new())
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = mode;
        Arc::new(MemoryPersistence::new())
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn default_notes_dir() -> std::path::PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        return std::path::PathBuf::from(home).join(".local/share/operon/notes");
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        return std::path::PathBuf::from(home).join("AppData/Local/operon/notes");
    }
    std::env::temp_dir().join("operon/notes")
}
