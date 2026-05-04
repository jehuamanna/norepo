//! Integration tests for the panel + log buffer.

mod common;

use common::{assert_log_contains, make_log_buffer, make_panel_manager, push_entry};
use operon_dioxus::log::{LogBuffer, LogEntry, LogLevel, MAX_ENTRIES};
use operon_dioxus::panel::{PanelManager, PanelTabId};

#[test]
fn panel_default_lists_four_tabs_in_order_with_logs_active() {
    let pm = PanelManager::default();
    let titles: Vec<_> = pm.iter().map(|t| t.title).collect();
    assert_eq!(titles, vec!["Terminal", "Output", "Problems", "Logs"]);
    assert_eq!(pm.active(), PanelTabId("logs"));
}

#[test]
fn panel_activate_round_trips() {
    let mut pm = PanelManager::default();
    pm.activate(PanelTabId("output"));
    assert_eq!(pm.active(), PanelTabId("output"));
    pm.activate(PanelTabId("logs"));
    assert_eq!(pm.active(), PanelTabId("logs"));
}

#[test]
fn log_buffer_caps_at_max_entries() {
    let buf = LogBuffer::new();
    for i in 0..(MAX_ENTRIES + 250) {
        buf.push_entry(LogEntry::new(LogLevel::Info, format!("msg {i}")));
    }
    assert_eq!(buf.snapshot().len(), MAX_ENTRIES);
}

#[test]
fn log_buffer_preserves_insertion_order() {
    let buf = LogBuffer::new();
    buf.push_entry(LogEntry::new(LogLevel::Info, "first".into()));
    buf.push_entry(LogEntry::new(LogLevel::Warn, "second".into()));
    buf.push_entry(LogEntry::new(LogLevel::Error, "third".into()));
    let snap = buf.snapshot();
    let messages: Vec<_> = snap.iter().map(|e| e.message.as_str()).collect();
    assert_eq!(messages, vec!["first", "second", "third"]);
}

#[test]
fn panel_manager_and_log_buffer_compose_via_shared_fixtures() {
    // Exercises tests/common/mod.rs builders so a future change to fixture
    // signatures fails fast in this canary test.
    let pm = make_panel_manager();
    let buf = make_log_buffer();

    push_entry(&buf, LogLevel::Info, "Operon: ready");
    push_entry(&buf, LogLevel::Warn, "tick");

    assert_eq!(pm.active(), PanelTabId("logs"));
    assert_log_contains(&buf, LogLevel::Info, "Operon: ready");
    assert_log_contains(&buf, LogLevel::Warn, "tick");
    assert_eq!(buf.snapshot().len(), 2);
}

#[test]
fn assert_log_contains_panics_when_no_match() {
    // Negative path for the helper. We don't actually want a panic at test
    // runtime, so we use std::panic::catch_unwind to assert the helper is
    // strict (and to document that it WILL panic on miss).
    let buf = make_log_buffer();
    push_entry(&buf, LogLevel::Info, "hello");

    let outcome = std::panic::catch_unwind(|| {
        assert_log_contains(&buf, LogLevel::Error, "hello");
    });
    assert!(
        outcome.is_err(),
        "assert_log_contains must panic when no entry matches the given level"
    );
}
