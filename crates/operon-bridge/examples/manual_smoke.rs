//! Manual smoke test — boot the bridge with one demo tool, print
//! everything needed to talk to it from another terminal.
//!
//! Usage:
//! ```sh
//! cargo run -p operon-bridge --example manual_smoke
//! # in another terminal:
//! OPERON_BRIDGE_SOCK=<sock> OPERON_BRIDGE_TOKEN=<token> \
//!     cargo run -p operon-bridge --bin operon-mcp
//! # then paste these MCP frames into the stub's stdin (one line each):
//! {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}
//! {"jsonrpc":"2.0","id":2,"method":"tools/list"}
//! {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"operon_echo","arguments":{"hello":"world"}}}
//! ```
//!
//! Or skip the stub entirely and talk to the socket directly with
//! socat (one frame per line; the first frame MUST be operon/hello):
//! ```sh
//! socat - UNIX-CONNECT:<sock>
//! {"jsonrpc":"2.0","id":0,"method":"operon/hello","params":{"token":"<TOKEN>","client":"socat"}}
//! ```

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use operon_bridge::{Server, ToolHandler, ToolHandlerError};
use serde_json::{json, Value};

struct EchoTool;

#[async_trait]
impl ToolHandler for EchoTool {
    fn name(&self) -> &str {
        "operon_echo"
    }
    fn description(&self) -> &str {
        "Echo arguments back. Smoke-test only."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "additionalProperties": true,
            "description": "Anything; gets stringified into the response."
        })
    }
    async fn call(&self, args: Value) -> Result<Value, ToolHandlerError> {
        Ok(json!([{ "type": "text", "text": format!("echo: {args}") }]))
    }
}

struct PingTool;

#[async_trait]
impl ToolHandler for PingTool {
    fn name(&self) -> &str {
        "operon_ping"
    }
    fn description(&self) -> &str {
        "Return 'pong'. Smoke-test only."
    }
    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {}, "additionalProperties": false })
    }
    async fn call(&self, _args: Value) -> Result<Value, ToolHandlerError> {
        Ok(json!([{ "type": "text", "text": "pong" }]))
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Put the socket somewhere the user can see + delete easily.
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let sock = runtime_dir.join(format!("operon-bridge-smoke-{}.sock", std::process::id()));
    let token = uuid::Uuid::new_v4().to_string();

    let server = Server::new(sock.clone(), token.clone())
        .register_tool(Arc::new(EchoTool))
        .register_tool(Arc::new(PingTool));

    let handle = server.serve().await?;

    println!("operon-bridge smoke server up");
    println!("  socket : {}", sock.display());
    println!("  token  : {token}");
    println!();
    println!("Talk to it via the stub:");
    println!(
        "  OPERON_BRIDGE_SOCK={} OPERON_BRIDGE_TOKEN={} \\\n    cargo run -p operon-bridge --bin operon-mcp",
        sock.display(),
        token
    );
    println!();
    println!("Or talk to the socket directly with socat:");
    println!("  socat - UNIX-CONNECT:{}", sock.display());
    println!(
        "  (then paste) {{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"operon/hello\",\"params\":{{\"token\":\"{token}\",\"client\":\"socat\"}}}}"
    );
    println!();
    println!("Ctrl+C to stop.");

    // Wait for Ctrl+C, then unwind cleanly.
    let _ = tokio::signal::ctrl_c().await;
    handle.shutdown().await;
    Ok(())
}
