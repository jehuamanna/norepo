//! Companion pane: raw Claude Code terminal surface (per-repo).
//!
//! Mounted by `CompanionArea` when the user picks Settings → Companion
//! pane → "Claude Code". Each git repo bound to a project gets its own
//! independent claude PTY session that survives:
//! - the user toggling between projects (returning to a repo restores
//!   the previous session, scrollback intact)
//! - collapse/expand of the companion pane
//! - flipping between Chat ↔ Claude Code modes in Settings
//!
//! The session is keyed by absolute repo path, so two projects bound to
//! the same repository share one terminal — what the user sees is
//! "the claude session for *this code*", not "the claude session for
//! *this project row*".
//!
//! ## When no repo is bound
//!
//! - `ChatScope::Vault` → "select a project" placeholder. The vault
//!   has no repo and so has no terminal.
//! - `ChatScope::Project(pid)` with `repo_path = NULL` → "this project
//!   has no bound repository" placeholder pointing at the project gear
//!   menu.
//! - `ChatScope::Project(pid)` with `repo_path = Some(p)` but `p` is
//!   not a directory → "repo path no longer exists" placeholder.
//!
//! Only the third state — a project pointing at an existing directory
//! — actually mounts an xterm + spawns claude.
//!
//! ## Session lifetime
//!
//! Sessions live in a process-global registry ([`SESSIONS`]). They are
//! created lazily on first attach and removed automatically when the
//! claude child exits (so the next attach gets a fresh session). The
//! reader runs on a dedicated OS thread; bytes are appended to a
//! capped scrollback buffer and broadcast to any active xterm
//! attaches via `tokio::sync::broadcast`. A new attach replays the
//! scrollback so the user sees the existing screen state immediately.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use dioxus::prelude::*;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tokio::sync::broadcast;

use crate::local_mode::desktop::{LocalNoteRepo, LocalProjectRepo};
use crate::local_mode::explorer::{LocalProjectVersion, SelectedNote, SelectedProject};
use crate::shell::companion_chat::resolve_claude_bin;

/// xterm.js + fit addon are vendored under `assets/xterm/` and
/// inlined into the binary. Shipping them in-process avoids two
/// failure modes the previous CDN-based version hit: (a) the dioxus
/// desktop webview's CSP blocking external `https://…` imports and
/// (b) the user being offline. They're injected on first attach via
/// `<script>`/`<style>` tags whose `textContent` is the file body —
/// the browser then evaluates the UMD wrappers, which assign
/// `window.Terminal` / `window.FitAddon.FitAddon`.
const XTERM_JS: &str = include_str!("../../assets/xterm/xterm.min.js");
const XTERM_CSS: &str = include_str!("../../assets/xterm/xterm.min.css");
const XTERM_FIT_JS: &str =
    include_str!("../../assets/xterm/xterm-addon-fit.min.js");

/// xterm `ITheme` JSON for the active theme kind. Colours stay close
/// to VS Code's Dark+ / Light+ defaults so the terminal blends with
/// the rest of the app. We fully specify the 16 ANSI slots so
/// claude's ratatui output (which uses 256-colour SGR) doesn't fall
/// back to xterm's built-in white-on-black palette and look out of
/// place under Light.
fn xterm_theme_json_for(kind: crate::theme::ThemeKind) -> String {
    use crate::theme::ThemeKind;
    match kind {
        ThemeKind::Light => serde_json::json!({
            "background": "#ffffff",
            "foreground": "#3b3b3b",
            "cursor": "#3b3b3b",
            "cursorAccent": "#ffffff",
            "selectionBackground": "#add6ff",
            "selectionForeground": "#000000",
            "black": "#000000",
            "red": "#cd3131",
            "green": "#00bc00",
            "yellow": "#949800",
            "blue": "#0451a5",
            "magenta": "#bc05bc",
            "cyan": "#0598bc",
            "white": "#555555",
            "brightBlack": "#666666",
            "brightRed": "#cd3131",
            "brightGreen": "#14ce14",
            "brightYellow": "#b5ba00",
            "brightBlue": "#0451a5",
            "brightMagenta": "#bc05bc",
            "brightCyan": "#0598bc",
            "brightWhite": "#a5a5a5",
        })
        .to_string(),
        // Dark + HighContrast both use the dark palette. HighContrast
        // could get its own, but the upstream claude TUI already uses
        // high-contrast-ready foregrounds, so this is OK for now.
        _ => serde_json::json!({
            "background": "#1e1e1e",
            "foreground": "#d4d4d4",
            "cursor": "#d4d4d4",
            "cursorAccent": "#1e1e1e",
            "selectionBackground": "#264f78",
            "black": "#000000",
            "red": "#cd3131",
            "green": "#0dbc79",
            "yellow": "#e5e510",
            "blue": "#2472c8",
            "magenta": "#bc3fbc",
            "cyan": "#11a8cd",
            "white": "#e5e5e5",
            "brightBlack": "#666666",
            "brightRed": "#f14c4c",
            "brightGreen": "#23d18b",
            "brightYellow": "#f5f543",
            "brightBlue": "#3b8eea",
            "brightMagenta": "#d670d6",
            "brightCyan": "#29b8db",
            "brightWhite": "#e5e5e5",
        })
        .to_string(),
    }
}

/// Cap on the per-session scrollback replay buffer. Big enough to
/// catch a full claude session bootstrap + a few screens of chat;
/// small enough that a long-running session doesn't grow unbounded.
/// xterm.js itself maintains its own scrollback (5000 lines, set on
/// the JS side); this buffer only matters for the *first* paint when
/// a new attach comes in.
const SCROLLBACK_BYTES: usize = 256 * 1024;

/// Broadcast channel capacity per session. Output bytes are
/// fan-out to active xterm attaches; if a subscriber falls behind the
/// channel drops oldest messages and the subscriber sees a `Lagged`
/// error — at that point we reset the screen and replay scrollback.
const BROADCAST_CAP: usize = 4096;

/// One persistent claude PTY session per repo path. Held in [`SESSIONS`].
struct ClaudeSession {
    /// Live byte-stream fan-out. New attaches subscribe; the reader
    /// thread broadcasts every PTY chunk.
    out_tx: broadcast::Sender<Vec<u8>>,
    /// Capped replay buffer so a fresh attach can paint the existing
    /// screen state before live bytes start flowing.
    scrollback: Arc<Mutex<VecDeque<u8>>>,
    /// Stdin side of the PTY. std `Mutex` (not `tokio::sync::Mutex`) on
    /// purpose — awaiting an async mutex from inside the bridge future
    /// can resume the future on a tokio worker that doesn't have the
    /// Dioxus runtime guard, and the next `Eval` access then panics
    /// with "Must be called from inside a Dioxus runtime". The writes
    /// are tiny (a few keystroke bytes), so the brief sync lock here
    /// is fine.
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Held for resize calls. `MasterPty::resize` takes `&self`, so a
    /// std `Mutex` is enough.
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
}

fn sessions() -> &'static Mutex<HashMap<PathBuf, Arc<ClaudeSession>>> {
    static MAP: OnceLock<Mutex<HashMap<PathBuf, Arc<ClaudeSession>>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Look up an existing session for `repo_path` or spawn a fresh
/// `claude` child in a PTY rooted there. Idempotent — the second
/// caller for the same repo path gets the same `Arc<ClaudeSession>`.
/// System-prompt snippet appended to terminal-mode claude via
/// `--append-system-prompt`. Tells the model that the operon MCP
/// tools exist and when to reach for them instead of asking the user
/// to paste note bodies / pick options in plain text.
///
/// Kept short on purpose: the prompt is paid on every turn and
/// already has the cache benefit of being identical across sessions.
/// If you add or rename a tool in `bridge_note_tools.rs` or
/// `bridge_ask_user_tool.rs`, update the corresponding line here.
const OPERON_TOOLS_SYSTEM_PROMPT: &str = "\
You have MCP tools provided by Operon (prefix `mcp__operon_notes__`). Prefer them \
over asking the user to paste note contents:\n\
- `get_note(note_id)` — read a note's body + on-disk path. Use the path with \
your built-in Edit/Write when you want surgical changes that round-trip \
through Operon's live editor.\n\
- `list_notes(project_id)` — enumerate a project's note tree.\n\
- `search_notes(query, project_id?)` — case-insensitive title substring search.\n\
- `create_note(project_id, title, kind?, parent_id?, body?)` — new note in a \
project; `kind` defaults to `markdown`.\n\
- `append_note(note_id, text)` — append text to an existing note.\n\
- `replace_note_range(note_id, old_text, new_text, replace_all?)` — anchor-based \
find/replace inside a note; same semantics as your Edit tool but targeted by UUID.\n\
- `ask_user({questions: [...]})` — structured-options picker. Use this instead \
of plain-text clarifying questions when there are discrete choices.\n\
The built-in AskUserQuestion is disabled here; reach for `ask_user` instead.\n\
\n\
When the user's message contains a `@[Title](note:<uuid>)` reference (typed by \
them or injected by the GUI's Send-to-Claude action), treat it as a request to \
fetch that note — call `get_note(<uuid>)` first, then answer with the note's \
contents in scope. Don't ask the user to paste it.";

/// Optional env injection for the spawned `claude`. When `Some`, the
/// bridge env vars are pushed into the child's environment AND
/// `--mcp-config <mcp_config_path>` is added to the args so claude
/// discovers the `operon` MCP server. `None` keeps the legacy
/// behaviour (no bridge — the in-process `BridgeAskUserExecutor` is
/// still the only path).
#[derive(Clone)]
pub struct BridgeEnv {
    pub sock_path: PathBuf,
    pub token: String,
    /// Path to the per-process `.mcp.json` written by
    /// `bridge_runtime::start_bridge_runtime`. `None` when the
    /// bridge came up but the config file couldn't be written
    /// (typically: missing `operon-mcp` binary); in that case env
    /// vars still get injected — they're harmless on their own —
    /// but no `--mcp-config` flag is added.
    pub mcp_config_path: Option<PathBuf>,
}

fn get_or_create_session(
    repo_path: &Path,
    claude_bin: &Path,
    bridge: Option<&BridgeEnv>,
) -> Result<Arc<ClaudeSession>, String> {
    {
        let map = sessions().lock().unwrap();
        if let Some(s) = map.get(repo_path) {
            return Ok(s.clone());
        }
    }

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("openpty failed: {e}"))?;

    let mut cmd = CommandBuilder::new(claude_bin.as_os_str());
    cmd.cwd(repo_path.as_os_str());
    // Inherit the parent env so claude finds login state, NODE_PATH
    // (standalone installer), PATH lookups for sub-tools, etc.
    // CommandBuilder defaults to a *clean* env, which would break
    // basically everything.
    for (k, v) in std::env::vars_os() {
        cmd.env(k, v);
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");

    // M4b.4/.5: hand the bridge handle to the child. Env vars are
    // belt-and-braces; the canonical config lives in the per-process
    // `.mcp.json` referenced via `--mcp-config`, which has its own
    // env block for the `operon-mcp` stub. Setting them on claude's
    // env too means a hand-written `.mcp.json` (or an `mcp__operon`
    // entry the user adds via the chat-mode UI) also resolves.
    if let Some(b) = bridge {
        cmd.env("OPERON_BRIDGE_SOCK", b.sock_path.as_os_str());
        cmd.env("OPERON_BRIDGE_TOKEN", &b.token);
        if let Some(cfg) = b.mcp_config_path.as_ref() {
            cmd.arg("--mcp-config");
            cmd.arg(cfg.as_os_str());
            // M4c.8: nudge the model toward the operon tools.
            // Without this, Claude tends to ask the user to paste
            // note bodies even though `mcp__operon_notes__get_note` would
            // fetch them in one round-trip. The chat-mode bridge has
            // its own (smaller) hint focused on ask_user — see
            // `plugin.rs::--append-system-prompt`. Keep this in
            // sync if you add or rename tools.
            cmd.arg("--append-system-prompt");
            cmd.arg(OPERON_TOOLS_SYSTEM_PROMPT);
            tracing::info!(
                target: "operon::companion_terminal",
                sock = %b.sock_path.display(),
                mcp_config = %cfg.display(),
                "injected operon-bridge env + --mcp-config + system prompt into claude spawn"
            );
        } else {
            tracing::warn!(
                target: "operon::companion_terminal",
                sock = %b.sock_path.display(),
                "bridge alive but no .mcp.json; claude will not see operon tools"
            );
        }
    }

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("spawn failed: {e}"))?;
    let pid = child.process_id().unwrap_or(0);
    tracing::info!(
        target: "operon::companion_terminal",
        pid,
        repo_path = %repo_path.display(),
        "spawned claude PTY child"
    );
    // Drop our slave handle so the master sees EOF when the child exits.
    drop(pair.slave);

    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("take_writer failed: {e}"))?;
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("try_clone_reader failed: {e}"))?;

    let scrollback: Arc<Mutex<VecDeque<u8>>> =
        Arc::new(Mutex::new(VecDeque::with_capacity(SCROLLBACK_BYTES)));
    let (out_tx, _) = broadcast::channel::<Vec<u8>>(BROADCAST_CAP);

    let scrollback_for_reader = scrollback.clone();
    let out_tx_for_reader = out_tx.clone();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = [0u8; 4096];
        let mut total: u64 = 0;
        let mut reads: u64 = 0;
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    tracing::warn!(
                        target: "operon::companion_terminal",
                        total_bytes = total,
                        reads,
                        "PTY reader EOF"
                    );
                    break;
                }
                Ok(n) => {
                    reads += 1;
                    total += n as u64;
                    tracing::trace!(
                        target: "operon::companion_terminal",
                        n,
                        total_bytes = total,
                        reads,
                        "PTY chunk"
                    );
                    let chunk = buf[..n].to_vec();
                    {
                        let mut sb = scrollback_for_reader.lock().unwrap();
                        let overflow = (sb.len() + chunk.len())
                            .saturating_sub(SCROLLBACK_BYTES);
                        for _ in 0..overflow {
                            sb.pop_front();
                        }
                        sb.extend(chunk.iter().copied());
                    }
                    // Ignore SendError — it just means no attaches
                    // are currently subscribed.
                    let _ = out_tx_for_reader.send(chunk);
                }
                Err(e) => {
                    tracing::warn!(
                        target: "operon::companion_terminal",
                        err = %e,
                        total_bytes = total,
                        reads,
                        "PTY reader error"
                    );
                    break;
                }
            }
        }
    });

    // Watcher: when claude exits, evict the session so the next
    // attach spawns a fresh one. We don't broadcast an "exit" message
    // — the reader thread's EOF naturally closes out_tx so the
    // subscriber loops fall through.
    let repo_path_for_cleanup = repo_path.to_path_buf();
    std::thread::spawn(move || {
        let exit = child.wait();
        tracing::warn!(
            target: "operon::companion_terminal",
            ?exit,
            repo_path = %repo_path_for_cleanup.display(),
            "claude PTY child exited; evicting session"
        );
        let mut map = sessions().lock().unwrap();
        map.remove(&repo_path_for_cleanup);
    });

    let session = Arc::new(ClaudeSession {
        out_tx,
        scrollback,
        writer: Arc::new(Mutex::new(writer)),
        master: Arc::new(Mutex::new(pair.master)),
    });
    sessions()
        .lock()
        .unwrap()
        .insert(repo_path.to_path_buf(), session.clone());
    Ok(session)
}

/// What to render based on the explorer's current selection.
#[derive(Clone, Debug, PartialEq)]
enum RepoState {
    /// No project (or note inside a project) is selected in the
    /// explorer.
    NoProjectSelected,
    /// Project is selected but `local_project.repo_path` is NULL.
    ProjectNoRepoBinding(String),
    /// repo_path is set but the directory no longer exists on disk.
    ProjectMissingRepo { project_name: String, path: PathBuf },
    /// Ready to mount a terminal.
    Bound { project_name: String, repo_path: PathBuf },
}

#[component]
pub fn CompanionClaudeTerminal() -> Element {
    // The terminal follows the EXPLORER's active project — not the
    // companion-rail's `ActiveChatScope` (which has its own Project /
    // Vault tabs and defaults to Vault even when a project is
    // selected). When the user clicks a project row we pick up the id
    // from `SelectedProject`; when they click a *note* inside a
    // project we resolve the owning project via `LocalNoteRepository::
    // list_for_project` so the terminal still attaches to that
    // project's repo.
    let selected_project = use_context::<SelectedProject>().0;
    let selected_note_ctx = try_consume_context::<SelectedNote>().map(|c| c.0);
    let project_repo = try_consume_context::<LocalProjectRepo>().map(|c| c.0);
    let note_repo = try_consume_context::<LocalNoteRepo>().map(|c| c.0);
    let project_version =
        try_consume_context::<LocalProjectVersion>().map(|c| c.0);

    let project_repo_for_memo = project_repo.clone();
    let note_repo_for_memo = note_repo.clone();
    let repo_state = use_memo(move || -> RepoState {
        if let Some(v) = project_version.as_ref() {
            let _ = v.read();
        }
        let pid_from_explorer = *selected_project.read();
        let nid_from_explorer = selected_note_ctx
            .as_ref()
            .map(|s| *s.read())
            .unwrap_or(None);

        let repo = match project_repo_for_memo.as_ref() {
            Some(r) => r,
            None => return RepoState::NoProjectSelected,
        };
        let projects = match repo.list() {
            Ok(rows) => rows,
            Err(_) => return RepoState::NoProjectSelected,
        };

        // Resolve the active project: explicit selection wins; falling
        // back to the project that owns the currently-open note.
        let project = if let Some(pid) = pid_from_explorer {
            projects.into_iter().find(|p| p.id == pid)
        } else if let (Some(nid), Some(notes)) =
            (nid_from_explorer, note_repo_for_memo.as_ref())
        {
            projects.into_iter().find(|p| {
                notes
                    .list_for_project(p.id)
                    .map(|rows| rows.iter().any(|r| r.id == nid))
                    .unwrap_or(false)
            })
        } else {
            None
        };

        let Some(project) = project else {
            return RepoState::NoProjectSelected;
        };
        let project_name = project.name.clone();
        let Some(rp) = project.repo_path else {
            return RepoState::ProjectNoRepoBinding(project_name);
        };
        if !rp.is_dir() {
            return RepoState::ProjectMissingRepo {
                project_name,
                path: rp,
            };
        }
        RepoState::Bound {
            project_name,
            repo_path: rp,
        }
    });

    let state = repo_state.read().clone();
    match state {
        RepoState::NoProjectSelected => rsx! {
            EmptyState {
                title: "No project selected",
                body: "Claude Code runs per repository. Pick a project with a bound repository in the explorer to start a session.",
            }
        },
        RepoState::ProjectNoRepoBinding(name) => rsx! {
            EmptyState {
                title: "No repository bound to “{name}”",
                body: "This project doesn’t have a git repository bound. Right-click the project in the explorer → “Bind repository…” to point it at a working tree, then come back here.",
            }
        },
        RepoState::ProjectMissingRepo { project_name, path } => rsx! {
            EmptyState {
                title: "Repository folder is missing",
                body: "“{project_name}” is bound to a path that no longer exists. Re-bind it from the explorer’s context menu.",
                detail: "{path.display()}",
            }
        },
        RepoState::Bound {
            project_name,
            repo_path,
        } => {
            // `key` is bound to the repo path so a project switch
            // remounts this child against the new repo's session.
            // The previous repo's session stays in the registry and
            // the user can come back to it later.
            let key = repo_path.display().to_string();
            rsx! {
                ClaudeRepoTerminal {
                    key: "{key}",
                    repo_path: repo_path.clone(),
                    project_name: project_name.clone(),
                }
            }
        }
    }
}

#[component]
fn EmptyState(
    title: String,
    body: String,
    #[props(default)] detail: Option<String>,
) -> Element {
    // Use the `--vscode-*` theme tokens the rest of the shell pulls
    // from. The `--operon-*` variables aren't actually populated —
    // they were a misread on my part and made the empty-state stay
    // dark on Light theme.
    rsx! {
        div {
            class: "operon-companion-terminal-empty",
            "data-testid": "companion-claude-terminal-empty",
            style: "display: flex; flex-direction: column; gap: 0.6rem; align-items: center; justify-content: center; height: 100%; padding: 2rem; text-align: center; color: var(--vscode-descriptionforeground, var(--vscode-editor-foreground, #8a8a8a)); background: var(--vscode-editor-background, #1e1e1e);",
            div {
                style: "font-size: 0.95em; font-weight: 600; color: var(--vscode-editor-foreground, #d4d4d4);",
                "{title}"
            }
            div {
                style: "font-size: 0.85em; max-width: 36rem; line-height: 1.45;",
                "{body}"
            }
            if let Some(d) = detail {
                code {
                    style: "font-size: 0.8em; padding: 0.25rem 0.5rem; background: var(--vscode-textcodeblock-background, var(--vscode-panel-background, #2a2a2a)); border-radius: 0.25rem;",
                    "{d}"
                }
            }
        }
    }
}

/// Per-repo terminal. The `key` on this in `CompanionClaudeTerminal`
/// guarantees a fresh mount — and a fresh xterm + bridge — when the
/// user switches between repos.
#[component]
fn ClaudeRepoTerminal(repo_path: PathBuf, project_name: String) -> Element {
    let host_seq: u64 = use_hook(|| {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        SEQ.fetch_add(1, Ordering::Relaxed)
    });
    let host_id = format!("operon-claude-terminal-{host_seq}");

    let claude_bin = resolve_claude_bin();
    let cwd_label = repo_path.display().to_string();
    let bin_label = claude_bin.display().to_string();

    // M4b.4: pick up the in-tree MCP bridge if it started cleanly at
    // app boot (`provide_local_state` in desktop.rs registers the
    // context). Snapshot the sock+token into a plain `BridgeEnv` so
    // the spawn closure doesn't need to hold the Arc<BridgeRuntime>.
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    let bridge_env: Option<BridgeEnv> = try_consume_context::<
        crate::local_mode::bridge_runtime::BridgeContext,
    >()
    .map(|ctx| BridgeEnv {
        sock_path: ctx.0.sock_path.clone(),
        token: ctx.0.token.clone(),
        mcp_config_path: ctx.0.mcp_config_path.clone(),
    });
    #[cfg(not(all(unix, not(target_arch = "wasm32"))))]
    let bridge_env: Option<BridgeEnv> = None;

    // M4d.2: drag-drop support. The explorer's note rows are the
    // drag source — `ondragstart` writes `DragKind::Note(uuid)` into
    // `DragSession`. We don't need the HTML5 dataTransfer payload:
    // the in-process signal is the source of truth.
    let drag_session_for_drop =
        try_consume_context::<crate::local_mode::ui::DragSession>().map(|s| s.0);
    let drop_note_repo: Option<Arc<dyn operon_store::repos::LocalNoteRepository>> =
        try_consume_context::<crate::local_mode::desktop::LocalNoteRepo>().map(|c| c.0);
    let drop_project_repo: Option<Arc<dyn operon_store::repos::LocalProjectRepository>> =
        try_consume_context::<crate::local_mode::desktop::LocalProjectRepo>().map(|c| c.0);

    // Try to start (or attach to) the per-repo claude session
    // synchronously on first mount. We do this BEFORE the xterm
    // bridge so the user sees the actual spawn error in plain UI
    // text — not buried behind an `await import(xterm.js)` that
    // might also be failing for offline/CDN reasons. Cached in
    // `use_hook` so a re-render doesn't re-spawn.
    let session_result: Result<Arc<ClaudeSession>, String> = use_hook({
        let repo_path = repo_path.clone();
        let claude_bin = claude_bin.clone();
        let bridge_env = bridge_env.clone();
        move || {
            tracing::info!(
                target: "operon::companion_terminal",
                repo_path = %repo_path.display(),
                claude_bin = %claude_bin.display(),
                bridge = bridge_env.is_some(),
                "ClaudeRepoTerminal mounted; resolving session"
            );
            let r = get_or_create_session(&repo_path, &claude_bin, bridge_env.as_ref());
            match &r {
                Ok(_) => tracing::info!(
                    target: "operon::companion_terminal",
                    repo_path = %repo_path.display(),
                    "session ready"
                ),
                Err(e) => tracing::warn!(
                    target: "operon::companion_terminal",
                    repo_path = %repo_path.display(),
                    claude_bin = %claude_bin.display(),
                    "session spawn failed: {e}"
                ),
            }
            r
        }
    });

    let session = match session_result {
        Ok(s) => s,
        Err(e) => {
            return rsx! {
                EmptyState {
                    title: "Could not start Claude Code",
                    body: "Spawning the `claude` binary failed. Make sure it's installed and on your `PATH`, or set the `OPERON_CLAUDE_BIN` env var to point at it.",
                    detail: format!("{} → {}\n{}", bin_label, cwd_label, e),
                }
            };
        }
    };

    // M4d.1: drain `PENDING_TERMINAL_INJECTION` — the toolbar's
    // "Send to Claude" button (and future drag/paste handlers) push
    // mention tokens here when the companion is in terminal mode.
    // We type them into the PTY (synchronously, same `Mutex<Writer>`
    // path the xterm `data` messages use) and reset the signal.
    // Re-fires whenever the signal changes; the snapshot+clear in
    // one effect run avoids re-injecting the same value.
    {
        use std::io::Write;
        let session = session.clone();
        use_effect(move || {
            let pending = crate::shell::companion_state::PENDING_TERMINAL_INJECTION
                .read()
                .clone();
            if let Some(text) = pending {
                if let Ok(mut w) = session.writer.lock() {
                    let _ = w.write_all(text.as_bytes());
                    let _ = w.flush();
                }
                *crate::shell::companion_state::PENDING_TERMINAL_INJECTION.write() = None;
            }
        });
    }

    // Theme palette for xterm. xterm renders to its own canvas/DOM
    // and doesn't read CSS variables, so we pass concrete hex
    // colours into its constructor. Pick a light or dark palette
    // based on the active app theme. If the theme flips at runtime
    // we push a `setTheme` message through the bridge below.
    let theme_signal = try_consume_context::<crate::theme::ThemeSignal>();
    let theme_kind = theme_signal
        .as_ref()
        .map(|t| t.read().kind)
        .unwrap_or(crate::theme::ThemeKind::Dark);
    let xterm_theme_json = xterm_theme_json_for(theme_kind);

    // Bridge: eval + recv loop in one `use_future`. Earlier revs
    // tried `use_hook` with a hand-written `spawn` and the use_hook +
    // use_future split. Both produced an empty terminal because the
    // Rust→JS sends never reached the JS recv loop in this mount —
    // we never saw the `[rs] bridge subscribed` diag in the floating
    // overlay even though the JS bootstrap completed. Folding the
    // eval into the same future that runs the bridge keeps the eval
    // handle owned by the polled future, avoids any cross-hook
    // ownership games, and matches the "long-lived future drives an
    // eval channel" shape that the Monaco bridge proved works.
    // Runtime theme flip — watch `ThemeSignal` and push the new
    // xterm palette directly via `document::eval` (bypasses the
    // long-lived bridge so we don't have to thread it through). On
    // first render `previous` matches the just-built theme so we
    // skip the redundant push.
    if let Some(sig) = theme_signal {
        let host_id_for_theme = host_id.clone();
        let mut previous = use_signal(|| theme_kind);
        use_effect(move || {
            let current = sig.read().kind;
            if *previous.peek() == current {
                return;
            }
            previous.set(current);
            let theme_json = xterm_theme_json_for(current);
            let host_id_json =
                serde_json::to_string(&host_id_for_theme).unwrap_or_default();
            let script = format!(
                "(function() {{ const e = (window.__operon_terms || {{}})[{host_id_json}]; if (e && e.term) {{ try {{ e.term.options.theme = {theme_json}; }} catch (_) {{}} }} }})();"
            );
            let _ = document::eval(&script);
        });
    }

    let host_id_for_future = host_id.clone();
    let session_for_future = session.clone();
    let xterm_theme_for_future = xterm_theme_json.clone();
    use_future(move || {
        let host_id = host_id_for_future.clone();
        let session = session_for_future.clone();
        let xterm_theme_json = xterm_theme_for_future.clone();
        async move {
        let host_id_js =
            serde_json::to_string(&host_id).unwrap_or_else(|_| "\"\"".into());
        // JSON-encode the asset bodies so they survive embedding in
        // the format! template (escapes quotes, backslashes, newlines).
        // The browser will set them as `textContent` on injected
        // `<script>` / `<style>` tags — appending the script tag
        // synchronously evaluates the UMD wrapper, populating
        // `window.Terminal` and `window.FitAddon`.
        let xterm_js_json = serde_json::to_string(XTERM_JS).unwrap_or_default();
        let xterm_css_json = serde_json::to_string(XTERM_CSS).unwrap_or_default();
        let xterm_fit_js_json =
            serde_json::to_string(XTERM_FIT_JS).unwrap_or_default();
        // CRITICAL: NO outer `(async function(){{ ... }})();` IIFE
        // here. The dioxus-desktop eval wrapper already runs our
        // body via `new AsyncFunction("dioxus", <script>)(dioxus)`
        // and calls `dioxus.close()` (which nulls
        // `window.__msg_queues[id]`) as soon as that outer AsyncFunction
        // resolves. If we wrap our body in an IIFE the AsyncFunction
        // body becomes a one-statement expression that resolves
        // immediately, dioxus.close() fires, and every subsequent
        // Rust→JS `handle.send` silently no-ops because
        // `window.getQuery(id)` returns null. Keeping the `await`s at
        // the top level forces the outer AsyncFunction's promise to
        // stay pending until our loop actually exits.
        let script = format!(
            r##"
                const hostId = {host_id_js};
                const showError = (msg) => {{
                    try {{ dioxus.send({{type: "error", message: msg}}); }} catch (_) {{}}
                    try {{
                        const t = document.getElementById(hostId);
                        if (t) {{
                            t.innerHTML = '';
                            const box = document.createElement("div");
                            box.style.cssText = "color:#f88;padding:1rem;font-family:ui-monospace,monospace;font-size:11px;white-space:pre-wrap;";
                            box.textContent = "Claude Code terminal failed to mount:\n\n" + msg;
                            t.appendChild(box);
                        }}
                    }} catch (_) {{}}
                }};
                let target = document.getElementById(hostId);
                let attempts = 0;
                while (!target && attempts < 60) {{
                    await new Promise(r => setTimeout(r, 33));
                    target = document.getElementById(hostId);
                    attempts++;
                }}
                if (!target) {{
                    try {{ dioxus.send({{type: "error", message: "host element not found id=" + hostId}}); }} catch (_) {{}}
                    return;
                }}
                // Inject the vendored xterm bundles into <head> on first
                // use. Each script tag synchronously evaluates its UMD
                // body, populating `window.Terminal` /
                // `window.FitAddon.FitAddon`. Idempotent — re-mounts of
                // the terminal reuse the globals.
                try {{
                    if (!document.getElementById("operon-xterm-css")) {{
                        const style = document.createElement("style");
                        style.id = "operon-xterm-css";
                        style.textContent = {xterm_css_json};
                        document.head.appendChild(style);
                    }}
                    if (!window.Terminal) {{
                        const s = document.createElement("script");
                        s.textContent = {xterm_js_json};
                        document.head.appendChild(s);
                    }}
                    if (!window.FitAddon) {{
                        const s = document.createElement("script");
                        s.textContent = {xterm_fit_js_json};
                        document.head.appendChild(s);
                    }}
                }} catch (e) {{
                    showError("xterm bootstrap failed: " + (e && e.message || e));
                    return;
                }}
                const FitCls = window.FitAddon && (window.FitAddon.FitAddon || window.FitAddon);
                if (typeof window.Terminal !== "function" || typeof FitCls !== "function") {{
                    showError("xterm constructors missing after inject");
                    return;
                }}
                let term;
                try {{
                    term = new window.Terminal({{
                        fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
                        fontSize: 13,
                        cursorBlink: true,
                        convertEol: false,
                        scrollback: 5000,
                        allowProposedApi: true,
                        theme: {xterm_theme_json},
                    }});
                }} catch (e) {{
                    showError("new Terminal() threw: " + (e && e.message || e));
                    return;
                }}
                let fit;
                try {{
                    fit = new FitCls();
                    term.loadAddon(fit);
                }} catch (e) {{
                    showError("FitAddon attach threw: " + (e && e.message || e));
                    return;
                }}
                target.innerHTML = "";
                try {{
                    term.open(target);
                }} catch (e) {{
                    showError("term.open() threw: " + (e && e.message || e));
                    return;
                }}
                try {{ fit.fit(); }} catch (_) {{}}
                window.__operon_terms = window.__operon_terms || {{}};
                window.__operon_terms[hostId] = {{ term, fit }};
                const sendSafe = (msg) => {{
                    try {{ dioxus.send(msg); }} catch (_) {{}}
                }};
                term.onData(d => sendSafe({{type: "data", data: d}}));
                term.onResize(({{cols, rows}}) => sendSafe({{type: "resize", cols, rows}}));
                // M4d.4: detect when the user types `@` so we can
                // pop a note-picker. We use
                // `attachCustomKeyEventHandler` which fires BEFORE
                // xterm's internal handling — returning `true` keeps
                // xterm processing the event (so `@` still types into
                // the PTY). The picker is fire-and-forget: we notify
                // Rust and let the picker UI take it from there.
                //
                // Cursor anchoring: best-effort compute of the
                // cursor's pixel position relative to the outer pane
                // (the `.operon-companion-terminal` ancestor), so the
                // picker anchors below the `@` the user just typed.
                // Cell dimensions are derived from
                // `term.element.getBoundingClientRect()` / cols/rows
                // — this is approximate (assumes no per-cell padding)
                // but lands within a character cell in practice.
                // On any error we omit x/y; the picker falls back to
                // its docked default position.
                try {{
                    term.attachCustomKeyEventHandler((ev) => {{
                        if (ev.type === 'keydown' && ev.key === '@') {{
                            let pos = null;
                            try {{
                                const buf = term.buffer.active;
                                const termRect = term.element.getBoundingClientRect();
                                const pane = target.closest('.operon-companion-terminal');
                                const paneRect = pane ? pane.getBoundingClientRect() : termRect;
                                if (termRect.width > 0 && termRect.height > 0 && term.cols > 0 && term.rows > 0) {{
                                    const cellW = termRect.width / term.cols;
                                    const cellH = termRect.height / term.rows;
                                    // Anchor just below the cursor row + one cell
                                    // past the column the `@` lands in. The `@`
                                    // hasn't been written yet at keydown time so
                                    // cursorX still points at where it will appear.
                                    const x = (termRect.left - paneRect.left) + Math.max(0, (buf.cursorX + 1) * cellW);
                                    const y = (termRect.top - paneRect.top) + Math.max(0, (buf.cursorY + 1) * cellH);
                                    pos = [x, y];
                                }}
                            }} catch (_) {{}}
                            const msg = {{type: 'at_keypress'}};
                            if (pos) {{ msg.x = pos[0]; msg.y = pos[1]; }}
                            sendSafe(msg);
                        }}
                        return true;
                    }});
                }} catch (_) {{}}
                // M4d.3: paste interception. xterm consumes pastes
                // via its own internal listener and forwards them
                // to `onData` verbatim. We attach BEFORE that with
                // `useCapture: true` so we can transform note-id
                // pastes into a `@[note:<uuid>](note:<uuid>) ` shape
                // the system prompt teaches claude to fetch via
                // `mcp__operon_notes__get_note`. Any paste that doesn't
                // contain a UUID anywhere falls through to xterm's
                // default handling unchanged.
                const uuidRe = /\b[0-9a-f]{{8}}-[0-9a-f]{{4}}-[0-9a-f]{{4}}-[0-9a-f]{{4}}-[0-9a-f]{{12}}\b/i;
                target.addEventListener('paste', (e) => {{
                    let text = '';
                    try {{
                        text = (e.clipboardData && e.clipboardData.getData('text/plain')) || '';
                    }} catch (_) {{
                        return;
                    }}
                    if (!text) return;
                    const m = text.match(uuidRe);
                    if (!m) return; // not a note id — let xterm paste it raw
                    e.preventDefault();
                    e.stopPropagation();
                    const uuid = m[0].toLowerCase();
                    // Placeholder title (`note:<uuid>`) is what the
                    // user sees in their prompt; claude resolves the
                    // real title via get_note on its next turn.
                    const token = `@[note:${{uuid}}](note:${{uuid}}) `;
                    sendSafe({{type: "data", data: token}});
                }}, true);
                const pushSize = () => {{
                    try {{ fit.fit(); }} catch (_) {{}}
                    sendSafe({{type: "resize", cols: term.cols, rows: term.rows}});
                }};
                pushSize();
                if (window.ResizeObserver) {{
                    const ro = new ResizeObserver(() => pushSize());
                    ro.observe(target);
                    window.__operon_terms[hostId].ro = ro;
                }}
                window.addEventListener("resize", pushSize);
                sendSafe({{type: "ready", cols: term.cols, rows: term.rows}});
                while (true) {{
                    let incoming;
                    try {{
                        incoming = await dioxus.recv();
                    }} catch (_) {{
                        break;
                    }}
                    if (!incoming || typeof incoming !== "object") continue;
                    if (incoming.type === "out" && typeof incoming.b64 === "string") {{
                        const bin = atob(incoming.b64);
                        const bytes = new Uint8Array(bin.length);
                        for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
                        term.write(bytes);
                    }} else if (incoming.type === "reset") {{
                        term.reset();
                    }} else if (incoming.type === "theme" && incoming.theme) {{
                        try {{ term.options.theme = incoming.theme; }} catch (_) {{}}
                    }} else if (incoming.type === "shutdown") {{
                        try {{ term.dispose(); }} catch (_) {{}}
                        const entry = window.__operon_terms[hostId];
                        if (entry && entry.ro) {{
                            try {{ entry.ro.disconnect(); }} catch (_) {{}}
                        }}
                        delete window.__operon_terms[hostId];
                        return;
                    }} else if (incoming.type === "exit" && typeof incoming.message === "string") {{
                        term.write("\r\n\x1b[2m" + incoming.message + "\x1b[0m\r\n");
                    }}
                }}
            "##,
            host_id_js = host_id_js,
            xterm_theme_json = xterm_theme_json,
        );
        let mut handle = document::eval(&script);
        {
            tracing::debug!(
                target: "operon::companion_terminal",
                host_id = %host_id,
                "bridge task started; waiting for JS ready handshake"
            );
            // CRITICAL: dioxus-desktop's eval bridge silently drops
            // Rust→JS messages that fire BEFORE the JS bootstrap
            // reaches `await dioxus.recv()` (see comment block at
            // shell/editor_host.rs MonacoChannel for the same hazard
            // on the Monaco mount). The JS side sends `{type:"ready"}`
            // once it's looping on recv; we drain handle.recv() until
            // we see it, THEN start forwarding output.
            //
            // Subscribe to broadcast BEFORE the await so any PTY
            // chunks claude produces during the wait land in the
            // channel and are replayed to us once the loop spins up
            // — they were already going to scrollback anyway.
            let mut out_rx = session.out_tx.subscribe();
            loop {
                match handle.recv::<serde_json::Value>().await {
                    Ok(v) => {
                        let kind = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
                        tracing::debug!(
                            target: "operon::companion_terminal",
                            kind,
                            "pre-ready msg from JS"
                        );
                        if kind == "ready" {
                            break;
                        }
                    }
                    Err(_) => {
                        tracing::warn!(
                            target: "operon::companion_terminal",
                            "handle.recv errored before ready handshake"
                        );
                        return;
                    }
                }
            }
            tracing::debug!(
                target: "operon::companion_terminal",
                "JS ready; starting forward loop"
            );

            // Wait for the JS side to come up before painting.
            // The script always sends a `ready` once xterm is mounted;
            // we drive the loop normally and the first `resize` from
            // the bridge will arrive in the same loop.
            //
            // Replay scrollback so the user sees the existing screen
            // state before live bytes start flowing.
            {
                let sb = session.scrollback.lock().unwrap();
                if !sb.is_empty() {
                    let bytes: Vec<u8> = sb.iter().copied().collect();
                    let b64 = B64.encode(&bytes);
                    let _ = handle.send(serde_json::json!({
                        "type": "out",
                        "b64": b64,
                    }));
                }
            }

            loop {
                tokio::select! {
                    biased;
                    msg = out_rx.recv() => {
                        match msg {
                            Ok(bytes) => {
                                let b64 = B64.encode(&bytes);
                                if handle
                                    .send(serde_json::json!({"type":"out","b64":b64}))
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => {
                                // We fell behind. Reset the screen and
                                // replay scrollback so the user isn't
                                // staring at a half-painted screen.
                                let _ = handle.send(serde_json::json!({"type":"reset"}));
                                let sb = session.scrollback.lock().unwrap();
                                if !sb.is_empty() {
                                    let bytes: Vec<u8> = sb.iter().copied().collect();
                                    let b64 = B64.encode(&bytes);
                                    let _ = handle.send(serde_json::json!({
                                        "type": "out",
                                        "b64": b64,
                                    }));
                                }
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                // claude exited. The cleanup watcher
                                // already removed the session; tell
                                // the user.
                                let _ = handle.send(serde_json::json!({
                                    "type": "exit",
                                    "message": "claude session ended (exit detected). Toggle Settings → Companion pane → Chat → Claude Code to restart.",
                                }));
                                break;
                            }
                        }
                    }
                    incoming = handle.recv::<serde_json::Value>() => {
                        let v = match incoming {
                            Ok(v) => v,
                            Err(_) => break, // component unmounted
                        };
                        let kind = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
                        match kind {
                            "data" => {
                                if let Some(s) = v.get("data").and_then(|x| x.as_str()) {
                                    // Sync `Mutex::lock` on purpose — see
                                    // the doc comment on `ClaudeSession::
                                    // writer`. Awaiting a tokio AsyncMutex
                                    // here previously hopped the future
                                    // onto a tokio worker without the
                                    // Dioxus runtime guard, panicking the
                                    // very next `handle.send`.
                                    let bytes = s.as_bytes();
                                    if let Ok(mut w) = session.writer.lock() {
                                        let _ = w.write_all(bytes);
                                        let _ = w.flush();
                                    }
                                }
                            }
                            "resize" => {
                                let cols =
                                    v.get("cols").and_then(|x| x.as_u64()).unwrap_or(80) as u16;
                                let rows =
                                    v.get("rows").and_then(|x| x.as_u64()).unwrap_or(24) as u16;
                                if let Ok(m) = session.master.lock() {
                                    let _ = m.resize(PtySize {
                                        rows,
                                        cols,
                                        pixel_width: 0,
                                        pixel_height: 0,
                                    });
                                }
                            }
                            "at_keypress" => {
                                // M4d.4: user typed `@` at the
                                // claude prompt. Open the floating
                                // note-picker. Reading + writing the
                                // GlobalSignal here is safe because
                                // this recv loop runs as a Dioxus
                                // `spawn` task — same runtime guard
                                // GlobalSignal writes need.
                                //
                                // Cursor anchoring (M4d.4 polish):
                                // if the JS side included `x`/`y`,
                                // stash them so the picker positions
                                // dynamically. Both fields missing
                                // → picker falls back to its docked
                                // default.
                                let xy = match (
                                    v.get("x").and_then(|n| n.as_f64()),
                                    v.get("y").and_then(|n| n.as_f64()),
                                ) {
                                    (Some(x), Some(y)) => Some((x, y)),
                                    _ => None,
                                };
                                *crate::shell::companion_state::MENTION_PICKER_POS.write() = xy;
                                *crate::shell::companion_state::MENTION_PICKER_OPEN.write() = true;
                            }
                            _ => {}
                        }
                    }
                }
            }

            // Component is unmounting OR the session ended. Tell the
            // JS side to dispose its xterm; the session itself stays
            // in the registry (or has already been removed by the
            // cleanup watcher if the child exited).
            let _ = handle.send(serde_json::json!({"type":"shutdown"}));
        }
        }
    });

    // Match the container's background to xterm's so the padding
    // gap doesn't show as a contrasting stripe. The header strip
    // pulls from the app's own theme tokens so it auto-flips with
    // the rest of the chrome.
    let (term_bg, term_fg) = match theme_kind {
        crate::theme::ThemeKind::Light => ("#ffffff", "#3b3b3b"),
        _ => ("#1e1e1e", "#d4d4d4"),
    };

    rsx! {
        div {
            class: "operon-companion-terminal",
            "data-testid": "companion-claude-terminal",
            "data-repo-path": "{cwd_label}",
            "data-theme": "{theme_kind.data_attr()}",
            style: "display: flex; flex-direction: column; height: 100%; min-height: 0; background: {term_bg}; color: {term_fg}; position: relative;",
            // M4d.2: accept dragged notes from the explorer. Same
            // signal protocol as the chat-mode drop handler (see
            // `companion_chat.rs::ondrop`). prevent_default on
            // dragover is required to mark this surface as a drop
            // target; without it the browser refuses the drop.
            ondragover: move |evt| evt.prevent_default(),
            ondrop: {
                let drop_note_repo = drop_note_repo.clone();
                let drop_project_repo = drop_project_repo.clone();
                let drag_session_opt = drag_session_for_drop;
                move |evt: Event<DragData>| {
                    let Some(mut drag_session) = drag_session_opt else {
                        return;
                    };
                    let kind = *drag_session.peek();
                    let note_id = match kind {
                        Some(crate::local_mode::ui::DragKind::Note(id)) => id,
                        _ => return,
                    };
                    evt.prevent_default();
                    let Some(note_repo) = drop_note_repo.as_ref() else {
                        drag_session.set(None);
                        return;
                    };
                    let title = crate::local_mode::note_lookup::lookup_note_title(
                        note_repo,
                        drop_project_repo.as_ref(),
                        note_id,
                    )
                    .unwrap_or_else(|| note_id.to_string());
                    // Mirror M4d.1's toolbar wiring: token + trailing
                    // space so the cursor lands past the mention. The
                    // PTY drain effect (above) types it at the
                    // prompt; the system-prompt hint tells claude to
                    // call `get_note(<uuid>)` on seeing this shape.
                    let token = format!("@[{}](note:{}) ", title, note_id);
                    *crate::shell::companion_state::PENDING_TERMINAL_INJECTION.write() =
                        Some(token);
                    drag_session.set(None);
                }
            },
            div {
                style: "flex: 0 0 auto; padding: 4px 8px; font-size: 11px; color: var(--vscode-descriptionforeground, var(--vscode-editor-foreground, #9d9d9d)); background: var(--vscode-panelheader-background, var(--vscode-panel-background, #252525)); border-bottom: 1px solid var(--vscode-panel-border, #333); display: flex; gap: 12px; align-items: center;",
                span { style: "opacity: 0.85;", "claude · {project_name}" }
                span { style: "opacity: 0.6;", "{bin_label}" }
                span { style: "margin-left: auto; opacity: 0.6;", "cwd: {cwd_label}" }
            }
            div {
                id: "{host_id}",
                "data-testid": "companion-claude-terminal-host",
                style: "flex: 1 1 auto; min-height: 0; padding: 6px 8px;",
            }
            // M4d.4: floating note picker. Self-gates on
            // `MENTION_PICKER_OPEN` — renders nothing when closed,
            // so this is essentially a no-op mount the rest of the
            // time.
            crate::shell::terminal_mention_picker::TerminalMentionPicker {}
        }
    }
}
