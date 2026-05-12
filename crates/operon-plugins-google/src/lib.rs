//! Operon ChatPlugin for Google Gemini (REST API, streaming via SSE).

#[cfg(not(target_arch = "wasm32"))]
pub mod google;
#[cfg(not(target_arch = "wasm32"))]
pub mod sse;

#[cfg(not(target_arch = "wasm32"))]
pub use google::{GoogleChatPlugin, GoogleConfig};
