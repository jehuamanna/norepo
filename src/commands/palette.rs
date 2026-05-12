//! Command palette modal.
//!
//! Mounts as the last child of [`crate::shell::Shell`]; the modal renders only when
//! `palette.open` is `true`. Input keystrokes are stopped from propagating so Shell-level
//! shortcuts do not trigger while the palette is focused.
//!
//! Modes:
//! - [`PaletteMode::Commands`]: fuzzy-match against [`CommandRegistry::iter`].
//! - [`PaletteMode::Notes`]: fuzzy-match against built-in sample notes.
//! - [`PaletteMode::Themes`]: list every shipped theme by display name; live-preview on
//!   focus change; commit on Enter; revert on Escape.

use std::rc::Rc;

use dioxus::prelude::*;

use crate::commands::{fuzzy, CommandContext, CommandRegistry, PaletteMode, PaletteState};
use crate::plugin::PluginRegistry;
use crate::plugins::notes_explorer::samples::SAMPLES;
use crate::shell::layout::LayoutState;
use crate::shell::state::{ActiveActivity, LastActiveActivity};
use crate::tabs::TabManager;
use crate::theme::persistence::{self as theme_persistence, WebLocalStorage};
use crate::theme::{Theme, ThemeId, ThemeRegistry};

#[derive(Clone, Copy)]
enum CandidateKind {
    Command,
    Note,
    Theme(ThemeId),
}

#[derive(Clone)]
struct Candidate {
    id: String,
    title: String,
    category: String,
    payload: String,
    kind: CandidateKind,
}

fn compute_candidates(
    mode: PaletteMode,
    query: &str,
    cmd_reg: &CommandRegistry,
    theme_reg: &ThemeRegistry,
) -> Vec<Candidate> {
    match mode {
        PaletteMode::Commands => {
            let mut v: Vec<(Candidate, i32)> = cmd_reg
                .iter()
                .filter_map(|cmd| {
                    fuzzy::score(query, &cmd.title).map(|s| {
                        (
                            Candidate {
                                id: cmd.id.clone(),
                                title: cmd.title.clone(),
                                category: cmd.category.clone(),
                                payload: String::new(),
                                kind: CandidateKind::Command,
                            },
                            s,
                        )
                    })
                })
                .collect();
            v.sort_by(|a, b| b.1.cmp(&a.1));
            v.truncate(50);
            v.into_iter().map(|(c, _)| c).collect()
        }
        PaletteMode::Notes => {
            let mut v: Vec<(Candidate, i32)> = SAMPLES
                .iter()
                .filter_map(|(id, title, content)| {
                    fuzzy::score(query, title).map(|s| {
                        (
                            Candidate {
                                id: (*id).to_string(),
                                title: (*title).to_string(),
                                category: "Notes".into(),
                                payload: (*content).to_string(),
                                kind: CandidateKind::Note,
                            },
                            s,
                        )
                    })
                })
                .collect();
            v.sort_by(|a, b| b.1.cmp(&a.1));
            v.truncate(50);
            v.into_iter().map(|(c, _)| c).collect()
        }
        PaletteMode::Themes => {
            // Themes mode preserves canonical (`ThemeId::ALL`) order when the query is
            // empty so users see "VSCode Dark+" first; falls back to fuzzy ranking when a
            // query is typed.
            let descriptors = theme_reg.available();
            if query.is_empty() {
                return descriptors
                    .into_iter()
                    .map(|d| Candidate {
                        id: d.id.slug().to_string(),
                        title: d.display_name.to_string(),
                        category: "Color Theme".into(),
                        payload: String::new(),
                        kind: CandidateKind::Theme(d.id),
                    })
                    .collect();
            }
            let mut v: Vec<(Candidate, i32)> = descriptors
                .into_iter()
                .filter_map(|d| {
                    fuzzy::score(query, d.display_name).map(|s| {
                        (
                            Candidate {
                                id: d.id.slug().to_string(),
                                title: d.display_name.to_string(),
                                category: "Color Theme".into(),
                                payload: String::new(),
                                kind: CandidateKind::Theme(d.id),
                            },
                            s,
                        )
                    })
                })
                .collect();
            v.sort_by(|a, b| b.1.cmp(&a.1));
            v.into_iter().map(|(c, _)| c).collect()
        }
    }
}

#[component]
pub fn CommandPalette() -> Element {
    let mut palette: Signal<PaletteState> = use_context();
    let cmd_reg: Rc<CommandRegistry> = use_context();
    let plugin_reg: Rc<PluginRegistry> = use_context();
    let theme_reg: Rc<ThemeRegistry> = use_context();
    let mut theme: Signal<Theme> = use_context();
    let mut tabs: Signal<TabManager> = use_context();
    let ActiveActivity(active) = use_context();
    let LastActiveActivity(last_active) = use_context();
    let layout: Signal<LayoutState> = use_context();
    let crate::shell::about::AboutOpen(about_open) = use_context();
    #[cfg(not(target_arch = "wasm32"))]
    let repo_permissions_open: Option<Signal<bool>> = {
        let crate::shell::repo_permissions::RepoPermissionsOpen(s) = use_context();
        Some(s)
    };
    #[cfg(target_arch = "wasm32")]
    let repo_permissions_open: Option<Signal<bool>> = None;

    let snapshot = palette.read();
    let open = snapshot.open;
    let mode = snapshot.mode;
    let query = snapshot.query.clone();
    let selection = snapshot.selection;
    drop(snapshot);

    if !open {
        return rsx! { div { class: "operon-palette-hidden", style: "display: none;" } };
    }

    let candidates = compute_candidates(mode, &query, &cmd_reg, &theme_reg);
    let candidates_for_keydown = candidates.clone();
    let candidates_for_input = candidates.clone();
    let cmd_reg_for_keydown = cmd_reg.clone();
    let plugin_reg_for_keydown = plugin_reg.clone();
    let theme_reg_for_input = theme_reg.clone();
    let theme_reg_for_keydown = theme_reg.clone();

    rsx! {
        div {
            class: "operon-palette-backdrop",
            "data-component": "palette-backdrop",
            onclick: move |_| {
                // Backdrop click acts like Escape for the theme picker — restore origin.
                let original = palette.read().themes_original;
                if mode == PaletteMode::Themes {
                    if let Some(id) = original {
                        let restored = theme_reg.get(id).clone();
                        theme.set(restored);
                    }
                }
                palette.with_mut(|p| {
                    p.open = false;
                    p.themes_original = None;
                    p.themes_focus_cache = None;
                });
            },
            div {
                class: "operon-palette-modal",
                "data-component": "command-palette",
                "data-palette-mode": match mode { PaletteMode::Commands => "commands", PaletteMode::Notes => "notes", PaletteMode::Themes => "themes" },
                role: "dialog",
                "aria-modal": "true",
                onclick: move |evt| { evt.stop_propagation(); },
                input {
                    class: "operon-palette-input",
                    "aria-label": match mode {
                        PaletteMode::Commands => "Command palette",
                        PaletteMode::Notes => "Note picker",
                        PaletteMode::Themes => "Select Color Theme",
                    },
                    autofocus: true,
                    value: "{query}",
                    placeholder: match mode {
                        PaletteMode::Commands => "Type a command...",
                        PaletteMode::Notes => "Type a note title...",
                        PaletteMode::Themes => "Select Color Theme...",
                    },
                    oninput: move |evt| {
                        let q = evt.value();
                        palette.with_mut(|p| { p.query = q; p.selection = 0; });
                        if mode == PaletteMode::Themes {
                            // Re-preview the now-focused (index 0) theme. Recompute the
                            // candidate list under the new query to find what's at idx 0.
                            let q = palette.read().query.clone();
                            let new_candidates = compute_candidates(
                                PaletteMode::Themes,
                                &q,
                                &cmd_reg,
                                &theme_reg_for_input,
                            );
                            if let Some(c) = new_candidates.first() {
                                if let CandidateKind::Theme(id) = c.kind {
                                    let cached = palette.read().themes_focus_cache;
                                    if cached != Some(id) {
                                        let next = theme_reg_for_input.get(id).clone();
                                        theme.set(next);
                                        palette.with_mut(|p| p.themes_focus_cache = Some(id));
                                    }
                                }
                            }
                            // Reuse the freshly computed list for the rest of this render
                            // cycle is not necessary — Dioxus will re-render due to the
                            // signal write above.
                            let _ = candidates_for_input.clone();
                        }
                    },
                    onkeydown: move |evt| {
                        evt.stop_propagation();
                        let key = evt.key().to_string();
                        match key.as_str() {
                            "Escape" => {
                                let original = palette.read().themes_original;
                                if mode == PaletteMode::Themes {
                                    if let Some(id) = original {
                                        let restored = theme_reg_for_keydown.get(id).clone();
                                        theme.set(restored);
                                    }
                                }
                                palette.with_mut(|p| {
                                    p.open = false;
                                    p.themes_original = None;
                                    p.themes_focus_cache = None;
                                });
                            }
                            "ArrowDown" => {
                                let count = candidates_for_keydown.len();
                                if count > 0 {
                                    palette.with_mut(|p| {
                                        p.selection = (p.selection + 1).min(count - 1);
                                    });
                                    if mode == PaletteMode::Themes {
                                        let sel = palette.read().selection;
                                        preview_theme_at(
                                            &candidates_for_keydown,
                                            sel,
                                            &theme_reg_for_keydown,
                                            &mut palette,
                                            &mut theme,
                                        );
                                    }
                                }
                                evt.prevent_default();
                            }
                            "ArrowUp" => {
                                palette.with_mut(|p| {
                                    p.selection = p.selection.saturating_sub(1);
                                });
                                if mode == PaletteMode::Themes {
                                    let sel = palette.read().selection;
                                    preview_theme_at(
                                        &candidates_for_keydown,
                                        sel,
                                        &theme_reg_for_keydown,
                                        &mut palette,
                                        &mut theme,
                                    );
                                }
                                evt.prevent_default();
                            }
                            "Enter" => {
                                let sel = palette.read().selection;
                                if let Some(c) = candidates_for_keydown.get(sel) {
                                    match c.kind {
                                        CandidateKind::Command => {
                                            // Snapshot the mode so we can detect if the
                                            // command re-opens the palette in another mode
                                            // (e.g. workbench.action.selectTheme).
                                            let mode_before = palette.read().mode;
                                            let context = CommandContext {
                                                theme,
                                                tabs,
                                                active_activity: active,
                                                last_active_activity: last_active,
                                                registry: plugin_reg_for_keydown.clone(),
                                                palette,
                                                layout,
                                                theme_registry: theme_reg_for_keydown.clone(),
                                                about_open,
                                                repo_permissions_open,
                                                local_save: try_consume_context(),
                                            };
                                            let _ = cmd_reg_for_keydown.execute(&c.id, &context);
                                            // Only auto-close if the command didn't switch
                                            // the palette into a different mode.
                                            if palette.read().mode == mode_before {
                                                palette.write().open = false;
                                            }
                                        }
                                        CandidateKind::Note => {
                                            tabs.write().open(
                                                c.id.clone(),
                                                "markdown".to_string(),
                                                c.title.clone(),
                                                c.payload.clone(),
                                            );
                                            palette.write().open = false;
                                        }
                                        CandidateKind::Theme(id) => {
                                            // Commit: apply, persist, record LRU, close.
                                            let next = theme_reg_for_keydown.get(id).clone();
                                            theme.set(next);
                                            theme_persistence::record_theme_change(
                                                &WebLocalStorage,
                                                id,
                                            );
                                            palette.with_mut(|p| {
                                                p.open = false;
                                                p.themes_original = None;
                                                p.themes_focus_cache = None;
                                            });
                                        }
                                    }
                                }
                                evt.prevent_default();
                            }
                            _ => {}
                        }
                    },
                }
                ul {
                    class: "operon-palette-list",
                    for (idx, c) in candidates.into_iter().enumerate() {
                        li {
                            class: if idx == selection { "operon-palette-item operon-palette-item-active" } else { "operon-palette-item" },
                            "data-id": "{c.id}",
                            "data-index": "{idx}",
                            span { class: "operon-palette-item-category", "{c.category}" }
                            span { class: "operon-palette-item-title", "{c.title}" }
                        }
                    }
                }
            }
        }
    }
}

/// Apply the theme at the focused row as a live preview, de-bounced via `themes_focus_cache`.
fn preview_theme_at(
    candidates: &[Candidate],
    sel: usize,
    theme_reg: &ThemeRegistry,
    palette: &mut Signal<PaletteState>,
    theme: &mut Signal<Theme>,
) {
    if let Some(c) = candidates.get(sel) {
        if let CandidateKind::Theme(id) = c.kind {
            let cached = palette.read().themes_focus_cache;
            if cached != Some(id) {
                let next = theme_reg.get(id).clone();
                theme.set(next);
                palette.with_mut(|p| p.themes_focus_cache = Some(id));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{register_builtin_commands, CommandRegistry};
    use crate::theme::ThemeRegistry;

    fn seeded_registry() -> CommandRegistry {
        let mut r = CommandRegistry::new();
        register_builtin_commands(&mut r).expect("builtin commands register");
        r
    }

    #[test]
    fn compute_candidates_returns_all_commands_when_query_empty() {
        let reg = seeded_registry();
        let theme_reg = ThemeRegistry::new();
        let total = reg.iter().count();
        let result = compute_candidates(PaletteMode::Commands, "", &reg, &theme_reg);
        assert_eq!(
            result.len(),
            total,
            "empty query should pass every registered command through fuzzy::score"
        );
    }

    #[test]
    fn compute_candidates_filters_to_matching_subset_when_query_narrows() {
        let reg = seeded_registry();
        let theme_reg = ThemeRegistry::new();
        let total = reg.iter().count();
        let result = compute_candidates(PaletteMode::Commands, "togglesidebar", &reg, &theme_reg);
        assert!(
            result.len() < total,
            "narrowed query should drop non-matching commands ({} of {})",
            result.len(),
            total
        );
        assert!(
            result.iter().any(|c| c.id.contains("toggleSideBar")),
            "expected view.toggleSideBar in narrowed candidates"
        );
    }

    #[test]
    fn compute_candidates_in_notes_mode_only_yields_note_kind_candidates() {
        let reg = seeded_registry();
        let theme_reg = ThemeRegistry::new();
        let result = compute_candidates(PaletteMode::Notes, "", &reg, &theme_reg);
        assert!(
            !result.is_empty(),
            "Notes mode should return seeded samples"
        );
        for c in &result {
            assert!(matches!(c.kind, CandidateKind::Note));
            assert_eq!(c.category, "Notes");
        }
    }

    #[test]
    fn compute_candidates_truncates_to_fifty() {
        let reg = seeded_registry();
        let theme_reg = ThemeRegistry::new();
        let result = compute_candidates(PaletteMode::Commands, "", &reg, &theme_reg);
        assert!(
            result.len() <= 50,
            "must be truncated to <= 50; got {}",
            result.len()
        );
    }

    #[test]
    fn themes_mode_returns_nine_candidates_in_canonical_order() {
        let reg = seeded_registry();
        let theme_reg = ThemeRegistry::new();
        let result = compute_candidates(PaletteMode::Themes, "", &reg, &theme_reg);
        assert_eq!(result.len(), 9);
        for (i, &expected) in ThemeId::ALL.iter().enumerate() {
            assert_eq!(result[i].id, expected.slug());
            assert_eq!(result[i].category, "Color Theme");
            assert!(matches!(result[i].kind, CandidateKind::Theme(_)));
        }
    }

    #[test]
    fn themes_mode_filters_to_match_on_query() {
        let reg = seeded_registry();
        let theme_reg = ThemeRegistry::new();
        let result = compute_candidates(PaletteMode::Themes, "Solar", &reg, &theme_reg);
        assert!(!result.is_empty());
        for c in &result {
            assert!(
                c.title.to_lowercase().contains("solar"),
                "expected Solar* match, got {:?}",
                c.title
            );
        }
    }

    #[test]
    fn themes_mode_no_match_returns_empty() {
        let reg = seeded_registry();
        let theme_reg = ThemeRegistry::new();
        let result = compute_candidates(PaletteMode::Themes, "zzz-no-match", &reg, &theme_reg);
        assert!(result.is_empty());
    }
}
