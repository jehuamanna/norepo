//! MCP settings panel — manages `claude mcp` server entries from inside
//! the Operon companion chat UI.
//!
//! Layout:
//! - `service.rs` — wraps the `claude mcp add/list/get/remove` CLI calls.
//! - `panel.rs`   — the modal frame + list/empty-state shell.
//! - `server_card.rs` — per-server row (active dot, transport, tools,
//!                      details, remove).
//! - `add_form.rs`    — the inline "add server" form.
//!
//! Mounted from `companion_chat.rs` via a button in the chat header. The
//! panel reads `MCP_LIVE_STATUS` (set by `apply_event` when claude emits
//! `system/init`) to drive live "is this server up + which tools are
//! exposed" indicators.

#![cfg(not(target_arch = "wasm32"))]

pub mod add_form;
pub mod panel;
pub mod server_card;
pub mod service;

pub use panel::{McpPanelMode, McpSettingsPanel};
pub use service::{AddArgs, McpDetails, McpEntry, McpService, Scope, Transport};

use std::sync::Arc;

/// Dioxus context wrapper so the panel can pull a shared `McpService`
/// without re-building one per render. The companion mounts this once at
/// chat-region scope; the panel reads it via `use_context()`.
#[derive(Clone)]
pub struct McpServiceCtx(pub Arc<McpService>);
