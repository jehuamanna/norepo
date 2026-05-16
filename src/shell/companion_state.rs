//! Cross-cutting state shared between the explorer / project list, the
//! companion-pane chat, and (in M2) skill / workflow plugins. The signals
//! here are provided in `local_mode::desktop` and consumed by various
//! components; this module is the one place the newtypes live so both
//! sides can import them without circular module deps.

use dioxus::prelude::{ReadableExt, Signal};
use dioxus::signals::GlobalSignal;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

pub use operon_store::repos::{
    ChatMessage, ChatMessageKind, ChatMessageRepository, ChatScope, ChatSessionRepository,
};

/// Absolute path to the active project's bound git repository. M1 single-
/// session compat: the companion's `claude` subprocess uses this as `cwd`
/// when the user has a project selected. `None` means no project / no repo
/// bound. M1.5 layers on top by treating this as one of two cwds (project
/// vs vault) selected by the active `ChatScope`.
#[derive(Clone, Copy)]
pub struct ActiveRepoPath(pub Signal<Option<PathBuf>>);

/// The chat session currently visible in the companion pane. `None` when
/// no session is selected (e.g., the active scope has no sessions yet —
/// the rail shows an empty state with a `+ New chat` prompt).
#[derive(Clone, Copy)]
pub struct ActiveChatSession(pub Signal<Option<Uuid>>);

/// The active scope tab in the companion's left rail. Defaults to
/// `Project(<selected>)` when a project is selected, else `Vault`.
/// Users can flip it manually via the scope tabs regardless of project
/// selection.
#[derive(Clone, Copy)]
pub struct ActiveChatScope(pub Signal<ChatScope>);

/// SQLite-backed `ChatSessionRepository` provided to the companion + rail.
#[derive(Clone)]
pub struct ChatSessionRepo(pub Arc<dyn ChatSessionRepository>);

/// SQLite-backed `ChatMessageRepository` for transcript persistence + replay.
#[derive(Clone)]
pub struct ChatMessageRepo(pub Arc<dyn ChatMessageRepository>);

/// Bumped by anything that mutates `chat_session` rows (create / rename /
/// delete / touch). The session rail's `use_memo` re-runs on changes so
/// the list refreshes without a full remount.
#[derive(Clone, Copy)]
pub struct ChatSessionVersion(pub Signal<u64>);

/// Bumped by background drainers (the artifact runner, the workflow
/// cascade) every time they `repo.append` to `chat_message`. The
/// companion's transcript-load `use_effect` watches this so a viewer
/// sees streaming events from a run that another component is
/// driving — without it, the load effect only fires on session
/// change and the transcript stays frozen at whatever was persisted
/// at switch time. Regular companion chats DON'T bump it (their
/// drainer updates the in-memory transcript directly).
///
/// **Why a `GlobalSignal` and not a context-provided `Signal`:** the
/// runner spawns its async work via `spawn_forever` which attaches
/// to the virtual root scope (ScopeId 0, "app") — that scope is an
/// ancestor of every user-defined component, not a descendant. A
/// `Signal` created in any user component (App, Workspace, …) is
/// owned by that component's scope; writes from outside its subtree
/// are silently dropped and Dioxus emits a `__copy_value_hoisted`
/// warning. `GlobalSignal` is application-wide, owned by no
/// component's scope, and safe to read/write from anywhere.
pub static CHAT_MESSAGE_VERSION: GlobalSignal<u64> = Signal::global(|| 0);

/// Application-wide version counter for `local_note` mutations. The
/// explorer subscribes to this so a change anywhere in the app
/// (artifact runner / cascade / workflow run / image attach / paste
/// import) re-fetches the project's note tree.
///
/// **Why a `GlobalSignal` and not a context-provided `Signal`:** same
/// reason as `CHAT_MESSAGE_VERSION` — the artifact cascade uses
/// `dioxus::core::spawn_forever` to detach work from the click
/// handler's scope, which means the spawned task runs in the virtual
/// root scope ("app", ScopeId 0). Writes from there to a
/// `Workspace`-scope `Signal<u64>` are silently dropped and Dioxus
/// emits a `__copy_value_hoisted` warning. The `GlobalSignal` is
/// application-wide, safe to write from any scope.
pub static LOCAL_NOTE_VERSION: GlobalSignal<u64> = Signal::global(|| 0);

/// Bumped whenever any note's display title changes. Renderers that
/// resolve a note UUID to its current title — primarily the user-message
/// mention chips in the companion transcript — read this signal through
/// the `NoteTitleResolver` callback so Dioxus re-runs them on rename.
/// Cheap: a single counter increment per rename, no payload.
pub static NOTE_TITLE_VERSION: GlobalSignal<u64> = Signal::global(|| 0);

/// Bumped by the window-level Ctrl+S capture listener (installed in
/// `shell::install_global_shortcuts`) so the keypress reaches the
/// save flow even when Monaco / a focused input would otherwise
/// swallow it. The Shell component subscribes to this via
/// `use_effect` and dispatches the active tab through the
/// already-installed `LocalSaveAction` callback. Single counter
/// (saturating add) is enough — the effect cares about transitions,
/// not values.
pub static SAVE_REQUEST_TICK: GlobalSignal<u64> = Signal::global(|| 0);

/// Bumped by `spawn_cascade` when Play is hit on an artifact so the
/// companion panel un-collapses (if the user had it tucked away)
/// before the cascade's session starts streaming thinking / tool_use
/// rows. `CompanionArea` subscribes via `use_effect`, tracks the
/// last-seen tick locally, and forces `companion_collapsed = false`
/// on each transition. `GlobalSignal` over context-bound signal for
/// the same reason as `CHAT_MESSAGE_VERSION` — `spawn_cascade` runs
/// off the virtual root scope and would silently drop writes to a
/// hook-bound signal.
pub static EXPAND_COMPANION_TICK: GlobalSignal<u64> = Signal::global(|| 0);

/// Bumped by the per-project Claude settings picker every time it writes
/// to `local_project.default_model` / `default_permission_mode`. The
/// chat-header picker's `picker_persisted` memo subscribes so the
/// "Inherit (X)" label refreshes when the project default changes
/// underneath an open chat. Application-wide because the two pickers
/// live in different scope trees (chat header vs project settings).
pub static PROJECT_SETTINGS_VERSION: GlobalSignal<u64> = Signal::global(|| 0);

/// Bumped by the app-settings Claude picker on writes to
/// `local_app_settings`' `claude.default_model` /
/// `claude.default_permission_mode`. Same purpose as
/// `PROJECT_SETTINGS_VERSION` but for the bottom tier — when the global
/// default changes, any open chat with a NULL chat row and (for Vault
/// scope) no project row needs its "Inherit (X)" label to refresh.
pub static GLOBAL_SETTINGS_VERSION: GlobalSignal<u64> = Signal::global(|| 0);

/// Bumped by the global Settings panel's "Companion pane" toggle when
/// the user flips between the rich Operon chat and the raw Claude Code
/// terminal. `CompanionArea` subscribes to this so the surface swaps
/// in-place without restarting the app. `GlobalSignal` so the toggle
/// (mounted inside the modal scope) and the consumer (mounted in the
/// workspace scope) can share the value across scope-tree boundaries.
pub static COMPANION_MODE_VERSION: GlobalSignal<u64> = Signal::global(|| 0);

/// User's app-wide cascade step-mode preference. Toggled via the
/// View menu's `cascade.toggleStepMode` command. Read by
/// `workflow::state::effective_step_mode` after the per-graph
/// `view_state.step_mode` check, before the heuristic.
///
/// `None`         → no override; per-workflow `view_state.step_mode`
///                   and the heuristic apply as before.
/// `Some(true)`   → step-mode ON globally (cascade pauses after every
///                   skill firing — granular debugging).
/// `Some(false)`  → step-mode OFF globally (cascade level-batches
///                   `cascade_stop` pauses; all sibling artifacts at
///                   a level get processed in one Play).
///
/// Per-workflow `view_state.step_mode` overrides this signal — set
/// it on a specific cascade workflow note to opt that one cascade
/// out of the global preference.
///
/// `GlobalSignal` and not a context-provided `Signal` for the same
/// reason as `CASCADE_STATE` etc. — read from the cascade's
/// `spawn_forever` task, which lives in the virtual root scope.
/// Resets to `None` on app restart (no persistence in v1).
pub static CASCADE_STEP_MODE_OVERRIDE: GlobalSignal<Option<bool>> =
    Signal::global(|| None);

/// State of the most recent artifact-skill run for a given source
/// artifact. The artifact view reads this to render its inline
/// status pill (`Running…` / `Created N artifact(s)` / `Run failed:
/// …`); the picker writes `Running` synchronously when the user
/// clicks a skill, and the runner's `spawn_forever` Result handler
/// writes `Done` / `Failed` after Claude finishes.
///
/// Why a `GlobalSignal<HashMap<Uuid, _>>` and not a per-artifact
/// `Signal`: the picker's `Running` write happens in the click
/// handler (component scope, fine), but the spawn_forever's
/// Done / Failed write happens at the virtual root scope. Writes
/// from there to a component-scoped `Signal` get the
/// `__copy_value_hoisted` warning ("may cause writes to fail"). The
/// HashMap-keyed `GlobalSignal` sidesteps that — it's app-wide and
/// safe to write from any scope.
#[derive(Clone, Debug, PartialEq)]
pub enum ArtifactRunState {
    Running,
    Done { artifact_count: usize },
    Failed { reason: String },
}

pub static ARTIFACT_RUN_STATE: GlobalSignal<HashMap<Uuid, ArtifactRunState>> =
    Signal::global(HashMap::new);

/// Project-level autonomous-cascade state, keyed on the *root*
/// artifact id (the Requirements / Approved seed the user clicked Play
/// on). Distinct from `ARTIFACT_RUN_STATE` (per-skill-run): cascades
/// span many skill runs across the artifact tree, so the Play button
/// needs its own status surface to know whether to render ▶ or ⏹.
///
/// The orchestrator in `src/plugins/artifact/cascade.rs` writes to
/// this map; the artifact view subscribes to render the morphing
/// Play button. Multiple cascades (different roots) can be active
/// simultaneously.
#[derive(Clone, Debug, PartialEq)]
pub enum CascadePhase {
    /// Currently executing. `artifact_id` + `skill_id` identify the
    /// in-flight skill run; `level` is BFS depth from the root (0 =
    /// the root itself).
    Running {
        artifact_id: Uuid,
        skill_id: Uuid,
        level: u32,
    },
    /// Cascade hit a checkpoint skill (`cascade_stop: true`) and is
    /// waiting on the user to review + approve the produced
    /// `artifact_id` before continuing. The view treats this like
    /// Completed for spinner-control purposes (no more work in
    /// flight) but renders a distinct "paused at checkpoint" status
    /// so the user knows to act.
    Paused {
        artifact_id: Uuid,
        skill_id: Uuid,
        level: u32,
    },
    Completed {
        artifacts_produced: usize,
    },
    Cancelled,
    Failed {
        reason: String,
    },
}

pub static CASCADE_STATE: GlobalSignal<HashMap<Uuid, CascadePhase>> =
    Signal::global(HashMap::new);

/// Cooperative cancellation tokens for active cascades, keyed on the
/// same root artifact id as `CASCADE_STATE`. The Play button stores a
/// fresh token here when it spawns; clicking ⏹ calls `.cancel()` on
/// it; the cascade orchestrator polls this between skill invocations
/// and exits at the next boundary.
///
/// Stored separately from `CASCADE_STATE` because `CancellationToken`
/// is `Clone`-only (not serializable / not part of phase state).
pub static CASCADE_CANCEL: GlobalSignal<HashMap<Uuid, tokio_util::sync::CancellationToken>> =
    Signal::global(HashMap::new);

/// Set of chat session ids that currently have a cascade run in
/// flight. Each Play click mints a fresh chat session UUID and
/// registers it here for the lifetime of the run; the companion's
/// transcript renderer checks
/// `CASCADE_RUNNING_SESSIONS.read().contains(&active_id)` and shows
/// a persistent "Claude is working…" row when true. Removed in
/// `spawn_cascade`'s terminal arms (Completed / Failed / Cancelled)
/// so the loader clears at exactly the right moment.
///
/// Keyed by chat session id (not artifact id) so two parallel
/// cascades — one per Play click — each get their own indicator
/// in the rail.
pub static CASCADE_RUNNING_SESSIONS: GlobalSignal<HashSet<Uuid>> =
    Signal::global(HashSet::new);

/// Per-chat-session cancellation tokens for in-flight `claude` turns,
/// keyed on `chat_session_id`. Inserted by `run_turn` at the start of
/// every turn; removed in the spawned drainer's terminal arms.
///
/// Two roles:
/// - **"Is this session running?"** — entry presence == in flight. The
///   header's Send button + backend / model / permission pickers gate
///   on `is_session_running(active_session)`; the Stop button is only
///   rendered when an entry exists.
/// - **"Stop this session's run"** — clicking Stop (either the header
///   button on the active tab or the per-row ⏹ in the rail) clones the
///   token out of the map and calls `.cancel()`. The drainer's
///   `tokio::select!` on `ct.cancelled()` (see `stream::drive_stream`)
///   kills the spawned `claude` subprocess and emits a final Error
///   event so `flush_pending_assistant` writes any partial assistant
///   row before the loop exits.
///
/// `GlobalSignal<HashMap<Uuid, _>>` rather than a per-component
/// `Signal`: the drainer is spawned via `spawn` (root-scope task) so
/// writes from there to a `Signal` owned by `CompanionChat`'s scope
/// would emit the `__copy_value_hoisted` warning. Same pattern as
/// `CASCADE_CANCEL` above.
///
/// Sessions are mutually exclusive on *themselves* (a session can't
/// double-fire) but parallel across distinct session ids — clicking
/// Send on chat A while chat B is mid-run is allowed and gives each
/// its own subprocess that the user can terminate independently.
pub static CHAT_RUN_CANCEL: GlobalSignal<HashMap<Uuid, tokio_util::sync::CancellationToken>> =
    Signal::global(HashMap::new);

/// `true` iff `chat_session` currently has a `claude` turn in flight.
/// Cheap read for UI gating (header Send button, rail row indicator,
/// backend picker enable) without exposing the token itself.
pub fn is_session_running(chat_session: Uuid) -> bool {
    CHAT_RUN_CANCEL.read().contains_key(&chat_session)
}

/// Cancel the in-flight turn for `chat_session`, if any. Returns
/// `true` when a token was found and signalled. The drainer's
/// `ct.cancelled()` arm kills the spawned subprocess and the
/// terminal arm clears the map entry.
pub fn cancel_session_run(chat_session: Uuid) -> bool {
    let token = CHAT_RUN_CANCEL.read().get(&chat_session).cloned();
    match token {
        Some(ct) => {
            ct.cancel();
            true
        }
        None => false,
    }
}

/// Per-workflow-note version counter that the cascade graph writer
/// bumps after every successful `flush()` to disk. The workflow
/// canvas subscribes by note id: when its entry changes, it re-loads
/// the body from persistence and re-parses the graph so the user
/// sees newly-produced artifact-snapshot nodes appear live as a
/// cascade runs.
///
/// Keyed by the **workflow note id** (the `Cascade: <root>` note),
/// not the source-artifact id, so two open canvases backed by
/// distinct cascade roots refresh independently.
///
/// We don't piggyback on `LocalNoteVersion` because that bumps for
/// every note write app-wide; per-workflow keying lets idle canvases
/// pay nothing and limits the re-render storm during a busy
/// cascade.
pub static WORKFLOW_GRAPH_VERSION: GlobalSignal<HashMap<Uuid, u64>> =
    Signal::global(HashMap::new);

/// Snapshot of an artifact's body taken when the user enters Revise
/// (Edit) mode from the explorer row's ✎ button. Lets the paired ✕
/// Cancel button revert disk + tab buffer to the pre-Revise state
/// without depending on undo history. Keyed by artifact note id;
/// removed once the user clicks Done or Cancel.
///
/// `GlobalSignal<HashMap<…>>` because the row's onclick spawns its
/// work via `spawn_forever` (attaches to root scope) — same pattern
/// as `ARTIFACT_RUN_STATE` / `CASCADE_STATE` above.
pub static ROW_REVISE_SNAPSHOTS: GlobalSignal<HashMap<Uuid, String>> =
    Signal::global(HashMap::new);

/// Live letter-by-letter streaming buffer for in-progress Claude
/// assistant text, keyed on `chat_session_id`. The runner appends
/// each `Text` event delta to the entry and clears it on flush
/// (when a non-Text event fires or the run completes). The
/// companion renders the entry as a transient streaming block at
/// the end of the transcript — same role as ChatGPT's "typing"
/// effect. Once flushed, the text moves into a regular Assistant
/// `chat_message` row and the map entry is cleared.
///
/// Map (rather than a single `Option<String>`) so multiple sessions
/// streaming concurrently don't trample each other.
pub static INPROGRESS_ASSISTANT: GlobalSignal<HashMap<Uuid, String>> =
    Signal::global(HashMap::new);

/// One-shot inbox the companion's composer subscribes to. When a remote
/// caller (e.g., the skill plugin's Play button) writes `Some(prompt)`,
/// the companion swaps that text into its composer field on the next
/// render and clears the signal. The user reviews + clicks Send.
#[derive(Clone, Copy)]
pub struct CompanionComposerInbox(pub Signal<Option<String>>);

/// Sibling to [`CompanionComposerInbox`] with append-semantics instead
/// of replace. The side-bar's "Send to chat" right-click action writes
/// a `@[<title>](note:<uuid>)` mention token here; the companion's
/// composer effect appends it to the current composer value (with a
/// leading space if non-empty), then resets the signal to `None`.
/// Keeps "send-to-chat" non-destructive over the user's draft.
#[derive(Clone, Copy)]
pub struct CompanionComposerAppend(pub Signal<Option<String>>);

/// M4d.4: open-state for the floating note picker that the terminal
/// pane mounts when the user types `@` at the claude prompt. `true`
/// means render the picker; `false` means hide it. The picker reads
/// from `LocalNoteRepo` + `LocalProjectRepo` (already in context via
/// `provide_local_state`) to enumerate notes; on selection it writes
/// `[Title](note:uuid) ` to `PENDING_TERMINAL_INJECTION` (no leading
/// `@` because the user already typed the `@` that triggered the
/// open).
///
/// `GlobalSignal` for the same reason as the other picker / inbox
/// signals here — written from the terminal pane's recv loop
/// (outside any Dioxus runtime guard until it reaches the picker
/// component) and read by the picker.
pub static MENTION_PICKER_OPEN: GlobalSignal<bool> = Signal::global(|| false);

/// Pixel position the picker should anchor to, relative to the
/// terminal pane's outer container. Set by the xterm bootstrap when
/// it intercepts `@` — JS computes from `term.buffer.active.cursorX/Y`
/// + cell dimensions measured off `term.element.getBoundingClientRect()`.
///
/// `None` means "fall back to the docked default" (top-left of the
/// pane) — used when the JS measurement fails (no `term` yet, or
/// `getBoundingClientRect` returned zero dims during a relayout).
/// The picker reads this and emits inline `style="top:Ypx; left:Xpx"`
/// when present.
pub static MENTION_PICKER_POS: GlobalSignal<Option<(f64, f64)>> =
    Signal::global(|| None);

/// M4d.1: terminal-mode counterpart to `CompanionComposerAppend`.
///
/// When the user clicks "Send to Claude" (or, in future iterations,
/// drags or pastes a note reference) and the companion is in
/// terminal mode, the gesture-handler writes a mention token here.
/// The currently-mounted `ClaudeRepoTerminal` reads the signal on
/// each render, writes the token (plus a space) into its PTY
/// writer, and resets to `None`.
///
/// Why a `GlobalSignal`: the toolbar lives in the main-area tree,
/// while the terminal pane lives in the companion-area tree. A
/// shared context-provided `Signal<Option<String>>` would work too,
/// but `GlobalSignal` matches the pattern the chat-side append
/// signal uses for a similar pub-sub flow.
///
/// Why a separate signal (not "also write to CompanionComposerAppend"):
/// chat-mode's append consumer turns the token into a chip in the
/// composer's attached-notes tray — silent without a tray to mount
/// into. Terminal mode wants the token actually typed at the
/// claude prompt so the user can edit or submit it.
pub static PENDING_TERMINAL_INJECTION: GlobalSignal<Option<String>> =
    Signal::global(|| None);

/// Snapshot of MCP servers + tools as reported by claude's `system/init`
/// event on the most recent turn. The MCP settings panel reads this to
/// drive the active/inactive indicator and the per-server tools list.
///
/// `GlobalSignal` for the same reason as `CHAT_MESSAGE_VERSION`: writes
/// happen from the per-turn drainer task which is detached from any
/// component scope.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpLiveStatus {
    /// MCP servers reported by claude in the most recent `system/init`,
    /// with their connection status (`connected` / `failed` / `needs-auth`
    /// / `unknown`). Empty when no turn has run yet in the current
    /// process.
    pub mcp_servers: Vec<operon_core::agent_event::McpServerStatus>,
    /// Full tool inventory for the same turn. Includes both built-in
    /// tools (`Bash`, `Read`, …) and `mcp__<server>__<tool>` entries —
    /// the panel parses the prefix to bucket tools by server.
    pub tools: Vec<String>,
    /// Operon chat session id that produced the snapshot, so the panel
    /// can show "stale — start a chat to refresh" when the user opens it
    /// from a session that never sent a turn.
    pub session: Option<Uuid>,
}

pub static MCP_LIVE_STATUS: GlobalSignal<McpLiveStatus> = Signal::global(McpLiveStatus::default);

/// Health gate for step processing. Returns `Some(message)` when the
/// most recent `system/init` reported any MCP server in a non-connected
/// state (`failed`, `needs-auth`, `unknown`, …), else `None`.
///
/// Empty roster → returns `None` (no information; the first turn of a
/// fresh process hasn't reported yet, so we can't block — the gate
/// kicks in on the next step once a status snapshot lands).
///
/// Used by every artifact / workflow / cascade entry point to refuse
/// to run any further skill while an MCP server is unhealthy. The
/// status string `connected` is treated as the only healthy state; the
/// server-reported status is surfaced verbatim in the message so the
/// user can tell `failed` from `needs-auth`.
pub fn mcp_health_gate_error() -> Option<String> {
    let snap = MCP_LIVE_STATUS.read();
    if snap.mcp_servers.is_empty() {
        return None;
    }
    let bad: Vec<String> = snap
        .mcp_servers
        .iter()
        .filter(|s| s.status != "connected")
        .map(|s| format!("{} ({})", s.name, s.status))
        .collect();
    if bad.is_empty() {
        None
    } else {
        Some(format!(
            "MCP server(s) not working — refusing to process step: {}",
            bad.join(", ")
        ))
    }
}

/// Per-skill MCP requirement gate. Each requirement is matched
/// (case-insensitively, as a substring) against connected MCP server
/// names AND the full tool inventory (`mcp__<server>__<tool>`) on the
/// most recent `system/init`. Returns `Some(message)` listing the
/// unmet requirements when any are missing, else `None`.
///
/// Matching the tool inventory too means a skill declaring
/// `requires_mcp: figma` is satisfied regardless of whether the user
/// named their server `figma`, `figma-mcp`, or `figma-developer-mcp`
/// — the `mcp__<server>__<tool>` tool name carries the substring
/// either way.
///
/// When the live snapshot is empty or doesn't satisfy a requirement,
/// the gate falls back to the **static** MCP config on disk (parsed
/// from `<cwd>/.mcp.json` for project scope and `~/.claude.json` for
/// user + local scopes). This is what lets a cold cascade run pick up
/// servers declared in `.mcp.json` even before the first chat turn
/// has produced a `system/init` event.
///
/// The gate is unconditional: every entry must be satisfied for the
/// skill to fire. If a skill can run productively without an MCP
/// tool, the skill author should simply omit that entry from
/// `requires_mcp` — not declare it and hope the runtime is lenient.
pub fn mcp_skill_requirements_gate_error(
    requirements: &[String],
    cwd: Option<&std::path::Path>,
) -> Option<String> {
    if requirements.is_empty() {
        return None;
    }
    let snap = MCP_LIVE_STATUS.read();
    let connected_servers: Vec<String> = snap
        .mcp_servers
        .iter()
        .filter(|s| s.status == "connected")
        .map(|s| s.name.to_lowercase())
        .collect();
    let tools_lc: Vec<String> = snap.tools.iter().map(|t| t.to_lowercase()).collect();
    // Read static config on demand and cache for the duration of this
    // call. Cheap (small JSON files) and skipped entirely when the
    // live snapshot already covers all requirements.
    let mut static_names: Option<Vec<String>> = None;
    let missing: Vec<String> = requirements
        .iter()
        .filter(|req| {
            let needle = req.to_lowercase();
            if connected_servers.iter().any(|s| s.contains(&needle))
                || tools_lc.iter().any(|t| t.contains(&needle))
            {
                return false;
            }
            let names = static_names
                .get_or_insert_with(|| static_mcp_server_names(cwd));
            !names.iter().any(|n| n.contains(&needle))
        })
        .cloned()
        .collect();
    if missing.is_empty() {
        None
    } else {
        Some(format!(
            "skill requires MCP server(s) not configured/connected: {} — \
             install the matching MCP server before running this skill",
            missing.join(", ")
        ))
    }
}

/// Read the union of MCP server names declared in static config files:
/// `<cwd>/.mcp.json` (project scope), and `~/.claude.json` for both
/// user-scope (`mcpServers` at root) and local-scope (project-local
/// `mcpServers` keyed by absolute path).
///
/// Returns lowercased names. Missing / malformed files are silently
/// ignored — the gate falls back to a (possibly empty) list rather
/// than failing the run on a config-parse glitch.
pub(crate) fn static_mcp_server_names(cwd: Option<&std::path::Path>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    // 1. Project-scope .mcp.json in cwd.
    if let Some(dir) = cwd {
        let p = dir.join(".mcp.json");
        if let Ok(text) = std::fs::read_to_string(&p) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(servers) = v.get("mcpServers").and_then(|x| x.as_object()) {
                    for k in servers.keys() {
                        out.push(k.to_ascii_lowercase());
                    }
                }
            }
        }
    }

    // 2. User-scope (and per-project local) entries in ~/.claude.json.
    if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
        let p = home.join(".claude.json");
        if let Ok(text) = std::fs::read_to_string(&p) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                // User scope: top-level "mcpServers".
                if let Some(servers) = v.get("mcpServers").and_then(|x| x.as_object()) {
                    for k in servers.keys() {
                        out.push(k.to_ascii_lowercase());
                    }
                }
                // Local scope: projects.<abs-path>.mcpServers, keyed by
                // the cwd we were given.
                if let Some(dir) = cwd {
                    let key = dir.display().to_string();
                    if let Some(servers) = v
                        .get("projects")
                        .and_then(|x| x.get(&key))
                        .and_then(|x| x.get("mcpServers"))
                        .and_then(|x| x.as_object())
                    {
                        for k in servers.keys() {
                            out.push(k.to_ascii_lowercase());
                        }
                    }
                }
            }
        }
    }

    out.sort();
    out.dedup();
    out
}

/// Live activity for the currently-executing tool call on a workflow
/// node. Set when a `ToolUse` event lands; cleared when the matching
/// `ToolResult` lands (or when the run terminates).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeLiveTool {
    pub id: String,
    pub name: String,
    /// One-line human summary derived from the tool input
    /// (e.g. `"Write epic-01.md"`, `"Bash: cargo check"`).
    pub summary: String,
}

/// Per-cascade-node real-time state, surfaced from the executor's
/// `AgentEvent` drain so the canvas can render activity on each node
/// tile while a cascade runs.
///
/// Streaming `Text` deltas are intentionally NOT mirrored here — they
/// fire dozens of times per second and would force a full canvas
/// re-render on every keystroke. The chat panel handles letter-by-
/// letter rendering via `INPROGRESS_ASSISTANT`; node tiles show
/// coarser activity (current tool, thinking pulse, last write).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NodeLiveState {
    pub active_tool: Option<NodeLiveTool>,
    pub thinking: bool,
    /// Filename (not full path) of the most recent `Write` tool input.
    /// Sticks on the tile until the next `Write` or run end.
    pub last_write_file: Option<String>,
    pub last_error: Option<String>,
}

/// Per-node live state keyed by workflow `NodeId`. Same `GlobalSignal
/// <HashMap<…>>` shape as `ARTIFACT_RUN_STATE` for the same reason:
/// the executor writes from `spawn_forever` work that lives in the
/// virtual root scope, so component-scoped `Signal`s would drop the
/// writes.
pub static NODE_LIVE_STATE: GlobalSignal<HashMap<Uuid, NodeLiveState>> =
    Signal::global(HashMap::new);

/// Apply `f` to the live-state entry for `node_id`, inserting a
/// default if no entry exists. Shared by `workflow::executor` and
/// `artifact::runner` so both code paths publish through the same
/// helper.
pub fn publish_node_live(node_id: Uuid, f: impl FnOnce(&mut NodeLiveState)) {
    NODE_LIVE_STATE.with_mut(|m| {
        f(m.entry(node_id).or_default());
    });
}

/// One-line human summary of a tool's JSON input for display on the
/// canvas tile. Best-effort: unknown tools fall back to the bare name.
pub fn summarize_tool_input(name: &str, input: &serde_json::Value) -> String {
    let truncate = |s: &str, n: usize| -> String {
        if s.chars().count() <= n {
            s.to_string()
        } else {
            let mut out: String = s.chars().take(n).collect();
            out.push('\u{2026}');
            out
        }
    };
    let basename = |path: &str| -> String {
        std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(path)
            .to_string()
    };
    match name {
        "Write" | "Edit" | "Read" | "NotebookEdit" => input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(|p| format!("{name} {}", basename(p)))
            .unwrap_or_else(|| name.to_string()),
        "Bash" => input
            .get("command")
            .and_then(|v| v.as_str())
            .map(|c| format!("Bash: {}", truncate(c, 48)))
            .unwrap_or_else(|| "Bash".to_string()),
        "Task" => input
            .get("description")
            .and_then(|v| v.as_str())
            .map(|d| format!("Task: {}", truncate(d, 48)))
            .unwrap_or_else(|| "Task".to_string()),
        "Glob" => input
            .get("pattern")
            .and_then(|v| v.as_str())
            .map(|p| format!("Glob: {}", truncate(p, 48)))
            .unwrap_or_else(|| "Glob".to_string()),
        "Grep" => input
            .get("pattern")
            .and_then(|v| v.as_str())
            .map(|p| format!("Grep: {}", truncate(p, 48)))
            .unwrap_or_else(|| "Grep".to_string()),
        _ => name.to_string(),
    }
}

/// Drop-guard that clears `NODE_LIVE_STATE[node_id]` if a run is
/// cancelled before reaching a terminal event. Happy paths
/// (Done / Error) call `disarm()` so the terminal entry (e.g.
/// `last_error`) survives. Shared by `executor` and `runner`.
pub struct NodeLiveGuard {
    node_id: Option<Uuid>,
}

impl NodeLiveGuard {
    pub fn armed(node_id: Uuid) -> Self {
        Self { node_id: Some(node_id) }
    }

    pub fn disarm(&mut self) {
        self.node_id = None;
    }
}

impl Drop for NodeLiveGuard {
    fn drop(&mut self) {
        if let Some(node_id) = self.node_id.take() {
            NODE_LIVE_STATE.with_mut(|m| {
                m.remove(&node_id);
            });
        }
    }
}

/// Shared `ClaudeCodeChatPlugin` instance — one Arc lives at App scope
/// so both the companion (interactive chat) and the workflow executor
/// (cascade Run) talk to the same long-lived `claude` subprocess
/// driver. Each consumer creates its own Operon session UUIDs and
/// binds them via `bind_session`.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
pub struct ClaudeCodePluginCtx(
    pub std::sync::Arc<operon_plugins_claude_code::ClaudeCodeChatPlugin>,
);

/// Slice A14 cutover: the active backend used by companion / cascade /
/// executor. Holds an `Arc<dyn AgentBackend>` so the same context can
/// vend either the legacy claude-code subprocess or the new in-process
/// runtime. The picker (`AgentBackendPicker`) writes a new value here
/// when the user switches.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
pub struct AgentBackendCtx(
    pub Signal<std::sync::Arc<dyn operon_core::agent_event::AgentBackend>>,
);

/// Slice A14 cutover: pre-built backends available for the picker. The
/// app constructs both at startup; `AgentBackendCtx` points at one of
/// them. Switching is a Signal write, not a re-construction.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
pub struct BackendsCtx {
    pub claude_code: std::sync::Arc<dyn operon_core::agent_event::AgentBackend>,
    pub runtime: std::sync::Arc<dyn operon_core::agent_event::AgentBackend>,
}

#[cfg(not(target_arch = "wasm32"))]
impl BackendsCtx {
    /// Pick the backend matching `kind`. Used by `AgentBackendPicker::on_change`.
    pub fn pick(
        &self,
        kind: crate::shell::agent_backend_picker::AgentBackendKind,
    ) -> std::sync::Arc<dyn operon_core::agent_event::AgentBackend> {
        match kind {
            crate::shell::agent_backend_picker::AgentBackendKind::ClaudeCode => {
                self.claude_code.clone()
            }
            crate::shell::agent_backend_picker::AgentBackendKind::Runtime => self.runtime.clone(),
        }
    }
}

/// UI status of an in-flight permission prompt. Card buttons
/// (Allow / Allow always / Skip / Deny) transition `Pending` to the
/// matching terminal state. `AllowedAuto` records prompts resolved
/// by the category auto-approve policy without user interaction —
/// kept in the audit trail so the user can see what flowed through
/// without them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PermissionStatus {
    Pending,
    Allowed,
    AllowedAlways,
    /// Auto-approved by [`crate::shell::auto_approve::AutoApprovePolicy`]
    /// (e.g. ReadOnly read defaulted by the policy). The responder
    /// fires immediately with `Allow`; the entry is kept in the list
    /// purely for the audit trail.
    AllowedAuto,
    /// "Skipped" — the bridge returned a synthetic result body to the
    /// model rather than running the tool. On the wire indistinguishable
    /// from `Deny`; tracked separately for UI labelling.
    Skipped,
    Denied,
}

/// Reactive map of prompt-id → current UI status. The bridge handler
/// inserts `Pending` when a request arrives; click handlers transition
/// it to a terminal state. The transcript render branch reads this so
/// the buttons re-render on click without needing a per-item Signal in
/// the variant (which complicates `TranscriptItem`'s Clone+PartialEq
/// derives).
///
/// `GlobalSignal` (not a context-provided `Signal`) for the same reason
/// as `CHAT_MESSAGE_VERSION`: writes happen from the bridge handler's
/// Tokio task, which is not in any component scope.
pub static PERMISSION_DECISIONS: GlobalSignal<HashMap<String, PermissionStatus>> =
    Signal::global(HashMap::new);

#[cfg(not(target_arch = "wasm32"))]
mod permission_responders {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use tokio::sync::oneshot;

    use operon_plugins_claude_code::PermissionDecision;

    static MAP: OnceLock<Mutex<HashMap<String, oneshot::Sender<PermissionDecision>>>> =
        OnceLock::new();

    fn cell() -> &'static Mutex<HashMap<String, oneshot::Sender<PermissionDecision>>> {
        MAP.get_or_init(|| Mutex::new(HashMap::new()))
    }

    /// Park a responder under `id` so a UI click later can take and
    /// resolve it. If an entry already exists for `id` the new one
    /// replaces it; the old one is dropped (which auto-denies the
    /// prior request via the bridge's oneshot-closed path).
    pub fn insert(id: String, responder: oneshot::Sender<PermissionDecision>) {
        if let Ok(mut m) = cell().lock() {
            m.insert(id, responder);
        }
    }

    /// Take the parked responder for `id`, if any. Returns `None` if
    /// the entry was already taken or never inserted (e.g., the user
    /// double-clicked Allow → the second click is a no-op).
    pub fn take(id: &str) -> Option<oneshot::Sender<PermissionDecision>> {
        cell().lock().ok().and_then(|mut m| m.remove(id))
    }
}

/// Park a permission responder under `prompt_id`. Called by the bridge
/// handler when a new permission_prompt MCP call arrives, after which
/// it pushes a `TranscriptItem::PermissionRequest` with the same id
/// into the chat transcript.
#[cfg(not(target_arch = "wasm32"))]
pub fn park_permission_responder(
    prompt_id: String,
    responder: tokio::sync::oneshot::Sender<operon_plugins_claude_code::PermissionDecision>,
) {
    permission_responders::insert(prompt_id, responder);
}

/// Take a parked permission responder by id. Called by the inline
/// button click handlers after they update `PERMISSION_DECISIONS`.
#[cfg(not(target_arch = "wasm32"))]
pub fn take_permission_responder(
    prompt_id: &str,
) -> Option<tokio::sync::oneshot::Sender<operon_plugins_claude_code::PermissionDecision>> {
    permission_responders::take(prompt_id)
}

/// Per-prompt responders for the custom `ask_user` MCP tool. The bridge
/// executor parks an `Option<Value>` sender keyed by the prompt id;
/// the picker's Submit click takes the sender and resolves it with the
/// answers map (`{ <question>: <label or array> }`). `None` means the
/// user cancelled — the executor surfaces that as a tool_use_error.
///
/// Kept structurally identical to [`permission_responders`] so the
/// same lifecycle guarantees apply (double-click is a no-op,
/// dropped-sender → executor error).
#[cfg(not(target_arch = "wasm32"))]
mod ask_user_responders {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use tokio::sync::oneshot;

    static MAP: OnceLock<Mutex<HashMap<String, oneshot::Sender<Option<serde_json::Value>>>>> =
        OnceLock::new();

    fn cell() -> &'static Mutex<HashMap<String, oneshot::Sender<Option<serde_json::Value>>>> {
        MAP.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub fn insert(id: String, responder: oneshot::Sender<Option<serde_json::Value>>) {
        if let Ok(mut m) = cell().lock() {
            m.insert(id, responder);
        }
    }

    pub fn take(id: &str) -> Option<oneshot::Sender<Option<serde_json::Value>>> {
        cell().lock().ok().and_then(|mut m| m.remove(id))
    }
}

/// Park an ask-user responder under `prompt_id`. The executor calls
/// this immediately before pushing an [`AskUserPromptEntry`].
#[cfg(not(target_arch = "wasm32"))]
pub fn park_ask_user_responder(
    prompt_id: String,
    responder: tokio::sync::oneshot::Sender<Option<serde_json::Value>>,
) {
    ask_user_responders::insert(prompt_id, responder);
}

/// Take a parked ask-user responder. Used by the picker's Submit/Cancel
/// handlers (Submit sends `Some(answers)`, Cancel sends `None`).
#[cfg(not(target_arch = "wasm32"))]
pub fn take_ask_user_responder(
    prompt_id: &str,
) -> Option<tokio::sync::oneshot::Sender<Option<serde_json::Value>>> {
    ask_user_responders::take(prompt_id)
}

/// One pending or already-resolved permission prompt rendered inline
/// in any active companion chat. Kept around after resolution so the
/// user has an audit trail of what was permitted.
#[derive(Clone, Debug, PartialEq)]
pub struct PermissionPromptEntry {
    pub id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
    /// Source chat session that triggered the prompt — useful when
    /// the prompt is from a background artifact runner / cascade.
    pub source_session: Option<Uuid>,
    /// Working directory of the spawned claude (i.e. the child
    /// project's repo root). The Allow-always button writes the
    /// derived permission rule into `<source_cwd>/.claude/settings.local.json`,
    /// matching the project-scoped settings the harness already reads.
    pub source_cwd: Option<PathBuf>,
    /// Risk bucket derived from `tool_name`. Drives the category
    /// badge on the card and the auto-approve gating in the
    /// permission-bridge handler.
    pub category: crate::shell::tool_category::ToolCategory,
    /// When the bridge handler pushed this entry. Drives the
    /// elapsed-time counter on Pending cards and newest-first
    /// ordering in the queued-approvals drawer.
    pub created_at: std::time::SystemTime,
    /// Backend that surfaced this prompt — `"claude-code"` for the
    /// subprocess plugin, `"runtime"` for the in-process agent
    /// runtime. Cards consult this to decide whether to render the
    /// runtime-only per-tool Cancel button.
    pub backend_id: String,
}

/// Global list of all permission prompts seen so far. The currently
/// active companion chat renders these at the bottom of its transcript
/// regardless of which session triggered them — that way a background
/// cascade can ask for permission and the user sees the prompt
/// wherever they happen to be looking. Resolution status is tracked
/// separately in `PERMISSION_DECISIONS`. Capped at
/// [`PERMISSION_PROMPTS_CAP`] entries; on push, *resolved* entries
/// (anything non-Pending) are FIFO-evicted to make room — pending
/// asks are never dropped so a long-running session can't lose a
/// waiting bridge responder.
pub static PERMISSION_PROMPTS: GlobalSignal<Vec<PermissionPromptEntry>> =
    Signal::global(Vec::new);

/// Soft cap on `PERMISSION_PROMPTS` size. With streaming Bash + busy
/// cascades the list can grow several entries per second; capping
/// trims the audit trail without dropping anything actionable.
pub const PERMISSION_PROMPTS_CAP: usize = 500;

/// Append a new permission prompt to `PERMISSION_PROMPTS` and seed
/// `PERMISSION_DECISIONS` with `Pending`. Called by the bridge handler.
#[cfg(not(target_arch = "wasm32"))]
pub fn push_permission_prompt(entry: PermissionPromptEntry) {
    push_permission_prompt_with_status(entry, PermissionStatus::Pending);
}

// =================================================================
// Thread-safe UI dispatch
//
// The chat-mode permission_bridge handler and the in-process
// BridgeAskUserExecutor both run on tokio tasks spawned WITHOUT the
// Dioxus runtime guard (the bridge's `tokio::spawn` accept loop
// doesn't inherit the per-task guard that `dioxus::prelude::spawn`
// installs). Writing `PERMISSION_PROMPTS` / `ASK_USER_PROMPTS`
// directly from those tasks panics with "Must be called from inside
// a Dioxus runtime", which silently kills the task (the panic is
// caught by tokio's JoinHandle but the parked oneshot responder is
// dropped — claude sees a hung MCP call and the user sees a tool
// stuck on RUNNING with no permission card to click).
//
// Fix: a process-global mpsc channel. The Dioxus side installs the
// drain task at app boot via `init_chat_ui_dispatch`; any thread can
// `dispatch_push_permission_prompt(...)` / `dispatch_push_ask_user_prompt(...)`
// without caring about runtime guards. The drain task is spawned
// from a Dioxus component so it inherits the guard and the writes
// succeed.
//
// Why not the existing `BridgeUiSender` from `local_mode::bridge_runtime`:
// that channel is tied to the operon-bridge OS thread and only
// initialized when `provide_bridge_runtime` runs (Local mode only).
// The chat plugin's permission_bridge runs in every mode and spins
// up later in the chat lifecycle, so it needs an always-available
// dispatcher that lives at companion-state scope.
// =================================================================

/// Variants for the thread-safe chat-UI dispatcher. Each variant maps
/// 1:1 to a function that needs a Dioxus runtime guard to run safely.
#[cfg(not(target_arch = "wasm32"))]
pub enum ChatUiCommand {
    PushPermissionPrompt(PermissionPromptEntry, PermissionStatus),
    PushAskUserPrompt(AskUserPromptEntry),
}

#[cfg(not(target_arch = "wasm32"))]
static CHAT_UI_DISPATCH: std::sync::OnceLock<
    tokio::sync::mpsc::UnboundedSender<ChatUiCommand>,
> = std::sync::OnceLock::new();

/// Install the global chat-UI dispatcher. Call once at app boot from
/// a Dioxus component scope, then spawn `drain_chat_ui_commands` on
/// the Dioxus runtime so the receiver runs with the runtime guard.
/// Subsequent calls are silently ignored — the first sender wins.
#[cfg(not(target_arch = "wasm32"))]
pub fn init_chat_ui_dispatch() -> tokio::sync::mpsc::UnboundedReceiver<ChatUiCommand> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let _ = CHAT_UI_DISPATCH.set(tx);
    rx
}

/// Apply one [`ChatUiCommand`] to the underlying GlobalSignals.
/// MUST be called from a Dioxus runtime guard (the drain task
/// satisfies this).
#[cfg(not(target_arch = "wasm32"))]
pub fn apply_chat_ui_command(cmd: ChatUiCommand) {
    match cmd {
        ChatUiCommand::PushPermissionPrompt(entry, status) => {
            push_permission_prompt_with_status(entry, status);
        }
        ChatUiCommand::PushAskUserPrompt(entry) => {
            push_ask_user_prompt(entry);
        }
    }
}

/// Thread-safe replacement for direct `push_permission_prompt` calls
/// from bridge handlers. Falls back to a direct write if the
/// dispatcher hasn't been initialised yet (which only happens during
/// early app boot, before any MCP server can connect — defensive).
#[cfg(not(target_arch = "wasm32"))]
pub fn dispatch_push_permission_prompt(
    entry: PermissionPromptEntry,
    status: PermissionStatus,
) {
    if let Some(tx) = CHAT_UI_DISPATCH.get() {
        match tx.send(ChatUiCommand::PushPermissionPrompt(entry, status)) {
            Ok(()) => return,
            Err(tokio::sync::mpsc::error::SendError(cmd)) => {
                // Drain task gone — fall through to a direct write
                // using the recovered values.
                if let ChatUiCommand::PushPermissionPrompt(entry, status) = cmd {
                    push_permission_prompt_with_status(entry, status);
                }
                return;
            }
        }
    }
    // Dispatcher never initialised — direct write (only safe under a
    // Dioxus runtime guard; will panic otherwise).
    push_permission_prompt_with_status(entry, status);
}

/// Thread-safe replacement for direct `push_ask_user_prompt` calls
/// from in-process bridge handlers. See [`dispatch_push_permission_prompt`]
/// for the rationale.
#[cfg(not(target_arch = "wasm32"))]
pub fn dispatch_push_ask_user_prompt(entry: AskUserPromptEntry) {
    if let Some(tx) = CHAT_UI_DISPATCH.get() {
        match tx.send(ChatUiCommand::PushAskUserPrompt(entry)) {
            Ok(()) => return,
            Err(tokio::sync::mpsc::error::SendError(cmd)) => {
                if let ChatUiCommand::PushAskUserPrompt(entry) = cmd {
                    push_ask_user_prompt(entry);
                }
                return;
            }
        }
    }
    push_ask_user_prompt(entry);
}

/// One pending or already-resolved `ask_user` picker rendered inline
/// in any active companion chat. Mirrors [`PermissionPromptEntry`] but
/// carries the typed question schema instead of a tool-name + input
/// blob.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug, PartialEq)]
pub struct AskUserPromptEntry {
    pub id: String,
    /// Verbatim `questions` array from the MCP `ask_user` input. The
    /// picker iterates this to render headers/options; the executor
    /// echoes it back to Claude as part of the response payload so
    /// the model sees `{questions, answers}` matching the harness's
    /// built-in shape.
    pub questions: serde_json::Value,
    pub source_session: Option<Uuid>,
    pub source_cwd: Option<PathBuf>,
    pub created_at: std::time::SystemTime,
}

/// UI status of an `ask_user` prompt. Picker buttons transition
/// `Pending` to `Answered` (submitted) or `Cancelled` (dismissed).
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AskUserStatus {
    Pending,
    Answered,
    Cancelled,
}

/// Reactive map of prompt-id → current ask-user status. Picker
/// re-renders on click without needing per-entry signals.
pub static ASK_USER_DECISIONS: GlobalSignal<HashMap<String, AskUserStatus>> =
    Signal::global(HashMap::new);

/// Global list of all `ask_user` prompts seen so far. Same rendering
/// model as `PERMISSION_PROMPTS` — the active companion chat reads
/// this regardless of which session triggered the prompt.
pub static ASK_USER_PROMPTS: GlobalSignal<Vec<AskUserPromptEntry>> = Signal::global(Vec::new);

/// Soft cap on `ASK_USER_PROMPTS`. Pickers are rare compared to
/// permission prompts, but keep a cap so a misbehaving Claude that
/// loops on `ask_user` can't grow the vec unbounded.
pub const ASK_USER_PROMPTS_CAP: usize = 200;

/// Captures the snapshot the picker shows after the user has
/// answered, so the resolved card can display *what was chosen*
/// rather than going inert. Keyed by prompt id.
pub static ASK_USER_RESOLVED_ANSWERS: GlobalSignal<HashMap<String, serde_json::Value>> =
    Signal::global(HashMap::new);

/// Append a new ask-user prompt and seed its status to `Pending`.
/// Called by [`crate::shell::bridge_ask_user_executor::BridgeAskUserExecutor`]
/// after parking the responder.
#[cfg(not(target_arch = "wasm32"))]
pub fn push_ask_user_prompt(entry: AskUserPromptEntry) {
    ASK_USER_DECISIONS
        .write()
        .insert(entry.id.clone(), AskUserStatus::Pending);
    ASK_USER_PROMPTS.with_mut(|list| {
        list.push(entry);
        // FIFO evict resolved entries (Answered/Cancelled) until back
        // under the cap. Pending entries are never dropped — they
        // own a parked responder.
        if list.len() > ASK_USER_PROMPTS_CAP {
            let decisions = ASK_USER_DECISIONS.read().clone();
            while list.len() > ASK_USER_PROMPTS_CAP {
                let pos = list.iter().position(|e| {
                    !matches!(
                        decisions.get(&e.id).cloned().unwrap_or(AskUserStatus::Pending),
                        AskUserStatus::Pending
                    )
                });
                match pos {
                    Some(i) => {
                        list.remove(i);
                    }
                    None => break,
                }
            }
        }
    });
}

/// Submit an answers map for the picker `prompt_id`. Resolves the
/// parked responder (executor returns to Claude) and flips the
/// status to `Answered`. Second call for the same id is a no-op —
/// matching the `take`-style consumption in
/// [`take_permission_responder`].
///
/// `answers` is the picker's selection: `{ "<question text>": "<label>" }`
/// for single-select, `"<question text>": ["a","b"]` for multiSelect.
#[cfg(not(target_arch = "wasm32"))]
pub fn submit_ask_user_answers(prompt_id: &str, answers: serde_json::Value) {
    if let Some(responder) = take_ask_user_responder(prompt_id) {
        ASK_USER_RESOLVED_ANSWERS
            .write()
            .insert(prompt_id.to_string(), answers.clone());
        let _ = responder.send(Some(answers));
        ASK_USER_DECISIONS
            .write()
            .insert(prompt_id.to_string(), AskUserStatus::Answered);
    }
}

/// Cancel an `ask_user` picker. Resolves the parked responder with
/// `None`, which the executor turns into an error result so Claude
/// surfaces the cancellation in its next turn.
#[cfg(not(target_arch = "wasm32"))]
pub fn cancel_ask_user_prompt(prompt_id: &str) {
    if let Some(responder) = take_ask_user_responder(prompt_id) {
        let _ = responder.send(None);
        ASK_USER_DECISIONS
            .write()
            .insert(prompt_id.to_string(), AskUserStatus::Cancelled);
    }
}

/// FIFO-evict resolved entries until the vec is back under the cap.
/// Pending entries are skipped (never dropped) — they own a parked
/// responder that the bridge is waiting on; evicting one would
/// silently auto-deny the request when the responder is later dropped.
#[cfg(not(target_arch = "wasm32"))]
fn trim_permission_prompts() {
    let decisions = PERMISSION_DECISIONS.read().clone();
    PERMISSION_PROMPTS.with_mut(|list| {
        while list.len() > PERMISSION_PROMPTS_CAP {
            // Find the oldest resolved entry; if none, the entire vec
            // is Pending, which is rare-but-possible (huge burst of
            // asks faster than the user can click) — leave the list
            // alone in that case.
            let pos = list.iter().position(|e| {
                !matches!(
                    decisions.get(&e.id).cloned().unwrap_or(PermissionStatus::Pending),
                    PermissionStatus::Pending
                )
            });
            match pos {
                Some(i) => {
                    list.remove(i);
                }
                None => break,
            }
        }
    });
}

/// Maximum time a permission prompt can sit `Pending` before the
/// watchdog auto-denies it. The cutoff exists so a background
/// cascade can't hang forever waiting for an approval the user
/// missed (e.g. they closed the drawer and went to lunch).
#[cfg(not(target_arch = "wasm32"))]
pub const STALE_PROMPT_CUTOFF: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// Watchdog tick interval. Cheap — the sweep is O(prompts) and
/// touches no I/O.
#[cfg(not(target_arch = "wasm32"))]
pub const STALE_PROMPT_TICK: std::time::Duration = std::time::Duration::from_secs(30);

/// Spawn a long-running task that auto-denies pending permission
/// prompts older than [`STALE_PROMPT_CUTOFF`]. Called once at app
/// boot from `app.rs`; safe to call multiple times (each call
/// spawns its own ticker — extra tickers are harmless overhead).
///
/// Why a watchdog: with Phase 4's cascade opt-in, cascades start
/// surfacing real prompts that block forward progress. A user who
/// dismisses the drawer or steps away would otherwise leave the
/// cascade hung forever on a parked responder.
#[cfg(not(target_arch = "wasm32"))]
pub fn start_permission_watchdog() {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(STALE_PROMPT_TICK).await;
            sweep_stale_permission_prompts();
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn sweep_stale_permission_prompts() {
    use operon_plugins_claude_code::PermissionDecision as BridgeDecision;

    let now = std::time::SystemTime::now();
    // Snapshot the (id, source_session) pairs that have aged out so
    // we don't hold the signal read-guard across responder sends.
    let stale: Vec<(String, Option<Uuid>)> = {
        let prompts = PERMISSION_PROMPTS.read();
        let decisions = PERMISSION_DECISIONS.read();
        prompts
            .iter()
            .filter(|e| {
                matches!(
                    decisions.get(&e.id).cloned().unwrap_or(PermissionStatus::Pending),
                    PermissionStatus::Pending
                )
            })
            .filter(|e| {
                now.duration_since(e.created_at)
                    .map(|d| d > STALE_PROMPT_CUTOFF)
                    .unwrap_or(false)
            })
            .map(|e| (e.id.clone(), e.source_session))
            .collect()
    };
    if stale.is_empty() {
        return;
    }
    for (id, source_session) in stale {
        PERMISSION_DECISIONS
            .write()
            .insert(id.clone(), PermissionStatus::Denied);
        if let Some(responder) = take_permission_responder(&id) {
            let _ = responder.send(BridgeDecision::Deny {
                message: format!(
                    "Operon: auto-denied — permission prompt sat pending longer than \
                     {} minutes",
                    STALE_PROMPT_CUTOFF.as_secs() / 60
                ),
            });
        }
        tracing::warn!(
            target: "operon::permission",
            "stale prompt {id} (session {source_session:?}) auto-denied by watchdog"
        );
    }
}

/// Audit-trail variant: push an entry with a *terminal* status (e.g.
/// `AllowedAuto` for category auto-approve, `Skipped` for synthetic
/// results pushed from the bridge handler before any UI interaction).
/// The card renders inert because the status is not `Pending`.
#[cfg(not(target_arch = "wasm32"))]
pub fn push_permission_prompt_with_status(
    entry: PermissionPromptEntry,
    status: PermissionStatus,
) {
    PERMISSION_DECISIONS
        .write()
        .insert(entry.id.clone(), status);
    PERMISSION_PROMPTS.write().push(entry);
    trim_permission_prompts();
}

/// Live stdout/stderr rolling buffer for a running tool call, keyed by
/// `tool_use_id`. Populated by `AgentEvent::ToolChunk` events from the
/// in-process runtime backend; the `tool_card` view subscribes to the
/// matching entry and renders a terminal-style live region while the
/// tool is in flight.
///
/// claude-code backend never writes here — claude only emits a single
/// `ToolResult` per call, so there's nothing to stream. The card
/// falls back to the post-completion result rendering for that
/// backend.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolStream {
    pub stdout: String,
    pub stderr: String,
    pub started_at: Option<std::time::SystemTime>,
    /// Set when the matching `ToolResult` arrives; the card stops
    /// updating its elapsed timer and the stream is held only for
    /// the audit-trail view.
    pub complete: bool,
}

/// Soft cap per stream so a runaway tool can't consume unbounded
/// memory. Older bytes are trimmed from the front when this is hit;
/// the tail (most recent output) is what the user wants to see.
pub const TOOL_STREAM_CAP_BYTES: usize = 64 * 1024;

pub static TOOL_STREAM_OUTPUT: GlobalSignal<HashMap<String, ToolStream>> =
    Signal::global(HashMap::new);

/// Append a chunk to the stream for `tool_use_id`. `kind` is
/// `"stdout"` or `"stderr"`; anything else is dropped onto `stdout`
/// for safety. Trims the head when over [`TOOL_STREAM_CAP_BYTES`].
#[cfg(not(target_arch = "wasm32"))]
pub fn append_tool_chunk(tool_use_id: &str, kind: &str, bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes).into_owned();
    TOOL_STREAM_OUTPUT.with_mut(|m| {
        let entry = m.entry(tool_use_id.to_string()).or_default();
        if entry.started_at.is_none() {
            entry.started_at = Some(std::time::SystemTime::now());
        }
        let target = if kind == "stderr" {
            &mut entry.stderr
        } else {
            &mut entry.stdout
        };
        target.push_str(&text);
        if target.len() > TOOL_STREAM_CAP_BYTES {
            // Drop from the front, keep the tail. Find a UTF-8
            // boundary so we don't slice a multi-byte char in half.
            let overflow = target.len() - TOOL_STREAM_CAP_BYTES;
            let mut cut = overflow;
            while cut < target.len() && !target.is_char_boundary(cut) {
                cut += 1;
            }
            target.drain(..cut);
        }
    });
}

/// Record the start time for `tool_use_id` if it isn't already set.
/// Idempotent: callers (the `ToolUse` event arm in both apply paths)
/// fire this when the card is first rendered; later `ToolChunk`
/// arrivals via `append_tool_chunk` see an already-populated
/// `started_at` and leave it alone. Without this, claude-code tools
/// — which never emit `ToolChunk` — would never get a timestamp and
/// the elapsed timer in `PendingToolFooter` would stay frozen.
#[cfg(not(target_arch = "wasm32"))]
pub fn mark_tool_started(tool_use_id: &str) {
    TOOL_STREAM_OUTPUT.with_mut(|m| {
        let entry = m.entry(tool_use_id.to_string()).or_default();
        if entry.started_at.is_none() {
            entry.started_at = Some(std::time::SystemTime::now());
        }
    });
}

/// Mark a tool's stream as complete (the matching `ToolResult` has
/// landed). Card stops updating its elapsed timer but the buffered
/// output stays in the map until the next session reset.
#[cfg(not(target_arch = "wasm32"))]
pub fn mark_tool_stream_complete(tool_use_id: &str) {
    TOOL_STREAM_OUTPUT.with_mut(|m| {
        if let Some(entry) = m.get_mut(tool_use_id) {
            entry.complete = true;
        }
    });
}

/// Per-tool cancellation is plumbed through the `AgentBackend` trait
/// rather than a global signal — the backend owns the actual
/// `CancellationToken`s and we'd just duplicate them here. The tool
/// card calls `backend.cancel_tool(session, id)` directly.
///
/// claude-code backend always returns `false` (claude owns its own
/// subprocess and Operon can only cancel the entire turn). The
/// runtime backend looks up the per-tool token in its
/// `tool_cancellations` registry and fires it.

/// Build a claude-style rule pattern key for the tool override lookup.
/// `Some("Bash(npm install *)")` for Bash; `None` for other tools
/// (a per-tool override at the tool-name level still wins).
#[cfg(not(target_arch = "wasm32"))]
fn derive_pattern_key(tool_name: &str, input: &serde_json::Value) -> Option<String> {
    if tool_name == "Bash" {
        let rule = crate::shell::permission_persist::derive_rule(tool_name, input);
        // `derive_rule` returns just `"Bash"` when the input was
        // empty / malformed; treat that as no rule-level override.
        if rule != "Bash" {
            return Some(rule);
        }
    }
    None
}

#[cfg(not(target_arch = "wasm32"))]
mod session_bridges {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};
    use tokio::sync::OnceCell;
    use uuid::Uuid;

    static CELLS: OnceLock<Mutex<HashMap<Uuid, Arc<OnceCell<()>>>>> = OnceLock::new();

    fn map() -> &'static Mutex<HashMap<Uuid, Arc<OnceCell<()>>>> {
        CELLS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    /// Per-session readiness handle. The first call inserts a fresh
    /// `OnceCell`; subsequent callers see the same cell and `await`
    /// the in-flight bind via `get_or_try_init`. On bind failure the
    /// cell is left empty so the next caller retries cleanly.
    ///
    /// Replaces the old "claim-once HashSet" gate, which let caller
    /// #2 return `Ok(())` before caller #1 had finished calling
    /// `set_session_bridge` — a race that left `spawn_turn` reading
    /// `binding.bridge = None` and spawning claude without
    /// `--permission-prompt-tool`.
    pub fn handle(id: Uuid) -> Arc<OnceCell<()>> {
        let mut m = map().lock().expect("session_bridges mutex poisoned");
        m.entry(id)
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone()
    }
}

/// Ensure a `PermissionBridge` is bound to `session_id` on `plugin`.
/// First call creates a per-session Unix socket under
/// `tempdir/operon-permission-sockets/<session>.sock`, attaches a
/// handler that pushes prompts into `PERMISSION_PROMPTS`, and registers
/// the bridge with the plugin so subsequent `spawn_turn` calls add the
/// `--mcp-config` + `--permission-prompt-tool` flags.
///
/// Concurrent callers for the same `session_id` all `await` the same
/// in-flight bind via a per-session `tokio::sync::OnceCell`; once it
/// resolves every caller returns `Ok(())`. Cheap to call repeatedly —
/// the send-message handler `await`s this immediately before every
/// `send_rich` so the first turn after a session opens can't race
/// ahead of the bridge bind.
///
/// Both the interactive companion chat and the headless artifact
/// runner call this after their own `plugin.bind_session(...)`.
/// Project context the bridge needs to advertise + serve the M4
/// `create_artifact` tool. `None` (or the convenience call
/// [`ensure_session_bridge`]) leaves the tool unadvertised — Claude
/// falls back to the legacy Write-tool path. Callers with a resolved
/// project (the cascade runner, per-node ▶ runs) pass `Some(ctx)` so
/// the bridge exposes typed artifact creation for that session.
#[cfg(not(target_arch = "wasm32"))]
pub struct ArtifactBridgeCtx {
    pub note_repo: Arc<dyn operon_store::repos::LocalNoteRepository>,
    pub persistence: Arc<dyn crate::persistence::Persistence>,
    pub project_id: Uuid,
}

/// Convenience wrapper that calls [`ensure_session_bridge_with_ctx`]
/// with `None`. Use this when the call site has no project context
/// (interactive chat in Vault scope, fresh-app companion turn before
/// any project is selected, …).
#[cfg(not(target_arch = "wasm32"))]
pub async fn ensure_session_bridge(
    plugin: &operon_plugins_claude_code::ClaudeCodeChatPlugin,
    session_id: Uuid,
    cwd: PathBuf,
) -> std::io::Result<()> {
    ensure_session_bridge_with_ctx(plugin, session_id, cwd, None).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn ensure_session_bridge_with_ctx(
    plugin: &operon_plugins_claude_code::ClaudeCodeChatPlugin,
    session_id: Uuid,
    cwd: PathBuf,
    artifact_ctx: Option<ArtifactBridgeCtx>,
) -> std::io::Result<()> {
    use operon_plugins_claude_code::{
        PermissionBridge, PermissionDecision, PermissionRequest,
    };
    use tokio::sync::oneshot;

    let cell = session_bridges::handle(session_id);
    cell.get_or_try_init(|| async {
        let dir = std::env::temp_dir().join("operon-permission-sockets");
        let socket = dir.join(format!("{session_id}.sock"));

        let cwd_for_handler = cwd.clone();
        let handler = move |req: PermissionRequest,
                            respond: oneshot::Sender<PermissionDecision>| {
            let id = req
                .tool_use_id
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| Uuid::new_v4().to_string());
            let category = crate::shell::tool_category::of(&req.tool_name);
            let policy = crate::shell::auto_approve::load_effective(&cwd_for_handler);
            // Pattern key for per-tool overrides like `Bash(git push *)`
            // — for Bash we synthesise a claude-style rule shape from
            // the input so a single override entry matches all
            // subcommand variants the user wants gated.
            let pattern_key = derive_pattern_key(&req.tool_name, &req.input);
            // Category-based auto-approve (override-aware): short-
            // circuit before parking a responder so the model doesn't
            // wait for a UI click. Audit-trail entry still goes into
            // PERMISSION_PROMPTS with status AllowedAuto so the user
            // can see what flowed through.
            if policy.auto_approve_for(&req.tool_name, pattern_key.as_deref(), category) {
                let _ = respond.send(PermissionDecision::Allow { updated_input: None });
                dispatch_push_permission_prompt(
                    PermissionPromptEntry {
                        id,
                        tool_name: req.tool_name,
                        input: req.input,
                        source_session: Some(session_id),
                        source_cwd: Some(cwd_for_handler.clone()),
                        category,
                        created_at: std::time::SystemTime::now(),
                        backend_id: "claude-code".to_string(),
                    },
                    PermissionStatus::AllowedAuto,
                );
                return;
            }
            park_permission_responder(id.clone(), respond);
            dispatch_push_permission_prompt(
                PermissionPromptEntry {
                    id,
                    tool_name: req.tool_name,
                    input: req.input,
                    source_session: Some(session_id),
                    source_cwd: Some(cwd_for_handler.clone()),
                    category,
                    created_at: std::time::SystemTime::now(),
                    backend_id: "claude-code".to_string(),
                },
                PermissionStatus::Pending,
            );
        };

        let bridge = PermissionBridge::bind(socket, handler).await?;
        // Phase 6 opt-in: when the policy turns `bash_via_operon`
        // on, install Operon's own bash runner as the bridge's
        // shell executor AND mark the session so spawn_turn adds
        // `--disallowedTools Bash` to the claude CLI. The bridge
        // then advertises `mcp__operon__operon_bash` in tools/list
        // and routes claude's bash invocations through it
        // (streaming + cancellable).
        let bash_via_operon = crate::shell::auto_approve::load_effective(&cwd).bash_via_operon;
        if bash_via_operon {
            bridge.set_shell_executor(Some(std::sync::Arc::new(
                crate::shell::bridge_shell_executor::BridgeShellExecutor::new(),
            )));
        }
        // M4 — when the caller supplies project context, install the
        // typed artifact executor so Claude can declare SDLC outputs
        // via `mcp__operon__create_artifact` instead of the legacy
        // Write-tool + mtime-scan handshake. Without the context, the
        // tool stays unadvertised and the old path remains in effect.
        if let Some(ctx) = artifact_ctx {
            bridge.set_artifact_executor(Some(std::sync::Arc::new(
                crate::shell::bridge_artifact_executor::BridgeArtifactExecutor::new(
                    ctx.note_repo,
                    ctx.persistence,
                    ctx.project_id,
                ),
            )));
        }
        // Replace the harness-owned built-in AskUserQuestion: the
        // harness intercepts tool_result frames for that tool in
        // non-TUI mode (synthesising empty answers), so the only
        // working channel for structured questions is a custom MCP
        // tool whose result the harness passes through verbatim.
        // The matching `--disallowedTools AskUserQuestion` flag in
        // `spawn_turn` keeps Claude from falling back to the broken
        // built-in. Always installed — no policy gate, because the
        // built-in is broken in all configurations of this backend.
        bridge.set_ask_user_executor(Some(std::sync::Arc::new(
            crate::shell::bridge_ask_user_executor::BridgeAskUserExecutor::new(
                session_id,
                cwd.clone(),
            ),
        )));
        plugin.set_session_bash_via_operon(session_id, bash_via_operon);
        plugin.set_session_bridge(session_id, Some(std::sync::Arc::new(bridge)));
        Ok::<(), std::io::Error>(())
    })
    .await
    .map(|_| ())
}

// ============================================================
// Note-edit proposals (M4c.7 — diff-card confirm flow)
//
// Backing state for `OperonReplaceNoteRangeTool` when invoked with
// `confirm: true`. The tool computes the proposed body, parks a
// one-shot responder, pushes a `NoteProposalEntry` here, and awaits.
// The user's Accept/Reject click in the companion chat surface
// resolves the responder; the tool then either persists the change
// or returns an error.
//
// Same shape as the ask-user infra above (responder map +
// pending-entry vec + status map). Kept separate so its eviction
// policy and rendering can evolve independently.
// ============================================================

#[cfg(not(target_arch = "wasm32"))]
mod note_proposal_responders {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use tokio::sync::oneshot;

    static MAP: OnceLock<Mutex<HashMap<String, oneshot::Sender<bool>>>> = OnceLock::new();

    fn cell() -> &'static Mutex<HashMap<String, oneshot::Sender<bool>>> {
        MAP.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub fn insert(id: String, responder: oneshot::Sender<bool>) {
        if let Ok(mut m) = cell().lock() {
            m.insert(id, responder);
        }
    }

    pub fn take(id: &str) -> Option<oneshot::Sender<bool>> {
        cell().lock().ok().and_then(|mut m| m.remove(id))
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn park_note_proposal_responder(
    proposal_id: String,
    responder: tokio::sync::oneshot::Sender<bool>,
) {
    note_proposal_responders::insert(proposal_id, responder);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn take_note_proposal_responder(
    proposal_id: &str,
) -> Option<tokio::sync::oneshot::Sender<bool>> {
    note_proposal_responders::take(proposal_id)
}

/// One pending or already-resolved note-edit proposal. The card
/// renders the pre-computed `diff_preview` (unified-diff text) so
/// the user can decide without a round-trip to the model. `old_body`
/// + `new_body` are kept too in case a future card wants to render
/// a richer side-by-side or syntax-highlighted diff.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug, PartialEq)]
pub struct NoteProposalEntry {
    /// Per-proposal id (UUID v4 string). Distinct from `note_id`
    /// because a single note can have multiple pending proposals
    /// queued from sequential tool calls.
    pub id: String,
    pub note_id: Uuid,
    /// Cached at proposal time for the card header. The live title
    /// could have been renamed in the meantime; the cached value is
    /// what Claude saw, which is what the user reviewing the diff
    /// should also see.
    pub note_title: String,
    pub old_body: String,
    pub new_body: String,
    pub diff_preview: String,
    pub source_session: Option<Uuid>,
    pub created_at: std::time::SystemTime,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoteProposalStatus {
    Pending,
    Accepted,
    Rejected,
}

/// Reactive status map for proposals. Card buttons read this to
/// switch between Pending (interactive) and resolved-render modes.
pub static NOTE_PROPOSAL_DECISIONS: GlobalSignal<HashMap<String, NoteProposalStatus>> =
    Signal::global(HashMap::new);

/// All proposals seen so far. Same rendering model as
/// `ASK_USER_PROMPTS`: the active companion chat renders every
/// pending entry regardless of which session triggered it.
#[cfg(not(target_arch = "wasm32"))]
pub static NOTE_PROPOSALS: GlobalSignal<Vec<NoteProposalEntry>> = Signal::global(Vec::new);

/// Soft cap on `NOTE_PROPOSALS`. Edit proposals are paced by user
/// review, so this cap is more about catching runaway bugs than
/// throttling normal usage.
pub const NOTE_PROPOSALS_CAP: usize = 200;

/// Append a proposal and seed its status to Pending. Mirrors
/// `push_ask_user_prompt` — same FIFO eviction policy that never
/// drops a Pending entry (it owns a parked responder).
#[cfg(not(target_arch = "wasm32"))]
pub fn push_note_proposal(entry: NoteProposalEntry) {
    NOTE_PROPOSAL_DECISIONS
        .write()
        .insert(entry.id.clone(), NoteProposalStatus::Pending);
    NOTE_PROPOSALS.with_mut(|list| {
        list.push(entry);
        if list.len() > NOTE_PROPOSALS_CAP {
            let decisions = NOTE_PROPOSAL_DECISIONS.read().clone();
            while list.len() > NOTE_PROPOSALS_CAP {
                let pos = list.iter().position(|e| {
                    !matches!(
                        decisions.get(&e.id).cloned().unwrap_or(NoteProposalStatus::Pending),
                        NoteProposalStatus::Pending
                    )
                });
                match pos {
                    Some(i) => {
                        list.remove(i);
                    }
                    None => break,
                }
            }
        }
    });
}

/// User clicked Accept on a proposal card. Resolves the responder
/// with `true` so the tool's await wakes up and persists the change.
/// Second call for the same id is a no-op (the responder is taken).
#[cfg(not(target_arch = "wasm32"))]
pub fn accept_note_proposal(proposal_id: &str) {
    if let Some(responder) = take_note_proposal_responder(proposal_id) {
        let _ = responder.send(true);
        NOTE_PROPOSAL_DECISIONS
            .write()
            .insert(proposal_id.to_string(), NoteProposalStatus::Accepted);
    }
}

/// User clicked Reject. Resolves with `false`; the tool returns an
/// error so Claude knows to back off / try a different edit.
#[cfg(not(target_arch = "wasm32"))]
pub fn reject_note_proposal(proposal_id: &str) {
    if let Some(responder) = take_note_proposal_responder(proposal_id) {
        let _ = responder.send(false);
        NOTE_PROPOSAL_DECISIONS
            .write()
            .insert(proposal_id.to_string(), NoteProposalStatus::Rejected);
    }
}

// ============================================================
// Note-deletion proposals (delete_note confirm card)
// ============================================================
//
// Same shape as the edit-proposal trio above but separate state so
// the deletion card UI can render distinctly (different copy, no
// diff preview — just title + descendant count). The responder map
// lives in its own OnceLock so accept/reject for deletions can't
// accidentally collide with edit-proposal ids.

#[cfg(not(target_arch = "wasm32"))]
mod note_deletion_responders {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use tokio::sync::oneshot;

    static MAP: OnceLock<Mutex<HashMap<String, oneshot::Sender<bool>>>> = OnceLock::new();

    fn cell() -> &'static Mutex<HashMap<String, oneshot::Sender<bool>>> {
        MAP.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub fn insert(id: String, responder: oneshot::Sender<bool>) {
        if let Ok(mut m) = cell().lock() {
            m.insert(id, responder);
        }
    }

    pub fn take(id: &str) -> Option<oneshot::Sender<bool>> {
        cell().lock().ok().and_then(|mut m| m.remove(id))
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn park_note_deletion_responder(
    proposal_id: String,
    responder: tokio::sync::oneshot::Sender<bool>,
) {
    note_deletion_responders::insert(proposal_id, responder);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn take_note_deletion_responder(
    proposal_id: &str,
) -> Option<tokio::sync::oneshot::Sender<bool>> {
    note_deletion_responders::take(proposal_id)
}

/// One pending deletion proposal. Captures just enough for the
/// confirm card: the note id, its title at proposal time, and how
/// many descendants would also be removed (`note_repo.delete`
/// cascades children via the foreign-key constraint).
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteDeletionProposalEntry {
    pub id: String,
    pub note_id: Uuid,
    pub note_title: String,
    pub descendant_count: usize,
    pub source_session: Option<Uuid>,
    pub created_at: std::time::SystemTime,
}

/// Reactive status map for deletion proposals; same Pending /
/// Accepted / Rejected vocabulary as edit proposals so the card
/// component can switch render modes the same way.
pub static NOTE_DELETION_DECISIONS: GlobalSignal<HashMap<String, NoteProposalStatus>> =
    Signal::global(HashMap::new);

#[cfg(not(target_arch = "wasm32"))]
pub static NOTE_DELETION_PROPOSALS: GlobalSignal<Vec<NoteDeletionProposalEntry>> =
    Signal::global(Vec::new);

#[cfg(not(target_arch = "wasm32"))]
pub fn push_note_deletion_proposal(entry: NoteDeletionProposalEntry) {
    NOTE_DELETION_DECISIONS
        .write()
        .insert(entry.id.clone(), NoteProposalStatus::Pending);
    NOTE_DELETION_PROPOSALS.with_mut(|list| {
        list.push(entry);
        // Reuse the edit-proposal cap; deletions are even rarer.
        if list.len() > NOTE_PROPOSALS_CAP {
            let decisions = NOTE_DELETION_DECISIONS.read().clone();
            while list.len() > NOTE_PROPOSALS_CAP {
                let pos = list.iter().position(|e| {
                    !matches!(
                        decisions
                            .get(&e.id)
                            .cloned()
                            .unwrap_or(NoteProposalStatus::Pending),
                        NoteProposalStatus::Pending
                    )
                });
                match pos {
                    Some(i) => {
                        list.remove(i);
                    }
                    None => break,
                }
            }
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
pub fn accept_note_deletion(proposal_id: &str) {
    if let Some(responder) = take_note_deletion_responder(proposal_id) {
        let _ = responder.send(true);
        NOTE_DELETION_DECISIONS
            .write()
            .insert(proposal_id.to_string(), NoteProposalStatus::Accepted);
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn reject_note_deletion(proposal_id: &str) {
    if let Some(responder) = take_note_deletion_responder(proposal_id) {
        let _ = responder.send(false);
        NOTE_DELETION_DECISIONS
            .write()
            .insert(proposal_id.to_string(), NoteProposalStatus::Rejected);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dioxus::prelude::*;
    use tokio_util::sync::CancellationToken;

    // GlobalSignal reads/writes require a Dioxus runtime to be
    // active. `VirtualDom::new` is the only public way to construct
    // one. The dom isn't progressed — we just need its runtime guard
    // installed for the body. Each test takes a snapshot/restore of
    // the global so siblings don't see each other's mutations.
    fn with_runtime<F: FnOnce()>(f: F) {
        fn app() -> Element {
            rsx! { "" }
        }
        let dom = VirtualDom::new(app);
        dom.in_runtime(f);
    }

    /// Load-bearing documentation for M4c.9 (the
    /// `BridgeUiCommand` channel). This test asserts the constraint
    /// that motivated the channel: writing a `GlobalSignal` from a
    /// thread that never had a Dioxus runtime guard installed
    /// **panics**, even though `GlobalSignal::write` looks like a
    /// plain `Mutex`-style API.
    ///
    /// If Dioxus ever loosens this requirement, this test will start
    /// failing — at which point the channel layer in
    /// `local_mode::bridge_runtime` becomes optional, not required.
    /// Keep this test green as a guardrail; the channel is an
    /// implementation cost we only pay because of this constraint.
    #[test]
    #[should_panic(expected = "Must be called from inside a Dioxus runtime")]
    fn global_signal_write_from_thread_without_dioxus_runtime_panics() {
        // We deliberately do NOT wrap this in `with_runtime` — the
        // whole point is to probe what happens with no runtime guard.
        // A bare main-thread write triggers the same panic the
        // bridge thread would: Dioxus's GlobalSignal write path
        // calls `Runtime::current().expect(...)` internally.
        *LOCAL_NOTE_VERSION.write() = LOCAL_NOTE_VERSION.read().saturating_add(1);
    }

    #[test]
    fn is_session_running_reflects_map_membership() {
        with_runtime(|| {
            let a = Uuid::new_v4();
            let b = Uuid::new_v4();
            let prior = CHAT_RUN_CANCEL.read().clone();
            CHAT_RUN_CANCEL.write().clear();

            assert!(!is_session_running(a));
            CHAT_RUN_CANCEL.write().insert(a, CancellationToken::new());
            assert!(is_session_running(a));
            assert!(!is_session_running(b));

            *CHAT_RUN_CANCEL.write() = prior;
        });
    }

    #[test]
    fn cancel_session_run_signals_only_target_token() {
        with_runtime(|| {
            let a = Uuid::new_v4();
            let b = Uuid::new_v4();
            let prior = CHAT_RUN_CANCEL.read().clone();
            CHAT_RUN_CANCEL.write().clear();

            let ct_a = CancellationToken::new();
            let ct_b = CancellationToken::new();
            CHAT_RUN_CANCEL.write().insert(a, ct_a.clone());
            CHAT_RUN_CANCEL.write().insert(b, ct_b.clone());

            assert!(cancel_session_run(a));
            assert!(ct_a.is_cancelled(), "target session's CT must be cancelled");
            assert!(
                !ct_b.is_cancelled(),
                "sibling session's CT must NOT be cancelled — parallel sessions terminate independently"
            );

            CHAT_RUN_CANCEL.write().remove(&a);
            CHAT_RUN_CANCEL.write().remove(&b);
            *CHAT_RUN_CANCEL.write() = prior;
        });
    }

    #[test]
    fn cancel_session_run_returns_false_when_no_entry() {
        with_runtime(|| {
            let a = Uuid::new_v4();
            let prior = CHAT_RUN_CANCEL.read().clone();
            CHAT_RUN_CANCEL.write().clear();

            assert!(!cancel_session_run(a));

            *CHAT_RUN_CANCEL.write() = prior;
        });
    }

    #[test]
    fn static_mcp_server_names_reads_project_mcp_json() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mcp_path = tmp.path().join(".mcp.json");
        std::fs::write(
            &mcp_path,
            r#"{
                "mcpServers": {
                    "figma": {"command": "npx", "args": []},
                    "Sentry": {"type": "http", "url": "x"}
                }
            }"#,
        )
        .expect("write");

        let names = static_mcp_server_names(Some(tmp.path()));
        assert!(names.iter().any(|n| n == "figma"));
        assert!(names.iter().any(|n| n == "sentry"));
    }

    #[test]
    fn static_mcp_server_names_handles_missing_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // No .mcp.json in tmp; result should not panic and should not
        // include any project-scoped names (user-scope ~/.claude.json
        // may still contribute, so we only assert non-panic + that
        // bogus names aren't fabricated).
        let names = static_mcp_server_names(Some(tmp.path()));
        assert!(!names.iter().any(|n| n == "figma_should_not_exist"));
    }

    #[test]
    fn requirements_gate_passes_when_static_mcp_json_has_server() {
        with_runtime(|| {
            // Empty live snapshot (cold-cascade scenario).
            let prior = MCP_LIVE_STATUS.read().clone();
            *MCP_LIVE_STATUS.write() = McpLiveStatus::default();

            let tmp = tempfile::tempdir().expect("tempdir");
            std::fs::write(
                tmp.path().join(".mcp.json"),
                r#"{"mcpServers":{"figma":{"command":"npx"}}}"#,
            )
            .expect("write");

            // requires_mcp: ["figma"] — gate should pass via static cfg
            // even though MCP_LIVE_STATUS is empty.
            let reqs = vec!["figma".to_string()];
            let result = mcp_skill_requirements_gate_error(&reqs, Some(tmp.path()));
            assert!(result.is_none(), "expected None, got {result:?}");

            *MCP_LIVE_STATUS.write() = prior;
        });
    }

    #[test]
    fn requirements_gate_blocks_when_static_mcp_json_missing_server() {
        with_runtime(|| {
            let prior = MCP_LIVE_STATUS.read().clone();
            *MCP_LIVE_STATUS.write() = McpLiveStatus::default();

            // Isolate from the developer's real ~/.claude.json — that
            // file may legitimately contain "figma" from prior CLI use
            // and would make this test pass spuriously.
            let tmp = tempfile::tempdir().expect("tempdir");
            let fake_home = tempfile::tempdir().expect("home tmpdir");
            let prior_home = std::env::var_os("HOME");
            // SAFETY (single-threaded test runner segment): we restore
            // HOME below; concurrent tests in the same process won't
            // observe this because cargo test runs each #[test] on its
            // own thread but env vars are process-global — see the
            // matching restore.
            unsafe {
                std::env::set_var("HOME", fake_home.path());
            }

            // .mcp.json exists but doesn't declare figma.
            std::fs::write(
                tmp.path().join(".mcp.json"),
                r#"{"mcpServers":{"sentry":{"type":"http","url":"x"}}}"#,
            )
            .expect("write");

            let reqs = vec!["figma".to_string()];
            let result = mcp_skill_requirements_gate_error(&reqs, Some(tmp.path()));

            // Restore env BEFORE assert so a panic still cleans up.
            match prior_home {
                Some(v) => unsafe { std::env::set_var("HOME", v) },
                None => unsafe { std::env::remove_var("HOME") },
            }
            *MCP_LIVE_STATUS.write() = prior;

            assert!(
                result.as_ref().map(|m| m.contains("figma")).unwrap_or(false),
                "expected figma to be flagged missing, got {result:?}"
            );
        });
    }
}
