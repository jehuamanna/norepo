//! PostToolUse reload hook.
//!
//! Spawned by Claude Code after every Write/Edit/MultiEdit/NotebookEdit
//! tool call (configured via `.claude/settings.local.json` →
//! `hooks.PostToolUse`). Reads Claude's hook payload from stdin, pulls
//! the touched file path out of `tool_input`, walks the transcript
//! file to extract Claude's preceding text explanation, and sends one
//! NDJSON line over a Unix socket to the running Operon process so the
//! matching open tab reloads from disk AND the artifact revision row
//! carries a meaningful summary (instead of `Edited body (N lines)`).
//!
//! Failure paths are silent: a missing socket, a malformed payload,
//! an unreachable transcript, or a tool we don't care about all exit 0
//! so Claude never sees the hook as a blocker. The reload + summary
//! are UX niceties, not correctness gates.
//!
//! Usage: `operon-posttool-hook --socket <path>`

#![cfg(not(target_arch = "wasm32"))]

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

/// Hard cap for the assistant-text summary baked into a revision row.
/// Long Claude explanations would otherwise blow up the table cell —
/// 240 chars is enough for a sentence-or-two summary while keeping
/// the rendered table readable.
const SUMMARY_MAX_CHARS: usize = 240;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let socket = match parse_socket_arg() {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("operon-posttool-hook: {msg}");
            return ExitCode::SUCCESS;
        }
    };

    let mut stdin_buf = String::new();
    if std::io::stdin().read_to_string(&mut stdin_buf).is_err() {
        return ExitCode::SUCCESS;
    }
    let payload: Value = match serde_json::from_str(&stdin_buf) {
        Ok(v) => v,
        Err(_) => return ExitCode::SUCCESS,
    };

    let tool = payload
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let path = match tool {
        "Write" | "Edit" | "MultiEdit" => payload
            .get("tool_input")
            .and_then(|i| i.get("file_path"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        "NotebookEdit" => payload
            .get("tool_input")
            .and_then(|i| i.get("notebook_path"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        _ => return ExitCode::SUCCESS,
    };
    let Some(path) = path else {
        return ExitCode::SUCCESS;
    };

    // Best-effort: walk the transcript file Claude writes for this
    // session and extract Claude's preceding text block for the
    // matching tool_use. The MCP hook payload reliably includes
    // `transcript_path` and (sometimes) `tool_use_id`; we use both
    // when present and degrade gracefully when not.
    let transcript_path = payload
        .get("transcript_path")
        .and_then(|v| v.as_str())
        .map(PathBuf::from);
    let tool_use_id = payload
        .get("tool_use_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let summary = transcript_path
        .as_deref()
        .and_then(|p| extract_assistant_text(p, tool, tool_use_id.as_deref()))
        .map(truncate_summary)
        .filter(|s| !s.is_empty());

    let mut frame_obj = serde_json::Map::new();
    frame_obj.insert("tool".to_string(), Value::String(tool.to_string()));
    frame_obj.insert("path".to_string(), Value::String(path));
    if let Some(s) = summary {
        frame_obj.insert("summary".to_string(), Value::String(s));
    }
    let frame = Value::Object(frame_obj).to_string() + "\n";

    // Best-effort send with a short timeout. If Operon isn't running
    // or the socket is stale, drop the event quietly.
    let send_fut = async move {
        let mut stream = UnixStream::connect(&socket).await.ok()?;
        stream.write_all(frame.as_bytes()).await.ok()?;
        stream.flush().await.ok()?;
        let _ = stream.shutdown().await;
        Some(())
    };
    let _ = tokio::time::timeout(Duration::from_millis(500), send_fut).await;
    ExitCode::SUCCESS
}

fn parse_socket_arg() -> Result<PathBuf, String> {
    let mut args = std::env::args().skip(1);
    let mut socket: Option<PathBuf> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--socket" => {
                socket = args.next().map(PathBuf::from);
            }
            other if other.starts_with("--socket=") => {
                socket = Some(PathBuf::from(&other["--socket=".len()..]));
            }
            "-h" | "--help" => {
                println!("usage: operon-posttool-hook --socket <path>");
                std::process::exit(0);
            }
            _ => return Err(format!("unrecognised arg: {a}")),
        }
    }
    socket.ok_or_else(|| "missing --socket <path>".into())
}

/// Walk the transcript JSONL and find the most-recent assistant
/// `text` block that sits in the same message as a `tool_use` whose
/// `name` matches `tool_name` (and `id` matches `target_id` when
/// supplied). Returns `None` when the transcript can't be read or no
/// matching message is found. The transcript is small — at most a few
/// hundred lines per session — so a full read+parse is fine.
fn extract_assistant_text(
    transcript_path: &Path,
    tool_name: &str,
    target_id: Option<&str>,
) -> Option<String> {
    let raw = std::fs::read_to_string(transcript_path).ok()?;
    let mut last_match: Option<String> = None;
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        // The transcript writer in claude-code v1.x emits envelopes
        // like `{"type":"assistant","message":{"role":"assistant","content":[...]}}`
        // but older betas used `{"role":"assistant","content":[...]}` directly.
        // Both shapes are handled.
        let entry_type = v
            .get("type")
            .and_then(|x| x.as_str())
            .or_else(|| v.get("role").and_then(|x| x.as_str()))
            .unwrap_or("");
        if entry_type != "assistant" {
            continue;
        }
        let content = v
            .get("message")
            .and_then(|m| m.get("content"))
            .or_else(|| v.get("content"))
            .and_then(|c| c.as_array());
        let Some(blocks) = content else {
            continue;
        };
        let mut text_for_this_msg: Option<String> = None;
        let mut matched_this_msg = false;
        for b in blocks {
            let btype = b.get("type").and_then(|x| x.as_str()).unwrap_or("");
            match btype {
                "text" => {
                    if let Some(t) = b.get("text").and_then(|x| x.as_str()) {
                        let clean = t.trim();
                        if !clean.is_empty() {
                            text_for_this_msg = Some(clean.to_string());
                        }
                    }
                }
                "tool_use" => {
                    let name = b
                        .get("name")
                        .and_then(|x| x.as_str())
                        .unwrap_or("");
                    if name != tool_name {
                        continue;
                    }
                    let id_ok = match target_id {
                        Some(t) => b
                            .get("id")
                            .and_then(|x| x.as_str())
                            .is_some_and(|id| id == t),
                        None => true,
                    };
                    if id_ok {
                        matched_this_msg = true;
                    }
                }
                _ => {}
            }
        }
        if matched_this_msg {
            if let Some(t) = text_for_this_msg {
                last_match = Some(t);
            }
        }
    }
    last_match
}

/// Truncate `s` to [`SUMMARY_MAX_CHARS`] characters (not bytes — we
/// preserve a valid UTF-8 boundary). Collapses any embedded newlines
/// to spaces because the revision-row cell is rendered single-line.
fn truncate_summary(s: String) -> String {
    let normalized: String = s
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    let collapsed = normalized
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if collapsed.chars().count() <= SUMMARY_MAX_CHARS {
        return collapsed;
    }
    let truncated: String = collapsed.chars().take(SUMMARY_MAX_CHARS - 1).collect();
    format!("{truncated}\u{2026}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_transcript(lines: &[&str]) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        f.flush().unwrap();
        f
    }

    #[test]
    fn extracts_text_from_envelope_form() {
        let f = write_transcript(&[
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hi"}]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Updated the acceptance criterion to reference an 8x8 round."},{"type":"tool_use","id":"toolu_01","name":"Edit","input":{"file_path":"/x.md"}}]}}"#,
        ]);
        let got = extract_assistant_text(f.path(), "Edit", Some("toolu_01")).unwrap();
        assert!(got.contains("acceptance criterion"));
    }

    #[test]
    fn extracts_text_from_flat_form() {
        let f = write_transcript(&[
            r#"{"role":"assistant","content":[{"type":"text","text":"Bumped to 8x8."},{"type":"tool_use","id":"x","name":"Edit","input":{}}]}"#,
        ]);
        let got = extract_assistant_text(f.path(), "Edit", None).unwrap();
        assert_eq!(got, "Bumped to 8x8.");
    }

    #[test]
    fn returns_last_match_when_multiple_edits_in_session() {
        let f = write_transcript(&[
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"First edit"},{"type":"tool_use","id":"a","name":"Edit","input":{}}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Second edit"},{"type":"tool_use","id":"b","name":"Edit","input":{}}]}}"#,
        ]);
        let got = extract_assistant_text(f.path(), "Edit", None).unwrap();
        assert_eq!(got, "Second edit");
    }

    #[test]
    fn matches_by_id_when_supplied() {
        let f = write_transcript(&[
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"First edit"},{"type":"tool_use","id":"a","name":"Edit","input":{}}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Second edit"},{"type":"tool_use","id":"b","name":"Edit","input":{}}]}}"#,
        ]);
        let got = extract_assistant_text(f.path(), "Edit", Some("a")).unwrap();
        assert_eq!(got, "First edit");
    }

    #[test]
    fn skips_non_matching_tools() {
        let f = write_transcript(&[
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Just reading"},{"type":"tool_use","id":"r","name":"Read","input":{}}]}}"#,
        ]);
        assert!(extract_assistant_text(f.path(), "Edit", None).is_none());
    }

    #[test]
    fn truncate_collapses_whitespace_and_caps_length() {
        let long = "a".repeat(SUMMARY_MAX_CHARS + 50);
        let out = truncate_summary(format!("hello\n\nworld   {long}"));
        assert!(out.starts_with("hello world"));
        assert!(out.chars().count() <= SUMMARY_MAX_CHARS);
        assert!(out.ends_with('\u{2026}'));
    }

    #[test]
    fn truncate_short_returns_input() {
        let out = truncate_summary("Updated criterion.".to_string());
        assert_eq!(out, "Updated criterion.");
    }
}
