//! Operon LSP client + `lsp` ToolPlugin.
//!
//! Drives language servers (rust-analyzer, pyright, typescript-language-server, …)
//! via stdio JSON-RPC. Auto-detects which server to spawn from project file globs.
//!
//! The `lsp` tool exposes: goto_definition, find_references, hover,
//! document_symbols, diagnostics. Each is one method call.
//!
//! Slice A0 scaffold: empty stubs. Slice A11 lands the protocol layer + tool.

#[cfg(not(target_arch = "wasm32"))]
pub mod client;
#[cfg(not(target_arch = "wasm32"))]
pub mod codec;
#[cfg(not(target_arch = "wasm32"))]
pub mod tool;

#[cfg(not(target_arch = "wasm32"))]
pub use client::{LspClient, LspServerConfig};
#[cfg(not(target_arch = "wasm32"))]
pub use tool::{LspTool, LspToolBuilder};
