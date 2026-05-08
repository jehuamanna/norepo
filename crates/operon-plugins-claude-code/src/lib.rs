//! Operon ChatPlugin that drives the Claude Code CLI via subprocess.
//!
//! Spawns `claude --print --input-format stream-json --output-format stream-json`
//! per turn, with `cwd` bound to the project's repo path. Session continuity
//! across turns is preserved by parsing `session_id` from the result event of
//! turn N and passing `--resume <session_id>` on turn N+1.
//!
//! Streams `assistant` text deltas as `ChatDelta::Text` and the final
//! `result` event as `ChatDelta::Stop` with usage info.

#[cfg(not(target_arch = "wasm32"))]
pub mod event;
#[cfg(not(target_arch = "wasm32"))]
pub mod plugin;
#[cfg(not(target_arch = "wasm32"))]
pub mod stream;

#[cfg(not(target_arch = "wasm32"))]
pub use event::ClaudeCodeEvent;
#[cfg(not(target_arch = "wasm32"))]
pub use plugin::{ClaudeCodeChatPlugin, ClaudeCodeConfig};
