//! Menubar — VS Code-style top strip with the Operon "O" brand on the left and dropdowns
//! of [`crate::commands::CommandRegistry`] entries grouped by category.

use dioxus::prelude::*;

use crate::shell::dropdown::Dropdown;
use crate::shell::layout::LayoutState;
use crate::ui::Icon;

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum MenuId {
    File,
    Edit,
    Selection,
    View,
    Run,
    Tools,
    Help,
}

impl MenuId {
    pub const ALL: &'static [MenuId] = &[
        Self::File,
        Self::Edit,
        Self::Selection,
        Self::View,
        Self::Run,
        Self::Tools,
        Self::Help,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::File => "File",
            Self::Edit => "Edit",
            Self::Selection => "Selection",
            Self::View => "View",
            Self::Run => "Run",
            Self::Tools => "Tools",
            Self::Help => "Help",
        }
    }

    /// Category string used to filter `CommandRegistry::iter()` (case-insensitive).
    /// `Help` maps to the existing `Palette` category until a real `Help` category exists.
    pub fn category_label(self) -> &'static str {
        match self {
            Self::Help => "Palette",
            other => other.label(),
        }
    }
}

#[component]
pub fn Menubar() -> Element {
    let mut open_menu: Signal<Option<MenuId>> = use_context();
    let mut layout: Signal<LayoutState> = use_context();

    rsx! {
        section {
            "data-region": "menubar",
            class: "operon-menubar",
            role: "menubar",
            "aria-label": "Application menu",
            OperonBrand {}
            div { class: "operon-menubar-items",
                for menu in MenuId::ALL.iter().copied() {
                    {
                        let is_open = open_menu.read().as_ref() == Some(&menu);
                        let label = menu.label();
                        let cls = if is_open {
                            "operon-menubar-button operon-menubar-button-open"
                        } else {
                            "operon-menubar-button"
                        };
                        rsx! {
                            div {
                                class: "operon-menubar-button-wrapper",
                                button {
                                    r#type: "button",
                                    class: "{cls}",
                                    "data-menu": "{label}",
                                    role: "menuitem",
                                    "aria-haspopup": "menu",
                                    "aria-expanded": if is_open { "true" } else { "false" },
                                    "aria-label": "{label}",
                                    onclick: move |evt| {
                                        evt.stop_propagation();
                                        let cur = open_menu.read().as_ref().copied();
                                        if cur == Some(menu) {
                                            open_menu.set(None);
                                        } else {
                                            open_menu.set(Some(menu));
                                        }
                                    },
                                    onkeydown: move |evt| {
                                        let key = evt.key().to_string();
                                        if key == "ArrowDown" || key == "Enter" || key == " " {
                                            evt.prevent_default();
                                            open_menu.set(Some(menu));
                                        } else if key == "ArrowLeft" || key == "ArrowRight" {
                                            evt.prevent_default();
                                            let dir = if key == "ArrowRight" { 1i32 } else { -1i32 };
                                            let script = format!(
                                                r#"
                                                (function() {{
                                                    var nodes = Array.prototype.slice.call(document.querySelectorAll('.operon-menubar-button'));
                                                    if (!nodes.length) return;
                                                    var cur = document.activeElement;
                                                    var idx = nodes.indexOf(cur);
                                                    if (idx < 0) idx = 0;
                                                    var next = idx + ({dir});
                                                    if (next < 0) next = nodes.length - 1;
                                                    if (next >= nodes.length) next = 0;
                                                    nodes[next].focus();
                                                }})();
                                                "#
                                            );
                                            document::eval(&script);
                                        }
                                    },
                                    "{label}"
                                }
                                if is_open { Dropdown { menu } }
                            }
                        }
                    }
                }
            }
            div {
                class: "operon-menubar-right",
                // `position: relative` so the help popover inside
                // `CompanionModeToggle` can drop down anchored to this
                // cluster (right: 0; top: 100%;) without measurement JS.
                style: "position: relative;",
                CompanionModeToggle {}
                button {
                    r#type: "button",
                    class: "operon-toggle-btn",
                    "data-action": "toggle-panel",
                    title: "Toggle Panel",
                    "aria-label": "Toggle bottom panel",
                    onclick: move |_| { layout.with_mut(|s| s.toggle_panel()); },
                    Icon { name: "panel".to_string() }
                }
                button {
                    r#type: "button",
                    class: "operon-toggle-btn",
                    "data-action": "toggle-companion",
                    title: "Toggle Companion",
                    "aria-label": "Toggle companion panel",
                    onclick: move |_| { layout.with_mut(|s| s.toggle_companion()); },
                    Icon { name: "sidebar-right".to_string() }
                }
            }
        }
    }
}

/// Two-segment toggle that flips the companion pane between the
/// Operon chat surface and the raw `claude` CLI terminal. Persists to
/// `local_app_settings` under [`SETTINGS_KEY_COMPANION_MODE`] and
/// bumps [`COMPANION_MODE_VERSION`] so `CompanionArea` swaps surfaces
/// in-place. Replaces the old Settings → Companion pane radio.
///
/// Web build mounts nothing — the settings repo is desktop-only.
#[cfg(not(target_arch = "wasm32"))]
#[component]
fn CompanionModeToggle() -> Element {
    use crate::local_mode::desktop::LocalSettingsRepo;
    use crate::local_mode::{
        COMPANION_MODE_CHAT, COMPANION_MODE_CLAUDE_CODE,
        SETTINGS_KEY_COMPANION_MODE,
    };

    let LocalSettingsRepo(settings_repo) = use_context();

    // Subscribe to the version signal so any other writer (or our own
    // click below) triggers a re-render of this scope.
    let _ = crate::shell::companion_state::COMPANION_MODE_VERSION.read();
    let current_mode = settings_repo
        .get(SETTINGS_KEY_COMPANION_MODE)
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| COMPANION_MODE_CHAT.to_string());
    let is_chat = current_mode == COMPANION_MODE_CHAT;

    let settings_for_chat = settings_repo.clone();
    let settings_for_cli = settings_repo.clone();

    // Disclosure state for the inline "?" help popover. Closed on
    // mount; toggled by the `?` button, the `×` inside the popover,
    // or Escape while the popover has focus. Outside-click dismissal
    // is deliberately omitted in this iteration — see plan.
    let mut help_open: Signal<bool> = use_signal(|| false);
    let help_is_open = *help_open.read();

    rsx! {
        div {
            class: "operon-companion-mode-toggle",
            role: "group",
            "aria-label": "Companion mode",
            "data-mode": if is_chat { "chat" } else { "claude_code" },
            "data-testid": "menubar-companion-mode-toggle",
            style: "display: inline-flex; border: 1px solid var(--vscode-panel-border, #444); border-radius: 4px; overflow: hidden; margin-right: 8px; align-self: center; font-size: 11px; line-height: 1;",
            button {
                r#type: "button",
                "data-testid": "menubar-companion-mode-chat",
                "aria-pressed": if is_chat { "true" } else { "false" },
                title: "Claude Code Chat — rich Operon chat surface",
                style: format!(
                    "padding: 3px 8px; border: none; cursor: pointer; background: {bg}; color: {fg};",
                    bg = if is_chat {
                        "var(--vscode-button-background, #0e639c)"
                    } else {
                        "transparent"
                    },
                    fg = if is_chat {
                        "var(--vscode-button-foreground, #ffffff)"
                    } else {
                        "var(--vscode-foreground, inherit)"
                    },
                ),
                onclick: move |_| {
                    if is_chat { return; }
                    if let Err(e) = settings_for_chat.set(
                        SETTINGS_KEY_COMPANION_MODE,
                        COMPANION_MODE_CHAT,
                    ) {
                        tracing::warn!(
                            target: "operon::menubar",
                            "persist companion mode (chat) failed: {e}"
                        );
                    }
                    *crate::shell::companion_state::COMPANION_MODE_VERSION.write() += 1;
                },
                "Chat"
            }
            button {
                r#type: "button",
                "data-testid": "menubar-companion-mode-cli",
                "aria-pressed": if is_chat { "false" } else { "true" },
                title: "Claude Code CLI — raw upstream terminal",
                style: format!(
                    "padding: 3px 8px; border: none; cursor: pointer; background: {bg}; color: {fg};",
                    bg = if !is_chat {
                        "var(--vscode-button-background, #0e639c)"
                    } else {
                        "transparent"
                    },
                    fg = if !is_chat {
                        "var(--vscode-button-foreground, #ffffff)"
                    } else {
                        "var(--vscode-foreground, inherit)"
                    },
                ),
                onclick: move |_| {
                    if !is_chat { return; }
                    if let Err(e) = settings_for_cli.set(
                        SETTINGS_KEY_COMPANION_MODE,
                        COMPANION_MODE_CLAUDE_CODE,
                    ) {
                        tracing::warn!(
                            target: "operon::menubar",
                            "persist companion mode (cli) failed: {e}"
                        );
                    }
                    *crate::shell::companion_state::COMPANION_MODE_VERSION.write() += 1;
                },
                "CLI"
            }
        }
        // "?" disclosure — small icon button that opens the help
        // popover below the menubar-right cluster. Lives outside the
        // toggle group so its hover/focus styling doesn't get pulled
        // into the role="group" semantics.
        button {
            r#type: "button",
            "data-testid": "menubar-companion-mode-help",
            "aria-haspopup": "dialog",
            "aria-expanded": if help_is_open { "true" } else { "false" },
            "aria-controls": "operon-companion-mode-help",
            title: "What's the difference between Chat and CLI?",
            style: "margin-right: 8px; padding: 0 6px; border: 1px solid var(--vscode-panel-border, #444); border-radius: 4px; background: transparent; color: var(--vscode-descriptionforeground, var(--vscode-foreground, inherit)); cursor: pointer; font-size: 11px; line-height: 1.6; align-self: center;",
            onclick: move |_| { help_open.set(!help_is_open); },
            "?"
        }
        if help_is_open {
            div {
                id: "operon-companion-mode-help",
                role: "dialog",
                "aria-label": "Companion mode help",
                "data-testid": "menubar-companion-mode-help-panel",
                tabindex: "-1",
                onkeydown: move |evt| {
                    if evt.key().to_string() == "Escape" {
                        help_open.set(false);
                    }
                },
                style: "position: absolute; top: 100%; right: 0; margin-top: 4px; z-index: 50; width: 520px; max-width: calc(100vw - 24px); padding: 12px 14px 14px 14px; background: var(--vscode-editorWidget-background, var(--vscode-panel-background, #1e1e1e)); color: var(--vscode-editorWidget-foreground, var(--vscode-foreground, inherit)); border: 1px solid var(--vscode-editorWidget-border, var(--vscode-panel-border, #444)); border-radius: 4px; box-shadow: 0 4px 12px rgba(0,0,0,0.25); font-size: 11.5px; line-height: 1.5; text-align: left;",
                div {
                    style: "display: flex; align-items: center; justify-content: space-between; margin-bottom: 8px;",
                    div {
                        style: "font-weight: 600; font-size: 12px;",
                        "Companion modes"
                    }
                    button {
                        r#type: "button",
                        autofocus: true,
                        "aria-label": "Close help",
                        title: "Close",
                        style: "border: none; background: transparent; color: inherit; cursor: pointer; font-size: 14px; line-height: 1; padding: 2px 6px; opacity: 0.7;",
                        onclick: move |_| { help_open.set(false); },
                        "\u{00d7}"
                    }
                }
                div {
                    style: "display: flex; gap: 16px; align-items: flex-start;",
                    div {
                        style: "flex: 1 1 0; min-width: 0;",
                        div {
                            style: "font-weight: 600; margin-bottom: 4px;",
                            "Chat"
                        }
                        ul {
                            style: "margin: 0; padding-left: 16px;",
                            li { "Question picker (single / multi-select + free-text \u{201C}Other\u{201D})" }
                            li { "Permission cards with diff preview + editable JSON" }
                            li { "\u{201C}Allow always\u{201D} persists in one click" }
                            li { "Note proposal / deletion cards" }
                            li { "Auto-approve policy UI (per category + per tool)" }
                            li { "Thinking deltas shown inline" }
                        }
                    }
                    div {
                        style: "flex: 1 1 0; min-width: 0;",
                        div {
                            style: "font-weight: 600; margin-bottom: 4px;",
                            "CLI"
                        }
                        ul {
                            style: "margin: 0; padding-left: 16px;",
                            li { "Raw upstream claude TUI in xterm.js" }
                            li { "Live Bash stdout / stderr streaming" }
                            li { "Slash commands (/clear, /compact, /cost)" }
                            li { "Whatever Anthropic ships next lands here first" }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
#[component]
fn CompanionModeToggle() -> Element {
    rsx! {}
}

#[component]
fn OperonBrand() -> Element {
    rsx! {
        div {
            class: "operon-brand",
            "data-brand": "operon",
            title: "Operon",
            "O"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_are_seven_in_order() {
        assert_eq!(MenuId::ALL.len(), 7);
        let labels: Vec<_> = MenuId::ALL.iter().map(|m| m.label()).collect();
        assert_eq!(
            labels,
            vec!["File", "Edit", "Selection", "View", "Run", "Tools", "Help"]
        );
    }

    #[test]
    fn help_maps_to_palette_category() {
        assert_eq!(MenuId::Help.category_label(), "Palette");
        assert_eq!(MenuId::View.category_label(), "View");
    }

    #[test]
    fn tools_category_label_is_tools() {
        // Tools menu hosts a real `Tools` command category (currently
        // just `tools.openRepoPermissions`). It does NOT fall through
        // to Palette like Help does.
        assert_eq!(MenuId::Tools.category_label(), "Tools");
    }
}
