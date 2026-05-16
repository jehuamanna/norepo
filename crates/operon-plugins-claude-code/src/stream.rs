//! Stream-json event parsing + the per-turn driver that pumps lines from
//! `claude --output-format stream-json` into `ClaudeCodeEvent`s. A single
//! claude line can produce multiple events (an `assistant` message with
//! both text and a tool_use yields two), so the parser returns a `Vec`.

#![cfg(not(target_arch = "wasm32"))]

use futures::channel::mpsc::UnboundedSender;
use operon_core::error::OperonError;
use operon_core::traits::{CancellationToken, StopReason, Usage};
use std::sync::{Arc, Mutex};
use tokio::io::{BufReader, Lines};
use tokio::process::{Child, ChildStderr, ChildStdout};
use uuid::Uuid;

use crate::event::ClaudeCodeEvent;
use crate::plugin::PluginState;

pub(crate) struct ClaudeProcess {
    pub child: Child,
    pub stdout: Lines<BufReader<ChildStdout>>,
    pub stderr: Option<Lines<BufReader<ChildStderr>>>,
}

pub(crate) async fn drive_stream(
    mut proc: ClaudeProcess,
    tx: UnboundedSender<ClaudeCodeEvent>,
    ct: CancellationToken,
    state: Arc<Mutex<PluginState>>,
    operon_session: Uuid,
) {
    let mut stderr_buf = String::new();
    let mut closed = false;
    loop {
        tokio::select! {
            _ = ct.cancelled() => {
                let _ = proc.child.start_kill();
                let _ = tx.unbounded_send(ClaudeCodeEvent::Error(
                    OperonError::Cancelled.to_string(),
                ));
                closed = true;
                break;
            }
            line_res = proc.stdout.next_line() => {
                match line_res {
                    Ok(Some(line)) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        for ev in parse_line(&line, &state, operon_session) {
                            if tx.unbounded_send(ev).is_err() {
                                let _ = proc.child.start_kill();
                                closed = true;
                                break;
                            }
                        }
                        if closed { break; }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        let _ = tx.unbounded_send(ClaudeCodeEvent::Error(
                            format!("stdout read: {e}"),
                        ));
                        break;
                    }
                }
            }
            stderr_res = read_stderr_line(&mut proc.stderr) => {
                if let Some(line) = stderr_res {
                    if !line.trim().is_empty() {
                        stderr_buf.push_str(&line);
                        stderr_buf.push('\n');
                        tracing::warn!(target: "claude-code", "stderr: {}", line);
                    }
                }
            }
        }
    }
    let exit = proc.child.wait().await;
    if let Ok(status) = exit {
        if !status.success() && !closed {
            let msg = if stderr_buf.is_empty() {
                format!("claude exited with {status}")
            } else {
                format!("claude exited with {status}: {stderr_buf}")
            };
            let _ = tx.unbounded_send(ClaudeCodeEvent::Error(msg));
        }
    }
}

async fn read_stderr_line(reader: &mut Option<Lines<BufReader<ChildStderr>>>) -> Option<String> {
    match reader {
        Some(r) => r.next_line().await.ok().flatten(),
        None => {
            futures::future::pending::<()>().await;
            None
        }
    }
}

/// Parse a single stream-json line into zero or more `ClaudeCodeEvent`s.
/// `assistant` events with multiple content blocks (text + tool_use, etc.)
/// fan out into multiple events; `user` events with `tool_result` blocks
/// fan out into one `ToolResult` per block.
pub(crate) fn parse_line(
    line: &str,
    state: &Arc<Mutex<PluginState>>,
    operon_session: Uuid,
) -> Vec<ClaudeCodeEvent> {
    let v: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let kind = match v.get("type").and_then(|t| t.as_str()) {
        Some(k) => k,
        None => return Vec::new(),
    };
    match kind {
        "assistant" => parse_assistant_blocks(&v),
        "user" => parse_user_blocks(&v),
        "result" => parse_result(&v, state, operon_session),
        "system" => parse_system(&v),
        "stream_event" => parse_stream_event(&v),
        "error" => {
            let msg = v
                .get("error")
                .and_then(|e| {
                    e.get("message")
                        .and_then(|m| m.as_str())
                        .or_else(|| e.as_str())
                })
                .unwrap_or("unknown error")
                .to_string();
            vec![ClaudeCodeEvent::Error(msg)]
        }
        // rate_limit_event / unknown — drop silently
        _ => Vec::new(),
    }
}

/// Parse a `stream_event` envelope — emitted only when claude was spawned
/// with `--include-partial-messages`. The envelope's `event` field carries
/// a raw Anthropic SSE event (`content_block_start`, `content_block_delta`,
/// `content_block_stop`, `message_*`); we surface only the `thinking_delta`
/// deltas so the chat UI can stream extended-thinking content live.
///
/// Text deltas are intentionally NOT surfaced here: the non-partial
/// `assistant` envelope already arrives at end-of-turn with the full text,
/// and the chat layer collates Text deltas by appending — emitting both
/// the partials and the final block would double the rendered text.
/// Thinking is the inverse: claude-code v2.1.8+ strips `thinking` blocks
/// from the non-partial envelope, so partial deltas are the only signal.
fn parse_stream_event(v: &serde_json::Value) -> Vec<ClaudeCodeEvent> {
    let event = match v.get("event") {
        Some(e) => e,
        None => return Vec::new(),
    };
    let ev_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
    if ev_type != "content_block_delta" {
        return Vec::new();
    }
    let delta = match event.get("delta") {
        Some(d) => d,
        None => return Vec::new(),
    };
    if delta.get("type").and_then(|t| t.as_str()) != Some("thinking_delta") {
        return Vec::new();
    }
    let text = delta
        .get("thinking")
        .and_then(|x| x.as_str())
        .unwrap_or("");
    if text.is_empty() {
        return Vec::new();
    }
    vec![ClaudeCodeEvent::Thinking(text.to_string())]
}

/// Parse a claude `system` envelope. Only the `init` subtype carries the
/// MCP server roster + tool inventory we care about; other subtypes are
/// dropped silently to preserve current behaviour.
fn parse_system(v: &serde_json::Value) -> Vec<ClaudeCodeEvent> {
    let subtype = v.get("subtype").and_then(|s| s.as_str()).unwrap_or("");
    if subtype != "init" {
        return Vec::new();
    }
    let mcp_servers = v
        .get("mcp_servers")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|entry| {
                    let name = entry.get("name").and_then(|n| n.as_str())?;
                    let status = entry
                        .get("status")
                        .and_then(|s| s.as_str())
                        .unwrap_or("unknown");
                    Some(operon_core::agent_event::McpServerStatus {
                        name: name.to_string(),
                        status: status.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let tools = v
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    vec![ClaudeCodeEvent::SessionInit { mcp_servers, tools }]
}

fn parse_assistant_blocks(v: &serde_json::Value) -> Vec<ClaudeCodeEvent> {
    let blocks = match v
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    {
        Some(b) => b,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for block in blocks {
        let bk = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match bk {
            "text" => {
                if let Some(t) = block.get("text").and_then(|x| x.as_str()) {
                    if !t.is_empty() {
                        out.push(ClaudeCodeEvent::Text(t.to_string()));
                    }
                }
            }
            "thinking" => {
                if let Some(t) = block.get("thinking").and_then(|x| x.as_str()) {
                    if !t.is_empty() {
                        out.push(ClaudeCodeEvent::Thinking(t.to_string()));
                    }
                }
            }
            "tool_use" => {
                let id = block
                    .get("id")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = block
                    .get("name")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let input = block
                    .get("input")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                out.push(ClaudeCodeEvent::ToolUse { id, name, input });
            }
            _ => {}
        }
    }
    out
}

fn parse_user_blocks(v: &serde_json::Value) -> Vec<ClaudeCodeEvent> {
    // claude reports tool results as user-role events with tool_result
    // content blocks. The content field can be either a string (legacy)
    // or an array of typed blocks.
    let blocks = v
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array());
    let blocks = match blocks {
        Some(b) => b,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for block in blocks {
        if block.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
            continue;
        }
        let tool_use_id = block
            .get("tool_use_id")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let is_error = block
            .get("is_error")
            .and_then(|x| x.as_bool())
            .unwrap_or(false);
        let content = match block.get("content") {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Array(items)) => {
                // Concatenate any text-typed sub-blocks.
                let mut acc = String::new();
                for it in items {
                    if it.get("type").and_then(|t| t.as_str()) == Some("text") {
                        if let Some(t) = it.get("text").and_then(|x| x.as_str()) {
                            if !acc.is_empty() {
                                acc.push('\n');
                            }
                            acc.push_str(t);
                        }
                    }
                }
                acc
            }
            Some(other) => other.to_string(),
            None => String::new(),
        };
        out.push(ClaudeCodeEvent::ToolResult {
            tool_use_id,
            content,
            is_error,
        });
    }
    out
}

fn parse_result(
    v: &serde_json::Value,
    state: &Arc<Mutex<PluginState>>,
    operon_session: Uuid,
) -> Vec<ClaudeCodeEvent> {
    // Cache claude's session_id against the in-flight Operon session
    // so the next turn can `--resume` it.
    if let Some(sid) = v.get("session_id").and_then(|x| x.as_str()) {
        if let Ok(mut s) = state.lock() {
            if let Some(b) = s.bindings.get_mut(&operon_session) {
                b.claude_session_id = Some(sid.to_string());
            }
        }
    }
    if v.get("is_error").and_then(|x| x.as_bool()).unwrap_or(false) {
        let msg = v
            .get("result")
            .and_then(|r| r.as_str())
            .unwrap_or("claude reported error")
            .to_string();
        return vec![ClaudeCodeEvent::Error(msg)];
    }
    let stop_reason = v
        .get("stop_reason")
        .and_then(|x| x.as_str())
        .map(map_stop_reason)
        .unwrap_or(StopReason::EndTurn);
    let usage = v.get("usage").map(|u| Usage {
        prompt: u
            .get("input_tokens")
            .and_then(|x| x.as_u64())
            .unwrap_or(0)
            + u.get("cache_creation_input_tokens")
                .and_then(|x| x.as_u64())
                .unwrap_or(0)
            + u.get("cache_read_input_tokens")
                .and_then(|x| x.as_u64())
                .unwrap_or(0),
        prompt_cached: u
            .get("cache_read_input_tokens")
            .and_then(|x| x.as_u64())
            .unwrap_or(0),
        completion: u
            .get("output_tokens")
            .and_then(|x| x.as_u64())
            .unwrap_or(0),
    });
    vec![ClaudeCodeEvent::Done {
        stop_reason,
        usage,
    }]
}

fn map_stop_reason(s: &str) -> StopReason {
    match s {
        "end_turn" => StopReason::EndTurn,
        "max_tokens" => StopReason::MaxTokens,
        "stop_sequence" => StopReason::StopSequence,
        "tool_use" => StopReason::Tool,
        other => StopReason::Other(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::SessionBinding;
    use std::path::PathBuf;

    fn fresh_state_with_binding(sid: Uuid) -> Arc<Mutex<PluginState>> {
        let mut state = PluginState::default();
        state.bindings.insert(
            sid,
            SessionBinding {
                cwd: PathBuf::from("/tmp/repo"),
                claude_session_id: None,
                permission_mode: None,
                bridge: None,
                bash_via_operon: false,
                extra_dirs: Vec::new(),
            },
        );
        Arc::new(Mutex::new(state))
    }

    #[test]
    fn parses_assistant_text_block() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hello"}]}}"#;
        let sid = Uuid::new_v4();
        let state = fresh_state_with_binding(sid);
        let events = parse_line(line, &state, sid);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ClaudeCodeEvent::Text(t) => assert_eq!(t, "hello"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn parses_assistant_text_and_tool_use_in_one_message() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"reading file"},{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/tmp/foo"}}]}}"#;
        let sid = Uuid::new_v4();
        let state = fresh_state_with_binding(sid);
        let events = parse_line(line, &state, sid);
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], ClaudeCodeEvent::Text(ref t) if t == "reading file"));
        match &events[1] {
            ClaudeCodeEvent::ToolUse { id, name, input } => {
                assert_eq!(id, "t1");
                assert_eq!(name, "Read");
                assert_eq!(input["file_path"], "/tmp/foo");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn parses_assistant_thinking_block() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hmm let me think"}]}}"#;
        let sid = Uuid::new_v4();
        let state = fresh_state_with_binding(sid);
        let events = parse_line(line, &state, sid);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ClaudeCodeEvent::Thinking(t) => assert_eq!(t, "hmm let me think"),
            other => panic!("expected Thinking, got {other:?}"),
        }
    }

    #[test]
    fn parses_stream_event_thinking_delta() {
        // claude --include-partial-messages emits one of these per chunk
        // of extended thinking. The chat layer coalesces successive
        // deltas into a single Thinking block, so partial chunks must
        // make it through parse_line without being dropped.
        let line = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"weighing options"}}}"#;
        let sid = Uuid::new_v4();
        let state = fresh_state_with_binding(sid);
        let events = parse_line(line, &state, sid);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ClaudeCodeEvent::Thinking(t) => assert_eq!(t, "weighing options"),
            other => panic!("expected Thinking, got {other:?}"),
        }
    }

    #[test]
    fn stream_event_text_delta_is_dropped() {
        // Text deltas inside `stream_event` envelopes are intentionally
        // ignored; the full assistant text arrives in a non-partial
        // `assistant` envelope and emitting both would double-render.
        let line = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}}"#;
        let sid = Uuid::new_v4();
        let state = fresh_state_with_binding(sid);
        assert!(parse_line(line, &state, sid).is_empty());
    }

    #[test]
    fn stream_event_non_delta_subtypes_are_dropped() {
        let sid = Uuid::new_v4();
        let state = fresh_state_with_binding(sid);
        assert!(parse_line(
            r#"{"type":"stream_event","event":{"type":"content_block_start"}}"#,
            &state,
            sid,
        )
        .is_empty());
        assert!(parse_line(
            r#"{"type":"stream_event","event":{"type":"message_stop"}}"#,
            &state,
            sid,
        )
        .is_empty());
    }

    #[test]
    fn parses_user_tool_result_string_content() {
        let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"file content","is_error":false}]}}"#;
        let sid = Uuid::new_v4();
        let state = fresh_state_with_binding(sid);
        let events = parse_line(line, &state, sid);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ClaudeCodeEvent::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "t1");
                assert_eq!(content, "file content");
                assert!(!is_error);
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn parses_user_tool_result_array_content() {
        let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"line1"},{"type":"text","text":"line2"}]}]}}"#;
        let sid = Uuid::new_v4();
        let state = fresh_state_with_binding(sid);
        let events = parse_line(line, &state, sid);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ClaudeCodeEvent::ToolResult { content, .. } => {
                assert_eq!(content, "line1\nline2");
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn parses_result_caches_claude_session_id_in_binding() {
        let line = r#"{"type":"result","subtype":"success","is_error":false,"session_id":"abc-123","stop_reason":"end_turn","usage":{"input_tokens":5,"cache_read_input_tokens":100,"output_tokens":7}}"#;
        let sid = Uuid::new_v4();
        let state = fresh_state_with_binding(sid);
        let events = parse_line(line, &state, sid);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ClaudeCodeEvent::Done { stop_reason, usage } => {
                assert!(matches!(stop_reason, StopReason::EndTurn));
                let u = usage.as_ref().unwrap();
                assert_eq!(u.prompt, 105);
                assert_eq!(u.prompt_cached, 100);
                assert_eq!(u.completion, 7);
            }
            other => panic!("expected Done, got {other:?}"),
        }
        let stored = state
            .lock()
            .unwrap()
            .bindings
            .get(&sid)
            .and_then(|b| b.claude_session_id.clone());
        assert_eq!(stored.as_deref(), Some("abc-123"));
    }

    #[test]
    fn parses_result_does_not_pollute_other_session_bindings() {
        let line = r#"{"type":"result","subtype":"success","is_error":false,"session_id":"abc-123","stop_reason":"end_turn"}"#;
        let sid_a = Uuid::new_v4();
        let sid_b = Uuid::new_v4();
        let state = fresh_state_with_binding(sid_a);
        state.lock().unwrap().bindings.insert(
            sid_b,
            SessionBinding {
                cwd: PathBuf::from("/tmp/other"),
                claude_session_id: Some("preexisting-B".into()),
                permission_mode: None,
                bridge: None,
                bash_via_operon: false,
                extra_dirs: Vec::new(),
            },
        );
        let _ = parse_line(line, &state, sid_a);
        let st = state.lock().unwrap();
        assert_eq!(
            st.bindings.get(&sid_a).and_then(|b| b.claude_session_id.as_deref()),
            Some("abc-123")
        );
        assert_eq!(
            st.bindings.get(&sid_b).and_then(|b| b.claude_session_id.as_deref()),
            Some("preexisting-B")
        );
    }

    #[test]
    fn parses_result_with_is_error_returns_error_event() {
        let line = r#"{"type":"result","is_error":true,"result":"boom","session_id":"x","stop_reason":"end_turn"}"#;
        let sid = Uuid::new_v4();
        let state = fresh_state_with_binding(sid);
        let events = parse_line(line, &state, sid);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], ClaudeCodeEvent::Error(ref m) if m == "boom"));
    }

    #[test]
    fn drops_rate_limit_events_and_unknown_system_subtypes() {
        let sid = Uuid::new_v4();
        let state = fresh_state_with_binding(sid);
        // Non-init system frames stay dropped — the chat surface only
        // consumes the init payload.
        assert!(parse_line(r#"{"type":"system","subtype":"compact"}"#, &state, sid).is_empty());
        assert!(parse_line(r#"{"type":"rate_limit_event"}"#, &state, sid).is_empty());
    }

    #[test]
    fn parses_system_init_into_session_init_event() {
        let sid = Uuid::new_v4();
        let state = fresh_state_with_binding(sid);
        let line = r#"{"type":"system","subtype":"init","mcp_servers":[{"name":"fs","status":"connected"},{"name":"http","status":"failed"}],"tools":["Bash","mcp__fs__read","mcp__fs__write","mcp__http__fetch"]}"#;
        let events = parse_line(line, &state, sid);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ClaudeCodeEvent::SessionInit { mcp_servers, tools } => {
                assert_eq!(mcp_servers.len(), 2);
                assert_eq!(mcp_servers[0].name, "fs");
                assert_eq!(mcp_servers[0].status, "connected");
                assert_eq!(mcp_servers[1].name, "http");
                assert_eq!(mcp_servers[1].status, "failed");
                assert_eq!(tools.len(), 4);
                assert!(tools.iter().any(|t| t == "mcp__fs__read"));
            }
            other => panic!("expected SessionInit, got {other:?}"),
        }
    }

    #[test]
    fn system_init_with_no_mcp_servers_is_still_emitted() {
        let sid = Uuid::new_v4();
        let state = fresh_state_with_binding(sid);
        let events = parse_line(
            r#"{"type":"system","subtype":"init","tools":["Bash"]}"#,
            &state,
            sid,
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            ClaudeCodeEvent::SessionInit { mcp_servers, tools } => {
                assert!(mcp_servers.is_empty());
                assert_eq!(tools, &vec!["Bash".to_string()]);
            }
            other => panic!("expected SessionInit, got {other:?}"),
        }
    }

    #[test]
    fn drops_invalid_json() {
        let sid = Uuid::new_v4();
        let state = fresh_state_with_binding(sid);
        assert!(parse_line("not json", &state, sid).is_empty());
    }
}
