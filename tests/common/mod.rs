//! Shared fixtures for Cargo integration tests.
//!
//! Each `tests/*.rs` file pulls these in via `mod common; use common::*;`.
//! Builders construct a fully-populated unit of state suitable for a test;
//! `assert_log_contains` is a friendlier panic message than open-coded loops.
//!
//! Authored under the "Playwright for testing" Archon seed
//! (84185cbf-0b4f-4211-bb33-145a9817ac0c, Plans-Phase-3-integration-test-scaffolding).

#![allow(dead_code)]

use operon_dioxus::commands::{register_builtin_commands, CommandRegistry};
use operon_dioxus::log::{LogBuffer, LogEntry, LogLevel};
use operon_dioxus::panel::PanelManager;
use operon_dioxus::plugin::PluginRegistry;
use operon_dioxus::shell::layout::LayoutState;

pub fn make_command_registry() -> CommandRegistry {
    let mut r = CommandRegistry::new();
    register_builtin_commands(&mut r).expect("builtin commands register");
    r
}

pub fn make_log_buffer() -> LogBuffer {
    LogBuffer::new()
}

pub fn make_panel_manager() -> PanelManager {
    PanelManager::default()
}

pub fn make_layout() -> LayoutState {
    LayoutState::default()
}

pub fn make_plugin_registry() -> PluginRegistry {
    PluginRegistry::new()
}

/// Push a log entry into a freshly-constructed buffer; convenience for tests
/// that want a buffer pre-populated with one or more entries.
pub fn push_entry(buf: &LogBuffer, level: LogLevel, message: &str) {
    buf.push_entry(LogEntry::new(level, message.to_string()));
}

/// Assert at least one entry in the buffer has the given level AND message
/// substring. Panics with a readable diff of the buffer contents on failure.
pub fn assert_log_contains(buf: &LogBuffer, level: LogLevel, needle: &str) {
    let hits: Vec<LogEntry> = buf
        .snapshot()
        .into_iter()
        .filter(|e| e.level == level && e.message.contains(needle))
        .collect();
    assert!(
        !hits.is_empty(),
        "expected a {level:?} log containing {needle:?}, got: {:?}",
        buf.snapshot()
    );
}
