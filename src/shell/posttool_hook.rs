//! Inject the PostToolUse reload hook into a project's
//! `<repo>/.claude/settings.local.json`.
//!
//! Claude Code reads `hooks.PostToolUse[*].matcher` and runs the
//! associated commands after each tool call. We install one entry that
//! matches the file-touching tools (Write / Edit / MultiEdit /
//! NotebookEdit) and points at the `operon-posttool-hook` binary —
//! the binary phones home to the Operon process via a Unix socket so
//! the matching open tab reloads from disk reliably (no inotify
//! flakiness, no `t.content != body` short-circuit).
//!
//! The installer is idempotent: any existing PostToolUse entry whose
//! command line starts with the absolute path of our hook binary is
//! replaced wholesale. Everything else under `hooks.PostToolUse` is
//! preserved so users can stack their own hooks.

#![cfg(not(target_arch = "wasm32"))]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// Matcher string passed to Claude Code. Matches the four tools that
/// modify files on disk; everything else (Read, Glob, Grep, LS, Bash)
/// is left alone so the hook stays cheap.
pub const HOOK_MATCHER: &str = "Write|Edit|MultiEdit|NotebookEdit";

/// Install or refresh the PostToolUse reload hook in
/// `<repo>/.claude/settings.local.json`. Returns Ok(()) on success;
/// callers log + ignore errors (the reload is a UX nicety, not a
/// correctness gate).
///
/// `hook_bin` is the absolute path to the `operon-posttool-hook`
/// binary; `socket_path` is the per-process Unix socket Operon is
/// listening on. Both end up in the `command` field verbatim:
///
/// ```text
/// /abs/operon-posttool-hook --socket /tmp/operon-reload-1234.sock
/// ```
pub fn install(
    repo_root: &Path,
    hook_bin: &Path,
    socket_path: &Path,
) -> std::io::Result<()> {
    let dir = repo_root.join(".claude");
    fs::create_dir_all(&dir)?;
    let path = settings_path(repo_root);

    let mut root: Value = if path.exists() {
        let raw = fs::read_to_string(&path)?;
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

    let obj = root.as_object_mut().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{} is not a JSON object", path.display()),
        )
    })?;

    let hooks_entry = obj.entry("hooks").or_insert_with(|| json!({}));
    if !hooks_entry.is_object() {
        *hooks_entry = json!({});
    }
    let hooks_obj = hooks_entry.as_object_mut().unwrap();
    let post_entry = hooks_obj
        .entry("PostToolUse")
        .or_insert_with(|| json!([]));
    if !post_entry.is_array() {
        *post_entry = json!([]);
    }
    let post_arr = post_entry.as_array_mut().unwrap();

    // Build the command line we'd inject right now, so existing-entry
    // detection can compare against it.
    let command = format!(
        "{} --socket {}",
        shell_quote(hook_bin),
        shell_quote(socket_path),
    );
    let bin_prefix = shell_quote(hook_bin);

    // Drop any pre-existing Operon-managed entry. We identify ours by
    // checking whether ANY nested `hooks[].command` starts with our
    // bin prefix. Survives both shape variants Claude has shipped
    // (the wrapper-with-matcher form below; an older flat form some
    // configs still use).
    post_arr.retain(|entry| !entry_is_operon_managed(entry, &bin_prefix));

    // Append the fresh entry. Claude Code v0.x schema:
    //   { "matcher": "Write|Edit|...", "hooks": [ { "type": "command", "command": "..." } ] }
    post_arr.push(json!({
        "matcher": HOOK_MATCHER,
        "hooks": [
            {
                "type": "command",
                "command": command,
            }
        ]
    }));

    let pretty = serde_json::to_string_pretty(&root).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::Other, format!("serialize: {e}"))
    })?;
    fs::write(&path, pretty + "\n")
}

fn settings_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".claude").join("settings.local.json")
}

/// True when `entry` (a member of `hooks.PostToolUse`) has at least
/// one nested `command` whose path part matches our hook binary.
fn entry_is_operon_managed(entry: &Value, bin_prefix: &str) -> bool {
    let arr = match entry.get("hooks").and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return command_starts_with(entry, bin_prefix),
    };
    arr.iter().any(|h| command_starts_with(h, bin_prefix))
}

fn command_starts_with(v: &Value, bin_prefix: &str) -> bool {
    v.get("command")
        .and_then(|c| c.as_str())
        .is_some_and(|c| c.starts_with(bin_prefix))
}

/// Minimal shell quoting. The settings file is JSON but Claude Code
/// dispatches the command via `/bin/sh -c "<command>"`, so the
/// embedded path needs to survive a sh pass. Paths with no special
/// characters are passed verbatim; anything else gets single-quoted
/// with embedded `'` escaped as `'\''`.
fn shell_quote(p: &Path) -> String {
    let s = p.to_string_lossy();
    if s.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(c, '/' | '.' | '_' | '-' | '+' | '=' | ':' | ',' | '@')
    }) {
        return s.into_owned();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn install_creates_settings_with_hook() {
        let dir = TempDir::new().unwrap();
        let hook_bin = PathBuf::from("/usr/local/bin/operon-posttool-hook");
        let sock = PathBuf::from("/tmp/operon-reload-1234.sock");
        install(dir.path(), &hook_bin, &sock).unwrap();
        let raw = fs::read_to_string(
            dir.path().join(".claude").join("settings.local.json"),
        )
        .unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        let entries = v["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["matcher"], HOOK_MATCHER);
        let cmd = entries[0]["hooks"][0]["command"].as_str().unwrap();
        assert!(cmd.contains("/usr/local/bin/operon-posttool-hook"));
        assert!(cmd.contains("--socket /tmp/operon-reload-1234.sock"));
    }

    #[test]
    fn install_replaces_prior_operon_entry_only() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join(".claude")).unwrap();
        fs::write(
            dir.path().join(".claude").join("settings.local.json"),
            r#"{
                "permissions": {"allow": []},
                "hooks": {
                    "PostToolUse": [
                        {"matcher": "Bash", "hooks": [{"type":"command","command":"/usr/local/bin/user-thing --foo"}]},
                        {"matcher": "Write|Edit", "hooks": [{"type":"command","command":"/old/operon-posttool-hook --socket /tmp/stale.sock"}]}
                    ]
                }
            }"#,
        )
        .unwrap();

        install(
            dir.path(),
            &PathBuf::from("/old/operon-posttool-hook"),
            &PathBuf::from("/tmp/operon-reload-99.sock"),
        )
        .unwrap();

        let raw = fs::read_to_string(
            dir.path().join(".claude").join("settings.local.json"),
        )
        .unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        let entries = v["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        // User's entry survives.
        let user_cmd = entries[0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(user_cmd, "/usr/local/bin/user-thing --foo");
        // Operon's entry was refreshed with the new socket path.
        let operon_cmd = entries[1]["hooks"][0]["command"].as_str().unwrap();
        assert!(operon_cmd.contains("/tmp/operon-reload-99.sock"));
        // Permissions block preserved.
        assert!(v["permissions"]["allow"].is_array());
    }

    #[test]
    fn install_quotes_paths_with_spaces() {
        let dir = TempDir::new().unwrap();
        let hook_bin = PathBuf::from("/Library/Application Support/operon/operon-posttool-hook");
        let sock = PathBuf::from("/tmp/operon-reload-1.sock");
        install(dir.path(), &hook_bin, &sock).unwrap();
        let raw = fs::read_to_string(
            dir.path().join(".claude").join("settings.local.json"),
        )
        .unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        let cmd = v["hooks"]["PostToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert!(cmd.contains("'/Library/Application Support/operon/operon-posttool-hook'"));
    }
}
