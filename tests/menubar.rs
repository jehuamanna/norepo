//! Integration tests for the menubar's mapping from `MenuId` → `Command::category`.

use operon_dioxus::commands::{register_builtin_commands, CommandRegistry};
use operon_dioxus::shell::menubar::MenuId;

fn count_for_category(reg: &CommandRegistry, label: &str) -> usize {
    reg.iter()
        .filter(|c| c.category.eq_ignore_ascii_case(label))
        .count()
}

#[test]
fn view_category_has_at_least_three_built_ins() {
    let mut reg = CommandRegistry::new();
    register_builtin_commands(&mut reg).unwrap();
    let n = count_for_category(&reg, MenuId::View.category_label());
    assert!(n >= 3, "expected at least 3 View commands, got {n}");
}

#[test]
fn help_category_label_resolves_to_palette_built_ins() {
    let mut reg = CommandRegistry::new();
    register_builtin_commands(&mut reg).unwrap();
    let n = count_for_category(&reg, MenuId::Help.category_label());
    assert!(n >= 1, "expected at least 1 Palette command for Help, got {n}");
}

#[test]
fn unfilled_categories_render_empty() {
    let mut reg = CommandRegistry::new();
    register_builtin_commands(&mut reg).unwrap();
    for menu in [MenuId::File, MenuId::Edit, MenuId::Selection, MenuId::Run] {
        let n = count_for_category(&reg, menu.category_label());
        assert_eq!(n, 0, "{menu:?} should be empty after Phase-5 builtins; got {n}");
    }
}
