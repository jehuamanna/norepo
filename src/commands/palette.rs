//! Command palette modal.
//!
//! Mounts as the last child of [`crate::shell::Shell`]; the modal renders only when
//! `palette.open` is `true`. Input keystrokes are stopped from propagating so Shell-level
//! shortcuts do not trigger while the palette is focused.

use std::rc::Rc;

use dioxus::prelude::*;

use crate::commands::{fuzzy, CommandContext, CommandRegistry, PaletteMode, PaletteState};
use crate::plugin::manifest::NoteKind;
use crate::plugin::PluginRegistry;
use crate::plugins::notes_explorer::samples::SAMPLES;
use crate::shell::layout::LayoutState;
use crate::shell::state::{ActiveActivity, LastActiveActivity};
use crate::tabs::TabManager;
use crate::theme::Theme;

#[derive(Clone, Copy)]
enum CandidateKind {
    Command,
    Note,
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
    }
}

#[component]
pub fn CommandPalette() -> Element {
    let mut palette: Signal<PaletteState> = use_context();
    let cmd_reg: Rc<CommandRegistry> = use_context();
    let plugin_reg: Rc<PluginRegistry> = use_context();
    let theme: Signal<Theme> = use_context();
    let mut tabs: Signal<TabManager> = use_context();
    let ActiveActivity(active) = use_context();
    let LastActiveActivity(last_active) = use_context();
    let layout: Signal<LayoutState> = use_context();

    let snapshot = palette.read();
    let open = snapshot.open;
    let mode = snapshot.mode;
    let query = snapshot.query.clone();
    let selection = snapshot.selection;
    drop(snapshot);

    if !open {
        return rsx! { div { class: "operon-palette-hidden", style: "display: none;" } };
    }

    let candidates = compute_candidates(mode, &query, &cmd_reg);
    let candidates_for_keydown = candidates.clone();
    let cmd_reg_for_keydown = cmd_reg.clone();
    let plugin_reg_for_keydown = plugin_reg.clone();

    rsx! {
        div {
            class: "operon-palette-backdrop",
            "data-component": "palette-backdrop",
            onclick: move |_| { palette.write().open = false; },
            div {
                class: "operon-palette-modal",
                "data-component": "command-palette",
                role: "dialog",
                "aria-modal": "true",
                onclick: move |evt| { evt.stop_propagation(); },
                input {
                    class: "operon-palette-input",
                    "aria-label": "Command palette",
                    autofocus: true,
                    value: "{query}",
                    placeholder: match mode { PaletteMode::Commands => "Type a command...", PaletteMode::Notes => "Type a note title..." },
                    oninput: move |evt| {
                        let q = evt.value();
                        palette.with_mut(|p| { p.query = q; p.selection = 0; });
                    },
                    onkeydown: move |evt| {
                        evt.stop_propagation();
                        let key = evt.key().to_string();
                        match key.as_str() {
                            "Escape" => { palette.write().open = false; }
                            "ArrowDown" => {
                                let count = candidates_for_keydown.len();
                                if count > 0 {
                                    palette.with_mut(|p| {
                                        p.selection = (p.selection + 1).min(count - 1);
                                    });
                                }
                                evt.prevent_default();
                            }
                            "ArrowUp" => {
                                palette.with_mut(|p| {
                                    p.selection = p.selection.saturating_sub(1);
                                });
                                evt.prevent_default();
                            }
                            "Enter" => {
                                let sel = palette.read().selection;
                                if let Some(c) = candidates_for_keydown.get(sel) {
                                    match c.kind {
                                        CandidateKind::Command => {
                                            let context = CommandContext {
                                                theme,
                                                tabs,
                                                active_activity: active,
                                                last_active_activity: last_active,
                                                registry: plugin_reg_for_keydown.clone(),
                                                palette,
                                                layout,
                                            };
                                            let _ = cmd_reg_for_keydown.execute(&c.id, &context);
                                            palette.write().open = false;
                                        }
                                        CandidateKind::Note => {
                                            tabs.write().open(
                                                c.id.clone(),
                                                NoteKind::Markdown,
                                                c.title.clone(),
                                                c.payload.clone(),
                                            );
                                            palette.write().open = false;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{register_builtin_commands, CommandRegistry};

    fn seeded_registry() -> CommandRegistry {
        let mut r = CommandRegistry::new();
        register_builtin_commands(&mut r).expect("builtin commands register");
        r
    }

    #[test]
    fn compute_candidates_returns_all_commands_when_query_empty() {
        let reg = seeded_registry();
        let total = reg.iter().count();
        let result = compute_candidates(PaletteMode::Commands, "", &reg);
        assert_eq!(
            result.len(),
            total,
            "empty query should pass every registered command through fuzzy::score"
        );
    }

    #[test]
    fn compute_candidates_filters_to_matching_subset_when_query_narrows() {
        let reg = seeded_registry();
        let total = reg.iter().count();
        let result = compute_candidates(PaletteMode::Commands, "togglesidebar", &reg);
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
        let result = compute_candidates(PaletteMode::Notes, "", &reg);
        assert!(!result.is_empty(), "Notes mode should return seeded samples");
        for c in &result {
            assert!(matches!(c.kind, CandidateKind::Note));
            assert_eq!(c.category, "Notes");
        }
    }

    #[test]
    fn compute_candidates_truncates_to_fifty() {
        let reg = seeded_registry();
        let result = compute_candidates(PaletteMode::Commands, "", &reg);
        assert!(result.len() <= 50, "must be truncated to <= 50; got {}", result.len());
    }
}
