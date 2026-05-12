//! Tiny stdio↔Unix-socket proxy spawned by `claude --mcp-config` so the
//! actual MCP permission server can live inside the operon process. The
//! shim does no protocol parsing — it just forwards newline-delimited
//! JSON frames in both directions until either side closes.
//!
//! Usage: `operon-mcp-permission --socket <path>`

#![cfg(not(target_arch = "wasm32"))]

use std::path::PathBuf;
use std::process::ExitCode;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let socket = match parse_socket_arg() {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("operon-mcp-permission: {msg}");
            return ExitCode::from(2);
        }
    };

    let stream = match UnixStream::connect(&socket).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "operon-mcp-permission: connect {}: {e}",
                socket.display()
            );
            return ExitCode::from(1);
        }
    };

    let (sock_read, mut sock_write) = stream.into_split();
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();

    // stdin → socket
    let stdin_to_sock = async move {
        let mut lines = BufReader::new(stdin).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            // Re-add the newline that `next_line` strips so the host
            // sees frames in the same NDJSON shape it expects.
            let frame = format!("{line}\n");
            if sock_write.write_all(frame.as_bytes()).await.is_err() {
                break;
            }
            if sock_write.flush().await.is_err() {
                break;
            }
        }
        // Close write half so the bridge sees EOF and tears down the
        // connection.
        let _ = sock_write.shutdown().await;
    };

    // socket → stdout
    let sock_to_stdout = async move {
        let mut lines = BufReader::new(sock_read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let frame = format!("{line}\n");
            if stdout.write_all(frame.as_bytes()).await.is_err() {
                break;
            }
            if stdout.flush().await.is_err() {
                break;
            }
        }
    };

    // Run both halves until either finishes (which means a side closed),
    // then exit cleanly.
    tokio::select! {
        _ = stdin_to_sock => {}
        _ = sock_to_stdout => {}
    }
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
                println!("usage: operon-mcp-permission --socket <path>");
                std::process::exit(0);
            }
            _ => return Err(format!("unrecognised arg: {a}")),
        }
    }
    socket.ok_or_else(|| "missing --socket <path>".into())
}
