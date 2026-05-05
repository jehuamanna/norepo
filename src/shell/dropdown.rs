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
    let mut open_menu: Signal<Option<MenuId>> = use_context();

    let category = menu.category_label();
    let items: Vec<(String, String)> = cmd_reg
        .iter()
        .filter(|c| c.category.eq_ignore_ascii_case(category))
        .map(|c| (c.id.clone(), c.title.clone()))
        .collect();
    let empty = items.is_empty();
    let label = menu.label();

    rsx! {
        div {
            class: "operon-dropdown-backdrop",
            onclick: move |_| { open_menu.set(None); },
        }
        ul {
            class: "operon-dropdown",
            "data-menu": "{label}",
            onclick: move |evt| { evt.stop_propagation(); },
            if empty {
                li { class: "operon-dropdown-empty", "(empty)" }
            }
            for (id, title) in items.into_iter() {
                {
                    let id_attr = id.clone();
                    let title_text = title.clone();
                    let cmd_reg = cmd_reg.clone();
                    let plugin_reg = plugin_reg.clone();
                    let theme_reg = theme_reg.clone();
                    rsx! {
                        li {
                            class: "operon-dropdown-item",
                            "data-id": "{id_attr}",
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
                                };
                                let _ = cmd_reg.execute(&id, &context);
                                open_menu.set(None);
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
        assert!(!empty_for(&r, MenuId::Help), "Help maps to Palette which has built-ins");
        assert!(empty_for(&r, MenuId::File));
        assert!(empty_for(&r, MenuId::Edit));
        assert!(empty_for(&r, MenuId::Selection));
        assert!(empty_for(&r, MenuId::Run));
    }
}
