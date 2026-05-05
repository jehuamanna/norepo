pub mod echo;
pub mod mock;

#[cfg(not(target_arch = "wasm32"))]
pub mod sse;
#[cfg(not(target_arch = "wasm32"))]
pub mod anthropic;

pub use echo::{EchoChatPlugin, EchoToolPlugin};
pub use mock::MockChatPlugin;

#[cfg(not(target_arch = "wasm32"))]
pub use anthropic::{AnthropicChatPlugin, AnthropicConfig};
