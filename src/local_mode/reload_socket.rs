//! Per-process Unix-domain socket that receives PostToolUse reload
//! notifications from the `operon-posttool-hook` binary.
//!
//! Claude Code's `Write`/`Edit` tools touch files on disk; an inotify
//! watcher catches *most* of those writes but is unreliable across
//! filesystem types (NFS, encrypted overlays, atomic-rename
//! sequences). The PostToolUse hook is a deterministic backstop:
//! Claude itself tells us "I just wrote `/abs/path/X`", and we walk
//! every open tab to reload the matching one.
//!
//! Socket lifetime is the Operon process — bound once at app start,
//! unlinked on app exit (best-effort; a leftover socket file is
//! harmless because the next bind unlinks it first).

#![cfg(not(target_arch = "wasm32"))]

use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex;

/// One reload request from the hook binary.
#[derive(Debug, Clone)]
pub struct ReloadEvent {
    /// Tool that fired the hook — informational; we treat all of them
    /// the same way (reload the matching tab from disk).
    pub tool: String,
    /// Absolute (or near-absolute) path Claude reported writing to.
    /// Operon resolves this against each open tab's
    /// `persistence.resolved_path(...)` to find the matching tab.
    pub path: PathBuf,
    /// Optional human-readable summary, extracted by the hook from
    /// the most-recent assistant text block in the session
    /// transcript. Used verbatim as the artifact revision-row
    /// summary when present — far more useful than the diff-based
    /// `Edited body (N lines)` fallback.
    pub summary: Option<String>,
}

/// Global sender installed once by [`start`]. The receiver half is
/// held in [`RECEIVER`] and consumed by the desktop-bootstrap task.
static SENDER: OnceLock<UnboundedSender<ReloadEvent>> = OnceLock::new();

/// Global receiver — wrapped in a `Mutex<Option<_>>` so the desktop
/// bootstrap can `take` it exactly once, then drive it from a Dioxus
/// `spawn`. Wrapping in `Mutex` keeps everything `Sync` for the
/// `OnceLock`.
static RECEIVER: OnceLock<Mutex<Option<UnboundedReceiver<ReloadEvent>>>> = OnceLock::new();

/// Tracks whether [`start`] has successfully bound. Once `true`, the
/// artifact watcher in `desktop.rs` defers revision-row appends to
/// the hook receiver (so the row carries Claude's actual explanation
/// instead of a diff-based fallback).
static BOUND_OK: OnceLock<bool> = OnceLock::new();

/// True iff the per-process socket is bound and accepting hook
/// callbacks. Callers use this to decide whether to wait for a
/// hook-supplied revision summary or fall back to the diff-based one
/// the watcher computes locally.
pub fn is_bound() -> bool {
    *BOUND_OK.get().unwrap_or(&false)
}

/// Bind a per-process Unix socket and start the accept loop.
///
/// Returns `Some(socket_path)` on success — that path is what gets
/// baked into the hook command line in `.claude/settings.local.json`.
/// Returns `None` if the bind failed (non-Unix FS, permission, etc.);
/// callers skip the hook-config installation in that case.
///
/// Idempotent: a second call returns the path the first call bound
/// (or `None` if the first call failed).
pub fn start() -> Option<PathBuf> {
    static BOUND_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    BOUND_PATH
        .get_or_init(|| {
            let (tx, rx) = unbounded_channel::<ReloadEvent>();
            let path = match bind_listener(tx.clone()) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(
                        target: "operon::reload_socket",
                        "bind failed: {e}"
                    );
                    let _ = BOUND_OK.set(false);
                    return None;
                }
            };
            // Stash sender + receiver in the globals before the
            // listener starts handing out events.
            let _ = SENDER.set(tx);
            let _ = RECEIVER.set(Mutex::new(Some(rx)));
            let _ = BOUND_OK.set(true);
            Some(path)
        })
        .clone()
}

/// Take the global receiver. Returns `None` on the second and later
/// calls — the desktop bootstrap expects this to be called exactly
/// once.
pub async fn take_receiver() -> Option<UnboundedReceiver<ReloadEvent>> {
    let slot = RECEIVER.get()?;
    slot.lock().await.take()
}

fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir());
    let pid = std::process::id();
    dir.join(format!("operon-reload-{pid}.sock"))
}

fn bind_listener(tx: UnboundedSender<ReloadEvent>) -> std::io::Result<PathBuf> {
    let path = socket_path();
    // Unlink any leftover from a previous run at this exact pid (rare
    // but cheap to guard against).
    let _ = std::fs::remove_file(&path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(&path)?;
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        let mut lines = BufReader::new(stream).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            let trimmed = line.trim();
                            if trimmed.is_empty() {
                                continue;
                            }
                            let Ok(v) = serde_json::from_str::<Value>(trimmed) else {
                                continue;
                            };
                            let Some(path) =
                                v.get("path").and_then(|x| x.as_str()).map(PathBuf::from)
                            else {
                                continue;
                            };
                            let tool = v
                                .get("tool")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string();
                            let summary = v
                                .get("summary")
                                .and_then(|x| x.as_str())
                                .filter(|s| !s.is_empty())
                                .map(|s| s.to_string());
                            let _ = tx.send(ReloadEvent { tool, path, summary });
                        }
                    });
                }
                Err(e) => {
                    tracing::warn!(
                        target: "operon::reload_socket",
                        "accept: {e}"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
        }
    });
    Ok(path)
}
