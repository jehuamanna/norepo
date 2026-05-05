//! Operon ChatPlugin for the Anthropic Messages API.
//!
//! Streams responses, honours prompt caching, surfaces cache-hit telemetry.

#[cfg(not(target_arch = "wasm32"))]
pub mod anthropic;
#[cfg(not(target_arch = "wasm32"))]
pub mod sse;

#[cfg(not(target_arch = "wasm32"))]
pub use anthropic::{AnthropicChatPlugin, AnthropicConfig};
