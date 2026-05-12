//! Convenience constructors that assemble the standard tool set.
//!
//! Cuts the boilerplate every consumer would otherwise repeat. Two flavours:
//! - `default_tools()` — every tool except `task` (sub-agent). Use this when
//!   you don't have a `SubAgentSpawner` ready.
//! - `default_tools_with_task(spawner)` — the full set, with `task` wired
//!   to the supplied spawner.
//!
//! Both produce `Vec<Arc<dyn ToolPlugin>>` ready to hand to
//! `AgentRuntime::new(chat, tools, memory, bus)`.

use crate::{
    apply_patch::ApplyPatchTool,
    edit::EditTool,
    git::GitTool,
    glob::GlobTool,
    grep::GrepTool,
    read::ReadTool,
    repo_overview::RepoOverviewTool,
    shell::ShellTool,
    task::{SubAgentSpawner, TaskTool},
    todo::TodoTool,
    web_fetch::WebFetchTool,
    web_search::WebSearchTool,
    write::WriteTool,
};
use operon_core::traits::ToolPlugin;
use std::sync::Arc;

/// What you usually want: every safe-default tool.
///
/// Excluded by default:
/// - `task` (needs a `SubAgentSpawner` — use `default_tools_with_task`).
/// - `lsp` (lives in `operon-plugins-lsp`; build it separately and append).
pub fn default_tools() -> Vec<Arc<dyn ToolPlugin>> {
    let v: Vec<Arc<dyn ToolPlugin>> = vec![
        Arc::new(ReadTool),
        Arc::new(WriteTool),
        Arc::new(EditTool),
        Arc::new(GlobTool),
        Arc::new(GrepTool),
        Arc::new(ShellTool),
        Arc::new(GitTool),
        Arc::new(WebSearchTool::default()),
        Arc::new(WebFetchTool::default()),
        Arc::new(TodoTool::default()),
        Arc::new(ApplyPatchTool),
        Arc::new(RepoOverviewTool),
    ];
    v
}

/// Full set including `task` wired to the supplied `SubAgentSpawner`.
pub fn default_tools_with_task(spawner: Arc<dyn SubAgentSpawner>) -> Vec<Arc<dyn ToolPlugin>> {
    let mut v = default_tools();
    v.push(Arc::new(TaskTool::new(spawner)));
    v
}

/// Builder-style convenience for selecting a subset of the defaults.
///
/// ```
/// use operon_plugins_tools::ToolSet;
/// let tools = ToolSet::new().with_files().with_git().build();
/// ```
pub struct ToolSet {
    files: bool,
    shell: bool,
    git: bool,
    web: bool,
    todo: bool,
    apply_patch: bool,
    repo_overview: bool,
    task_spawner: Option<Arc<dyn SubAgentSpawner>>,
}

impl Default for ToolSet {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolSet {
    pub fn new() -> Self {
        Self {
            files: false,
            shell: false,
            git: false,
            web: false,
            todo: false,
            apply_patch: false,
            repo_overview: false,
            task_spawner: None,
        }
    }
    pub fn all() -> Self {
        Self {
            files: true,
            shell: true,
            git: true,
            web: true,
            todo: true,
            apply_patch: true,
            repo_overview: true,
            task_spawner: None,
        }
    }
    /// read / write / edit / glob / grep
    pub fn with_files(mut self) -> Self {
        self.files = true;
        self
    }
    pub fn with_shell(mut self) -> Self {
        self.shell = true;
        self
    }
    pub fn with_git(mut self) -> Self {
        self.git = true;
        self
    }
    /// web_search + web_fetch
    pub fn with_web(mut self) -> Self {
        self.web = true;
        self
    }
    pub fn with_todo(mut self) -> Self {
        self.todo = true;
        self
    }
    pub fn with_apply_patch(mut self) -> Self {
        self.apply_patch = true;
        self
    }
    pub fn with_repo_overview(mut self) -> Self {
        self.repo_overview = true;
        self
    }
    pub fn with_task(mut self, spawner: Arc<dyn SubAgentSpawner>) -> Self {
        self.task_spawner = Some(spawner);
        self
    }
    pub fn build(self) -> Vec<Arc<dyn ToolPlugin>> {
        let mut v: Vec<Arc<dyn ToolPlugin>> = Vec::new();
        if self.files {
            v.push(Arc::new(ReadTool));
            v.push(Arc::new(WriteTool));
            v.push(Arc::new(EditTool));
            v.push(Arc::new(GlobTool));
            v.push(Arc::new(GrepTool));
        }
        if self.shell {
            v.push(Arc::new(ShellTool));
        }
        if self.git {
            v.push(Arc::new(GitTool));
        }
        if self.web {
            v.push(Arc::new(WebSearchTool::default()));
            v.push(Arc::new(WebFetchTool::default()));
        }
        if self.todo {
            v.push(Arc::new(TodoTool::default()));
        }
        if self.apply_patch {
            v.push(Arc::new(ApplyPatchTool));
        }
        if self.repo_overview {
            v.push(Arc::new(RepoOverviewTool));
        }
        if let Some(spawner) = self.task_spawner {
            v.push(Arc::new(TaskTool::new(spawner)));
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_tools_includes_files_and_web() {
        let tools = default_tools();
        let names: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();
        for required in [
            "read", "write", "edit", "glob", "grep",
            "shell", "git",
            "web_search", "web_fetch",
            "todo", "apply_patch", "repo_overview",
        ] {
            assert!(names.iter().any(|n| n == required), "missing {required}");
        }
        assert!(!names.iter().any(|n| n == "task"), "task not in default set");
    }

    #[test]
    fn default_tools_with_task_appends_task() {
        use crate::task::EchoSpawner;
        let tools = default_tools_with_task(Arc::new(EchoSpawner));
        let names: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();
        assert!(names.iter().any(|n| n == "task"));
    }

    #[test]
    fn toolset_builder_selects_files_only() {
        let tools = ToolSet::new().with_files().build();
        let names: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();
        assert_eq!(names.len(), 5);
        assert!(names.iter().any(|n| n == "read"));
        assert!(!names.iter().any(|n| n == "shell"));
    }

    #[test]
    fn toolset_builder_all_includes_everything_except_task() {
        let tools = ToolSet::all().build();
        let names: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();
        // 5 files + shell + git + 2 web + todo + apply_patch + repo_overview = 12
        assert_eq!(names.len(), 12);
        assert!(!names.iter().any(|n| n == "task"));
    }

    #[test]
    fn toolset_builder_empty_returns_no_tools() {
        let tools = ToolSet::new().build();
        assert!(tools.is_empty());
    }
}
