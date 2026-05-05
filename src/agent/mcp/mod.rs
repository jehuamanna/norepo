//! MCP (Model Context Protocol) client integration.
//!
//! `StdioMcpClient` spawns a subprocess and speaks newline-delimited JSON-RPC.
//! Discovered tools are exposed as `ToolPlugin` instances via `McpToolProxy`,
//! gated by capability grants (`GrantHandler`).

#[cfg(not(target_arch = "wasm32"))]
pub mod stdio;
pub mod proxy;
pub mod grant;

#[cfg(not(target_arch = "wasm32"))]
pub use stdio::StdioMcpClient;
pub use proxy::McpToolProxy;
pub use grant::{AutoApproveGrantHandler, DenyAllGrantHandler, GrantHandler, SecretStoreGrantHandler};
