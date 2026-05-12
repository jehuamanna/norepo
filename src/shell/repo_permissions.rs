//! Tools → Repo Permissions panel.
//!
//! Per-repo audit + edit UI for the `permissions.allow` array in each
//! repo's `.claude/settings.local.json`. Lists every project Operon
//! knows about (via `LocalProjectRepository`) with a repo path bound,
//! and for each:
//!   - shows the current allow rules,
//!   - lets the user revoke a rule (× button),
//!   - lets the user grant an MCP server via the shared
//!     `permission_persist::mcp_wildcard_rule` helper (so the rule
//!     shape matches what Claude already understands), and
//!   - exposes a "Custom rule" escape hatch for power-users who want to
//!     hand-author a `Bash(...)` / `Edit` / etc. rule.
//!
//! Storage is entirely per-repo — we read/write the same files Claude
//! reads natively. No SQLite, no app-level allowlist.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::HashMap;
use std::path::PathBuf;

use dioxus::prelude::*;

use crate::local_mode::desktop::LocalProjectRepo;
use crate::shell::permission_persist::{
    append_allow_rule, mcp_wildcard_rule, read_allow_rules, remove_allow_rule,
};

/// App-scope visibility signal for the Repo Permissions panel. Provided
/// in `App` and flipped by the `tools.openRepoPermissions` command. The
/// panel owns the close write (Esc / scrim click / Close button).
#[derive(Clone, Copy)]
pub struct RepoPermissionsOpen(pub Signal<bool>);

/// One row in the panel's render — a bound repo plus its current allow
/// rules. Unbound projects (no `repo_path`) are filtered out before this
/// shape is built; nothing to allow-list against.
#[derive(Clone, Debug, PartialEq)]
struct RepoRow {
    name: String,
    path: PathBuf,
    rules: Vec<String>,
}

#[component]
pub fn RepoPermissionsPanel() -> Element {
    let RepoPermissionsOpen(mut open) = use_context();
    if !*open.read() {
        return rsx! {};
    }

    let LocalProjectRepo(project_repo) = use_context();

    // Per-repo refresh trigger: bumping this signal causes the
    // memo-style `rows` recompute to re-read each repo's
    // settings.local.json. We bump it after every mutation (add /
    // remove) and on initial mount.
    let mut refresh_token: Signal<u64> = use_signal(|| 0u64);
    // Per-repo expand/collapse state. Default: collapsed.
    let mut expanded: Signal<HashMap<PathBuf, bool>> =
        use_signal(HashMap::<PathBuf, bool>::new);
    // Per-repo "Add MCP server" form text. One field per repo because the
    // user may type into several before committing.
    let mut mcp_drafts: Signal<HashMap<PathBuf, String>> =
        use_signal(HashMap::<PathBuf, String>::new);
    // Per-repo "Add custom rule" form text.
    let mut custom_drafts: Signal<HashMap<PathBuf, String>> =
        use_signal(HashMap::<PathBuf, String>::new);
    // Last error per repo (e.g. invalid MCP server name, IO failure).
    // Surfaced inline next to the offending input.
    let mut errors: Signal<HashMap<PathBuf, String>> =
        use_signal(HashMap::<PathBuf, String>::new);

    let rows: Vec<RepoRow> = {
        let _ = refresh_token.read();
        match project_repo.list() {
            Ok(projects) => projects
                .into_iter()
                .filter_map(|p| {
                    let path = p.repo_path.clone()?;
                    let rules = read_allow_rules(&path).unwrap_or_default();
                    Some(RepoRow {
                        name: p.name,
                        path,
                        rules,
                    })
                })
                .collect(),
            Err(e) => {
                tracing::warn!(
                    target: "operon::permissions",
                    "list projects for permissions panel: {e}"
                );
                Vec::new()
            }
        }
    };

    let close = move |_| open.set(false);

    rsx! {
        div {
            class: "operon-modal-scrim",
            "data-testid": "repo-permissions-panel",
            onclick: close,
            onkeydown: move |evt| {
                if evt.key().to_string() == "Escape" {
                    evt.prevent_default();
                    open.set(false);
                }
            },
            tabindex: "0",
            div {
                class: "operon-modal-card",
                style: "max-width: 720px; max-height: 80vh; display: flex; flex-direction: column;",
                onclick: move |evt| evt.stop_propagation(),
                h2 { class: "operon-modal-title", "Tool Permissions" }
                p {
                    class: "operon-modal-message",
                    "Per-repo allow rules. Each entry is written to ",
                    code { ".claude/settings.local.json" }
                    " under ",
                    code { "permissions.allow" }
                    " — the same file Claude reads natively. Pre-allow an MCP server here to avoid the inline prompt on its first call."
                }
                div {
                    style: "overflow-y: auto; flex: 1; margin-top: 12px;",
                    if rows.is_empty() {
                        p {
                            class: "operon-modal-message",
                            style: "font-style: italic; opacity: 0.7;",
                            "No repositories bound. Open a project and bind a repo path first."
                        }
                    }
                    for row in rows.into_iter() {
                        {
                            let path = row.path.clone();
                            let name = row.name.clone();
                            let rules = row.rules.clone();
                            let path_str = path.display().to_string();
                            let is_open = *expanded.read().get(&path).unwrap_or(&false);
                            let mcp_draft = mcp_drafts.read().get(&path).cloned().unwrap_or_default();
                            let custom_draft = custom_drafts.read().get(&path).cloned().unwrap_or_default();
                            let err = errors.read().get(&path).cloned().unwrap_or_default();

                            let path_for_toggle = path.clone();
                            let path_for_mcp_input = path.clone();
                            let path_for_mcp_submit = path.clone();
                            let path_for_custom_input = path.clone();
                            let path_for_custom_submit = path.clone();
                            let mcp_draft_for_submit = mcp_draft.clone();
                            let custom_draft_for_submit = custom_draft.clone();
                            rsx! {
                                div {
                                    style: "border: 1px solid var(--operon-border, #444); border-radius: 4px; margin-bottom: 8px;",
                                    "data-testid": "repo-permissions-row",
                                    "data-repo": "{path_str}",
                                    button {
                                        r#type: "button",
                                        style: "width: 100%; text-align: left; padding: 8px 10px; background: transparent; border: none; cursor: pointer; color: inherit; font: inherit;",
                                        onclick: move |_| {
                                            let mut m = expanded.write();
                                            let cur = *m.get(&path_for_toggle).unwrap_or(&false);
                                            m.insert(path_for_toggle.clone(), !cur);
                                        },
                                        span { style: "margin-right: 6px;", if is_open { "▼" } else { "▶" } }
                                        strong { "{name}" }
                                        span { style: "margin-left: 8px; opacity: 0.6; font-size: 0.9em;",
                                            "{path_str}"
                                        }
                                        {
                                            let count = rules.len();
                                            let suffix = if count == 1 { "" } else { "s" };
                                            rsx! {
                                                span { style: "margin-left: 8px; opacity: 0.5; font-size: 0.85em;",
                                                    "({count} rule{suffix})"
                                                }
                                            }
                                        }
                                    }
                                    if is_open {
                                        div {
                                            style: "padding: 4px 12px 12px 30px;",
                                            if rules.is_empty() {
                                                p {
                                                    style: "margin: 4px 0; opacity: 0.6; font-style: italic;",
                                                    "No rules yet."
                                                }
                                            }
                                            for rule in rules.iter() {
                                                {
                                                    let rule_str = rule.clone();
                                                    let rule_for_remove = rule.clone();
                                                    let path_for_remove = path.clone();
                                                    rsx! {
                                                        div {
                                                            style: "display: flex; align-items: center; gap: 8px; padding: 2px 0;",
                                                            "data-testid": "permission-rule",
                                                            "data-rule": "{rule_str}",
                                                            code {
                                                                style: "flex: 1;",
                                                                "{rule_str}"
                                                            }
                                                            button {
                                                                r#type: "button",
                                                                style: "background: transparent; border: 1px solid var(--operon-border, #444); border-radius: 3px; color: inherit; cursor: pointer; padding: 1px 6px;",
                                                                title: "Revoke",
                                                                onclick: move |_| {
                                                                    if let Err(e) = remove_allow_rule(&path_for_remove, &rule_for_remove) {
                                                                        errors.write().insert(path_for_remove.clone(), format!("remove: {e}"));
                                                                    } else {
                                                                        errors.write().remove(&path_for_remove);
                                                                        refresh_token.with_mut(|t| *t = t.wrapping_add(1));
                                                                    }
                                                                },
                                                                "×"
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            div {
                                                style: "margin-top: 10px; display: flex; gap: 6px; align-items: center;",
                                                label { style: "font-size: 0.9em; opacity: 0.8;", "Add MCP server:" }
                                                input {
                                                    r#type: "text",
                                                    placeholder: "e.g. figma-mcp",
                                                    value: "{mcp_draft}",
                                                    style: "flex: 1; padding: 2px 6px;",
                                                    oninput: move |evt| {
                                                        mcp_drafts.write().insert(path_for_mcp_input.clone(), evt.value());
                                                    },
                                                }
                                                button {
                                                    r#type: "button",
                                                    style: "padding: 2px 10px;",
                                                    onclick: move |_| {
                                                        let name = mcp_draft_for_submit.clone();
                                                        match mcp_wildcard_rule(&name) {
                                                            None => {
                                                                errors.write().insert(
                                                                    path_for_mcp_submit.clone(),
                                                                    format!("invalid MCP server name: {name:?}"),
                                                                );
                                                            }
                                                            Some(rule) => {
                                                                if let Err(e) = append_allow_rule(&path_for_mcp_submit, &rule) {
                                                                    errors.write().insert(
                                                                        path_for_mcp_submit.clone(),
                                                                        format!("write: {e}"),
                                                                    );
                                                                } else {
                                                                    errors.write().remove(&path_for_mcp_submit);
                                                                    mcp_drafts.write().remove(&path_for_mcp_submit);
                                                                    refresh_token.with_mut(|t| *t = t.wrapping_add(1));
                                                                }
                                                            }
                                                        }
                                                    },
                                                    "Add"
                                                }
                                            }
                                            div {
                                                style: "margin-top: 6px; display: flex; gap: 6px; align-items: center;",
                                                label { style: "font-size: 0.9em; opacity: 0.8;", "Custom rule:" }
                                                input {
                                                    r#type: "text",
                                                    placeholder: "e.g. Bash(git push *)",
                                                    value: "{custom_draft}",
                                                    style: "flex: 1; padding: 2px 6px;",
                                                    oninput: move |evt| {
                                                        custom_drafts.write().insert(path_for_custom_input.clone(), evt.value());
                                                    },
                                                }
                                                button {
                                                    r#type: "button",
                                                    style: "padding: 2px 10px;",
                                                    onclick: move |_| {
                                                        let rule = custom_draft_for_submit.trim().to_string();
                                                        if rule.is_empty() {
                                                            return;
                                                        }
                                                        if let Err(e) = append_allow_rule(&path_for_custom_submit, &rule) {
                                                            errors.write().insert(
                                                                path_for_custom_submit.clone(),
                                                                format!("write: {e}"),
                                                            );
                                                        } else {
                                                            errors.write().remove(&path_for_custom_submit);
                                                            custom_drafts.write().remove(&path_for_custom_submit);
                                                            refresh_token.with_mut(|t| *t = t.wrapping_add(1));
                                                        }
                                                    },
                                                    "Add"
                                                }
                                            }
                                            if !err.is_empty() {
                                                p {
                                                    style: "margin: 6px 0 0; color: var(--operon-error, #cc4444); font-size: 0.85em;",
                                                    "{err}"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                div {
                    class: "operon-modal-actions",
                    button {
                        r#type: "button",
                        class: "operon-modal-button operon-modal-button-primary",
                        "data-testid": "repo-permissions-close",
                        onclick: close,
                        "Close"
                    }
                }
            }
        }
    }
}
