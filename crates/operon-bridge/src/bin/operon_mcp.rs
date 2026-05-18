//! `operon-mcp` — standalone stdio MCP stub.
//!
//! Production builds embed the same stub into the main
//! `operon-dioxus` binary and dispatch via `--operon-mcp`, so this
//! bin is no longer shipped in release bundles. It stays in the
//! workspace for dev workflows that pin `OPERON_MCP_BIN` to a
//! separately built binary (e.g. to swap stubs without rebuilding
//! the GUI).
//!
//! All the wire logic lives in `operon_bridge::run_stub`; keep this
//! file a one-liner so the embedded dispatch and the standalone bin
//! can't drift.

#![cfg(unix)]

#[tokio::main(flavor = "current_thread")]
async fn main() {
    std::process::exit(operon_bridge::run_stub().await);
}
