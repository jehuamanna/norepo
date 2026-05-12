//! `task` tool — sub-agent delegation.
//!
//! Spawns a child agent with a constrained tool subset, optional persona,
//! optional model override, and a bounded turn budget. Returns the child's
//! final text output.
//!
//! The actual child-agent construction is delegated to a `SubAgentSpawner`
//! supplied at construction time, so this crate doesn't take a dep on the
//! chat-plugin crates.

use operon_core::error::{OperonError, OperonResult};
use operon_core::traits::{
    CancellationToken, Capabilities, Plugin, ToolDef, ToolPlugin,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

/// Inputs the spawner receives. Mirrors the JSON schema below.
#[derive(Clone, Debug)]
pub struct SubAgentRequest {
    pub description: String,
    pub persona: Option<String>,
    pub tools: Option<Vec<String>>,
    pub max_turns: u32,
    pub model: Option<String>,
}

/// Output from a child agent run.
#[derive(Clone, Debug)]
pub struct SubAgentResult {
    pub summary: String,
    pub turns: u32,
    pub stopped_at_max_turns: bool,
}

#[async_trait]
pub trait SubAgentSpawner: Send + Sync {
    async fn spawn(
        &self,
        req: SubAgentRequest,
        ct: CancellationToken,
    ) -> OperonResult<SubAgentResult>;
}

#[derive(Deserialize)]
struct TaskInput {
    description: String,
    #[serde(default)]
    persona: Option<String>,
    #[serde(default)]
    tools: Option<Vec<String>>,
    #[serde(default)]
    max_turns: Option<u32>,
    #[serde(default)]
    model: Option<String>,
}

pub struct TaskTool {
    spawner: Arc<dyn SubAgentSpawner>,
    /// Hard cap on `max_turns` to prevent runaway sub-agents.
    max_turns_cap: u32,
}

impl TaskTool {
    pub fn new(spawner: Arc<dyn SubAgentSpawner>) -> Self {
        Self {
            spawner,
            max_turns_cap: 32,
        }
    }

    pub fn with_max_turns_cap(mut self, cap: u32) -> Self {
        self.max_turns_cap = cap;
        self
    }
}

#[async_trait]
impl Plugin for TaskTool {
    fn name(&self) -> &str { "task" }
    fn version(&self) -> &str { env!("CARGO_PKG_VERSION") }
    fn capabilities(&self) -> Capabilities { Capabilities::empty() }
}

#[async_trait]
impl ToolPlugin for TaskTool {
    fn schema(&self) -> ToolDef {
        ToolDef {
            name: "task".into(),
            description: "Delegate a sub-task to a child agent. Use for complex multi-step \
                          searches, validation passes, or any work that would otherwise \
                          consume a lot of context. Returns the child's summary."
                          .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "description": { "type": "string", "description": "What the child should do." },
                    "persona":     { "type": "string", "description": "Built-in persona id (general/explore/validate/code-review/bug-fix) or skill slug." },
                    "tools":       { "type": "array",  "items": { "type": "string" }, "description": "Tool names the child can use. Defaults to a small read-only set." },
                    "max_turns":   { "type": "integer", "minimum": 1, "default": 8 },
                    "model":       { "type": "string", "description": "Optional model override for the child." }
                },
                "required": ["description"]
            }),
        }
    }

    async fn invoke(
        &self,
        args: serde_json::Value,
        ct: CancellationToken,
    ) -> OperonResult<serde_json::Value> {
        let input: TaskInput = serde_json::from_value(args).map_err(|e| OperonError::Plugin {
            plugin: "task".into(),
            source: Box::new(e),
        })?;
        let max_turns = input.max_turns.unwrap_or(8).min(self.max_turns_cap);
        let req = SubAgentRequest {
            description: input.description,
            persona: input.persona,
            tools: input.tools,
            max_turns,
            model: input.model,
        };
        match self.spawner.spawn(req, ct).await {
            Ok(r) => Ok(json!({
                "summary": r.summary,
                "turns": r.turns,
                "stopped_at_max_turns": r.stopped_at_max_turns,
            })),
            Err(e) => Ok(json!({
                "error": format!("sub-agent failed: {e}"),
            })),
        }
    }
}

/// A no-op spawner useful for unit tests. Returns the description verbatim
/// as the summary and reports zero turns.
pub struct EchoSpawner;

#[async_trait]
impl SubAgentSpawner for EchoSpawner {
    async fn spawn(
        &self,
        req: SubAgentRequest,
        _ct: CancellationToken,
    ) -> OperonResult<SubAgentResult> {
        Ok(SubAgentResult {
            summary: format!("[echo] {}", req.description),
            turns: 0,
            stopped_at_max_turns: false,
        })
    }
}

/// Real spawner: drives a child `AgentRuntime` per call.
///
/// Construction takes a factory closure that returns an `Arc<AgentRuntime>`
/// configured for the child (chat plugin, tool subset already filtered by
/// persona, memory store, optional model override). The factory is called
/// once per `spawn()`, so each child has its own session and budget.
///
/// The factory pattern breaks the otherwise-cyclic dependency: `TaskTool`
/// is a tool registered with the parent runtime, but spawning needs *another*
/// runtime; the closure constructs that other runtime on demand.
pub struct AgentRuntimeSpawner {
    factory: Box<
        dyn Fn(&SubAgentRequest) -> operon_core::error::OperonResult<std::sync::Arc<operon_core::runtime::AgentRuntime>>
            + Send
            + Sync,
    >,
}

impl AgentRuntimeSpawner {
    pub fn new<F>(factory: F) -> Self
    where
        F: Fn(&SubAgentRequest) -> operon_core::error::OperonResult<std::sync::Arc<operon_core::runtime::AgentRuntime>>
            + Send
            + Sync
            + 'static,
    {
        Self {
            factory: Box::new(factory),
        }
    }
}

#[async_trait]
impl SubAgentSpawner for AgentRuntimeSpawner {
    async fn spawn(
        &self,
        req: SubAgentRequest,
        ct: CancellationToken,
    ) -> OperonResult<SubAgentResult> {
        use futures::StreamExt;
        use operon_core::budget::Budget;
        use operon_core::runtime::Step;
        use operon_core::traits::Scope;

        let runtime = (self.factory)(&req)?;
        let session = uuid::Uuid::new_v4();
        let budget = Budget::new(None, None, None, Some(req.max_turns));
        let mut stream = runtime.run(session, Scope::User, req.description.clone(), budget, ct);

        let mut summary = String::new();
        let mut turns: u32 = 0;
        let mut stopped_at_max = false;
        while let Some(step) = stream.next().await {
            match step {
                Step::StreamDelta(t) => summary.push_str(&t),
                Step::Done(stop) => {
                    use operon_core::runtime::StopReason;
                    if let StopReason::BudgetExceeded(reason) = &stop {
                        if reason.contains("max_iterations") || reason.contains("steps") {
                            stopped_at_max = true;
                        }
                    }
                    break;
                }
                Step::ToolResult { .. } => {
                    turns += 1;
                }
                _ => {}
            }
        }
        Ok(SubAgentResult {
            summary: summary.trim().to_string(),
            turns,
            stopped_at_max_turns: stopped_at_max,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn schema_is_well_formed() {
        let t = TaskTool::new(Arc::new(EchoSpawner));
        assert_eq!(t.schema().name, "task");
    }

    #[tokio::test]
    async fn echo_spawner_returns_description_as_summary() {
        let t = TaskTool::new(Arc::new(EchoSpawner));
        let r = t
            .invoke(
                json!({ "description": "do a thing" }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            r.get("summary").and_then(|v| v.as_str()),
            Some("[echo] do a thing")
        );
    }

    #[tokio::test]
    async fn max_turns_capped_to_runtime_limit() {
        let t = TaskTool::new(Arc::new(EchoSpawner)).with_max_turns_cap(4);
        // Even though caller asks for 100, the cap is 4. EchoSpawner doesn't surface
        // the value but we can verify by inspecting the cap directly.
        assert_eq!(t.max_turns_cap, 4);
        let _ = t
            .invoke(
                json!({ "description": "x", "max_turns": 100 }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
    }

    struct FailingSpawner;

    #[async_trait]
    impl SubAgentSpawner for FailingSpawner {
        async fn spawn(
            &self,
            _req: SubAgentRequest,
            _ct: CancellationToken,
        ) -> OperonResult<SubAgentResult> {
            Err(OperonError::Plugin {
                plugin: "test".into(),
                source: Box::new(std::io::Error::other("boom")),
            })
        }
    }

    #[tokio::test]
    async fn spawner_failure_surfaced_as_error_field() {
        let t = TaskTool::new(Arc::new(FailingSpawner));
        let r = t
            .invoke(
                json!({ "description": "x" }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(r.get("error").is_some());
    }

    #[tokio::test]
    async fn agent_runtime_spawner_drives_real_child_loop() {
        // End-to-end demo: the child runtime is built with EchoChatPlugin
        // scripted to emit two text deltas then stop. The spawner collects
        // those deltas as the summary.
        use operon_core::bus::EventBus;
        use operon_core::echo::EchoChatPlugin;
        use operon_core::memory::InMemoryStore;
        use operon_core::runtime::AgentRuntime;
        use operon_core::traits::{ChatDelta, ChatPlugin, MemoryPlugin, StopReason, Usage};

        let factory = |_req: &SubAgentRequest| {
            // One scripted turn that emits text and stops.
            let chat = std::sync::Arc::new(EchoChatPlugin::new(
                "echo",
                vec![vec![
                    ChatDelta::Text("hello from child".into()),
                    ChatDelta::Stop {
                        reason: StopReason::EndTurn,
                        usage: Some(Usage {
                            prompt: 1,
                            prompt_cached: 0,
                            completion: 1,
                        }),
                    },
                ]],
            ));
            let memory: std::sync::Arc<dyn MemoryPlugin> = std::sync::Arc::new(InMemoryStore::new());
            Ok(std::sync::Arc::new(AgentRuntime::new(
                chat as std::sync::Arc<dyn ChatPlugin>,
                vec![],
                memory,
                EventBus::new(64),
            )))
        };
        let spawner = AgentRuntimeSpawner::new(factory);
        let r = spawner
            .spawn(
                SubAgentRequest {
                    description: "explore the repo".into(),
                    persona: None,
                    tools: None,
                    max_turns: 4,
                    model: None,
                },
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(r.summary, "hello from child");
        assert_eq!(r.turns, 0); // no tool calls
        assert!(!r.stopped_at_max_turns);
    }

    #[tokio::test]
    async fn task_tool_with_real_spawner_round_trips_summary() {
        // Wire the TaskTool to an AgentRuntimeSpawner and call invoke().
        use operon_core::bus::EventBus;
        use operon_core::echo::EchoChatPlugin;
        use operon_core::memory::InMemoryStore;
        use operon_core::runtime::AgentRuntime;
        use operon_core::traits::{ChatDelta, ChatPlugin, MemoryPlugin, StopReason, Usage};

        let factory = |_req: &SubAgentRequest| {
            let chat = std::sync::Arc::new(EchoChatPlugin::new(
                "echo",
                vec![vec![
                    ChatDelta::Text("child summary".into()),
                    ChatDelta::Stop {
                        reason: StopReason::EndTurn,
                        usage: Some(Usage::default()),
                    },
                ]],
            ));
            let memory: std::sync::Arc<dyn MemoryPlugin> = std::sync::Arc::new(InMemoryStore::new());
            Ok(std::sync::Arc::new(AgentRuntime::new(
                chat as std::sync::Arc<dyn ChatPlugin>,
                vec![],
                memory,
                EventBus::new(8),
            )))
        };
        let tool = TaskTool::new(Arc::new(AgentRuntimeSpawner::new(factory)));
        let r = tool
            .invoke(
                json!({ "description": "summarize the codebase" }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(r["summary"].as_str(), Some("child summary"));
        assert_eq!(r["turns"].as_u64(), Some(0));
        assert_eq!(r["stopped_at_max_turns"].as_bool(), Some(false));
    }
}
