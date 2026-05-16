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

/// Resolve the global (user-scope) Operon auto-approve settings file:
/// `~/.claude/settings.json`. Shared with Claude Code's user-scope
/// settings — Operon only owns the `operonAutoApprove` key inside.
///
/// Returns `None` when neither `HOME` nor `USERPROFILE` is set (rare,
/// but happens in some sandboxed contexts) so callers can fall back
/// without panicking.
pub fn global_settings_path() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return Some(PathBuf::from(home).join(".claude").join("settings.json"));
        }
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        if !profile.is_empty() {
            return Some(PathBuf::from(profile).join(".claude").join("settings.json"));
        }
    }
    None
}

/// Read the policy from any settings JSON file (project or global) and
/// return `Some(policy)` iff the `operonAutoApprove` key is present.
/// Missing file or absent key → `None` (caller decides what fallback
/// to use). Corrupted JSON / wrong-shape → `Some(default)` with a
/// `tracing::warn!` so the user sees something in the dev console.
fn load_policy_from(path: &Path) -> Option<AutoApprovePolicy> {
    let raw = match fs::read_to_string(path) {
        Ok(s) if !s.trim().is_empty() => s,
        _ => return None,
    };
    let root: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                target: "operon::permission",
                "auto_approve parse {} failed: {e} — ignoring file",
                path.display()
            );
            return Some(AutoApprovePolicy::default());
        }
    };
    let policy_v = root.get(KEY)?.clone();
    match serde_json::from_value::<AutoApprovePolicy>(policy_v) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!(
                target: "operon::permission",
                "auto_approve shape {} invalid: {e} — using default",
                path.display()
            );
            Some(AutoApprovePolicy::default())
        }
    }
}

/// Write the `operonAutoApprove` key into a settings JSON file (project
/// or global). Creates the parent directory and the file if needed.
/// Preserves all sibling top-level keys (notably `permissions` and any
/// Claude Code user-scope keys like `theme` / `model`).
fn save_policy_to(path: &Path, policy: AutoApprovePolicy) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }

    let mut root: Value = if path.exists() {
        let raw = fs::read_to_string(path)?;
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
    fs::write(path, pretty + "\n")
}

/// Load the policy for `repo_root`. Missing file or missing key → the
/// default policy. Corrupted JSON or wrong-shape value → default
/// policy with a `tracing::warn!` so the user sees something in the
/// dev console.
///
/// This is the *project-only* read — it deliberately does **not** fall
/// back to the user-scope global file. The per-project Tool Permissions
/// modal uses this so the toggles reflect what is actually persisted
/// for the project (not a merged view that would confuse "what am I
/// editing here?"). Runtime callers that want the effective policy —
/// project overrides global, global overrides default — should use
/// [`load_effective`] instead.
pub fn load(repo_root: &Path) -> AutoApprovePolicy {
    load_policy_from(&settings_path(repo_root)).unwrap_or_default()
}

/// Load the *effective* policy for `repo_root`. Resolution order:
/// 1. `<repo>/.claude/settings.local.json` if it contains an
///    `operonAutoApprove` key — explicit per-project choice wins.
/// 2. Else `~/.claude/settings.json` (user-scope global).
/// 3. Else [`AutoApprovePolicy::default`].
///
/// Use this from the runtime (permission-bridge handler, cascade
/// runner, bash-via-operon dispatch). Use [`load`] from the per-project
/// settings UI so each tier is editable in isolation.
pub fn load_effective(repo_root: &Path) -> AutoApprovePolicy {
    if let Some(p) = load_policy_from(&settings_path(repo_root)) {
        return p;
    }
    load_global()
}

/// Persist `policy` for `repo_root`. Creates `.claude/` and the
/// settings file if needed. Preserves any pre-existing top-level keys
/// (notably `permissions`) — only the `operonAutoApprove` slot is
/// touched.
pub fn save(repo_root: &Path, policy: AutoApprovePolicy) -> std::io::Result<()> {
    save_policy_to(&settings_path(repo_root), policy)
}

/// Load the user-scope global policy from `~/.claude/settings.json`.
/// Missing file, absent key, or unresolvable home dir → the default
/// policy. Used as the project-tier fallback by [`load_effective`] and
/// read directly by the global Chat Permissions settings UI.
pub fn load_global() -> AutoApprovePolicy {
    global_settings_path()
        .as_deref()
        .and_then(load_policy_from)
        .unwrap_or_default()
}

/// Persist `policy` to the user-scope global settings file. Creates
/// `~/.claude/` and the file if needed. Returns an error when neither
/// `HOME` nor `USERPROFILE` is set so the caller can surface it.
pub fn save_global(policy: AutoApprovePolicy) -> std::io::Result<()> {
    let path = global_settings_path().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no HOME or USERPROFILE env var; cannot resolve ~/.claude/settings.json",
        )
    })?;
    save_policy_to(&path, policy)
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

    // --- Global-tier (load_policy_from / save_policy_to) ---
    //
    // We test the path-based helpers directly so the assertions don't
    // race against a developer's real `~/.claude/settings.json`. The
    // `load_global` / `save_global` wrappers are thin shims over these
    // plus `global_settings_path` — they get exercised in manual UX
    // tests once HOME is populated.

    #[test]
    fn save_policy_to_creates_file_and_round_trips() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("nested").join("settings.json");
        let mut overrides = BTreeMap::new();
        overrides.insert("Edit".into(), ToolOverride::AlwaysAllow);
        let policy = AutoApprovePolicy {
            read_only: false,
            fs_write: true,
            shell: true,
            network: false,
            other: false,
            cascade_uses_auto_approve_policy: false,
            tool_overrides: overrides,
            bash_via_operon: false,
        };
        save_policy_to(&path, policy.clone()).unwrap();
        // Parent dir auto-created.
        assert!(path.parent().unwrap().is_dir());
        // Round-trips through load_policy_from.
        assert_eq!(load_policy_from(&path), Some(policy));
    }

    #[test]
    fn save_policy_to_preserves_sibling_keys() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("settings.json");
        fs::write(
            &path,
            r#"{ "theme": "dark", "model": "claude-opus-4-7" }"#,
        )
        .unwrap();
        save_policy_to(&path, AutoApprovePolicy::default()).unwrap();

        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["model"], "claude-opus-4-7");
        assert!(v.get(KEY).is_some(), "operonAutoApprove key written");
    }

    #[test]
    fn load_policy_from_returns_none_when_key_absent() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("settings.json");
        fs::write(&path, r#"{ "theme": "dark" }"#).unwrap();
        // None signals "no Operon policy here" → caller can fall back.
        assert!(load_policy_from(&path).is_none());
    }

    #[test]
    fn load_effective_uses_project_when_key_present() {
        // The fallback chain only descends if the project file lacks
        // the key. With a project policy in place, the global tier
        // (whatever HOME points at) must NOT be consulted — verified
        // by setting a non-default project value and asserting it
        // round-trips even if the developer has a real ~/.claude
        // policy stored on their machine.
        let td = TempDir::new().unwrap();
        let mut policy = AutoApprovePolicy::default();
        policy.shell = true;
        policy.bash_via_operon = true;
        save(td.path(), policy.clone()).unwrap();
        assert_eq!(load_effective(td.path()), policy);
    }
}
