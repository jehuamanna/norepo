//! Thin facade re-exporting the workspace-split agent crates.
//!
//! After the Plans-Phase-4 split, the actual code lives in:
//! - `operon-core` (traits, runtime, memory, error, budget, bus, secrets, config, registry, echo, mock)
//! - `operon-plugins-anthropic` (AnthropicChatPlugin, SSE parser)
//! - `operon-plugins-mcp` (StdioMcpClient, McpToolProxy, GrantHandler)
//!
//! This module preserves the pre-split `operon_dioxus::agent::*` import surface so
//! existing tests, future migration efforts, and downstream code continue to work.

pub use operon_core::*;

pub mod error {
    pub use operon_core::error::*;
}
pub mod budget {
    pub use operon_core::budget::*;
}
pub mod bus {
    pub use operon_core::bus::*;
}
pub mod config {
    pub use operon_core::config::*;
}
pub mod secrets {
    pub use operon_core::secrets::*;
}
pub mod traits {
    pub use operon_core::traits::*;
}
pub mod registry {
    pub use operon_core::registry::*;
}
pub mod memory {
    pub use operon_core::memory::*;
}
pub mod tracing_init {
    pub use operon_core::tracing_init::*;
}
pub mod session {
    pub use operon_core::session::*;
}

#[cfg(not(target_arch = "wasm32"))]
pub mod runtime {
    pub use operon_core::runtime::*;
}

pub mod plugins {
    pub use operon_core::echo::{EchoChatPlugin, EchoToolPlugin};
    pub use operon_core::mock::MockChatPlugin;
    #[cfg(not(target_arch = "wasm32"))]
    pub use operon_plugins_anthropic::{AnthropicChatPlugin, AnthropicConfig};
}

pub mod mcp {
    pub use operon_plugins_mcp::{
        AutoApproveGrantHandler, DenyAllGrantHandler, GrantHandler, McpToolProxy,
        SecretStoreGrantHandler,
    };
    #[cfg(not(target_arch = "wasm32"))]
    pub use operon_plugins_mcp::{StdioMcpClient, StdioMcpServerConfig};
}
