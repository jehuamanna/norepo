//! `shell` tool — execute a bash command with timeout, cwd binding, captured stdout/stderr.

use operon_core::error::{OperonError, OperonResult};
use operon_core::traits::{
    CancellationToken, Capabilities, Plugin, ToolChunk, ToolDef, ToolPlugin,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::timeout;

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_OUTPUT_BYTES: usize = 256 * 1024;

#[derive(Deserialize)]
struct ShellInput {
    command: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

pub struct ShellTool;

#[async_trait]
impl Plugin for ShellTool {
    fn name(&self) -> &str { "shell" }
    fn version(&self) -> &str { env!("CARGO_PKG_VERSION") }
    fn capabilities(&self) -> Capabilities { Capabilities::empty() }
}

#[async_trait]
impl ToolPlugin for ShellTool {
    fn schema(&self) -> ToolDef {
        ToolDef {
            name: "shell".into(),
            description: "Execute a bash command. Captures stdout, stderr, exit code. \
                          Hard-timeouts at the configured limit. Output is capped at 256 KiB."
                          .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command":     { "type": "string", "description": "Bash command to execute." },
                    "cwd":         { "type": "string", "description": "Absolute working directory." },
                    "timeout_ms":  { "type": "integer", "minimum": 1, "default": 120000 }
                },
                "required": ["command"]
            }),
        }
    }

    async fn invoke(
        &self,
        args: serde_json::Value,
        ct: CancellationToken,
    ) -> OperonResult<serde_json::Value> {
        // Delegate to the streaming path with a discarded chunk
        // channel — chunks are emitted into the receiver, which is
        // dropped immediately after this call returns. This keeps a
        // single implementation of the bash spawn/wait/cancel logic
        // (the runtime calls `invoke_streaming` directly; the
        // non-streaming path is just for parity with the trait).
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<ToolChunk>();
        self.invoke_streaming(args, ct, tx).await
    }

    async fn invoke_streaming(
        &self,
        args: serde_json::Value,
        ct: CancellationToken,
        chunk_tx: UnboundedSender<ToolChunk>,
    ) -> OperonResult<serde_json::Value> {
        let input: ShellInput = serde_json::from_value(args).map_err(|e| OperonError::Plugin {
            plugin: "shell".into(),
            source: Box::new(e),
        })?;

        let timeout_ms = input.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
        let mut cmd = Command::new("bash");
        cmd.arg("-c").arg(&input.command);
        if let Some(cwd) = &input.cwd {
            let p = PathBuf::from(cwd);
            if !p.is_absolute() {
                return Ok(json!({ "error": "cwd must be absolute", "cwd": cwd }));
            }
            cmd.current_dir(p);
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return Ok(json!({ "error": format!("spawn failed: {e}") }));
            }
        };
        let stdout_pipe = child.stdout.take().expect("stdout piped");
        let stderr_pipe = child.stderr.take().expect("stderr piped");

        let read_stdout = drain_stream(stdout_pipe, "stdout", chunk_tx.clone());
        let read_stderr = drain_stream(stderr_pipe, "stderr", chunk_tx.clone());
        // Drop our chunk_tx clone so the channel closes once both
        // drain tasks have finished — the runtime's forwarder uses
        // that as the signal to exit.
        drop(chunk_tx);

        let wait = async {
            let (so, se, status) = tokio::join!(read_stdout, read_stderr, child.wait());
            (so, se, status)
        };

        let result = tokio::select! {
            r = timeout(Duration::from_millis(timeout_ms), wait) => r,
            _ = ct.cancelled() => {
                // `kill_on_drop(true)` (line 81) means the child dies
                // when `child` drops at the end of this scope; we
                // return immediately to surface the cancel without
                // waiting for the natural exit.
                return Ok(json!({ "error": "cancelled", "stdout": "", "stderr": "" }));
            }
        };

        match result {
            Err(_) => Ok(json!({
                "error": "timeout",
                "timeout_ms": timeout_ms,
                "stdout": "",
                "stderr": "",
            })),
            Ok((stdout, stderr, status)) => {
                let exit_code = status.ok().and_then(|s| s.code()).unwrap_or(-1);
                Ok(json!({
                    "exit_code": exit_code,
                    "stdout": stdout,
                    "stderr": stderr,
                    "stdout_truncated": stdout.len() >= MAX_OUTPUT_BYTES,
                    "stderr_truncated": stderr.len() >= MAX_OUTPUT_BYTES,
                }))
            }
        }
    }
}

/// Drain one of the child's pipes: forward every read as a `ToolChunk`
/// AND accumulate the bytes into a capped buffer for the final
/// `ToolResult`. Returns the (lossy-decoded) full text once EOF is
/// hit. Stops appending to the buffer at [`MAX_OUTPUT_BYTES`] but
/// keeps streaming chunks until EOF so the UI sees the complete
/// run.
async fn drain_stream<R>(
    mut pipe: R,
    kind: &'static str,
    chunk_tx: UnboundedSender<ToolChunk>,
) -> String
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = match pipe.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        let slice = &chunk[..n];
        // Best-effort send. A closed receiver (runtime task gone)
        // just means streaming is no longer being consumed — keep
        // draining so the final result is still complete.
        let _ = chunk_tx.send(ToolChunk {
            kind: kind.to_string(),
            bytes: slice.to_vec(),
        });
        if buf.len() < MAX_OUTPUT_BYTES {
            let take = MAX_OUTPUT_BYTES.saturating_sub(buf.len()).min(n);
            buf.extend_from_slice(&slice[..take]);
        }
    }
    String::from_utf8_lossy(&buf).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn echoes_stdout() {
        let r = ShellTool
            .invoke(json!({ "command": "echo hello" }), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(r.get("exit_code").and_then(|v| v.as_i64()), Some(0));
        assert!(r.get("stdout").and_then(|v| v.as_str()).unwrap().contains("hello"));
    }

    #[tokio::test]
    async fn captures_nonzero_exit() {
        let r = ShellTool
            .invoke(json!({ "command": "exit 7" }), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(r.get("exit_code").and_then(|v| v.as_i64()), Some(7));
    }

    #[tokio::test]
    async fn cwd_binds_command() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("marker.txt"), b"").unwrap();
        let r = ShellTool
            .invoke(
                json!({ "command": "ls marker.txt", "cwd": tmp.path().to_str().unwrap() }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(r.get("exit_code").and_then(|v| v.as_i64()), Some(0));
    }

    #[tokio::test]
    async fn timeout_kills_long_command() {
        let r = ShellTool
            .invoke(
                json!({ "command": "sleep 10", "timeout_ms": 200 }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(r.get("error").and_then(|v| v.as_str()), Some("timeout"));
    }

    #[tokio::test]
    async fn rejects_relative_cwd() {
        let r = ShellTool
            .invoke(
                json!({ "command": "true", "cwd": "relative" }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(r.get("error").and_then(|v| v.as_str()), Some("cwd must be absolute"));
    }
}
