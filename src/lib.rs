//! Operon — Rust + Dioxus 0.7 GUI shell.
//!
//! The crate exposes its modules so that integration tests under `tests/` can drive the
//! plugin/tab/command surfaces without launching the full Dioxus runtime.

pub mod app;
pub mod commands;
pub mod plugin;
pub mod plugins;
pub mod shell;
pub mod tabs;
pub mod theme;
