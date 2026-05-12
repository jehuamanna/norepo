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
