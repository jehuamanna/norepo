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
/// The gate is unconditional: every entry must be satisfied for the
/// skill to fire. If a skill can run productively without an MCP
/// tool, the skill author should simply omit that entry from
/// `requires_mcp` — not declare it and hope the runtime is lenient.
pub fn mcp_skill_requirements_gate_error(requirements: &[String]) -> Option<String> {
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
    let missing: Vec<String> = requirements
        .iter()
        .filter(|req| {
            let needle = req.to_lowercase();
            !connected_servers.iter().any(|s| s.contains(&needle))
                && !tools_lc.iter().any(|t| t.contains(&needle))
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
            let policy = crate::shell::auto_approve::load(&cwd_for_handler);
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
                push_permission_prompt_with_status(
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
            push_permission_prompt(PermissionPromptEntry {
                id,
                tool_name: req.tool_name,
                input: req.input,
                source_session: Some(session_id),
                source_cwd: Some(cwd_for_handler.clone()),
                category,
                created_at: std::time::SystemTime::now(),
                backend_id: "claude-code".to_string(),
            });
        };

        let bridge = PermissionBridge::bind(socket, handler).await?;
        // Phase 6 opt-in: when the policy turns `bash_via_operon`
        // on, install Operon's own bash runner as the bridge's
        // shell executor AND mark the session so spawn_turn adds
        // `--disallowedTools Bash` to the claude CLI. The bridge
        // then advertises `mcp__operon__operon_bash` in tools/list
        // and routes claude's bash invocations through it
        // (streaming + cancellable).
        let bash_via_operon = crate::shell::auto_approve::load(&cwd).bash_via_operon;
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
        plugin.set_session_bash_via_operon(session_id, bash_via_operon);
        plugin.set_session_bridge(session_id, Some(std::sync::Arc::new(bridge)));
        Ok::<(), std::io::Error>(())
    })
    .await
    .map(|_| ())
}
