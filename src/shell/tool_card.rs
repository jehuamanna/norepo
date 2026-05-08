//! Collapsible card for a single Claude Code tool invocation.
//!
//! Each card shows a one-line summary closed (icon + tool + key argument)
//! and the full input + result on expand. Result rendering is per-tool —
//! `Read`/`Write` show the file body, `Edit` shows the unified diff,
//! `Bash` shows stdout/stderr, `Glob`/`Grep` show match listings, anything
//! else falls back to pretty JSON.

#![cfg(not(target_arch = "wasm32"))]

use dioxus::prelude::*;
use serde_json::Value;

#[derive(Clone, PartialEq, Debug)]
pub struct ToolResultBody {
    pub content: String,
    pub is_error: bool,
}

#[derive(Props, Clone, PartialEq)]
pub struct ToolCardProps {
    /// claude's tool_use id (matches a later tool_result.tool_use_id)
    pub id: String,
    pub name: String,
    pub input: Value,
    /// `None` means the tool is still running; `Some(_)` means complete.
    pub result: Option<ToolResultBody>,
}

#[component]
pub fn ToolCard(props: ToolCardProps) -> Element {
    let summary = summarize_tool(&props.name, &props.input);
    let icon = tool_icon(&props.name);
    let pending = props.result.is_none();
    let is_error = props.result.as_ref().map(|r| r.is_error).unwrap_or(false);

    let status_class = if pending {
        "operon-tool-card-status-pending"
    } else if is_error {
        "operon-tool-card-status-error"
    } else {
        "operon-tool-card-status-ok"
    };

    rsx! {
        details {
            class: "operon-tool-card",
            "data-testid": "tool-card",
            "data-tool-name": "{props.name}",
            "data-tool-id": "{props.id}",
            "data-pending": if pending { "true" } else { "false" },
            "data-error": if is_error { "true" } else { "false" },
            summary { class: "operon-tool-card-summary",
                span { class: "operon-tool-card-icon", "{icon}" }
                span { class: "operon-tool-card-name", "{props.name}" }
                span { class: "operon-tool-card-arg truncate", "{summary}" }
                span { class: "operon-tool-card-status {status_class}",
                    if pending {
                        "running\u{2026}"
                    } else if is_error {
                        "error"
                    } else {
                        "done"
                    }
                }
            }
            div { class: "operon-tool-card-body",
                ToolBody {
                    name: props.name.clone(),
                    input: props.input.clone(),
                    result: props.result.clone(),
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct ToolBodyProps {
    name: String,
    input: Value,
    result: Option<ToolResultBody>,
}

#[component]
fn ToolBody(props: ToolBodyProps) -> Element {
    match props.name.as_str() {
        "Read" => render_read(&props.input, props.result.as_ref()),
        "Write" => render_write(&props.input, props.result.as_ref()),
        "Edit" => render_edit(&props.input, props.result.as_ref()),
        "Bash" => render_bash(&props.input, props.result.as_ref()),
        "Glob" => render_glob(&props.input, props.result.as_ref()),
        "Grep" => render_grep(&props.input, props.result.as_ref()),
        _ => render_generic(&props.input, props.result.as_ref()),
    }
}

fn tool_icon(name: &str) -> &'static str {
    match name {
        "Read" => "\u{1F4C4}",       // 📄
        "Write" => "\u{1F4DD}",      // 📝
        "Edit" => "\u{270F}\u{FE0F}", // ✏️
        "Bash" => "\u{25B6}",        // ▶
        "Glob" => "\u{1F50D}",       // 🔍
        "Grep" => "\u{1F50E}",       // 🔎
        "TodoWrite" => "\u{2705}",   // ✅
        "Task" => "\u{1F916}",       // 🤖
        "WebFetch" => "\u{1F310}",   // 🌐
        "WebSearch" => "\u{1F50E}",  // 🔎
        _ => "\u{2699}\u{FE0F}",     // ⚙️
    }
}

fn summarize_tool(name: &str, input: &Value) -> String {
    match name {
        "Read" | "Write" | "Edit" => input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "Bash" => {
            let desc = input.get("description").and_then(|v| v.as_str());
            let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
            desc.map(|d| d.to_string())
                .unwrap_or_else(|| cmd.lines().next().unwrap_or("").to_string())
        }
        "Glob" | "Grep" => {
            let pat = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            let path = input
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if path.is_empty() {
                pat.to_string()
            } else {
                format!("{pat} in {path}")
            }
        }
        "TodoWrite" => "todos".to_string(),
        "Task" => input
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("subagent")
            .to_string(),
        _ => input
            .as_object()
            .and_then(|o| o.iter().next())
            .map(|(k, v)| format!("{k}={}", short_value(v)))
            .unwrap_or_default(),
    }
}

fn short_value(v: &Value) -> String {
    let s = match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if s.chars().count() > 60 {
        let mut head: String = s.chars().take(60).collect();
        head.push('\u{2026}');
        head
    } else {
        s
    }
}

fn render_read(input: &Value, result: Option<&ToolResultBody>) -> Element {
    let path = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
    let offset = input.get("offset").and_then(|v| v.as_u64());
    let limit = input.get("limit").and_then(|v| v.as_u64());
    rsx! {
        div { class: "operon-tool-card-meta",
            span { "path: " code { class: "md-inline-code", "{path}" } }
            if let Some(o) = offset { span { " offset: " code { class: "md-inline-code", "{o}" } } }
            if let Some(l) = limit { span { " limit: " code { class: "md-inline-code", "{l}" } } }
        }
        ResultPre { result: result.cloned() }
    }
}

fn render_write(input: &Value, result: Option<&ToolResultBody>) -> Element {
    let path = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
    let content = input
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let preview: String = content.lines().take(20).collect::<Vec<_>>().join("\n");
    rsx! {
        div { class: "operon-tool-card-meta",
            span { "path: " code { class: "md-inline-code", "{path}" } }
        }
        pre { class: "operon-tool-card-pre", code { class: "md-code-block", "{preview}" } }
        ResultPre { result: result.cloned() }
    }
}

fn render_edit(input: &Value, result: Option<&ToolResultBody>) -> Element {
    let path = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
    let old = input.get("old_string").and_then(|v| v.as_str()).unwrap_or("");
    let new_s = input.get("new_string").and_then(|v| v.as_str()).unwrap_or("");
    rsx! {
        div { class: "operon-tool-card-meta",
            span { "path: " code { class: "md-inline-code", "{path}" } }
        }
        div { class: "operon-tool-card-diff",
            pre { class: "operon-tool-card-pre operon-tool-card-diff-old",
                code { class: "md-code-block", "{old}" }
            }
            pre { class: "operon-tool-card-pre operon-tool-card-diff-new",
                code { class: "md-code-block", "{new_s}" }
            }
        }
        ResultPre { result: result.cloned() }
    }
}

fn render_bash(input: &Value, result: Option<&ToolResultBody>) -> Element {
    let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
    rsx! {
        div { class: "operon-tool-card-meta",
            span { "$ " code { class: "md-inline-code", "{cmd}" } }
        }
        ResultPre { result: result.cloned() }
    }
}

fn render_glob(input: &Value, result: Option<&ToolResultBody>) -> Element {
    let pat = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
    let path = input.get("path").and_then(|v| v.as_str()).unwrap_or("");
    rsx! {
        div { class: "operon-tool-card-meta",
            span { "glob: " code { class: "md-inline-code", "{pat}" } }
            if !path.is_empty() { span { " in " code { class: "md-inline-code", "{path}" } } }
        }
        ResultPre { result: result.cloned() }
    }
}

fn render_grep(input: &Value, result: Option<&ToolResultBody>) -> Element {
    let pat = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
    let path = input.get("path").and_then(|v| v.as_str()).unwrap_or("");
    rsx! {
        div { class: "operon-tool-card-meta",
            span { "grep: " code { class: "md-inline-code", "{pat}" } }
            if !path.is_empty() { span { " in " code { class: "md-inline-code", "{path}" } } }
        }
        ResultPre { result: result.cloned() }
    }
}

fn render_generic(input: &Value, result: Option<&ToolResultBody>) -> Element {
    let pretty = serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string());
    rsx! {
        pre { class: "operon-tool-card-pre",
            code { class: "md-code-block", "{pretty}" }
        }
        ResultPre { result: result.cloned() }
    }
}

#[derive(Props, Clone, PartialEq)]
struct ResultPreProps {
    result: Option<ToolResultBody>,
}

#[component]
fn ResultPre(props: ResultPreProps) -> Element {
    match props.result {
        None => rsx! {
            div { class: "operon-tool-card-result-pending", "(running)" }
        },
        Some(body) => {
            let class = if body.is_error {
                "operon-tool-card-pre operon-tool-card-result-error"
            } else {
                "operon-tool-card-pre operon-tool-card-result-ok"
            };
            rsx! {
                pre { class: "{class}",
                    code { class: "md-code-block", "{body.content}" }
                }
            }
        }
    }
}
