//! Per-repository auto-approve policy for tool categories.
//!
//! Persisted in `<repo>/.claude/settings.local.json` under an
//! `operonAutoApprove` key — a sibling of the existing `permissions`
//! object so we don't collide with claude's own schema. Read by the
//! permission-bridge handler before parking the responder: if the
//! category is auto-approved, the handler short-circuits with an
//! immediate `Allow`, skipping the inline prompt entirely.
//!
//! Default policy: only `ReadOnly` auto-approves. Read/Glob/Grep/LS
//! never prompt; everything else (Edit/Write/Bash/Web*) does. The
//! user can flip individual categories from the companion settings
//! panel.
//!
//! Missing-file and corrupted-JSON paths both fall back to the default
//! policy (the latter with a tracing warn). This mirrors
//! [`crate::shell::permission_persist::read_allow_rules`]'s tolerance
//! semantics — a fresh project with no `.claude/` dir should "just
//! work".

#![cfg(not(target_arch = "wasm32"))]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::shell::tool_category::ToolCategory;

/// Settings key we own inside `settings.local.json`. Chosen to avoid
/// collision with claude's `permissions` block.
const KEY: &str = "operonAutoApprove";

/// Per-tool override that beats the per-category default. Useful when
/// a user has `shell: true` (auto-approve Bash) but still wants a
/// prompt for `Bash(git push *)`, or when `fs_write: false` but
/// `Edit(README.md)` is fine to auto-approve.
///
/// Stored under `operonAutoApprove.tool_overrides` keyed by tool name
/// (e.g. `"Bash"`, `"Edit"`) or by full claude-style rule pattern
/// (`"Bash(git push *)"`). Lookup is by exact match for tool names
/// and by pattern-prefix match for rule-style entries (so `"Bash(git
/// push *)"` overrides a specific Bash subcommand). The first match
/// wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolOverride {
    /// Always render the inline permission card regardless of the
    /// per-category default. Use for risky tools you want to review
    /// case-by-case.
    AlwaysPrompt,
    /// Auto-resolve to Allow regardless of the per-category default.
    /// Use for safe tools you've decided you trust unconditionally.
    AlwaysAllow,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoApprovePolicy {
    /// Default `true` — reading the workspace is the cheap, low-risk
    /// kind of tool call that users almost always want to flow without
    /// interruption.
    pub read_only: bool,
    pub fs_write: bool,
    pub shell: bool,
    pub network: bool,
    pub other: bool,
    /// When `true`, the artifact-runner / cascade stops forcing
    /// `acceptEdits` on its claude sessions and instead lets the
    /// permission-bridge handler apply this same policy. With the
    /// flag off (default), cascades keep today's behaviour — every
    /// `Edit` / `Write` auto-approves so SDLC chains don't block on
    /// per-file prompts. Flip on if you want the cascade to surface
    /// the same prompts an interactive chat would.
    ///
    /// Independent of the per-category toggles: a user can set
    /// `read_only=true, fs_write=true` and keep `cascade_uses_auto_
    /// approve_policy=false` to opt into auto-read in interactive
    /// chats while keeping cascades fully automated.
    pub cascade_uses_auto_approve_policy: bool,
    /// Per-tool overrides that win over the per-category default.
    /// `BTreeMap` so the settings UI lists overrides in a stable
    /// alphabetical order and so the JSON round-trips deterministic.
    pub tool_overrides: BTreeMap<String, ToolOverride>,
    /// **Experimental.** When `true`, route claude-code's built-in
    /// `Bash` tool through Operon's own runner via the `operon_bash`
    /// MCP tool. The plugin disallows claude's `Bash` tool on the
    /// CLI; the bridge advertises `mcp__operon__operon_bash` instead.
    /// Gains: streaming stdout/stderr in the chat UI, per-tool
    /// cancellation, the same chunk pipeline runtime backend already
    /// uses. Cost: any pre-existing `Bash(...)` allow rules in
    /// `.claude/settings.local.json` stop matching (you'd need to
    /// migrate them to `mcp__operon__operon_bash(...)`). Defaults
    /// `false`.
    pub bash_via_operon: bool,
}

impl Default for AutoApprovePolicy {
    fn default() -> Self {
        Self {
            read_only: true,
            fs_write: false,
            shell: false,
            network: false,
            other: false,
            cascade_uses_auto_approve_policy: false,
            tool_overrides: BTreeMap::new(),
            bash_via_operon: false,
        }
    }
}

impl AutoApprovePolicy {
    /// Whether a tool of `category` should auto-approve under this
    /// policy. The runtime calls this from the permission handler
    /// just before parking the responder.
    pub fn allows(&self, category: ToolCategory) -> bool {
        match category {
            ToolCategory::ReadOnly => self.read_only,
            ToolCategory::FsWrite => self.fs_write,
            ToolCategory::Shell => self.shell,
            ToolCategory::Network => self.network,
            ToolCategory::Other => self.other,
        }
    }

    /// Look up the override for a tool call, if any. Match order:
    /// 1. Exact tool name (e.g. `"Bash"`)
    /// 2. Rule-style pattern prefix (e.g. `"Bash(git push *)"`) — the
    ///    bridge passes a `pattern_key` derived from the tool input
    ///    when available.
    ///
    /// Returns `None` when no override applies; the caller should
    /// fall back to `allows(category)`.
    pub fn override_for<'a>(
        &'a self,
        tool_name: &str,
        pattern_key: Option<&str>,
    ) -> Option<ToolOverride> {
        if let Some(key) = pattern_key {
            if let Some(v) = self.tool_overrides.get(key) {
                return Some(*v);
            }
        }
        self.tool_overrides.get(tool_name).copied()
    }

    /// Resolve the final auto-approve decision for one tool call.
    /// Override wins; otherwise falls back to the category bucket.
    /// Returns `true` to auto-approve, `false` to surface a prompt.
    pub fn auto_approve_for(
        &self,
        tool_name: &str,
        pattern_key: Option<&str>,
        category: ToolCategory,
    ) -> bool {
        match self.override_for(tool_name, pattern_key) {
            Some(ToolOverride::AlwaysAllow) => true,
            Some(ToolOverride::AlwaysPrompt) => false,
            None => self.allows(category),
        }
    }
}

fn settings_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".claude").join("settings.local.json")
}

/// Load the policy for `repo_root`. Missing file or missing key → the
/// default policy. Corrupted JSON or wrong-shape value → default
/// policy with a `tracing::warn!` so the user sees something in the
/// dev console.
pub fn load(repo_root: &Path) -> AutoApprovePolicy {
    let path = settings_path(repo_root);
    let raw = match fs::read_to_string(&path) {
        Ok(s) if !s.trim().is_empty() => s,
        _ => return AutoApprovePolicy::default(),
    };
    let root: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                target: "operon::permission",
                "auto_approve::load {} parse failed: {e} — using default policy",
                path.display()
            );
            return AutoApprovePolicy::default();
        }
    };
    let policy_v = match root.get(KEY) {
        Some(v) => v.clone(),
        None => return AutoApprovePolicy::default(),
    };
    match serde_json::from_value::<AutoApprovePolicy>(policy_v) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                target: "operon::permission",
                "auto_approve::load {} policy shape invalid: {e} — using default",
                path.display()
            );
            AutoApprovePolicy::default()
        }
    }
}

/// Persist `policy` for `repo_root`. Creates `.claude/` and the
/// settings file if needed. Preserves any pre-existing top-level keys
/// (notably `permissions`) — only the `operonAutoApprove` slot is
/// touched.
pub fn save(repo_root: &Path, policy: AutoApprovePolicy) -> std::io::Result<()> {
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
    obj.insert(KEY.to_string(), serde_json::to_value(policy).unwrap());

    let pretty = serde_json::to_string_pretty(&root).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::Other, format!("serialize: {e}"))
    })?;
    fs::write(&path, pretty + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn default_when_no_file() {
        let td = TempDir::new().unwrap();
        let p = load(td.path());
        assert_eq!(p, AutoApprovePolicy::default());
        assert!(p.read_only);
        assert!(!p.fs_write && !p.shell && !p.network && !p.other);
    }

    #[test]
    fn round_trip() {
        let td = TempDir::new().unwrap();
        let mut tool_overrides = BTreeMap::new();
        tool_overrides.insert("Bash(git push *)".into(), ToolOverride::AlwaysPrompt);
        tool_overrides.insert("Read".into(), ToolOverride::AlwaysAllow);
        let policy = AutoApprovePolicy {
            read_only: false,
            fs_write: true,
            shell: false,
            network: true,
            other: false,
            cascade_uses_auto_approve_policy: true,
            tool_overrides,
            bash_via_operon: true,
        };
        save(td.path(), policy.clone()).unwrap();
        let loaded = load(td.path());
        assert_eq!(loaded, policy);
    }

    #[test]
    fn override_beats_category() {
        let mut p = AutoApprovePolicy::default();
        p.shell = true; // category default: auto-approve all Bash
        p.tool_overrides.insert(
            "Bash(git push *)".into(),
            ToolOverride::AlwaysPrompt,
        );
        // Bash(other) → falls back to shell=true, auto-approves.
        assert!(p.auto_approve_for("Bash", Some("Bash(ls *)"), ToolCategory::Shell));
        // Bash(git push) → explicit AlwaysPrompt → does not auto-approve.
        assert!(!p.auto_approve_for("Bash", Some("Bash(git push *)"), ToolCategory::Shell));
    }

    #[test]
    fn override_can_opt_in_against_category() {
        let mut p = AutoApprovePolicy::default(); // fs_write=false
        p.tool_overrides
            .insert("Edit".into(), ToolOverride::AlwaysAllow);
        // Without override, Edit (FsWrite) would prompt.
        assert!(p.auto_approve_for("Edit", None, ToolCategory::FsWrite));
    }

    #[test]
    fn save_preserves_other_keys() {
        let td = TempDir::new().unwrap();
        let dir = td.path().join(".claude");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.local.json");
        fs::write(
            &path,
            r#"{
  "permissions": { "allow": ["Bash(npm install *)"] }
}"#,
        )
        .unwrap();
        save(td.path(), AutoApprovePolicy::default()).unwrap();

        let raw = fs::read_to_string(&path).unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert!(v.get("permissions").is_some(), "permissions key preserved");
        assert!(v.get(KEY).is_some(), "operonAutoApprove key written");
    }

    #[test]
    fn corrupted_json_returns_default_with_warn() {
        let td = TempDir::new().unwrap();
        let dir = td.path().join(".claude");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("settings.local.json"), "{not valid json").unwrap();
        let p = load(td.path());
        assert_eq!(p, AutoApprovePolicy::default());
    }

    #[test]
    fn wrong_shape_returns_default() {
        let td = TempDir::new().unwrap();
        let dir = td.path().join(".claude");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("settings.local.json"),
            r#"{ "operonAutoApprove": "not an object" }"#,
        )
        .unwrap();
        let p = load(td.path());
        assert_eq!(p, AutoApprovePolicy::default());
    }

    #[test]
    fn allows_matches_default_policy() {
        let p = AutoApprovePolicy::default();
        assert!(p.allows(ToolCategory::ReadOnly));
        assert!(!p.allows(ToolCategory::FsWrite));
        assert!(!p.allows(ToolCategory::Shell));
        assert!(!p.allows(ToolCategory::Network));
        assert!(!p.allows(ToolCategory::Other));
    }
}
