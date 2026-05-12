//! Operon built-in ToolPlugins.
//!
//! Each tool ships a Rust `ToolPlugin` impl + a prompt fragment. Prompt fragments
//! are kept in `PROMPTS.md` (markdown for ease of editing). The runtime composes the
//! per-tool descriptions into the ChatRequest's `tools:` slice.
//!
//! Slice A0 scaffold: module stubs only. Slices A2–A4 land each tool. Slice A8 lands
//! the `task` tool that spawns child agents.

// File operations (Slice A2 + A3).
#[cfg(not(target_arch = "wasm32"))]
pub mod read;
#[cfg(not(target_arch = "wasm32"))]
pub mod write;
#[cfg(not(target_arch = "wasm32"))]
pub mod glob;
#[cfg(not(target_arch = "wasm32"))]
pub mod edit;
#[cfg(not(target_arch = "wasm32"))]
pub mod grep;

// System (Slice A3).
#[cfg(not(target_arch = "wasm32"))]
pub mod shell;
#[cfg(not(target_arch = "wasm32"))]
pub mod git;

// Web (Slice A4).
#[cfg(not(target_arch = "wasm32"))]
pub mod web_search;
#[cfg(not(target_arch = "wasm32"))]
pub mod web_fetch;

// Sub-agent (Slice A8).
#[cfg(not(target_arch = "wasm32"))]
pub mod task;

// Snapshot / revert (Slice A13).
#[cfg(not(target_arch = "wasm32"))]
pub mod snapshot;

// Multi-step work tracking.
#[cfg(not(target_arch = "wasm32"))]
pub mod todo;

// Patch application (unified diff).
#[cfg(not(target_arch = "wasm32"))]
pub mod apply_patch;

// Repo summary (no LLM).
#[cfg(not(target_arch = "wasm32"))]
pub mod repo_overview;

// AgentBackend impl for the in-process runtime (Slice A14).
#[cfg(not(target_arch = "wasm32"))]
pub mod runtime_backend;
#[cfg(not(target_arch = "wasm32"))]
pub use runtime_backend::{RuntimeAgentBackend, RuntimeBuildArgs, RuntimeFactory};

#[cfg(not(target_arch = "wasm32"))]
mod registry;
#[cfg(not(target_arch = "wasm32"))]
pub use registry::{default_tools, default_tools_with_task, ToolSet};
