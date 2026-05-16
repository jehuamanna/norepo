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
    /// When true, the scope picker is hidden and writes are pinned to
    /// `initial_scope`. Used by the scope-locked panel modes (Global /
    /// Project) so the user can't accidentally write to a different
    /// tier than the panel claims to manage.
    #[props(default = false)]
    pub lock_scope: bool,
    /// Working directory passed to `claude mcp add`. When `None`, falls
    /// back to the active repo path from context. Project-scope writes
    /// MUST set this to the project's repo so `.mcp.json` lands in the
    /// right place; global writes leave it `None`.
    #[props(default)]
    pub cwd_override: Option<std::path::PathBuf>,
    /// Fired when the form closes. `Some(msg)` → reload + toast; `None`
    /// → cancelled.
    pub on_done: EventHandler<Option<String>>,
}

/// Whether the user is filling out the structured form or pasting a raw
/// JSON config. JSON is dispatched via `claude mcp add-json` and accepts
/// both a single server object and a full `{ "mcpServers": {...} }`
/// wrapper (matching the `.mcp.json` shape).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputMode {
    Form,
    Json,
}

#[component]
pub fn AddForm(props: AddFormProps) -> Element {
    let McpServiceCtx(service) = use_context();
    let ActiveRepoPath(active_repo) = use_context();
    let cwd_override = props.cwd_override.clone();
    let lock_scope = props.lock_scope;
    // Resolve `cwd` once per render: an explicit override (the panel
    // is scope-locked to a specific project) wins, else fall through
    // to the active repo from chat context for legacy callers.
    let resolve_cwd = {
        let cwd_override = cwd_override.clone();
        let active_repo = active_repo;
        move || -> Option<std::path::PathBuf> {
            cwd_override
                .clone()
                .or_else(|| active_repo.read().clone())
        }
    };

    let mut mode: Signal<InputMode> = use_signal(|| InputMode::Form);
    let mut scope: Signal<Scope> = use_signal(|| props.initial_scope);
    let mut transport: Signal<Transport> = use_signal(|| Transport::Stdio);
    let mut name: Signal<String> = use_signal(String::new);
    let mut command_or_url: Signal<String> = use_signal(String::new);
    let mut args_text: Signal<String> = use_signal(String::new);
    let env_rows: Signal<Vec<(String, String)>> = use_signal(Vec::new);
    let header_rows: Signal<Vec<(String, String)>> = use_signal(Vec::new);
    let mut json_text: Signal<String> = use_signal(String::new);
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
            let current_mode = *mode.read();
            err.set(None);
            match current_mode {
                InputMode::Form => {
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
                    let cwd = resolve_cwd();
                    let allow_after_add = *auto_allow.read();
                    submitting.set(true);
                    spawn(async move {
                        match service.add(&args, cwd.as_deref()).await {
                            Ok(()) => {
                                let mut toast = format!("Added `{saved_name}`.");
                                apply_auto_allow(
                                    &saved_name,
                                    cwd.as_deref(),
                                    allow_after_add,
                                    &mut toast,
                                );
                                // Reset so the form is reusable for the
                                // next add without needing to be closed.
                                submitting.set(false);
                                name.set(String::new());
                                command_or_url.set(String::new());
                                args_text.set(String::new());
                                env_rows.clone().write().clear();
                                header_rows.clone().write().clear();
                                on_done.call(Some(toast));
                            }
                            Err(e) => {
                                submitting.set(false);
                                err.set(Some(e));
                            }
                        }
                    });
                }
                InputMode::Json => {
                    let raw = json_text.read().trim().to_string();
                    if raw.is_empty() {
                        err.set(Some(
                            "Paste the JSON config or load it from a .json file.".into(),
                        ));
                        return;
                    }
                    let entries = match parse_json_entries(&raw, name.read().trim()) {
                        Ok(v) if v.is_empty() => {
                            err.set(Some(
                                "No server entries found in the JSON.".into(),
                            ));
                            return;
                        }
                        Ok(v) => v,
                        Err(e) => {
                            err.set(Some(e));
                            return;
                        }
                    };
                    let service = service.clone();
                    let on_done = on_done;
                    let cwd = resolve_cwd();
                    let chosen_scope = *scope.read();
                    let allow_after_add = *auto_allow.read();
                    submitting.set(true);
                    spawn(async move {
                        let mut added: Vec<String> = Vec::new();
                        for (entry_name, entry_json) in entries {
                            if let Err(e) = service
                                .add_json(&entry_name, chosen_scope, &entry_json, cwd.as_deref())
                                .await
                            {
                                submitting.set(false);
                                err.set(Some(format!(
                                    "Adding `{entry_name}` failed: {e}"
                                )));
                                return;
                            }
                            added.push(entry_name);
                        }
                        let mut toast = if added.len() == 1 {
                            format!("Added `{}` from JSON.", added[0])
                        } else {
                            format!(
                                "Added {} servers from JSON: {}.",
                                added.len(),
                                added.join(", ")
                            )
                        };
                        if allow_after_add {
                            for n in &added {
                                apply_auto_allow(n, cwd.as_deref(), true, &mut toast);
                            }
                        }
                        // Reset so the form is reusable for the next
                        // import without needing to be closed.
                        submitting.set(false);
                        json_text.set(String::new());
                        on_done.call(Some(toast));
                    });
                }
            }
        }
    };

    let on_load_from_file = {
        move |_| {
            spawn(async move {
                let Some(handle) = rfd::AsyncFileDialog::new()
                    .set_title("Import MCP server JSON")
                    .add_filter("JSON", &["json"])
                    .pick_file()
                    .await
                else {
                    return;
                };
                let path = handle.path().to_path_buf();
                match tokio::fs::read_to_string(&path).await {
                    Ok(s) => {
                        json_text.set(s);
                        err.set(None);
                    }
                    Err(e) => err.set(Some(format!(
                        "Could not read {}: {e}",
                        path.display()
                    ))),
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

    let is_json = matches!(*mode.read(), InputMode::Json);

    rsx! {
        div { class: "operon-mcp-add-form", "data-testid": "mcp-add-form",
            div { class: "operon-mcp-mode-tabs", role: "tablist",
                button {
                    r#type: "button",
                    role: "tab",
                    "aria-selected": "{!is_json}",
                    class: if is_json {
                        "operon-mcp-mode-tab"
                    } else {
                        "operon-mcp-mode-tab operon-mcp-mode-tab-active"
                    },
                    "data-testid": "mcp-add-mode-form",
                    onclick: move |_| mode.set(InputMode::Form),
                    "Form"
                }
                button {
                    r#type: "button",
                    role: "tab",
                    "aria-selected": "{is_json}",
                    class: if is_json {
                        "operon-mcp-mode-tab operon-mcp-mode-tab-active"
                    } else {
                        "operon-mcp-mode-tab"
                    },
                    "data-testid": "mcp-add-mode-json",
                    onclick: move |_| mode.set(InputMode::Json),
                    "JSON file"
                }
            }
            if !lock_scope {
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
            }
            if !is_json {
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
            } else {
                div { class: "operon-mcp-form-row",
                    label { class: "operon-modal-label", "Name (optional)" }
                    input {
                        r#type: "text",
                        class: "operon-modal-input",
                        "data-testid": "mcp-add-name",
                        placeholder: "used only if the JSON has a single server without one",
                        value: "{name.read()}",
                        oninput: move |e| name.set(e.value()),
                    }
                }
                div { class: "operon-mcp-form-kv",
                    label { class: "operon-modal-label", "JSON config" }
                    p { class: "operon-modal-help",
                        "Paste a `.mcp.json` (`{{\"mcpServers\": {{ … }}}}`) or a single server config."
                    }
                    textarea {
                        class: "operon-modal-input operon-mcp-json-textarea",
                        "data-testid": "mcp-add-json",
                        rows: "8",
                        placeholder: "{{\n  \"mcpServers\": {{\n    \"time\": {{\n      \"command\": \"uvx\",\n      \"args\": [\"mcp-server-time\", \"--utc\"]\n    }}\n  }}\n}}",
                        value: "{json_text.read()}",
                        oninput: move |e| json_text.set(e.value()),
                    }
                    div { class: "operon-mcp-json-actions",
                        button {
                            r#type: "button",
                            class: "operon-modal-button",
                            "data-testid": "mcp-add-json-load",
                            onclick: on_load_from_file,
                            "Load from .json file…"
                        }
                    }
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

/// Auto-allow `mcp__<name>__*` in the active repo's settings, mutating
/// `toast` to mention what happened (or note failure). No-op when
/// `enabled` is false or no repo is bound.
fn apply_auto_allow(
    server_name: &str,
    cwd: Option<&std::path::Path>,
    enabled: bool,
    toast: &mut String,
) {
    if !enabled {
        return;
    }
    let Some(repo) = cwd else { return };
    let Some(rule) = mcp_wildcard_rule(server_name) else {
        return;
    };
    match append_allow_rule(repo, &rule) {
        Ok(_) => {
            toast.push_str(&format!(" Allow-listed `{rule}` in this repo."));
        }
        Err(e) => {
            tracing::warn!(
                target: "operon::mcp",
                "auto-allow {rule} in {}: {e}",
                repo.display()
            );
            toast.push_str(" (couldn't auto-allow — see logs)");
        }
    }
}

/// Parse the JSON-import textarea into `(name, single-server JSON string)`
/// tuples ready for `claude mcp add-json`. Accepts three shapes:
///
/// 1. Full `.mcp.json` wrapper: `{ "mcpServers": { "name": {...}, ... } }`
/// 2. A bare map of `{ "name": {...}, ... }` (each value is a server cfg)
/// 3. A single server object — uses `fallback_name` if provided, or any
///    `"name"` key inside the object.
///
/// Returns `Err` with a user-facing message for malformed input.
pub(crate) fn parse_json_entries(
    raw: &str,
    fallback_name: &str,
) -> Result<Vec<(String, String)>, String> {
    let root: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| format!("Invalid JSON: {e}"))?;

    // Shape 1: { "mcpServers": { ... } }
    if let Some(servers) = root.get("mcpServers").and_then(|v| v.as_object()) {
        let mut out = Vec::with_capacity(servers.len());
        for (k, v) in servers {
            if !v.is_object() {
                return Err(format!(
                    "mcpServers.{k} is not an object — expected a server config"
                ));
            }
            out.push((k.clone(), v.to_string()));
        }
        return Ok(out);
    }

    // Shape 3: looks like a single server config (has command or url or type)
    // — we check this BEFORE the bare-map shape so that a single-server
    // object like {"command": "uvx", "args": [...]} isn't misread as a map
    // of two servers ("command" and "args").
    let obj = root.as_object().ok_or_else(|| {
        "JSON root must be an object (either a server config or a { name: config } map)"
            .to_string()
    })?;
    let looks_single = obj.contains_key("command")
        || obj.contains_key("url")
        || obj.contains_key("type")
        || obj.contains_key("transport");
    if looks_single {
        let entry_name = obj
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| fallback_name.to_string());
        if entry_name.is_empty() {
            return Err(
                "Server name is required — fill the Name field or include \"name\" in the JSON."
                    .into(),
            );
        }
        // Strip an optional top-level "name" key so the JSON we hand to
        // `claude mcp add-json` is just the config payload.
        let mut clone = obj.clone();
        clone.remove("name");
        return Ok(vec![(
            entry_name,
            serde_json::Value::Object(clone).to_string(),
        )]);
    }

    // Shape 2: bare map of name → config
    let mut out = Vec::with_capacity(obj.len());
    for (k, v) in obj {
        if !v.is_object() {
            return Err(format!(
                "`{k}` is not an object — expected a server config"
            ));
        }
        out.push((k.clone(), v.to_string()));
    }
    Ok(out)
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

    #[test]
    fn parse_json_entries_full_mcp_servers_wrapper() {
        let raw = r#"{
            "mcpServers": {
                "time": {"command": "uvx", "args": ["mcp-server-time", "--utc"]},
                "sentry": {"type": "http", "url": "https://mcp.sentry.dev/mcp"}
            }
        }"#;
        let v = parse_json_entries(raw, "").expect("should parse");
        assert_eq!(v.len(), 2);
        let names: Vec<&str> = v.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"time"));
        assert!(names.contains(&"sentry"));
    }

    #[test]
    fn parse_json_entries_single_server_uses_fallback_name() {
        let raw = r#"{"command": "uvx", "args": ["mcp-server-time"]}"#;
        let v = parse_json_entries(raw, "time").expect("should parse");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].0, "time");
        // The fallback name must NOT have been baked into the JSON payload.
        assert!(!v[0].1.contains("\"name\""));
        assert!(v[0].1.contains("\"command\""));
    }

    #[test]
    fn parse_json_entries_single_server_prefers_embedded_name() {
        let raw = r#"{"name": "embedded", "command": "uvx", "args": []}"#;
        let v = parse_json_entries(raw, "fallback").expect("should parse");
        assert_eq!(v[0].0, "embedded");
        // Embedded "name" key is stripped so claude doesn't see it as
        // part of the server config.
        assert!(!v[0].1.contains("\"name\""));
    }

    #[test]
    fn parse_json_entries_single_server_without_name_errors() {
        let raw = r#"{"command": "uvx"}"#;
        assert!(parse_json_entries(raw, "").is_err());
    }

    #[test]
    fn parse_json_entries_bare_name_map() {
        let raw = r#"{
            "time": {"command": "uvx", "args": []},
            "sentry": {"type": "http", "url": "https://x"}
        }"#;
        let v = parse_json_entries(raw, "").expect("should parse");
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn parse_json_entries_invalid_json() {
        assert!(parse_json_entries("not json", "").is_err());
    }
}
