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
use crate::shell::companion_state::{ArtifactRunState, ARTIFACT_RUN_STATE};

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

/// Result of one successful node run. Caller writes these back into
/// the node before kicking off the next step in a cascade.
///
/// Multi-output support: `produced` lists every `.md` file claude
/// wrote during this run, in lexicographic order — one element for
/// `output_count: one` skills, N for `output_count: many`. The legacy
/// `output_path` / `output_body` fields hold the FIRST produced file
/// so callers that haven't been ported to iterate `produced` keep
/// working (they see the lead artifact).
#[derive(Debug, Clone)]
pub struct RunArtifact {
    pub output_path: PathBuf,
    pub output_body: String,
    pub input_hash: String,
    pub produced: Vec<(PathBuf, String)>,
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

    // Per-node output directory: each run's produced .md files are
    // imported as their own Outputs notes, so multi-output skills
    // (ba-discover-epics, ba-decompose-features, …) can write N
    // separate files. Path is keyed on (workflow_id, node_id) so
    // re-runs on the same node clear and repopulate ONE dir, while
    // sibling nodes don't collide.
    let out_dir = output_dir(&repo_path)
        .join(workflow_id.to_string())
        .join(node_id.to_string());
    // Clear stale outputs from any prior run on this node so the
    // scan-for-produced-files diff has a clean starting state.
    if out_dir.exists() {
        for entry in std::fs::read_dir(&out_dir)?.flatten() {
            let p = entry.path();
            if p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("md") {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
    std::fs::create_dir_all(&out_dir)?;
    let _ = skill_slug; // legacy single-file slug naming no longer used

    // Pre-snapshot directory state — `scan_produced_files` diffs
    // against this set + run-start mtime to find the files this run
    // actually wrote.
    let pre_existing: std::collections::HashSet<PathBuf> = list_md_files(&out_dir);
    let run_started_at = std::time::SystemTime::now();

    // Build the prompt — points to the directory, not a single path,
    // and instructs Claude to use Write ONCE PER ARTIFACT.
    let prompt = build_prompt(
        node,
        skill_body,
        skill_version,
        upstream_outputs,
        &out_dir,
    );

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
        bump_message_version();
    }

    // Stamp the global run-state map so the companion's
    // "Claude is thinking…" loader renders for the entire duration
    // of this Claude subprocess. The companion's match arms read
    // ARTIFACT_RUN_STATE keyed on the chat session id (which is the
    // same as `operon_session` here for workflow runs) and surface
    // the spinner whenever the entry is `Running`. We always either
    // mutate to `Done` (Ok path) or `Failed` (every error path after
    // this point) so the loader doesn't get stuck.
    ARTIFACT_RUN_STATE.with_mut(|m| {
        m.insert(operon_session, ArtifactRunState::Running);
    });

    // Run claude.
    eprintln!(
        "operon: executor::run_node [{node_id}] calling plugin.send_rich \
         (prompt_len={}, out_dir={})",
        prompt.len(),
        out_dir.display()
    );
    let ct = CancellationToken::new();
    let mut rx = match plugin.send_rich(prompt, operon_session, ct).await {
        Ok(rx) => rx,
        Err(e) => {
            let reason = format!("send_rich: {e}");
            ARTIFACT_RUN_STATE.with_mut(|m| {
                m.insert(operon_session, ArtifactRunState::Failed { reason: reason.clone() });
            });
            return Err(ExecError::Plugin(reason));
        }
    };
    eprintln!("operon: executor::run_node [{node_id}] send_rich returned, draining events");

    // Accumulate Text deltas across the turn — claude streams them in
    // pieces, but the rail wants ONE assistant row per turn (matches
    // the regular companion's `flush_pending_assistant` behavior).
    let mut assistant_buf = String::new();
    let flush_assistant = |buf: &mut String| {
        if let Some(sink) = transcript_sink.as_ref() {
            let appended = !buf.is_empty();
            if appended {
                let _ = sink.chat_repo.append(
                    sink.chat_session_id,
                    ChatMessageKind::Assistant,
                    None,
                    &serde_json::json!({ "body": std::mem::take(buf) }),
                );
            }
            // Always clear the streaming entry on flush so the
            // transient block disappears even when there's no text
            // to persist (e.g., flush before a Thinking block).
            crate::shell::companion_state::INPROGRESS_ASSISTANT.with_mut(|m| {
                m.remove(&sink.chat_session_id);
            });
            if appended {
                bump_message_version();
            }
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
                // Stream the delta into the in-progress map so the
                // companion's render shows letter-by-letter typing.
                // No CHAT_MESSAGE_VERSION bump needed — the companion
                // subscribes to INPROGRESS_ASSISTANT directly, and
                // bumping the version here would force a DB re-fetch
                // per character (slow + noisy).
                if let Some(sink) = transcript_sink.as_ref() {
                    let chat_session_id = sink.chat_session_id;
                    crate::shell::companion_state::INPROGRESS_ASSISTANT.with_mut(|m| {
                        m.entry(chat_session_id).or_default().push_str(&t);
                    });
                }
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
                    bump_message_version();
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
                    bump_message_version();
                    // Phase F: mirror Write tool content into the
                    // rail as a readable Assistant message — same
                    // pattern as the artifact runner.
                    if name == "Write" {
                        if let Some(content) =
                            input.get("content").and_then(|v| v.as_str())
                        {
                            let path = input
                                .get("file_path")
                                .and_then(|v| v.as_str())
                                .unwrap_or("artifact");
                            let label = std::path::Path::new(path)
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or(path);
                            let body =
                                format!("\u{1F4C4} **{label}**\n\n{content}");
                            let _ = sink.chat_repo.append(
                                sink.chat_session_id,
                                ChatMessageKind::Assistant,
                                None,
                                &serde_json::json!({ "body": body }),
                            );
                            bump_message_version();
                        }
                    }
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
                    bump_message_version();
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
                    bump_message_version();
                }
                ARTIFACT_RUN_STATE.with_mut(|m| {
                    m.insert(
                        operon_session,
                        ArtifactRunState::Failed { reason: msg.clone() },
                    );
                });
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

    // Scan the per-node output dir for files this run produced —
    // either entirely new (not in pre_existing) or modified after
    // run_started_at (re-runs that overwrote a stale file). Mirrors
    // the artifact runner's import semantics. Bounded retries
    // tolerate the fsync-after-Write race.
    let mut produced: Vec<(PathBuf, String)> = Vec::new();
    for _attempt in 0..5 {
        produced = scan_produced_files(&out_dir, &pre_existing, run_started_at);
        if !produced.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(80));
    }
    if produced.is_empty() {
        ARTIFACT_RUN_STATE.with_mut(|m| {
            m.insert(
                operon_session,
                ArtifactRunState::Failed {
                    reason: format!(
                        "claude finished but no .md files appeared in {}",
                        out_dir.display()
                    ),
                },
            );
        });
        return Err(ExecError::OutputMissing(out_dir.clone()));
    }
    // Lead-output backward compat for callers that still read
    // `output_path` / `output_body` directly (the inspector's
    // "last output" panel, hash-based dirty propagation).
    let (output_path, output_body) = produced
        .first()
        .cloned()
        .expect("non-empty by guard above");
    ARTIFACT_RUN_STATE.with_mut(|m| {
        m.insert(
            operon_session,
            ArtifactRunState::Done {
                artifact_count: produced.len(),
            },
        );
    });
    Ok(RunArtifact {
        output_path,
        output_body,
        input_hash,
        produced,
    })
}

/// List `.md` files in `dir` (top-level only). Returns absolute
/// canonicalised paths so the diff against post-run state is stable
/// regardless of how `dir` was originally constructed.
fn list_md_files(dir: &Path) -> std::collections::HashSet<PathBuf> {
    let mut out = std::collections::HashSet::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(it) => it,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("md") {
            if let Ok(canon) = path.canonicalize() {
                out.insert(canon);
            } else {
                out.insert(path);
            }
        }
    }
    out
}

/// Find files in `dir` that are either NEW (not in `pre_existing`) or
/// modified after `run_started_at`. Returns `(path, body)` pairs in
/// lexicographic order so imports are deterministic across re-runs.
fn scan_produced_files(
    dir: &Path,
    pre_existing: &std::collections::HashSet<PathBuf>,
    run_started_at: std::time::SystemTime,
) -> Vec<(PathBuf, String)> {
    let mut out: Vec<(PathBuf, String)> = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(it) => it,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !(path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("md")) {
            continue;
        }
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        let is_new = !pre_existing.contains(&canonical);
        let is_recent = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .map(|t| t >= run_started_at)
            .unwrap_or(false);
        if is_new || is_recent {
            if let Ok(body) = std::fs::read_to_string(&path) {
                out.push((canonical, body));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Bump the global live-transcript version so the companion's poll
/// loop re-fetches `chat_message`. Same rationale as the mirror in
/// `plugins::artifact::runner::bump_message_version`: the executor
/// task lives in the virtual root scope (via `spawn_forever`),
/// where writes to a scope-bound `Signal` are silently dropped.
/// `CHAT_MESSAGE_VERSION` is a `GlobalSignal` so it's safe to
/// mutate from any scope.
fn bump_message_version() {
    crate::shell::companion_state::CHAT_MESSAGE_VERSION.with_mut(|v| {
        *v = v.saturating_add(1);
    });
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
    out_dir: &Path,
) -> String {
    let mut buf = String::new();
    buf.push_str(
        "You are running a workflow node. Follow the skill below and use the\n\
         Write tool to produce one or more artifact files in the directory\n\
         given at the end of this prompt.\n\n\
         **One artifact = one Write call = one file.** If the skill says\n\
         `output_count: many` and you produce N artifacts, call the Write\n\
         tool N times — once per artifact, each to its own `.md` file in\n\
         the directory below. Do NOT concatenate multiple artifacts into a\n\
         single file with `# filename.md` header separators; the engine\n\
         imports each `.md` file as its own note, so concatenating loses\n\
         every artifact except the first. If the skill says\n\
         `output_count: one`, call Write exactly once with a single `.md`\n\
         file in the directory.\n\n",
    );
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
        "Output directory (absolute path): {}\n\n\
         Use kebab-case filenames that match what the skill body suggests\n\
         (e.g. `epic-01-core-timer-engine.md`, `feature-02-cycle-workflow.md`).\n\
         Each file MUST start with YAML frontmatter (`---` block) declaring\n\
         the artifact_kind the skill produces, then the markdown body. The\n\
         first heading should match the file's purpose in human-readable\n\
         form.\n",
        out_dir.display()
    ));
    buf
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
            is_artifact_snapshot: false,
            artifact_ref: None,
            artifact_kind_label: None,
            artifact_title: None,
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
    fn scan_produced_files_picks_up_new_md_files() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join("stale.md");
        std::fs::write(&stale, "old").unwrap();
        let pre = list_md_files(dir.path());
        let started = std::time::SystemTime::now();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(dir.path().join("epic-01.md"), "alpha").unwrap();
        std::fs::write(dir.path().join("epic-02.md"), "beta").unwrap();
        let produced = scan_produced_files(dir.path(), &pre, started);
        assert_eq!(produced.len(), 2);
        assert_eq!(produced[0].1, "alpha");
        assert_eq!(produced[1].1, "beta");
    }

    #[test]
    fn scan_produced_files_includes_overwritten_pre_existing() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("epic-01.md");
        std::fs::write(&p, "original").unwrap();
        let pre = list_md_files(dir.path());
        let started = std::time::SystemTime::now();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&p, "rewritten").unwrap();
        let produced = scan_produced_files(dir.path(), &pre, started);
        assert_eq!(produced.len(), 1);
        assert_eq!(produced[0].1, "rewritten");
    }
}
