//! `operon-mcp` — stdio MCP stub that Claude spawns.
//!
//! Claude only knows how to talk to MCP servers over stdio. We need
//! to reach the in-process server running inside the Operon GUI,
//! which lives on a unix-domain socket. This binary bridges the two.
//!
//! Wire model
//! ----------
//! 1. Read env: `OPERON_BRIDGE_SOCK` (path) + `OPERON_BRIDGE_TOKEN`
//!    (uuid). Both are required — exit non-zero if either is missing
//!    so Claude shows a clear MCP-init failure rather than hanging.
//! 2. Connect to the socket. Send an `operon/hello` frame with the
//!    token. If the bridge rejects, exit non-zero.
//! 3. After hello, splice the two streams: every line on stdin is
//!    forwarded to the socket, every line from the socket is written
//!    to stdout. Each direction runs as its own task so neither side
//!    can starve the other.
//! 4. On EOF in either direction, close cleanly.
//!
//! Intentionally minimal — the stub never parses MCP frames. Schema
//! evolution lives in the bridge server, not here.

#![cfg(unix)]

use std::env;
use std::process::ExitCode;

use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

const ENV_SOCK: &str = "OPERON_BRIDGE_SOCK";
const ENV_TOKEN: &str = "OPERON_BRIDGE_TOKEN";
const ENV_SESSION: &str = "OPERON_SESSION_ID";

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    // The stub deliberately stays log-light: anything we print on
    // stdout would corrupt Claude's MCP framing. Diagnostics go to
    // stderr via eprintln! only.
    let sock_path = match env::var(ENV_SOCK) {
        Ok(v) if !v.is_empty() => v,
        _ => {
            eprintln!("operon-mcp: {ENV_SOCK} not set; refusing to start");
            return ExitCode::from(2);
        }
    };
    let token = match env::var(ENV_TOKEN) {
        Ok(v) if !v.is_empty() => v,
        _ => {
            eprintln!("operon-mcp: {ENV_TOKEN} not set; refusing to start");
            return ExitCode::from(2);
        }
    };
    let session = env::var(ENV_SESSION).ok().unwrap_or_default();

    let stream = match UnixStream::connect(&sock_path).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("operon-mcp: connect {sock_path}: {e}");
            return ExitCode::from(3);
        }
    };

    let (sock_read, mut sock_write) = stream.into_split();
    let mut sock_buf = BufReader::new(sock_read);

    // ---- Hello / auth ----
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
            return ExitCode::from(4);
        }
    };
    hello_line.push(b'\n');
    if let Err(e) = sock_write.write_all(&hello_line).await {
        eprintln!("operon-mcp: write hello: {e}");
        return ExitCode::from(5);
    }
    if let Err(e) = sock_write.flush().await {
        eprintln!("operon-mcp: flush hello: {e}");
        return ExitCode::from(5);
    }

    let mut hello_reply = String::new();
    match sock_buf.read_line(&mut hello_reply).await {
        Ok(0) => {
            eprintln!("operon-mcp: bridge closed before hello reply");
            return ExitCode::from(6);
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!("operon-mcp: read hello reply: {e}");
            return ExitCode::from(6);
        }
    }
    // Quick sanity check on the reply. We don't fully validate — the
    // bridge replies with `{ "result": { "ok": true, ... } }` on
    // success and `{ "error": { ... } }` on failure. Either way the
    // line is short.
    if !hello_reply.contains("\"ok\":true") {
        eprintln!("operon-mcp: hello rejected: {}", hello_reply.trim());
        return ExitCode::from(7);
    }

    // ---- Bidirectional pipe ----
    // stdin → socket
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

    // socket → stdout
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

    // Wait for either direction to finish. The far side (Claude or
    // the GUI) hanging up should cause us to exit cleanly; if one
    // task panics or errors, we still want to drain the other so the
    // last frame in flight isn't lost.
    let _ = tokio::join!(stdin_to_sock, sock_to_stdout);
    ExitCode::SUCCESS
}
