//! Newline-delimited JSON-RPC 2.0 framing for the bridge socket.
//!
//! Same wire shape as `StdioMcpClient` (see
//! `crates/operon-plugins-mcp/src/stdio.rs`): one JSON object per
//! line, no length prefix. The stub blindly forwards every frame
//! between Claude's stdio and the bridge socket — neither side parses
//! the inner JSON, so the *only* thing this module owns is reading
//! and writing lines.
//!
//! Why not framed length-prefixed transport: existing Operon code
//! already speaks line-delimited JSON to MCP servers, and we get to
//! tail / debug the socket with `socat - UNIX-CONNECT:.../bridge.sock`
//! and friends without writing a custom parser.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// One JSON-RPC 2.0 frame as it appears on the wire. We don't insist
/// on a particular shape (request / response / notification) because
/// the stub copies frames between Claude and the bridge without ever
/// inspecting them. The bridge server *does* parse — see
/// [`server::dispatch`] — but it works from `serde_json::Value`
/// directly to keep schema evolution painless.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frame {
    pub raw: serde_json::Value,
}

impl Frame {
    pub fn new(raw: serde_json::Value) -> Self {
        Self { raw }
    }

    /// Convenience for the bridge handshake response. JSON-RPC 2.0
    /// requires `jsonrpc: "2.0"` plus `id` for results — leave `id`
    /// to the caller since it has to echo the request's id.
    pub fn result(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self::new(serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }))
    }

    pub fn error(id: serde_json::Value, code: i32, message: impl Into<String>) -> Self {
        Self::new(serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message.into() },
        }))
    }
}

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("frame too large ({0} bytes) — refusing to allocate")]
    TooLarge(usize),

    #[error("invalid utf-8 in frame")]
    Utf8,

    #[error("invalid json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("eof")]
    Eof,

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Hard cap on a single frame. MCP `tools/list` responses can be
/// large (tool schemas), but a megabyte is plenty and protects
/// against a runaway peer.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

#[cfg(unix)]
mod io {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    /// Read one newline-terminated JSON frame off `r`. Returns `Eof`
    /// once the peer half-closes — callers treat that as a normal
    /// shutdown.
    pub async fn read_frame<R>(r: &mut BufReader<R>) -> Result<Frame, FrameError>
    where
        R: tokio::io::AsyncRead + Unpin,
    {
        let mut buf = String::new();
        // BufReader::read_line includes the trailing `\n`. Empty
        // string means EOF.
        let n = r.read_line(&mut buf).await?;
        if n == 0 {
            return Err(FrameError::Eof);
        }
        if buf.len() > MAX_FRAME_BYTES {
            return Err(FrameError::TooLarge(buf.len()));
        }
        // Trim trailing `\n` / `\r\n` so serde_json doesn't choke on
        // whitespace at the tail of the object.
        let trimmed = buf.trim_end_matches(['\n', '\r']);
        let value: serde_json::Value = serde_json::from_str(trimmed)?;
        Ok(Frame::new(value))
    }

    /// Write `frame` as one JSON object + `\n`. Caller must flush
    /// when they're done with a batch.
    pub async fn write_frame<W>(w: &mut W, frame: &Frame) -> Result<(), FrameError>
    where
        W: tokio::io::AsyncWrite + Unpin,
    {
        let bytes = serde_json::to_vec(&frame.raw)?;
        if bytes.len() + 1 > MAX_FRAME_BYTES {
            return Err(FrameError::TooLarge(bytes.len() + 1));
        }
        w.write_all(&bytes).await?;
        w.write_all(b"\n").await?;
        Ok(())
    }
}

#[cfg(unix)]
pub use io::{read_frame, write_frame};

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use tokio::io::BufReader;

    #[tokio::test]
    async fn round_trips_one_frame() {
        let (a, b) = tokio::io::duplex(4096);
        let (read_half, _write_half) = tokio::io::split(a);
        let mut reader = BufReader::new(read_half);

        let sent = Frame::new(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "ping",
        }));
        // Run the writer on the other half so the round-trip exercises
        // both sides of the duplex. `WriteHalf` doesn't expose
        // `shutdown` directly; dropping it is enough for read_frame to
        // see EOF if we ever needed that signal here.
        let (_read_other, mut write_other) = tokio::io::split(b);
        let writer = tokio::spawn({
            let sent = sent.clone();
            async move {
                write_frame(&mut write_other, &sent).await.unwrap();
            }
        });
        let got = read_frame(&mut reader).await.unwrap();
        writer.await.unwrap();

        assert_eq!(got.raw, sent.raw);
    }

    #[tokio::test]
    async fn eof_after_clean_close() {
        let (a, b) = tokio::io::duplex(64);
        let (read_half, _w) = tokio::io::split(a);
        let mut reader = BufReader::new(read_half);
        drop(b); // peer hangs up
        match read_frame(&mut reader).await {
            Err(FrameError::Eof) => {}
            other => panic!("expected Eof, got {other:?}"),
        }
    }

    #[test]
    fn frame_result_helper_carries_id() {
        let f = Frame::result(serde_json::json!(42), serde_json::json!({"ok": true}));
        assert_eq!(f.raw["jsonrpc"], "2.0");
        assert_eq!(f.raw["id"], 42);
        assert_eq!(f.raw["result"]["ok"], true);
    }

    #[test]
    fn frame_error_helper_shapes_match_spec() {
        let f = Frame::error(serde_json::json!(1), -32600, "Invalid Request");
        assert_eq!(f.raw["error"]["code"], -32600);
        assert_eq!(f.raw["error"]["message"], "Invalid Request");
    }
}

#[cfg(test)]
mod tests_no_io {
    use super::*;

    #[test]
    fn frames_are_clone_safe() {
        let f = Frame::new(serde_json::json!({"k": "v"}));
        let g = f.clone();
        assert_eq!(f.raw, g.raw);
    }
}
