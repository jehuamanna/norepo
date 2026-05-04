//! Menubar — VS Code-style top strip with the Operon "O" brand on the left and dropdowns
//! of [`crate::commands::CommandRegistry`] entries grouped by category.

use dioxus::prelude::*;

use crate::shell::dropdown::Dropdown;
use crate::shell::layout::LayoutState;

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum MenuId {
    File,
    Edit,
    Selection,
    View,
    Run,
    Help,
}

impl MenuId {
    pub const ALL: &'static [MenuId] = &[
        Self::File,
        Self::Edit,
        Self::Selection,
        Self::View,
        Self::Run,
        Self::Help,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::File => "File",
            Self::Edit => "Edit",
            Self::Selection => "Selection",
            Self::View => "View",
            Self::Run => "Run",
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
                                    class: "{cls}",
                                    "data-menu": "{label}",
                                    onclick: move |evt| {
                                        evt.stop_propagation();
                                        let cur = open_menu.read().as_ref().copied();
                                        if cur == Some(menu) {
                                            open_menu.set(None);
                                        } else {
                                            open_menu.set(Some(menu));
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
            div { class: "operon-menubar-right",
                button {
                    class: "operon-toggle-btn",
                    "data-action": "toggle-panel",
                    title: "Toggle Panel",
                    onclick: move |_| { layout.with_mut(|s| s.toggle_panel()); },
                    "▾"
                }
                button {
                    class: "operon-toggle-btn",
                    "data-action": "toggle-companion",
                    title: "Toggle Companion",
                    onclick: move |_| { layout.with_mut(|s| s.toggle_companion()); },
                    "▸"
                }
            }
        }
    }
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
    fn menu_ids_are_six_in_order() {
        assert_eq!(MenuId::ALL.len(), 6);
        let labels: Vec<_> = MenuId::ALL.iter().map(|m| m.label()).collect();
        assert_eq!(labels, vec!["File", "Edit", "Selection", "View", "Run", "Help"]);
    }

    #[test]
    fn help_maps_to_palette_category() {
        assert_eq!(MenuId::Help.category_label(), "Palette");
        assert_eq!(MenuId::View.category_label(), "View");
    }
}
