//! In-process log buffer.
//!
//! `LogBuffer` is a fixed-cap (1000-entry) `VecDeque` of `LogEntry`s shared via `Arc<RwLock<…>>`.
//! It is provided to the Dioxus tree as `Signal<LogBuffer>` so writes via [`Signal::with_mut`]
//! also trigger a re-render of subscribers like [`crate::panel::logs::LogsView`].
//!
//! Use the [`log_info!`], [`log_warn!`], [`log_error!`], [`log_debug!`], [`log_trace!`] macros
//! to push entries; they internally call `Signal::with_mut`.

use std::collections::VecDeque;
use std::sync::{Arc, RwLock};
use web_time::SystemTime;

pub const MAX_ENTRIES: usize = 1000;

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn label(self) -> &'static str {
        match self {
            Self::Trace => "TRACE",
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

#[derive(Clone, Debug)]
pub struct LogEntry {
    pub ts: SystemTime,
    pub level: LogLevel,
    pub message: String,
}

impl LogEntry {
    pub fn new(level: LogLevel, message: String) -> Self {
        Self {
            ts: SystemTime::now(),
            level,
            message,
        }
    }
}

#[derive(Clone, Default)]
pub struct LogBuffer {
    inner: Arc<RwLock<VecDeque<LogEntry>>>,
}

impl LogBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_entry(&self, entry: LogEntry) {
        let mut q = match self.inner.write() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if q.len() >= MAX_ENTRIES {
            q.pop_front();
        }
        q.push_back(entry);
    }

    pub fn snapshot(&self) -> Vec<LogEntry> {
        match self.inner.read() {
            Ok(g) => g.iter().cloned().collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        match self.inner.read() {
            Ok(g) => g.len(),
            Err(_) => 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Format a timestamp as `HH:MM:SS` (UTC). No timezone library; modulo 24/60/60 of seconds-since-epoch.
pub fn format_ts(ts: SystemTime) -> String {
    let secs = ts
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{:02}:{:02}:{:02}", h, m, s)
}

#[macro_export]
macro_rules! log_trace {
    ($sig:expr, $($arg:tt)*) => {{
        let entry = $crate::log::LogEntry::new($crate::log::LogLevel::Trace, format!($($arg)*));
        $sig.with_mut(|b| b.push_entry(entry));
    }};
}

#[macro_export]
macro_rules! log_debug {
    ($sig:expr, $($arg:tt)*) => {{
        let entry = $crate::log::LogEntry::new($crate::log::LogLevel::Debug, format!($($arg)*));
        $sig.with_mut(|b| b.push_entry(entry));
    }};
}

#[macro_export]
macro_rules! log_info {
    ($sig:expr, $($arg:tt)*) => {{
        let entry = $crate::log::LogEntry::new($crate::log::LogLevel::Info, format!($($arg)*));
        $sig.with_mut(|b| b.push_entry(entry));
    }};
}

#[macro_export]
macro_rules! log_warn {
    ($sig:expr, $($arg:tt)*) => {{
        let entry = $crate::log::LogEntry::new($crate::log::LogLevel::Warn, format!($($arg)*));
        $sig.with_mut(|b| b.push_entry(entry));
    }};
}

#[macro_export]
macro_rules! log_error {
    ($sig:expr, $($arg:tt)*) => {{
        let entry = $crate::log::LogEntry::new($crate::log::LogLevel::Error, format!($($arg)*));
        $sig.with_mut(|b| b.push_entry(entry));
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_respects_cap() {
        let buf = LogBuffer::new();
        for i in 0..1500 {
            buf.push_entry(LogEntry::new(LogLevel::Info, format!("msg {i}")));
        }
        let snap = buf.snapshot();
        assert_eq!(snap.len(), MAX_ENTRIES);
        assert_eq!(snap.first().unwrap().message, "msg 500");
        assert_eq!(snap.last().unwrap().message, "msg 1499");
    }

    #[test]
    fn level_labels_are_uppercase() {
        assert_eq!(LogLevel::Info.label(), "INFO");
        assert_eq!(LogLevel::Warn.label(), "WARN");
        assert_eq!(LogLevel::Error.label(), "ERROR");
        assert_eq!(LogLevel::Debug.label(), "DEBUG");
        assert_eq!(LogLevel::Trace.label(), "TRACE");
    }

    #[test]
    fn format_ts_is_zero_padded() {
        let zero = SystemTime::UNIX_EPOCH;
        assert_eq!(format_ts(zero), "00:00:00");
    }

    #[test]
    fn format_ts_known_value() {
        let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(3 * 3600 + 17 * 60 + 5);
        assert_eq!(format_ts(t), "03:17:05");
    }
}
