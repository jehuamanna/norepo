//! Operon — Rust + Dioxus 0.7 GUI shell.
//!
//! The crate exposes its modules so that integration tests under `tests/` can drive the
//! plugin/tab/command surfaces without launching the full Dioxus runtime.

pub mod app;
pub mod plugin;
pub mod shell;
pub mod theme;
