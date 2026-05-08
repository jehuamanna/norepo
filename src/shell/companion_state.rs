//! Cross-cutting state shared between the explorer / project list, the
//! companion-pane chat, and (in M2) skill / workflow plugins. The signals
//! here are provided in `local_mode::desktop` and consumed by various
//! components; this module is the one place the newtypes live so both
//! sides can import them without circular module deps.

use dioxus::prelude::*;
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

/// One-shot inbox the companion's composer subscribes to. When a remote
/// caller (e.g., the skill plugin's Play button) writes `Some(prompt)`,
/// the companion swaps that text into its composer field on the next
/// render and clears the signal. The user reviews + clicks Send.
#[derive(Clone, Copy)]
pub struct CompanionComposerInbox(pub Signal<Option<String>>);
