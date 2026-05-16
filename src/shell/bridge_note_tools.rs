//! Read-only note tools exposed to Claude via the bridge:
//! `operon_get_note`, `operon_list_notes`, `operon_search_notes`.
//!
//! All three are thin wrappers over [`LocalNoteRepository`] and
//! [`crate::persistence::Persistence`] — the same two interfaces the
//! chat-mode mention path uses (`resolve_mentions_for_prompt` in
//! `companion_chat.rs`). Body content comes from `Persistence`
//! because notes are stored in SQLite + a Loro engine, not on disk;
//! the on-disk path returned to Claude (`<vault>/notes/<uuid>`,
//! `<repo>/.operon/artifacts/.../index.md`, …) is what Claude's
//! native `Read`/`Edit`/`Write` tools should target.
//!
//! Write tools (`operon_append_note`, `operon_replace_note_range`,
//! `operon_create_note`) land in M4c.4-.7.
//!
//! All tools surface as `mcp__operon__<tool_name>` once the bridge's
//! `.mcp.json` is loaded by Claude. The `operon` server name matches
//! the chat-mode in-process bridge — see `crates/operon-plugins-claude-code/src/permission_bridge.rs::MCP_SERVER_NAME`.

#![cfg(all(unix, not(target_arch = "wasm32")))]

use std::sync::Arc;

use async_trait::async_trait;
use operon_bridge::{ToolHandler, ToolHandlerError};
use operon_store::repos::NoteKind;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::local_mode::bridge_runtime::BridgeRepos;
use crate::local_mode::explorer::creatable_kind::scaffold_body;
use crate::plugins::artifact::frontmatter::ArtifactKind;
use crate::plugins::skill::install::{install_skills_into_project, SkillSource};
use crate::plugins::skill::materialize::{
    remove_skill_from_repo, write_skill_to_repo, MaterializeError,
};
use crate::plugins::skill::seed::{seed_readme, seed_skill_list};

/// Parse a uuid string out of a tool-argument blob. Returns a
/// `ToolHandlerError` (which the bridge turns into MCP `isError:
/// true`) when the value is missing or malformed, so the model sees
/// a recoverable error instead of a transport failure.
fn parse_uuid(args: &Value, field: &str) -> Result<Uuid, ToolHandlerError> {
    let s = args
        .get(field)
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolHandlerError::new(format!("{field}: missing or not a string")))?;
    Uuid::parse_str(s).map_err(|e| ToolHandlerError::new(format!("{field}: invalid uuid ({e})")))
}

/// Wrap a single text payload in the MCP `content` envelope.
fn text_content(body: String) -> Value {
    json!([{ "type": "text", "text": body }])
}

/// Cut a ~200-char window around the first match position in a body
/// scan, prefixed/suffixed with `…` when truncated. Used by
/// `search_notes` (in_content path) so the model can see where the
/// match occurred without loading the whole body. `match_pos` and
/// `needle` are in lower-cased byte coordinates from the caller's
/// existing `body.to_lowercase().find(&query)` call; we walk the
/// original-case body using char-boundary-safe slicing.
fn make_snippet(body: &str, match_pos: usize, needle: &str) -> String {
    const WINDOW: usize = 200;
    let start = match_pos.saturating_sub(WINDOW / 2);
    let end = (match_pos + needle.len() + WINDOW / 2).min(body.len());
    // Walk to a char boundary so we don't slice mid-UTF-8 codepoint.
    let safe_start = (start..=match_pos).rev().find(|i| body.is_char_boundary(*i)).unwrap_or(0);
    let safe_end = (end..body.len().saturating_add(1))
        .find(|i| body.is_char_boundary(*i))
        .unwrap_or(body.len());
    let mut out = String::new();
    if safe_start > 0 {
        out.push_str("…");
    }
    out.push_str(&body[safe_start..safe_end]);
    if safe_end < body.len() {
        out.push_str("…");
    }
    out
}

// ============================================================
// operon_get_note
// ============================================================

pub struct OperonGetNoteTool {
    repos: BridgeRepos,
}

impl OperonGetNoteTool {
    pub fn new(repos: BridgeRepos) -> Self {
        Self { repos }
    }
}

#[async_trait]
impl ToolHandler for OperonGetNoteTool {
    fn name(&self) -> &str {
        "get_note"
    }

    fn description(&self) -> &str {
        "Read a single note by its UUID. Returns JSON with `id`, `title`, `kind`, \
         `body` (full markdown / source text), `path` (the on-disk file you can \
         pass to your built-in `Read`/`Edit`/`Write` tools), `links` — the \
         outbound wikilinks parsed from the body, each as `{target_text, \
         target_note_id, is_embed}` (`target_note_id` is null for unresolved \
         wikilinks) — and `attachments` — sidecar files / images pinned to this \
         note, each as `{id, filename, mime_type, sha256_hex, size_bytes, \
         blob_path, created_at_ms}`. Use this when the user mentions or attaches \
         a note and you need its current contents — don't ask the user to paste \
         it. The `links` field lets you follow a specific reference without \
         re-parsing the body or running `search_notes` to resolve titles; for \
         traversing an entire reference tree, prefer `crawl_note_graph` instead. \
         The `attachments` field saves a follow-up `list_attachments` call when \
         the user asks about pinned files. The on-disk `path` is the canonical \
         edit target: editing it through `Edit`/`Write` round-trips through the \
         same Loro engine the GUI uses, so the user sees changes immediately."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "note_id": {
                    "type": "string",
                    "description": "The note's UUID. Required.",
                }
            },
            "required": ["note_id"]
        })
    }

    async fn call(&self, args: Value) -> Result<Value, ToolHandlerError> {
        let note_id = parse_uuid(&args, "note_id")?;

        // Title + kind via the note repo, plus the outbound link
        // rows from `local_note_link`, plus pinned attachments. All
        // folded into one blocking-pool trip so the whole repo
        // lookup is one round through the runtime — saves Claude
        // a follow-up search_notes per `[[wikilink]]` and a
        // follow-up list_attachments per pinned file.
        //
        // The repo calls are synchronous SQLite; run them on the
        // blocking pool so they don't hold up the bridge's
        // current-thread runtime (which would otherwise stall both
        // the writer task and other in-flight tool calls).
        let note_repo = self.repos.note_repo.clone();
        let link_repo = self.repos.link_repo.clone();
        let attachment_repo = self.repos.attachment_repo.clone();
        let (title, kind, created_at_ms, updated_at_ms, link_rows, attachments) =
            tokio::task::spawn_blocking(move || {
                let project_id = note_repo.find_project_for_note(note_id).ok().flatten();
                let (title, kind, created_at_ms, updated_at_ms) = match project_id {
                    Some(pid) => note_repo
                        .list_for_project(pid)
                        .ok()
                        .and_then(|rows| {
                            rows.into_iter().find(|r| r.id == note_id).map(|r| {
                                (
                                    r.title,
                                    r.kind.as_str().to_string(),
                                    r.created_at_ms,
                                    r.updated_at_ms,
                                )
                            })
                        })
                        .unwrap_or_else(|| (note_id.to_string(), "unknown".to_string(), 0, 0)),
                    None => (note_id.to_string(), "unknown".to_string(), 0, 0),
                };
                // Link rows degrade to [] on error rather than failing
                // the whole get_note — title/body/path are still useful
                // even if the link table is somehow unreadable. Same
                // shape for attachment rows.
                let links = link_repo.list_for_source(note_id).unwrap_or_default();
                let attachments = attachment_repo.list_by_note(note_id).unwrap_or_default();
                (title, kind, created_at_ms, updated_at_ms, links, attachments)
            })
            .await
            .map_err(|e| ToolHandlerError::new(format!("note_repo task join: {e}")))?;

        // Body via persistence (Loro engine). Non-UTF-8 bodies are
        // unusual for our note kinds (markdown, mdx, code) but we
        // still surface a deterministic placeholder rather than
        // erroring out — the title + path remain useful even when
        // the body is opaque.
        //
        // `Persistence::load` returns a `!Send` future (the trait
        // erases concrete Send-ness so the wasm `WebPersistence`
        // can hold JsValue handles). Our bridge runs on tokio's
        // multi-threaded scheduler via `tokio::spawn`, so this
        // async fn's future must be `Send`. Workaround: do the load
        // inside `spawn_blocking` and `futures::executor::block_on`
        // the future there — the !Send future is created and
        // dropped on the same blocking-pool thread and never
        // crosses a boundary. Only the resulting `Vec<u8>` (which
        // IS Send) makes it back here.
        let id_str = note_id.to_string();
        let persistence = self.repos.persistence.clone();
        let id_for_load = id_str.clone();
        let load_result: Result<Vec<u8>, _> = tokio::task::spawn_blocking(move || {
            futures::executor::block_on(persistence.load(&id_for_load))
        })
        .await
        .map_err(|e| ToolHandlerError::new(format!("persistence task join: {e}")))?;
        let body = match load_result {
            Ok(bytes) => String::from_utf8(bytes)
                .unwrap_or_else(|_| "(non-UTF-8 body — not inlined)".to_string()),
            Err(e) => {
                return Err(ToolHandlerError::new(format!(
                    "load body for {note_id}: {e}"
                )));
            }
        };

        // Canonical on-disk path — for artifact notes this is
        // `<repo>/.operon/<pid>/artifacts/<slug>/.../index.md`; for
        // free-form notes it's `<vault>/notes/<uuid>`. If
        // `resolved_path` returns None we leave it absent rather than
        // guessing — Claude will then know it can't `Edit` the file
        // and either ask the user or use append_note (M4c.5).
        let path = self.repos.persistence.resolved_path(&id_str);

        let links: Vec<Value> = link_rows
            .into_iter()
            .map(|row| {
                json!({
                    "target_text": row.target_text,
                    "target_note_id": row.target_note_id.map(|u| u.to_string()),
                    "is_embed": row.is_embed,
                })
            })
            .collect();
        let attachments_json: Vec<Value> = attachments
            .into_iter()
            .map(|a| {
                json!({
                    "id": a.id.to_string(),
                    "filename": a.filename,
                    "mime_type": a.mime_type,
                    "sha256_hex": a.sha256_hex,
                    "size_bytes": a.size_bytes,
                    "blob_path": a.blob_path,
                    "created_at_ms": a.created_at_ms,
                })
            })
            .collect();

        let payload = json!({
            "id": id_str,
            "title": title,
            "kind": kind,
            "body": body,
            "path": path.map(|p| p.to_string_lossy().into_owned()),
            "links": links,
            "attachments": attachments_json,
            "created_at_ms": created_at_ms,
            "updated_at_ms": updated_at_ms,
        });
        Ok(text_content(payload.to_string()))
    }
}

// ============================================================
// operon_list_notes
// ============================================================

pub struct OperonListNotesTool {
    repos: BridgeRepos,
}

impl OperonListNotesTool {
    pub fn new(repos: BridgeRepos) -> Self {
        Self { repos }
    }
}

#[async_trait]
impl ToolHandler for OperonListNotesTool {
    fn name(&self) -> &str {
        "list_notes"
    }

    fn description(&self) -> &str {
        "List notes in a project as a flat array. Each entry includes `id`, \
         `title`, `kind`, `parent_id` (UUID of the parent note or null for \
         project roots), `depth` (0 for roots), and `sibling_index`. Use this \
         to find a note's UUID before calling `get_note` or to enumerate the \
         project structure. Pass `project_id`; vault-wide listing is not \
         supported yet."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "string",
                    "description": "Project UUID. Required.",
                }
            },
            "required": ["project_id"]
        })
    }

    async fn call(&self, args: Value) -> Result<Value, ToolHandlerError> {
        let project_id = parse_uuid(&args, "project_id")?;
        // Sync SQLite call — see OperonGetNoteTool for the
        // spawn_blocking rationale (bridge runs on a current-thread
        // runtime; blocking the runtime thread stalls the writer task
        // and sibling tool calls).
        let note_repo = self.repos.note_repo.clone();
        let notes = tokio::task::spawn_blocking(move || note_repo.list_for_project(project_id))
            .await
            .map_err(|e| ToolHandlerError::new(format!("note_repo task join: {e}")))?
            .map_err(|e| ToolHandlerError::new(format!("list_for_project: {e}")))?;

        let rows: Vec<Value> = notes
            .into_iter()
            .map(|n| {
                json!({
                    "id": n.id.to_string(),
                    "title": n.title,
                    "kind": n.kind.as_str(),
                    "parent_id": n.parent_id.map(|p| p.to_string()),
                    "depth": n.depth,
                    "sibling_index": n.sibling_index,
                    "created_at_ms": n.created_at_ms,
                    "updated_at_ms": n.updated_at_ms,
                })
            })
            .collect();

        Ok(text_content(json!({ "notes": rows }).to_string()))
    }
}

// ============================================================
// operon_search_notes
// ============================================================

pub struct OperonSearchNotesTool {
    repos: BridgeRepos,
}

impl OperonSearchNotesTool {
    pub fn new(repos: BridgeRepos) -> Self {
        Self { repos }
    }
}

#[async_trait]
impl ToolHandler for OperonSearchNotesTool {
    fn name(&self) -> &str {
        "search_notes"
    }

    fn description(&self) -> &str {
        "Case-insensitive substring search across note titles, and optionally \
         note bodies. Returns up to `limit` results (default 20, max 200), each as \
         `{id, title, kind, project_id, created_at_ms, updated_at_ms, matched_in, \
         snippet?}` where `matched_in` is `\"title\"` or `\"body\"` and `snippet` \
         is a short body excerpt (only when matched_in == \"body\"). Scopes to a \
         single project when `project_id` is given; otherwise searches every \
         project in the vault (which requires the project repo to be available). \
         \
         Default is title-only matching, which is cheap. Pass `in_content: true` \
         to also scan note bodies — this is O(notes × body size) and loads every \
         note's content from disk, so prefer it only when titles wouldn't find the \
         match (e.g. the user says \"find where I wrote about the March deadline\")."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Substring to look for. Empty string returns the first `limit` notes.",
                },
                "project_id": {
                    "type": "string",
                    "description": "Optional. Restrict the search to one project.",
                },
                "limit": {
                    "type": "integer",
                    "description": "Optional. Default 20, max 200.",
                },
                "in_content": {
                    "type": "boolean",
                    "description": "Optional. Default false. When true, also scan note bodies in \
                                    addition to titles. Expensive on large vaults.",
                }
            },
            "required": ["query"]
        })
    }

    async fn call(&self, args: Value) -> Result<Value, ToolHandlerError> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolHandlerError::new("query: missing or not a string"))?
            .to_lowercase();
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(20)
            .clamp(1, 200);
        let in_content = args
            .get("in_content")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Project scoping: when `project_id` is specified we list a
        // single project; otherwise we enumerate every project via
        // the projects repo. If the projects repo isn't available
        // (rare — only some tests skip it), a vault-wide query
        // degrades to an explicit error rather than silently
        // returning [] so the model can ask the user for a
        // project_id.
        let scoped_pid: Option<Uuid> = match args.get("project_id") {
            Some(_) => Some(parse_uuid(&args, "project_id")?),
            None => None,
        };
        if scoped_pid.is_none() && self.repos.project_repo.is_none() {
            return Err(ToolHandlerError::new(
                "vault-wide search requires the project repo; pass `project_id` \
                 to scope the search to one project",
            ));
        }

        // The scan itself is pure synchronous SQLite + (optionally)
        // a body load per note. On a sizeable vault this can take
        // real time, so run the whole thing on the blocking pool —
        // leaving it inline would block the bridge's current-thread
        // runtime, stalling the writer and every sibling tool call.
        let note_repo = self.repos.note_repo.clone();
        let project_repo = self.repos.project_repo.clone();
        let persistence = self.repos.persistence.clone();
        let hits: Vec<Value> = tokio::task::spawn_blocking(move || {
            let candidate_pids: Vec<Uuid> = match scoped_pid {
                Some(pid) => vec![pid],
                None => match project_repo {
                    Some(prepo) => prepo
                        .list()
                        .map_err(|e| format!("list projects: {e}"))?
                        .into_iter()
                        .map(|p| p.id)
                        .collect(),
                    // Pre-checked above; unreachable in practice.
                    None => Vec::new(),
                },
            };

            let mut out: Vec<Value> = Vec::new();
            'outer: for pid in candidate_pids {
                let notes = match note_repo.list_for_project(pid) {
                    Ok(v) => v,
                    Err(_) => continue, // skip unreadable projects; don't fail the whole search
                };
                for n in notes {
                    let title_match = query.is_empty()
                        || n.title.to_lowercase().contains(&query);
                    // Compute body match only when needed: in_content is
                    // on AND the title didn't already match (so we don't
                    // double-load bodies for title hits). `block_on` on
                    // the !Send persistence future is safe here because
                    // we're inside `spawn_blocking` — the future is
                    // created and dropped on this thread.
                    let body_match: Option<String> =
                        if in_content && !title_match && !query.is_empty() {
                            let id_str = n.id.to_string();
                            futures::executor::block_on(persistence.load(&id_str))
                                .ok()
                                .and_then(|b| String::from_utf8(b).ok())
                                .and_then(|body| {
                                    let lower = body.to_lowercase();
                                    lower.find(&query).map(|pos| make_snippet(&body, pos, &query))
                                })
                        } else {
                            None
                        };

                    if title_match || body_match.is_some() {
                        let matched_in = if title_match { "title" } else { "body" };
                        let mut row = json!({
                            "id": n.id.to_string(),
                            "title": n.title,
                            "kind": n.kind.as_str(),
                            "project_id": pid.to_string(),
                            "created_at_ms": n.created_at_ms,
                            "updated_at_ms": n.updated_at_ms,
                            "matched_in": matched_in,
                        });
                        if let Some(snip) = body_match {
                            row["snippet"] = Value::String(snip);
                        }
                        out.push(row);
                        if out.len() >= limit {
                            break 'outer;
                        }
                    }
                }
            }
            Ok::<Vec<Value>, String>(out)
        })
        .await
        .map_err(|e| ToolHandlerError::new(format!("search task join: {e}")))?
        .map_err(ToolHandlerError::new)?;

        Ok(text_content(
            json!({ "results": hits, "limit": limit }).to_string(),
        ))
    }
}

// ============================================================
// operon_list_projects
// ============================================================

pub struct OperonListProjectsTool {
    repos: BridgeRepos,
}

impl OperonListProjectsTool {
    pub fn new(repos: BridgeRepos) -> Self {
        Self { repos }
    }
}

#[async_trait]
impl ToolHandler for OperonListProjectsTool {
    fn name(&self) -> &str {
        "list_projects"
    }

    fn description(&self) -> &str {
        "List every project in the vault, sorted by display order. Each entry \
         returns `{id, name, sibling_index, default_model?, default_permission_mode?, \
         repo_path?}`. Use this FIRST when the user mentions a project by name \
         instead of UUID — every other tool that takes `project_id` needs one of \
         these ids. Errors when no projects repo is wired (only happens in unusual \
         test setups)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn call(&self, _args: Value) -> Result<Value, ToolHandlerError> {
        let project_repo = self
            .repos
            .project_repo
            .clone()
            .ok_or_else(|| ToolHandlerError::new("projects repo not available in this session"))?;
        // Same blocking-pool discipline as the rest of the tools —
        // SQLite is sync, bridge runtime is current-thread.
        let mut projects = tokio::task::spawn_blocking(move || project_repo.list())
            .await
            .map_err(|e| ToolHandlerError::new(format!("project_repo task join: {e}")))?
            .map_err(|e| ToolHandlerError::new(format!("list projects: {e}")))?;
        projects.sort_by_key(|p| p.sibling_index);

        let rows: Vec<Value> = projects
            .into_iter()
            .map(|p| {
                json!({
                    "id": p.id.to_string(),
                    "name": p.name,
                    "sibling_index": p.sibling_index,
                    "created_at_ms": p.created_at_ms,
                    "default_model": p.default_model,
                    "default_permission_mode": p.default_permission_mode,
                    "repo_path": p.repo_path.map(|p| p.to_string_lossy().into_owned()),
                })
            })
            .collect();

        Ok(text_content(json!({ "projects": rows }).to_string()))
    }
}

// ============================================================
// operon_create_note
// ============================================================

pub struct OperonCreateNoteTool {
    repos: BridgeRepos,
}

impl OperonCreateNoteTool {
    pub fn new(repos: BridgeRepos) -> Self {
        Self { repos }
    }
}

#[async_trait]
impl ToolHandler for OperonCreateNoteTool {
    fn name(&self) -> &str {
        "create_note"
    }

    fn description(&self) -> &str {
        "Create a new note in a project. Returns `{id, path, title, kind}`. \
         The `path` is the on-disk file you can immediately `Edit`/`Write` if you \
         need to refine the body further. `kind` controls how the note is rendered \
         in the GUI — `markdown` is the safe default; use `artifact`, `skill`, \
         `workflow`, `phase`, `ce`, `code`, `mdx`, `kanban`, `canvas`, or \
         `excalidraw` only when you specifically want that surface. Image notes \
         (`image`) cannot be created with this tool — they need a blob upload. \
         \n\nFor `kind=\"artifact\"`, `artifact_kind` is REQUIRED — pick one of \
         master_requirement, requirements, epic, feature, story, task, \
         architecture, implementation_plan, implementation, test_cases, \
         test_results (cascade-pipeline kinds), or plan, summary, bug, \
         clarification, prioritized_backlog (cascade-internal kinds). Without \
         the right frontmatter the artifact is silently skipped by the cascade \
         and the typed toolbar. When `body` is empty, the tool seeds the full \
         section scaffold matching the GUI's `New > Artifact > <kind>` menu; \
         when `body` is set but lacks frontmatter, the tool prepends a minimal \
         `---\\nartifact_kind: <kind>\\n---` block; when `body` already starts \
         with `---\\n` the caller is in full control and nothing is injected."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "string",
                    "description": "Project UUID the note belongs to. Required.",
                },
                "title": {
                    "type": "string",
                    "description": "Note title shown in the explorer. Required.",
                },
                "parent_id": {
                    "type": "string",
                    "description": "Optional. Parent note UUID for nesting. \
                                    Omit for a top-level note in the project.",
                },
                "kind": {
                    "type": "string",
                    "description": "Optional. Note kind; defaults to 'markdown'. \
                                    Other accepted values: artifact, skill, workflow, \
                                    phase, ce, code, mdx, kanban, canvas, excalidraw.",
                },
                "artifact_kind": {
                    "type": "string",
                    "description": "REQUIRED when kind=\"artifact\"; rejected for any \
                                    other kind. One of: master_requirement, requirements, \
                                    epic, feature, story, task, architecture, \
                                    implementation_plan, implementation, test_cases, \
                                    test_results, plan, summary, bug, clarification, \
                                    prioritized_backlog. Drives both the seeded body \
                                    scaffold and the typed toolbar / cascade gates.",
                },
                "body": {
                    "type": "string",
                    "description": "Optional. Initial body / markdown source. Empty if omitted.",
                }
            },
            "required": ["project_id", "title"]
        })
    }

    async fn call(&self, args: Value) -> Result<Value, ToolHandlerError> {
        let project_id = parse_uuid(&args, "project_id")?;
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolHandlerError::new("title: missing or not a string"))?
            .to_string();
        let parent_id = match args.get("parent_id") {
            Some(v) if !v.is_null() => Some(parse_uuid(&args, "parent_id")?),
            _ => None,
        };
        let kind = args
            .get("kind")
            .and_then(|v| v.as_str())
            .map(NoteKind::from_str)
            .unwrap_or(NoteKind::Markdown);
        // `Image` notes require a blob upload alongside the row;
        // create_with_kind alone would leave a row with no
        // resolvable image, which the GUI then can't render. Refuse
        // explicitly rather than create a broken row.
        if matches!(kind, NoteKind::Image) {
            return Err(ToolHandlerError::new(
                "image notes cannot be created via this tool — they need a blob upload",
            ));
        }

        // `artifact_kind` is meaningful only for artifact notes. The cascade
        // gates and typed toolbar both key on the frontmatter `artifact_kind`
        // field — an artifact with no `artifact_kind:` is silently skipped by
        // `resolve_artifact_kind` and `claude_context.rs`, so it would look
        // created but behave as a dead note. Require it for artifacts and
        // reject it for everything else so the caller can't accidentally pass
        // it on a markdown note and expect it to mean something.
        let artifact_kind_arg = args
            .get("artifact_kind")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if matches!(kind, NoteKind::Artifact) && artifact_kind_arg.is_none() {
            return Err(ToolHandlerError::new(
                "artifact_kind is required when kind=\"artifact\"; pick one of: \
                 master_requirement, requirements, epic, feature, story, task, \
                 architecture, implementation_plan, implementation, test_cases, \
                 test_results, plan, summary, bug, clarification, prioritized_backlog",
            ));
        }
        if !matches!(kind, NoteKind::Artifact) && artifact_kind_arg.is_some() {
            return Err(ToolHandlerError::new(
                "artifact_kind only valid when kind=\"artifact\"",
            ));
        }

        let mut body = args
            .get("body")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_default();

        // Inject artifact frontmatter to match what the GUI's
        // `New > Artifact > <kind>` flow writes (explorer/mod.rs:895).
        //   - empty body  → full section scaffold + revision history
        //   - body w/o frontmatter → minimal `---\nartifact_kind: X\n---\n\n` prepended
        //   - body starting with `---\n` → caller owns the frontmatter, untouched
        if let Some(akind_str) = artifact_kind_arg {
            let akind = ArtifactKind::parse(&akind_str);
            if body.is_empty() {
                body = scaffold_body(&akind);
            } else if !body.starts_with("---\n") {
                body = format!("---\nartifact_kind: {}\n---\n\n{body}", akind.as_str());
            }
        }

        // Create the row. This is sync; bumps the SQLite tree state
        // but does not yet write a body. If this fails (FK
        // violation on project_id / parent_id, etc.) we bail before
        // touching persistence so there's no orphan body file.
        // spawn_blocking for the same reason as get/list/search —
        // keep SQLite work off the bridge's current-thread runtime.
        let note_repo = self.repos.note_repo.clone();
        let new_note = tokio::task::spawn_blocking(move || {
            note_repo.create_with_kind(project_id, parent_id, &title, kind)
        })
        .await
        .map_err(|e| ToolHandlerError::new(format!("note_repo task join: {e}")))?
        .map_err(|e| ToolHandlerError::new(format!("create_with_kind: {e}")))?;

        // Write the body via persistence. Same !Send workaround as
        // OperonGetNoteTool — Persistence::save returns a !Send
        // future; we run it on the blocking pool so it never crosses
        // a thread boundary.
        let id_str = new_note.id.to_string();
        let persistence = self.repos.persistence.clone();
        let id_for_save = id_str.clone();
        let body_for_save = body.into_bytes();
        let save_result: Result<(), _> = tokio::task::spawn_blocking(move || {
            futures::executor::block_on(persistence.save(&id_for_save, &body_for_save))
        })
        .await
        .map_err(|e| ToolHandlerError::new(format!("persistence task join: {e}")))?;
        if let Err(e) = save_result {
            // The row exists but the body write failed — surface
            // both halves of the bad state so the model can decide
            // whether to retry or delete the orphan row.
            return Err(ToolHandlerError::new(format!(
                "row {id_str} created but body save failed: {e}; \
                 consider calling delete_note (not yet wired) or retrying with append_note"
            )));
        }

        // Bump the explorer's reactive version so the new row
        // appears immediately. `LOCAL_NOTE_VERSION` is a Dioxus
        // GlobalSignal; same write-from-non-Dioxus-runtime pattern
        // as `push_ask_user_prompt` (see `companion_state.rs`).
        self.repos
            .ui
            .send(crate::local_mode::bridge_runtime::BridgeUiCommand::BumpNoteVersion);

        let path = self.repos.persistence.resolved_path(&id_str);
        let payload = json!({
            "id": id_str,
            "title": new_note.title,
            "kind": new_note.kind.as_str(),
            "path": path.map(|p| p.to_string_lossy().into_owned()),
        });
        Ok(text_content(payload.to_string()))
    }
}

// ============================================================
// operon_append_note
// ============================================================

pub struct OperonAppendNoteTool {
    repos: BridgeRepos,
}

impl OperonAppendNoteTool {
    pub fn new(repos: BridgeRepos) -> Self {
        Self { repos }
    }
}

#[async_trait]
impl ToolHandler for OperonAppendNoteTool {
    fn name(&self) -> &str {
        "append_note"
    }

    fn description(&self) -> &str {
        "Append text to the end of an existing note's body. Use this for \
         incremental additions (a new section, a log entry, an appendix) — it's \
         cheaper than reading the whole body, concatenating, and writing it back. \
         Returns `{id, path, new_length}`. The change is eager: the live editor \
         picks it up via the same FS reload path the chat-mode tools use."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "note_id": {
                    "type": "string",
                    "description": "UUID of the note to append to. Required.",
                },
                "text": {
                    "type": "string",
                    "description": "Text to append. A leading newline is added \
                                    automatically if the existing body doesn't end with one.",
                }
            },
            "required": ["note_id", "text"]
        })
    }

    async fn call(&self, args: Value) -> Result<Value, ToolHandlerError> {
        let note_id = parse_uuid(&args, "note_id")?;
        let mut text = args
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolHandlerError::new("text: missing or not a string"))?
            .to_string();
        if text.is_empty() {
            return Err(ToolHandlerError::new("text: must be non-empty"));
        }

        // Read current body, concat, write back. The read-modify-write
        // happens entirely on one blocking-pool thread so the
        // sequence is atomic from the bridge's perspective. We can't
        // make stronger atomicity claims without changing the
        // Persistence trait — concurrent appends from the user
        // typing in the editor could interleave. Acceptable for an
        // eager-write tool; the diff-card variant (M4c.7) is the
        // path to take when conflict-free semantics matter.
        let id_str = note_id.to_string();
        let persistence = self.repos.persistence.clone();
        let id_for_io = id_str.clone();

        let new_length: usize = tokio::task::spawn_blocking(move || {
            // Load → concat → save, all on the same thread so the
            // !Send Persistence futures stay local.
            let existing = futures::executor::block_on(persistence.load(&id_for_io))
                .map_err(|e| format!("load existing body: {e}"))?;
            let mut buf =
                String::from_utf8(existing).map_err(|_| "existing body is non-UTF-8".to_string())?;
            if !buf.is_empty() && !buf.ends_with('\n') {
                buf.push('\n');
            }
            buf.push_str(&text);
            let bytes = buf.into_bytes();
            let len = bytes.len();
            futures::executor::block_on(persistence.save(&id_for_io, &bytes))
                .map_err(|e| format!("save updated body: {e}"))?;
            // Suppress the unused-mut warning if we never mutated
            // `text` above. Kept as `let mut` because future variants
            // may normalize trailing whitespace.
            let _ = &mut text;
            Ok::<usize, String>(len)
        })
        .await
        .map_err(|e| ToolHandlerError::new(format!("persistence task join: {e}")))?
        .map_err(ToolHandlerError::new)?;

        // Same explorer-refresh signal as create_note — `LOCAL_NOTE_VERSION`
        // also keys "the note content version changed" consumers.
        self.repos
            .ui
            .send(crate::local_mode::bridge_runtime::BridgeUiCommand::BumpNoteVersion);

        let path = self.repos.persistence.resolved_path(&id_str);
        let payload = json!({
            "id": id_str,
            "new_length": new_length,
            "path": path.map(|p| p.to_string_lossy().into_owned()),
        });
        Ok(text_content(payload.to_string()))
    }
}

// ============================================================
// operon_replace_note_range (eager, anchor-based)
// ============================================================

pub struct OperonReplaceNoteRangeTool {
    repos: BridgeRepos,
}

impl OperonReplaceNoteRangeTool {
    pub fn new(repos: BridgeRepos) -> Self {
        Self { repos }
    }
}

#[async_trait]
impl ToolHandler for OperonReplaceNoteRangeTool {
    fn name(&self) -> &str {
        "replace_note_range"
    }

    fn description(&self) -> &str {
        "Find `old_text` in a note's body and replace it with `new_text`. Mirrors \
         your built-in `Edit` tool's semantics, but targets an Operon note by UUID \
         instead of an on-disk path. Use this when you want a precise change inside \
         an existing note rather than reading-then-rewriting the whole body. \
         \
         By default `old_text` must occur exactly once in the body — otherwise the \
         call errors so you can supply more surrounding context to disambiguate. \
         Pass `replace_all: true` to substitute every occurrence (useful for \
         renames). Empty `new_text` is allowed and acts as a pure deletion of \
         `old_text`. \
         \
         Pass `confirm: true` for a human-in-the-loop flow: the change is staged, \
         a diff card is shown in the companion chat, the tool call blocks until \
         the user clicks Accept or Reject, then either applies the change or \
         returns an error. Use confirm for risky edits, large replacements, or \
         when you're uncertain about the surrounding context."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "note_id": {
                    "type": "string",
                    "description": "UUID of the note to edit. Required.",
                },
                "old_text": {
                    "type": "string",
                    "description": "Verbatim text to find. Must be non-empty. \
                                    Include surrounding context (a few characters or \
                                    a whole line) to make the match unique unless \
                                    `replace_all` is true.",
                },
                "new_text": {
                    "type": "string",
                    "description": "Replacement. May be empty (acts as deletion).",
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Optional. Default false. When true, every \
                                    occurrence of `old_text` is replaced; when \
                                    false, the call errors if there isn't exactly one match.",
                },
                "confirm": {
                    "type": "boolean",
                    "description": "Optional. Default false. When true, the change \
                                    is staged and a diff card is shown to the user; \
                                    the tool blocks until they click Accept or Reject. \
                                    Use for risky or large edits.",
                }
            },
            "required": ["note_id", "old_text", "new_text"]
        })
    }

    async fn call(&self, args: Value) -> Result<Value, ToolHandlerError> {
        let note_id = parse_uuid(&args, "note_id")?;
        let old_text = args
            .get("old_text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolHandlerError::new("old_text: missing or not a string"))?
            .to_string();
        if old_text.is_empty() {
            return Err(ToolHandlerError::new("old_text: must be non-empty"));
        }
        let new_text = args
            .get("new_text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolHandlerError::new("new_text: missing or not a string"))?
            .to_string();
        let replace_all = args
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let confirm = args
            .get("confirm")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let id_str = note_id.to_string();
        let persistence = self.repos.persistence.clone();
        let id_for_io = id_str.clone();

        // Phase 1: load + compute the next body. Same load-modify
        // story as the eager path; we just defer the save until
        // after the (optional) user confirmation. Returns
        // (old_body, new_body, occurrences) so the eager and
        // confirm branches share the validation + match logic.
        let load_and_diff: Result<(String, String, usize), String> =
            tokio::task::spawn_blocking({
                let persistence = persistence.clone();
                let id_for_io = id_for_io.clone();
                let old_text = old_text.clone();
                let new_text = new_text.clone();
                move || {
                    let existing = futures::executor::block_on(persistence.load(&id_for_io))
                        .map_err(|e| format!("load existing body: {e}"))?;
                    let buf = String::from_utf8(existing).map_err(|_| {
                        "existing body is non-UTF-8; cannot anchor-replace".to_string()
                    })?;
                    let count = buf.matches(old_text.as_str()).count();
                    if count == 0 {
                        return Err(format!(
                            "old_text not found in note body — supply the verbatim slice \
                             you want to replace (note body length: {})",
                            buf.len()
                        ));
                    }
                    if count > 1 && !replace_all {
                        return Err(format!(
                            "old_text matched {count} times — add surrounding context to \
                             make the match unique, or pass `replace_all: true`"
                        ));
                    }
                    let next = if replace_all {
                        buf.replace(old_text.as_str(), &new_text)
                    } else {
                        buf.replacen(old_text.as_str(), &new_text, 1)
                    };
                    Ok((buf, next, count))
                }
            })
            .await
            .map_err(|e| ToolHandlerError::new(format!("persistence task join: {e}")))?;

        let (old_body, new_body, occurrences) = load_and_diff.map_err(ToolHandlerError::new)?;

        // Phase 2: confirm gate. When `confirm: true`, stage the
        // proposal and wait for the user. The Accept/Reject buttons
        // on the card resolve the responder; we then either persist
        // or return an error.
        if confirm {
            // Resolve note title for the card header — falls back
            // to the uuid when the lookup fails (rare; same shape
            // as get_note's title resolution). Same spawn_blocking
            // discipline so the sync SQLite lookups don't stall the
            // bridge runtime.
            let note_repo = self.repos.note_repo.clone();
            let title = tokio::task::spawn_blocking(move || {
                note_repo
                    .find_project_for_note(note_id)
                    .ok()
                    .flatten()
                    .and_then(|pid| note_repo.list_for_project(pid).ok())
                    .and_then(|rows| {
                        rows.into_iter()
                            .find(|r| r.id == note_id)
                            .map(|r| r.title)
                    })
                    .unwrap_or_else(|| note_id.to_string())
            })
            .await
            .map_err(|e| ToolHandlerError::new(format!("note_repo task join: {e}")))?;

            let diff_preview =
                render_unified_diff(&format!("note:{id_str}"), &old_body, &new_body);

            let proposal_id = Uuid::new_v4().to_string();
            let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
            // park_note_proposal_responder writes the OnceLock<Mutex>
            // responder map (not a GlobalSignal), so it's safe from
            // the bridge thread directly. The `push_note_proposal`
            // call below goes through the channel because that one
            // touches the `NOTE_PROPOSALS` GlobalSignal.
            crate::shell::companion_state::park_note_proposal_responder(
                proposal_id.clone(),
                tx,
            );
            self.repos.ui.send(
                crate::local_mode::bridge_runtime::BridgeUiCommand::PushNoteProposal(
                    crate::shell::companion_state::NoteProposalEntry {
                        id: proposal_id.clone(),
                        note_id,
                        note_title: title,
                        old_body: old_body.clone(),
                        new_body: new_body.clone(),
                        diff_preview,
                        source_session: None,
                        created_at: std::time::SystemTime::now(),
                    },
                ),
            );

            let accepted = rx
                .await
                .map_err(|_| ToolHandlerError::new("proposal responder dropped"))?;
            if !accepted {
                return Err(ToolHandlerError::new(
                    "user rejected the proposed edit — refine and try again, or \
                     ask the user what they want changed",
                ));
            }
        }

        // Phase 3: persist. Identical for both eager and confirmed
        // paths once we have `new_body`.
        let new_body_for_save = new_body.into_bytes();
        let id_for_save = id_str.clone();
        let new_length: usize = tokio::task::spawn_blocking(move || {
            let len = new_body_for_save.len();
            futures::executor::block_on(persistence.save(&id_for_save, &new_body_for_save))
                .map(|_| len)
                .map_err(|e| format!("save updated body: {e}"))
        })
        .await
        .map_err(|e| ToolHandlerError::new(format!("persistence task join: {e}")))?
        .map_err(ToolHandlerError::new)?;

        // Refresh the explorer + editor reactivity. See create_note
        // for the GlobalSignal-from-bridge-thread caveat.
        self.repos
            .ui
            .send(crate::local_mode::bridge_runtime::BridgeUiCommand::BumpNoteVersion);

        let path = self.repos.persistence.resolved_path(&id_str);
        let payload = json!({
            "id": id_str,
            "new_length": new_length,
            "occurrences_replaced": occurrences,
            "path": path.map(|p| p.to_string_lossy().into_owned()),
            "confirmed": confirm,
        });
        Ok(text_content(payload.to_string()))
    }
}

// ============================================================
// operon_delete_note (always-confirm)
// ============================================================

pub struct OperonDeleteNoteTool {
    repos: BridgeRepos,
}

impl OperonDeleteNoteTool {
    pub fn new(repos: BridgeRepos) -> Self {
        Self { repos }
    }
}

#[async_trait]
impl ToolHandler for OperonDeleteNoteTool {
    fn name(&self) -> &str {
        "delete_note"
    }

    fn description(&self) -> &str {
        "Delete a note from the vault. ALWAYS shows a confirmation card in the \
         companion chat first; the tool blocks until the user clicks Delete or \
         Keep. Deletion cascades to descendants via the foreign-key constraint, \
         so the card surfaces how many child notes would also be removed. After \
         the rows are gone, the bridge runs a refcount-based GC over every \
         blob_path the subtree referenced (image notes + attachments) and \
         unlinks any blob no longer pointed to from any other row. Returns \
         `{id, deleted: true, descendants_removed, blob_gc_count}` on accept, \
         or an error string on reject so you can iterate. There is no undo — \
         review carefully before suggesting this."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "note_id": { "type": "string", "description": "UUID of the note to delete." }
            },
            "required": ["note_id"]
        })
    }

    async fn call(&self, args: Value) -> Result<Value, ToolHandlerError> {
        let note_id = parse_uuid(&args, "note_id")?;
        let note_repo = self.repos.note_repo.clone();
        let attachment_repo = self.repos.attachment_repo.clone();

        // Resolve title + descendant count on the blocking pool so
        // the confirm card has accurate info. snapshot_subtree
        // returns the full subtree without mutating; we subtract 1
        // (the root itself) to get the descendant count. We also
        // collect every blob_path the subtree references — image
        // notes (`local_note.blob_path`) AND attachments under each
        // subtree note (`local_attachments.blob_path`) — so the
        // post-delete GC pass knows which on-disk blobs to consider.
        let preview = tokio::task::spawn_blocking({
            let note_repo = note_repo.clone();
            let attachment_repo = attachment_repo.clone();
            move || {
                let pid = note_repo
                    .find_project_for_note(note_id)
                    .map_err(|e| format!("find_project_for_note: {e}"))?
                    .ok_or_else(|| format!("note {note_id} not found"))?;
                let title = note_repo
                    .list_for_project(pid)
                    .map_err(|e| format!("list_for_project: {e}"))?
                    .into_iter()
                    .find(|r| r.id == note_id)
                    .map(|r| r.title)
                    .unwrap_or_else(|| note_id.to_string());
                let snap = note_repo
                    .snapshot_subtree(note_id)
                    .map_err(|e| format!("snapshot_subtree: {e}"))?;
                // SubtreeSnapshot's notes vec includes the root, so the
                // descendant count is len - 1. Defensive saturating_sub
                // in case the snapshot ever returns empty.
                let descendants = snap.notes.len().saturating_sub(1);
                let mut candidate_blobs: Vec<String> = Vec::new();
                for n in &snap.notes {
                    if let Some(bp) = &n.blob_path {
                        candidate_blobs.push(bp.clone());
                    }
                    if let Ok(atts) = attachment_repo.list_by_note(n.id) {
                        for a in atts {
                            candidate_blobs.push(a.blob_path);
                        }
                    }
                }
                Ok::<(String, usize, Vec<String>), String>((title, descendants, candidate_blobs))
            }
        })
        .await
        .map_err(|e| ToolHandlerError::new(format!("delete preview task join: {e}")))?
        .map_err(ToolHandlerError::new)?;
        let (note_title, descendant_count, candidate_blobs) = preview;

        // Park the responder + push the proposal card. Same shape
        // as OperonReplaceNoteRangeTool's confirm path; the only
        // change is the variant on BridgeUiCommand.
        let proposal_id = Uuid::new_v4().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
        crate::shell::companion_state::park_note_deletion_responder(proposal_id.clone(), tx);
        self.repos.ui.send(
            crate::local_mode::bridge_runtime::BridgeUiCommand::PushNoteDeletionProposal(
                crate::shell::companion_state::NoteDeletionProposalEntry {
                    id: proposal_id.clone(),
                    note_id,
                    note_title,
                    descendant_count,
                    source_session: None,
                    created_at: std::time::SystemTime::now(),
                },
            ),
        );

        let accepted = rx
            .await
            .map_err(|_| ToolHandlerError::new("deletion responder dropped"))?;
        if !accepted {
            return Err(ToolHandlerError::new(
                "user rejected the deletion — ask before retrying, or pick a different note",
            ));
        }

        // Commit the delete + run blob GC on the blocking pool. The
        // FK cascade handles descendant `local_note` rows and their
        // `local_attachments`; we just need to GC the on-disk blobs
        // that those rows referenced.
        let vault = self.repos.vault_root.clone();
        let blob_gc_count: usize = tokio::task::spawn_blocking({
            let note_repo = note_repo.clone();
            let attachment_repo = attachment_repo.clone();
            move || {
                note_repo
                    .delete(note_id)
                    .map_err(|e| format!("delete: {e}"))?;
                let gc = match vault {
                    Some(vault) if !candidate_blobs.is_empty() => {
                        crate::local_mode::images::gc_unreferenced_blobs(
                            &vault,
                            &candidate_blobs,
                            &|p| note_repo.count_by_blob_path(p).unwrap_or(0),
                            &|p| attachment_repo.count_by_blob_path(p).unwrap_or(0),
                        )
                    }
                    _ => 0,
                };
                Ok::<usize, String>(gc)
            }
        })
        .await
        .map_err(|e| ToolHandlerError::new(format!("delete task join: {e}")))?
        .map_err(ToolHandlerError::new)?;

        self.repos
            .ui
            .send(crate::local_mode::bridge_runtime::BridgeUiCommand::BumpNoteVersion);

        let payload = json!({
            "id": note_id.to_string(),
            "deleted": true,
            "descendants_removed": descendant_count,
            "blob_gc_count": blob_gc_count,
        });
        Ok(text_content(payload.to_string()))
    }
}

// ============================================================
// operon_crawl_note_graph
// ============================================================

/// Which way the BFS follows edges from each visited node.
/// `Out` walks outbound (what does this note link to?), `In` walks
/// inbound (what links to this note?), `Both` does both — useful for
/// "show me the entire neighborhood around this note".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrawlDirection {
    Out,
    In,
    Both,
}

impl CrawlDirection {
    fn includes_out(self) -> bool {
        matches!(self, Self::Out | Self::Both)
    }
    fn includes_in(self) -> bool {
        matches!(self, Self::In | Self::Both)
    }
}

/// Knobs the BFS reads. Carved out as a struct so the algorithm can
/// be unit-tested without going through the MCP envelope (build
/// `CrawlArgs` directly, call `crawl_note_graph_bfs`).
#[derive(Debug, Clone)]
pub struct CrawlArgs {
    pub root_id: Uuid,
    pub max_depth: usize,
    pub max_nodes: usize,
    pub include_bodies: bool,
    pub embeds_only: bool,
    pub direction: CrawlDirection,
}

/// Why the crawl stopped before exhausting the reachable subgraph,
/// if it did. Surfaces in the response so Claude can decide whether
/// to ask for a larger cap or a deeper crawl.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrawlTruncation {
    MaxDepth,
    MaxNodes,
}

impl CrawlTruncation {
    fn as_str(self) -> &'static str {
        match self {
            Self::MaxDepth => "max_depth",
            Self::MaxNodes => "max_nodes",
        }
    }
}

/// Result type the BFS hands back. Serialized into the MCP `content`
/// envelope by [`OperonCrawlNoteGraphTool::call`].
#[derive(Debug, Clone)]
pub struct CrawlResult {
    pub nodes: Vec<Value>,
    pub edges: Vec<Value>,
    pub truncated: bool,
    pub truncation_reason: Option<CrawlTruncation>,
}

/// Server-side breadth-first walk of the `local_note_link` graph.
/// Factored out of the tool's `call` method so tests can exercise
/// the algorithm against an in-memory `Store` without setting up
/// MCP plumbing.
///
/// Cycle handling: a `HashSet<Uuid>` of visited ids; a node is only
/// enqueued the first time we see it. Back-edges still show up in
/// `edges` (so the caller can see the cycle) but the target is not
/// re-walked. Depth and node caps are enforced inline; truncation is
/// reported via `CrawlResult::truncation_reason` so the caller can
/// decide whether to retry with a larger budget.
///
/// All SQLite calls (`link_repo.list_for_source`,
/// `note_repo.find_project_for_note`, `note_repo.list_for_project`)
/// run on the calling thread; invoke this inside
/// `tokio::task::spawn_blocking` from the async tool entry point so
/// the bridge's current-thread runtime isn't pinned.
pub fn crawl_note_graph_bfs(
    args: &CrawlArgs,
    link_repo: &Arc<dyn operon_store::repos::LocalNoteLinkRepository>,
    note_repo: &Arc<dyn operon_store::repos::LocalNoteRepository>,
    persistence: Option<&Arc<dyn crate::persistence::Persistence>>,
) -> Result<CrawlResult, String> {
    use std::collections::{HashMap, HashSet, VecDeque};

    // Per-project row cache: pid → (note_id → LocalNote). Filled
    // lazily — we only list a project when we encounter a node
    // belonging to it. Reuses `find_project_for_note` +
    // `list_for_project` because the repo trait has no
    // get-row-by-uuid endpoint (same pattern OperonGetNoteTool uses).
    let mut project_cache: HashMap<Uuid, HashMap<Uuid, operon_store::repos::LocalNote>> =
        HashMap::new();

    // Build a NodeOut JSON value for a given note id. Returns None
    // when the row can't be resolved (orphan / deleted target);
    // callers fall back to a stub with title=uuid so Claude can
    // still see "something was here."
    let mut resolve_node =
        |id: Uuid,
         depth: usize,
         project_cache: &mut HashMap<
            Uuid,
            HashMap<Uuid, operon_store::repos::LocalNote>,
        >|
         -> Value {
            let pid = match note_repo.find_project_for_note(id) {
                Ok(Some(pid)) => pid,
                _ => {
                    // Unknown project — return a stub node so Claude
                    // sees it was unreachable rather than missing
                    // from `nodes` entirely.
                    return json!({
                        "id": id.to_string(),
                        "title": id.to_string(),
                        "kind": "unknown",
                        "depth": depth,
                        "project_id": Value::Null,
                    });
                }
            };
            let rows = project_cache.entry(pid).or_insert_with(|| {
                note_repo
                    .list_for_project(pid)
                    .ok()
                    .map(|v| v.into_iter().map(|n| (n.id, n)).collect())
                    .unwrap_or_default()
            });
            let (title, kind) = match rows.get(&id) {
                Some(n) => (n.title.clone(), n.kind.as_str().to_string()),
                None => (id.to_string(), "unknown".to_string()),
            };
            let mut node = json!({
                "id": id.to_string(),
                "title": title,
                "kind": kind,
                "depth": depth,
                "project_id": pid.to_string(),
            });
            if let Some(p) = persistence {
                // Same `!Send` workaround as OperonGetNoteTool — we're
                // already on a blocking-pool thread because the caller
                // wrapped us in spawn_blocking, so the !Send future
                // never crosses a runtime boundary.
                let id_str = id.to_string();
                let body = match futures::executor::block_on(p.load(&id_str)) {
                    Ok(bytes) => String::from_utf8(bytes)
                        .unwrap_or_else(|_| "(non-UTF-8 body — not inlined)".to_string()),
                    Err(_) => String::new(),
                };
                node["body"] = Value::String(body);
            }
            node
        };

    let mut visited: HashSet<Uuid> = HashSet::new();
    let mut nodes: Vec<Value> = Vec::new();
    let mut edges: Vec<Value> = Vec::new();
    let mut queue: VecDeque<(Uuid, usize)> = VecDeque::new();
    let mut truncation: Option<CrawlTruncation> = None;

    visited.insert(args.root_id);
    nodes.push(resolve_node(args.root_id, 0, &mut project_cache));
    queue.push_back((args.root_id, 0));

    // Outbound link rows are needed in two situations: walking
    // outbound from a node ("what does this point at?"), and
    // recovering the LinkRow metadata (target_text, is_embed) for an
    // inbound edge discovered via `referrers_of` (which only returns
    // source UUIDs). Memoizing avoids re-querying the same source
    // when "both" mode visits a node first as a target, then later
    // as an outbound source.
    use operon_store::repos::LinkRow;
    let mut links_cache: HashMap<Uuid, Vec<LinkRow>> = HashMap::new();
    let mut get_outbound = |src: Uuid| -> Result<Vec<LinkRow>, String> {
        if let Some(v) = links_cache.get(&src) {
            return Ok(v.clone());
        }
        let v = link_repo
            .list_for_source(src)
            .map_err(|e| format!("list_for_source({src}): {e}"))?;
        links_cache.insert(src, v.clone());
        Ok(v)
    };

    'outer: while let Some((id, depth)) = queue.pop_front() {
        if args.direction.includes_out() {
            let out_links = get_outbound(id)?;
            for link in out_links {
                if args.embeds_only && !link.is_embed {
                    continue;
                }
                edges.push(json!({
                    "from": id.to_string(),
                    "to": link.target_note_id.map(|u| u.to_string()),
                    "target_text": link.target_text,
                    "is_embed": link.is_embed,
                    "direction": "out",
                }));
                let Some(target) = link.target_note_id else {
                    // Unresolved wikilink (target_note_id IS NULL).
                    // Edge is reported above; nothing to enqueue.
                    continue;
                };
                if visited.contains(&target) {
                    continue;
                }
                if depth + 1 > args.max_depth {
                    truncation.get_or_insert(CrawlTruncation::MaxDepth);
                    continue;
                }
                if nodes.len() >= args.max_nodes {
                    truncation = Some(CrawlTruncation::MaxNodes);
                    break 'outer;
                }
                visited.insert(target);
                nodes.push(resolve_node(target, depth + 1, &mut project_cache));
                queue.push_back((target, depth + 1));
            }
        }

        if args.direction.includes_in() {
            // `referrers_of` returns the bare source UUIDs that link
            // to `id`. We pull each source's full outbound rows to
            // recover the matching LinkRow (so `target_text` and
            // `is_embed` show up in the edge), then enqueue the
            // source if it passes the same caps as the outbound
            // path. embeds_only still applies — a source that links
            // to us via a plain `[[…]]` is suppressed when the
            // caller only wants transclusions.
            let sources = link_repo
                .referrers_of(id)
                .map_err(|e| format!("referrers_of({id}): {e}"))?;
            for src in sources {
                let src_links = get_outbound(src)?;
                let matching: Vec<&LinkRow> = src_links
                    .iter()
                    .filter(|l| l.target_note_id == Some(id))
                    .collect();
                if matching.is_empty() {
                    // referrers_of and list_for_source disagree —
                    // shouldn't happen with a sane DB, but skip
                    // rather than fabricate an edge with no metadata.
                    continue;
                }
                let mut enqueue = false;
                for link in matching {
                    if args.embeds_only && !link.is_embed {
                        continue;
                    }
                    edges.push(json!({
                        "from": src.to_string(),
                        "to": id.to_string(),
                        "target_text": link.target_text,
                        "is_embed": link.is_embed,
                        "direction": "in",
                    }));
                    enqueue = true;
                }
                if !enqueue {
                    continue;
                }
                if visited.contains(&src) {
                    continue;
                }
                if depth + 1 > args.max_depth {
                    truncation.get_or_insert(CrawlTruncation::MaxDepth);
                    continue;
                }
                if nodes.len() >= args.max_nodes {
                    truncation = Some(CrawlTruncation::MaxNodes);
                    break 'outer;
                }
                visited.insert(src);
                nodes.push(resolve_node(src, depth + 1, &mut project_cache));
                queue.push_back((src, depth + 1));
            }
        }
    }

    Ok(CrawlResult {
        nodes,
        edges,
        truncated: truncation.is_some(),
        truncation_reason: truncation,
    })
}

pub struct OperonCrawlNoteGraphTool {
    repos: BridgeRepos,
}

impl OperonCrawlNoteGraphTool {
    pub fn new(repos: BridgeRepos) -> Self {
        Self { repos }
    }
}

#[async_trait]
impl ToolHandler for OperonCrawlNoteGraphTool {
    fn name(&self) -> &str {
        "crawl_note_graph"
    }

    fn description(&self) -> &str {
        "Walk the wikilink graph starting from a note, BFS-style, and return the \
         reachable subgraph in one call. Cycle-safe (each node visited at most once) \
         and bounded by `max_depth` (default 5) and `max_nodes` (default 200). \
         \
         `direction` controls which edges are followed: `\"out\"` (default) walks \
         outbound — what does this note link to? Use it for \"what does this depend \
         on / reference?\". `\"in\"` walks inbound via the backlink index — what \
         links to this note? Use it for \"what depends on / references this?\". \
         `\"both\"` walks both directions for a full neighborhood view. \
         \
         Returns `{nodes, edges, truncated, truncation_reason}`. Each node has \
         `id`, `title`, `kind`, `depth` (hop count from the root), and `project_id`; \
         pass `include_bodies: true` to inline full bodies (expensive — prefer a \
         second `get_note` call for the ones you actually need to read). Each edge \
         has `from`, `to` (null for unresolved wikilinks), `target_text` (the raw \
         `[[...]]` slug), `is_embed` (true for `![[...]]`), and `direction` \
         (`\"out\"` or `\"in\"` — which traversal pass discovered the edge). \
         Edges to nodes that were skipped (depth/node cap, unresolved target) \
         still appear so you can see where the graph continues. \
         \
         Use this instead of looping `get_note` + `search_notes` when the user asks \
         about everything connected to a note, a requirements tree, a chain of \
         linked design docs, etc. Pass `embeds_only: true` to follow only `![[...]]` \
         transclusions (content actually pulled in)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "root_id": {
                    "type": "string",
                    "description": "UUID of the note to start crawling from. Required.",
                },
                "max_depth": {
                    "type": "integer",
                    "description": "Optional. Hop limit from the root. Default 5, clamped to [1, 20].",
                },
                "max_nodes": {
                    "type": "integer",
                    "description": "Optional. Cap on total nodes returned. Default 200, clamped to [1, 1000].",
                },
                "include_bodies": {
                    "type": "boolean",
                    "description": "Optional. Default false. When true, each node carries its full body. \
                                    Skip this unless you genuinely need every body — payloads grow fast.",
                },
                "embeds_only": {
                    "type": "boolean",
                    "description": "Optional. Default false. When true, only follow `![[...]]` embed \
                                    links (the ones that actually transclude content), ignore plain `[[...]]`.",
                },
                "direction": {
                    "type": "string",
                    "enum": ["out", "in", "both"],
                    "description": "Optional. Default `\"out\"`. `\"out\"` follows outbound links, \
                                    `\"in\"` follows backlinks (what references this note), `\"both\"` \
                                    walks both directions for a full neighborhood view.",
                }
            },
            "required": ["root_id"]
        })
    }

    async fn call(&self, args: Value) -> Result<Value, ToolHandlerError> {
        let root_id = parse_uuid(&args, "root_id")?;
        let max_depth = args
            .get("max_depth")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(5)
            .clamp(1, 20);
        let max_nodes = args
            .get("max_nodes")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(200)
            .clamp(1, 1000);
        let include_bodies = args
            .get("include_bodies")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let embeds_only = args
            .get("embeds_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let direction = match args.get("direction").and_then(|v| v.as_str()) {
            Some("out") | None => CrawlDirection::Out,
            Some("in") => CrawlDirection::In,
            Some("both") => CrawlDirection::Both,
            Some(other) => {
                return Err(ToolHandlerError::new(format!(
                    "direction: must be \"out\", \"in\", or \"both\" (got {other:?})"
                )));
            }
        };

        let crawl_args = CrawlArgs {
            root_id,
            max_depth,
            max_nodes,
            include_bodies,
            embeds_only,
            direction,
        };

        let link_repo = self.repos.link_repo.clone();
        let note_repo = self.repos.note_repo.clone();
        let persistence = self.repos.persistence.clone();
        // Whole BFS runs on the blocking pool — same discipline as
        // the other note tools. SQLite is sync, and the persistence
        // load (when include_bodies is true) returns a `!Send`
        // future we need to block_on locally.
        let result: Result<CrawlResult, String> = tokio::task::spawn_blocking(move || {
            let persistence_opt = if include_bodies {
                Some(persistence)
            } else {
                None
            };
            crawl_note_graph_bfs(
                &crawl_args,
                &link_repo,
                &note_repo,
                persistence_opt.as_ref(),
            )
        })
        .await
        .map_err(|e| ToolHandlerError::new(format!("crawl task join: {e}")))?;

        let result = result.map_err(ToolHandlerError::new)?;
        let payload = json!({
            "nodes": result.nodes,
            "edges": result.edges,
            "truncated": result.truncated,
            "truncation_reason": result.truncation_reason.map(|t| t.as_str()),
        });
        Ok(text_content(payload.to_string()))
    }
}

// ============================================================
// operon_open_note (FocusNote UI command)
// ============================================================

pub struct OperonOpenNoteTool {
    repos: BridgeRepos,
}

impl OperonOpenNoteTool {
    pub fn new(repos: BridgeRepos) -> Self {
        Self { repos }
    }
}

#[async_trait]
impl ToolHandler for OperonOpenNoteTool {
    fn name(&self) -> &str {
        "open_note"
    }

    fn description(&self) -> &str {
        "Open / focus a note tab in the editor pane so the user can see it. \
         Fire-and-forget — returns `{focused: true}` immediately; the actual \
         tab activation happens on the next Dioxus tick. Useful after making a \
         change the user should review, or when answering 'show me X'."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "note_id": { "type": "string", "description": "UUID of the note to open." }
            },
            "required": ["note_id"]
        })
    }

    async fn call(&self, args: Value) -> Result<Value, ToolHandlerError> {
        let note_id = parse_uuid(&args, "note_id")?;
        self.repos
            .ui
            .send(crate::local_mode::bridge_runtime::BridgeUiCommand::FocusNote(note_id));
        Ok(text_content(
            json!({ "focused": true, "id": note_id.to_string() }).to_string(),
        ))
    }
}

// ============================================================
// operon_rename_note
// ============================================================

pub struct OperonRenameNoteTool {
    repos: BridgeRepos,
}

impl OperonRenameNoteTool {
    pub fn new(repos: BridgeRepos) -> Self {
        Self { repos }
    }
}

#[async_trait]
impl ToolHandler for OperonRenameNoteTool {
    fn name(&self) -> &str {
        "rename_note"
    }

    fn description(&self) -> &str {
        "Change a note's title. Eager — applies immediately. After renaming, any \
         wikilinks in OTHER notes whose stored `target_text` exactly matched the \
         old title get rewritten to the new title (mirrors the in-GUI rename). \
         Returns `{id, old_title, new_title, links_rewritten}`. Limitation: \
         aliased wikilinks like `[[X|alias]]` or path-form `[[notes/X]]` won't be \
         touched because their stored `target_text` differs from the bare title — \
         tell the user when you rename so they can review affected references."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "note_id": { "type": "string", "description": "UUID of the note to rename." },
                "new_title": { "type": "string", "description": "Replacement title; must be non-empty." }
            },
            "required": ["note_id", "new_title"]
        })
    }

    async fn call(&self, args: Value) -> Result<Value, ToolHandlerError> {
        let note_id = parse_uuid(&args, "note_id")?;
        let new_title = args
            .get("new_title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolHandlerError::new("new_title: missing or not a string"))?
            .to_string();
        if new_title.trim().is_empty() {
            return Err(ToolHandlerError::new("new_title: must be non-empty"));
        }

        let note_repo = self.repos.note_repo.clone();
        let link_repo = self.repos.link_repo.clone();
        let title_for_rewrite = new_title.clone();
        let (old_title, links_rewritten): (String, u64) =
            tokio::task::spawn_blocking(move || {
                let pid = note_repo
                    .find_project_for_note(note_id)
                    .map_err(|e| format!("find_project_for_note: {e}"))?
                    .ok_or_else(|| format!("note {note_id} not found"))?;
                let old_title = note_repo
                    .list_for_project(pid)
                    .map_err(|e| format!("list_for_project: {e}"))?
                    .into_iter()
                    .find(|r| r.id == note_id)
                    .map(|r| r.title)
                    .ok_or_else(|| format!("note {note_id} not in project list"))?;
                note_repo
                    .rename(note_id, &title_for_rewrite)
                    .map_err(|e| format!("rename: {e}"))?;
                // Best-effort link rewrite: skip on error so the
                // rename itself is still durable. Returns the number
                // of `local_note_link` rows that had their stored
                // target_text swapped to the new title.
                let rewritten = link_repo
                    .rewrite_target_text(note_id, &old_title, &title_for_rewrite)
                    .unwrap_or(0);
                Ok::<(String, u64), String>((old_title, rewritten))
            })
            .await
            .map_err(|e| ToolHandlerError::new(format!("rename task join: {e}")))?
            .map_err(ToolHandlerError::new)?;

        self.repos
            .ui
            .send(crate::local_mode::bridge_runtime::BridgeUiCommand::BumpNoteVersion);

        let payload = json!({
            "id": note_id.to_string(),
            "old_title": old_title,
            "new_title": new_title,
            "links_rewritten": links_rewritten,
        });
        Ok(text_content(payload.to_string()))
    }
}

// ============================================================
// operon_list_recent_notes
// ============================================================

pub struct OperonListRecentNotesTool {
    repos: BridgeRepos,
}

impl OperonListRecentNotesTool {
    pub fn new(repos: BridgeRepos) -> Self {
        Self { repos }
    }
}

#[async_trait]
impl ToolHandler for OperonListRecentNotesTool {
    fn name(&self) -> &str {
        "list_recent_notes"
    }

    fn description(&self) -> &str {
        "List notes sorted by `updated_at_ms` descending — most recently edited \
         first. Scopes to a single project when `project_id` is given; otherwise \
         walks every project in the vault (requires the projects repo). Each \
         result is `{id, title, kind, project_id, updated_at_ms, created_at_ms}`. \
         Use this for 'what did I work on yesterday', 'recently changed notes', \
         or to find a note the user is iterating on without remembering its name. \
         Pass `since_ms` (unix-ms) to filter to edits after a cutoff."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "project_id": { "type": "string", "description": "Optional. Restrict to one project." },
                "limit": { "type": "integer", "description": "Optional. Default 50, max 500." },
                "since_ms": { "type": "integer", "description": "Optional unix-ms cutoff; only edits at or after this time are returned." }
            }
        })
    }

    async fn call(&self, args: Value) -> Result<Value, ToolHandlerError> {
        let scoped_pid: Option<Uuid> = match args.get("project_id") {
            Some(v) if !v.is_null() => Some(parse_uuid(&args, "project_id")?),
            _ => None,
        };
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(50)
            .clamp(1, 500);
        let since_ms: Option<i64> = args.get("since_ms").and_then(|v| v.as_i64());

        if scoped_pid.is_none() && self.repos.project_repo.is_none() {
            return Err(ToolHandlerError::new(
                "vault-wide recent listing requires the projects repo; pass `project_id`",
            ));
        }

        let note_repo = self.repos.note_repo.clone();
        let project_repo = self.repos.project_repo.clone();
        let rows: Vec<Value> = tokio::task::spawn_blocking(move || {
            let pids: Vec<Uuid> = match scoped_pid {
                Some(pid) => vec![pid],
                None => project_repo
                    .as_ref()
                    .map(|pr| pr.list().ok().unwrap_or_default())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|p| p.id)
                    .collect(),
            };
            let mut all: Vec<(Uuid, operon_store::repos::LocalNote)> = Vec::new();
            for pid in pids {
                if let Ok(notes) = note_repo.list_for_project(pid) {
                    for n in notes {
                        if since_ms.map_or(true, |cutoff| n.updated_at_ms >= cutoff) {
                            all.push((pid, n));
                        }
                    }
                }
            }
            all.sort_by(|a, b| b.1.updated_at_ms.cmp(&a.1.updated_at_ms));
            all.truncate(limit);
            all.into_iter()
                .map(|(pid, n)| {
                    json!({
                        "id": n.id.to_string(),
                        "title": n.title,
                        "kind": n.kind.as_str(),
                        "project_id": pid.to_string(),
                        "updated_at_ms": n.updated_at_ms,
                        "created_at_ms": n.created_at_ms,
                    })
                })
                .collect::<Vec<Value>>()
        })
        .await
        .map_err(|e| ToolHandlerError::new(format!("recent task join: {e}")))?;

        Ok(text_content(json!({ "results": rows }).to_string()))
    }
}

// ============================================================
// operon_get_vault_info
// ============================================================

pub struct OperonGetVaultInfoTool {
    repos: BridgeRepos,
}

impl OperonGetVaultInfoTool {
    pub fn new(repos: BridgeRepos) -> Self {
        Self { repos }
    }
}

#[async_trait]
impl ToolHandler for OperonGetVaultInfoTool {
    fn name(&self) -> &str {
        "get_vault_info"
    }

    fn description(&self) -> &str {
        "Return basic grounding info about the open vault: `{vault_path?, \
         project_count?, note_count?}`. `vault_path` is the absolute on-disk \
         path of the vault root (null if no vault is configured — rare). \
         `project_count` / `note_count` are null when the projects repo isn't \
         wired. Call once per session to know where you are."
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    async fn call(&self, _args: Value) -> Result<Value, ToolHandlerError> {
        let vault_path = self
            .repos
            .vault_root
            .as_ref()
            .map(|v| v.path().to_string_lossy().into_owned());

        let note_repo = self.repos.note_repo.clone();
        let project_repo = self.repos.project_repo.clone();
        let (project_count, note_count): (Option<usize>, Option<usize>) =
            tokio::task::spawn_blocking(move || match project_repo {
                Some(pr) => {
                    let projects = pr.list().unwrap_or_default();
                    let pc = projects.len();
                    let nc: usize = projects
                        .into_iter()
                        .map(|p| note_repo.list_for_project(p.id).map(|v| v.len()).unwrap_or(0))
                        .sum();
                    (Some(pc), Some(nc))
                }
                None => (None, None),
            })
            .await
            .map_err(|e| ToolHandlerError::new(format!("vault info task join: {e}")))?;

        let payload = json!({
            "vault_path": vault_path,
            "project_count": project_count,
            "note_count": note_count,
        });
        Ok(text_content(payload.to_string()))
    }
}

// ============================================================
// operon_reorder_note
// ============================================================

pub struct OperonReorderNoteTool {
    repos: BridgeRepos,
}

impl OperonReorderNoteTool {
    pub fn new(repos: BridgeRepos) -> Self {
        Self { repos }
    }
}

#[async_trait]
impl ToolHandler for OperonReorderNoteTool {
    fn name(&self) -> &str {
        "reorder_note"
    }

    fn description(&self) -> &str {
        "Move a note around its current siblings in the explorer tree. \
         `op` is one of: `indent` (make it a child of the previous sibling), \
         `outdent` (move up to grandparent / root), `move_up` (swap with prior \
         sibling), `move_down` (swap with next sibling). Eager; mirrors the \
         keyboard shortcuts used in the explorer. Returns `{id, op, new_depth, \
         new_sibling_index}` so you can see where it landed."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "note_id": { "type": "string", "description": "UUID of the note to move." },
                "op": {
                    "type": "string",
                    "enum": ["indent", "outdent", "move_up", "move_down"],
                    "description": "Movement to perform.",
                }
            },
            "required": ["note_id", "op"]
        })
    }

    async fn call(&self, args: Value) -> Result<Value, ToolHandlerError> {
        let note_id = parse_uuid(&args, "note_id")?;
        let op = args
            .get("op")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolHandlerError::new("op: missing or not a string"))?
            .to_string();
        let note_repo = self.repos.note_repo.clone();
        let op_for_task = op.clone();
        let (new_depth, new_sibling_index): (i64, i64) = tokio::task::spawn_blocking(move || {
            match op_for_task.as_str() {
                "indent" => note_repo.indent(note_id).map_err(|e| format!("indent: {e}"))?,
                "outdent" => note_repo.outdent(note_id).map_err(|e| format!("outdent: {e}"))?,
                "move_up" => note_repo.move_up(note_id).map_err(|e| format!("move_up: {e}"))?,
                "move_down" => note_repo
                    .move_down(note_id)
                    .map_err(|e| format!("move_down: {e}"))?,
                other => return Err(format!("op: must be indent|outdent|move_up|move_down (got {other:?})")),
            };
            // Look up the row to surface where it landed. Best-
            // effort: degrades to (-1, -1) if the lookup fails so
            // Claude still gets a deterministic shape.
            let pid = note_repo
                .find_project_for_note(note_id)
                .map_err(|e| format!("find_project_for_note: {e}"))?
                .ok_or_else(|| "note vanished after reorder".to_string())?;
            let row = note_repo
                .list_for_project(pid)
                .map_err(|e| format!("list_for_project: {e}"))?
                .into_iter()
                .find(|r| r.id == note_id)
                .ok_or_else(|| "note vanished after reorder".to_string())?;
            Ok::<(i64, i64), String>((row.depth, row.sibling_index))
        })
        .await
        .map_err(|e| ToolHandlerError::new(format!("reorder task join: {e}")))?
        .map_err(ToolHandlerError::new)?;

        self.repos
            .ui
            .send(crate::local_mode::bridge_runtime::BridgeUiCommand::BumpNoteVersion);

        let payload = json!({
            "id": note_id.to_string(),
            "op": op,
            "new_depth": new_depth,
            "new_sibling_index": new_sibling_index,
        });
        Ok(text_content(payload.to_string()))
    }
}

// ============================================================
// operon_move_note
// ============================================================

pub struct OperonMoveNoteTool {
    repos: BridgeRepos,
}

impl OperonMoveNoteTool {
    pub fn new(repos: BridgeRepos) -> Self {
        Self { repos }
    }
}

#[async_trait]
impl ToolHandler for OperonMoveNoteTool {
    fn name(&self) -> &str {
        "move_note"
    }

    fn description(&self) -> &str {
        "Move a note (and its entire subtree) to a new parent and/or project. At \
         least one of `new_project_id`, `new_parent_id`, or `new_sibling_index` \
         must be provided; missing fields default to the note's current values. \
         Pass `new_parent_id: null` to move to a project root. Rejects \
         descendant-into-self cycles. Eager. Returns `{id, project_id, \
         parent_id, sibling_index}`."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "note_id": { "type": "string", "description": "UUID of the note (and its subtree) to move." },
                "new_project_id": { "type": "string", "description": "Optional. Target project; defaults to current." },
                "new_parent_id": { "type": ["string", "null"], "description": "Optional. Target parent; null for project root; default keeps current parent." },
                "new_sibling_index": { "type": "integer", "description": "Optional. Position among siblings (0-based)." }
            },
            "required": ["note_id"]
        })
    }

    async fn call(&self, args: Value) -> Result<Value, ToolHandlerError> {
        let note_id = parse_uuid(&args, "note_id")?;
        // Defaulting strategy: pull the current row first so missing
        // args fall back to current values. Simpler than letting
        // `move_to` decide what defaults mean (the trait takes
        // concrete values for everything).
        let new_project_arg: Option<Uuid> = match args.get("new_project_id") {
            Some(v) if !v.is_null() => Some(parse_uuid(&args, "new_project_id")?),
            _ => None,
        };
        // Distinguish "absent" from "explicit null". An explicit
        // null means "move to project root"; absent means "keep
        // current parent".
        let parent_arg: Option<Option<Uuid>> = match args.get("new_parent_id") {
            Some(Value::Null) => Some(None),
            Some(v) if v.is_string() => Some(Some(parse_uuid(&args, "new_parent_id")?)),
            Some(other) => {
                return Err(ToolHandlerError::new(format!(
                    "new_parent_id: must be a uuid string or null (got {other})"
                )));
            }
            None => None,
        };
        let new_sibling_arg: Option<i64> = args.get("new_sibling_index").and_then(|v| v.as_i64());

        if new_project_arg.is_none() && parent_arg.is_none() && new_sibling_arg.is_none() {
            return Err(ToolHandlerError::new(
                "move_note: provide at least one of new_project_id / new_parent_id / new_sibling_index",
            ));
        }

        let note_repo = self.repos.note_repo.clone();
        let resolved: (Uuid, Option<Uuid>, i64) = tokio::task::spawn_blocking(move || {
            // Fetch current row to fill in missing defaults.
            let current_pid = note_repo
                .find_project_for_note(note_id)
                .map_err(|e| format!("find_project_for_note: {e}"))?
                .ok_or_else(|| format!("note {note_id} not found"))?;
            let current = note_repo
                .list_for_project(current_pid)
                .map_err(|e| format!("list_for_project: {e}"))?
                .into_iter()
                .find(|r| r.id == note_id)
                .ok_or_else(|| format!("note {note_id} not in project list"))?;

            let target_project = new_project_arg.unwrap_or(current_pid);
            let target_parent = match parent_arg {
                Some(opt) => opt,
                None => current.parent_id,
            };
            let target_sibling = new_sibling_arg.unwrap_or(current.sibling_index);

            note_repo
                .move_to(note_id, target_project, target_parent, target_sibling)
                .map_err(|e| format!("move_to: {e}"))?;
            Ok::<(Uuid, Option<Uuid>, i64), String>((target_project, target_parent, target_sibling))
        })
        .await
        .map_err(|e| ToolHandlerError::new(format!("move task join: {e}")))?
        .map_err(ToolHandlerError::new)?;

        self.repos
            .ui
            .send(crate::local_mode::bridge_runtime::BridgeUiCommand::BumpNoteVersion);

        let payload = json!({
            "id": note_id.to_string(),
            "project_id": resolved.0.to_string(),
            "parent_id": resolved.1.map(|u| u.to_string()),
            "sibling_index": resolved.2,
        });
        Ok(text_content(payload.to_string()))
    }
}

// ============================================================
// operon_list_attachments
// ============================================================

pub struct OperonListAttachmentsTool {
    repos: BridgeRepos,
}

impl OperonListAttachmentsTool {
    pub fn new(repos: BridgeRepos) -> Self {
        Self { repos }
    }
}

#[async_trait]
impl ToolHandler for OperonListAttachmentsTool {
    fn name(&self) -> &str {
        "list_attachments"
    }

    fn description(&self) -> &str {
        "List all attachments pinned to a note. Each entry returns `{id, filename, \
         mime_type, sha256_hex, size_bytes, blob_path, created_at_ms}`. Use after \
         `get_note` if you need to enumerate what's attached (images, files) so \
         you can reference one in a `replace_note_range` edit."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "note_id": { "type": "string", "description": "UUID of the parent note." }
            },
            "required": ["note_id"]
        })
    }

    async fn call(&self, args: Value) -> Result<Value, ToolHandlerError> {
        let note_id = parse_uuid(&args, "note_id")?;
        let attachment_repo = self.repos.attachment_repo.clone();
        let attachments = tokio::task::spawn_blocking(move || {
            attachment_repo.list_by_note(note_id)
        })
        .await
        .map_err(|e| ToolHandlerError::new(format!("attachment task join: {e}")))?
        .map_err(|e| ToolHandlerError::new(format!("list_by_note: {e}")))?;

        let rows: Vec<Value> = attachments
            .into_iter()
            .map(|a| {
                json!({
                    "id": a.id.to_string(),
                    "filename": a.filename,
                    "mime_type": a.mime_type,
                    "sha256_hex": a.sha256_hex,
                    "size_bytes": a.size_bytes,
                    "blob_path": a.blob_path,
                    "created_at_ms": a.created_at_ms,
                })
            })
            .collect();

        Ok(text_content(json!({ "attachments": rows }).to_string()))
    }
}

// ============================================================
// operon_delete_attachment
// ============================================================

pub struct OperonDeleteAttachmentTool {
    repos: BridgeRepos,
}

impl OperonDeleteAttachmentTool {
    pub fn new(repos: BridgeRepos) -> Self {
        Self { repos }
    }
}

#[async_trait]
impl ToolHandler for OperonDeleteAttachmentTool {
    fn name(&self) -> &str {
        "delete_attachment"
    }

    fn description(&self) -> &str {
        "Delete an attachment row by its id. Eager. The blob on disk is \
         content-addressed under `<vault>/.operon/images/<sha>.<ext>` and may \
         be shared with image notes or other attachments — after the row \
         delete, the bridge runs a refcount-based GC: if no `local_note` or \
         `local_attachments` row still references the blob_path, the file is \
         unlinked. Returns `{deleted: true, attachment_id, blob_gc_count}` \
         where `blob_gc_count` is 0 or 1 (0 means the blob is still \
         referenced elsewhere)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "attachment_id": { "type": "string", "description": "UUID-shaped id of the attachment row." }
            },
            "required": ["attachment_id"]
        })
    }

    async fn call(&self, args: Value) -> Result<Value, ToolHandlerError> {
        let attachment_id = parse_uuid(&args, "attachment_id")?;
        let attachment_repo = self.repos.attachment_repo.clone();
        let note_repo = self.repos.note_repo.clone();
        let vault = self.repos.vault_root.clone();

        // Snapshot blob_path BEFORE the delete (the row is gone
        // after), then commit the delete, then GC if we still have
        // a vault on hand. Missing vault → SQL delete still
        // succeeds, GC just doesn't run.
        let blob_gc_count: usize = tokio::task::spawn_blocking(move || {
            let blob_path = attachment_repo
                .get(attachment_id)
                .ok()
                .flatten()
                .map(|a| a.blob_path);
            attachment_repo
                .delete(attachment_id)
                .map_err(|e| format!("delete: {e}"))?;
            let gc = match (vault, blob_path) {
                (Some(vault), Some(path)) => crate::local_mode::images::gc_unreferenced_blobs(
                    &vault,
                    &[path],
                    &|p| note_repo.count_by_blob_path(p).unwrap_or(0),
                    &|p| attachment_repo.count_by_blob_path(p).unwrap_or(0),
                ),
                _ => 0,
            };
            Ok::<usize, String>(gc)
        })
        .await
        .map_err(|e| ToolHandlerError::new(format!("delete task join: {e}")))?
        .map_err(ToolHandlerError::new)?;

        Ok(text_content(
            json!({
                "deleted": true,
                "attachment_id": attachment_id.to_string(),
                "blob_gc_count": blob_gc_count,
            })
            .to_string(),
        ))
    }
}

// ============================================================
// operon_create_image_note + operon_attach_image_to_note
// ============================================================

/// Decode a base64 image, validate mime + size, and write the blob
/// content-addressed under the vault. Shared between the
/// create-image-note and attach-image-to-note tools.
///
/// Returns the `ImageWrite` (relative path, sha, mime, size). On
/// error returns a `ToolHandlerError` with a human-readable message
/// so Claude can recover (typically by asking the user for a
/// smaller image or a supported format).
#[cfg(not(target_arch = "wasm32"))]
fn decode_and_write_image(
    vault: &crate::local_mode::vault::VaultRoot,
    image_base64: &str,
    mime_type: &str,
) -> Result<crate::local_mode::images::ImageWrite, ToolHandlerError> {
    use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
    let bytes = B64
        .decode(image_base64)
        .map_err(|e| ToolHandlerError::new(format!("image_base64: invalid base64 ({e})")))?;
    crate::local_mode::images::write_image(vault, &bytes, mime_type).map_err(|e| {
        ToolHandlerError::new(format!("write_image: {e}"))
    })
}

pub struct OperonCreateImageNoteTool {
    repos: BridgeRepos,
}

impl OperonCreateImageNoteTool {
    pub fn new(repos: BridgeRepos) -> Self {
        Self { repos }
    }
}

#[async_trait]
impl ToolHandler for OperonCreateImageNoteTool {
    fn name(&self) -> &str {
        "create_image_note"
    }

    fn description(&self) -> &str {
        "Create a new image note from a base64-encoded image. The blob is hashed \
         and stored content-addressed under `<vault>/.operon/images/`, so the \
         same image pasted twice deduplicates. Returns `{id, title, project_id, \
         blob_path, sha256_hex, size_bytes, embed_markdown}` where `embed_markdown` \
         is the wikilink form (`![[<title>^<short_id>]]`) you can paste into \
         another note via `replace_note_range`. Supported mime types: image/png, \
         image/jpeg, image/webp, image/gif, image/svg+xml, image/avif. \
         \
         Practical size limit is ~600 KB pre-base64 because the MCP bridge frames \
         each call at 1 MB; the underlying writer accepts up to 25 MB if you find \
         a way around that. Use when the user pastes a screenshot and asks for it \
         to be saved as a standalone note in the vault."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "project_id": { "type": "string", "description": "Project UUID the note belongs to." },
                "parent_id": { "type": "string", "description": "Optional parent UUID for nesting." },
                "title": { "type": "string", "description": "Display title; also used as the embed stem." },
                "image_base64": { "type": "string", "description": "Image bytes, base64-encoded." },
                "mime_type": { "type": "string", "description": "e.g. \"image/png\". Required so we can pick the extension." }
            },
            "required": ["project_id", "title", "image_base64", "mime_type"]
        })
    }

    async fn call(&self, args: Value) -> Result<Value, ToolHandlerError> {
        let project_id = parse_uuid(&args, "project_id")?;
        let parent_id = match args.get("parent_id") {
            Some(v) if !v.is_null() => Some(parse_uuid(&args, "parent_id")?),
            _ => None,
        };
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolHandlerError::new("title: missing or not a string"))?
            .to_string();
        let image_base64 = args
            .get("image_base64")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolHandlerError::new("image_base64: missing or not a string"))?
            .to_string();
        let mime_type = args
            .get("mime_type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolHandlerError::new("mime_type: missing or not a string"))?
            .to_string();
        let vault = self.repos.vault_root.clone().ok_or_else(|| {
            ToolHandlerError::new(
                "no vault configured — the user needs to pick a vault before image notes can be created",
            )
        })?;

        let note_repo = self.repos.note_repo.clone();
        let persistence = self.repos.persistence.clone();
        let title_for_task = title.clone();
        let (note_id, blob_path, sha256_hex, size_bytes) =
            tokio::task::spawn_blocking(move || {
                let written = decode_and_write_image(&vault, &image_base64, &mime_type)?;
                let new_note = note_repo
                    .create_with_kind(project_id, parent_id, &title_for_task, NoteKind::Image)
                    .map_err(|e| ToolHandlerError::new(format!("create_with_kind: {e}")))?;
                let rel = written.relative_path.to_string_lossy().into_owned();
                if let Err(e) = note_repo.set_blob_path(new_note.id, Some(&rel)) {
                    return Err(ToolHandlerError::new(format!(
                        "row {} created but set_blob_path failed: {e}",
                        new_note.id
                    )));
                }
                // Image notes have empty bodies in Loro — the blob
                // IS the content. Persisting an empty body keeps
                // resolved_path consistent with non-image kinds.
                let id_str = new_note.id.to_string();
                let _ = futures::executor::block_on(persistence.save(&id_str, &[]));
                Ok::<_, ToolHandlerError>((
                    new_note.id,
                    rel,
                    written.sha256_hex,
                    written.size_bytes,
                ))
            })
            .await
            .map_err(|e| ToolHandlerError::new(format!("create image task join: {e}")))??;

        self.repos
            .ui
            .send(crate::local_mode::bridge_runtime::BridgeUiCommand::BumpNoteVersion);

        let short = operon_store::vfs::short_id(note_id);
        let embed_markdown = format!("![[{title}^{short}]]");
        let payload = json!({
            "id": note_id.to_string(),
            "title": title,
            "project_id": project_id.to_string(),
            "blob_path": blob_path,
            "sha256_hex": sha256_hex,
            "size_bytes": size_bytes,
            "embed_markdown": embed_markdown,
        });
        Ok(text_content(payload.to_string()))
    }
}

pub struct OperonAttachImageToNoteTool {
    repos: BridgeRepos,
}

impl OperonAttachImageToNoteTool {
    pub fn new(repos: BridgeRepos) -> Self {
        Self { repos }
    }
}

#[async_trait]
impl ToolHandler for OperonAttachImageToNoteTool {
    fn name(&self) -> &str {
        "attach_image_to_note"
    }

    fn description(&self) -> &str {
        "Attach a base64-encoded image to an existing note. The blob is hashed \
         and stored content-addressed under `<vault>/.operon/images/` — same \
         pool that image notes use, so duplicate bytes deduplicate. Creates an \
         `attachments` row pinning the blob to `note_id`. Returns `{attachment_id, \
         note_id, blob_path, sha256_hex, size_bytes, embed_markdown}` where \
         `embed_markdown` is plain markdown image syntax (`![alt](relative_path)`) \
         you can paste into the note's body via `replace_note_range`. \
         \
         Supported mime types and size limits match `create_image_note`. Use this \
         when the user pastes a screenshot they want associated with an existing \
         note rather than as a standalone vault entry."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "note_id": { "type": "string", "description": "UUID of the parent note." },
                "image_base64": { "type": "string", "description": "Image bytes, base64-encoded." },
                "filename": { "type": "string", "description": "Display filename (e.g. \"screenshot.png\")." },
                "mime_type": { "type": "string", "description": "e.g. \"image/png\". Required." },
                "alt_text": { "type": "string", "description": "Optional alt text for the embed markdown." }
            },
            "required": ["note_id", "image_base64", "filename", "mime_type"]
        })
    }

    async fn call(&self, args: Value) -> Result<Value, ToolHandlerError> {
        let note_id = parse_uuid(&args, "note_id")?;
        let image_base64 = args
            .get("image_base64")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolHandlerError::new("image_base64: missing or not a string"))?
            .to_string();
        let filename = args
            .get("filename")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolHandlerError::new("filename: missing or not a string"))?
            .to_string();
        let mime_type = args
            .get("mime_type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolHandlerError::new("mime_type: missing or not a string"))?
            .to_string();
        let alt_text = args
            .get("alt_text")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| filename.clone());
        let vault = self.repos.vault_root.clone().ok_or_else(|| {
            ToolHandlerError::new(
                "no vault configured — pick a vault before attaching images",
            )
        })?;

        let attachment_repo = self.repos.attachment_repo.clone();
        let filename_for_task = filename.clone();
        let mime_for_task = mime_type.clone();
        let (attachment_id_str, blob_path, sha256_hex, size_bytes) =
            tokio::task::spawn_blocking(move || {
                let written = decode_and_write_image(&vault, &image_base64, &mime_for_task)?;
                let rel = written.relative_path.to_string_lossy().into_owned();
                let mut attachment = operon_store::repos::LocalAttachment::new(
                    note_id,
                    &filename_for_task,
                    &written.sha256_hex,
                    written.size_bytes as i64,
                    &rel,
                );
                attachment.mime_type = Some(mime_for_task);
                let aid_str = attachment.id.to_string();
                attachment_repo
                    .create(&attachment)
                    .map_err(|e| ToolHandlerError::new(format!("create attachment: {e}")))?;
                Ok::<_, ToolHandlerError>((
                    aid_str,
                    rel,
                    written.sha256_hex,
                    written.size_bytes,
                ))
            })
            .await
            .map_err(|e| ToolHandlerError::new(format!("attach image task join: {e}")))??;

        let embed_markdown = format!("![{alt_text}]({blob_path})");
        let payload = json!({
            "attachment_id": attachment_id_str,
            "note_id": note_id.to_string(),
            "blob_path": blob_path,
            "sha256_hex": sha256_hex,
            "size_bytes": size_bytes,
            "embed_markdown": embed_markdown,
        });
        Ok(text_content(payload.to_string()))
    }
}

// ============================================================
// Project + repo CRUD + skill install / materialize tools.
//
// Shared helpers + five `ToolHandler` impls (`create_project`,
// `update_project`, `delete_project`, `install_seed_skills`,
// `materialize_skills_to_disk`). All five funnel through
// `BridgeRepos.project_repo` (an Option — errors if unwired, matching
// `list_projects` at :517-580) and reuse the explorer's existing
// install / materialize cores so the GUI and MCP paths can't drift.
// ============================================================

/// Pull the `project_repo` out of `BridgeRepos`, erroring with the
/// same wording `list_projects` uses when the GUI didn't wire one.
/// Hoisted so every project-tier tool returns the same message.
fn project_repo(
    repos: &BridgeRepos,
) -> Result<std::sync::Arc<dyn operon_store::repos::LocalProjectRepository>, ToolHandlerError> {
    repos
        .project_repo
        .clone()
        .ok_or_else(|| ToolHandlerError::new("project repo not wired"))
}

/// Parse `args.field` as an absolute path string. `None` when the
/// caller omitted the key. Returns a `ToolHandlerError` only when the
/// key is present but malformed (not a string, or a relative path).
/// Relative paths are rejected because `companion_chat::cwd_for_scope`
/// feeds `repo_path` straight into the terminal as `cwd`; a relative
/// path would resolve against an unpredictable working directory.
fn parse_abs_path(
    args: &Value,
    field: &str,
) -> Result<Option<std::path::PathBuf>, ToolHandlerError> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            let path = std::path::PathBuf::from(s);
            if !path.is_absolute() {
                return Err(ToolHandlerError::new(format!(
                    "{field}: must be an absolute path, got {s}"
                )));
            }
            Ok(Some(path))
        }
        Some(other) => Err(ToolHandlerError::new(format!(
            "{field}: must be a string or null, got {other}"
        ))),
    }
}

/// Three-state JSON field decoder used by `update_project`. Returns:
///   * `None` when the field is absent (caller wants no change),
///   * `Some(None)` when the field is `null` (caller wants to clear),
///   * `Some(Some(s))` when the field is a string.
fn three_state_string(args: &Value, field: &str) -> Result<Option<Option<String>>, ToolHandlerError> {
    match args.get(field) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(s)) => Ok(Some(Some(s.clone()))),
        Some(other) => Err(ToolHandlerError::new(format!(
            "{field}: must be a string or null, got {other}"
        ))),
    }
}

/// Build the canonical JSON shape `list_projects` returns, for a
/// single project. Centralised so create/update/delete return the
/// same fields callers can pipe straight back into other tools.
fn project_to_json(p: &operon_store::repos::LocalProject) -> Value {
    json!({
        "id": p.id.to_string(),
        "name": p.name,
        "sibling_index": p.sibling_index,
        "created_at_ms": p.created_at_ms,
        "updated_at_ms": p.updated_at_ms,
        "repo_path": p.repo_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
        "default_model": p.default_model.clone(),
        "default_permission_mode": p.default_permission_mode.clone(),
    })
}

/// Walk every `NoteKind::Skill` row under the project and call
/// `write_skill_to_repo` for each one (after loading its body via
/// `Persistence`). The skill's title is used as the slug — the
/// existing `to_claude_compat` only injects `name:` when the body
/// doesn't already supply one, so titled-as-slug works whether the
/// skill came from a seed file or a hand-authored note.
///
/// Returns the absolute paths of every file written so the caller can
/// surface them in the tool response. Skills with empty bodies are
/// silently skipped (matching `write_skill_to_repo`'s behaviour).
///
/// The whole pass runs inside `spawn_blocking` because
/// `Persistence::load` returns a `!Send` future — same workaround the
/// other write-side tools (`create_note`, `replace_note_range`) use.
async fn materialize_all_skills(
    note_repo: std::sync::Arc<dyn operon_store::repos::LocalNoteRepository>,
    persistence: std::sync::Arc<dyn crate::persistence::Persistence>,
    project_id: Uuid,
    repo_path: std::path::PathBuf,
) -> Result<Vec<String>, ToolHandlerError> {
    tokio::task::spawn_blocking(move || -> Result<Vec<String>, String> {
        let skills: Vec<(Uuid, String)> = note_repo
            .list_for_project(project_id)
            .map_err(|e| format!("list_for_project: {e}"))?
            .into_iter()
            .filter(|n| matches!(n.kind, operon_store::repos::NoteKind::Skill))
            .map(|n| (n.id, n.title))
            .collect();
        let mut written: Vec<String> = Vec::with_capacity(skills.len());
        for (note_id, title) in skills {
            let bytes = futures::executor::block_on(persistence.load(&note_id.to_string()))
                .map_err(|e| format!("load skill {title}: {e}"))?;
            let body = String::from_utf8(bytes)
                .map_err(|e| format!("decode skill {title}: {e}"))?;
            match write_skill_to_repo(&repo_path, &title, &body) {
                Ok(path) => written.push(path.to_string_lossy().into_owned()),
                Err(MaterializeError::EmptyBody) => {
                    eprintln!("operon: materialize skipped empty body for {title}");
                }
                Err(e) => return Err(format!("write_skill_to_repo({title}): {e}")),
            }
        }
        Ok(written)
    })
    .await
    .map_err(|e| ToolHandlerError::new(format!("materialize task join: {e}")))?
    .map_err(ToolHandlerError::new)
}

/// Vault-only install of every embedded seed skill into `project_id`.
/// Wraps `install_skills_into_project` with the embedded `SEED_SKILLS`
/// bundle. Returns the report (installed / skipped / failed counts).
/// Whole call lives inside `spawn_blocking` for the same `!Send`
/// reason as `materialize_all_skills`.
async fn install_seed_skills_vault(
    note_repo: std::sync::Arc<dyn operon_store::repos::LocalNoteRepository>,
    persistence: std::sync::Arc<dyn crate::persistence::Persistence>,
    project_id: Uuid,
) -> Result<crate::plugins::skill::install::SkillInstallReport, ToolHandlerError> {
    tokio::task::spawn_blocking(move || {
        let entries: Vec<(String, String)> = seed_skill_list()
            .map(|s| (s.stem.to_string(), s.body.to_string()))
            .collect();
        let readme = seed_readme().map(str::to_string);
        let skill_iter = entries.iter().map(|(stem, body)| SkillSource {
            stem: stem.as_str(),
            body: body.as_str(),
        });
        futures::executor::block_on(install_skills_into_project(
            &note_repo,
            &persistence,
            project_id,
            skill_iter,
            readme.as_deref(),
        ))
    })
    .await
    .map_err(|e| ToolHandlerError::new(format!("install seed skills task join: {e}")))?
    .map_err(ToolHandlerError::new)
}

// ------------------------------------------------------------
// operon_create_project
// ------------------------------------------------------------

pub struct OperonCreateProjectTool {
    repos: BridgeRepos,
}

impl OperonCreateProjectTool {
    pub fn new(repos: BridgeRepos) -> Self {
        Self { repos }
    }
}

#[async_trait]
impl ToolHandler for OperonCreateProjectTool {
    fn name(&self) -> &str {
        "create_project"
    }

    fn description(&self) -> &str {
        "Create a new top-level project (sibling of existing ones, appended at the end). \
         Optional `repo_path` binds it to a git repository — once bound, the companion's \
         native Bash/Read/Edit tools land in that directory when the user chats in Project \
         scope. Optional `install_seed_skills` (default true) populates the project with \
         the 15 SDLC cascade skills as `Skill` notes under a `SKILLS` index note; idempotent \
         on repeated calls. Optional `materialize_skills_to_disk` (default false) ALSO \
         writes those skills as `<repo>/.claude/skills/<slug>.md` so Claude Code's skill \
         loader can discover them — requires `repo_path` to be set. Returns the full \
         project row plus install / materialize counts. Pass `install_seed_skills: false` \
         to skip the SDLC chain for a vanilla project."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Project name shown in the explorer. Required.",
                },
                "repo_path": {
                    "type": "string",
                    "description": "Optional. Absolute path to a git repo. \
                                    Companion's working directory in Project scope.",
                },
                "default_model": {
                    "type": "string",
                    "description": "Optional. Project-tier override for `claude --model`.",
                },
                "default_permission_mode": {
                    "type": "string",
                    "description": "Optional. Project-tier override for `claude --permission-mode`.",
                },
                "install_seed_skills": {
                    "type": "boolean",
                    "description": "Optional, default true. Install the SDLC seed skills \
                                    as Skill notes under a SKILLS index note.",
                },
                "materialize_skills_to_disk": {
                    "type": "boolean",
                    "description": "Optional, default false. Also write the installed \
                                    skills to <repo>/.claude/skills/. Requires repo_path.",
                },
            },
            "required": ["name"],
        })
    }

    async fn call(&self, args: Value) -> Result<Value, ToolHandlerError> {
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolHandlerError::new("name: missing or not a string"))?
            .trim()
            .to_string();
        if name.is_empty() {
            return Err(ToolHandlerError::new("name: must be non-empty"));
        }
        let repo_path = parse_abs_path(&args, "repo_path")?;
        let default_model = args
            .get("default_model")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let default_permission_mode = args
            .get("default_permission_mode")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let install_seeds = args
            .get("install_seed_skills")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let materialize = args
            .get("materialize_skills_to_disk")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if materialize && repo_path.is_none() {
            return Err(ToolHandlerError::new(
                "materialize_skills_to_disk=true requires repo_path to be set",
            ));
        }

        let project_repo = project_repo(&self.repos)?;
        let note_repo = self.repos.note_repo.clone();
        let persistence = self.repos.persistence.clone();

        tracing::info!(
            target: "operon::bridge::tools",
            name = %name,
            install_seeds,
            materialize,
            "create_project: start"
        );

        // Step 1: create + apply settings on the blocking pool.
        let project_id: Uuid = tokio::task::spawn_blocking({
            let project_repo = project_repo.clone();
            let repo_path = repo_path.clone();
            let default_model = default_model.clone();
            let default_permission_mode = default_permission_mode.clone();
            move || -> Result<Uuid, String> {
                let row = project_repo
                    .create(&name)
                    .map_err(|e| format!("create project: {e}"))?;
                if let Some(rp) = repo_path.as_ref() {
                    project_repo
                        .set_repo_path(row.id, Some(rp.as_path()))
                        .map_err(|e| format!("set_repo_path: {e}"))?;
                }
                if let Some(m) = default_model.as_deref() {
                    project_repo
                        .set_default_model(row.id, Some(m))
                        .map_err(|e| format!("set_default_model: {e}"))?;
                }
                if let Some(pm) = default_permission_mode.as_deref() {
                    project_repo
                        .set_default_permission_mode(row.id, Some(pm))
                        .map_err(|e| format!("set_default_permission_mode: {e}"))?;
                }
                Ok(row.id)
            }
        })
        .await
        .map_err(|e| ToolHandlerError::new(format!("create project task join: {e}")))?
        .map_err(ToolHandlerError::new)?;

        tracing::info!(
            target: "operon::bridge::tools",
            project_id = %project_id,
            "create_project: step 1 done (row + settings)"
        );

        // Step 2: install seed skills (vault). Always idempotent.
        let install_report = if install_seeds {
            Some(
                install_seed_skills_vault(note_repo.clone(), persistence.clone(), project_id)
                    .await?,
            )
        } else {
            None
        };

        tracing::info!(
            target: "operon::bridge::tools",
            installed = install_report.as_ref().map(|r| r.installed).unwrap_or(0),
            skipped = install_report.as_ref().map(|r| r.skipped).unwrap_or(0),
            "create_project: step 2 done (seed skills)"
        );

        // Step 3: materialize to disk if asked. Reuses the vault Skill
        // notes just created so on-disk and vault stay in sync.
        let materialized_files = if materialize {
            // unwrap is fine — guarded above.
            let rp = repo_path.clone().unwrap();
            Some(materialize_all_skills(note_repo.clone(), persistence.clone(), project_id, rp).await?)
        } else {
            None
        };

        // Step 4: refetch canonical row + bump UI.
        let final_row = tokio::task::spawn_blocking({
            let project_repo = project_repo.clone();
            move || {
                project_repo
                    .get(project_id)
                    .map_err(|e| format!("get project: {e}"))?
                    .ok_or_else(|| format!("project {project_id} disappeared after create"))
            }
        })
        .await
        .map_err(|e| ToolHandlerError::new(format!("refetch project task join: {e}")))?
        .map_err(ToolHandlerError::new)?;

        tracing::info!(
            target: "operon::bridge::tools",
            project_id = %project_id,
            "create_project: step 4 done (refetched row); bumping UI"
        );

        self.repos
            .ui
            .send(crate::local_mode::bridge_runtime::BridgeUiCommand::BumpNoteVersion);

        let mut payload = project_to_json(&final_row);
        if let Some(rep) = install_report {
            payload["seed_skills_installed"] = json!(rep.installed);
            payload["seed_skills_skipped"] = json!(rep.skipped);
        } else {
            payload["seed_skills_installed"] = Value::Null;
        }
        if let Some(files) = materialized_files {
            payload["skills_materialized"] = json!(files.len());
            payload["materialized_files"] = json!(files);
        } else {
            payload["skills_materialized"] = Value::Null;
        }
        Ok(text_content(payload.to_string()))
    }
}

// ------------------------------------------------------------
// operon_update_project
// ------------------------------------------------------------

pub struct OperonUpdateProjectTool {
    repos: BridgeRepos,
}

impl OperonUpdateProjectTool {
    pub fn new(repos: BridgeRepos) -> Self {
        Self { repos }
    }
}

#[async_trait]
impl ToolHandler for OperonUpdateProjectTool {
    fn name(&self) -> &str {
        "update_project"
    }

    fn description(&self) -> &str {
        "Update a project's mutable fields. Three-state per field: omit to leave unchanged, \
         pass null to clear, pass a string to set. `name` rename (string only, no null). \
         `repo_path` binds / re-binds / unbinds the git repo — must be absolute when set. \
         `default_model` / `default_permission_mode` set / clear the project-tier overrides. \
         Returns the post-update project row (same shape as list_projects)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string", "description": "Project UUID. Required." },
                "name": { "type": "string", "description": "Optional. New name (non-empty)." },
                "repo_path": {
                    "type": ["string", "null"],
                    "description": "Optional. Absolute path, or null to unbind.",
                },
                "default_model": {
                    "type": ["string", "null"],
                    "description": "Optional. Model override, or null to clear.",
                },
                "default_permission_mode": {
                    "type": ["string", "null"],
                    "description": "Optional. Permission-mode override, or null to clear.",
                },
            },
            "required": ["id"],
        })
    }

    async fn call(&self, args: Value) -> Result<Value, ToolHandlerError> {
        let id = parse_uuid(&args, "id")?;
        let name = match args.get("name") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => {
                let trimmed = s.trim().to_string();
                if trimmed.is_empty() {
                    return Err(ToolHandlerError::new("name: must be non-empty when set"));
                }
                Some(trimmed)
            }
            Some(other) => {
                return Err(ToolHandlerError::new(format!(
                    "name: must be a string, got {other}"
                )))
            }
        };
        // repo_path uses three-state because we want explicit `null`
        // to mean "unbind" — distinct from "leave alone".
        let repo_path_arg = match args.get("repo_path") {
            None => None,
            Some(Value::Null) => Some(None),
            Some(Value::String(s)) => {
                let p = std::path::PathBuf::from(s);
                if !p.is_absolute() {
                    return Err(ToolHandlerError::new(format!(
                        "repo_path: must be an absolute path, got {s}"
                    )));
                }
                Some(Some(p))
            }
            Some(other) => {
                return Err(ToolHandlerError::new(format!(
                    "repo_path: must be a string or null, got {other}"
                )))
            }
        };
        let default_model_arg = three_state_string(&args, "default_model")?;
        let default_permission_mode_arg =
            three_state_string(&args, "default_permission_mode")?;

        let project_repo = project_repo(&self.repos)?;

        tokio::task::spawn_blocking({
            let project_repo = project_repo.clone();
            move || -> Result<(), String> {
                if let Some(new_name) = name.as_deref() {
                    project_repo
                        .rename(id, new_name)
                        .map_err(|e| format!("rename: {e}"))?;
                }
                if let Some(rp_opt) = repo_path_arg.as_ref() {
                    project_repo
                        .set_repo_path(id, rp_opt.as_deref())
                        .map_err(|e| format!("set_repo_path: {e}"))?;
                }
                if let Some(m_opt) = default_model_arg.as_ref() {
                    project_repo
                        .set_default_model(id, m_opt.as_deref())
                        .map_err(|e| format!("set_default_model: {e}"))?;
                }
                if let Some(pm_opt) = default_permission_mode_arg.as_ref() {
                    project_repo
                        .set_default_permission_mode(id, pm_opt.as_deref())
                        .map_err(|e| format!("set_default_permission_mode: {e}"))?;
                }
                Ok(())
            }
        })
        .await
        .map_err(|e| ToolHandlerError::new(format!("update project task join: {e}")))?
        .map_err(ToolHandlerError::new)?;

        let final_row = tokio::task::spawn_blocking({
            let project_repo = project_repo.clone();
            move || {
                project_repo
                    .get(id)
                    .map_err(|e| format!("get project: {e}"))?
                    .ok_or_else(|| format!("project {id} not found"))
            }
        })
        .await
        .map_err(|e| ToolHandlerError::new(format!("refetch project task join: {e}")))?
        .map_err(ToolHandlerError::new)?;

        self.repos
            .ui
            .send(crate::local_mode::bridge_runtime::BridgeUiCommand::BumpNoteVersion);
        Ok(text_content(project_to_json(&final_row).to_string()))
    }
}

// ------------------------------------------------------------
// operon_delete_project
// ------------------------------------------------------------

pub struct OperonDeleteProjectTool {
    repos: BridgeRepos,
}

impl OperonDeleteProjectTool {
    pub fn new(repos: BridgeRepos) -> Self {
        Self { repos }
    }
}

#[async_trait]
impl ToolHandler for OperonDeleteProjectTool {
    fn name(&self) -> &str {
        "delete_project"
    }

    fn description(&self) -> &str {
        "Delete a project and every note inside it. Requires `confirm: true` to proceed — \
         otherwise the tool errors with the impact summary (counts of notes and materialized \
         skill files that WOULD be removed) so you can show it to the user and have them \
         confirm. When `confirm: true`, cascades through the FK to remove notes, then \
         unlinks any `<repo>/.claude/skills/*.md` files that came from this project's Skill \
         notes. Returns `{id, deleted: true, notes_removed, skills_unmaterialized}`. \
         No undo."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string", "description": "Project UUID." },
                "confirm": {
                    "type": "boolean",
                    "description": "Required (default false). Must be true to proceed.",
                },
            },
            "required": ["id"],
        })
    }

    async fn call(&self, args: Value) -> Result<Value, ToolHandlerError> {
        let id = parse_uuid(&args, "id")?;
        let confirm = args.get("confirm").and_then(|v| v.as_bool()).unwrap_or(false);

        let project_repo = project_repo(&self.repos)?;
        let note_repo = self.repos.note_repo.clone();

        // Resolve impact before doing anything destructive: name,
        // note count, current `repo_path` + slugs for the skill-file
        // cleanup. We need the path NOW because `project_repo.delete`
        // cascades the row away before we get a chance to read it.
        let preview = tokio::task::spawn_blocking({
            let project_repo = project_repo.clone();
            let note_repo = note_repo.clone();
            move || -> Result<(String, Option<std::path::PathBuf>, Vec<String>, usize), String> {
                let project = project_repo
                    .get(id)
                    .map_err(|e| format!("get project: {e}"))?
                    .ok_or_else(|| format!("project {id} not found"))?;
                let notes = note_repo
                    .list_for_project(id)
                    .map_err(|e| format!("list_for_project: {e}"))?;
                let skill_slugs: Vec<String> = notes
                    .iter()
                    .filter(|n| matches!(n.kind, operon_store::repos::NoteKind::Skill))
                    .map(|n| n.title.clone())
                    .collect();
                Ok((project.name, project.repo_path, skill_slugs, notes.len()))
            }
        })
        .await
        .map_err(|e| ToolHandlerError::new(format!("delete preview task join: {e}")))?
        .map_err(ToolHandlerError::new)?;
        let (project_name, repo_path, skill_slugs, note_count) = preview;

        if !confirm {
            return Err(ToolHandlerError::new(format!(
                "delete_project requires confirm=true. \
                 Impact: project \"{project_name}\" ({id}) has {note_count} notes \
                 ({} of which are skills); repo_path={}. Show the user this summary \
                 and re-call with confirm=true to proceed.",
                skill_slugs.len(),
                repo_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "<unbound>".into()),
            )));
        }

        // Unlink materialized skill files first so a failure here
        // surfaces before the irreversible row delete. Best-effort:
        // missing files are not errors (remove_skill_from_repo is
        // already no-op on missing).
        let mut skills_unmaterialized = 0usize;
        if let Some(rp) = repo_path.as_ref() {
            for slug in &skill_slugs {
                match remove_skill_from_repo(rp, slug) {
                    Ok(()) => skills_unmaterialized += 1,
                    Err(e) => eprintln!(
                        "operon: delete_project remove_skill_from_repo({slug}): {e}"
                    ),
                }
            }
        }

        tokio::task::spawn_blocking({
            let project_repo = project_repo.clone();
            move || {
                project_repo
                    .delete(id)
                    .map_err(|e| format!("delete project: {e}"))
            }
        })
        .await
        .map_err(|e| ToolHandlerError::new(format!("delete project task join: {e}")))?
        .map_err(ToolHandlerError::new)?;

        self.repos
            .ui
            .send(crate::local_mode::bridge_runtime::BridgeUiCommand::BumpNoteVersion);

        let payload = json!({
            "id": id.to_string(),
            "deleted": true,
            "notes_removed": note_count,
            "skills_unmaterialized": skills_unmaterialized,
        });
        Ok(text_content(payload.to_string()))
    }
}

// ------------------------------------------------------------
// operon_install_seed_skills
// ------------------------------------------------------------

pub struct OperonInstallSeedSkillsTool {
    repos: BridgeRepos,
}

impl OperonInstallSeedSkillsTool {
    pub fn new(repos: BridgeRepos) -> Self {
        Self { repos }
    }
}

#[async_trait]
impl ToolHandler for OperonInstallSeedSkillsTool {
    fn name(&self) -> &str {
        "install_seed_skills"
    }

    fn description(&self) -> &str {
        "Install the SDLC seed skills (embedded in the binary at build time) into a \
         project as `Skill` notes under a `SKILLS` index note. Idempotent on title: \
         re-runs report `skills_skipped` for any title already present. \
         When `materialize_to_disk` is true, ALSO writes each newly-installed skill \
         to `<repo>/.claude/skills/<slug>.md` so Claude Code's skill loader can discover \
         them — target is the project's bound `repo_path` unless `repo_path_override` \
         is supplied. Works on projects with no `repo_path` bound, vault-only — useful \
         when you want the skill prompts available for editing before binding a repo."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "project_id": { "type": "string", "description": "Project UUID. Required." },
                "materialize_to_disk": {
                    "type": "boolean",
                    "description": "Optional, default false. Also write to <repo>/.claude/skills/.",
                },
                "repo_path_override": {
                    "type": "string",
                    "description": "Optional. Absolute path to materialize into, overriding \
                                    the project's bound repo_path.",
                },
            },
            "required": ["project_id"],
        })
    }

    async fn call(&self, args: Value) -> Result<Value, ToolHandlerError> {
        let project_id = parse_uuid(&args, "project_id")?;
        let materialize = args
            .get("materialize_to_disk")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let repo_path_override = parse_abs_path(&args, "repo_path_override")?;

        let project_repo = project_repo(&self.repos)?;
        let note_repo = self.repos.note_repo.clone();
        let persistence = self.repos.persistence.clone();

        // Resolve the materialize target up-front so we error before
        // writing vault notes if the user asked for disk + we can't
        // find a path. `repo_path_override` wins over the bound value.
        let materialize_target: Option<std::path::PathBuf> = if materialize {
            if let Some(override_path) = repo_path_override.clone() {
                Some(override_path)
            } else {
                let pr = project_repo.clone();
                let path = tokio::task::spawn_blocking(move || {
                    pr.get(project_id)
                        .map_err(|e| format!("get project: {e}"))?
                        .ok_or_else(|| format!("project {project_id} not found"))
                        .map(|p| p.repo_path)
                })
                .await
                .map_err(|e| ToolHandlerError::new(format!("resolve project task join: {e}")))?
                .map_err(ToolHandlerError::new)?;
                Some(path.ok_or_else(|| {
                    ToolHandlerError::new(
                        "materialize_to_disk=true but project has no repo_path bound \
                         and no repo_path_override supplied",
                    )
                })?)
            }
        } else {
            None
        };

        // Vault install.
        let report = install_seed_skills_vault(note_repo.clone(), persistence.clone(), project_id)
            .await?;

        // Disk materialize when asked. We walk every Skill note (not
        // just the newly-installed ones) so a re-run with
        // `materialize_to_disk=true` after an earlier vault-only run
        // still gets the files written.
        let materialized_files = if let Some(rp) = materialize_target {
            Some(materialize_all_skills(note_repo.clone(), persistence.clone(), project_id, rp.clone()).await?)
        } else {
            None
        };

        self.repos
            .ui
            .send(crate::local_mode::bridge_runtime::BridgeUiCommand::BumpNoteVersion);

        let mut payload = json!({
            "project_id": project_id.to_string(),
            "skills_installed": report.installed,
            "skills_skipped": report.skipped,
            "skills_failed": report.failed,
        });
        if let Some(files) = materialized_files {
            payload["skills_materialized"] = json!(files.len());
            payload["materialized_files"] = json!(files);
        } else {
            payload["skills_materialized"] = Value::Null;
        }
        Ok(text_content(payload.to_string()))
    }
}

// ------------------------------------------------------------
// operon_materialize_skills_to_disk
// ------------------------------------------------------------

pub struct OperonMaterializeSkillsToDiskTool {
    repos: BridgeRepos,
}

impl OperonMaterializeSkillsToDiskTool {
    pub fn new(repos: BridgeRepos) -> Self {
        Self { repos }
    }
}

#[async_trait]
impl ToolHandler for OperonMaterializeSkillsToDiskTool {
    fn name(&self) -> &str {
        "materialize_skills_to_disk"
    }

    fn description(&self) -> &str {
        "Write every `Skill` note in a project to `<repo>/.claude/skills/<title>.md` so \
         Claude Code's native skill loader can discover them on its next turn. Target is \
         the project's bound `repo_path` unless `repo_path_override` is supplied. Useful \
         after hand-authoring skill notes, importing from a folder, or re-binding the \
         project to a different repo. Bodies are run through `to_claude_compat` which \
         injects required `name:`/`description:` keys and strips the Operon-only \
         `## Revision history` table. Skills with empty bodies are silently skipped."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "project_id": { "type": "string", "description": "Project UUID. Required." },
                "repo_path_override": {
                    "type": "string",
                    "description": "Optional. Absolute path to materialize into, overriding \
                                    the project's bound repo_path.",
                },
            },
            "required": ["project_id"],
        })
    }

    async fn call(&self, args: Value) -> Result<Value, ToolHandlerError> {
        let project_id = parse_uuid(&args, "project_id")?;
        let repo_path_override = parse_abs_path(&args, "repo_path_override")?;
        let project_repo = project_repo(&self.repos)?;
        let note_repo = self.repos.note_repo.clone();
        let persistence = self.repos.persistence.clone();

        let target: std::path::PathBuf = if let Some(p) = repo_path_override {
            p
        } else {
            let pr = project_repo.clone();
            let bound = tokio::task::spawn_blocking(move || {
                pr.get(project_id)
                    .map_err(|e| format!("get project: {e}"))?
                    .ok_or_else(|| format!("project {project_id} not found"))
                    .map(|p| p.repo_path)
            })
            .await
            .map_err(|e| ToolHandlerError::new(format!("resolve project task join: {e}")))?
            .map_err(ToolHandlerError::new)?;
            bound.ok_or_else(|| {
                ToolHandlerError::new(
                    "project has no repo_path bound and no repo_path_override supplied",
                )
            })?
        };

        let files = materialize_all_skills(note_repo, persistence, project_id, target.clone()).await?;
        let payload = json!({
            "project_id": project_id.to_string(),
            "repo_path": target.to_string_lossy().into_owned(),
            "skills_written": files.len(),
            "files": files,
        });
        Ok(text_content(payload.to_string()))
    }
}

/// Pre-render a unified diff for the proposal card. Wraps
/// `similar::TextDiff::from_lines` directly because `diff_preview.rs`
/// only exposes a `Value`-keyed entry point that's tuned for
/// `Edit`/`Write` tool-input shapes — different domain, but the
/// rendering math is the same.
fn render_unified_diff(label: &str, before: &str, after: &str) -> String {
    use similar::{ChangeTag, TextDiff};
    const MAX_LINES: usize = 600;

    if before == after {
        return format!("--- {label}\n(no textual change)\n");
    }
    let diff = TextDiff::from_lines(before, after);
    let mut out = String::new();
    out.push_str(&format!("--- {label}\n+++ {label}\n"));
    let mut emitted = 0usize;
    for change in diff.iter_all_changes() {
        if emitted >= MAX_LINES {
            out.push_str("(… more lines hidden …)\n");
            break;
        }
        let sign = match change.tag() {
            ChangeTag::Delete => '-',
            ChangeTag::Insert => '+',
            ChangeTag::Equal => ' ',
        };
        out.push(sign);
        let v = change.value();
        out.push_str(v);
        if !v.ends_with('\n') {
            out.push('\n');
        }
        emitted += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_uuid_rejects_missing_field() {
        let res = parse_uuid(&json!({}), "note_id");
        assert!(res.is_err());
    }

    #[test]
    fn parse_uuid_rejects_malformed() {
        let res = parse_uuid(&json!({ "note_id": "not-a-uuid" }), "note_id");
        assert!(res.is_err());
    }

    #[test]
    fn parse_uuid_accepts_valid() {
        let u = Uuid::new_v4();
        let res = parse_uuid(&json!({ "note_id": u.to_string() }), "note_id");
        assert_eq!(res.unwrap(), u);
    }

    #[test]
    fn schemas_compile_and_have_required_fields() {
        // Cheap structural sanity check — the schemas are static
        // JSON so this is really just "did we typo a field name".
        let repos = stub_repos();
        let get = OperonGetNoteTool::new(repos.clone());
        let list = OperonListNotesTool::new(repos.clone());
        let search = OperonSearchNotesTool::new(repos.clone());
        let create = OperonCreateNoteTool::new(repos.clone());
        let append = OperonAppendNoteTool::new(repos.clone());
        let replace = OperonReplaceNoteRangeTool::new(repos.clone());
        let crawl = OperonCrawlNoteGraphTool::new(repos.clone());
        let projects = OperonListProjectsTool::new(repos.clone());
        let delete = OperonDeleteNoteTool::new(repos.clone());
        let rename = OperonRenameNoteTool::new(repos.clone());
        let recent = OperonListRecentNotesTool::new(repos.clone());
        let vault_info = OperonGetVaultInfoTool::new(repos.clone());
        let reorder = OperonReorderNoteTool::new(repos.clone());
        let move_t = OperonMoveNoteTool::new(repos.clone());
        let open_t = OperonOpenNoteTool::new(repos.clone());
        let list_att = OperonListAttachmentsTool::new(repos.clone());
        let delete_att = OperonDeleteAttachmentTool::new(repos.clone());
        let create_img = OperonCreateImageNoteTool::new(repos.clone());
        let attach_img = OperonAttachImageToNoteTool::new(repos.clone());
        let create_proj = OperonCreateProjectTool::new(repos.clone());
        let update_proj = OperonUpdateProjectTool::new(repos.clone());
        let delete_proj = OperonDeleteProjectTool::new(repos.clone());
        let install_seeds = OperonInstallSeedSkillsTool::new(repos.clone());
        let materialize_disk = OperonMaterializeSkillsToDiskTool::new(repos);

        assert_eq!(get.input_schema()["required"], json!(["note_id"]));
        assert_eq!(list.input_schema()["required"], json!(["project_id"]));
        assert_eq!(search.input_schema()["required"], json!(["query"]));
        assert_eq!(
            create.input_schema()["required"],
            json!(["project_id", "title"])
        );
        assert_eq!(
            append.input_schema()["required"],
            json!(["note_id", "text"])
        );
        assert_eq!(
            replace.input_schema()["required"],
            json!(["note_id", "old_text", "new_text"])
        );
        assert_eq!(crawl.input_schema()["required"], json!(["root_id"]));
        // list_projects + get_vault_info take no args — neither has
        // a "required" key. The other tools have at least one.
        assert!(projects.input_schema().get("required").is_none());
        assert!(vault_info.input_schema().get("required").is_none());
        assert_eq!(delete.input_schema()["required"], json!(["note_id"]));
        assert_eq!(
            rename.input_schema()["required"],
            json!(["note_id", "new_title"])
        );
        assert!(recent.input_schema().get("required").is_none());
        assert_eq!(reorder.input_schema()["required"], json!(["note_id", "op"]));
        assert_eq!(move_t.input_schema()["required"], json!(["note_id"]));
        assert_eq!(open_t.input_schema()["required"], json!(["note_id"]));
        assert_eq!(list_att.input_schema()["required"], json!(["note_id"]));
        assert_eq!(
            delete_att.input_schema()["required"],
            json!(["attachment_id"])
        );
        assert_eq!(
            create_img.input_schema()["required"],
            json!(["project_id", "title", "image_base64", "mime_type"])
        );
        assert_eq!(
            attach_img.input_schema()["required"],
            json!(["note_id", "image_base64", "filename", "mime_type"])
        );
        assert_eq!(create_proj.input_schema()["required"], json!(["name"]));
        assert_eq!(update_proj.input_schema()["required"], json!(["id"]));
        assert_eq!(delete_proj.input_schema()["required"], json!(["id"]));
        assert_eq!(install_seeds.input_schema()["required"], json!(["project_id"]));
        assert_eq!(
            materialize_disk.input_schema()["required"],
            json!(["project_id"])
        );

        assert_eq!(get.name(), "get_note");
        assert_eq!(list.name(), "list_notes");
        assert_eq!(search.name(), "search_notes");
        assert_eq!(create.name(), "create_note");
        assert_eq!(append.name(), "append_note");
        assert_eq!(replace.name(), "replace_note_range");
        assert_eq!(crawl.name(), "crawl_note_graph");
        assert_eq!(projects.name(), "list_projects");
        assert_eq!(delete.name(), "delete_note");
        assert_eq!(rename.name(), "rename_note");
        assert_eq!(recent.name(), "list_recent_notes");
        assert_eq!(vault_info.name(), "get_vault_info");
        assert_eq!(reorder.name(), "reorder_note");
        assert_eq!(move_t.name(), "move_note");
        assert_eq!(open_t.name(), "open_note");
        assert_eq!(list_att.name(), "list_attachments");
        assert_eq!(delete_att.name(), "delete_attachment");
        assert_eq!(create_img.name(), "create_image_note");
        assert_eq!(attach_img.name(), "attach_image_to_note");
        assert_eq!(create_proj.name(), "create_project");
        assert_eq!(update_proj.name(), "update_project");
        assert_eq!(delete_proj.name(), "delete_project");
        assert_eq!(install_seeds.name(), "install_seed_skills");
        assert_eq!(materialize_disk.name(), "materialize_skills_to_disk");
    }

    #[tokio::test]
    async fn replace_rejects_empty_old_text() {
        let tool = OperonReplaceNoteRangeTool::new(stub_repos());
        let res = tool
            .call(json!({
                "note_id": Uuid::new_v4().to_string(),
                "old_text": "",
                "new_text": "x",
            }))
            .await;
        assert!(res.is_err());
        assert!(res.err().unwrap().to_string().contains("non-empty"));
    }

    #[tokio::test]
    async fn create_note_rejects_image_kind() {
        let tool = OperonCreateNoteTool::new(stub_repos());
        let res = tool
            .call(json!({
                "project_id": Uuid::new_v4().to_string(),
                "title": "no",
                "kind": "image",
            }))
            .await;
        assert!(res.is_err());
        let msg = res.err().unwrap().to_string();
        assert!(msg.contains("image notes"), "got: {msg}");
    }

    #[tokio::test]
    async fn create_note_rejects_artifact_without_artifact_kind() {
        let tool = OperonCreateNoteTool::new(stub_repos());
        let res = tool
            .call(json!({
                "project_id": Uuid::new_v4().to_string(),
                "title": "Headless",
                "kind": "artifact",
            }))
            .await;
        let msg = res.err().expect("artifact w/o kind should error").to_string();
        assert!(
            msg.contains("artifact_kind is required"),
            "got: {msg}"
        );
        // List a few of the valid variants so the caller can self-correct.
        assert!(msg.contains("epic"), "got: {msg}");
        assert!(msg.contains("master_requirement"), "got: {msg}");
    }

    #[tokio::test]
    async fn create_note_rejects_artifact_kind_on_non_artifact() {
        // Passing artifact_kind on a markdown note would be a no-op
        // surprise — the kind isn't artifact so the frontmatter would
        // be ignored. Loud error so the caller picks one.
        let tool = OperonCreateNoteTool::new(stub_repos());
        let res = tool
            .call(json!({
                "project_id": Uuid::new_v4().to_string(),
                "title": "Mismatch",
                "kind": "markdown",
                "artifact_kind": "epic",
            }))
            .await;
        let msg = res.err().expect("kind mismatch should error").to_string();
        assert!(
            msg.contains("artifact_kind only valid when kind=\"artifact\""),
            "got: {msg}"
        );
    }

    #[tokio::test]
    async fn create_note_artifact_with_empty_body_seeds_full_scaffold() {
        // Empty body → caller wants the same scaffold the GUI menu writes
        // (frontmatter + section headers + revision history).
        let repos = stub_repos();
        let project_repo = repos.project_repo.clone().expect("project repo");
        let project = project_repo.create("P").expect("create project");
        let tool = OperonCreateNoteTool::new(repos.clone());

        let res = tool
            .call(json!({
                "project_id": project.id.to_string(),
                "title": "Auth Epic",
                "kind": "artifact",
                "artifact_kind": "epic",
            }))
            .await
            .expect("create succeeds");
        let payload: Value = serde_json::from_str(
            res.as_array().unwrap()[0]["text"].as_str().unwrap(),
        )
        .unwrap();
        let id = payload["id"].as_str().unwrap();

        let body = String::from_utf8(
            futures::executor::block_on(repos.persistence.load(id)).expect("load body"),
        )
        .unwrap();
        assert!(
            body.starts_with("---\nartifact_kind: epic\n---"),
            "body should start with epic frontmatter; got: {:.120}…",
            body
        );
        assert!(
            body.contains("## Revision history"),
            "scaffold should include revision history; got: {:.200}…",
            body
        );
    }

    #[tokio::test]
    async fn create_note_artifact_with_plain_body_prepends_minimal_frontmatter() {
        // Caller wrote their own body but didn't bother with frontmatter
        // — we prepend `---\nartifact_kind: X\n---\n\n` so the artifact
        // is cascade-visible, preserving their body verbatim afterwards.
        let repos = stub_repos();
        let project_repo = repos.project_repo.clone().expect("project repo");
        let project = project_repo.create("P").expect("create project");
        let tool = OperonCreateNoteTool::new(repos.clone());

        let res = tool
            .call(json!({
                "project_id": project.id.to_string(),
                "title": "Custom",
                "kind": "artifact",
                "artifact_kind": "feature",
                "body": "## Custom outline\n\nfreeform notes",
            }))
            .await
            .expect("create succeeds");
        let payload: Value = serde_json::from_str(
            res.as_array().unwrap()[0]["text"].as_str().unwrap(),
        )
        .unwrap();
        let id = payload["id"].as_str().unwrap();

        let body = String::from_utf8(
            futures::executor::block_on(repos.persistence.load(id)).unwrap(),
        )
        .unwrap();
        assert_eq!(
            body,
            "---\nartifact_kind: feature\n---\n\n## Custom outline\n\nfreeform notes"
        );
    }

    // ===== Project + repo + skill-install tools =====

    fn payload_from(res: &Value) -> Value {
        // Tool responses come back as the MCP content envelope:
        // `[{"type": "text", "text": "<json string>"}]`. Tests unwrap
        // the inner JSON here so assertions read cleanly.
        let text = res.as_array().unwrap()[0]["text"].as_str().unwrap();
        serde_json::from_str(text).unwrap()
    }

    #[tokio::test]
    async fn create_project_persists_name_and_returns_row() {
        let repos = stub_repos();
        let tool = OperonCreateProjectTool::new(repos.clone());
        let res = tool
            .call(json!({ "name": "V1", "install_seed_skills": false }))
            .await
            .expect("create succeeds");
        let payload = payload_from(&res);
        assert_eq!(payload["name"], "V1");
        assert!(payload["id"].as_str().is_some());
        assert!(payload["repo_path"].is_null());
        // Default-install was disabled, so install counts come back null.
        assert!(payload["seed_skills_installed"].is_null());

        let id_str = payload["id"].as_str().unwrap();
        let id = Uuid::parse_str(id_str).unwrap();
        let project_repo = repos.project_repo.clone().unwrap();
        let row = project_repo.get(id).unwrap().expect("row persisted");
        assert_eq!(row.name, "V1");
    }

    #[tokio::test]
    async fn create_project_rejects_empty_name() {
        let tool = OperonCreateProjectTool::new(stub_repos());
        let res = tool.call(json!({ "name": "   " })).await;
        let msg = res.err().expect("empty name should error").to_string();
        assert!(msg.contains("non-empty"), "got: {msg}");
    }

    #[tokio::test]
    async fn create_project_rejects_relative_repo_path() {
        let tool = OperonCreateProjectTool::new(stub_repos());
        let res = tool
            .call(json!({
                "name": "Bad",
                "repo_path": "./relative",
                "install_seed_skills": false,
            }))
            .await;
        let msg = res.err().expect("relative path should error").to_string();
        assert!(msg.contains("absolute"), "got: {msg}");
    }

    #[tokio::test]
    async fn create_project_with_absolute_repo_path_persists_binding() {
        let repos = stub_repos();
        let tool = OperonCreateProjectTool::new(repos.clone());
        let res = tool
            .call(json!({
                "name": "Bound",
                "repo_path": "/abs/some/repo",
                "install_seed_skills": false,
            }))
            .await
            .expect("create succeeds");
        let payload = payload_from(&res);
        assert_eq!(payload["repo_path"], "/abs/some/repo");
    }

    #[tokio::test]
    async fn create_project_materialize_requires_repo_path() {
        // materialize_skills_to_disk=true with no repo_path must error
        // BEFORE creating the project row.
        let repos = stub_repos();
        let tool = OperonCreateProjectTool::new(repos.clone());
        let res = tool
            .call(json!({
                "name": "NoRepo",
                "materialize_skills_to_disk": true,
            }))
            .await;
        let msg = res.err().expect("should error").to_string();
        assert!(msg.contains("repo_path"), "got: {msg}");
        // And nothing was created.
        let project_repo = repos.project_repo.clone().unwrap();
        assert!(project_repo.list().unwrap().is_empty());
    }

    #[tokio::test]
    async fn create_project_default_installs_seed_skills() {
        // Default behaviour (no explicit install_seed_skills flag) is
        // to populate the SDLC chain. Locked in the plan.
        let repos = stub_repos();
        let tool = OperonCreateProjectTool::new(repos.clone());
        let res = tool
            .call(json!({ "name": "WithSeeds" }))
            .await
            .expect("create succeeds");
        let payload = payload_from(&res);
        let installed = payload["seed_skills_installed"].as_u64().unwrap();
        assert!(
            installed >= 10,
            "expected the cascade chain, got {installed} installed"
        );

        // SKILLS index + Skill children should now live in the project.
        let id = Uuid::parse_str(payload["id"].as_str().unwrap()).unwrap();
        let notes = repos.note_repo.list_for_project(id).unwrap();
        assert!(notes.iter().any(|n| n.title == "SKILLS"));
        let skill_count = notes
            .iter()
            .filter(|n| matches!(n.kind, operon_store::repos::NoteKind::Skill))
            .count();
        assert_eq!(skill_count as u64, installed);
    }

    #[tokio::test]
    async fn create_project_materialize_writes_skill_files_to_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let repos = stub_repos();
        let tool = OperonCreateProjectTool::new(repos.clone());
        let res = tool
            .call(json!({
                "name": "Disk",
                "repo_path": tmp.path().to_string_lossy(),
                "materialize_skills_to_disk": true,
            }))
            .await
            .expect("create succeeds");
        let payload = payload_from(&res);
        let written = payload["skills_materialized"].as_u64().unwrap();
        assert!(written >= 10, "got {written} files written");
        // Directory exists with at least one .md file.
        let dir = tmp.path().join(".claude").join("skills");
        assert!(dir.is_dir(), "skills dir should exist");
        let on_disk = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(on_disk as u64, written);
    }

    #[tokio::test]
    async fn update_project_renames_and_clears_repo_path() {
        let repos = stub_repos();
        // Seed a bound project.
        let project_repo = repos.project_repo.clone().unwrap();
        let p = project_repo.create("Orig").unwrap();
        project_repo
            .set_repo_path(p.id, Some(std::path::Path::new("/abs/old")))
            .unwrap();

        let tool = OperonUpdateProjectTool::new(repos.clone());
        let res = tool
            .call(json!({
                "id": p.id.to_string(),
                "name": "Renamed",
                "repo_path": null,
            }))
            .await
            .expect("update succeeds");
        let payload = payload_from(&res);
        assert_eq!(payload["name"], "Renamed");
        assert!(payload["repo_path"].is_null());
    }

    #[tokio::test]
    async fn update_project_three_state_leaves_unset_fields_alone() {
        let repos = stub_repos();
        let project_repo = repos.project_repo.clone().unwrap();
        let p = project_repo.create("Keep").unwrap();
        project_repo
            .set_default_model(p.id, Some("inherit-me"))
            .unwrap();

        let tool = OperonUpdateProjectTool::new(repos.clone());
        // Set repo_path only — default_model is omitted, must survive.
        let res = tool
            .call(json!({
                "id": p.id.to_string(),
                "repo_path": "/abs/new",
            }))
            .await
            .expect("update succeeds");
        let payload = payload_from(&res);
        assert_eq!(payload["repo_path"], "/abs/new");
        assert_eq!(payload["default_model"], "inherit-me");
    }

    #[tokio::test]
    async fn update_project_rejects_relative_repo_path() {
        let repos = stub_repos();
        let project_repo = repos.project_repo.clone().unwrap();
        let p = project_repo.create("X").unwrap();
        let tool = OperonUpdateProjectTool::new(repos);
        let res = tool
            .call(json!({
                "id": p.id.to_string(),
                "repo_path": "rel/path",
            }))
            .await;
        let msg = res.err().expect("relative repo_path should error").to_string();
        assert!(msg.contains("absolute"), "got: {msg}");
    }

    #[tokio::test]
    async fn delete_project_requires_explicit_confirm() {
        let repos = stub_repos();
        let project_repo = repos.project_repo.clone().unwrap();
        let p = project_repo.create("Doomed").unwrap();

        let tool = OperonDeleteProjectTool::new(repos.clone());
        let res = tool.call(json!({ "id": p.id.to_string() })).await;
        let msg = res.err().expect("should require confirm").to_string();
        assert!(msg.contains("confirm=true"), "got: {msg}");
        // Project still exists.
        assert!(project_repo.get(p.id).unwrap().is_some());
    }

    #[tokio::test]
    async fn delete_project_with_confirm_removes_row_and_returns_counts() {
        let repos = stub_repos();
        let project_repo = repos.project_repo.clone().unwrap();
        let p = project_repo.create("Bye").unwrap();
        // Seed a couple of notes so the count is non-zero.
        repos.note_repo.create(p.id, None, "n1").unwrap();
        repos.note_repo.create(p.id, None, "n2").unwrap();

        let tool = OperonDeleteProjectTool::new(repos.clone());
        let res = tool
            .call(json!({ "id": p.id.to_string(), "confirm": true }))
            .await
            .expect("delete succeeds");
        let payload = payload_from(&res);
        assert_eq!(payload["deleted"], true);
        assert_eq!(payload["notes_removed"], 2);
        // Project is gone.
        assert!(project_repo.get(p.id).unwrap().is_none());
    }

    #[tokio::test]
    async fn install_seed_skills_vault_only_for_unbound_project() {
        let repos = stub_repos();
        let project_repo = repos.project_repo.clone().unwrap();
        let p = project_repo.create("Vault").unwrap();
        let tool = OperonInstallSeedSkillsTool::new(repos.clone());
        let res = tool
            .call(json!({ "project_id": p.id.to_string() }))
            .await
            .expect("install succeeds");
        let payload = payload_from(&res);
        let installed = payload["skills_installed"].as_u64().unwrap();
        assert!(installed >= 10);
        assert!(payload["skills_materialized"].is_null());
    }

    #[tokio::test]
    async fn install_seed_skills_is_idempotent_on_re_run() {
        let repos = stub_repos();
        let project_repo = repos.project_repo.clone().unwrap();
        let p = project_repo.create("Idem").unwrap();
        let tool = OperonInstallSeedSkillsTool::new(repos.clone());
        let first = payload_from(
            &tool
                .call(json!({ "project_id": p.id.to_string() }))
                .await
                .unwrap(),
        );
        let second = payload_from(
            &tool
                .call(json!({ "project_id": p.id.to_string() }))
                .await
                .unwrap(),
        );
        assert!(first["skills_installed"].as_u64().unwrap() >= 10);
        // Second pass installs zero new, skips all the prior ones.
        assert_eq!(second["skills_installed"].as_u64().unwrap(), 0);
        assert_eq!(
            second["skills_skipped"].as_u64().unwrap(),
            first["skills_installed"].as_u64().unwrap()
        );
    }

    #[tokio::test]
    async fn install_seed_skills_materialize_requires_repo_or_override() {
        let repos = stub_repos();
        let project_repo = repos.project_repo.clone().unwrap();
        let p = project_repo.create("NoBound").unwrap();
        let tool = OperonInstallSeedSkillsTool::new(repos);
        let res = tool
            .call(json!({
                "project_id": p.id.to_string(),
                "materialize_to_disk": true,
            }))
            .await;
        let msg = res
            .err()
            .expect("no repo + no override should error")
            .to_string();
        assert!(msg.contains("repo_path"), "got: {msg}");
    }

    #[tokio::test]
    async fn install_seed_skills_materialize_with_override_writes_to_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let repos = stub_repos();
        let project_repo = repos.project_repo.clone().unwrap();
        let p = project_repo.create("Override").unwrap();
        let tool = OperonInstallSeedSkillsTool::new(repos);
        let res = tool
            .call(json!({
                "project_id": p.id.to_string(),
                "materialize_to_disk": true,
                "repo_path_override": tmp.path().to_string_lossy(),
            }))
            .await
            .expect("install succeeds");
        let payload = payload_from(&res);
        assert!(payload["skills_materialized"].as_u64().unwrap() >= 10);
        assert!(tmp.path().join(".claude").join("skills").is_dir());
    }

    #[tokio::test]
    async fn materialize_skills_to_disk_errors_when_no_repo_resolvable() {
        let repos = stub_repos();
        let project_repo = repos.project_repo.clone().unwrap();
        let p = project_repo.create("Floating").unwrap();
        let tool = OperonMaterializeSkillsToDiskTool::new(repos);
        let res = tool
            .call(json!({ "project_id": p.id.to_string() }))
            .await;
        let msg = res.err().expect("no repo should error").to_string();
        assert!(msg.contains("repo_path"), "got: {msg}");
    }

    #[tokio::test]
    async fn materialize_skills_to_disk_writes_using_override() {
        let tmp = tempfile::tempdir().unwrap();
        let repos = stub_repos();
        let project_repo = repos.project_repo.clone().unwrap();
        let p = project_repo.create("Mat").unwrap();
        // Vault-install first so there are skills to materialize.
        OperonInstallSeedSkillsTool::new(repos.clone())
            .call(json!({ "project_id": p.id.to_string() }))
            .await
            .unwrap();
        let tool = OperonMaterializeSkillsToDiskTool::new(repos);
        let res = tool
            .call(json!({
                "project_id": p.id.to_string(),
                "repo_path_override": tmp.path().to_string_lossy(),
            }))
            .await
            .expect("materialize succeeds");
        let payload = payload_from(&res);
        assert!(payload["skills_written"].as_u64().unwrap() >= 10);
        let on_disk = std::fs::read_dir(tmp.path().join(".claude").join("skills"))
            .unwrap()
            .count();
        assert_eq!(on_disk as u64, payload["skills_written"].as_u64().unwrap());
    }

    #[tokio::test]
    async fn create_note_artifact_with_pre_authored_frontmatter_is_untouched() {
        // Power-user path: caller already wrote frontmatter. We do not
        // double-wrap — their body wins, even if their declared
        // artifact_kind contradicts the parameter (the parameter is
        // still required to be present, but doesn't override the body).
        let repos = stub_repos();
        let project_repo = repos.project_repo.clone().expect("project repo");
        let project = project_repo.create("P").expect("create project");
        let tool = OperonCreateNoteTool::new(repos.clone());

        let custom_body = "---\nartifact_kind: story\nstatus: approved\n---\n\n# Hand-rolled";
        let res = tool
            .call(json!({
                "project_id": project.id.to_string(),
                "title": "Power",
                "kind": "artifact",
                "artifact_kind": "feature",
                "body": custom_body,
            }))
            .await
            .expect("create succeeds");
        let payload: Value = serde_json::from_str(
            res.as_array().unwrap()[0]["text"].as_str().unwrap(),
        )
        .unwrap();
        let id = payload["id"].as_str().unwrap();

        let body = String::from_utf8(
            futures::executor::block_on(repos.persistence.load(id)).unwrap(),
        )
        .unwrap();
        assert_eq!(body, custom_body);
    }

    #[tokio::test]
    async fn append_note_rejects_empty_text() {
        let tool = OperonAppendNoteTool::new(stub_repos());
        let res = tool
            .call(json!({
                "note_id": Uuid::new_v4().to_string(),
                "text": "",
            }))
            .await;
        assert!(res.is_err());
        let msg = res.err().unwrap().to_string();
        assert!(msg.contains("non-empty"), "got: {msg}");
    }

    /// Fixture for the BFS tests. Spins up an in-memory Store and
    /// returns the three repos the BFS reads plus the project id and
    /// a `make_note` closure callers use to seed the graph. Same
    /// pattern as `local_note_link::tests::fixture` but yields
    /// `Arc<dyn …>` so it lines up with `crawl_note_graph_bfs`'s
    /// signature.
    fn bfs_fixture() -> (
        Arc<dyn operon_store::repos::LocalNoteLinkRepository>,
        Arc<dyn operon_store::repos::LocalNoteRepository>,
        Arc<dyn operon_store::repos::LocalProjectRepository>,
        Uuid,
    ) {
        use operon_store::repos::{
            LocalProjectRepository, SqliteLocalNoteLinkRepository, SqliteLocalNoteRepository,
            SqliteLocalProjectRepository,
        };
        use operon_store::Store;

        let store = Store::for_test().expect("in-memory store");
        let link_repo: Arc<dyn operon_store::repos::LocalNoteLinkRepository> =
            Arc::new(SqliteLocalNoteLinkRepository::new(store.clone()));
        let note_repo: Arc<dyn operon_store::repos::LocalNoteRepository> =
            Arc::new(SqliteLocalNoteRepository::new(store.clone()));
        let project_repo: Arc<dyn operon_store::repos::LocalProjectRepository> =
            Arc::new(SqliteLocalProjectRepository::new(store));
        let project = project_repo.create("BFS").expect("create project");
        (link_repo, note_repo, project_repo, project.id)
    }

    /// Convenience: insert a note row into the fixture project.
    fn make_note(
        note_repo: &Arc<dyn operon_store::repos::LocalNoteRepository>,
        project_id: Uuid,
        title: &str,
    ) -> Uuid {
        use operon_store::repos::LocalNoteRepository;
        note_repo
            .create(project_id, None, title)
            .expect("create note")
            .id
    }

    /// Convenience: replace the outbound link set for one source.
    fn set_links(
        link_repo: &Arc<dyn operon_store::repos::LocalNoteLinkRepository>,
        source: Uuid,
        edges: &[(&str, Uuid, bool)],
    ) {
        use operon_store::repos::LinkRow;
        let rows: Vec<LinkRow> = edges
            .iter()
            .map(|(text, target, is_embed)| LinkRow {
                source_note_id: source,
                target_text: (*text).to_string(),
                target_note_id: Some(*target),
                is_embed: *is_embed,
            })
            .collect();
        link_repo
            .replace_for(source, &rows)
            .expect("replace_for");
    }

    #[test]
    fn crawl_handles_cycle_and_depth_limit() {
        // Graph: A → B, B → C, B → D, C → A (back-edge cycle).
        // A BFS from A should visit {A, B, C, D} exactly once each.
        // The back-edge C→A should still appear in `edges` so Claude
        // can see the cycle.
        let (link_repo, note_repo, _project_repo, pid) = bfs_fixture();
        let a = make_note(&note_repo, pid, "A");
        let b = make_note(&note_repo, pid, "B");
        let c = make_note(&note_repo, pid, "C");
        let d = make_note(&note_repo, pid, "D");
        set_links(&link_repo, a, &[("B", b, false)]);
        set_links(&link_repo, b, &[("C", c, false), ("D", d, false)]);
        set_links(&link_repo, c, &[("A", a, false)]);

        let args = CrawlArgs {
            root_id: a,
            max_depth: 5,
            max_nodes: 100,
            include_bodies: false,
            embeds_only: false,
            direction: CrawlDirection::Out,
        };
        let result = crawl_note_graph_bfs(&args, &link_repo, &note_repo, None).expect("crawl ok");

        let visited: std::collections::HashSet<String> = result
            .nodes
            .iter()
            .map(|n| n["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            visited.len(),
            4,
            "expected 4 nodes, got {}: {visited:?}",
            visited.len()
        );
        assert!(visited.contains(&a.to_string()));
        assert!(visited.contains(&b.to_string()));
        assert!(visited.contains(&c.to_string()));
        assert!(visited.contains(&d.to_string()));

        // Back-edge C→A must be reported (cycle visibility).
        let back_edge = result.edges.iter().any(|e| {
            e["from"].as_str() == Some(&c.to_string())
                && e["to"].as_str() == Some(&a.to_string())
        });
        assert!(back_edge, "back-edge C→A missing from edges: {:?}", result.edges);

        assert!(!result.truncated, "should not truncate on a 4-node graph");
        assert!(result.truncation_reason.is_none());
    }

    #[test]
    fn crawl_truncates_at_max_nodes() {
        // Chain A → B → C → D; max_nodes=2 must stop after 2 nodes
        // and still report the edge from the boundary so the model
        // sees the graph continues.
        let (link_repo, note_repo, _project_repo, pid) = bfs_fixture();
        let a = make_note(&note_repo, pid, "A");
        let b = make_note(&note_repo, pid, "B");
        let c = make_note(&note_repo, pid, "C");
        let d = make_note(&note_repo, pid, "D");
        set_links(&link_repo, a, &[("B", b, false)]);
        set_links(&link_repo, b, &[("C", c, false)]);
        set_links(&link_repo, c, &[("D", d, false)]);

        let args = CrawlArgs {
            root_id: a,
            max_depth: 10,
            max_nodes: 2,
            include_bodies: false,
            embeds_only: false,
            direction: CrawlDirection::Out,
        };
        let result = crawl_note_graph_bfs(&args, &link_repo, &note_repo, None).expect("crawl ok");

        assert_eq!(result.nodes.len(), 2, "should stop at 2 nodes");
        assert!(result.truncated);
        assert_eq!(result.truncation_reason, Some(CrawlTruncation::MaxNodes));

        // Edge from B → C should be in `edges` (we still scan B's
        // outbound links before bailing on the enqueue) so Claude
        // can see the chain continues past the cap.
        let saw_boundary = result.edges.iter().any(|e| {
            e["from"].as_str() == Some(&b.to_string())
                && e["to"].as_str() == Some(&c.to_string())
        });
        assert!(
            saw_boundary,
            "expected boundary edge B→C to be reported; got {:?}",
            result.edges
        );
    }

    #[test]
    fn crawl_embeds_only_skips_plain_links() {
        // A has two outbound: B (plain), C (embed). With
        // embeds_only=true, B must be invisible — neither in nodes
        // nor in edges (the filter is at the link-iteration level).
        let (link_repo, note_repo, _project_repo, pid) = bfs_fixture();
        let a = make_note(&note_repo, pid, "A");
        let b = make_note(&note_repo, pid, "B");
        let c = make_note(&note_repo, pid, "C");
        set_links(&link_repo, a, &[("B", b, false), ("C", c, true)]);

        let args = CrawlArgs {
            root_id: a,
            max_depth: 5,
            max_nodes: 100,
            include_bodies: false,
            embeds_only: true,
            direction: CrawlDirection::Out,
        };
        let result = crawl_note_graph_bfs(&args, &link_repo, &note_repo, None).expect("crawl ok");

        let visited: std::collections::HashSet<String> = result
            .nodes
            .iter()
            .map(|n| n["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(visited.len(), 2, "expected just root + embed target");
        assert!(visited.contains(&a.to_string()));
        assert!(visited.contains(&c.to_string()));
        assert!(!visited.contains(&b.to_string()));

        // No edge to B either — the embeds_only filter is applied
        // before edge recording so plain links don't pollute output.
        let saw_plain = result.edges.iter().any(|e| e["to"].as_str() == Some(&b.to_string()));
        assert!(!saw_plain, "plain-link edge leaked through embeds_only");
    }

    #[test]
    fn crawl_direction_in_follows_backlinks_only() {
        // A → B, A → C. Crawl from B with direction=in must return
        // {B, A} (A is the referrer of B) and must NOT walk forward
        // to C (A's outbound link to C belongs to an outbound
        // traversal, not visible in an inbound-only pass).
        let (link_repo, note_repo, _project_repo, pid) = bfs_fixture();
        let a = make_note(&note_repo, pid, "A");
        let b = make_note(&note_repo, pid, "B");
        let c = make_note(&note_repo, pid, "C");
        set_links(&link_repo, a, &[("B", b, false), ("C", c, false)]);

        let args = CrawlArgs {
            root_id: b,
            max_depth: 5,
            max_nodes: 100,
            include_bodies: false,
            embeds_only: false,
            direction: CrawlDirection::In,
        };
        let result = crawl_note_graph_bfs(&args, &link_repo, &note_repo, None).expect("crawl ok");

        let visited: std::collections::HashSet<String> = result
            .nodes
            .iter()
            .map(|n| n["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(visited.len(), 2, "expected B + A; got {visited:?}");
        assert!(visited.contains(&b.to_string()));
        assert!(visited.contains(&a.to_string()));
        assert!(
            !visited.contains(&c.to_string()),
            "outbound C must not be visited in `in` mode"
        );

        // Inbound edge A→B should be reported with direction="in".
        let inbound = result.edges.iter().find(|e| {
            e["from"].as_str() == Some(&a.to_string())
                && e["to"].as_str() == Some(&b.to_string())
        });
        assert!(
            inbound.is_some(),
            "missing inbound edge A→B: {:?}",
            result.edges
        );
        assert_eq!(inbound.unwrap()["direction"].as_str(), Some("in"));
    }

    #[test]
    fn crawl_direction_both_walks_in_and_out() {
        // A → B, B → C, X → A. From B with direction=both we should
        // visit B's outbound (C), B's inbound (A), and A's inbound
        // (X). Visited set = {A, B, C, X}; both edge directions
        // should appear in the output.
        let (link_repo, note_repo, _project_repo, pid) = bfs_fixture();
        let a = make_note(&note_repo, pid, "A");
        let b = make_note(&note_repo, pid, "B");
        let c = make_note(&note_repo, pid, "C");
        let x = make_note(&note_repo, pid, "X");
        set_links(&link_repo, a, &[("B", b, false)]);
        set_links(&link_repo, b, &[("C", c, false)]);
        set_links(&link_repo, x, &[("A", a, false)]);

        let args = CrawlArgs {
            root_id: b,
            max_depth: 5,
            max_nodes: 100,
            include_bodies: false,
            embeds_only: false,
            direction: CrawlDirection::Both,
        };
        let result = crawl_note_graph_bfs(&args, &link_repo, &note_repo, None).expect("crawl ok");

        let visited: std::collections::HashSet<String> = result
            .nodes
            .iter()
            .map(|n| n["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(visited.len(), 4, "expected A,B,C,X; got {visited:?}");
        for id in [a, b, c, x] {
            assert!(visited.contains(&id.to_string()), "missing {id}");
        }

        let has_out = result
            .edges
            .iter()
            .any(|e| e["direction"].as_str() == Some("out"));
        let has_in = result
            .edges
            .iter()
            .any(|e| e["direction"].as_str() == Some("in"));
        assert!(
            has_out,
            "no outbound edges in `both` traversal: {:?}",
            result.edges
        );
        assert!(
            has_in,
            "no inbound edges in `both` traversal: {:?}",
            result.edges
        );
    }

    // ============================================================
    // Behavioral tests for the new tools added this round
    // ============================================================

    #[tokio::test]
    async fn list_projects_returns_seeded_projects_sorted() {
        use operon_store::repos::LocalProjectRepository;
        let repos = stub_repos();
        let prepo = repos.project_repo.clone().unwrap();
        // Create out-of-order; tool should sort by sibling_index.
        let _b = prepo.create("BB").unwrap();
        let _a = prepo.create("AA").unwrap();
        let tool = OperonListProjectsTool::new(repos);
        let resp = tool.call(json!({})).await.expect("ok");
        let text = resp[0]["text"].as_str().unwrap();
        let v: Value = serde_json::from_str(text).unwrap();
        let names: Vec<&str> = v["projects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        // Create returns the row with an assigned sibling_index; the
        // first created gets the smaller index regardless of name.
        assert_eq!(names, vec!["BB", "AA"]);
    }

    #[tokio::test]
    async fn search_notes_in_content_finds_body_match() {
        use operon_store::repos::{LocalNoteRepository, LocalProjectRepository};
        let repos = stub_repos();
        let prepo = repos.project_repo.clone().unwrap();
        let project = prepo.create("P").unwrap();
        // Title doesn't match; body does.
        let note = repos.note_repo.create(project.id, None, "Some Note").unwrap();
        repos
            .persistence
            .save(&note.id.to_string(), b"hello deadlineX is in March")
            .await
            .unwrap();

        let tool = OperonSearchNotesTool::new(repos);
        // Title-only: zero hits.
        let resp = tool
            .call(json!({ "query": "deadlineX" }))
            .await
            .expect("ok");
        let v: Value = serde_json::from_str(resp[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(v["results"].as_array().unwrap().len(), 0, "title-only should miss body match");

        // in_content: should find the body match with snippet.
        let resp = tool
            .call(json!({ "query": "deadlineX", "in_content": true }))
            .await
            .expect("ok");
        let v: Value = serde_json::from_str(resp[0]["text"].as_str().unwrap()).unwrap();
        let hits = v["results"].as_array().unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["matched_in"].as_str(), Some("body"));
        assert!(hits[0]["snippet"].as_str().unwrap().contains("deadlineX"));
    }

    #[tokio::test]
    async fn get_note_returns_timestamps() {
        use operon_store::repos::{LocalNoteRepository, LocalProjectRepository};
        let repos = stub_repos();
        let project = repos.project_repo.clone().unwrap().create("P").unwrap();
        let note = repos.note_repo.create(project.id, None, "T").unwrap();
        repos.persistence.save(&note.id.to_string(), b"hi").await.unwrap();

        let tool = OperonGetNoteTool::new(repos);
        let resp = tool.call(json!({ "note_id": note.id.to_string() })).await.expect("ok");
        let v: Value = serde_json::from_str(resp[0]["text"].as_str().unwrap()).unwrap();
        assert!(v["created_at_ms"].as_i64().unwrap() > 0);
        assert!(v["updated_at_ms"].as_i64().unwrap() > 0);
    }

    #[tokio::test]
    async fn get_note_inlines_attachments() {
        use operon_store::repos::{LocalAttachment, LocalNoteRepository, LocalProjectRepository};
        let repos = stub_repos();
        let project = repos.project_repo.clone().unwrap().create("P").unwrap();
        let note = repos.note_repo.create(project.id, None, "Host").unwrap();
        repos.persistence.save(&note.id.to_string(), b"hi").await.unwrap();

        // Seed two attachments directly through the repo so we don't
        // need a vault / write_image. The point of this test is the
        // get_note response wiring, not the image pipeline.
        let mut a1 = LocalAttachment::new(note.id, "a.png", "sha-a", 1, "p/a.png");
        a1.mime_type = Some("image/png".into());
        let mut a2 = LocalAttachment::new(note.id, "b.png", "sha-b", 2, "p/b.png");
        a2.mime_type = Some("image/png".into());
        repos.attachment_repo.create(&a1).unwrap();
        repos.attachment_repo.create(&a2).unwrap();

        let tool = OperonGetNoteTool::new(repos);
        let resp = tool.call(json!({ "note_id": note.id.to_string() })).await.expect("ok");
        let v: Value = serde_json::from_str(resp[0]["text"].as_str().unwrap()).unwrap();
        let attachments = v["attachments"].as_array().expect("attachments array");
        assert_eq!(attachments.len(), 2);
        let names: std::collections::HashSet<&str> = attachments
            .iter()
            .map(|a| a["filename"].as_str().unwrap())
            .collect();
        assert!(names.contains("a.png"));
        assert!(names.contains("b.png"));
    }

    #[tokio::test]
    async fn rename_note_rewrites_link_target_text() {
        use operon_store::repos::{LinkRow, LocalNoteRepository, LocalProjectRepository};
        let repos = stub_repos();
        let project = repos.project_repo.clone().unwrap().create("P").unwrap();
        let a = repos.note_repo.create(project.id, None, "A").unwrap();
        let b = repos.note_repo.create(project.id, None, "B").unwrap();
        // Seed A→B with target_text exactly equal to B's title ("B")
        // — that's the case rename should rewrite.
        repos
            .link_repo
            .replace_for(
                a.id,
                &[LinkRow {
                    source_note_id: a.id,
                    target_text: "B".into(),
                    target_note_id: Some(b.id),
                    is_embed: false,
                }],
            )
            .unwrap();

        let tool = OperonRenameNoteTool::new(repos.clone());
        let resp = tool
            .call(json!({ "note_id": b.id.to_string(), "new_title": "Bee" }))
            .await
            .expect("ok");
        let v: Value = serde_json::from_str(resp[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(v["new_title"].as_str(), Some("Bee"));
        assert_eq!(v["links_rewritten"].as_u64(), Some(1));

        let rows = repos.link_repo.list_for_source(a.id).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].target_text, "Bee");
    }

    #[tokio::test]
    async fn list_recent_notes_sorts_by_updated_desc() {
        use operon_store::repos::{LocalNoteRepository, LocalProjectRepository};
        let repos = stub_repos();
        let project = repos.project_repo.clone().unwrap().create("P").unwrap();
        let a = repos.note_repo.create(project.id, None, "A").unwrap();
        // Sleep enough that timestamps differ at ms resolution.
        std::thread::sleep(std::time::Duration::from_millis(5));
        let b = repos.note_repo.create(project.id, None, "B").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        // Bump A's updated_at to be newest.
        repos.note_repo.touch_updated(a.id).unwrap();

        let tool = OperonListRecentNotesTool::new(repos);
        let resp = tool.call(json!({})).await.expect("ok");
        let v: Value = serde_json::from_str(resp[0]["text"].as_str().unwrap()).unwrap();
        let ids: Vec<&str> = v["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids[0], a.id.to_string(), "A should be most recent after touch");
        assert_eq!(ids[1], b.id.to_string());
    }

    #[tokio::test]
    async fn reorder_note_indent_changes_depth() {
        use operon_store::repos::{LocalNoteRepository, LocalProjectRepository};
        let repos = stub_repos();
        let project = repos.project_repo.clone().unwrap().create("P").unwrap();
        let _a = repos.note_repo.create(project.id, None, "A").unwrap();
        let b = repos.note_repo.create(project.id, None, "B").unwrap();

        let tool = OperonReorderNoteTool::new(repos.clone());
        let resp = tool
            .call(json!({ "note_id": b.id.to_string(), "op": "indent" }))
            .await
            .expect("ok");
        let v: Value = serde_json::from_str(resp[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(v["op"].as_str(), Some("indent"));
        // After indenting B becomes a child of A, so depth jumps to 1.
        assert_eq!(v["new_depth"].as_i64(), Some(1));
    }

    #[tokio::test]
    async fn move_note_cross_project_moves_row() {
        use operon_store::repos::{LocalNoteRepository, LocalProjectRepository};
        let repos = stub_repos();
        let prepo = repos.project_repo.clone().unwrap();
        let p1 = prepo.create("P1").unwrap();
        let p2 = prepo.create("P2").unwrap();
        let n = repos.note_repo.create(p1.id, None, "movable").unwrap();

        let tool = OperonMoveNoteTool::new(repos.clone());
        let resp = tool
            .call(json!({
                "note_id": n.id.to_string(),
                "new_project_id": p2.id.to_string(),
                "new_parent_id": Value::Null,
            }))
            .await
            .expect("ok");
        let v: Value = serde_json::from_str(resp[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(v["project_id"].as_str(), Some(p2.id.to_string().as_str()));
        assert_eq!(
            repos.note_repo.find_project_for_note(n.id).unwrap(),
            Some(p2.id)
        );
    }

    #[tokio::test]
    async fn attach_image_then_list_then_delete_round_trips_through_local_attachments() {
        use operon_store::repos::{LocalNoteRepository, LocalProjectRepository};
        // 1x1 transparent PNG (smallest valid PNG payload).
        const PIXEL: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];

        let vault_tmp = tempfile::tempdir().expect("tempdir");
        let mut repos = stub_repos();
        repos.vault_root = Some(crate::local_mode::vault::VaultRoot {
            path: vault_tmp.path().to_path_buf(),
        });
        let project = repos.project_repo.clone().unwrap().create("P").unwrap();
        let note = repos.note_repo.create(project.id, None, "Host").unwrap();

        use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
        let b64 = B64.encode(PIXEL);

        // attach
        let attach_tool = OperonAttachImageToNoteTool::new(repos.clone());
        let resp = attach_tool
            .call(json!({
                "note_id": note.id.to_string(),
                "image_base64": b64,
                "filename": "pixel.png",
                "mime_type": "image/png",
                "alt_text": "tiny pixel",
            }))
            .await
            .expect("attach ok");
        let v: Value = serde_json::from_str(resp[0]["text"].as_str().unwrap()).unwrap();
        let attachment_id = v["attachment_id"].as_str().unwrap().to_string();
        let blob_rel = v["blob_path"].as_str().unwrap();
        let blob_abs = vault_tmp.path().join(blob_rel);
        assert!(blob_abs.exists(), "blob file missing at {}", blob_abs.display());
        assert!(v["embed_markdown"].as_str().unwrap().starts_with("![tiny pixel]("));

        // list
        let list_tool = OperonListAttachmentsTool::new(repos.clone());
        let resp = list_tool
            .call(json!({ "note_id": note.id.to_string() }))
            .await
            .expect("list ok");
        let v: Value = serde_json::from_str(resp[0]["text"].as_str().unwrap()).unwrap();
        let attachments = v["attachments"].as_array().unwrap();
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0]["filename"].as_str(), Some("pixel.png"));
        assert_eq!(attachments[0]["mime_type"].as_str(), Some("image/png"));
        assert_eq!(attachments[0]["id"].as_str(), Some(attachment_id.as_str()));

        // delete
        let del_tool = OperonDeleteAttachmentTool::new(repos.clone());
        let resp = del_tool
            .call(json!({ "attachment_id": attachment_id }))
            .await
            .expect("delete ok");
        let v: Value = serde_json::from_str(resp[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(v["deleted"].as_bool(), Some(true));

        // list again — empty
        let resp = list_tool
            .call(json!({ "note_id": note.id.to_string() }))
            .await
            .expect("list ok");
        let v: Value = serde_json::from_str(resp[0]["text"].as_str().unwrap()).unwrap();
        assert!(v["attachments"].as_array().unwrap().is_empty());
    }

    /// Tiny valid PNG used by the GC tests (1×1 transparent pixel).
    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
        0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
        0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78,
        0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
        0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[tokio::test]
    async fn delete_attachment_gcs_blob_when_unreferenced() {
        // Single attachment → delete → blob file unlinked from disk.
        use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
        use operon_store::repos::{LocalNoteRepository, LocalProjectRepository};

        let vault_tmp = tempfile::tempdir().expect("tempdir");
        let mut repos = stub_repos();
        repos.vault_root = Some(crate::local_mode::vault::VaultRoot {
            path: vault_tmp.path().to_path_buf(),
        });
        let project = repos.project_repo.clone().unwrap().create("P").unwrap();
        let note = repos.note_repo.create(project.id, None, "Host").unwrap();

        let attach_tool = OperonAttachImageToNoteTool::new(repos.clone());
        let resp = attach_tool
            .call(json!({
                "note_id": note.id.to_string(),
                "image_base64": B64.encode(TINY_PNG),
                "filename": "p.png",
                "mime_type": "image/png",
            }))
            .await
            .expect("attach ok");
        let v: Value = serde_json::from_str(resp[0]["text"].as_str().unwrap()).unwrap();
        let attachment_id = v["attachment_id"].as_str().unwrap().to_string();
        let blob_abs = vault_tmp.path().join(v["blob_path"].as_str().unwrap());
        assert!(blob_abs.exists());

        let del_tool = OperonDeleteAttachmentTool::new(repos);
        let resp = del_tool
            .call(json!({ "attachment_id": attachment_id }))
            .await
            .expect("delete ok");
        let v: Value = serde_json::from_str(resp[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(v["blob_gc_count"].as_u64(), Some(1), "blob should be unlinked");
        assert!(!blob_abs.exists(), "blob file should be gone");
    }

    #[tokio::test]
    async fn delete_attachment_keeps_blob_when_still_referenced_by_image_note() {
        // Same blob backs both an image note AND an attachment;
        // deleting the attachment must NOT unlink the file because
        // the image note still references it (count_by_blob_path on
        // local_note returns 1).
        use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
        use operon_store::repos::LocalProjectRepository;

        let vault_tmp = tempfile::tempdir().expect("tempdir");
        let mut repos = stub_repos();
        repos.vault_root = Some(crate::local_mode::vault::VaultRoot {
            path: vault_tmp.path().to_path_buf(),
        });
        let project = repos.project_repo.clone().unwrap().create("P").unwrap();

        // First write the image as a standalone note — that
        // populates local_note.blob_path.
        let img_tool = OperonCreateImageNoteTool::new(repos.clone());
        let resp = img_tool
            .call(json!({
                "project_id": project.id.to_string(),
                "title": "Sketch",
                "image_base64": B64.encode(TINY_PNG),
                "mime_type": "image/png",
            }))
            .await
            .expect("image note ok");
        let v: Value = serde_json::from_str(resp[0]["text"].as_str().unwrap()).unwrap();
        let blob_abs = vault_tmp.path().join(v["blob_path"].as_str().unwrap());
        assert!(blob_abs.exists());

        // Now attach the same bytes to a different (host) note. The
        // image writer is content-addressed, so the blob_path is
        // identical to the image note's.
        use operon_store::repos::LocalNoteRepository;
        let host = repos.note_repo.create(project.id, None, "Host").unwrap();
        let attach_tool = OperonAttachImageToNoteTool::new(repos.clone());
        let resp = attach_tool
            .call(json!({
                "note_id": host.id.to_string(),
                "image_base64": B64.encode(TINY_PNG),
                "filename": "shared.png",
                "mime_type": "image/png",
            }))
            .await
            .expect("attach ok");
        let v: Value = serde_json::from_str(resp[0]["text"].as_str().unwrap()).unwrap();
        let attachment_id = v["attachment_id"].as_str().unwrap().to_string();

        // Delete the attachment — blob must remain because the
        // image note still owns blob_path.
        let del_tool = OperonDeleteAttachmentTool::new(repos);
        let resp = del_tool
            .call(json!({ "attachment_id": attachment_id }))
            .await
            .expect("delete ok");
        let v: Value = serde_json::from_str(resp[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(v["blob_gc_count"].as_u64(), Some(0), "blob still referenced by image note");
        assert!(blob_abs.exists(), "blob must survive — image note still references it");
    }

    #[tokio::test]
    async fn delete_attachment_keeps_blob_when_other_attachment_shares_sha() {
        // Two attachments on different notes share the same blob
        // (content-addressing dedupes on identical bytes). Deleting
        // one attachment must NOT unlink the blob — the other still
        // references it.
        use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
        use operon_store::repos::{LocalNoteRepository, LocalProjectRepository};

        let vault_tmp = tempfile::tempdir().expect("tempdir");
        let mut repos = stub_repos();
        repos.vault_root = Some(crate::local_mode::vault::VaultRoot {
            path: vault_tmp.path().to_path_buf(),
        });
        let project = repos.project_repo.clone().unwrap().create("P").unwrap();
        let n1 = repos.note_repo.create(project.id, None, "A").unwrap();
        let n2 = repos.note_repo.create(project.id, None, "B").unwrap();

        let attach_tool = OperonAttachImageToNoteTool::new(repos.clone());
        let r1 = attach_tool
            .call(json!({
                "note_id": n1.id.to_string(),
                "image_base64": B64.encode(TINY_PNG),
                "filename": "x.png",
                "mime_type": "image/png",
            }))
            .await
            .expect("attach 1");
        let v1: Value = serde_json::from_str(r1[0]["text"].as_str().unwrap()).unwrap();
        let blob_abs = vault_tmp.path().join(v1["blob_path"].as_str().unwrap());

        let r2 = attach_tool
            .call(json!({
                "note_id": n2.id.to_string(),
                "image_base64": B64.encode(TINY_PNG),
                "filename": "y.png",
                "mime_type": "image/png",
            }))
            .await
            .expect("attach 2");
        let v2: Value = serde_json::from_str(r2[0]["text"].as_str().unwrap()).unwrap();
        let attachment_id_2 = v2["attachment_id"].as_str().unwrap().to_string();
        assert!(blob_abs.exists());

        // Delete the second attachment. n1's attachment still
        // references the same blob_path, so GC should keep it.
        let del_tool = OperonDeleteAttachmentTool::new(repos);
        let resp = del_tool
            .call(json!({ "attachment_id": attachment_id_2 }))
            .await
            .expect("delete ok");
        let v: Value = serde_json::from_str(resp[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(v["blob_gc_count"].as_u64(), Some(0));
        assert!(blob_abs.exists());
    }

    #[tokio::test]
    async fn create_image_note_writes_row_and_blob() {
        use operon_store::repos::LocalProjectRepository;
        const PIXEL: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];

        let vault_tmp = tempfile::tempdir().expect("tempdir");
        let mut repos = stub_repos();
        repos.vault_root = Some(crate::local_mode::vault::VaultRoot {
            path: vault_tmp.path().to_path_buf(),
        });
        let project = repos.project_repo.clone().unwrap().create("P").unwrap();

        use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
        let b64 = B64.encode(PIXEL);

        let tool = OperonCreateImageNoteTool::new(repos.clone());
        let resp = tool
            .call(json!({
                "project_id": project.id.to_string(),
                "title": "Sketch",
                "image_base64": b64,
                "mime_type": "image/png",
            }))
            .await
            .expect("ok");
        let v: Value = serde_json::from_str(resp[0]["text"].as_str().unwrap()).unwrap();
        let blob_rel = v["blob_path"].as_str().unwrap();
        let blob_abs = vault_tmp.path().join(blob_rel);
        assert!(blob_abs.exists(), "image blob missing at {}", blob_abs.display());
        // embed_markdown should be wikilink form starting with ![[Sketch^
        let embed = v["embed_markdown"].as_str().unwrap();
        assert!(embed.starts_with("![[Sketch^"), "got {embed}");
    }

    #[tokio::test]
    async fn create_image_note_rejects_unsupported_mime() {
        let vault_tmp = tempfile::tempdir().expect("tempdir");
        let mut repos = stub_repos();
        repos.vault_root = Some(crate::local_mode::vault::VaultRoot {
            path: vault_tmp.path().to_path_buf(),
        });
        use operon_store::repos::LocalProjectRepository;
        let project = repos.project_repo.clone().unwrap().create("P").unwrap();

        use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
        let b64 = B64.encode(b"not-an-image");
        let tool = OperonCreateImageNoteTool::new(repos);
        let res = tool
            .call(json!({
                "project_id": project.id.to_string(),
                "title": "Bad",
                "image_base64": b64,
                "mime_type": "application/octet-stream",
            }))
            .await;
        assert!(res.is_err(), "expected unsupported mime to error");
    }

    fn stub_repos() -> BridgeRepos {
        use crate::local_mode::bridge_runtime::make_ui_channel;
        use operon_store::repos::{
            SqliteLocalAttachmentRepository, SqliteLocalNoteLinkRepository,
            SqliteLocalNoteRepository, SqliteLocalProjectRepository,
            SqliteLocalSearchRepository,
        };
        use operon_store::Store;
        use std::sync::Arc;

        let store = Store::for_test().expect("in-memory store");
        let note_repo: Arc<dyn operon_store::repos::LocalNoteRepository> =
            Arc::new(SqliteLocalNoteRepository::new(store.clone()));
        let project_repo: Arc<dyn operon_store::repos::LocalProjectRepository> =
            Arc::new(SqliteLocalProjectRepository::new(store.clone()));
        let link_repo: Arc<dyn operon_store::repos::LocalNoteLinkRepository> =
            Arc::new(SqliteLocalNoteLinkRepository::new(store.clone()));
        let attachment_repo: Arc<dyn operon_store::repos::LocalAttachmentRepository> =
            Arc::new(SqliteLocalAttachmentRepository::new(store.clone()));
        let search_repo: Arc<dyn operon_store::repos::LocalSearchRepository> =
            Arc::new(SqliteLocalSearchRepository::new(store));
        // Drop the receiver — the validation-path tests never get
        // as far as `self.repos.ui.send(...)`, but real runs in
        // `provide_bridge_runtime` keep the receiver alive in the
        // drain task.
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
}
