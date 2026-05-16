# Companion ↔ Claude Code: Note interaction plan

> Status: draft, 2026-05-16. Author: Jehu.
> Inspired by `claudecode.nvim` (PROTOCOL.md / lockfile + WebSocket pattern), but adapted
> for the Operon-dioxus reality: notes are SQLite-backed, the running app already owns
> a Claude subprocess, and we have both a rich-chat surface and a raw PTY terminal.

---

## 0. Shipped state (2026-05-16 end-of-session)

All eight wishlist items from §1 are functionally covered in both chat and
terminal modes (where applicable). Below: per-milestone status, code pointers,
the constraints we hit and how we worked around them, and what's left.

### Milestones

| ID | Description | Code | Status |
|---|---|---|---|
| **M4a** | Send-to-Claude toolbar button | `src/shell/mode_toolbar.rs::build_send_to_claude_cluster` | ✓ |
| **M4b.1–.3** | `operon-bridge` crate (unix socket + JSON-RPC + MCP dispatch + `operon-mcp` stub) | `crates/operon-bridge/` | ✓ |
| **M4b.4** | Lockfile + env injection at PTY spawn | `src/local_mode/bridge_runtime.rs`, `companion_terminal.rs::get_or_create_session` | ✓ |
| **M4b.5** | `operon_ask_user` over the bridge + `.mcp.json` write | `src/shell/bridge_ask_user_tool.rs` | ✓ |
| **M4c.0** | `BridgeRepos` plumbing | `bridge_runtime.rs::BridgeRepos` | ✓ |
| **M4c.1–.3** | Read tools (`get_note`, `list_notes`, `search_notes`) | `src/shell/bridge_note_tools.rs` | ✓ |
| **M4c.4–.5** | Write tools (`create_note`, `append_note`) | same file | ✓ |
| **M4c.6** | `replace_note_range` (eager, anchor-based) | same file | ✓ |
| **M4c.7** | `replace_note_range` (confirm + diff card) | same file + `src/shell/note_proposal_card.rs` | ✓ |
| **M4c.8** | System-prompt hint (terminal mode) | `companion_terminal.rs::OPERON_TOOLS_SYSTEM_PROMPT` | ✓ |
| **M4c.9** | Channel-route GlobalSignal writes from bridge thread | `bridge_runtime.rs::BridgeUiCommand` + drain task in `desktop.rs::provide_bridge_runtime` | ✓ |
| **M4d.1** | Terminal-mode Send-to-Claude (PTY injection) | `companion_state.rs::PENDING_TERMINAL_INJECTION` + drain in `ClaudeRepoTerminal` | ✓ |
| **M4d.2** | Drag-drop notes onto terminal pane | `ClaudeRepoTerminal::ondrop` | ✓ |
| **M4d.3** | Paste interception (UUID → mention) | xterm bootstrap `addEventListener('paste', …, true)` | ✓ |
| **M4d.4** | `@`-picker keydown in xterm + keyboard nav | `src/shell/terminal_mention_picker.rs` | ✓ |
| **M4d-selection** | Send-selection toolbar button | `mode_toolbar.rs` + active-monaco focus tracking in `editor_host.rs` | ✓ |
| **M4e** | Bulk-send (multi-select context menu) | `local_mode/explorer/note_row.rs` + multi-mention consumer in `companion_chat.rs` | ✓ |
| **Chat-mode bridge** | All 6 note tools available in chat mode too | `ClaudeCodeChatPlugin::set_extra_mcp_servers` + wiring in `desktop.rs` | ✓ |
| **Chat-mode prompt** | System prompt lists `operon_notes` tools when wired | `plugin.rs::spawn_turn` conditional | ✓ |

### Wishlist coverage (§1 mapping)

| # | Item | Surfaces |
|---|---|---|
| 1 | Run claude in terminal | Pre-M4 (`companion_terminal.rs`) |
| 2 | Talk to claude in terminal | Pre-M4 |
| 3 | Notes via @ / drag / paste | M4d.4 / M4d.2 / M4d.3 (terminal); M4a + chat composer (chat) |
| 4 | Claude modifies repo + note files | M4c (note tools) + native Edit/Write |
| 5 | Send single note | M4a + M4d.1 |
| 6 | Send highlighted selection | M4d-selection |
| 7 | Bulk add notes | M4e |
| 8 | External MCP plugins | Pre-existing `mcp_settings/` (project-scope `.mcp.json` is auto-loaded by claude alongside our `--mcp-config`) |

### Constraints discovered along the way

These are non-obvious facts the codebase now lives with — load-bearing
context for future maintainers.

#### A. GlobalSignal writes panic from non-Dioxus threads (M4c.9)

`*LOCAL_NOTE_VERSION.write() += 1` from any thread without a Dioxus runtime
guard panics with `Must be called from inside a Dioxus runtime`. The bridge
thread runs its own tokio runtime, never a Dioxus one — so every tool that
needs to bump a UI signal **must** go through the `BridgeUiCommand` channel.

- Probe test: `companion_state::tests::global_signal_write_from_thread_without_dioxus_runtime_panics` (#[should_panic]) — documents the constraint as a guardrail.
- Channel definition: `bridge_runtime::BridgeUiCommand` + `BridgeUiSender`.
- Drain: `provide_bridge_runtime` spawns a `use_hook(|| spawn(async { while let Some(cmd) = rx.recv().await { apply_bridge_ui_command(cmd) } }))`.
- The drain task runs under Dioxus's runtime guard, so the apply path is safe.

If Dioxus ever loosens the guard requirement, the `#[should_panic]` test starts
failing and the channel layer becomes optional.

#### B. `Persistence::load` futures are `!Send` (M4c.1)

The trait deliberately erases concrete `Send`-ness so the wasm `WebPersistence`
can hold `JsValue` handles. Bridge tools' `async fn call` must be `Send` because
`operon-bridge::Server` spawns connection handlers with `tokio::spawn`.

Workaround: every read of `persistence.load(...)` / `persistence.save(...)` is
wrapped in `tokio::task::spawn_blocking` + `futures::executor::block_on`. The
`!Send` future is created, polled, and dropped on a single blocking-pool
thread; only the resulting `Vec<u8>` crosses a boundary, which is `Send`.

Affected: `OperonGetNoteTool::call`, `OperonAppendNoteTool::call`,
`OperonReplaceNoteRangeTool::call`, `OperonCreateNoteTool::call`.

#### C. Server-name collision with chat-mode permission_bridge

Chat-mode's existing in-process `permission_bridge` advertises tools under
server name `operon` (`mcp__operon__ask_user`, `mcp__operon__permission_prompt`).
Our out-of-process bridge can't reuse that name in chat-mode-shared spawns.

Resolution: our bridge uses **`operon_notes`** as its server name everywhere
(constant `bridge_runtime::BRIDGE_SERVER_NAME`). Tools surface as
`mcp__operon_notes__*`. Both servers coexist cleanly in chat-mode claude.

#### D. `dx serve` binary layout

`operon-mcp` must live somewhere `bridge_runtime::resolve_operon_mcp_bin` can
find it. Search order:
1. `$OPERON_MCP_BIN` env var.
2. Sibling of `current_exe()`.
3. Up to 10 ancestors of the exe dir, checking `<ancestor>/operon-mcp`,
   `<ancestor>/debug/operon-mcp`, `<ancestor>/release/operon-mcp`.

For dev workflow with `dx serve`, ensure the bin is built first:
```sh
cargo build -p operon-bridge --bin operon-mcp
dx serve
```

Look for `wrote operon-bridge .mcp.json` in the boot log to confirm.

### Final tool surface

The bridge advertises **7 tools** to claude (both modes after the chat-mode
wiring):

| Tool | Purpose | Confirm flow? |
|---|---|---|
| `mcp__operon_notes__ask_user` | Structured-options picker (terminal mode only — chat mode uses the in-process `mcp__operon__ask_user`) | n/a |
| `mcp__operon_notes__get_note` | Read a note by uuid → `{id, title, kind, body, path}` | n/a |
| `mcp__operon_notes__list_notes` | List a project's notes | n/a |
| `mcp__operon_notes__search_notes` | Case-insensitive title substring search | n/a |
| `mcp__operon_notes__create_note` | New note + body via `LocalNoteRepository::create_with_kind` | n/a |
| `mcp__operon_notes__append_note` | Append to existing body | n/a |
| `mcp__operon_notes__replace_note_range` | Anchor-based find/replace (like `Edit`); `confirm:true` opens a diff card and blocks | Yes when `confirm:true` |

### End-to-end test plan

After every code change:
```sh
cargo build -p operon-bridge --bin operon-mcp
dx serve
```

#### Terminal mode

1. Switch companion to "Claude Code (terminal)".
2. In claude: `/mcp` — `operon_notes` should appear with all 7 tools.
3. Toolbar `Send to Claude` → token types at prompt.
4. Drag a note from explorer → token types at prompt.
5. Paste a UUID anywhere → transforms to `@[note:UUID](note:UUID) `.
6. Type `@` → picker opens; type-to-filter, ↑/↓, Enter to insert.
7. Multi-select notes in explorer → right-click `Send N notes to Claude`.
8. Toolbar `Send selection` (with text highlighted in a note editor) → focus-hint payload.
9. Ask claude: *"create a new note titled X with body Y"* → `create_note` fires; note appears in explorer.
10. Ask claude: *"rewrite the introduction in note &lt;uuid&gt; using replace_note_range with confirm:true"* → diff card appears; Accept persists, Reject errors back.

#### Chat mode

1. Switch companion to "Chat".
2. Ask claude: *"list the notes in project &lt;uuid&gt;"* → calls `list_notes` (per the chat-mode system prompt addition).
3. Mention a note via `@`-picker → body inlines as `--- referenced note ---`; claude responds **without** redundantly calling `get_note`.
4. Right-click multiple notes → `Send N notes to Claude` → chips appear in tray.
5. `ask_user` flows continue to use the in-process `mcp__operon__ask_user`.

### Open follow-ups

- **`@`-picker cursor anchoring** — picker is docked at top-left of the
  terminal pane today; anchoring at the xterm cursor pixel would feel more
  integrated. Needs xterm cell-size math + `term.element.getBoundingClientRect()`.
- **In-process bridge migration** — chat-mode's `permission_bridge` still runs
  in-process for `ask_user` + `permission_prompt`. Long-term, those tools
  could move to the operon-bridge too, retiring the in-process plumbing.
  Would unify all MCP tools under one server name.
- **Bridge introspection lockfile** — original plan §2.1 specced a
  `<vault>/.operon/bridge.lock` pointer file. We didn't ship it because env
  injection covers all current consumers; trivially added if an external tool
  ever wants to discover the socket without env access.
- **Chat-mode access to `ask_user` via the bridge** — for parity, chat-mode
  could route ask_user through `operon-bridge` too instead of having two
  parallel paths. Not urgent; both work.

---

## 1. What we want

User-facing capabilities, in priority order:

1. **Raw `claude` in the companion pane** — already shipped (`src/shell/companion_terminal.rs:86-423`). Stays as the default surface for power users.
2. **Send a note → companion** — one-click "Send to Claude" from a note's toolbar, both in chat mode and terminal mode.
3. **@-mention a note from the input box** — type `@`, fuzzy-pick a note by title, insert a stable reference Claude can resolve.
4. **Drag a note from the side bar onto the companion pane.**
5. **Paste a note id** — `operon://note/<uuid>` (or just the bare uuid) auto-expands into a mention.
6. **Send a single highlighted range** — select text in Monaco, "Send selection to Claude".
7. **Bulk add** — multi-select notes in the explorer, "Send N notes to Claude".
8. **Claude can read/write notes** — not just repo files. Edits go through the same SQLite + Loro path the editor uses, so they show up live.
9. **External MCP servers + plugins** — user-configured MCP servers (Atlassian, Figma, custom) work in both chat and terminal modes; current MCP settings panel (`src/shell/mcp_settings/`) drives them.

Non-goals (this iteration):
- Multiplexing more than one Claude session per pane.
- A standalone MCP server other apps can use. The bridge is single-instance, single-user.
- Live cursor-position broadcasting like claudecode.nvim's `selection_changed`. Operon is not an "IDE for claude" — we send explicitly, not implicitly.

## 2. Architecture: where the bridge lives

The hard problem is that **chat mode and terminal mode talk to different Claudes**.

- **Chat mode** uses `ClaudeCodeChatPlugin::send_rich()` — Claude runs as a `claude --print --stream-json` child whose stdio the Rust process owns. The `mcp__operon__ask_user` bridge works because it is *in-process*: the tool executor is a Rust struct passed into the SDK.
- **Terminal mode** launches `claude` interactively inside a PTY (`companion_terminal.rs:248-272`). That child reads its own config (`~/.claude/settings.json`, project `.mcp.json`) and is otherwise opaque to us. An in-process Rust executor is unreachable from it.

Borrowing from `claudecode.nvim`: ship a **small MCP server that the running Operon app advertises out-of-band**, and point Claude at it via stdio. The server runs in-tree as a thread inside the Operon app and exposes itself over a **unix domain socket** (Windows: a named pipe). Claude's stdio MCP client connects to it via a stub binary.

```
                                Operon GUI process
                       ┌────────────────────────────────────────┐
                       │ Dioxus shell + companion_state         │
                       │ ┌────────────────────────────────────┐ │
   chat mode  ─────────┤ │ in-process bridge (today)          │ │
                       │ │  BridgeAskUserExecutor             │ │
                       │ └────────────────────────────────────┘ │
                       │ ┌────────────────────────────────────┐ │
                       │ │ operon_bridge::Server   (NEW)      │◄┼──── unix socket
                       │ │  - resolves note ids               │ │     OPERON_BRIDGE_SOCK
                       │ │  - dispatches tool calls           │ │
                       │ │  - holds pending-mention queue     │ │
                       │ └────────────────────────────────────┘ │
                       └────────────────────────────────────────┘
                                       ▲
                                       │ unix socket, JSON-RPC framed
                                       │
                       ┌───────────────┴────────────────────────┐
                       │ operon-mcp (stub binary)               │
                       │  - stdio MCP server                    │
                       │  - forwards every JSON-RPC frame to    │
                       │    the GUI's bridge socket             │
                       └────────────────────────────────────────┘
                                       ▲
                                       │ stdio MCP (JSON-RPC 2.0)
                       ┌───────────────┴────────────────────────┐
                       │ claude CLI  (PTY child OR chat child)  │
                       └────────────────────────────────────────┘
```

Why a stub binary instead of having `claude` connect directly to the unix socket?
Claude's MCP server config is `{ "command": "...", "args": [...] }` — stdio only. The stub adapts stdio ↔ unix-socket so we don't need any change inside Claude Code itself. It is also the cleanest place to drop credentials and a per-spawn token.

### 2.1 Discovery + auth

Mirror `claudecode.nvim`'s lockfile shape:

- Operon writes `<vault>/.operon/bridge.lock` on app start with `{ "socket": "/run/user/<uid>/operon-bridge-<pid>.sock", "token": "<uuid>" }` and `chmod 600`.
- When Operon spawns `claude` (terminal or chat), it sets env:
  - `OPERON_BRIDGE_SOCK=/…/operon-bridge-<pid>.sock`
  - `OPERON_BRIDGE_TOKEN=<uuid>`
  - `OPERON_SESSION_ID=<uuid>` (so the bridge knows which session the call belongs to)
- It also injects a project-scoped `.mcp.json` (or merges into the existing one) that points at the `operon-mcp` stub.
- The stub reads those env vars, opens the socket, handshakes with the token, and tunnels JSON-RPC frames in both directions.

The token gates against *other* processes on the box probing the socket. The socket already gives us same-user isolation.

### 2.2 Why not a websocket like claudecode.nvim?

We don't need to support an external CLI that the user launches in a separate terminal. Operon always spawns its own Claude. Unix socket + env-var discovery is simpler than allocating a port + writing a discovery lockfile + auth header, and dodges localhost firewall prompts on macOS.

## 3. The tools Operon's MCP server exposes

| Tool | Direction | Purpose |
|---|---|---|
| `operon_get_note` | Claude → app | Read a note by uuid. Returns `{ id, title, kind, markdown, path? }`. |
| `operon_list_notes` | Claude → app | Tree listing under a project / parent. For bulk operations. |
| `operon_search_notes` | Claude → app | Fuzzy title + body search, returns `[ {id, title, snippet} ]`. |
| `operon_append_note` | Claude → app | Append markdown to a note. Goes through the same Loro doc the editor uses, so it's live. |
| `operon_replace_note_range` | Claude → app | Replace text by line/col or by string-anchor (`before`/`after`). |
| `operon_create_note` | Claude → app | Create a new note (kind, title, parent, body). |
| `operon_get_active_selection` | Claude → app | What the user has selected in the active editor right now. |
| `operon_get_pending_context` | Claude → app | Drain the *mention queue* — notes the user clicked "send" on. |
| `operon_ask_user` | Claude → app | Keep the existing picker. The terminal-mode bridge re-uses the same `ASK_USER_PROMPTS` machinery; the chat path keeps its in-process executor for backward compat. |

Plus pass-through:
- The user's configured external MCP servers (Atlassian, Figma, etc.) are *not* tunneled through us. We just make sure the merged `.mcp.json` we write for a spawn includes them so Claude can call them directly.

### 3.1 Note-write semantics

This is where we differ most from claudecode.nvim's `openDiff` blocking flow. Notes live in SQLite + Loro, not raw files. Two write modes:

- **Eager** (default for `operon_append_note`): the bridge applies the edit immediately, returns success. The editor's `MonacoChannel` picks up the doc change via the existing Loro subscription. Cheap, frequent, low-friction — feels like Claude typing into a buffer.
- **Proposed** (opt-in for `operon_replace_note_range` when `confirm: true`): the bridge stages the edit, opens a `diff_preview.rs` card in the chat surface or as a side panel for the active note, and **blocks the tool call until the user accepts/rejects** — same `oneshot::channel` + `park_*_responder` shape as `BridgeAskUserExecutor`. This is the analog of claudecode.nvim's coroutine-based deferred response.

Repo-file writes do *not* go through us — Claude uses its native `Edit`/`Write` tools, which already work inside the PTY child since `cwd` is set to the repo.

## 4. UI work (the bigger half)

### 4.1 Mention queue + composer

Backing store: a `GlobalSignal<Vec<Mention>>` named `PENDING_MENTIONS` in `companion_state.rs`, peer to `ASK_USER_PROMPTS`. A `Mention` is:

```rust
struct Mention {
    id: Uuid,                   // mention's own id, for removal
    note_id: Uuid,              // the note being mentioned
    note_title: String,         // cached for chips
    range: Option<TextRange>,   // optional: line/col span for selection mentions
    kind: MentionKind,          // ExplicitSend, AtMention, DragDrop, Paste, BulkAdd
    created_at: SystemTime,
}
```

How the queue is consumed:
- **Chat mode**: when the user hits Send, the composer reads the queue, materializes each as a `<note id=… title=…>...body...</note>` XML block prepended to the user message, and clears the queue.
- **Terminal mode**: we cannot prepend to stdin without confusing the user. Instead the composer renders a "📎 3 notes" chip above the xterm; pressing it injects a small marker like `<<operon:pending#3>>` into the input. The chip is also typed in literally by the user if they want. Claude (because the project `.mcp.json` registered our server) sees the marker and is instructed by the system prompt to call `operon_get_pending_context` to drain the queue. The terminal child needs no other change.

### 4.2 @-mention picker

- Hook `@` keydown in both the chat composer and the xterm host. xterm's `onKey` event fires before the byte reaches the PTY — we can intercept and pop a Dioxus floating panel anchored at the cursor's screen position (already computed by xterm for IME support).
- Picker: fuzzy match over `LocalNoteRepository::list_titles()`. Result inserts a chip in the composer / a `@note:<short-id>` token in the xterm input.
- Selection → `PENDING_MENTIONS` entry (kind = `AtMention`).
- File: `src/shell/mention_picker.rs` (NEW).

### 4.3 Drag and drop

Existing pattern in `src/plugins/image/view.rs:489-510` (drop target accepts files). Re-use for notes:
- Side-bar tree (`src/shell/side_bar.rs`) sets `draggable: true` on each row, `ondragstart` packs `application/x-operon-note` with `note_id`.
- The companion pane (both surfaces) registers `ondragover`/`ondrop` handlers on the outer container.
- Drop → enqueue Mention(kind = `DragDrop`), flash a toast.

### 4.4 Paste note-id

Composer keydown handler inspects clipboard text on paste:
- Bare UUID matching `[0-9a-f]{8}-...` → look up by id; if found, intercept paste and add a mention.
- `operon://note/<uuid>` → same.
- Anything else → let the paste through.

xterm side: monkey-patch the `onData` filter so a pasted `operon://note/…` line is *not* sent to the PTY; instead it's converted to a mention and a `<<operon:pending#N>>` marker is inserted.

### 4.5 Send-single-note / send-selection

Add a chevron menu to the note editor toolbar (already present for Skill nodes; see `src/plugins/skill/mod.rs`):
- "Send note to Claude" → push Mention(kind = ExplicitSend, no range).
- "Send selection to Claude" → only enabled when `MonacoChannel::snapshot().selection` is non-empty. Range carried in the Mention.
- If the companion pane is collapsed, expand it (`EXPAND_COMPANION_TICK` already exists in `companion_state.rs:114`).

### 4.6 Bulk add

Explorer tree already supports multi-select for move/delete. Add a "Send to Claude (N)" entry to the context menu; it pushes N mentions in one frame. The composer chip then reads "📎 N notes".

## 5. Milestones

> Estimates assume single developer, in the same cadence as M1–M3c.

### M4a — Mention queue + send-from-note (2 days)
- `PENDING_MENTIONS` global signal in `companion_state.rs`.
- Composer chip in chat mode, marker injection in terminal mode.
- "Send note to Claude" + "Send selection to Claude" toolbar items.
- No bridge yet; chat-mode test is end-to-end (XML block in user message).

**Acceptance**: in chat mode, sending a note results in Claude visibly seeing its title + body in the next turn.

### M4b — `operon-bridge` server + `operon-mcp` stub (3 days)
- New crate `crates/operon-bridge`: a tokio server on a unix socket, JSON-RPC 2.0 framing.
- New crate `crates/operon-mcp` (binary): stdio MCP server forwarding to the socket.
- Lockfile + env injection in both `companion_terminal.rs` spawn and `ClaudeCodeChatPlugin::send_rich()` spawn.
- Generated `.mcp.json` merged from user's `~/.claude/settings.json` + Operon's entry. Existing user config wins on key clashes except for `operon`, which we own.
- Wire `operon_ask_user` and `operon_get_pending_context` as the two first tools. Tests use a fake stdio client.

**Acceptance**: launch terminal mode, drag a note onto the pane, type "summarise the pending context", confirm Claude calls `operon_get_pending_context` and reads the note.

### M4c — Note read/write tools (2 days)
- `operon_get_note`, `operon_list_notes`, `operon_search_notes`, `operon_create_note`.
- `operon_append_note` (eager).
- `operon_replace_note_range` (eager and proposed variants; the proposed path re-uses `diff_preview.rs`).
- System-prompt snippet added to chat sessions ("you can read and write notes via tools …"). Terminal-mode users get the same hint via the per-project `CLAUDE.md` we already drop for skills.

**Acceptance**: ask Claude in terminal mode to "rewrite this artifact's introduction" — it calls `operon_get_note`, computes a new body, calls `operon_replace_note_range` with `confirm: true`, Operon shows a diff card, user accepts, the note's Monaco view updates live.

### M4d — @-picker, drag-drop, paste (2 days)
- Mention picker component, anchored to caret in chat / xterm cursor in terminal.
- Drag handlers on side-bar rows; drop handlers on companion pane.
- Clipboard paste interception.

**Acceptance**: each input gesture (`@`, drag, paste) lands a mention in `PENDING_MENTIONS` and is reflected in the composer chip / marker.

### M4e — Bulk + polish (1 day)
- Multi-select "Send N notes" in explorer.
- Mention chip → click to expand → per-mention remove.
- Pending-mention queue persists across pane collapse/expand (already true since it's a `GlobalSignal`); verify it does *not* persist across app restart (deliberate — staleness > surprise).
- Telemetry counters for which input gesture is most used.

### M4f — External MCP passthrough audit (1 day, can parallel-ize with M4e)
- Confirm that the merged `.mcp.json` correctly carries the user's existing MCP server entries from `mcp_settings/panel.rs`.
- Test against Atlassian + Figma servers (already listed in deferred tools) end-to-end in terminal mode.
- Document the precedence rule in `mcp_settings/` UI.

**Total**: ~10 working days. M4a is independently shippable; M4b is the prerequisite for everything terminal-side.

## 6. Open questions

These are flagged for revisit during M4b, not blockers right now.

- **Concurrent edits**: if Claude calls `operon_append_note` while the user is typing in Monaco, Loro merges deterministically — but the user's caret may jump. Acceptable? `claudecode.nvim` punts on this by only ever opening diff views (proposed mode). Default to eager and watch.
- **Mention chip in terminal mode**: how visible is `<<operon:pending#N>>` to the model? May need a stronger system-prompt hint or a special escape sequence the xterm host intercepts. Plan B: bump the mention marker into the actual PTY stream as a comment line before each user submit. Test in M4b.
- **Cross-pane mentions**: should sending a mention while the companion is set to chat mode automatically switch surfaces? Default: no, keep modes orthogonal.
- **Multi-session terminal**: today the terminal is single-instance per pane. Future work, not in scope.

## 7. What we are *not* borrowing from claudecode.nvim

- Their WebSocket transport: stdio MCP via a stub is closer to how Claude's MCP layer expects to talk.
- `selection_changed` continuous broadcast: explicit sends only. Less noise for the model, less ambient I/O for us.
- Lockfile in `~/.claude/ide/`: ours lives in the vault so it is naturally namespaced per Operon instance.
- xterm.js authoring: we use xterm.js already; no shared code with their pure-Lua RFC 6455 implementation.

## 8. References

- `claudecode.nvim/PROTOCOL.md` and `lua/claudecode/server/` — original inspiration for the lockfile + auth + JSON-RPC pattern.
- `src/shell/companion_terminal.rs:50-83` — current PTY spawn + cwd resolution. The bridge env vars get injected here in M4b.
- `src/shell/bridge_ask_user_executor.rs` — template for blocking tool flow; replicate for `operon_replace_note_range` with `confirm: true`.
- `src/shell/companion_state.rs` — home for `PENDING_MENTIONS` and the existing `ASK_USER_PROMPTS`/`EXPAND_COMPANION_TICK` patterns.
- `src/plugins/image/view.rs:489-510` — drag-and-drop reference implementation.
- `crates/operon-plugins-mcp/src/lib.rs` — existing stdio MCP client. Reused as-is for *outbound* MCP (Operon → other servers); the bridge is its *inbound* counterpart.
