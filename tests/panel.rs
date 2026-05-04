//! Integration tests for the panel + log buffer.

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
