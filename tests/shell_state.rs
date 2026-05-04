//! Integration tests covering interactions across LayoutState, PanelManager,
//! and CommandRegistry — the three "shell-state" pieces App wires together.
//!
//! Single-module behaviour is covered by inline `#[cfg(test)] mod tests`
//! blocks in each source module; this file specifically exercises the
//! cross-module shape that `App` composes at startup.
//!
//! Authored under the "Playwright for testing" Archon seed
//! (84185cbf-0b4f-4211-bb33-145a9817ac0c, Plans-Phase-3-integration-test-scaffolding).

mod common;

use common::{
    make_command_registry, make_layout, make_log_buffer, make_panel_manager,
    make_plugin_registry, push_entry,
};
use operon_dioxus::log::LogLevel;
use operon_dioxus::panel::PanelTabId;

#[test]
fn shell_state_default_graph_is_internally_consistent() {
    let layout = make_layout();
    let panel = make_panel_manager();
    let logs = make_log_buffer();
    let cmds = make_command_registry();
    let plugins = make_plugin_registry();

    // PanelManager defaults to logs active so the LogsView is the visible body.
    assert_eq!(panel.active(), PanelTabId("logs"));
    // LogBuffer starts empty; no startup tracing without an explicit push.
    assert!(logs.is_empty());
    // CommandRegistry has the built-ins seeded by register_builtin_commands.
    assert!(cmds.iter().count() >= 6, "builtin commands should be registered");
    // PluginRegistry begins empty until App or tests register plugins.
    assert_eq!(plugins.note_plugins().count(), 0);
    // Layout is in an interactive (non-collapsed) shape so all three tracks
    // contribute non-zero width — wiring is sound.
    assert!(layout.sidebar_track() > 0);
    assert!(layout.companion_track() > 0);
    assert!(layout.panel_track() > 0);
}

#[test]
fn layout_toggles_do_not_disturb_panel_manager_or_log_buffer() {
    let mut layout = make_layout();
    let panel = make_panel_manager();
    let buf = make_log_buffer();

    push_entry(&buf, LogLevel::Info, "before-toggle");

    layout.toggle_sidebar();
    layout.toggle_panel();

    // Panel + buffer state is independent of layout.
    assert_eq!(panel.active(), PanelTabId("logs"));
    assert_eq!(buf.snapshot().len(), 1);

    // Round-trip the toggles to confirm idempotence end-to-end.
    layout.toggle_sidebar();
    layout.toggle_panel();
    assert_eq!(panel.active(), PanelTabId("logs"));
    assert_eq!(buf.snapshot().len(), 1);
}

#[test]
fn collapsed_panel_track_returns_zero_but_logs_continue_to_buffer() {
    // App-level invariant: collapsing the panel hides it visually but logs
    // pushed during that interval still land in the buffer (no data loss).
    let mut layout = make_layout();
    let buf = make_log_buffer();

    layout.toggle_panel();
    assert_eq!(layout.panel_track(), 0, "collapsed panel reports zero track");

    push_entry(&buf, LogLevel::Warn, "warn-while-collapsed");
    push_entry(&buf, LogLevel::Error, "error-while-collapsed");

    assert_eq!(buf.snapshot().len(), 2);
    let levels: Vec<_> = buf.snapshot().iter().map(|e| e.level).collect();
    assert_eq!(levels, vec![LogLevel::Warn, LogLevel::Error]);
}
