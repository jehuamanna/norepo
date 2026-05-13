//! VS Code-style Shell layout.
//!
//! [`Shell`] arranges the five canonical regions in a CSS Grid (activity bar, side bar,
//! main area, companion area, status bar) and mounts the [`CommandPalette`] as the last
//! child (modal overlay above all regions). Owns Shell-level keyboard shortcuts:
//! `Ctrl+W` / `Cmd+W` closes the active tab; `Ctrl+B` / `Cmd+B` toggles the side bar;
//! `Ctrl+Shift+P` / `Cmd+Shift+P` opens the palette in commands mode; `Ctrl+P` / `Cmd+P`
//! opens it in notes mode. Tab/SideBar shortcuts are skipped while the palette is open
//! (the palette captures and stops keystroke propagation from its own input).

use std::rc::Rc;

use dioxus::prelude::*;

use crate::commands::{CommandPalette, PaletteMode, PaletteState};
use crate::panel::PanelStrip;
use crate::plugin::{PluginRegistry, PluginSurface};
use crate::rbag::state::{AppState, Mode};
use crate::shell::layout::{DragState, LayoutState, SplitterKind};
use crate::shell::splitter::{BottomSplitter, LeftSplitter, RightSplitter};
use crate::shell::state::{ActiveActivity, ActivityItemId, LastActiveActivity};
use crate::tabs::TabManager;

mod activity_bar;
pub mod about;
pub mod codemirror_host;
mod companion_area;
#[cfg(not(target_arch = "wasm32"))]
pub mod companion_chat;
pub mod companion_state;
#[cfg(not(target_arch = "wasm32"))]
pub mod agent_backend_picker;
#[cfg(not(target_arch = "wasm32"))]
pub mod permission_persist;
#[cfg(not(target_arch = "wasm32"))]
pub mod permission_prompt;
#[cfg(not(target_arch = "wasm32"))]
pub mod repo_permissions;
#[cfg(not(target_arch = "wasm32"))]
pub mod clarification_prompt;
#[cfg(not(target_arch = "wasm32"))]
pub mod settings;
#[cfg(not(target_arch = "wasm32"))]
pub mod mcp_settings;
#[cfg(not(target_arch = "wasm32"))]
mod session_rail;
#[cfg(not(target_arch = "wasm32"))]
mod tool_card;
pub mod dropdown;
pub mod editor_host;
pub mod layout;
mod main_area;
pub mod menubar;
pub mod mode_toolbar;
mod side_bar;
pub mod split_view;
pub mod splitter;
pub mod state;
mod status_bar;
pub mod tiptap_host;

pub use activity_bar::ActivityBar;
pub use companion_area::CompanionArea;
pub use main_area::MainArea;
pub use menubar::{MenuId, Menubar};
pub use mode_toolbar::ModeToolbar;
pub use side_bar::SideBar;
pub use split_view::SplitView;
pub use status_bar::StatusBar;

#[component]
pub fn Shell() -> Element {
    let mut tabs: Signal<TabManager> = use_context();
    let ActiveActivity(mut active) = use_context();
    let LastActiveActivity(last) = use_context();
    let registry: Rc<PluginRegistry> = use_context();
    let mut palette: Signal<PaletteState> = use_context();
    let mut open_menu: Signal<Option<menubar::MenuId>> = use_context();
    let mut layout: Signal<LayoutState> = use_context();
    let mut drag: Signal<Option<DragState>> = use_context();
    let app_state: Signal<AppState> = use_context();
    let local_save: Option<crate::local_mode::LocalSaveAction> = try_consume_context();

    let layout_read = layout.read();
    // When no activity is selected, side_bar.rs hides the panel via
    // `display: none`, but the grid column track still occupies its width
    // unless we zero it out here. Same applies to the explicit collapse flag.
    let no_active = active.read().is_none();
    let side_track = if layout_read.sidebar_collapsed || no_active {
        0
    } else {
        layout_read.sidebar_track()
    };
    let layout_style = format!(
        "--operon-side-bar-width: {}px; --operon-companion-width: {}px; --operon-panel-height: {}px; --operon-companion-rail-width: {}px;",
        side_track,
        layout_read.companion_track(),
        layout_read.panel_track(),
        layout_read.rail_width,
    );
    let collapsed_attr = if layout_read.sidebar_collapsed || no_active {
        "true"
    } else {
        "false"
    };
    drop(layout_read);

    // Global keyboard shim — installs a window-level capture-phase
    // keydown listener so a handful of shortcuts work even when Monaco
    // (or another input that captures keys) holds focus. The shim
    // forwards a small action vocabulary back to Rust via the eval
    // channel; the recv loop below dispatches the actions against
    // TabManager / ActiveActivity / LayoutState. Pure DOM actions
    // (focus.activitybar) are executed in JS to avoid a round-trip.
    install_global_shortcuts(tabs, active, layout, app_state);

    // Window-capture Ctrl+S → SAVE_REQUEST_TICK bridge. The capture
    // listener installed by `install_global_shortcuts` bumps the
    // tick when Ctrl+S fires (so Monaco / focused inputs can't
    // swallow the keypress). This effect observes the tick and
    // dispatches through `LocalSaveAction` — the same callback the
    // shell-level onkeydown handler uses, so the save flow is
    // unified regardless of where focus lives.
    {
        let local_save_for_tick = local_save.clone();
        use_effect(move || {
            // Subscribe — read forces a dependency on the signal.
            let _tick = *crate::shell::companion_state::SAVE_REQUEST_TICK.read();
            // Skip the initial render's value (tick starts at 0);
            // only react to subsequent bumps. We can't know "this
            // is the first run" without a side-channel, so the
            // tick start is treated as the no-op baseline: the JS
            // listener bumps to 1 on the user's first Ctrl+S, which
            // re-runs this effect and triggers the save.
            if _tick == 0 {
                return;
            }
            if let Some(action) = &local_save_for_tick {
                action.callback.call(());
            }
        });
    }

    rsx! {
        div {
            id: "operon-shell",
            class: "operon-shell-grid",
            tabindex: "-1",
            "data-sidebar-collapsed": "{collapsed_attr}",
            style: "{layout_style}",
            onmousemove: move |e| {
                let cur = *drag.read();
                if let Some(d) = cur {
                    let new_size = match d.kind {
                        SplitterKind::Left => {
                            let dx = e.client_coordinates().x as i32 - d.start_pos;
                            (d.start_size as i32 + dx).max(0) as u32
                        }
                        SplitterKind::Right => {
                            let dx = e.client_coordinates().x as i32 - d.start_pos;
                            (d.start_size as i32 - dx).max(0) as u32
                        }
                        SplitterKind::Bottom => {
                            let dy = e.client_coordinates().y as i32 - d.start_pos;
                            (d.start_size as i32 - dy).max(0) as u32
                        }
                        SplitterKind::Rail => {
                            let dx = e.client_coordinates().x as i32 - d.start_pos;
                            (d.start_size as i32 + dx).max(0) as u32
                        }
                    };
                    layout.with_mut(|s| match d.kind {
                        SplitterKind::Left => s.drag_sidebar(new_size),
                        SplitterKind::Right => s.drag_companion(new_size),
                        SplitterKind::Bottom => s.drag_panel(new_size),
                        SplitterKind::Rail => s.drag_rail(new_size),
                    });
                }
            },
            onmouseup: move |_| {
                if drag.read().is_some() {
                    drag.set(None);
                }
            },
            onkeydown: move |event| {
                let key_str = event.key().to_string();

                // Escape closes any open menubar dropdown — works without modifiers.
                if key_str == "Escape" {
                    if open_menu.read().is_some() {
                        open_menu.set(None);
                        event.prevent_default();
                        return;
                    }
                }

                let mods = event.modifiers();
                let with_meta = mods.contains(Modifiers::META) || mods.contains(Modifiers::CONTROL);
                if !with_meta { return; }

                // Mode-gated Ctrl+S: when Local Mode is active and a save
                // action is installed (from `provide_local_app_signals`), it
                // intercepts Ctrl+S before any later branch.
                if !mods.contains(Modifiers::SHIFT)
                    && !mods.contains(Modifiers::ALT)
                    && key_str.eq_ignore_ascii_case("s")
                    && app_state.read().mode == Mode::Local
                {
                    if let Some(action) = &local_save {
                        action.callback.call(());
                        event.prevent_default();
                        return;
                    }
                }

                let palette_open = palette.read().open;

                // Tab cycling: Ctrl/Cmd+PageDown or Ctrl+Tab → next tab,
                // Ctrl/Cmd+PageUp or Ctrl+Shift+Tab → prev tab. Skipped while
                // the palette is open so the palette's own keymap wins.
                if !palette_open {
                    let is_pgdn = key_str == "PageDown";
                    let is_pgup = key_str == "PageUp";
                    let is_tab = key_str == "Tab";
                    let go_next = is_pgdn || (is_tab && !mods.contains(Modifiers::SHIFT));
                    let go_prev = is_pgup || (is_tab && mods.contains(Modifiers::SHIFT));
                    if go_next && tabs.read().len() > 1 {
                        tabs.write().activate_next();
                        event.prevent_default();
                        return;
                    }
                    if go_prev && tabs.read().len() > 1 {
                        tabs.write().activate_prev();
                        event.prevent_default();
                        return;
                    }

                    // Ctrl/Cmd+1..9 → activate Nth tab. Skip when Shift/Alt
                    // are also held so they don't collide with future combos.
                    if !mods.contains(Modifiers::SHIFT)
                        && !mods.contains(Modifiers::ALT)
                        && key_str.len() == 1
                    {
                        if let Some(d) = key_str.chars().next().and_then(|c| c.to_digit(10)) {
                            if (1..=9).contains(&d) {
                                tabs.write().activate_index((d - 1) as usize);
                                event.prevent_default();
                                return;
                            }
                        }
                    }
                }

                // Focus the explorer panel: Ctrl/Cmd+Shift+E. Mirrors VS Code.
                // Switches the active activity to the projects/notes
                // explorer first (so the tree is mounted) before focusing,
                // and expands the side bar if it's currently collapsed —
                // otherwise pressing this from the search panel had no
                // visible effect because the explorer DOM wasn't there.
                if !palette_open
                    && mods.contains(Modifiers::SHIFT)
                    && !mods.contains(Modifiers::ALT)
                    && key_str.eq_ignore_ascii_case("e")
                {
                    let target = match app_state.read().mode {
                        Mode::Local => ActivityItemId(
                            "local-projects-explorer:default".to_string(),
                        ),
                        Mode::NonLocal => {
                            ActivityItemId("notes-explorer:default".to_string())
                        }
                    };
                    if active.read().as_ref() != Some(&target) {
                        active.set(Some(target));
                    }
                    if layout.read().sidebar_collapsed {
                        layout.with_mut(|s| s.toggle_sidebar());
                    }
                    document::eval(
                        r#"
                        (function() {
                            // Defer focus to next tick so Dioxus has time
                            // to mount the freshly-activated panel.
                            setTimeout(function() {
                                var el = document.querySelector('[data-testid="explorer-panel"]');
                                if (!el) {
                                    el = document.querySelector('[data-region="side-bar"] input')
                                        || document.querySelector('[data-region="side-bar"] [tabindex="0"]')
                                        || document.querySelector('[data-region="side-bar"]');
                                }
                                if (el && typeof el.focus === 'function') el.focus();
                            }, 0);
                        })();
                        "#,
                    );
                    event.prevent_default();
                    return;
                }

                // Focus the activity bar: Ctrl/Cmd+0. Lets keyboard users
                // jump from anywhere into the side-bar switcher.
                if !palette_open
                    && !mods.contains(Modifiers::SHIFT)
                    && !mods.contains(Modifiers::ALT)
                    && key_str == "0"
                {
                    document::eval(
                        r#"
                        (function() {
                            var el = document.querySelector('[data-region="activity-bar"] [data-activity-id]');
                            if (el && typeof el.focus === 'function') el.focus();
                        })();
                        "#,
                    );
                    event.prevent_default();
                    return;
                }

                if key_str.eq_ignore_ascii_case("p") {
                    let mode = if mods.contains(Modifiers::SHIFT) {
                        PaletteMode::Commands
                    } else {
                        PaletteMode::Notes
                    };
                    palette.set(PaletteState {
                        open: true,
                        mode,
                        query: String::new(),
                        selection: 0,
                        themes_original: None,
                        themes_focus_cache: None,
                    });
                    event.prevent_default();
                    return;
                }

                if palette_open { return; }

                if key_str.eq_ignore_ascii_case("w") {
                    let active_id = tabs.read().active_id();
                    if let Some(id) = active_id {
                        tabs.write().close(id);
                        event.prevent_default();
                    }
                } else if key_str.eq_ignore_ascii_case("b") {
                    layout.with_mut(|s| s.toggle_sidebar());
                    if active.read().is_none() {
                        let next = last.read().clone().or_else(|| {
                            registry
                                .contributions(PluginSurface::ActivityBar)
                                .next()
                                .map(|p| ActivityItemId(format!("{}:default", p.manifest().id)))
                        });
                        if let Some(id) = next {
                            active.set(Some(id));
                        }
                    }
                    event.prevent_default();
                }
            },
            a {
                class: "operon-skip-link",
                href: "#operon-main-area",
                onclick: move |evt| {
                    evt.prevent_default();
                    document::eval(
                        r#"
                        (function() {
                            var el = document.getElementById('operon-main-area');
                            if (!el) el = document.querySelector('[data-region="main-area"]');
                            if (el) {
                                if (!el.hasAttribute('tabindex')) el.setAttribute('tabindex', '-1');
                                if (typeof el.focus === 'function') el.focus();
                            }
                        })();
                        "#,
                    );
                },
                "Skip to editor"
            }
            Menubar {}
            ActivityBar {}
            SideBar {}
            MainArea {}
            PanelStrip {}
            CompanionArea {}
            StatusBar {}
            LeftSplitter {}
            RightSplitter {}
            BottomSplitter {}
            CommandPalette {}
        }
    }
}

/// Install a window-level capture-phase keydown listener that translates
/// a small set of global shortcuts into action messages, then route those
/// messages to TabManager / ActiveActivity / LayoutState in a Rust recv
/// loop. The shim is necessary because Monaco (and other inputs) install
/// their own capture-phase listeners on inner elements; once the bubbling
/// event reaches the shell's onkeydown the editor has typically already
/// handled or swallowed it. By listening at window-capture we run before
/// any descendant and can preventDefault + stopPropagation cleanly.
///
/// The complementary shell-level onkeydown above stays in place as a
/// belt-and-braces fallback (and to keep mouse-only / non-Monaco UI flows
/// fast — those never round-trip through this channel).
fn install_global_shortcuts(
    mut tabs: Signal<TabManager>,
    mut active: Signal<Option<ActivityItemId>>,
    mut layout: Signal<LayoutState>,
    app_state: Signal<AppState>,
) {
    use crate::editor::EditorMode;
    use_hook(move || {
        let mut handle = document::eval(
            r#"
            if (!window.__operonShortcutsInstalled) {
                window.__operonShortcutsInstalled = true;
                window.addEventListener('keydown', function(e) {
                    var meta = e.metaKey || e.ctrlKey;
                    if (!meta) return;
                    if (e.altKey) return;
                    var key = e.key;
                    var shift = e.shiftKey;
                    var action = null;

                    if (key === 'Tab') {
                        action = shift ? 'tab.prev' : 'tab.next';
                    } else if (!shift && key === 'PageDown') {
                        action = 'tab.next';
                    } else if (!shift && key === 'PageUp') {
                        action = 'tab.prev';
                    } else if (!shift && (key === '\\' || e.code === 'Backslash')) {
                        action = 'editor.split';
                    } else if (shift && (key === 'e' || key === 'E')) {
                        action = 'focus.explorer';
                    } else if (!shift && key === '0') {
                        action = 'focus.activitybar';
                    } else if (!shift && (key === 's' || key === 'S')) {
                        // Ctrl/Cmd+S: route through this capture-phase
                        // listener so Monaco / focused inputs can't
                        // swallow the keypress. The Shell component's
                        // use_effect on SAVE_REQUEST_TICK fires the
                        // active tab through LocalSaveAction.
                        action = 'file.save';
                    } else if (!shift && key.length === 1 && '123456789'.indexOf(key) >= 0) {
                        action = 'tab.' + key;
                    }
                    if (!action) return;
                    e.preventDefault();
                    e.stopPropagation();
                    try { dioxus.send({type: 'shortcut', action: action}); } catch (err) {}
                }, true);
            }
            // Block forever so the eval channel stays alive.
            await new Promise(function() {});
            "#,
        );
        spawn(async move {
            loop {
                let msg: serde_json::Value = match handle.recv().await {
                    Ok(v) => v,
                    Err(_) => break,
                };
                let action = msg.get("action").and_then(|v| v.as_str()).unwrap_or("");
                match action {
                    "tab.next" => {
                        if tabs.read().len() > 1 {
                            tabs.write().activate_next();
                        }
                    }
                    "tab.prev" => {
                        if tabs.read().len() > 1 {
                            tabs.write().activate_prev();
                        }
                    }
                    "editor.split" => {
                        // Toggle Split <-> Edit on the active tab. Falls
                        // back to a no-op if no tab is open.
                        let info = tabs
                            .read()
                            .active()
                            .map(|t| (t.id, t.mode));
                        if let Some((id, cur)) = info {
                            let next = match cur {
                                EditorMode::Split => EditorMode::Edit,
                                _ => EditorMode::Split,
                            };
                            tabs.write().set_mode(id, next);
                        }
                    }
                    "focus.explorer" => {
                        let target = match app_state.read().mode {
                            Mode::Local => ActivityItemId(
                                "local-projects-explorer:default".to_string(),
                            ),
                            Mode::NonLocal => {
                                ActivityItemId("notes-explorer:default".to_string())
                            }
                        };
                        if active.read().as_ref() != Some(&target) {
                            active.set(Some(target));
                        }
                        if layout.read().sidebar_collapsed {
                            layout.with_mut(|s| s.toggle_sidebar());
                        }
                        document::eval(
                            r#"
                            setTimeout(function() {
                                var el = document.querySelector('[data-testid="explorer-panel"]')
                                    || document.querySelector('[data-region="side-bar"] input')
                                    || document.querySelector('[data-region="side-bar"] [tabindex="0"]')
                                    || document.querySelector('[data-region="side-bar"]');
                                if (el && typeof el.focus === 'function') el.focus();
                            }, 0);
                            "#,
                        );
                    }
                    "focus.activitybar" => {
                        document::eval(
                            r#"
                            (function() {
                                var el = document.querySelector('[data-region="activity-bar"] [data-activity-id]');
                                if (el && typeof el.focus === 'function') el.focus();
                            })();
                            "#,
                        );
                    }
                    "file.save" => {
                        // Ctrl/Cmd+S: bump the global save-request
                        // tick. The Shell component watches it via
                        // use_effect and dispatches the active tab
                        // through LocalSaveAction. Going through the
                        // tick (instead of calling the save callback
                        // here) keeps this recv loop free of the
                        // LocalSaveAction context — that lives at
                        // Shell scope, which isn't reachable from
                        // this hook installed at App scope.
                        crate::shell::companion_state::SAVE_REQUEST_TICK
                            .with_mut(|n| *n = n.saturating_add(1));
                    }
                    other => {
                        if let Some(num) = other.strip_prefix("tab.") {
                            if let Ok(n) = num.parse::<usize>() {
                                if (1..=9).contains(&n) {
                                    tabs.write().activate_index(n - 1);
                                }
                            }
                        }
                    }
                }
            }
        });
    });
}
