//! Inline form widget for `clarification` artifacts.
//!
//! The `00-coherence-check` skill emits clarification artifacts when
//! it detects cross-level discrepancies (A0→A4) in the SDLC chain.
//! Each artifact's body declares a question type (`single_choice` or
//! `multi_choice`), a list of options as `- [ ] <label>` bullets, and
//! a `## Resolution target` slug list — the artifact(s) whose
//! `revision_notes` should be updated when the user answers.
//!
//! This module
//!
//! 1. Parses a clarification body into a typed `Clarification` struct,
//! 2. Renders an interactive form (`ClarificationPanel`) with radio
//!    buttons for single-choice or checkboxes for multi-choice, plus
//!    a free-text "Other" slot that round-trips through the same
//!    answer payload, and
//! 3. Exposes a single `on_submit` event with the user's chosen
//!    labels (each item is either a verbatim option label or the
//!    `Other: <typed text>` form).
//!
//! The artifact view drives the rest of the flow: persisting the
//! answer back into the body, flipping the artifact to Approved, and
//! waking the cascade.

#![cfg(not(target_arch = "wasm32"))]

use dioxus::prelude::*;

/// Question shape declared in a clarification artifact body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum QuestionType {
    #[default]
    SingleChoice,
    MultiChoice,
}

/// Parsed form of a clarification artifact body. Constructed by
/// `parse_clarification`; passed to `ClarificationPanel` as a prop.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Clarification {
    pub question_type: QuestionType,
    /// `- [ ] <label>` lines under `## Options`. The trailing
    /// `Other: ___` option is hoisted into `has_other` rather than
    /// kept in this list so the UI can render a free-text slot.
    pub options: Vec<String>,
    /// `true` when the parsed bullets included an `Other: ___` line.
    /// The renderer always shows an "Other" slot regardless, but this
    /// flag lets the artifact-view writer decide whether to preserve
    /// or normalize the body on save.
    pub has_other: bool,
    /// Slugs listed under `## Resolution target` — the artifacts the
    /// user's answer should be merged into. Empty when the section is
    /// missing or only contains prose.
    pub resolution_targets: Vec<String>,
}

/// Parse a clarification artifact's body (the markdown after the
/// frontmatter). Lenient: missing sections fall back to defaults so a
/// half-written artifact still renders an empty form rather than
/// crashing the view.
pub fn parse_clarification(body: &str) -> Clarification {
    let mut question_type = QuestionType::SingleChoice;
    let mut options: Vec<String> = Vec::new();
    let mut has_other = false;
    let mut resolution_targets: Vec<String> = Vec::new();

    let mut section: Option<Section> = None;
    for raw in body.lines() {
        let line = raw.trim_end();
        if let Some(rest) = line.strip_prefix("## ") {
            let heading = rest.trim().to_ascii_lowercase();
            section = match heading.as_str() {
                "question type" => Some(Section::QuestionType),
                "options" => Some(Section::Options),
                "resolution target" | "resolution targets" => {
                    Some(Section::ResolutionTargets)
                }
                _ => None,
            };
            continue;
        }
        if line.starts_with("# ") {
            // A `# H1` resets us — clarification bodies start with
            // `# Clarification: <topic>` and the sections above are
            // the only places we care about.
            section = None;
            continue;
        }
        match section {
            Some(Section::QuestionType) => {
                let t = line.trim().to_ascii_lowercase();
                if t.contains("multi") {
                    question_type = QuestionType::MultiChoice;
                } else if t.contains("single") {
                    question_type = QuestionType::SingleChoice;
                }
            }
            Some(Section::Options) => {
                if let Some(label) = parse_option_bullet(line) {
                    if is_other_option(&label) {
                        has_other = true;
                    } else {
                        options.push(label);
                    }
                }
            }
            Some(Section::ResolutionTargets) => {
                if let Some(slug) = parse_slug_bullet(line) {
                    resolution_targets.push(slug);
                }
            }
            None => {}
        }
    }
    Clarification {
        question_type,
        options,
        has_other,
        resolution_targets,
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Section {
    QuestionType,
    Options,
    ResolutionTargets,
}

fn parse_option_bullet(line: &str) -> Option<String> {
    let l = line.trim();
    let rest = l
        .strip_prefix("- [ ]")
        .or_else(|| l.strip_prefix("- [x]"))
        .or_else(|| l.strip_prefix("- [X]"))
        .or_else(|| l.strip_prefix("- "))?;
    let label = rest.trim().to_string();
    if label.is_empty() {
        None
    } else {
        Some(label)
    }
}

fn is_other_option(label: &str) -> bool {
    let lower = label.to_ascii_lowercase();
    lower == "other"
        || lower.starts_with("other:")
        || lower.starts_with("other —")
        || lower.starts_with("other -")
        || lower.starts_with("other:_")
}

fn parse_slug_bullet(line: &str) -> Option<String> {
    let l = line.trim();
    let rest = l.strip_prefix("- ")?;
    let token = rest.split_whitespace().next()?;
    if token.is_empty() {
        None
    } else {
        Some(token.trim_matches('`').to_string())
    }
}

/// Submission payload: every selected option + the free-text `Other`
/// value, if the user typed one. The artifact-view writer turns this
/// into a `## Answer (YYYY-MM-DD)` block appended to the artifact
/// body and an updated `revision_notes` on each resolution target.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ClarificationAnswer {
    /// Labels of the option(s) the user picked. For `SingleChoice`
    /// this is exactly one entry when the user picked a labelled
    /// option; empty when they only filled in `other`. For
    /// `MultiChoice` it can be zero or more.
    pub selected: Vec<String>,
    /// Free-text the user typed in the "Other" slot. Empty when the
    /// user didn't fill it in.
    pub other: String,
}

#[derive(Props, Clone, PartialEq)]
pub struct ClarificationPanelProps {
    pub clarification: Clarification,
    /// `true` while the parent is persisting the answer; disables the
    /// form so the user can't double-submit.
    #[props(default = false)]
    pub disabled: bool,
    pub on_submit: EventHandler<ClarificationAnswer>,
}

#[component]
pub fn ClarificationPanel(props: ClarificationPanelProps) -> Element {
    let clarification = props.clarification.clone();
    let on_submit = props.on_submit;
    let disabled = props.disabled;

    // Selection state. Single-choice keeps Some(index) | None;
    // multi-choice keeps a HashSet of selected indices. We model both
    // as a Vec<bool> the same length as `options` so the rendering
    // path stays a single map.
    let initial_selected: Vec<bool> = vec![false; clarification.options.len()];
    let mut selected: Signal<Vec<bool>> = use_signal(|| initial_selected);
    let mut other_text: Signal<String> = use_signal(String::new);

    let question_type = clarification.question_type;
    let options = clarification.options.clone();
    let resolution_targets = clarification.resolution_targets.clone();

    let submit_handler = {
        let options = options.clone();
        move |_| {
            let mask = selected.read().clone();
            let picked: Vec<String> = options
                .iter()
                .enumerate()
                .filter_map(|(i, label)| if mask.get(i).copied().unwrap_or(false) {
                    Some(label.clone())
                } else {
                    None
                })
                .collect();
            let other = other_text.read().trim().to_string();
            on_submit.call(ClarificationAnswer {
                selected: picked,
                other,
            });
        }
    };

    let has_selection = selected.read().iter().any(|b| *b)
        || !other_text.read().trim().is_empty();

    rsx! {
        div {
            class: "operon-clarification-panel",
            "data-testid": "clarification-panel",
            "data-clarification-type": match question_type {
                QuestionType::SingleChoice => "single",
                QuestionType::MultiChoice => "multi",
            },
            div {
                class: "operon-clarification-panel-header",
                span {
                    class: "operon-clarification-panel-kind",
                    match question_type {
                        QuestionType::SingleChoice => "Single choice",
                        QuestionType::MultiChoice => "Multiple choice",
                    }
                }
                if !resolution_targets.is_empty() {
                    span {
                        class: "operon-clarification-panel-targets",
                        "Updates: "
                        for (i, slug) in resolution_targets.iter().enumerate() {
                            if i > 0 { ", " }
                            code { class: "md-inline-code", "{slug}" }
                        }
                    }
                }
            }
            ul {
                class: "operon-clarification-panel-options",
                for (idx, label) in options.iter().enumerate() {
                    li {
                        class: "operon-clarification-panel-option",
                        label {
                            input {
                                r#type: match question_type {
                                    QuestionType::SingleChoice => "radio",
                                    QuestionType::MultiChoice => "checkbox",
                                },
                                name: "clarification-option",
                                disabled: disabled,
                                checked: selected.read().get(idx).copied().unwrap_or(false),
                                onchange: move |evt| {
                                    let checked = evt.value() == "true" || evt.checked();
                                    selected.with_mut(|m| {
                                        match question_type {
                                            QuestionType::SingleChoice => {
                                                for entry in m.iter_mut() {
                                                    *entry = false;
                                                }
                                                if let Some(slot) = m.get_mut(idx) {
                                                    *slot = checked;
                                                }
                                            }
                                            QuestionType::MultiChoice => {
                                                if let Some(slot) = m.get_mut(idx) {
                                                    *slot = checked;
                                                }
                                            }
                                        }
                                    });
                                },
                            }
                            span { class: "operon-clarification-panel-option-label", "{label}" }
                        }
                    }
                }
            }
            div {
                class: "operon-clarification-panel-other",
                label {
                    class: "operon-clarification-panel-other-label",
                    "Other:"
                    input {
                        r#type: "text",
                        class: "operon-clarification-panel-other-input",
                        placeholder: "Type a custom answer\u{2026}",
                        disabled: disabled,
                        value: "{other_text.read()}",
                        oninput: move |evt| other_text.set(evt.value()),
                    }
                }
            }
            div {
                class: "operon-clarification-panel-actions",
                button {
                    r#type: "button",
                    class: "operon-clarification-panel-submit",
                    "data-testid": "clarification-panel-submit",
                    disabled: disabled || !has_selection,
                    onclick: submit_handler,
                    "Submit answer"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_extracts_single_choice_with_options_and_target() {
        let body = "\
# Clarification: scope of analytics\n\
\n\
## Levels involved\n\
- epic-02-analytics [A1]\n\
- feature-04-dashboard [A2]\n\
\n\
## The discrepancy\n\
prose...\n\
\n\
## Question type\n\
single_choice\n\
\n\
## Options\n\
- [ ] Keep analytics in scope \u{2014} ship in v1\n\
- [ ] Drop analytics from v1\n\
- [ ] Other: ___\n\
\n\
## Resolution target\n\
- epic-02-analytics\n\
- feature-04-dashboard\n\
";
        let c = parse_clarification(body);
        assert_eq!(c.question_type, QuestionType::SingleChoice);
        assert_eq!(c.options.len(), 2);
        assert!(c.options[0].starts_with("Keep analytics"));
        assert!(c.options[1].starts_with("Drop analytics"));
        assert!(c.has_other);
        assert_eq!(c.resolution_targets, vec![
            "epic-02-analytics".to_string(),
            "feature-04-dashboard".to_string()
        ]);
    }

    #[test]
    fn parse_detects_multi_choice() {
        let body = "## Question type\nmulti_choice\n\n## Options\n- [ ] A\n- [ ] B\n";
        let c = parse_clarification(body);
        assert_eq!(c.question_type, QuestionType::MultiChoice);
        assert_eq!(c.options, vec!["A".to_string(), "B".to_string()]);
        assert!(!c.has_other);
    }

    #[test]
    fn parse_handles_missing_sections() {
        let c = parse_clarification("# Clarification: empty\n");
        assert_eq!(c.question_type, QuestionType::SingleChoice);
        assert!(c.options.is_empty());
        assert!(c.resolution_targets.is_empty());
        assert!(!c.has_other);
    }

    #[test]
    fn parse_accepts_alternate_other_phrasings() {
        for raw_other in [
            "Other: ___",
            "Other",
            "Other \u{2014} custom",
            "other: please specify",
        ] {
            let body = format!(
                "## Options\n- [ ] Pick A\n- [ ] {raw_other}\n",
            );
            let c = parse_clarification(&body);
            assert_eq!(c.options, vec!["Pick A".to_string()], "raw_other={raw_other:?}");
            assert!(c.has_other, "raw_other={raw_other:?}");
        }
    }
}
