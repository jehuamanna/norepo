//! Dropdown component for [`super::menubar::Menubar`] menus.
//!
//! Renders a positioned `<ul>` of [`crate::commands::Command`]s filtered by the active
//! [`super::menubar::MenuId`]'s category (case-insensitive), plus a transparent backdrop
//! that closes the dropdown on outside-click.

use std::rc::Rc;

use dioxus::prelude::*;

use crate::commands::{CommandContext, CommandRegistry, PaletteState};
use crate::plugin::PluginRegistry;
use crate::shell::layout::LayoutState;
use crate::shell::menubar::MenuId;
use crate::shell::state::{ActiveActivity, LastActiveActivity};
use crate::tabs::TabManager;
use crate::theme::{Theme, ThemeRegistry};

#[component]
pub fn Dropdown(menu: MenuId) -> Element {
    let cmd_reg: Rc<CommandRegistry> = use_context();
    let plugin_reg: Rc<PluginRegistry> = use_context();
    let theme_reg: Rc<ThemeRegistry> = use_context();
    let theme: Signal<Theme> = use_context();
    let tabs: Signal<TabManager> = use_context();
    let ActiveActivity(active) = use_context();
    let LastActiveActivity(last_active) = use_context();
    let palette: Signal<PaletteState> = use_context();
    let layout: Signal<LayoutState> = use_context();
    let crate::shell::about::AboutOpen(about_open) = use_context();
    let mut open_menu: Signal<Option<MenuId>> = use_context();

    let category = menu.category_label();
    let items: Vec<(String, String)> = cmd_reg
        .iter()
        .filter(|c| c.category.eq_ignore_ascii_case(category))
        .map(|c| {
            // Toggle commands get a state-aware label so the user can
            // see at a glance whether they're on/off/unset. The
            // CommandRegistry stores static titles; rather than
            // refactoring Command to hold a closure, we look up the
            // GlobalSignal state at render time and decorate the
            // displayed string. Each open of the View dropdown
            // re-renders this match, so the indicator stays current.
            let title = match c.id.as_str() {
                "cascade.toggleStepMode" => {
                    let state = *crate::shell::companion_state::
                        CASCADE_STEP_MODE_OVERRIDE
                        .read();
                    let suffix = match state {
                        None => "(default)",
                        Some(true) => "ON \u{2014} pause every skill",
                        Some(false) => "OFF \u{2014} batch by level",
                    };
                    format!("{} \u{2022} {}", c.title, suffix)
                }
                _ => c.title.clone(),
            };
            (c.id.clone(), title)
        })
        .collect();
    let empty = items.is_empty();
    let label = menu.label();

    let label_for_mounted = label.to_string();
    rsx! {
        div {
            class: "operon-dropdown-backdrop",
            onclick: move |_| { open_menu.set(None); },
        }
        ul {
            class: "operon-dropdown",
            "data-menu": "{label}",
            role: "menu",
            "aria-label": "{label}",
            tabindex: "-1",
            onclick: move |evt| { evt.stop_propagation(); },
            onkeydown: move |evt| {
                let key = evt.key().to_string();
                if key == "Escape" {
                    evt.prevent_default();
                    evt.stop_propagation();
                    open_menu.set(None);
                } else if key == "ArrowDown" || key == "ArrowUp" {
                    evt.prevent_default();
                    let dir = if key == "ArrowDown" { 1i32 } else { -1i32 };
                    let script = format!(
                        r#"
                        (function() {{
                            var dd = document.querySelector('.operon-dropdown[data-menu="{label}"]');
                            if (!dd) return;
                            var nodes = Array.prototype.slice.call(dd.querySelectorAll('[role="menuitem"]'));
                            if (!nodes.length) return;
                            var cur = document.activeElement;
                            var idx = nodes.indexOf(cur);
                            if (idx < 0) idx = -1;
                            var next = idx + ({dir});
                            if (next < 0) next = nodes.length - 1;
                            if (next >= nodes.length) next = 0;
                            nodes[next].focus();
                        }})();
                        "#
                    );
                    document::eval(&script);
                } else if key == "Home" {
                    evt.prevent_default();
                    let script = format!(
                        r#"
                        (function() {{
                            var dd = document.querySelector('.operon-dropdown[data-menu="{label}"]');
                            if (!dd) return;
                            var first = dd.querySelector('[role="menuitem"]');
                            if (first) first.focus();
                        }})();
                        "#
                    );
                    document::eval(&script);
                } else if key == "End" {
                    evt.prevent_default();
                    let script = format!(
                        r#"
                        (function() {{
                            var dd = document.querySelector('.operon-dropdown[data-menu="{label}"]');
                            if (!dd) return;
                            var nodes = dd.querySelectorAll('[role="menuitem"]');
                            if (nodes.length) nodes[nodes.length - 1].focus();
                        }})();
                        "#
                    );
                    document::eval(&script);
                }
            },
            // Auto-focus the first menuitem when the dropdown opens so the
            // user can immediately ArrowDown/ArrowUp through it.
            onmounted: move |_| {
                let script = format!(
                    r#"
                    (function() {{
                        var dd = document.querySelector('.operon-dropdown[data-menu="{l}"]');
                        if (!dd) return;
                        var first = dd.querySelector('[role="menuitem"]');
                        if (first && typeof first.focus === 'function') first.focus();
                    }})();
                    "#,
                    l = label_for_mounted,
                );
                document::eval(&script);
            },
            if empty {
                li { class: "operon-dropdown-empty", role: "presentation", "(empty)" }
            }
            for (id, title) in items.into_iter() {
                {
                    let id_attr = id.clone();
                    let title_text = title.clone();
                    let cmd_reg = cmd_reg.clone();
                    let plugin_reg = plugin_reg.clone();
                    let theme_reg = theme_reg.clone();
                    let id_for_keys = id.clone();
                    let cmd_reg_keys = cmd_reg.clone();
                    let plugin_reg_keys = plugin_reg.clone();
                    let theme_reg_keys = theme_reg.clone();
                    rsx! {
                        li {
                            class: "operon-dropdown-item",
                            "data-id": "{id_attr}",
                            role: "menuitem",
                            tabindex: "-1",
                            onclick: move |_| {
                                let context = CommandContext {
                                    theme,
                                    tabs,
                                    active_activity: active,
                                    last_active_activity: last_active,
                                    registry: plugin_reg.clone(),
                                    palette,
                                    layout,
                                    theme_registry: theme_reg.clone(),
                                    about_open,
                                    local_save: try_consume_context(),
                                };
                                let _ = cmd_reg.execute(&id, &context);
                                open_menu.set(None);
                            },
                            onkeydown: move |evt| {
                                let key = evt.key().to_string();
                                if key == "Enter" || key == " " {
                                    evt.prevent_default();
                                    let context = CommandContext {
                                        theme,
                                        tabs,
                                        active_activity: active,
                                        last_active_activity: last_active,
                                        registry: plugin_reg_keys.clone(),
                                        palette,
                                        layout,
                                        theme_registry: theme_reg_keys.clone(),
                                        about_open,
                                        local_save: try_consume_context(),
                                    };
                                    let _ = cmd_reg_keys.execute(&id_for_keys, &context);
                                    open_menu.set(None);
                                }
                            },
                            "{title_text}"
                        }
                    }
                }
            }
        }
    }
}

/// True iff no command matches the menu's category. Used by tests and by the menubar to
/// short-circuit empty dropdowns.
pub fn empty_for(reg: &CommandRegistry, menu: MenuId) -> bool {
    let cat = menu.category_label();
    !reg.iter().any(|c| c.category.eq_ignore_ascii_case(cat))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::register_builtin_commands;

    #[test]
    fn empty_categories_report_empty_after_builtins() {
        let mut r = CommandRegistry::new();
        register_builtin_commands(&mut r).unwrap();
        assert!(!empty_for(&r, MenuId::View), "View has built-ins");
        assert!(
            !empty_for(&r, MenuId::Help),
            "Help maps to Palette which has built-ins"
        );
        assert!(!empty_for(&r, MenuId::File), "File now hosts file.saveNote");
        assert!(empty_for(&r, MenuId::Edit));
        assert!(empty_for(&r, MenuId::Selection));
        assert!(empty_for(&r, MenuId::Run));
    }
}
