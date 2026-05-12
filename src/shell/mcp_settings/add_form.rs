//! Inline "Add MCP server" form. Renders all fields needed for the
//! three transports (stdio / sse / http) and submits via `McpService::add`.

#![cfg(not(target_arch = "wasm32"))]

use dioxus::prelude::*;

use crate::shell::companion_state::ActiveRepoPath;
use crate::shell::mcp_settings::{AddArgs, McpServiceCtx, Scope, Transport};
use crate::shell::permission_persist::{append_allow_rule, mcp_wildcard_rule};

#[derive(Props, Clone, PartialEq)]
pub struct AddFormProps {
    pub initial_scope: Scope,
    /// Fired when the form closes. `Some(msg)` → reload + toast; `None`
    /// → cancelled.
    pub on_done: EventHandler<Option<String>>,
}

#[component]
pub fn AddForm(props: AddFormProps) -> Element {
    let McpServiceCtx(service) = use_context();
    let ActiveRepoPath(active_repo) = use_context();

    let mut scope: Signal<Scope> = use_signal(|| props.initial_scope);
    let mut transport: Signal<Transport> = use_signal(|| Transport::Stdio);
    let mut name: Signal<String> = use_signal(String::new);
    let mut command_or_url: Signal<String> = use_signal(String::new);
    let mut args_text: Signal<String> = use_signal(String::new);
    let env_rows: Signal<Vec<(String, String)>> = use_signal(Vec::new);
    let header_rows: Signal<Vec<(String, String)>> = use_signal(Vec::new);
    let mut submitting: Signal<bool> = use_signal(|| false);
    let mut err: Signal<Option<String>> = use_signal(|| None);
    // Default-on: when the user adds an MCP, also append `mcp__<name>__*`
    // to the active repo's `.claude/settings.local.json` allow list so
    // skill runs (which run in `acceptEdits` mode and don't auto-approve
    // MCP tool calls) stop blocking on permission prompts. Toggleable
    // per-add so the user can opt out for a particular server.
    let mut auto_allow: Signal<bool> = use_signal(|| true);

    let on_cancel = {
        let on_done = props.on_done;
        move |_| on_done.call(None)
    };

    let on_save = {
        let service = service.clone();
        let on_done = props.on_done;
        move |_| {
            if name.read().trim().is_empty() {
                err.set(Some("Name is required.".into()));
                return;
            }
            if command_or_url.read().trim().is_empty() {
                err.set(Some(match *transport.read() {
                    Transport::Stdio => "Command is required for stdio.".into(),
                    Transport::Sse => "URL is required for SSE.".into(),
                    Transport::Http => "URL is required for HTTP.".into(),
                }));
                return;
            }
            let args = AddArgs {
                scope: *scope.read(),
                transport: *transport.read(),
                name: name.read().trim().to_string(),
                command_or_url: command_or_url.read().trim().to_string(),
                args: split_args(&args_text.read()),
                env: env_rows
                    .read()
                    .iter()
                    .filter(|(k, _)| !k.trim().is_empty())
                    .cloned()
                    .collect(),
                headers: header_rows
                    .read()
                    .iter()
                    .filter(|(k, _)| !k.trim().is_empty())
                    .cloned()
                    .collect(),
            };
            let service = service.clone();
            let on_done = on_done;
            let saved_name = args.name.clone();
            let cwd = active_repo.read().clone();
            let allow_after_add = *auto_allow.read();
            submitting.set(true);
            err.set(None);
            spawn(async move {
                match service.add(&args, cwd.as_deref()).await {
                    Ok(()) => {
                        let mut toast = format!("Added `{saved_name}`.");
                        if allow_after_add {
                            if let (Some(repo), Some(rule)) =
                                (cwd.as_ref(), mcp_wildcard_rule(&saved_name))
                            {
                                match append_allow_rule(repo, &rule) {
                                    Ok(_) => {
                                        toast.push_str(&format!(
                                            " Allow-listed `{rule}` in this repo."
                                        ));
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            target: "operon::mcp",
                                            "auto-allow {rule} in {}: {e}",
                                            repo.display()
                                        );
                                        toast.push_str(
                                            " (couldn't auto-allow — see logs)",
                                        );
                                    }
                                }
                            }
                        }
                        on_done.call(Some(toast));
                    }
                    Err(e) => {
                        submitting.set(false);
                        err.set(Some(e));
                    }
                }
            });
        }
    };

    let is_stdio = matches!(*transport.read(), Transport::Stdio);
    let url_or_cmd_label = if is_stdio { "Command" } else { "URL" };
    let url_or_cmd_placeholder = if is_stdio {
        "/usr/local/bin/uvx"
    } else {
        "https://example.com/mcp"
    };

    rsx! {
        div { class: "operon-mcp-add-form", "data-testid": "mcp-add-form",
            h3 { class: "operon-modal-section", "Add MCP server" }
            div { class: "operon-mcp-form-row",
                label { class: "operon-modal-label", "Scope" }
                select {
                    class: "operon-modal-input",
                    "data-testid": "mcp-add-scope",
                    onchange: move |e| {
                        scope.set(match e.value().as_str() {
                            "user" => Scope::User,
                            "project" => Scope::Project,
                            _ => Scope::Local,
                        });
                    },
                    option { value: "local", selected: matches!(*scope.read(), Scope::Local), "{Scope::Local.label()}" }
                    option { value: "user", selected: matches!(*scope.read(), Scope::User), "{Scope::User.label()}" }
                    option { value: "project", selected: matches!(*scope.read(), Scope::Project), "{Scope::Project.label()}" }
                }
            }
            div { class: "operon-mcp-form-row",
                label { class: "operon-modal-label", "Transport" }
                select {
                    class: "operon-modal-input",
                    "data-testid": "mcp-add-transport",
                    onchange: move |e| {
                        transport.set(match e.value().as_str() {
                            "sse" => Transport::Sse,
                            "http" => Transport::Http,
                            _ => Transport::Stdio,
                        });
                    },
                    option { value: "stdio", selected: matches!(*transport.read(), Transport::Stdio), "stdio" }
                    option { value: "sse", selected: matches!(*transport.read(), Transport::Sse), "sse" }
                    option { value: "http", selected: matches!(*transport.read(), Transport::Http), "http" }
                }
            }
            div { class: "operon-mcp-form-row",
                label { class: "operon-modal-label", "Name" }
                input {
                    r#type: "text",
                    class: "operon-modal-input",
                    "data-testid": "mcp-add-name",
                    placeholder: "my-server",
                    value: "{name.read()}",
                    oninput: move |e| name.set(e.value()),
                }
            }
            div { class: "operon-mcp-form-row",
                label { class: "operon-modal-label", "{url_or_cmd_label}" }
                input {
                    r#type: "text",
                    class: "operon-modal-input",
                    "data-testid": "mcp-add-cmd",
                    placeholder: "{url_or_cmd_placeholder}",
                    value: "{command_or_url.read()}",
                    oninput: move |e| command_or_url.set(e.value()),
                }
            }
            if is_stdio {
                div { class: "operon-mcp-form-row",
                    label { class: "operon-modal-label", "Args (space-separated)" }
                    input {
                        r#type: "text",
                        class: "operon-modal-input",
                        "data-testid": "mcp-add-args",
                        placeholder: "mcp-server-time --utc",
                        value: "{args_text.read()}",
                        oninput: move |e| args_text.set(e.value()),
                    }
                }
                KvRows {
                    title: "Environment",
                    rows: env_rows,
                    placeholder_key: "API_KEY",
                    placeholder_value: "secret",
                    test_id_prefix: "mcp-add-env",
                }
            } else {
                KvRows {
                    title: "Headers",
                    rows: header_rows,
                    placeholder_key: "Authorization",
                    placeholder_value: "Bearer …",
                    test_id_prefix: "mcp-add-header",
                }
            }
            div { class: "operon-mcp-form-row",
                label { class: "operon-modal-label", "Permissions" }
                label { class: "operon-mcp-form-toggle",
                    input {
                        r#type: "checkbox",
                        "data-testid": "mcp-add-auto-allow",
                        checked: *auto_allow.read(),
                        onchange: move |e| auto_allow.set(e.checked()),
                    }
                    " Allow all tools from this server in the active repo"
                }
            }
            if let Some(msg) = err.read().clone() {
                p { class: "operon-modal-error", "{msg}" }
            }
            div { class: "operon-modal-actions",
                button {
                    r#type: "button",
                    class: "operon-modal-button",
                    disabled: *submitting.read(),
                    onclick: on_cancel,
                    "Cancel"
                }
                button {
                    r#type: "button",
                    class: "operon-modal-button operon-modal-button-primary",
                    "data-testid": "mcp-add-save",
                    disabled: *submitting.read(),
                    onclick: on_save,
                    { if *submitting.read() { "Adding…" } else { "Add" } }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct KvRowsProps {
    title: &'static str,
    rows: Signal<Vec<(String, String)>>,
    placeholder_key: &'static str,
    placeholder_value: &'static str,
    test_id_prefix: &'static str,
}

#[component]
fn KvRows(props: KvRowsProps) -> Element {
    let mut rows = props.rows;
    let row_count = rows.read().len();
    rsx! {
        div { class: "operon-mcp-form-kv",
            label { class: "operon-modal-label", "{props.title}" }
            for i in 0..row_count {
                div { class: "operon-mcp-kv-row", key: "{i}",
                    input {
                        r#type: "text",
                        class: "operon-modal-input operon-mcp-kv-key",
                        "data-testid": "{props.test_id_prefix}-key-{i}",
                        placeholder: "{props.placeholder_key}",
                        value: "{rows.read()[i].0}",
                        oninput: move |e| {
                            let mut v = rows.write();
                            if let Some(row) = v.get_mut(i) {
                                row.0 = e.value();
                            }
                        },
                    }
                    input {
                        r#type: "text",
                        class: "operon-modal-input operon-mcp-kv-val",
                        "data-testid": "{props.test_id_prefix}-val-{i}",
                        placeholder: "{props.placeholder_value}",
                        value: "{rows.read()[i].1}",
                        oninput: move |e| {
                            let mut v = rows.write();
                            if let Some(row) = v.get_mut(i) {
                                row.1 = e.value();
                            }
                        },
                    }
                    button {
                        r#type: "button",
                        class: "operon-modal-button",
                        onclick: move |_| {
                            let mut v = rows.write();
                            if i < v.len() {
                                v.remove(i);
                            }
                        },
                        "Remove"
                    }
                }
            }
            button {
                r#type: "button",
                class: "operon-modal-button",
                "data-testid": "{props.test_id_prefix}-add",
                onclick: move |_| {
                    rows.write().push((String::new(), String::new()));
                },
                "+ Add row"
            }
        }
    }
}

/// Split a free-text args field by whitespace OR commas, treating
/// quoted spans as a single token. The placeholder labels the field
/// as "space-separated" but users routinely type a comma-delimited
/// list ("-y, figma-developer-mcp, --stdio"); accepting both is more
/// forgiving than rejecting half of likely inputs. Trailing
/// punctuation on the last token (`--stdio,`) gets stripped too.
fn split_args(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_single = false;
    let mut in_double = false;
    for c in s.chars() {
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            ' ' | '\t' | ',' if !in_single && !in_double => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_args_basic() {
        assert_eq!(split_args("a b c"), vec!["a", "b", "c"]);
    }

    #[test]
    fn split_args_quoted() {
        assert_eq!(
            split_args("foo \"bar baz\" qux"),
            vec!["foo", "bar baz", "qux"]
        );
    }

    #[test]
    fn split_args_empty() {
        let v: Vec<String> = Vec::new();
        assert_eq!(split_args("   "), v);
    }

    #[test]
    fn split_args_tolerates_commas() {
        // Common user pattern: comma-separated list pasted from a
        // README or chat snippet. We accept both space- and
        // comma-delimited input so the form's "space-separated" hint
        // doesn't bite users who default to commas.
        assert_eq!(
            split_args("-y, figma-developer-mcp, --stdio"),
            vec!["-y", "figma-developer-mcp", "--stdio"]
        );
        assert_eq!(
            split_args("a,b,c"),
            vec!["a", "b", "c"]
        );
        // Trailing comma is gracefully dropped (no empty token).
        assert_eq!(
            split_args("a, b,"),
            vec!["a", "b"]
        );
    }

    #[test]
    fn split_args_keeps_quoted_commas_intact() {
        assert_eq!(
            split_args("foo \"a, b\" bar"),
            vec!["foo", "a, b", "bar"]
        );
    }
}
