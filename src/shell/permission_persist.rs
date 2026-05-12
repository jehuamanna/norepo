//! Persist "Allow always" decisions into the spawned project's
//! `.claude/settings.local.json` so the same tool call doesn't re-prompt
//! on the next turn. Mirrors the existing format the harness already
//! reads — e.g. operon-dioxus's own `.claude/settings.local.json`
//! contains `"Bash(cargo test *)"` under `permissions.allow`.
//!
//! Idempotent: rules already present are not duplicated.

#![cfg(not(target_arch = "wasm32"))]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// Append `rule` to the `permissions.allow` array in
/// `<repo_root>/.claude/settings.local.json`. Creates the directory,
/// the file, and the surrounding JSON structure if any of them is
/// missing. Returns the rule that was actually written (or the
/// already-existing match) so the caller can surface it in a
/// confirmation message.
pub fn append_allow_rule(repo_root: &Path, rule: &str) -> std::io::Result<String> {
    let dir = repo_root.join(".claude");
    fs::create_dir_all(&dir)?;
    let path: PathBuf = dir.join("settings.local.json");

    let mut root: Value = if path.exists() {
        let raw = fs::read_to_string(&path)?;
        // Tolerate an empty file as `{}`. Fail hard on a non-empty
        // file with bad JSON — the user (or a prior process) put
        // something there we'd rather not clobber.
        if raw.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&raw).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("parse {}: {e}", path.display()),
                )
            })?
        }
    } else {
        json!({})
    };

    let permissions = root
        .as_object_mut()
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{} is not a JSON object", path.display()),
            )
        })?
        .entry("permissions")
        .or_insert_with(|| json!({}));
    let allow = permissions
        .as_object_mut()
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "permissions is not a JSON object".to_string(),
            )
        })?
        .entry("allow")
        .or_insert_with(|| json!([]));
    let arr = allow.as_array_mut().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "permissions.allow is not a JSON array".to_string(),
        )
    })?;

    let already = arr.iter().any(|v| v.as_str() == Some(rule));
    if !already {
        arr.push(Value::String(rule.to_string()));
    }

    let pretty = serde_json::to_string_pretty(&root).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::Other, format!("serialize: {e}"))
    })?;
    fs::write(&path, pretty + "\n")?;
    Ok(rule.to_string())
}

/// Read the `permissions.allow` array from
/// `<repo_root>/.claude/settings.local.json`. Returns an empty Vec when
/// the file, the `.claude/` dir, the `permissions` key, or the `allow`
/// key is missing — those are all "no rules yet" states, not errors.
/// Errors only on malformed JSON (matches `append_allow_rule`'s
/// tolerance: we'd rather fail loudly than silently clobber).
pub fn read_allow_rules(repo_root: &Path) -> std::io::Result<Vec<String>> {
    let path = repo_root.join(".claude").join("settings.local.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(&path)?;
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    let root: Value = serde_json::from_str(&raw).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("parse {}: {e}", path.display()),
        )
    })?;
    let Some(arr) = root
        .get("permissions")
        .and_then(|p| p.get("allow"))
        .and_then(|a| a.as_array())
    else {
        return Ok(Vec::new());
    };
    Ok(arr
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect())
}

/// Remove `rule` from the `permissions.allow` array in
/// `<repo_root>/.claude/settings.local.json`. Idempotent: a no-op if the
/// file or the rule is absent. Preserves sibling JSON. Leaves `allow` as
/// `[]` (not removed) when emptied — Claude's harness expects the key to
/// remain present.
pub fn remove_allow_rule(repo_root: &Path, rule: &str) -> std::io::Result<()> {
    let path = repo_root.join(".claude").join("settings.local.json");
    if !path.exists() {
        return Ok(());
    }
    let raw = fs::read_to_string(&path)?;
    if raw.trim().is_empty() {
        return Ok(());
    }
    let mut root: Value = serde_json::from_str(&raw).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("parse {}: {e}", path.display()),
        )
    })?;
    let Some(arr) = root
        .get_mut("permissions")
        .and_then(|p| p.get_mut("allow"))
        .and_then(|a| a.as_array_mut())
    else {
        return Ok(());
    };
    let before = arr.len();
    arr.retain(|v| v.as_str() != Some(rule));
    if arr.len() == before {
        return Ok(());
    }
    let pretty = serde_json::to_string_pretty(&root).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::Other, format!("serialize: {e}"))
    })?;
    fs::write(&path, pretty + "\n")?;
    Ok(())
}

/// Build a Claude Code permission rule that allow-lists every tool
/// exposed by an MCP server. `server_name` is the human-readable name
/// the user typed in the Add-MCP form (e.g. `figma`, `atlassian`); the
/// result is the wildcard `mcp__<server>__*`.
///
/// Returns `None` if the name is empty or contains characters that
/// claude's `mcp__<server>__<tool>` tool-naming convention can't carry
/// (anything outside `[A-Za-z0-9_-]`). Same character class
/// `claude mcp add` already enforces on its `--name` flag.
pub fn mcp_wildcard_rule(server_name: &str) -> Option<String> {
    let trimmed = server_name.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    Some(format!("mcp__{trimmed}__*"))
}

/// Build a permission rule string from the tool name and the proposed
/// input verbatim. Heuristic; the user can hand-edit
/// `.claude/settings.local.json` if the derived glob is too narrow.
///
/// - `Bash` → `Bash(<command up to the first path-like arg> *)`. e.g.
///   `node --test foo.js` → `Bash(node --test *)`.
/// - Other tools → `<ToolName>` (matches "this tool, any input").
pub fn derive_rule(tool_name: &str, input: &Value) -> String {
    if tool_name == "Bash" {
        if let Some(cmd) = input.get("command").and_then(|v| v.as_str()) {
            return derive_bash_rule(cmd);
        }
    }
    tool_name.to_string()
}

/// Glob a Bash command by keeping all leading non-path tokens and
/// replacing the rest with `*`. A token is "path-like" if it contains
/// `/` or — for non-flag tokens — contains a `.` (typical file
/// extension). When no path-like token is found, we still wildcard the
/// tail so e.g. `npm install` becomes `Bash(npm install *)` — close
/// enough for "any args".
fn derive_bash_rule(command: &str) -> String {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    if tokens.is_empty() {
        return "Bash(*)".to_string();
    }
    let mut keep: Vec<&str> = Vec::new();
    for (i, tok) in tokens.iter().enumerate() {
        if i == 0 {
            keep.push(*tok);
            continue;
        }
        let path_like =
            tok.contains('/') || (!tok.starts_with('-') && tok.contains('.'));
        if path_like {
            break;
        }
        keep.push(*tok);
    }
    if keep.is_empty() {
        return "Bash(*)".to_string();
    }
    format!("Bash({} *)", keep.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn mcp_wildcard_rule_basic() {
        assert_eq!(mcp_wildcard_rule("figma"), Some("mcp__figma__*".into()));
        assert_eq!(
            mcp_wildcard_rule("atlassian"),
            Some("mcp__atlassian__*".into())
        );
        // Names with hyphens/underscores are valid MCP server names.
        assert_eq!(
            mcp_wildcard_rule("my-server_v2"),
            Some("mcp__my-server_v2__*".into())
        );
        // Surrounding whitespace is tolerated (UI inputs commonly have it).
        assert_eq!(mcp_wildcard_rule("  figma  "), Some("mcp__figma__*".into()));
    }

    #[test]
    fn mcp_wildcard_rule_rejects_bad_names() {
        assert_eq!(mcp_wildcard_rule(""), None);
        assert_eq!(mcp_wildcard_rule("   "), None);
        // Tool-naming convention is `mcp__<server>__<tool>` with `__` as
        // the separator — a name containing `__` would corrupt the
        // parser. Same for spaces, dots, slashes.
        assert_eq!(mcp_wildcard_rule("bad name"), None);
        assert_eq!(mcp_wildcard_rule("bad.name"), None);
        assert_eq!(mcp_wildcard_rule("bad/name"), None);
        assert_eq!(mcp_wildcard_rule("../etc/passwd"), None);
    }

    #[test]
    fn mcp_wildcard_rule_round_trip_through_append() {
        let dir = tempdir().unwrap();
        let rule = mcp_wildcard_rule("figma").unwrap();
        append_allow_rule(dir.path(), &rule).unwrap();
        let raw =
            std::fs::read_to_string(dir.path().join(".claude/settings.local.json"))
                .unwrap();
        let parsed: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed["permissions"]["allow"][0], "mcp__figma__*");
    }

    #[test]
    fn derive_rule_for_bash_keeps_subcommand_then_wildcards() {
        let cmd = json!({ "command": "node --test foo.js" });
        assert_eq!(derive_rule("Bash", &cmd), "Bash(node --test *)");

        let cmd = json!({ "command": "npm install" });
        assert_eq!(derive_rule("Bash", &cmd), "Bash(npm install *)");

        let cmd = json!({ "command": "git commit -m message" });
        assert_eq!(
            derive_rule("Bash", &cmd),
            "Bash(git commit -m message *)"
        );

        let cmd = json!({ "command": "cargo test --workspace" });
        assert_eq!(
            derive_rule("Bash", &cmd),
            "Bash(cargo test --workspace *)"
        );

        let cmd = json!({ "command": "ls /tmp" });
        assert_eq!(derive_rule("Bash", &cmd), "Bash(ls *)");
    }

    #[test]
    fn derive_rule_for_non_bash_tool_uses_tool_name() {
        assert_eq!(derive_rule("Edit", &json!({})), "Edit");
        assert_eq!(derive_rule("Write", &json!({"file_path": "/x"})), "Write");
    }

    #[test]
    fn derive_rule_for_bash_without_command_field_falls_back_to_tool_name() {
        // Should not panic; falls through to the generic branch.
        assert_eq!(derive_rule("Bash", &json!({})), "Bash");
    }

    #[test]
    fn derive_bash_rule_handles_empty_command() {
        // The public derive_rule will never produce this because the
        // empty-command JSON would carry no `command` field; this
        // covers the internal helper directly.
        assert_eq!(derive_bash_rule(""), "Bash(*)");
    }

    #[test]
    fn append_creates_file_with_permissions_allow_array() {
        let dir = tempdir().unwrap();
        let written = append_allow_rule(dir.path(), "Bash(node --test *)").unwrap();
        assert_eq!(written, "Bash(node --test *)");
        let raw = std::fs::read_to_string(dir.path().join(".claude/settings.local.json"))
            .unwrap();
        let parsed: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            parsed["permissions"]["allow"][0],
            "Bash(node --test *)"
        );
    }

    #[test]
    fn append_to_existing_settings_preserves_other_keys() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        std::fs::write(
            dir.path().join(".claude/settings.local.json"),
            r#"{
              "permissions": { "allow": ["Bash(cargo test *)"] },
              "theme": "dark"
            }"#,
        )
        .unwrap();
        append_allow_rule(dir.path(), "Bash(node --test *)").unwrap();
        let raw = std::fs::read_to_string(dir.path().join(".claude/settings.local.json"))
            .unwrap();
        let parsed: Value = serde_json::from_str(&raw).unwrap();
        let allow = parsed["permissions"]["allow"].as_array().unwrap();
        assert_eq!(allow.len(), 2);
        assert!(allow.iter().any(|v| v == "Bash(cargo test *)"));
        assert!(allow.iter().any(|v| v == "Bash(node --test *)"));
        assert_eq!(parsed["theme"], "dark");
    }

    #[test]
    fn append_is_idempotent() {
        let dir = tempdir().unwrap();
        append_allow_rule(dir.path(), "Bash(node --test *)").unwrap();
        append_allow_rule(dir.path(), "Bash(node --test *)").unwrap();
        let raw = std::fs::read_to_string(dir.path().join(".claude/settings.local.json"))
            .unwrap();
        let parsed: Value = serde_json::from_str(&raw).unwrap();
        let allow = parsed["permissions"]["allow"].as_array().unwrap();
        assert_eq!(allow.len(), 1);
    }

    #[test]
    fn read_returns_empty_when_file_missing() {
        let dir = tempdir().unwrap();
        assert!(read_allow_rules(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn read_returns_empty_when_permissions_absent() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        std::fs::write(
            dir.path().join(".claude/settings.local.json"),
            r#"{ "theme": "dark" }"#,
        )
        .unwrap();
        assert!(read_allow_rules(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn read_returns_empty_when_allow_array_missing() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        std::fs::write(
            dir.path().join(".claude/settings.local.json"),
            r#"{ "permissions": { "deny": ["Bash(rm -rf *)"] } }"#,
        )
        .unwrap();
        assert!(read_allow_rules(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn read_round_trips_after_append() {
        let dir = tempdir().unwrap();
        append_allow_rule(dir.path(), "Bash(cargo test *)").unwrap();
        append_allow_rule(dir.path(), "mcp__figma-mcp__*").unwrap();
        let rules = read_allow_rules(dir.path()).unwrap();
        assert_eq!(rules.len(), 2);
        assert!(rules.contains(&"Bash(cargo test *)".to_string()));
        assert!(rules.contains(&"mcp__figma-mcp__*".to_string()));
    }

    #[test]
    fn read_treats_empty_file_as_no_rules() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        std::fs::write(dir.path().join(".claude/settings.local.json"), "").unwrap();
        assert!(read_allow_rules(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn remove_is_noop_when_file_missing() {
        let dir = tempdir().unwrap();
        remove_allow_rule(dir.path(), "Bash(anything *)").unwrap();
        // Should not have created the file.
        assert!(!dir.path().join(".claude/settings.local.json").exists());
    }

    #[test]
    fn remove_is_noop_when_rule_missing() {
        let dir = tempdir().unwrap();
        append_allow_rule(dir.path(), "Bash(cargo test *)").unwrap();
        remove_allow_rule(dir.path(), "mcp__figma-mcp__*").unwrap();
        let rules = read_allow_rules(dir.path()).unwrap();
        assert_eq!(rules, vec!["Bash(cargo test *)".to_string()]);
    }

    #[test]
    fn remove_preserves_other_rules_and_sibling_keys() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        std::fs::write(
            dir.path().join(".claude/settings.local.json"),
            r#"{
              "permissions": {
                "allow": ["Bash(cargo test *)", "mcp__figma-mcp__*", "Edit"],
                "deny": ["Bash(rm -rf *)"]
              },
              "theme": "dark"
            }"#,
        )
        .unwrap();
        remove_allow_rule(dir.path(), "mcp__figma-mcp__*").unwrap();
        let raw = std::fs::read_to_string(dir.path().join(".claude/settings.local.json"))
            .unwrap();
        let parsed: Value = serde_json::from_str(&raw).unwrap();
        let allow: Vec<String> = parsed["permissions"]["allow"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(allow, vec!["Bash(cargo test *)", "Edit"]);
        assert_eq!(parsed["permissions"]["deny"][0], "Bash(rm -rf *)");
        assert_eq!(parsed["theme"], "dark");
    }

    #[test]
    fn remove_leaves_empty_array_in_place() {
        let dir = tempdir().unwrap();
        append_allow_rule(dir.path(), "mcp__figma-mcp__*").unwrap();
        remove_allow_rule(dir.path(), "mcp__figma-mcp__*").unwrap();
        let raw = std::fs::read_to_string(dir.path().join(".claude/settings.local.json"))
            .unwrap();
        let parsed: Value = serde_json::from_str(&raw).unwrap();
        let allow = parsed["permissions"]["allow"].as_array().unwrap();
        assert!(allow.is_empty());
    }

    #[test]
    fn remove_is_idempotent() {
        let dir = tempdir().unwrap();
        append_allow_rule(dir.path(), "Bash(cargo test *)").unwrap();
        remove_allow_rule(dir.path(), "Bash(cargo test *)").unwrap();
        remove_allow_rule(dir.path(), "Bash(cargo test *)").unwrap();
        let rules = read_allow_rules(dir.path()).unwrap();
        assert!(rules.is_empty());
    }

    #[test]
    fn append_handles_empty_existing_file_as_object() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        std::fs::write(dir.path().join(".claude/settings.local.json"), "").unwrap();
        append_allow_rule(dir.path(), "Bash(node --test *)").unwrap();
        let raw = std::fs::read_to_string(dir.path().join(".claude/settings.local.json"))
            .unwrap();
        let parsed: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            parsed["permissions"]["allow"][0],
            "Bash(node --test *)"
        );
    }
}
