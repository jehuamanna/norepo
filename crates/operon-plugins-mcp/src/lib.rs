//! Operon MCP (Model Context Protocol) client plugins.
//!
//! `StdioMcpClient` speaks newline-delimited JSON-RPC over a subprocess's
//! stdin/stdout. `McpToolProxy` adapts MCP-served tools as Operon `ToolPlugin`
//! instances, gated by `GrantHandler` capability checks.

pub mod grant;
pub mod proxy;
#[cfg(not(target_arch = "wasm32"))]
pub mod stdio;

pub use grant::{
    AutoApproveGrantHandler, DenyAllGrantHandler, GrantHandler, SecretStoreGrantHandler,
};
pub use proxy::McpToolProxy;
#[cfg(not(target_arch = "wasm32"))]
pub use stdio::{StdioMcpClient, StdioMcpServerConfig};
