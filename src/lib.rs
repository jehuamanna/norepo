//! Operon — Rust + Dioxus 0.7 GUI shell.
//!
//! The crate exposes its modules so that integration tests under `tests/` can drive the
//! plugin/tab/command surfaces without launching the full Dioxus runtime.

pub mod agent;
pub mod app;
pub mod commands;
pub mod editor;
pub mod log;
pub mod panel;
pub mod persistence;
pub mod plugin;
pub mod plugins;
pub mod shell;
pub mod tabs;
pub mod theme;
pub mod ui;
