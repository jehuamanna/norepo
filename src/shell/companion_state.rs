//! Cross-cutting state shared between the explorer / project list, the
//! companion-pane chat, and (in M2) skill / workflow plugins. The signals
//! here are provided in `local_mode::desktop` and consumed by various
//! components; this module is the one place the newtypes live so both
//! sides can import them without circular module deps.

use dioxus::prelude::Signal;
use dioxus::signals::GlobalSignal;
use std::collections::HashMap;
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
