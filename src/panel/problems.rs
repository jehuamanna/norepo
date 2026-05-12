//! Problems tab body — renders entries from the global
//! [`crate::problems::PROBLEMS`] buffer in chronological order
//! (oldest at top, newest at bottom). Empty state shows "No
//! problems yet."

use dioxus::prelude::*;

use crate::log::format_ts;
use crate::problems::PROBLEMS;

const VISIBLE_TAIL: usize = 200;

#[component]
pub fn ProblemsView() -> Element {
    let snap = PROBLEMS.read().snapshot();
    let total = snap.len();
    let start = total.saturating_sub(VISIBLE_TAIL);
    let visible = snap[start..].to_vec();

    rsx! {
        div { class: "operon-logs",
            if visible.is_empty() {
                div { class: "operon-logs-empty", "No problems yet." }
            }
            for problem in visible.into_iter() {
                {
                    let ts_text = format_ts(problem.ts);
                    let source_text = problem.source.label();
                    let label_text = problem.label.clone().unwrap_or_default();
                    let message = problem.message.clone();
                    rsx! {
                        div { class: "operon-logs-row",
                            span { class: "operon-logs-ts", "{ts_text}" }
                            span {
                                class: "operon-logs-level operon-logs-level-error",
                                "[{source_text}]"
                            }
                            if !label_text.is_empty() {
                                span { class: "operon-logs-level", "{label_text}" }
                            }
                            span { class: "operon-logs-message", "{message}" }
                        }
                    }
                }
            }
        }
    }
}
