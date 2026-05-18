//! Startup glue for the in-tree MCP bridge.
//!
//! The bridge server lives in `operon-bridge` (a workspace crate).
//! This module spawns it on a dedicated std thread that owns a
//! current-thread tokio runtime — kept separate from Dioxus's runtime
//! so the bridge's lifetime is plain Drop semantics on the
//! [`BridgeContext`] held in the Dioxus context tree.
//!
//! Why a fresh thread and not `tokio::spawn` onto Dioxus's runtime:
//! - The bridge needs `Server::serve` to block forever in an async
//!   accept loop. That's fine on its own runtime; on a shared
//!   runtime it would compete for the same worker slots as Dioxus's
//!   event loop on every accepted connection.
//! - Shutdown semantics stay obvious: drop `BridgeContext` → oneshot
//!   fires → server task exits → runtime drops → thread exits.
//! - The bridge doesn't need to interact with Dioxus signals; it
//!   reads from registered `ToolHandler`s instead, which already
//!   handle their own cross-thread access (`Arc`+`tokio::sync`).
//!
//! Socket placement: `$XDG_RUNTIME_DIR/operon-bridge-<pid>.sock` (or
//! `/tmp/operon-bridge-<pid>.sock` if the env var isn't set). The
//! socket path lives outside the vault deliberately — unix sockets
//! have a ~108-byte path limit on Linux that long vault paths blow
//! through. The vault-local artifact for discovery (an optional
//! "bridge.lock" pointer file) is deferred to a later milestone; we
//! don't need it because Operon injects the path + token directly
//! into the env of every `claude` it spawns.

#![cfg(all(unix, not(target_arch = "wasm32")))]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use operon_bridge::Server;
use operon_store::repos::{
    LocalAttachmentRepository, LocalNoteLinkRepository, LocalNoteRepository,
    LocalProjectRepository, LocalSearchRepository,
};

use crate::persistence::Persistence;

/// Bundle of GUI-side data sources the bridge's tool handlers need
/// to do their jobs. Each field is an `Arc` so the bundle is cheap
/// to clone into each tool's constructor.
///
/// Constructed in `App` (`src/app.rs`) right after `Persistence` is
/// registered — that's the earliest point where every field is
/// available. `provide_local_state` registers the repos; `App`
/// registers `Persistence`; `provide_bridge_runtime` (this module)
/// glues them together.
#[derive(Clone)]
pub struct BridgeRepos {
    pub note_repo: Arc<dyn LocalNoteRepository>,
    pub persistence: Arc<dyn Persistence>,
    /// Optional because some bootstrap paths (notably tests) install
    /// a notes repo without a projects repo. Tools that need to
    /// resolve a project from a note id degrade to "skip" when this
    /// is None.
    pub project_repo: Option<Arc<dyn LocalProjectRepository>>,
    /// Wikilink graph repo — required by `crawl_note_graph` to walk
    /// the precomputed link table the save pipeline maintains. Same
    /// instance the editor's save path writes to, so a crawl
    /// reflects the current vault without re-parsing bodies.
    pub link_repo: Arc<dyn LocalNoteLinkRepository>,
    /// Local-mode attachment metadata repo — `list_attachments` /
    /// `delete_attachment` / `attach_image_to_note` read and write
    /// through this. FKs to `local_note(id)`, so attachments cascade-
    /// delete when the host note is removed. Blobs themselves go
    /// through `crate::local_mode::images` (content-addressed under
    /// `<vault>/.operon/images/`).
    pub attachment_repo: Arc<dyn LocalAttachmentRepository>,
    /// Search repo — `search_notes` uses this when `in_content: true`
    /// is passed (otherwise it falls back to an in-memory title scan
    /// via the note_repo).
    pub search_repo: Arc<dyn LocalSearchRepository>,
    /// Snapshot of the active vault root at bridge startup. `None`
    /// when no vault is configured (rare — image tools and
    /// `get_vault_info` return helpful errors in that state).
    /// Vault rebinds require a GUI restart in practice, so a startup
    /// snapshot tracks live state well enough.
    pub vault_root: Option<crate::local_mode::vault::VaultRoot>,
    /// Sender for UI mutations that have to land on the Dioxus
    /// thread. See [`BridgeUiCommand`] for the contract.
    pub ui: BridgeUiSender,
}

/// Commands posted from bridge tools (running on the bridge thread)
/// to the Dioxus side. Each variant is something the bridge cannot
/// do itself because it requires a Dioxus runtime guard — primarily
/// GlobalSignal writes. The drain task (spawned by
/// [`provide_bridge_runtime`] via `dioxus::prelude::spawn`) loops on
/// the receiver and applies them on the Dioxus runtime.
///
/// Loose-end M4c.9: proven necessary by
/// `companion_state::tests::global_signal_write_from_thread_without_dioxus_runtime`.
/// Direct `LOCAL_NOTE_VERSION.write()` or `push_ask_user_prompt(...)`
/// from the bridge thread panics with "Must be called from inside a
/// Dioxus runtime"; routing them here fixes that for every tool.
#[derive(Debug)]
pub enum BridgeUiCommand {
    /// Bump `LOCAL_NOTE_VERSION` so the explorer + open editors
    /// re-read note state. Issued by create_note / append_note /
    /// replace_note_range after a successful save.
    BumpNoteVersion,
    /// Show an ask_user picker card in the companion chat surface.
    PushAskUserPrompt(crate::shell::companion_state::AskUserPromptEntry),
    /// Show a proposed-edit diff card (from `replace_note_range`
    /// with confirm:true).
    PushNoteProposal(crate::shell::companion_state::NoteProposalEntry),
    /// Show a deletion-confirm card for `delete_note`. The tool
    /// blocks on the responder until the user accepts or rejects.
    PushNoteDeletionProposal(crate::shell::companion_state::NoteDeletionProposalEntry),
    /// Open / focus a note tab in the editor pane. Fire-and-forget
    /// from the tool's perspective; the handler resolves title +
    /// body + kind from context and calls
    /// `crate::local_mode::editor::open_local_note_tab`.
    FocusNote(uuid::Uuid),
}

/// Cheap-to-clone sender half. Tools hold one of these in their
/// captured `BridgeRepos`. Send is `&self` (no `.lock()` needed) and
/// never blocks — the channel is unbounded; low-frequency commands
/// don't justify backpressure.
#[derive(Clone)]
pub struct BridgeUiSender {
    tx: tokio::sync::mpsc::UnboundedSender<BridgeUiCommand>,
}

impl BridgeUiSender {
    pub fn send(&self, cmd: BridgeUiCommand) {
        if self.tx.send(cmd).is_err() {
            // Drain task has been dropped — the GUI is tearing down.
            // The command is meaningless without a UI to apply it to,
            // so log + swallow.
            tracing::warn!(
                target: "operon::bridge",
                "UI command dropped — drain task gone (app shutdown?)"
            );
        }
    }
}

/// Create a fresh channel pair. The sender lives in `BridgeRepos`
/// (and through that, in every tool's captured state); the receiver
/// is owned by the Dioxus-side drain task spawned in
/// [`crate::local_mode::desktop::provide_bridge_runtime`].
pub fn make_ui_channel() -> (
    BridgeUiSender,
    tokio::sync::mpsc::UnboundedReceiver<BridgeUiCommand>,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    (BridgeUiSender { tx }, rx)
}

/// Apply one [`BridgeUiCommand`] on the Dioxus side. Called from the
/// drain task spawned by `provide_bridge_runtime`; runs under the
/// Dioxus runtime guard so GlobalSignal writes are safe.
pub fn apply_bridge_ui_command(cmd: BridgeUiCommand) {
    match cmd {
        BridgeUiCommand::BumpNoteVersion => {
            *crate::shell::companion_state::LOCAL_NOTE_VERSION.write() += 1;
        }
        BridgeUiCommand::PushAskUserPrompt(entry) => {
            crate::shell::companion_state::push_ask_user_prompt(entry);
        }
        BridgeUiCommand::PushNoteProposal(entry) => {
            crate::shell::companion_state::push_note_proposal(entry);
        }
        BridgeUiCommand::PushNoteDeletionProposal(entry) => {
            crate::shell::companion_state::push_note_deletion_proposal(entry);
        }
        BridgeUiCommand::FocusNote(note_id) => {
            // Resolve title + body + kind from Dioxus context (the
            // drain task runs under the runtime guard so use_context
            // is valid here), then forward to the editor's
            // open-tab helper. Best-effort: a missing note repo,
            // unreadable body, or absent TabManager context all log
            // and silently skip — the tool returns success either
            // way because the user-visible failure mode is "the
            // tab doesn't open" which is benign.
            use dioxus::prelude::{try_consume_context, use_context};
            let Some(note_repo_wrapper) = try_consume_context::<
                crate::local_mode::desktop::LocalNoteRepo,
            >() else {
                tracing::warn!(target: "operon::bridge", "FocusNote: LocalNoteRepo context missing");
                return;
            };
            let note_repo = note_repo_wrapper.0;
            let pid = match note_repo.find_project_for_note(note_id) {
                Ok(Some(pid)) => pid,
                _ => {
                    tracing::warn!(target: "operon::bridge", note=%note_id, "FocusNote: project for note not found");
                    return;
                }
            };
            let (title, kind) = match note_repo.list_for_project(pid) {
                Ok(rows) => match rows.into_iter().find(|r| r.id == note_id) {
                    Some(n) => (n.title, n.kind),
                    None => {
                        tracing::warn!(target: "operon::bridge", note=%note_id, "FocusNote: note row not in project list");
                        return;
                    }
                },
                Err(e) => {
                    tracing::warn!(target: "operon::bridge", error=%e, "FocusNote: list_for_project failed");
                    return;
                }
            };
            let persistence: std::sync::Arc<dyn crate::persistence::Persistence> = use_context();
            let body = futures::executor::block_on(persistence.load(&note_id.to_string()))
                .ok()
                .and_then(|b| String::from_utf8(b).ok())
                .unwrap_or_default();
            // TabManager + SaveScheduler are both registered at the
            // shell scope; they're in scope here because the drain
            // task is spawned from `provide_bridge_runtime` which
            // also runs at that scope.
            let tabs: dioxus::prelude::Signal<crate::tabs::TabManager> = use_context();
            let save_scheduler: crate::tabs::SaveScheduler = use_context();
            let _ = crate::local_mode::editor::open_local_note_tab(
                tabs,
                save_scheduler,
                note_id,
                title,
                body,
                kind,
            );
        }
    }
}

/// Resolved runtime info for the live bridge: where the socket is,
/// what token to authenticate with, and where the per-spawn
/// `--mcp-config <path>` file lives. Cloned into every Claude spawn
/// site via the [`BridgeContext`] wrapper below.
pub struct BridgeRuntime {
    pub sock_path: PathBuf,
    pub token: String,
    /// Tempfile written at startup containing the `.mcp.json` Claude
    /// reads via `--mcp-config <path>`. Holds the `operon` MCP
    /// server entry pointing at the `operon-mcp` stub binary, with
    /// the sock + token in its `env` block so the stub can
    /// authenticate. `Option` so failures to resolve/write degrade
    /// gracefully — the bridge still runs, but terminal-mode Claude
    /// won't discover any of its tools without this config.
    pub mcp_config_path: Option<PathBuf>,
    /// Drop sends a shutdown notification to the bridge thread, which
    /// closes the accept loop and unlinks the socket file. `Option`
    /// because Drop needs to `take()` it (oneshot::Sender::send is
    /// by-value).
    shutdown: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl Drop for BridgeRuntime {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.shutdown.lock() {
            if let Some(tx) = guard.take() {
                let _ = tx.send(());
            }
        }
        // Best-effort cleanup of the .mcp.json tempfile. The
        // socket file is unlinked by the bridge thread's accept-
        // loop teardown; the mcp config is ours.
        if let Some(p) = &self.mcp_config_path {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Dioxus context wrapper. `Clone` is required by the context system;
/// the inner `Arc` keeps shutdown single-fire (Drop on the last Arc
/// triggers it once).
#[derive(Clone)]
pub struct BridgeContext(pub Arc<BridgeRuntime>);

/// Start the bridge on a dedicated thread and return its runtime
/// info. Blocks the caller only until the socket is bound + the
/// token-rejection / accept loop is live, which is a few hundred
/// microseconds in practice — fine to call from a render hook.
///
/// `repos` carries the GUI-side data sources tool handlers need.
/// Currently consumed by `OperonGetNoteTool`, `OperonListNotesTool`,
/// `OperonSearchNotesTool`; `OperonAskUserTool` is stateless (uses
/// the `ASK_USER_PROMPTS` GlobalSignal directly) and ignores it.
///
/// Failures here are non-fatal to the app: the caller should log
/// the error and skip context registration. Companion features that
/// need the bridge degrade gracefully (the toolbar Send-to-Claude
/// button still works because it routes through the in-process
/// chat composer, not the bridge).
pub fn start_bridge_runtime(repos: BridgeRepos) -> Result<BridgeRuntime, String> {
    let sock_path = pick_socket_path();
    // Best-effort cleanup of a stale socket from a previous crashed
    // process under the same pid (rare on Linux because pids
    // recycle slowly, but cheap insurance). The server also
    // unlinks before `bind`, so this is belt-and-braces.
    let _ = std::fs::remove_file(&sock_path);

    let token = uuid::Uuid::new_v4().to_string();

    // Ready signal carries the bind result back. `sync_channel(1)`
    // because the producer (the new thread) sends exactly once;
    // the consumer (this fn) receives once.
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let sock_for_thread = sock_path.clone();
    let token_for_thread = token.clone();

    std::thread::Builder::new()
        .name("operon-bridge".into())
        .spawn(move || {
            // Single-threaded runtime: the bridge's workload is
            // socket I/O and JSON parsing — nothing that benefits
            // from work-stealing across cores.
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("tokio runtime: {e}")));
                    return;
                }
            };
            rt.block_on(async move {
                // M4b.5: register the cross-transport `ask_user` tool.
                // Same wire shape as the chat-mode in-process executor
                // — see `crate::shell::bridge_ask_user_tool` for the
                // payload contract.
                //
                // M4c: read-only note tools follow. Each gets its own
                // clone of `repos`; the Arc-fields keep that cheap.
                let server = Server::new(sock_for_thread, token_for_thread)
                    .register_tool(Arc::new(
                        crate::shell::bridge_ask_user_tool::OperonAskUserTool::new(
                            repos.ui.clone(),
                        ),
                    ))
                    .register_tool(Arc::new(
                        crate::shell::bridge_note_tools::OperonGetNoteTool::new(
                            repos.clone(),
                        ),
                    ))
                    .register_tool(Arc::new(
                        crate::shell::bridge_note_tools::OperonListNotesTool::new(
                            repos.clone(),
                        ),
                    ))
                    .register_tool(Arc::new(
                        crate::shell::bridge_note_tools::OperonSearchNotesTool::new(
                            repos.clone(),
                        ),
                    ))
                    // M4c.4/.5: write tools. Both go through
                    // `LocalNoteRepository` + `Persistence`; the GUI
                    // sees the change live via `LOCAL_NOTE_VERSION`.
                    .register_tool(Arc::new(
                        crate::shell::bridge_note_tools::OperonCreateNoteTool::new(
                            repos.clone(),
                        ),
                    ))
                    .register_tool(Arc::new(
                        crate::shell::bridge_note_tools::OperonAppendNoteTool::new(
                            repos.clone(),
                        ),
                    ))
                    .register_tool(Arc::new(
                        crate::shell::bridge_note_tools::OperonReplaceNoteRangeTool::new(
                            repos.clone(),
                        ),
                    ))
                    // BFS over the precomputed link graph. One round
                    // trip instead of N for "everything reachable
                    // from this note".
                    .register_tool(Arc::new(
                        crate::shell::bridge_note_tools::OperonCrawlNoteGraphTool::new(
                            repos.clone(),
                        ),
                    ))
                    // Project / metadata / tree ops.
                    .register_tool(Arc::new(
                        crate::shell::bridge_note_tools::OperonListProjectsTool::new(
                            repos.clone(),
                        ),
                    ))
                    .register_tool(Arc::new(
                        crate::shell::bridge_note_tools::OperonDeleteNoteTool::new(repos.clone()),
                    ))
                    .register_tool(Arc::new(
                        crate::shell::bridge_note_tools::OperonRenameNoteTool::new(repos.clone()),
                    ))
                    .register_tool(Arc::new(
                        crate::shell::bridge_note_tools::OperonListRecentNotesTool::new(
                            repos.clone(),
                        ),
                    ))
                    .register_tool(Arc::new(
                        crate::shell::bridge_note_tools::OperonGetVaultInfoTool::new(
                            repos.clone(),
                        ),
                    ))
                    .register_tool(Arc::new(
                        crate::shell::bridge_note_tools::OperonReorderNoteTool::new(
                            repos.clone(),
                        ),
                    ))
                    .register_tool(Arc::new(
                        crate::shell::bridge_note_tools::OperonMoveNoteTool::new(repos.clone()),
                    ))
                    .register_tool(Arc::new(
                        crate::shell::bridge_note_tools::OperonOpenNoteTool::new(repos.clone()),
                    ))
                    // Image / screenshot ingress + attachment ops.
                    // Standalone image notes plus per-note attachments
                    // (the latter back-ended by the local-mode
                    // `local_attachments` table FK'd to `local_note(id)`
                    // — see migration 023).
                    .register_tool(Arc::new(
                        crate::shell::bridge_note_tools::OperonCreateImageNoteTool::new(
                            repos.clone(),
                        ),
                    ))
                    .register_tool(Arc::new(
                        crate::shell::bridge_note_tools::OperonAttachImageToNoteTool::new(
                            repos.clone(),
                        ),
                    ))
                    .register_tool(Arc::new(
                        crate::shell::bridge_note_tools::OperonListAttachmentsTool::new(
                            repos.clone(),
                        ),
                    ))
                    .register_tool(Arc::new(
                        crate::shell::bridge_note_tools::OperonDeleteAttachmentTool::new(
                            repos.clone(),
                        ),
                    ))
                    // Project CRUD + seed-skill installation. None of
                    // these existed pre-`create_project` — the bridge
                    // could read projects via `list_projects` but
                    // never mutate them, so a companion-driven
                    // bootstrap had to fall back to GUI clicks for
                    // every "create a project + bind a repo + load
                    // the SDLC chain" step.
                    .register_tool(Arc::new(
                        crate::shell::bridge_note_tools::OperonCreateProjectTool::new(
                            repos.clone(),
                        ),
                    ))
                    .register_tool(Arc::new(
                        crate::shell::bridge_note_tools::OperonUpdateProjectTool::new(
                            repos.clone(),
                        ),
                    ))
                    .register_tool(Arc::new(
                        crate::shell::bridge_note_tools::OperonDeleteProjectTool::new(
                            repos.clone(),
                        ),
                    ))
                    .register_tool(Arc::new(
                        crate::shell::bridge_note_tools::OperonInstallSeedSkillsTool::new(
                            repos.clone(),
                        ),
                    ))
                    .register_tool(Arc::new(
                        crate::shell::bridge_note_tools::OperonMaterializeSkillsToDiskTool::new(
                            repos.clone(),
                        ),
                    ));
                let handle = match server.serve().await {
                    Ok(h) => h,
                    Err(e) => {
                        let _ = ready_tx.send(Err(format!("serve: {e}")));
                        return;
                    }
                };
                // Bind succeeded — wake the caller.
                let _ = ready_tx.send(Ok(()));
                // Park until the GUI drops the BridgeRuntime, then
                // tear down the accept loop. Errors here mean the
                // sender was dropped without sending, which is also
                // a shutdown signal.
                let _ = shutdown_rx.await;
                handle.shutdown().await;
            });
        })
        .map_err(|e| format!("spawn bridge thread: {e}"))?;

    // Wait for the bind result. Use a recv with timeout so a bug in
    // the spawn path doesn't deadlock the app at startup.
    let bind_result = ready_rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .map_err(|e| format!("bridge thread did not signal ready: {e}"))?;
    bind_result?;

    // Write the .mcp.json that terminal-mode Claude reads via
    // `--mcp-config <path>`. Best-effort: a missing operon-mcp
    // binary or unwritable runtime dir just means terminal-mode
    // Claude won't have our tools — the rest of the GUI still works.
    let mcp_config_path = match write_mcp_config(&sock_path, &token) {
        Ok(p) => {
            tracing::info!(
                target: "operon::bridge",
                config = %p.display(),
                "wrote operon-bridge .mcp.json"
            );
            Some(p)
        }
        Err(e) => {
            tracing::warn!(
                target: "operon::bridge",
                error = %e,
                "could not write operon-bridge .mcp.json; \
                 terminal-mode Claude will not discover bridge tools"
            );
            None
        }
    };

    tracing::info!(
        target: "operon::bridge",
        socket = %sock_path.display(),
        "operon-bridge started"
    );

    Ok(BridgeRuntime {
        sock_path,
        token,
        mcp_config_path,
        shutdown: std::sync::Mutex::new(Some(shutdown_tx)),
    })
}

/// MCP server name our bridge advertises itself under, both in the
/// per-process `.mcp.json` (terminal mode) and in the chat plugin's
/// extra-servers map (chat mode). Tool names are prefixed
/// `mcp__operon_notes__<tool>`. Distinct from the chat-mode
/// permission_bridge's `operon` so the two can coexist in chat-mode
/// claude.
pub const BRIDGE_SERVER_NAME: &str = "operon_notes";

/// Build the MCP server entry (the value side of the `mcpServers`
/// map keyed by `BRIDGE_SERVER_NAME`). Exposed publicly so the chat
/// plugin's `set_extra_mcp_servers` integration can compose it into
/// the chat-mode `.mcp.json` without re-doing the path resolution
/// or env-block construction.
pub fn server_entry_value(
    mcp_bin: &Path,
    mcp_args: &[String],
    sock_path: &Path,
    token: &str,
) -> serde_json::Value {
    serde_json::json!({
        "type": "stdio",
        "command": mcp_bin.to_string_lossy(),
        "args": mcp_args,
        "env": {
            "OPERON_BRIDGE_SOCK": sock_path.to_string_lossy(),
            "OPERON_BRIDGE_TOKEN": token,
        },
    })
}

impl BridgeRuntime {
    /// Compose the chat-mode MCP server entry from this runtime's
    /// state. Returns the bare server config object — the caller
    /// keys it under `BRIDGE_SERVER_NAME` (or whatever name they
    /// want chat-mode to use). `None` when the operon-mcp binary
    /// can't be resolved; chat-mode degrades to its existing
    /// in-process tools only.
    pub fn chat_mode_mcp_entry(&self) -> Option<(&'static str, serde_json::Value)> {
        let (mcp_bin, mcp_args) = resolve_operon_mcp_bin().ok()?;
        Some((
            BRIDGE_SERVER_NAME,
            server_entry_value(&mcp_bin, &mcp_args, &self.sock_path, &self.token),
        ))
    }
}

/// Build + write the `.mcp.json` Claude reads via `--mcp-config`. The
/// file pins the `operon_notes` MCP server to the `operon-mcp` binary
/// with the bridge sock + token in the env block, so the stub
/// authenticates on connect without needing claude to inherit env
/// from the GUI.
///
/// Path lives alongside the socket in `$XDG_RUNTIME_DIR` (or
/// `$TMPDIR`) so a crash leaves at most two files behind, both keyed
/// by pid so a restart claims fresh names.
fn write_mcp_config(sock_path: &Path, token: &str) -> Result<PathBuf, String> {
    let (mcp_bin, mcp_args) = resolve_operon_mcp_bin()?;
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .unwrap_or_else(std::env::temp_dir);
    let path = dir.join(format!("operon-bridge-{}.mcp.json", std::process::id()));

    // Server name is `operon_notes` (NOT `operon`) so the bridge
    // can coexist with the chat-mode in-process `permission_bridge`
    // — which owns `operon` for its `ask_user`/`permission_prompt`
    // tools. Tools surface as `mcp__operon_notes__<tool>`. Terminal
    // mode only loads this config (no permission_bridge); chat mode
    // loads both (see `ClaudeCodeChatPlugin::set_extra_mcp_servers`).
    let config = serde_json::json!({
        "mcpServers": {
            BRIDGE_SERVER_NAME: server_entry_value(&mcp_bin, &mcp_args, sock_path, token)
        }
    });
    let body = serde_json::to_string_pretty(&config)
        .map_err(|e| format!("serialize mcp config: {e}"))?;
    std::fs::write(&path, body).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path)
}

/// Locate the `operon-mcp` stub and the args needed to invoke it.
/// Resolution order:
/// 1. `OPERON_MCP_BIN` env var — explicit override that points at a
///    standalone `operon-mcp` binary (legacy / dev escape hatch). No
///    extra args.
/// 2. The running `operon-dioxus` executable itself, invoked with
///    `--operon-mcp` so `main()` dispatches into the embedded stub.
///    This is the production path: the release bundle ships a single
///    binary and the stub is "inbuilt" rather than a sidecar.
///
/// Returns `(path, args)`.
fn resolve_operon_mcp_bin() -> Result<(PathBuf, Vec<String>), String> {
    if let Some(explicit) = std::env::var_os("OPERON_MCP_BIN") {
        let p = PathBuf::from(explicit);
        if p.exists() {
            return Ok((p, Vec::new()));
        }
        return Err(format!(
            "OPERON_MCP_BIN set to {} but the file does not exist",
            p.display()
        ));
    }

    let exe = std::env::current_exe()
        .map_err(|e| format!("current_exe: {e}"))?;
    Ok((exe, vec!["--operon-mcp".to_string()]))
}

fn pick_socket_path() -> PathBuf {
    // XDG_RUNTIME_DIR is tmpfs-backed on most Linux distros and per-
    // user-mode-700 — exactly what we want for socket placement.
    // Falls back to /tmp on macOS / older systems where the env var
    // isn't set. Pid in the filename so multiple Operon instances on
    // the same machine don't collide.
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .unwrap_or_else(std::env::temp_dir);
    dir.join(format!("operon-bridge-{}.sock", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_uses_xdg_runtime_dir_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::var_os("XDG_RUNTIME_DIR");
        // SAFETY: this test runs single-threaded — env mutation
        // ordering across other tests would otherwise be undefined.
        // We don't run parallel tests in this file.
        std::env::set_var("XDG_RUNTIME_DIR", tmp.path());
        let p = pick_socket_path();
        assert!(p.starts_with(tmp.path()), "got {}", p.display());
        match prev {
            Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
            None => std::env::remove_var("XDG_RUNTIME_DIR"),
        }
    }

    /// Test-only `BridgeRepos` populated with minimal stubs. The
    /// startup test only verifies bind + socket cleanup; no tool
    /// handler is invoked, so the repos are never dereferenced. We
    /// build them via the real SQLite-backed impls against
    /// `Store::for_test()` (in-memory) — same pattern other
    /// store-level tests in the workspace use.
    fn dummy_repos() -> BridgeRepos {
        use operon_store::repos::{
            SqliteLocalAttachmentRepository, SqliteLocalNoteLinkRepository,
            SqliteLocalNoteRepository, SqliteLocalProjectRepository,
            SqliteLocalSearchRepository,
        };
        use operon_store::Store;

        let store = Store::for_test().expect("in-memory store");
        let note_repo: Arc<dyn LocalNoteRepository> =
            Arc::new(SqliteLocalNoteRepository::new(store.clone()));
        let project_repo: Arc<dyn LocalProjectRepository> =
            Arc::new(SqliteLocalProjectRepository::new(store.clone()));
        let link_repo: Arc<dyn LocalNoteLinkRepository> =
            Arc::new(SqliteLocalNoteLinkRepository::new(store.clone()));
        let attachment_repo: Arc<dyn LocalAttachmentRepository> =
            Arc::new(SqliteLocalAttachmentRepository::new(store.clone()));
        let search_repo: Arc<dyn LocalSearchRepository> =
            Arc::new(SqliteLocalSearchRepository::new(store));
        // Drop the receiver — the test exercises bind + drop only;
        // no tool calls run so no UI commands are emitted.
        let (ui, _rx) = make_ui_channel();
        BridgeRepos {
            note_repo,
            persistence: Arc::new(crate::persistence::MemoryPersistence::new()),
            project_repo: Some(project_repo),
            link_repo,
            attachment_repo,
            search_repo,
            vault_root: None,
            ui,
        }
    }

    #[test]
    fn start_and_drop_runs_cleanly() {
        let prev = std::env::var_os("XDG_RUNTIME_DIR");
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_RUNTIME_DIR", tmp.path());

        let rt = start_bridge_runtime(dummy_repos()).expect("start");
        let sock = rt.sock_path.clone();
        assert!(sock.exists(), "socket file should exist while running");
        drop(rt);
        // Drop triggers shutdown asynchronously on the bridge thread.
        // Give it a moment to unlink the socket file.
        for _ in 0..50 {
            if !sock.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            !sock.exists(),
            "socket file should be removed after drop: {}",
            sock.display()
        );

        match prev {
            Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
            None => std::env::remove_var("XDG_RUNTIME_DIR"),
        }
    }
}
