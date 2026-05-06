//! Application root: provides theme, tab manager, plugin registry, activity-state, command
//! registry, and palette-state contexts; loads stylesheets; mounts the [`Shell`].

use std::rc::Rc;

use dioxus::prelude::*;

use std::sync::Arc;

use crate::commands::{register_builtin_commands, CommandRegistry, PaletteState};
#[cfg(not(target_arch = "wasm32"))]
use crate::local_mode::vault::VaultRoot;
#[cfg(not(target_arch = "wasm32"))]
use crate::local_mode::VaultDirPicker;
use crate::local_mode::StartupChooser;
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
// `with_static_head(true)` makes dx-cli emit `<link rel="stylesheet">` into
// the served HTML head at template-build time, with the correct hashed path,
// so the browser fetches the CSS in parallel with WASM and chrome rules are
// applied as soon as Dioxus mounts. Without it, the link tag would only be
// added during App's first VDOM render — i.e. *after* WASM finished loading
// and rendered the chrome divs, producing a flash of unstyled content.
const MAIN_CSS: Asset = asset!(
    "/assets/main.css",
    AssetOptions::css().with_static_head(true)
);
const TAILWIND_CSS: Asset = asset!(
    "/assets/tailwind.css",
    AssetOptions::css().with_static_head(true)
);
const THEME_CSS: Asset = asset!(
    "/assets/theme.css",
    AssetOptions::css().with_static_head(true)
);
const SHELL_CSS: Asset = asset!(
    "/assets/shell.css",
    AssetOptions::css().with_static_head(true)
);
const MARKDOWN_CSS: Asset = asset!(
    "/assets/markdown.css",
    AssetOptions::css().with_static_head(true)
);

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

    // Plans-Phase-2-editor-auto-focus: app-scope signal that asks the
    // editor host to take keyboard focus after mount. Carries the note id
    // (string) of the editor that should be focused; cleared by the host
    // once it dispatches `EditorCommand::Focus`.
    let request_editor_focus: Signal<Option<String>> = use_signal(|| None);
    use_context_provider(|| crate::editor::RequestEditorFocus(request_editor_focus));

    // Local Mode wiring: install the LocalUserRepo / LocalSettingsRepo before any
    // component reads them. Then resolve the remembered mode from
    // `local_app_settings`; if absent, AppState defaults to NonLocal but we
    // render the chooser instead of mounting a shell.
    crate::local_mode::provide_local_state();

    let mut app_state: Signal<AppState> = use_signal(AppState::default);
    use_context_provider(|| app_state);

    #[cfg(not(target_arch = "wasm32"))]
    let initial_mode_remembered: Option<Mode> = {
        let crate::local_mode::LocalSettingsRepo(settings) = use_context();
        use_hook(|| crate::local_mode::read_remembered_mode(&settings))
    };
    // Plans-Phase-2-saving / Phase E: with `wasm-sqlite` on, wasm boots
    // straight into Local Mode (no Cloud RBAG path on web). Without the
    // feature, the wasm_stub shell is mounted under NonLocal as before.
    #[cfg(all(target_arch = "wasm32", feature = "wasm-sqlite"))]
    let initial_mode_remembered: Option<Mode> = Some(Mode::Local);
    #[cfg(all(target_arch = "wasm32", not(feature = "wasm-sqlite")))]
    let initial_mode_remembered: Option<Mode> = Some(Mode::NonLocal);

    // Local Mode also requires a chosen notes vault directory. On first run
    // (no `vault.root.path` setting) we render the `VaultDirPicker` modal in
    // place of the workspace until the user picks one. The vault is held in
    // App-scope state via `CurrentVaultRoot` so SettingsPanel "Change…" can
    // hot-apply a re-pick without a reload.
    #[cfg(not(target_arch = "wasm32"))]
    let mut vault_root: Signal<Option<VaultRoot>> = {
        let crate::local_mode::LocalSettingsRepo(settings) = use_context();
        use_signal(|| crate::local_mode::read_vault_root(&settings))
    };
    #[cfg(not(target_arch = "wasm32"))]
    use_context_provider(|| crate::local_mode::CurrentVaultRoot(vault_root));
    // Plans-Phase-1-vault-dir: process-lifetime lock guard for the chosen
    // vault. Picker writes here on success; App scope keeps the lock file
    // alive until the user closes Operon. On returning-user boot (vault
    // already in settings), we attempt to acquire the lock immediately so
    // a second instance pointed at the same vault is rejected.
    #[cfg(not(target_arch = "wasm32"))]
    let mut vault_lock: Signal<Option<crate::local_mode::vault::LockGuard>> =
        use_signal(|| None);
    #[cfg(not(target_arch = "wasm32"))]
    use_hook(|| {
        if let Some(root) = vault_root.read().clone() {
            match crate::local_mode::vault::acquire_lock(&root) {
                Ok(guard) => vault_lock.set(Some(guard)),
                Err(e) => eprintln!(
                    "operon: could not acquire vault lock at boot ({e}); \
                     other instance may be running."
                ),
            }
        }
    });
    #[cfg(not(target_arch = "wasm32"))]
    use_context_provider(|| crate::local_mode::desktop::VaultLockHolder(vault_lock));

    use_hook(|| {
        if let Some(m) = initial_mode_remembered {
            app_state.with_mut(|s| s.mode = m);
        }
    });

    // Mode is now resolved — pick the right default activity item and the
    // right plugin registry contributions accordingly.
    let initial_activity_id = match initial_mode_remembered {
        Some(Mode::Local) => Some(ActivityItemId(
            "local-projects-explorer:default".to_string(),
        )),
        Some(Mode::NonLocal) => Some(ActivityItemId("notes-explorer:default".to_string())),
        None => None,
    };
    let active: Signal<Option<ActivityItemId>> = use_signal(|| initial_activity_id);
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

    let resolved_mode = app_state.read().mode;
    #[cfg(not(target_arch = "wasm32"))]
    let persistence = {
        // Plans-Phase-2-saving: route Local-Mode FS persistence through the
        // user-chosen vault. Cloud mode stays on the legacy default dir until
        // we finish migrating cloud-mode notes too.
        let vault_now = vault_root.read().clone();
        provide_persistence_with_vault(resolved_mode, vault_now.as_ref())
    };
    #[cfg(target_arch = "wasm32")]
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
        let outcome = match resolved_mode {
            Mode::Local => crate::plugin::register_local_builtins(&mut registry, &ctx),
            Mode::NonLocal => register_builtins(&mut registry, &ctx),
        };
        if let Err(err) = outcome {
            eprintln!("operon: plugin register_builtins ({resolved_mode:?}) failed: {err}");
        }
        Rc::new(registry)
    });

    // Local-only app-scope signals — must be installed before mounting Shell so
    // the mode-aware StatusBar/ActivityBar/SideBar plugin can read them.
    if resolved_mode == Mode::Local {
        crate::local_mode::provide_local_app_signals();
    }

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
    #[cfg(not(target_arch = "wasm32"))]
    let vault_set = vault_root.read().is_some();
    #[cfg(target_arch = "wasm32")]
    let vault_set = true; // wasm path skips the local-mode picker
    #[cfg(target_arch = "wasm32")]
    let _ = vault_set;

    rsx! {
        document::Link { rel: "icon", href: FAVICON }
        // Stylesheets are emitted in <head> at template-build time via
        // manganis static_head (see asset!() options at the top of this
        // file). The runtime document::Stylesheet entries below ensure
        // hot-reload still re-applies CSS after a non-hot-reloadable
        // change; Dioxus dedupes them against the static-head links by
        // href so there is no duplicate fetch.
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
                {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        if !vault_set {
                            rsx! {
                                VaultDirPicker {
                                    blocking: true,
                                    on_chosen: move |root: VaultRoot| {
                                        vault_root.set(Some(root));
                                    },
                                }
                            }
                        } else {
                            rsx! {
                                crate::local_mode::LocalShellOverlay { Shell {} }
                            }
                        }
                    }
                    #[cfg(target_arch = "wasm32")]
                    rsx! { crate::local_mode::LocalShellOverlay { Shell {} } }
                }
            } else {
                Shell {}
            }
        }
    }
}

/// Wasm `Persistence` provider: returns `MemoryPersistence` until
/// `Plans-Phase-2-saving` lands the OPFS-backed implementation. Desktop
/// goes through [`provide_persistence_with_vault`] so the user's chosen
/// vault is honored.
#[cfg(target_arch = "wasm32")]
fn provide_persistence(mode: Mode) -> Arc<dyn Persistence> {
    let _ = mode;
    Arc::new(MemoryPersistence::new())
}

#[cfg(not(target_arch = "wasm32"))]
fn provide_persistence_with_vault(
    mode: Mode,
    vault_root: Option<&VaultRoot>,
) -> Arc<dyn Persistence> {
    use crate::persistence::FilesystemPersistence;
    let dir = match mode {
        Mode::Local => match vault_root {
            // Plans-Phase-2-saving: vault-rooted Local Mode persistence.
            // Markdown bodies live at <vault>/notes/<id>.md.
            Some(root) => root.notes_dir(),
            None => default_notes_dir().join("local"),
        },
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
