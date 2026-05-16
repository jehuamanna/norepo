//! Operon bridge — lets a Claude Code process running outside the
//! Operon GUI (most importantly: the PTY-hosted `claude` in the
//! companion terminal pane) call back into the GUI to read/write
//! notes, drain pending mentions, ask the user questions, and so on.
//!
//! Architecture
//! ============
//!
//! ```text
//!   Operon GUI (this lib)            operon-mcp stub bin              claude CLI
//!   ──────────────────────           ─────────────────                ──────────
//!   Server::serve                    main()                           (subprocess)
//!     ↓ unix socket bind               ↓ unix socket connect            ↓ stdio MCP
//!   accept loop                      hello handshake                  initialize / tools/list / tools/call
//!     ↓                                ↓                                ↑
//!   per-conn task ←── newline JSON ──→ pipe loop ←──── stdio JSON ────→ MCP client (Claude)
//! ```
//!
//! `Server` exposes a registration API (`register_tool`) for the GUI
//! to plug in tool handlers. Handlers are simple async functions; the
//! server takes care of MCP wire-format details (initialize,
//! tools/list, tools/call dispatch) so the GUI's tools never see the
//! protocol layer.
//!
//! Trust model
//! -----------
//! The socket lives at a path the GUI controls (typically
//! `<vault>/.operon/bridge.sock` or `$XDG_RUNTIME_DIR/operon-bridge-<pid>.sock`).
//! Same-user isolation comes from filesystem permissions (chmod 600
//! on the parent directory or socket itself). The hello frame
//! additionally carries a per-server-instance UUID token; the bridge
//! rejects connections that don't present it. This guards against
//! another process on the same user account stumbling onto the socket.
//!
//! Wire format
//! -----------
//! Newline-delimited JSON-RPC 2.0, same shape the existing
//! `StdioMcpClient` (`crates/operon-plugins-mcp/src/stdio.rs`) speaks.
//! The bridge handshake adds one Operon-specific method:
//!
//! ```text
//! → {"jsonrpc":"2.0","id":1,"method":"operon/hello",
//!    "params":{"token":"<uuid>","client":"operon-mcp/0.1","session":"<uuid?>"}}
//! ← {"jsonrpc":"2.0","id":1,"result":{"ok":true,"server":"operon-bridge/0.1"}}
//! ```
//!
//! All subsequent frames are forwarded verbatim between Claude and
//! the server; the stub neither parses nor rewrites them.

#![cfg(unix)]

pub mod protocol;
pub mod server;

pub use protocol::{Frame, FrameError};
pub use server::{Server, ServerHandle, ToolHandler, ToolHandlerError};

use thiserror::Error;

/// All ways the bridge can fail. Keep this enum stable — surfaces in
/// the bin's exit code mapping and in the GUI's error reporter.
#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("framing: {0}")]
    Frame(#[from] protocol::FrameError),

    #[error("auth: {0}")]
    Auth(String),

    #[error("tool: {0}")]
    Tool(String),

    #[error("shutdown")]
    Shutdown,
}

pub type Result<T> = std::result::Result<T, BridgeError>;
