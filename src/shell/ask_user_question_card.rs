//! Inline picker card for the custom `mcp__operon__ask_user` MCP tool.
//!
//! When the model invokes `ask_user`, the bridge's
//! [`crate::shell::bridge_ask_user_executor::BridgeAskUserExecutor`]
//! parks a oneshot responder and pushes an [`AskUserPromptEntry`] onto
//! `ASK_USER_PROMPTS`. The companion-chat surface iterates that signal
//! and renders one of these cards per pending prompt. The Submit
//! button collects the selected option(s) for every question and calls
//! [`submit_ask_user_answers`], which resolves the parked oneshot —
//! Claude then receives the structured `{questions, answers}` payload
//! and continues the turn.
//!
//! Visual model mirrors [`crate::shell::permission_prompt::PermissionPrompt`]
//! so the two card families look like siblings in the chat surface.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::HashMap;

use dioxus::prelude::*;
use serde_json::Value;

use crate::shell::companion_state::{
    cancel_ask_user_prompt, submit_ask_user_answers, AskUserPromptEntry, AskUserStatus,
    ASK_USER_RESOLVED_ANSWERS,
};

#[derive(Clone, PartialEq, Props)]
pub struct AskUserPromptCardProps {
    pub entry: AskUserPromptEntry,
    pub status: AskUserStatus,
}

/// Per-question UI state. For single-select we track the chosen
/// label; for multiSelect we track a set of labels. The free-text
/// "Other…" field is a separate string — non-empty values win over
/// (or extend, for multiSelect) the option labels.
#[derive(Clone, Debug, Default)]
struct QuestionPick {
    single: Option<String>,
    multi: Vec<String>,
    other: String,
}

#[component]
pub fn AskUserPromptCard(props: AskUserPromptCardProps) -> Element {
    let entry = props.entry;
    let status = props.status;
    let pending = matches!(status, AskUserStatus::Pending);

    let questions = entry.questions.as_array().cloned().unwrap_or_default();
    // Per-question selection state, keyed by question index. Initial
    // map is empty; rendering reads via `.get(&i)`.
    let mut picks = use_signal(|| HashMap::<usize, QuestionPick>::new());

    // Are all questions answered? Submit button gates on this.
    let questions_for_check = questions.clone();
    let all_answered = move || -> bool {
        let snap = picks.read();
        questions_for_check.iter().enumerate().all(|(i, q)| {
            let multi = q
                .get("multiSelect")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let entry = snap.get(&i);
            let other = entry.map(|p| p.other.trim().to_string()).unwrap_or_default();
            if !other.is_empty() {
                return true;
            }
            if multi {
                entry.map(|p| !p.multi.is_empty()).unwrap_or(false)
            } else {
                entry
                    .and_then(|p| p.single.as_ref())
                    .map(|s| !s.is_empty())
                    .unwrap_or(false)
            }
        })
    };

    let questions_for_submit = questions.clone();
    let entry_id_for_submit = entry.id.clone();
    let questions_for_cancel = questions.clone();

    let submit = move |_evt: MouseEvent| {
        // Build the answers map: { <question text>: <label or array> }.
        let snap = picks.read().clone();
        let mut answers = serde_json::Map::new();
        for (i, q) in questions_for_submit.iter().enumerate() {
            let qtext = q
                .get("question")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let multi = q
                .get("multiSelect")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let pick = snap.get(&i);
            let other = pick
                .map(|p| p.other.trim().to_string())
                .unwrap_or_default();
            let value = if multi {
                let mut labels = pick.map(|p| p.multi.clone()).unwrap_or_default();
                if !other.is_empty() {
                    labels.push(other);
                }
                Value::Array(labels.into_iter().map(Value::String).collect())
            } else if !other.is_empty() {
                Value::String(other)
            } else {
                Value::String(pick.and_then(|p| p.single.clone()).unwrap_or_default())
            };
            answers.insert(qtext, value);
        }
        submit_ask_user_answers(&entry_id_for_submit, Value::Object(answers));
    };

    let entry_id_for_cancel = entry.id.clone();
    let cancel = move |_evt: MouseEvent| {
        let _ = questions_for_cancel.len(); // keep moved
        cancel_ask_user_prompt(&entry_id_for_cancel);
    };

    let resolved_answers = if !pending {
        ASK_USER_RESOLVED_ANSWERS.read().get(&entry.id).cloned()
    } else {
        None
    };

    rsx! {
        div {
            class: "operon-ask-user-card",
            "data-testid": "ask-user-card",
            "data-status": match status {
                AskUserStatus::Pending => "pending",
                AskUserStatus::Answered => "answered",
                AskUserStatus::Cancelled => "cancelled",
            },
            div {
                class: "operon-ask-user-header",
                span { class: "operon-ask-user-chip", "ask_user" }
                span {
                    class: "operon-ask-user-status",
                    {match status {
                        AskUserStatus::Pending => "awaiting answer",
                        AskUserStatus::Answered => "answered",
                        AskUserStatus::Cancelled => "cancelled",
                    }}
                }
            }
            for (qi, q) in questions.iter().cloned().enumerate() {
                {render_question(qi, q, pending, picks, resolved_answers.as_ref())}
            }
            if pending {
                div {
                    class: "operon-ask-user-actions",
                    button {
                        class: "operon-ask-user-submit",
                        disabled: !all_answered(),
                        onclick: submit,
                        "Submit"
                    }
                    button {
                        class: "operon-ask-user-cancel",
                        onclick: cancel,
                        "Cancel"
                    }
                }
            }
        }
    }
}

fn render_question(
    qi: usize,
    q: Value,
    pending: bool,
    mut picks: Signal<HashMap<usize, QuestionPick>>,
    resolved_answers: Option<&Value>,
) -> Element {
    let qtext = q
        .get("question")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let header = q
        .get("header")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let multi = q
        .get("multiSelect")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let options = q
        .get("options")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Resolved-state lookup: what did the user pick last time?
    let chosen_summary: Option<String> = resolved_answers.and_then(|m| {
        m.get(&qtext).map(|v| match v {
            Value::String(s) => s.clone(),
            Value::Array(arr) => arr
                .iter()
                .filter_map(|x| x.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            other => other.to_string(),
        })
    });

    rsx! {
        div {
            class: "operon-ask-user-question",
            div {
                class: "operon-ask-user-question-head",
                if !header.is_empty() {
                    span { class: "operon-ask-user-question-chip", "{header}" }
                }
                span { class: "operon-ask-user-question-text", "{qtext}" }
            }
            if let Some(summary) = chosen_summary {
                div {
                    class: "operon-ask-user-resolved-pick",
                    "Chosen: {summary}"
                }
            } else {
                div {
                    class: "operon-ask-user-options",
                    for (oi, opt) in options.iter().cloned().enumerate() {
                        {render_option(qi, oi, opt, multi, pending, picks)}
                    }
                    if pending {
                        div {
                            class: "operon-ask-user-other",
                            input {
                                r#type: "text",
                                placeholder: "Other…",
                                value: "{picks.read().get(&qi).map(|p| p.other.clone()).unwrap_or_default()}",
                                oninput: move |evt| {
                                    let v = evt.value();
                                    picks.with_mut(|m| {
                                        m.entry(qi).or_default().other = v;
                                    });
                                },
                            }
                        }
                    }
                }
            }
        }
    }
}

fn render_option(
    qi: usize,
    oi: usize,
    opt: Value,
    multi: bool,
    pending: bool,
    mut picks: Signal<HashMap<usize, QuestionPick>>,
) -> Element {
    let label = opt
        .get("label")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let description = opt
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let label_for_change = label.clone();

    // Determine whether this option is currently checked.
    let is_checked = {
        let snap = picks.read();
        let pick = snap.get(&qi);
        if multi {
            pick.map(|p| p.multi.iter().any(|l| l == &label))
                .unwrap_or(false)
        } else {
            pick.and_then(|p| p.single.as_ref())
                .map(|s| s == &label)
                .unwrap_or(false)
        }
    };

    let input_type = if multi { "checkbox" } else { "radio" };
    let group_name = format!("ask-user-q-{qi}");
    let opt_id = format!("ask-user-q-{qi}-opt-{oi}");
    // Click handler: toggle for multi-select, set for single-select.
    // Used on both the label and the input so either surface works,
    // and we don't need to read the `checked` attribute back off the
    // form event (which the Dioxus event API doesn't expose
    // consistently across input types).
    let on_click = move |_evt: MouseEvent| {
        if !pending {
            return;
        }
        let label = label_for_change.clone();
        picks.with_mut(|m| {
            let entry = m.entry(qi).or_default();
            if multi {
                if let Some(pos) = entry.multi.iter().position(|l| l == &label) {
                    entry.multi.remove(pos);
                } else {
                    entry.multi.push(label);
                }
            } else {
                entry.single = Some(label);
            }
        });
    };

    rsx! {
        label {
            class: "operon-ask-user-option",
            r#for: "{opt_id}",
            onclick: on_click,
            input {
                id: "{opt_id}",
                r#type: "{input_type}",
                name: "{group_name}",
                checked: is_checked,
                disabled: !pending,
                // No onchange — the parent label's onclick already
                // owns the state mutation. Marking it readonly-ish
                // avoids double-fires when the browser also dispatches
                // the input's own change event.
                onclick: move |evt: MouseEvent| { evt.stop_propagation(); },
            }
            span { class: "operon-ask-user-option-label", "{label}" }
            if !description.is_empty() {
                span { class: "operon-ask-user-option-desc", " — {description}" }
            }
        }
    }
}

