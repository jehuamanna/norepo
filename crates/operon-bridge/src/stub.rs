//! `operon-mcp` stdio stub — the implementation Claude spawns to
//! reach the in-process bridge server.
//!
//! Historically this lived in `src/bin/operon_mcp.rs` as a standalone
//! binary that shipped alongside the GUI. To collapse the deploy down
//! to a single executable, the body moved here as a library function
//! ([`run_stub`]) so the main `operon-dioxus` binary can dispatch to
//! it from `main()` when invoked with `--operon-mcp`. The standalone
//! `operon-mcp` bin is preserved as a thin wrapper for dev workflows
//! that pin `OPERON_MCP_BIN` to a separately built binary.
//!
//! Wire model
//! ----------
//! 1. Read env: `OPERON_BRIDGE_SOCK` (path) + `OPERON_BRIDGE_TOKEN`
//!    (uuid). Both required — exit non-zero if missing so Claude
//!    surfaces a clear MCP-init failure instead of hanging.
//! 2. Connect to the socket. Send an `operon/hello` frame with the
//!    token. If the bridge rejects, exit non-zero.
//! 3. Splice the streams: every line on stdin → socket, every line
//!    from the socket → stdout. Each direction runs as its own task
//!    so neither side starves the other.
//! 4. On EOF in either direction, close cleanly.
//!
//! The stub never parses MCP frames — schema evolution lives in the
//! bridge server, not here.

use std::env;

use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

const ENV_SOCK: &str = "OPERON_BRIDGE_SOCK";
const ENV_TOKEN: &str = "OPERON_BRIDGE_TOKEN";
const ENV_SESSION: &str = "OPERON_SESSION_ID";

/// Run the operon-mcp stub. Returns the exit code to pass to
/// `process::exit`. The caller owns the tokio runtime (use
/// `tokio::runtime::Builder::new_current_thread().enable_all()` —
/// see the standalone bin or the dispatch in `operon-dioxus`'s
/// `main.rs`).
///
/// Anything printed to stdout would corrupt Claude's MCP framing, so
/// all diagnostics go to stderr via `eprintln!`.
pub async fn run_stub() -> i32 {
    let sock_path = match env::var(ENV_SOCK) {
        Ok(v) if !v.is_empty() => v,
        _ => {
            eprintln!("operon-mcp: {ENV_SOCK} not set; refusing to start");
            return 2;
        }
    };
    let token = match env::var(ENV_TOKEN) {
        Ok(v) if !v.is_empty() => v,
        _ => {
            eprintln!("operon-mcp: {ENV_TOKEN} not set; refusing to start");
            return 2;
        }
    };
    let session = env::var(ENV_SESSION).ok().unwrap_or_default();

    let stream = match UnixStream::connect(&sock_path).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("operon-mcp: connect {sock_path}: {e}");
            return 3;
        }
    };

    let (sock_read, mut sock_write) = stream.into_split();
    let mut sock_buf = BufReader::new(sock_read);

    let hello = json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "operon/hello",
        "params": {
            "token": token,
            "client": format!("operon-mcp/{}", env!("CARGO_PKG_VERSION")),
            "session": session,
        },
    });
    let mut hello_line = match serde_json::to_vec(&hello) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("operon-mcp: serialise hello: {e}");
            return 4;
        }
    };
    hello_line.push(b'\n');
    if let Err(e) = sock_write.write_all(&hello_line).await {
        eprintln!("operon-mcp: write hello: {e}");
        return 5;
    }
    if let Err(e) = sock_write.flush().await {
        eprintln!("operon-mcp: flush hello: {e}");
        return 5;
    }

    let mut hello_reply = String::new();
    match sock_buf.read_line(&mut hello_reply).await {
        Ok(0) => {
            eprintln!("operon-mcp: bridge closed before hello reply");
            return 6;
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!("operon-mcp: read hello reply: {e}");
            return 6;
        }
    }
    // The bridge replies `{ "result": { "ok": true, ... } }` on
    // success and `{ "error": { ... } }` on failure; the line is
    // short either way so a substring check is enough.
    if !hello_reply.contains("\"ok\":true") {
        eprintln!("operon-mcp: hello rejected: {}", hello_reply.trim());
        return 7;
    }

    let stdin_to_sock = tokio::spawn(async move {
        let stdin = tokio::io::stdin();
        let mut br = BufReader::new(stdin);
        let mut buf = String::new();
        loop {
            buf.clear();
            match br.read_line(&mut buf).await {
                Ok(0) => break,
                Ok(_) => {
                    if let Err(e) = sock_write.write_all(buf.as_bytes()).await {
                        eprintln!("operon-mcp: write to socket: {e}");
                        break;
                    }
                    if !buf.ends_with('\n') {
                        let _ = sock_write.write_all(b"\n").await;
                    }
                    if let Err(e) = sock_write.flush().await {
                        eprintln!("operon-mcp: flush socket: {e}");
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("operon-mcp: read from stdin: {e}");
                    break;
                }
            }
        }
        let _ = sock_write.shutdown().await;
    });

    let sock_to_stdout = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        let mut buf = String::new();
        loop {
            buf.clear();
            match sock_buf.read_line(&mut buf).await {
                Ok(0) => break,
                Ok(_) => {
                    if let Err(e) = stdout.write_all(buf.as_bytes()).await {
                        eprintln!("operon-mcp: write to stdout: {e}");
                        break;
                    }
                    if !buf.ends_with('\n') {
                        let _ = stdout.write_all(b"\n").await;
                    }
                    if let Err(e) = stdout.flush().await {
                        eprintln!("operon-mcp: flush stdout: {e}");
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("operon-mcp: read from socket: {e}");
                    break;
                }
            }
        }
    });

    // Wait for either direction to finish — if one task panics or
    // errors we still drain the other so the last frame in flight
    // isn't lost.
    let _ = tokio::join!(stdin_to_sock, sock_to_stdout);
    0
}
