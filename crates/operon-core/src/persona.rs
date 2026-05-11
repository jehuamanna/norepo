//! Agent personas — each skill in the SDLC cascade can declare its own
//! persona (system prompt + tool subset + model preference + sub-agent
//! allowlist).
//!
//! Built-in personas:
//!  - `general`     — broad-purpose, can use all tools
//!  - `explore`     — read-only investigation; read/glob/grep/lsp/web_*
//!  - `validate`    — read + run tests; read/glob/grep/shell/git
//!  - `code-review` — diffs and explanation; read/glob/grep/git/lsp
//!  - `bug-fix`     — diagnose + edit; read/glob/grep/edit/shell/git/lsp
//!  - `BA`          — business analyst; plan-mode read-only; for the
//!                    SDLC chain's master_requirement → epic → … → task
//!                    decomposition skills
//!  - `SA`          — solution architect; plan-mode read-only; for the
//!                    architecture skill
//!  - `SDE`         — software development engineer; build-mode; for
//!                    implementation, test-generation, test-execution
//!                    and bug-fix skills
//!
//! Skill notes can declare additional personas via the `agent:` frontmatter
//! block; those personas are loaded from the note body as markdown.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentPersona {
    pub id: String,
    /// System prompt content. May be empty (skill body provides it instead).
    pub system_prompt: String,
    /// Tool names this persona is allowed to call. Empty = allow all.
    pub tools: Vec<String>,
    /// Optional preferred provider (e.g. "anthropic", "openai", "google").
    pub provider: Option<String>,
    /// Optional preferred model id (e.g. "claude-sonnet-4-6", "gpt-5").
    pub model: Option<String>,
    /// Restricts certain tools when set. Currently advisory; runtime support lands in Slice A12.
    pub mode: Option<AgentMode>,
    /// Personas this persona may spawn via the `task` tool.
    pub sub_agents: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentMode {
    /// Full access (write + edit + shell + git commit).
    Build,
    /// Read-only investigation.
    Plan,
}

impl AgentPersona {
    pub fn allows_tool(&self, name: &str) -> bool {
        if self.tools.is_empty() {
            return true;
        }
        self.tools.iter().any(|t| t == name)
    }
}

/// Catalogue of built-in personas. Look up by id.
pub struct PersonaRegistry {
    personas: HashMap<String, AgentPersona>,
}

impl PersonaRegistry {
    pub fn with_builtins() -> Self {
        let mut personas = HashMap::new();
        for p in builtins() {
            personas.insert(p.id.clone(), p);
        }
        Self { personas }
    }

    pub fn empty() -> Self {
        Self {
            personas: HashMap::new(),
        }
    }

    pub fn get(&self, id: &str) -> Option<&AgentPersona> {
        self.personas.get(id)
    }

    pub fn insert(&mut self, p: AgentPersona) {
        self.personas.insert(p.id.clone(), p);
    }

    pub fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.personas.keys().cloned().collect();
        ids.sort();
        ids
    }
}

impl Default for PersonaRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

fn builtins() -> Vec<AgentPersona> {
    vec![
        AgentPersona {
            id: "general".into(),
            system_prompt: "You are a general-purpose coding assistant. \
                            Be concise, prefer terse summaries, and avoid speculation."
                            .into(),
            tools: vec![],
            mode: Some(AgentMode::Build),
            sub_agents: vec!["explore".into(), "validate".into()],
            ..Default::default()
        },
        AgentPersona {
            id: "explore".into(),
            system_prompt: "You are a read-only investigation agent. \
                            Use grep / glob / read / lsp / web_search / web_fetch. \
                            Do not modify files. Return a concise summary of findings."
                            .into(),
            tools: vec!["read".into(), "glob".into(), "grep".into(), "lsp".into(),
                        "web_search".into(), "web_fetch".into()],
            mode: Some(AgentMode::Plan),
            sub_agents: vec![],
            ..Default::default()
        },
        AgentPersona {
            id: "validate".into(),
            system_prompt: "You are a validation agent. \
                            Run tests and lints; surface failures clearly with reproduction steps. \
                            You may use shell to invoke build/test commands but must not modify code."
                            .into(),
            tools: vec!["read".into(), "glob".into(), "grep".into(), "shell".into(),
                        "git".into(), "lsp".into()],
            mode: Some(AgentMode::Plan),
            sub_agents: vec![],
            ..Default::default()
        },
        AgentPersona {
            id: "code-review".into(),
            system_prompt: "You are a code reviewer. \
                            Read diffs, suggest concrete improvements, and flag risks. \
                            Cite file paths and line numbers. Do not modify code."
                            .into(),
            tools: vec!["read".into(), "glob".into(), "grep".into(), "git".into(), "lsp".into()],
            mode: Some(AgentMode::Plan),
            sub_agents: vec![],
            ..Default::default()
        },
        AgentPersona {
            id: "bug-fix".into(),
            system_prompt: "You are a bug-fix agent. \
                            Diagnose by reading code and tests, then propose minimal patches. \
                            Make focused edits, run tests to confirm, commit with a clear message."
                            .into(),
            tools: vec!["read".into(), "glob".into(), "grep".into(), "edit".into(),
                        "write".into(), "shell".into(), "git".into(), "lsp".into()],
            mode: Some(AgentMode::Build),
            sub_agents: vec!["explore".into(), "validate".into()],
            ..Default::default()
        },
        AgentPersona {
            id: "BA".into(),
            system_prompt: "You are a senior Business Analyst working in the SDLC \
                            cascade. Read the source artifact and the inherited / \
                            aggregated context, then produce the requested output \
                            artifact(s) per the skill body's instructions. Bias toward \
                            coverage over minimalism: a capability the input clearly \
                            calls for must not be silently dropped. Preserve revision \
                            history when the prompt inlines prior bodies — append a \
                            `## Revision N (YYYY-MM-DD)` row and stash the prior body \
                            under a collapsed `<details>` block rather than \
                            discarding it. Do not modify code; this persona is \
                            read-only investigation."
                            .into(),
            tools: vec!["read".into(), "glob".into(), "grep".into(), "lsp".into(),
                        "web_search".into(), "web_fetch".into()],
            mode: Some(AgentMode::Plan),
            sub_agents: vec!["explore".into()],
            ..Default::default()
        },
        AgentPersona {
            id: "SA".into(),
            system_prompt: "You are a senior Solution Architect. Produce or revise \
                            the project's architecture artifact from the \
                            master_requirement plus every aggregated detail \
                            Requirement. Cover components, data model, public \
                            contracts, tech-stack rationale, NFRs, risks, and \
                            rollout strategy. When prior architecture revisions are \
                            inlined, append a new revision and stash older content \
                            under a collapsed `<details>` block — never silently \
                            overwrite prior decisions. Do not modify code; this \
                            persona is read-only design work."
                            .into(),
            tools: vec!["read".into(), "glob".into(), "grep".into(), "lsp".into(),
                        "web_search".into(), "web_fetch".into()],
            mode: Some(AgentMode::Plan),
            sub_agents: vec!["explore".into()],
            ..Default::default()
        },
        AgentPersona {
            id: "SDE".into(),
            system_prompt: "You are a senior software engineer. Implement the Task \
                            end-to-end strictly within the inherited Architecture's \
                            constraints. Use the codebase's existing patterns; \
                            don't introduce new abstractions. Stay scoped to the \
                            Task's `## What changes` bullets — out-of-scope work \
                            belongs in the implementation note's Follow-ups. Run \
                            local tests as a sanity check; the dedicated \
                            execute-tests stage produces the canonical report. \
                            Commit when done with a clear message; one commit per \
                            Task. For bug-fix runs, never loosen tests to hide the \
                            bug — note it under Follow-ups instead."
                            .into(),
            tools: vec!["read".into(), "glob".into(), "grep".into(), "edit".into(),
                        "write".into(), "shell".into(), "git".into(), "lsp".into()],
            mode: Some(AgentMode::Build),
            sub_agents: vec!["explore".into(), "validate".into(), "bug-fix".into()],
            ..Default::default()
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_starts_with_all_builtins() {
        let r = PersonaRegistry::with_builtins();
        let ids = r.ids();
        assert_eq!(ids.len(), 8);
        for id in [
            "general", "explore", "validate", "code-review", "bug-fix",
            "BA", "SA", "SDE",
        ] {
            assert!(ids.iter().any(|i| i == id), "missing persona {id}");
        }
    }

    #[test]
    fn ba_persona_is_plan_mode_with_read_only_tools() {
        let r = PersonaRegistry::with_builtins();
        let p = r.get("BA").unwrap();
        assert_eq!(p.mode, Some(AgentMode::Plan));
        assert!(p.allows_tool("read"));
        assert!(p.allows_tool("grep"));
        assert!(!p.allows_tool("edit"));
        assert!(!p.allows_tool("write"));
        assert!(!p.allows_tool("shell"));
    }

    #[test]
    fn sa_persona_is_plan_mode() {
        let r = PersonaRegistry::with_builtins();
        let p = r.get("SA").unwrap();
        assert_eq!(p.mode, Some(AgentMode::Plan));
        assert!(p.allows_tool("read"));
        assert!(!p.allows_tool("write"));
    }

    #[test]
    fn sde_persona_is_build_mode_with_edit_and_shell() {
        let r = PersonaRegistry::with_builtins();
        let p = r.get("SDE").unwrap();
        assert_eq!(p.mode, Some(AgentMode::Build));
        assert!(p.allows_tool("edit"));
        assert!(p.allows_tool("write"));
        assert!(p.allows_tool("shell"));
        assert!(p.allows_tool("git"));
    }

    #[test]
    fn explore_persona_is_plan_mode_with_read_only_tools() {
        let r = PersonaRegistry::with_builtins();
        let p = r.get("explore").unwrap();
        assert_eq!(p.mode, Some(AgentMode::Plan));
        assert!(p.allows_tool("read"));
        assert!(!p.allows_tool("shell"));
        assert!(!p.allows_tool("write"));
    }

    #[test]
    fn empty_tool_list_allows_all() {
        let p = AgentPersona {
            id: "x".into(),
            tools: vec![],
            ..Default::default()
        };
        assert!(p.allows_tool("anything"));
    }

    #[test]
    fn explicit_tool_list_filters() {
        let p = AgentPersona {
            id: "x".into(),
            tools: vec!["read".into(), "grep".into()],
            ..Default::default()
        };
        assert!(p.allows_tool("read"));
        assert!(p.allows_tool("grep"));
        assert!(!p.allows_tool("write"));
    }

    #[test]
    fn bug_fix_persona_can_edit_and_commit() {
        let r = PersonaRegistry::with_builtins();
        let p = r.get("bug-fix").unwrap();
        assert!(p.allows_tool("edit"));
        assert!(p.allows_tool("git"));
        assert_eq!(p.mode, Some(AgentMode::Build));
    }

    #[test]
    fn code_review_cannot_modify() {
        let r = PersonaRegistry::with_builtins();
        let p = r.get("code-review").unwrap();
        assert!(p.allows_tool("read"));
        assert!(!p.allows_tool("edit"));
        assert!(!p.allows_tool("write"));
        assert!(!p.allows_tool("shell"));
    }

    #[test]
    fn persona_serde_round_trip() {
        let p = AgentPersona {
            id: "t".into(),
            system_prompt: "you are t".into(),
            tools: vec!["read".into()],
            provider: Some("anthropic".into()),
            model: Some("claude-sonnet-4-6".into()),
            mode: Some(AgentMode::Build),
            sub_agents: vec!["explore".into()],
        };
        let s = serde_json::to_string(&p).unwrap();
        let back: AgentPersona = serde_json::from_str(&s).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn registry_supports_user_inserted_persona() {
        let mut r = PersonaRegistry::with_builtins();
        r.insert(AgentPersona {
            id: "ba".into(),
            system_prompt: "you are a senior business analyst".into(),
            ..Default::default()
        });
        assert!(r.get("ba").is_some());
    }
}
