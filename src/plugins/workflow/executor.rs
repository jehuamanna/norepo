//! Workflow executor — drives one or many nodes through `claude` and
//! captures their outputs to disk. Pure async + I/O; the canvas in
//! `view.rs` calls into it from a `dioxus::spawn` task.
//!
//! v1 simplifications:
//! - Sequential cascade (one node at a time). Topo-level parallelism is
//!   a v2 concern.
//! - Output is captured by Claude's native `Write` tool to a path the
//!   prompt explicitly tells it to use, then we read it back.
//! - Skill body is inlined into the prompt verbatim. Claude doesn't
//!   need to resolve the skill via its on-disk skill loader for the
//!   workflow path; the body is right there.

#![cfg(not(target_arch = "wasm32"))]

use futures::StreamExt;
use operon_plugins_claude_code::{ClaudeCodeChatPlugin, ClaudeCodeEvent};
use operon_store::repos::{ChatMessageKind, ChatMessageRepository};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::plugins::workflow::engine::{compute_input_hash, hash_body, SkillBag, SkillSnapshot};
use crate::plugins::workflow::state::{Node, NodeId, WorkflowGraph};

/// Phase-4 cascade transcript persistence: when the cascade routes
/// through a real `chat_session`, the executor records each Claude
/// turn into `chat_message` so the rail entry is browseable like any
/// other companion chat. Both fields must be `Some` for persistence
/// to fire — per-node ▶ runs that don't go through the rail can pass
/// `None`/`None` to disable.
#[derive(Clone)]
pub struct CascadeTranscriptSink {
    pub chat_session_id: Uuid,
    pub chat_repo: Arc<dyn ChatMessageRepository>,
}

/// Canonical project-scoped outputs directory. Skill ▶ runs and
/// workflow node runs both target this directory so the explorer's
/// `Outputs` note can list everything in one place.
pub fn output_dir(repo_path: &Path) -> PathBuf {
    repo_path.join(".operon").join("outputs")
}

/// Compute the canonical output file path for a slug.
pub fn output_file(repo_path: &Path, slug: &str) -> PathBuf {
    output_dir(repo_path).join(format!("{slug}-output.md"))
}

/// Result of one successful node run. Caller writes these back into
/// the node before kicking off the next step in a cascade.
#[derive(Debug, Clone)]
pub struct RunArtifact {
    pub output_path: PathBuf,
    pub output_body: String,
    pub input_hash: String,
}

#[derive(Debug)]
pub enum ExecError {
    NoSkillBody(Uuid),
    InvalidPath(String),
    Plugin(String),
    OutputMissing(PathBuf),
    Io(std::io::Error),
    Cancelled,
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSkillBody(id) => write!(f, "skill note {id} body could not be loaded"),
            Self::InvalidPath(s) => write!(f, "invalid output path: {s}"),
            Self::Plugin(msg) => write!(f, "claude: {msg}"),
            Self::OutputMissing(p) => write!(
                f,
                "claude finished but no output file at {}",
                p.display()
            ),
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl std::error::Error for ExecError {}

impl From<std::io::Error> for ExecError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Bind one operon-session id (per workflow) to the project's repo and
/// run a single node through the claude plugin.
///
/// The caller is responsible for:
/// - having called `plugin.bind_session(operon_session, repo_path)` for
///   this `operon_session` before the first call (or having an existing
///   binding, e.g. the `chat_session` already used elsewhere)
/// - flipping the node's `status` to `Running` before the call so the UI
///   reflects the in-flight state, and to `Fresh`/`Error` afterward
///   based on the returned `Result`.
#[allow(clippy::too_many_arguments)]
pub async fn run_node(
    plugin: Arc<ClaudeCodeChatPlugin>,
    operon_session: Uuid,
    repo_path: PathBuf,
    workflow_id: Uuid,
    node_id: NodeId,
    node: &Node,
    skill_body: &str,
    skill_version: &str,
    // `skill_slug` is derived from the skill's note title and drives
    // the output filename: `<repo>/.operon/outputs/<slug>-output.md`.
    // Re-runs of the same skill (whether triggered from the standalone
    // Play button, a workflow cascade, or a per-node ▶) overwrite the
    // same file. Two workflow nodes referencing the same skill share
    // one file by design — the caller is responsible for slug
    // uniqueness if that matters.
    skill_slug: &str,
    upstream_outputs: &[(NodeId, String)],
    graph_for_hash: &WorkflowGraph,
    transcript_sink: Option<CascadeTranscriptSink>,
) -> Result<RunArtifact, ExecError> {
    if skill_body.trim().is_empty() {
        return Err(ExecError::NoSkillBody(node.skill_note_id));
    }
    let _ = workflow_id; // Kept for callers that may want it later (cwd subdir, telemetry).

    // Unified output path: one folder per repo, file named after the
    // skill's slug. See doc-comment on `skill_slug` above for the
    // overwrite semantics.
    let out_dir = output_dir(&repo_path);
    std::fs::create_dir_all(&out_dir)?;
    let output_path = out_dir.join(format!("{skill_slug}-output.md"));

    // Build the prompt.
    let prompt = build_prompt(
        node,
        skill_body,
        skill_version,
        upstream_outputs,
        &output_path,
    );

    // Pre-clear any stale output file so we can detect "claude said done
    // but never wrote anything" with a clean File::exists check.
    if output_path.exists() {
        std::fs::remove_file(&output_path)?;
    }

    // Compute the input hash that will be cached on success. The graph
    // snapshot supplied here MUST reflect the current upstream cached
    // outputs for the hash to be meaningful — the caller is expected to
    // pass a graph with all upstreams already settled.
    let mut skill_bag = SkillBag::new();
    skill_bag.insert(
        node.skill_note_id,
        SkillSnapshot {
            version: skill_version.to_string(),
            body_hash: hash_body(skill_body),
        },
    );
    let input_hash = compute_input_hash(node_id, graph_for_hash, &skill_bag)
        .map_err(|e| ExecError::Plugin(format!("hash: {e}")))?;

    // Phase-4: persist the prompt as a User message before send_rich
    // so the rail's transcript reads "user: <prompt> / assistant: …"
    // for each cascade turn. The user-visible label includes the node
    // id so a multi-node cascade is readable.
    if let Some(sink) = transcript_sink.as_ref() {
        let header = format!("[workflow node {node_id}]");
        let _ = sink.chat_repo.append(
            sink.chat_session_id,
            ChatMessageKind::User,
            None,
            &serde_json::json!({ "text": format!("{header}\n\n{prompt}") }),
        );
    }

    // Run claude.
    eprintln!(
        "operon: executor::run_node [{node_id}] calling plugin.send_rich \
         (prompt_len={}, output_path={})",
        prompt.len(),
        output_path.display()
    );
    let ct = CancellationToken::new();
    let mut rx = plugin
        .send_rich(prompt, operon_session, ct)
        .await
        .map_err(|e| ExecError::Plugin(format!("send_rich: {e}")))?;
    eprintln!("operon: executor::run_node [{node_id}] send_rich returned, draining events");

    // Accumulate Text deltas across the turn — claude streams them in
    // pieces, but the rail wants ONE assistant row per turn (matches
    // the regular companion's `flush_pending_assistant` behavior).
    let mut assistant_buf = String::new();
    let flush_assistant = |buf: &mut String| {
        if buf.is_empty() {
            return;
        }
        if let Some(sink) = transcript_sink.as_ref() {
            // Body shape MUST be `{ "body": "<text>" }` to match
            // `transcript_item_from_message`'s Assistant case in
            // `companion_chat`. The earlier `{ "text": ... }` shape
            // caused every assistant message to be filtered out of
            // the rail's transcript.
            let _ = sink.chat_repo.append(
                sink.chat_session_id,
                ChatMessageKind::Assistant,
                None,
                &serde_json::json!({ "body": std::mem::take(buf) }),
            );
        } else {
            buf.clear();
        }
    };

    let mut events = 0usize;
    while let Some(ev) = rx.next().await {
        events += 1;
        let kind = match &ev {
            ClaudeCodeEvent::Done { .. } => "Done",
            ClaudeCodeEvent::Error(_) => "Error",
            ClaudeCodeEvent::Text(_) => "Text",
            ClaudeCodeEvent::Thinking(_) => "Thinking",
            ClaudeCodeEvent::ToolUse { .. } => "ToolUse",
            ClaudeCodeEvent::ToolResult { .. } => "ToolResult",
        };
        eprintln!("operon: executor::run_node [{node_id}] event {events}: {kind}");
        // Persist + apply the event's effect.
        match ev {
            ClaudeCodeEvent::Text(t) => {
                assistant_buf.push_str(&t);
            }
            ClaudeCodeEvent::Thinking(t) => {
                flush_assistant(&mut assistant_buf);
                if let Some(sink) = transcript_sink.as_ref() {
                    let _ = sink.chat_repo.append(
                        sink.chat_session_id,
                        ChatMessageKind::Thinking,
                        None,
                        &serde_json::json!({ "text": t }),
                    );
                }
            }
            ClaudeCodeEvent::ToolUse { id, name, input } => {
                flush_assistant(&mut assistant_buf);
                if let Some(sink) = transcript_sink.as_ref() {
                    let _ = sink.chat_repo.append(
                        sink.chat_session_id,
                        ChatMessageKind::ToolCall,
                        Some(&id),
                        &serde_json::json!({
                            "id": id,
                            "name": name,
                            "input": input,
                            "result": serde_json::Value::Null,
                        }),
                    );
                }
            }
            ClaudeCodeEvent::ToolResult { tool_use_id, content, is_error } => {
                if let Some(sink) = transcript_sink.as_ref() {
                    // Patch the prior ToolCall row so the rail reads as
                    // a complete round-trip card. Same shape regular
                    // companion uses (companion_chat.rs::apply_event).
                    let _ = sink.chat_repo.update_tool_result(
                        sink.chat_session_id,
                        &tool_use_id,
                        &serde_json::json!({
                            "id": tool_use_id,
                            "result": {
                                "content": content,
                                "is_error": is_error,
                            },
                        }),
                    );
                }
            }
            ClaudeCodeEvent::Done { .. } => {
                flush_assistant(&mut assistant_buf);
                break;
            }
            ClaudeCodeEvent::Error(msg) => {
                flush_assistant(&mut assistant_buf);
                if let Some(sink) = transcript_sink.as_ref() {
                    let _ = sink.chat_repo.append(
                        sink.chat_session_id,
                        ChatMessageKind::System,
                        None,
                        &serde_json::json!({ "text": format!("error: {msg}") }),
                    );
                }
                return Err(ExecError::Plugin(msg));
            }
        }
    }
    // In case the stream ended without a Done event, flush any
    // accumulated assistant text so it doesn't get lost.
    flush_assistant(&mut assistant_buf);
    eprintln!(
        "operon: executor::run_node [{node_id}] event-stream ended, {events} event(s) total"
    );

    // claude's Write tool reports success before fsync; tolerate a
    // tight retry window.
    let body = read_with_retry(&output_path, 5)?;
    Ok(RunArtifact {
        output_path,
        output_body: body,
        input_hash,
    })
}

/// Read upstream output bodies for `node_id`. Reads from disk via the
/// `cached_output_path` recorded on each upstream Node. Missing files
/// are skipped (caller should already have run those nodes; if they
/// haven't, the prompt is best-effort with the upstreams that do
/// exist).
pub fn collect_upstream_outputs(
    graph: &WorkflowGraph,
    node_id: NodeId,
) -> Result<Vec<(NodeId, String)>, ExecError> {
    let mut out = Vec::new();
    for upstream_id in graph.upstream_of(node_id) {
        if let Some(node) = graph.nodes.get(&upstream_id) {
            if let Some(path) = node.cached_output_path.as_ref() {
                if path.exists() {
                    let body = std::fs::read_to_string(path)?;
                    out.push((upstream_id, body));
                }
            }
        }
    }
    Ok(out)
}

fn build_prompt(
    node: &Node,
    skill_body: &str,
    skill_version: &str,
    upstream_outputs: &[(NodeId, String)],
    output_path: &Path,
) -> String {
    let mut buf = String::new();
    buf.push_str("You are running a workflow node. Follow the skill below and write your\n");
    buf.push_str("output (markdown body, optionally preceded by YAML frontmatter) to the\n");
    buf.push_str("path provided at the end of this prompt — using the Write tool.\n\n");
    buf.push_str(&format!(
        "Skill version: {}\n",
        if skill_version.is_empty() {
            "(unspecified)"
        } else {
            skill_version
        }
    ));
    buf.push_str("\n--- skill body ---\n");
    buf.push_str(skill_body.trim_end());
    buf.push_str("\n--- /skill body ---\n\n");

    if !node.typed_fields.is_null() {
        buf.push_str("--- typed inputs (JSON) ---\n");
        if let Ok(pretty) = serde_json::to_string_pretty(&node.typed_fields) {
            buf.push_str(&pretty);
        } else {
            buf.push_str(&node.typed_fields.to_string());
        }
        buf.push_str("\n--- /typed inputs ---\n\n");
    }

    if !node.extra_instructions.trim().is_empty() {
        buf.push_str("--- extra instructions ---\n");
        buf.push_str(node.extra_instructions.trim_end());
        buf.push_str("\n--- /extra instructions ---\n\n");
    }

    if !upstream_outputs.is_empty() {
        buf.push_str("--- upstream outputs ---\n");
        for (id, body) in upstream_outputs {
            buf.push_str(&format!("=== from upstream node {id} ===\n"));
            buf.push_str(body.trim_end());
            buf.push('\n');
        }
        buf.push_str("--- /upstream outputs ---\n\n");
    }

    buf.push_str(&format!(
        "Write your output to (absolute path): {}\n",
        output_path.display()
    ));
    buf
}

/// Re-read the output file with bounded retries to tolerate the
/// fsync-after-Write-tool race claude has on some platforms. Returns
/// `OutputMissing` if the file never appears.
fn read_with_retry(path: &Path, attempts: usize) -> Result<String, ExecError> {
    for _ in 0..attempts {
        if path.exists() {
            return Ok(std::fs::read_to_string(path)?);
        }
        std::thread::sleep(std::time::Duration::from_millis(80));
    }
    Err(ExecError::OutputMissing(path.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::workflow::state::NodeStatus;

    fn node_for(id: NodeId, skill: Uuid, extra: &str) -> Node {
        Node {
            id,
            skill_note_id: skill,
            typed_fields: serde_json::json!({"a": 1}),
            extra_instructions: extra.into(),
            position: (0.0, 0.0),
            cached_output_path: None,
            cached_input_hash: None,
            cached_output_note_id: None,
            status: NodeStatus::Dirty,
        }
    }

    #[test]
    fn build_prompt_includes_skill_body_and_target_path() {
        let id = Uuid::new_v4();
        let n = node_for(id, Uuid::new_v4(), "");
        let prompt = build_prompt(
            &n,
            "you are a BA",
            "1",
            &[],
            Path::new("/tmp/o.md"),
        );
        assert!(prompt.contains("you are a BA"));
        assert!(prompt.contains("/tmp/o.md"));
        assert!(prompt.contains("Skill version: 1"));
        assert!(!prompt.contains("upstream outputs"));
    }

    #[test]
    fn build_prompt_omits_empty_sections() {
        let id = Uuid::new_v4();
        let mut n = node_for(id, Uuid::new_v4(), "");
        n.typed_fields = serde_json::Value::Null;
        n.extra_instructions = String::new();
        let prompt = build_prompt(&n, "body", "", &[], Path::new("/tmp/x"));
        assert!(!prompt.contains("typed inputs"));
        assert!(!prompt.contains("extra instructions"));
        assert!(prompt.contains("(unspecified)"));
    }

    #[test]
    fn build_prompt_lists_upstream_outputs() {
        let id = Uuid::new_v4();
        let n = node_for(id, Uuid::new_v4(), "tweak");
        let up_a = Uuid::new_v4();
        let up_b = Uuid::new_v4();
        let prompt = build_prompt(
            &n,
            "body",
            "2",
            &[(up_a, "alpha out".into()), (up_b, "beta out".into())],
            Path::new("/tmp/x"),
        );
        assert!(prompt.contains("alpha out"));
        assert!(prompt.contains("beta out"));
        assert!(prompt.contains(&up_a.to_string()));
        assert!(prompt.contains("tweak"));
    }

    #[test]
    fn read_with_retry_returns_missing_for_absent_file() {
        let p = std::path::PathBuf::from("/tmp/operon-test-this-should-not-exist-xyz123.md");
        let err = read_with_retry(&p, 1).unwrap_err();
        assert!(matches!(err, ExecError::OutputMissing(_)));
    }

    #[test]
    fn read_with_retry_returns_body_on_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("out.md");
        std::fs::write(&p, "hello world").unwrap();
        let body = read_with_retry(&p, 1).unwrap();
        assert_eq!(body, "hello world");
    }
}
