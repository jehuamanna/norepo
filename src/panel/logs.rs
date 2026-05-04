//! Logs tab body — renders the most recent ~200 entries from the context-provided
//! `Signal<LogBuffer>` in chronological order (oldest at top, newest at bottom).

use dioxus::prelude::*;

use crate::log::{format_ts, LogBuffer};

const VISIBLE_TAIL: usize = 200;

#[component]
pub fn LogsView() -> Element {
    let buf: Signal<LogBuffer> = use_context();
    let snap = buf.read().snapshot();
    let total = snap.len();
    let start = total.saturating_sub(VISIBLE_TAIL);
    let visible = snap[start..].to_vec();

    rsx! {
        div { class: "operon-logs",
            if visible.is_empty() {
                div { class: "operon-logs-empty", "No log entries." }
            }
            for entry in visible.into_iter() {
                {
                    let ts_text = format_ts(entry.ts);
                    let level_text = entry.level.label();
                    let level_class = match entry.level {
                        crate::log::LogLevel::Error => "operon-logs-level operon-logs-level-error",
                        crate::log::LogLevel::Warn => "operon-logs-level operon-logs-level-warn",
                        _ => "operon-logs-level",
                    };
                    let message = entry.message;
                    rsx! {
                        div { class: "operon-logs-row",
                            span { class: "operon-logs-ts", "{ts_text}" }
                            span { class: "{level_class}", "[{level_text}]" }
                            span { class: "operon-logs-message", "{message}" }
                        }
                    }
                }
            }
        }
    }
}
