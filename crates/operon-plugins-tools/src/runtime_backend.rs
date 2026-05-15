//! `AgentBackend` impl that drives the in-process `AgentRuntime`.
//!
//! Translates the runtime's `Step` stream into the backend-agnostic
//! `AgentEvent` flow that cascade / executor / companion consume. With this
//! adapter, swapping the cascade from claude-code-subprocess to the
//! native runtime is one Arc constructor — no consumer changes.
//!
//! Construction is via a factory closure so the `AgentRuntime` is built
//! per session (unique session id, fresh memory store, etc.). Re-binding a
//! session is supported by replacing the runtime under that session id.

use async_trait::async_trait;
use futures::StreamExt;
use operon_core::agent_event::{AgentBackend, AgentEvent};
use operon_core::error::OperonResult;
use operon_core::runtime::{AgentRuntime, Step, StopReason};
use operon_core::traits::{CancellationToken, Scope, Usage};
use operon_core::Budget;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::Mutex;
use uuid::Uuid;

/// Per-session config the factory receives — gives it enough to build the
/// right `AgentRuntime` (provider, persona, cwd, model overrides).
pub struct RuntimeBuildArgs {
    pub session_id: Uuid,
    pub cwd: PathBuf,
}

pub type RuntimeFactory =
    Arc<dyn Fn(&RuntimeBuildArgs) -> OperonResult<Arc<AgentRuntime>> + Send + Sync>;

pub struct RuntimeAgentBackend {
    factory: RuntimeFactory,
    /// Per-session bindings: session id → (cwd, runtime). The cwd is the
    /// last value passed to `bind_session`. The runtime is lazily built on
    /// the first `send_rich`.
    bindings: Mutex<HashMap<Uuid, Binding>>,
    /// Default agent loop step cap for sessions that don't override it.
    default_max_steps: u32,
}

struct Binding {
    cwd: PathBuf,
    runtime: Option<Arc<AgentRuntime>>,
}

impl RuntimeAgentBackend {
    pub fn new(factory: RuntimeFactory) -> Self {
        Self {
            factory,
            bindings: Mutex::new(HashMap::new()),
            default_max_steps: 16,
        }
    }

    pub fn with_max_steps(mut self, n: u32) -> Self {
        self.default_max_steps = n;
        self
    }

    async fn ensure_runtime(&self, session: Uuid) -> OperonResult<Arc<AgentRuntime>> {
        let mut g = self.bindings.lock().await;
        let binding = g
            .get_mut(&session)
            .ok_or_else(|| operon_core::error::OperonError::Plugin {
                plugin: "runtime-agent-backend".into(),
                source: Box::new(std::io::Error::other(format!(
                    "session {session} not bound — call bind_session first"
                ))),
            })?;
        if let Some(rt) = &binding.runtime {
            return Ok(rt.clone());
        }
        let args = RuntimeBuildArgs {
            session_id: session,
            cwd: binding.cwd.clone(),
        };
        let rt = (self.factory)(&args)?;
        binding.runtime = Some(rt.clone());
        Ok(rt)
    }
}

#[async_trait]
impl AgentBackend for RuntimeAgentBackend {
    fn id(&self) -> &str {
        "runtime"
    }

    async fn bind_session(&self, operon_session: Uuid, cwd: PathBuf) -> OperonResult<()> {
        let mut g = self.bindings.lock().await;
        let entry = g.entry(operon_session).or_insert_with(|| Binding {
            cwd: cwd.clone(),
            runtime: None,
        });
        entry.cwd = cwd;
        Ok(())
    }

    async fn cancel_tool(&self, operon_session: Uuid, tool_use_id: &str) -> bool {
        // Look up the session's runtime; cancelling a tool on an
        // unbound session is a no-op (nothing to cancel).
        let g = self.bindings.lock().await;
        let Some(binding) = g.get(&operon_session) else {
            return false;
        };
        let Some(rt) = binding.runtime.as_ref() else {
            return false;
        };
        rt.cancel_tool(tool_use_id)
    }

    async fn send_rich(
        &self,
        prompt: String,
        operon_session: Uuid,
        ct: CancellationToken,
    ) -> OperonResult<UnboundedReceiver<AgentEvent>> {
        let runtime = self.ensure_runtime(operon_session).await?;
        let max_steps = self.default_max_steps;

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
        let runtime_for_task = runtime.clone();
        tokio::spawn(async move {
            let budget = Budget::new(None, None, None, Some(max_steps));
            let mut stream = runtime_for_task.run(
                operon_session,
                Scope::User,
                prompt,
                budget,
                ct,
            );
            // Track text deltas so we can synthesize a final Done event with
            // a token-usage placeholder. The runtime emits Done(StopReason)
            // but doesn't surface usage there yet — Slice A12 follow-up.
            while let Some(step) = stream.next().await {
                if let Some(ev) = map_step(step) {
                    if tx.send(ev).is_err() {
                        break;
                    }
                }
            }
        });
        Ok(rx)
    }
}

fn map_step(step: Step) -> Option<AgentEvent> {
    match step {
        Step::Started => None,
        Step::StreamDelta(t) => Some(AgentEvent::Text(t)),
        Step::Thinking(t) => Some(AgentEvent::Thinking(t)),
        Step::ToolCall { id, name, input } => Some(AgentEvent::ToolUse { id, name, input }),
        Step::ToolResult {
            tool_use_id,
            output,
            is_error,
        } => Some(AgentEvent::ToolResult {
            tool_use_id,
            content: output.to_string(),
            is_error,
        }),
        Step::ToolChunk {
            tool_use_id,
            kind,
            bytes,
        } => Some(AgentEvent::ToolChunk {
            tool_use_id,
            kind,
            bytes,
        }),
        Step::PermissionRequest {
            id,
            title,
            kind,
            locations,
            raw_input,
        } => Some(AgentEvent::PermissionRequest {
            id,
            title,
            kind,
            locations,
            raw_input,
        }),
        Step::Done(reason) => {
            // Map runtime's StopReason → traits::StopReason.
            let stop = match reason {
                StopReason::EndTurn => operon_core::traits::StopReason::EndTurn,
                StopReason::BudgetExceeded(s) => operon_core::traits::StopReason::Other(format!(
                    "budget_exceeded: {s}"
                )),
                StopReason::Cancelled => operon_core::traits::StopReason::Other("cancelled".into()),
                StopReason::Error(e) => return Some(AgentEvent::Error(e)),
            };
            Some(AgentEvent::Done {
                stop_reason: stop,
                usage: Some(Usage::default()),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use operon_core::bus::EventBus;
    use operon_core::echo::EchoChatPlugin;
    use operon_core::memory::InMemoryStore;
    use operon_core::traits::{ChatDelta, ChatPlugin, MemoryPlugin};

    fn build_factory() -> RuntimeFactory {
        Arc::new(|_args: &RuntimeBuildArgs| {
            let chat = Arc::new(EchoChatPlugin::new(
                "echo",
                vec![vec![
                    ChatDelta::Text("hi".into()),
                    ChatDelta::Stop {
                        reason: operon_core::traits::StopReason::EndTurn,
                        usage: Some(Usage::default()),
                    },
                ]],
            ));
            let memory: Arc<dyn MemoryPlugin> = Arc::new(InMemoryStore::new());
            Ok(Arc::new(AgentRuntime::new(
                chat as Arc<dyn ChatPlugin>,
                vec![],
                memory,
                EventBus::new(8),
            )))
        })
    }

    #[tokio::test]
    async fn bind_then_send_streams_text_then_done() {
        let backend = RuntimeAgentBackend::new(build_factory());
        let session = Uuid::new_v4();
        backend
            .bind_session(session, PathBuf::from("/tmp"))
            .await
            .unwrap();
        let mut rx = backend
            .send_rich("ping".into(), session, CancellationToken::new())
            .await
            .unwrap();
        let mut events = Vec::new();
        while let Some(ev) = rx.recv().await {
            events.push(ev);
        }
        // Expect at least one Text and one Done.
        assert!(events.iter().any(|e| matches!(e, AgentEvent::Text(s) if s == "hi")));
        assert!(events.iter().any(|e| matches!(e, AgentEvent::Done { .. })));
    }

    #[tokio::test]
    async fn send_without_bind_errors() {
        let backend = RuntimeAgentBackend::new(build_factory());
        let session = Uuid::new_v4();
        let res = backend
            .send_rich("ping".into(), session, CancellationToken::new())
            .await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn rebind_updates_cwd_keeps_runtime() {
        let backend = RuntimeAgentBackend::new(build_factory());
        let session = Uuid::new_v4();
        backend.bind_session(session, PathBuf::from("/a")).await.unwrap();
        backend.bind_session(session, PathBuf::from("/b")).await.unwrap();
        let g = backend.bindings.lock().await;
        assert_eq!(g.get(&session).unwrap().cwd, PathBuf::from("/b"));
    }

    #[test]
    fn id_is_runtime() {
        let backend = RuntimeAgentBackend::new(build_factory());
        assert_eq!(backend.id(), "runtime");
    }

    #[test]
    fn map_step_started_filtered() {
        assert!(map_step(Step::Started).is_none());
    }

    #[test]
    fn map_step_text_to_text() {
        match map_step(Step::StreamDelta("x".into())).unwrap() {
            AgentEvent::Text(s) => assert_eq!(s, "x"),
            _ => panic!(),
        }
    }

    #[test]
    fn map_step_done_endturn_emits_done() {
        match map_step(Step::Done(StopReason::EndTurn)).unwrap() {
            AgentEvent::Done { .. } => {}
            _ => panic!(),
        }
    }

    #[test]
    fn map_step_done_error_emits_error() {
        match map_step(Step::Done(StopReason::Error("boom".into()))).unwrap() {
            AgentEvent::Error(s) => assert_eq!(s, "boom"),
            _ => panic!(),
        }
    }
}
