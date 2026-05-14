//! Cross-cutting state shared between the explorer / project list, the
//! companion-pane chat, and (in M2) skill / workflow plugins. The signals
//! here are provided in `local_mode::desktop` and consumed by various
//! components; this module is the one place the newtypes live so both
//! sides can import them without circular module deps.

use dioxus::prelude::Signal;
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

/// UI status of an in-flight permission prompt. The transcript renders
/// three buttons (Allow / Allow always / Deny); clicking one transitions
/// `Pending` to the matching terminal state and disables the buttons.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PermissionStatus {
    Pending,
    Allowed,
    AllowedAlways,
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
}

/// Global list of all permission prompts seen so far. The currently
/// active companion chat renders these at the bottom of its transcript
/// regardless of which session triggered them — that way a background
/// cascade can ask for permission and the user sees the prompt
/// wherever they happen to be looking. Resolution status is tracked
/// separately in `PERMISSION_DECISIONS`; this list is append-only for
/// the lifetime of the app.
pub static PERMISSION_PROMPTS: GlobalSignal<Vec<PermissionPromptEntry>> =
    Signal::global(Vec::new);

/// Append a new permission prompt to `PERMISSION_PROMPTS` and seed
/// `PERMISSION_DECISIONS` with `Pending`. Called by the bridge handler.
#[cfg(not(target_arch = "wasm32"))]
pub fn push_permission_prompt(entry: PermissionPromptEntry) {
    PERMISSION_DECISIONS
        .write()
        .insert(entry.id.clone(), PermissionStatus::Pending);
    PERMISSION_PROMPTS.write().push(entry);
}

#[cfg(not(target_arch = "wasm32"))]
mod session_bridges {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    use uuid::Uuid;

    static BOUND: OnceLock<Mutex<HashSet<Uuid>>> = OnceLock::new();

    fn cell() -> &'static Mutex<HashSet<Uuid>> {
        BOUND.get_or_init(|| Mutex::new(HashSet::new()))
    }

    /// Returns true on the first call for `id`, false on subsequent
    /// calls — used to make `ensure_session_bridge` idempotent.
    pub fn claim(id: Uuid) -> bool {
        cell()
            .lock()
            .map(|mut s| s.insert(id))
            .unwrap_or(false)
    }
}

/// Ensure a `PermissionBridge` is bound to `session_id` on `plugin`.
/// First call creates a per-session Unix socket under
/// `tempdir/operon-permission-sockets/<session>.sock`, attaches a
/// handler that pushes prompts into `PERMISSION_PROMPTS`, and registers
/// the bridge with the plugin so subsequent `spawn_turn` calls add the
/// `--mcp-config` + `--permission-prompt-tool` flags. Subsequent calls
/// for the same `session_id` are no-ops.
///
/// Both the interactive companion chat and the headless artifact
/// runner call this after their own `plugin.bind_session(...)`.
#[cfg(not(target_arch = "wasm32"))]
pub async fn ensure_session_bridge(
    plugin: &operon_plugins_claude_code::ClaudeCodeChatPlugin,
    session_id: Uuid,
    cwd: PathBuf,
) -> std::io::Result<()> {
    use operon_plugins_claude_code::{
        PermissionBridge, PermissionDecision, PermissionRequest,
    };
    use tokio::sync::oneshot;

    if !session_bridges::claim(session_id) {
        return Ok(());
    }

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
        park_permission_responder(id.clone(), respond);
        push_permission_prompt(PermissionPromptEntry {
            id,
            tool_name: req.tool_name,
            input: req.input,
            source_session: Some(session_id),
            source_cwd: Some(cwd_for_handler.clone()),
        });
    };

    let bridge = PermissionBridge::bind(socket, handler).await?;
    plugin.set_session_bridge(session_id, Some(std::sync::Arc::new(bridge)));
    Ok(())
}
