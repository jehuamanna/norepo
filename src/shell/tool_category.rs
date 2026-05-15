//! Coarse risk categorisation for tool calls.
//!
//! Drives two features of the per-tool permission UX:
//!
//! 1. **Category-based auto-approve** ([`crate::shell::auto_approve`]):
//!    the user toggles which categories run without prompting. Defaults
//!    auto-approve `ReadOnly` only so reading files, listing dirs, and
//!    searching never interrupts the user, while everything else still
//!    surfaces a permission card.
//! 2. **Category badge** on the permission card so the user can see at a
//!    glance whether claude is about to read a file, edit a file, run a
//!    shell command, or touch the network.
//!
//! The mapping is intentionally short and explicit — adding a new
//! tool to the list is a one-line code change, and unknown tool names
//! fall through to `Other` (always-prompt by default).
//!
//! `mcp__*` tools all bucket into `Other` regardless of the underlying
//! capability because we can't infer category from a server-side tool
//! description without round-tripping to the MCP server. A skill author
//! who wants finer-grained handling can add an entry here for any MCP
//! tool whose name they know.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolCategory {
    /// Reads filesystem state (Read, Glob, Grep, LS, NotebookRead).
    /// Default-auto-approved.
    ReadOnly,
    /// Mutates filesystem state (Write, Edit, NotebookEdit).
    FsWrite,
    /// Runs an arbitrary shell command (Bash). The riskiest category.
    Shell,
    /// Touches the network (WebFetch, WebSearch).
    Network,
    /// Catch-all: MCP server tools and anything not in the known set.
    Other,
}

impl ToolCategory {
    /// Short display label used by the permission card badge.
    pub fn label(self) -> &'static str {
        match self {
            Self::ReadOnly => "Read-only",
            Self::FsWrite => "Filesystem write",
            Self::Shell => "Shell",
            Self::Network => "Network",
            Self::Other => "Other",
        }
    }

    /// Stable key used in settings.local.json under `operonAutoApprove`.
    /// Lowercase snake_case so it reads naturally in JSON.
    pub fn settings_key(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::FsWrite => "fs_write",
            Self::Shell => "shell",
            Self::Network => "network",
            Self::Other => "other",
        }
    }
}

/// Map a tool name to its risk category.
///
/// Known claude built-ins are listed explicitly. Anything starting with
/// `mcp__` (the convention for MCP server tools) or unknown is `Other`,
/// which means it follows the user's "Other" auto-approve setting —
/// off by default, so MCP tools always prompt unless the user opts in.
pub fn of(tool_name: &str) -> ToolCategory {
    match tool_name {
        // Read-only filesystem inspection.
        "Read" | "Glob" | "Grep" | "LS" | "NotebookRead" => ToolCategory::ReadOnly,
        // Filesystem mutation.
        "Write" | "Edit" | "NotebookEdit" | "MultiEdit" => ToolCategory::FsWrite,
        // Arbitrary shell.
        "Bash" => ToolCategory::Shell,
        // Network.
        "WebFetch" | "WebSearch" => ToolCategory::Network,
        _ => ToolCategory::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_built_ins() {
        for t in ["Read", "Glob", "Grep", "LS", "NotebookRead"] {
            assert_eq!(of(t), ToolCategory::ReadOnly, "expected {t} to be ReadOnly");
        }
    }

    #[test]
    fn fs_write_built_ins() {
        for t in ["Write", "Edit", "NotebookEdit", "MultiEdit"] {
            assert_eq!(of(t), ToolCategory::FsWrite, "expected {t} to be FsWrite");
        }
    }

    #[test]
    fn shell_built_ins() {
        assert_eq!(of("Bash"), ToolCategory::Shell);
    }

    #[test]
    fn network_built_ins() {
        for t in ["WebFetch", "WebSearch"] {
            assert_eq!(of(t), ToolCategory::Network, "expected {t} to be Network");
        }
    }

    #[test]
    fn mcp_tools_fall_through_to_other() {
        assert_eq!(of("mcp__figma__get_figma_data"), ToolCategory::Other);
        assert_eq!(of("mcp__operon__permission_prompt"), ToolCategory::Other);
    }

    #[test]
    fn unknown_tool_is_other() {
        assert_eq!(of("SomethingNew"), ToolCategory::Other);
        assert_eq!(of(""), ToolCategory::Other);
    }

    #[test]
    fn settings_keys_are_unique_and_stable() {
        let keys = [
            ToolCategory::ReadOnly.settings_key(),
            ToolCategory::FsWrite.settings_key(),
            ToolCategory::Shell.settings_key(),
            ToolCategory::Network.settings_key(),
            ToolCategory::Other.settings_key(),
        ];
        let mut sorted = keys.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), keys.len(), "settings keys must be unique");
    }
}
