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
use crate::panel::{PanelManager, TerminalsManager};
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

    // App-scope reveal-line request: a search-panel line click writes
    // `(note_id, line)` here so the editor host can scroll + place the caret
    // when its backend mounts (or immediately, when the tab was already open).
    let request_editor_reveal_line: Signal<Option<(String, u32)>> = use_signal(|| None);
    use_context_provider(|| {
        crate::editor::RequestEditorRevealLine(request_editor_reveal_line)
    });

    // Plans-Phase-8-explorer-undo: app-scope toast slot. Producers (e.g.
    // failed undo) write here; ToastHost reads + auto-clears after 3 s.
    // Gated on the same cfg as the local_mode::ui module.
    #[cfg(any(not(target_arch = "wasm32"), feature = "wasm-sqlite"))]
    let toast_slot: Signal<Option<crate::local_mode::ui::Toast>> = use_signal(|| None);
    #[cfg(any(not(target_arch = "wasm32"), feature = "wasm-sqlite"))]
    use_context_provider(|| crate::local_mode::ui::ToastSlot(toast_slot));

    // (note: `CHAT_MESSAGE_VERSION` is a `GlobalSignal` defined in
    // `shell::companion_state` — application-wide, not provided via
    // context, so it doesn't need to live in any specific scope.
    // Background drainers write to it; the companion's load-effect
    // watches it.)

    // Local Mode wiring: install the LocalUserRepo / LocalSettingsRepo before any
    // component reads them. Then resolve the remembered mode from
    // `local_app_settings`; if absent, AppState defaults to NonLocal but we
    // render the chooser instead of mounting a shell.
    crate::local_mode::provide_local_state();

    let mut app_state: Signal<AppState> = use_signal(AppState::default);
    use_context_provider(|| app_state);

    // App-scope visibility for the About dialog (surfaced from Help → About
    // and the `help.about` command). Provided here so any descendant
    // (palette, dropdown, command handlers) can flip it without prop-
    // drilling. The dialog itself owns the close path.
    let about_open: Signal<bool> = use_signal(|| false);
    use_context_provider(|| crate::shell::about::AboutOpen(about_open));

    // App-scope visibility for the Repo Permissions panel (Tools menu +
    // `tools.openRepoPermissions` command). Desktop-only — the panel
    // reads/writes per-repo `.claude/settings.local.json` files which
    // don't exist on wasm.
    #[cfg(not(target_arch = "wasm32"))]
    let repo_permissions_open: Signal<bool> = use_signal(|| false);
    #[cfg(not(target_arch = "wasm32"))]
    use_context_provider(|| {
        crate::shell::repo_permissions::RepoPermissionsOpen(repo_permissions_open)
    });

    // App-scope visibility for the Project Claude Defaults panel (Tools
    // menu + `tools.openProjectClaudeSettings` command). Same wasm
    // carve-out — the panel touches `local_project` columns added in
    // migration 019, which only exist in the desktop sqlite store.
    #[cfg(not(target_arch = "wasm32"))]
    let project_claude_settings_open: Signal<bool> = use_signal(|| false);
    #[cfg(not(target_arch = "wasm32"))]
    use_context_provider(|| {
        crate::shell::project_claude_settings::ProjectClaudeSettingsOpen(
            project_claude_settings_open,
        )
    });

    // App-scope target for the per-project Tool Permissions modal. Set
    // to Some(project_id) by the explorer row's gear icon / context-menu
    // entry; cleared by the modal's close paths. Desktop-only because
    // the policy file lives on disk in the project's repo.
    #[cfg(not(target_arch = "wasm32"))]
    let project_tool_permissions_target: Signal<Option<uuid::Uuid>> = use_signal(|| None);
    #[cfg(not(target_arch = "wasm32"))]
    use_context_provider(|| {
        crate::shell::project_tool_permissions::ProjectToolPermissionsTarget(
            project_tool_permissions_target,
        )
    });

    // App-scope target for the per-project MCP servers modal. Set to
    // Some(project_id) by the project row's context-menu entry; cleared
    // by the modal's close paths. Desktop-only because configuration is
    // written into `<repo>/.mcp.json` via the `claude mcp` CLI.
    #[cfg(not(target_arch = "wasm32"))]
    let project_mcp_settings_target: Signal<Option<uuid::Uuid>> = use_signal(|| None);
    #[cfg(not(target_arch = "wasm32"))]
    use_context_provider(|| {
        crate::shell::project_mcp_settings::ProjectMcpSettingsTarget(
            project_mcp_settings_target,
        )
    });

    // App-scope visibility for the global (user-scope) MCP servers
    // modal. Flipped by the Settings dialog button.
    #[cfg(not(target_arch = "wasm32"))]
    let global_mcp_settings_open: Signal<bool> = use_signal(|| false);
    #[cfg(not(target_arch = "wasm32"))]
    use_context_provider(|| {
        crate::shell::global_mcp_settings::GlobalMcpSettingsOpen(global_mcp_settings_open)
    });

    // App-scope visibility for the global Chat Permissions modal.
    // Flipped by the Settings dialog button. Desktop-only because the
    // policy is persisted to `~/.claude/settings.json` on the local
    // filesystem.
    #[cfg(not(target_arch = "wasm32"))]
    let global_chat_permissions_open: Signal<bool> = use_signal(|| false);
    #[cfg(not(target_arch = "wasm32"))]
    use_context_provider(|| {
        crate::shell::global_chat_permissions::GlobalChatPermissionsOpen(
            global_chat_permissions_open,
        )
    });

    // App-scope MCP service. Has to live up here (not inside
    // `Workspace`) because the `GlobalMcpSettingsPanelHost` and
    // `ProjectMcpSettingsPanelHost` are siblings of `Workspace` in
    // the App rsx — same comment as `AboutDialog` for "modal floats
    // above StartupChooser AND Workspace alike." The panels' inner
    // `McpSettingsPanel` reads `McpServiceCtx` via `use_context`,
    // which would panic if the provider lived only in Workspace.
    #[cfg(not(target_arch = "wasm32"))]
    use_context_provider(|| {
        let claude_bin = crate::shell::companion_chat::resolve_claude_bin();
        crate::shell::mcp_settings::McpServiceCtx(std::sync::Arc::new(
            crate::shell::mcp_settings::McpService::new(claude_bin),
        ))
    });

    // App-scope `ActiveRepoPath`. Same reason as `McpServiceCtx`
    // above — the `AddForm` and `ServerCard` rendered by the
    // sibling MCP panel hosts read it via `use_context()`. The
    // signal is mutated from inside `Workspace` (its `use_effect`
    // mirrors the explorer's selected project), but the *cell* is
    // created here so both subtrees see the same one.
    #[cfg(not(target_arch = "wasm32"))]
    let active_repo_path: Signal<Option<std::path::PathBuf>> = use_signal(|| None);
    #[cfg(not(target_arch = "wasm32"))]
    use_context_provider(|| {
        crate::shell::companion_state::ActiveRepoPath(active_repo_path)
    });

    // App-scope visibility for the queued-permissions drawer. Toggled
    // by the activity-bar badge; component overlays the app shell.
    #[cfg(not(target_arch = "wasm32"))]
    let permission_drawer_open: Signal<bool> = use_signal(|| false);
    #[cfg(not(target_arch = "wasm32"))]
    use_context_provider(|| {
        crate::shell::permission_drawer::PermissionDrawerOpen(permission_drawer_open)
    });

    // Background watchdog: auto-denies pending permission prompts
    // older than 5 minutes. Especially relevant once cascades opt
    // into honoring the AutoApprovePolicy (Phase 4) — a missed
    // approval would otherwise hang the cascade indefinitely.
    // `use_hook` runs exactly once per component instance, which
    // matches "spawn the ticker once at app boot."
    #[cfg(not(target_arch = "wasm32"))]
    use_hook(|| {
        crate::shell::companion_state::start_permission_watchdog();
    });

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
    let vault_root: Signal<Option<VaultRoot>> = {
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

    // Reactive "user has chosen a mode" flag — flipped by StartupChooser when
    // either button is clicked so App can transition out of the chooser
    // without requiring a restart. Seeded from the once-read remembered mode.
    let mode_chosen: Signal<bool> = use_signal(|| initial_mode_remembered.is_some());
    use_context_provider(|| crate::local_mode::ModeChosen(mode_chosen));

    // The `ActiveActivity` signal is mode-dependent (its initial item id
    // differs between Local and NonLocal builtins) and so is provided by
    // `Workspace`, not here.

    let last_active: Signal<Option<ActivityItemId>> = use_signal(|| None);
    use_context_provider(|| LastActiveActivity(last_active));

    let palette: Signal<PaletteState> = use_signal(PaletteState::default);
    use_context_provider(|| palette);

    let open_menu: Signal<Option<MenuId>> = use_signal(|| None);
    use_context_provider(|| open_menu);

    let panel: Signal<PanelManager> = use_signal(PanelManager::new);
    use_context_provider(|| panel);

    // Terminal panel tabs. Cross-platform descriptor list; the PTY
    // side is native-only and reads this from context.
    let terminals: Signal<TerminalsManager> = use_signal(TerminalsManager::new);
    use_context_provider(|| terminals);

    let layout: Signal<LayoutState> = use_signal(LayoutState::load_or_default);
    use_context_provider(|| layout);
    use_effect(move || {
        let snapshot = *layout.read();
        snapshot.save();
    });

    let drag: Signal<Option<DragState>> = use_signal(|| None);
    use_context_provider(|| drag);

    let mut log_buffer: Signal<LogBuffer> = use_signal(LogBuffer::new);
    use_context_provider(|| log_buffer);

    // Mode-dependent setup (persistence path, plugin registry, default
    // activity item, Local-Mode app-scope signals) lives in `Workspace`
    // below. `Workspace` is only mounted after the user has chosen a mode,
    // so its hooks always run with the resolved mode in hand — avoiding
    // the "registry initialised once for the wrong mode" bug that came
    // from running these in App during the StartupChooser phase.

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

    // HTML5 drag-and-drop on wry's WebKit/webkit2gtk backend silently aborts
    // a `dragstart` whose handler doesn't populate `dataTransfer`. Dioxus
    // 0.7's `DragData` exposes no source-side `setData` API, so we install a
    // tiny capture-phase JS shim here that stuffs a placeholder payload on
    // any draggable explorer row (`data-explorer="true"`). The Rust event
    // chain (DragSession signal, descendant cycle check, drop dispatch) is
    // the source of truth; this shim only exists to keep the native drag
    // alive so dragover/drop fire.
    use_hook(|| {
        document::eval(
            r#"
            if (!window.__operonDndShimInstalled) {
                window.__operonDndShimInstalled = true;
                document.addEventListener('dragstart', function(e) {
                    if (!e.target || !e.dataTransfer) return;
                    var closest = e.target.closest;
                    if (!closest) return;
                    // Cover the explorer tree and the tab strip together.
                    // webkit2gtk silently aborts a dragstart whose handler
                    // didn't populate dataTransfer; without this shim
                    // ondragover / ondrop never fire on the receiving side.
                    var t = closest.call(e.target,
                        '[data-explorer="true"][draggable="true"], ' +
                        '.operon-tab[draggable="true"]'
                    );
                    if (!t) return;
                    e.dataTransfer.effectAllowed = 'move';
                    var id = t.dataset.noteId
                        || t.dataset.projectId
                        || t.dataset.tabId
                        || 'operon-row';
                    try { e.dataTransfer.setData('text/plain', id); } catch (_) {}
                }, true);
            }
            "#,
        );
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

    let mode_known = *mode_chosen.read();

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
            } else {
                Workspace {}
            }
        }
        // Top-level overlay so the dialog floats above StartupChooser and
        // Workspace alike. Component returns an empty fragment when the
        // signal is false.
        crate::shell::about::AboutDialog {}
        // Same pattern for the Tools → Repo Permissions panel. Desktop-
        // only because the panel touches the filesystem; the wasm stub
        // renders nothing.
        RepoPermissionsPanelHost {}
        ProjectClaudeSettingsPanelHost {}
        ProjectToolPermissionsPanelHost {}
        GlobalChatPermissionsPanelHost {}
        GlobalMcpSettingsPanelHost {}
        ProjectMcpSettingsPanelHost {}
        PermissionDrawerHost {}
    }
}

/// Thin host that mounts `RepoPermissionsPanel` on desktop only. The
/// panel itself reads/writes per-repo `.claude/settings.local.json`
/// files which don't exist in the wasm sandbox.
#[component]
fn RepoPermissionsPanelHost() -> Element {
    #[cfg(not(target_arch = "wasm32"))]
    {
        rsx! { crate::shell::repo_permissions::RepoPermissionsPanel {} }
    }
    #[cfg(target_arch = "wasm32")]
    {
        rsx! {}
    }
}

/// Same shape as `RepoPermissionsPanelHost` for the Project Claude
/// Defaults panel — desktop-only because the underlying
/// `local_project.{default_model, default_permission_mode}` columns are
/// SQLite-only (migration 019).
#[component]
fn ProjectClaudeSettingsPanelHost() -> Element {
    #[cfg(not(target_arch = "wasm32"))]
    {
        rsx! { crate::shell::project_claude_settings::ProjectClaudeSettingsPanel {} }
    }
    #[cfg(target_arch = "wasm32")]
    {
        rsx! {}
    }
}

/// Per-project Tool Permissions modal host. Desktop-only because the
/// policy is persisted to a file on disk in the project's bound repo.
#[component]
fn ProjectToolPermissionsPanelHost() -> Element {
    #[cfg(not(target_arch = "wasm32"))]
    {
        rsx! { crate::shell::project_tool_permissions::ProjectToolPermissionsPanel {} }
    }
    #[cfg(target_arch = "wasm32")]
    {
        rsx! {}
    }
}

/// Global Chat Permissions modal host. Desktop-only because the policy
/// is persisted to `~/.claude/settings.json` on the local filesystem.
#[component]
fn GlobalChatPermissionsPanelHost() -> Element {
    #[cfg(not(target_arch = "wasm32"))]
    {
        rsx! { crate::shell::global_chat_permissions::GlobalChatPermissionsPanel {} }
    }
    #[cfg(target_arch = "wasm32")]
    {
        rsx! {}
    }
}

/// Global (user-scope) MCP servers modal host. Desktop-only because
/// the underlying `claude mcp` CLI is unavailable in the wasm sandbox.
#[component]
fn GlobalMcpSettingsPanelHost() -> Element {
    #[cfg(not(target_arch = "wasm32"))]
    {
        rsx! { crate::shell::global_mcp_settings::GlobalMcpSettingsPanel {} }
    }
    #[cfg(target_arch = "wasm32")]
    {
        rsx! {}
    }
}

/// Per-project MCP servers modal host. Desktop-only — project-scope
/// configuration writes to `<repo>/.mcp.json` on disk.
#[component]
fn ProjectMcpSettingsPanelHost() -> Element {
    #[cfg(not(target_arch = "wasm32"))]
    {
        rsx! { crate::shell::project_mcp_settings::ProjectMcpSettingsPanel {} }
    }
    #[cfg(target_arch = "wasm32")]
    {
        rsx! {}
    }
}

/// Desktop-only host for the queued-permissions drawer. The drawer's
/// data (`PERMISSION_PROMPTS`, `PERMISSION_DECISIONS`) and bridge are
/// non-wasm — there are no permission asks to display on wasm — so
/// this just renders nothing in that case.
#[component]
fn PermissionDrawerHost() -> Element {
    #[cfg(not(target_arch = "wasm32"))]
    {
        rsx! { crate::shell::permission_drawer::PermissionDrawer {} }
    }
    #[cfg(target_arch = "wasm32")]
    {
        rsx! {}
    }
}

/// Mounts only after the user has picked a mode. Owns every context
/// provider whose initialiser depends on `AppState.mode` or the chosen
/// vault so they are computed exactly once with the resolved values.
#[component]
fn Workspace() -> Element {
    let app_state = use_context::<Signal<AppState>>();
    let theme = use_context::<Signal<Theme>>();
    let tabs = use_context::<Signal<TabManager>>();

    let resolved_mode = app_state.read().mode;

    #[cfg(not(target_arch = "wasm32"))]
    let crate::local_mode::CurrentVaultRoot(mut vault_root) = use_context();

    #[cfg(not(target_arch = "wasm32"))]
    let persistence: Arc<dyn Persistence> = {
        let vault_now = vault_root.read().clone();
        let inner = provide_persistence_with_vault(resolved_mode, vault_now.as_ref());
        // Migration 018: route artifact saves/loads through the
        // hierarchy-derived `.operon/artifacts/<slug>/.../index.md` path so
        // the on-disk layout mirrors the UI tree 1:1. Non-artifact notes
        // continue to use the opaque UUID-indexed `inner` store.
        if resolved_mode == Mode::Local {
            use crate::local_mode::desktop::{LocalNoteRepo, LocalProjectRepo};
            let LocalNoteRepo(note_repo) = use_context::<LocalNoteRepo>();
            let LocalProjectRepo(project_repo) = use_context::<LocalProjectRepo>();
            // One-shot legacy reshape: backfill slugs and migrate any
            // legacy `<UUID>/<title>.md` staging files + opaque
            // `<notes_dir>/<UUID>` bodies onto the canonical
            // `<vault>/.operon/<project-id>/artifacts/<slug>/index.md` path.
            // Sentinel-gated per-project, so this is a no-op after first run.
            if let Some(ref vault) = vault_now {
                crate::plugins::artifact::migrate_v018::migrate_all_projects(
                    &project_repo,
                    &note_repo,
                    &vault.notes_dir(),
                    vault,
                );
            }
            Arc::new(crate::persistence::ArtifactPersistence::new(
                inner,
                note_repo,
                vault_now.clone(),
            )) as Arc<dyn Persistence>
        } else {
            inner
        }
    };
    #[cfg(target_arch = "wasm32")]
    let persistence: Arc<dyn Persistence> = provide_persistence(resolved_mode);
    let scheduler = SaveScheduler::new(persistence.clone());
    use_context_provider(|| persistence);
    use_context_provider(|| scheduler);

    // Chat-UI dispatcher: route GlobalSignal mutations from non-
    // Dioxus tokio tasks (the permission_bridge handler closure, the
    // in-process ask_user executor) through a single drain task that
    // runs under the Dioxus runtime guard. Without this, those tasks
    // panic on `PERMISSION_PROMPTS.write()` / `ASK_USER_PROMPTS.write()`
    // and the parked responders silently leak — the user sees a
    // tool stuck on RUNNING with no permission card to click.
    // See `companion_state::ChatUiCommand` for the contract.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let rx = crate::shell::companion_state::init_chat_ui_dispatch();
        use_hook(move || {
            let mut rx = rx;
            dioxus::prelude::spawn(async move {
                while let Some(cmd) = rx.recv().await {
                    crate::shell::companion_state::apply_chat_ui_command(cmd);
                }
            });
        });
    }

    // M4c.0: bring up the in-tree MCP bridge. Must be after the
    // persistence provider above — bridge tools (operon_get_note
    // etc.) read note bodies through it.
    crate::local_mode::provide_bridge_runtime();

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

    // Local-Mode app-scope signals (consume `tabs`, `persistence`, and
    // the SQLite repos installed by `provide_local_state` in App).
    crate::local_mode::provide_local_app_signals();

    let initial_activity_id = match resolved_mode {
        Mode::Local => Some(ActivityItemId(
            "local-projects-explorer:default".to_string(),
        )),
        Mode::NonLocal => Some(ActivityItemId("notes-explorer:default".to_string())),
    };
    let active: Signal<Option<ActivityItemId>> = use_signal(|| initial_activity_id);
    use_context_provider(|| ActiveActivity(active));

    #[cfg(not(target_arch = "wasm32"))]
    let vault_set = vault_root.read().is_some();
    #[cfg(target_arch = "wasm32")]
    let vault_set = true;
    #[cfg(target_arch = "wasm32")]
    let _ = vault_set;

    rsx! {
        if resolved_mode == Mode::Local {
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
                        rsx! { crate::local_mode::LocalShellOverlay { Shell {} } }
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
