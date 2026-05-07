//! Side-bar panel for [`super::LocalSearch`].
//!
//! VS Code-style global content search: a single input over `Local*Repository`
//! that returns hits grouped by note, with per-matched-line snippets. The
//! body cache is loaded once per session on first mount (lazy-on-mount); the
//! repo's `search()` is invoked with `in_content=true` and the cache acts as
//! the body loader.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use dioxus::prelude::*;
use operon_store::repos::{NoteKind, SearchKind, DEFAULT_SEARCH_LIMIT};
use uuid::Uuid;

use crate::editor::RequestEditorRevealLine;
use crate::local_mode::desktop::{LocalNoteRepo, LocalProjectRepo};
use crate::local_mode::editor::open_local_note_tab;
use crate::local_mode::explorer::{
    debounce_window, load_body_cache, BodyCache, ExplorerSearchRepo, LocalNoteVersion,
    LocalProjectVersion, SelectedNote, SelectedProject, WorkspaceOpenMap, WorkspaceTreeQueueCtx,
};
use crate::persistence::Persistence;
use crate::tabs::{SaveScheduler, TabManager};

const SCOPE_WORKSPACE: &str = "workspace";
const MAX_LINE_HITS_PER_NOTE: usize = 50;

/// Bumped by `Ctrl+Shift+F` to re-focus the panel input even when the panel
/// is already mounted.
#[derive(Clone, Copy)]
pub struct LocalSearchFocus(pub Signal<u64>);

#[derive(Clone)]
struct LineMatch {
    line_number: usize,
    line_text: String,
    /// Char-index half-open ranges within `line_text` that match the needle.
    match_ranges: Vec<(usize, usize)>,
}

#[derive(Clone)]
struct NoteFileHit {
    note_id: Uuid,
    project_id: Option<Uuid>,
    breadcrumb: String,
    line_matches: Vec<LineMatch>,
}

#[derive(Clone)]
struct ProjectHit {
    project_id: Uuid,
    name: String,
}

/// Walk a body line-by-line and collect every matched line up to
/// `MAX_LINE_HITS_PER_NOTE`. Lowercase comparison; ASCII-fast path is fine
/// for the substring check, and char ranges are recomputed against the
/// original line so highlight offsets render correctly.
fn collect_line_matches(body: &str, needle_lower: &str) -> Vec<LineMatch> {
    let mut out = Vec::new();
    if needle_lower.is_empty() {
        return out;
    }
    for (idx, line) in body.lines().enumerate() {
        let line_lower = line.to_lowercase();
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        let mut cursor = 0usize;
        while cursor < line_lower.len() {
            let Some(rel) = line_lower[cursor..].find(needle_lower) else {
                break;
            };
            let byte_start = cursor + rel;
            let byte_end = byte_start + needle_lower.len();
            let char_start = line.char_indices().take_while(|(b, _)| *b < byte_start).count();
            let char_end = line
                .char_indices()
                .take_while(|(b, _)| *b < byte_end)
                .count();
            ranges.push((char_start, char_end));
            cursor = byte_end;
        }
        if !ranges.is_empty() {
            out.push(LineMatch {
                line_number: idx + 1,
                line_text: line.to_string(),
                match_ranges: ranges,
            });
            if out.len() >= MAX_LINE_HITS_PER_NOTE {
                break;
            }
        }
    }
    out
}

#[component]
pub fn LocalSearchPanel() -> Element {
    let LocalProjectRepo(project_repo) = use_context();
    let LocalNoteRepo(note_repo) = use_context();
    let ExplorerSearchRepo(search_repo) = use_context();
    let persistence: Arc<dyn Persistence> = use_context();
    let LocalSearchFocus(focus_tick) = use_context();

    let tabs: Signal<TabManager> = use_context();
    let save_scheduler: SaveScheduler = use_context();
    let SelectedNote(selected_note) = use_context();
    let SelectedProject(selected_project) = use_context();
    let WorkspaceOpenMap(workspace_open) = use_context();
    let WorkspaceTreeQueueCtx(tree_queue) = use_context();
    let RequestEditorRevealLine(reveal_request) = use_context();
    let LocalProjectVersion(project_version) = use_context();
    let LocalNoteVersion(note_version) = use_context();

    // Local panel state.
    let query: Signal<String> = use_signal(String::new);
    let debounced: Signal<String> = use_signal(String::new);
    // None ⇒ not yet loaded; Some(_) ⇒ ready (possibly empty if the vault
    // genuinely holds no notes). Loaded once on mount.
    let body_cache: Signal<Option<BodyCache>> = use_signal(|| None);
    // Notes that are collapsed in the results tree. Default = expanded; flip
    // an entry on user click.
    let collapsed: Signal<HashSet<Uuid>> = use_signal(HashSet::new);
    // Element handle for programmatic re-focus on `focus_tick` bump.
    let input_handle: Signal<Option<Rc<MountedData>>> = use_signal(|| None);

    // ===== Lazy body-cache load on first mount =====
    {
        let p_repo = project_repo.clone();
        let n_repo = note_repo.clone();
        let persistence_for_cache = persistence.clone();
        let mut body_cache_setter = body_cache;
        use_hook(move || {
            // Collect every note id, then offload the file IO to a spawned task.
            let Ok(projects) = p_repo.list() else {
                body_cache_setter.set(Some(BodyCache::default()));
                return;
            };
            let mut all_ids: Vec<Uuid> = Vec::new();
            for p in projects {
                if let Ok(rows) = n_repo.list_for_project(p.id) {
                    all_ids.extend(rows.iter().map(|r| r.id));
                }
            }
            let persistence = persistence_for_cache.clone();
            spawn(async move {
                let map = load_body_cache(all_ids, persistence).await;
                body_cache_setter.set(Some(BodyCache(Arc::new(map))));
            });
        });
    }

    // ===== Debounce query → debounced =====
    let debounce_gen: Signal<u64> = use_signal(|| 0u64);
    {
        let mut debounced_setter = debounced;
        let mut gen_signal = debounce_gen;
        use_effect(move || {
            let q = query.read().clone();
            let my_gen = {
                let next = *gen_signal.peek() + 1;
                gen_signal.set(next);
                next
            };
            spawn(async move {
                debounce_window().await;
                if *gen_signal.peek() == my_gen {
                    debounced_setter.set(q);
                }
            });
        });
    }

    // ===== Re-focus input when `focus_tick` bumps (Ctrl+Shift+F) =====
    {
        let handle = input_handle;
        use_effect(move || {
            // Subscribe to the tick.
            let _ = focus_tick.read();
            if let Some(h) = handle.peek().clone() {
                spawn(async move {
                    let _ = h.set_focus(true).await;
                });
            }
        });
    }

    // ===== Memoized result computation =====
    // The search + per-line scan only re-runs when `debounced` or `body_cache`
    // change. Stored in a signal so render reads are O(1) regardless of
    // unrelated re-renders (collapse toggles, scroll, etc.).
    let results: Signal<(Vec<ProjectHit>, Vec<NoteFileHit>)> =
        use_signal(|| (Vec::new(), Vec::new()));
    {
        let mut results_setter = results;
        let search_repo_for_compute = search_repo.clone();
        use_effect(move || {
            let needle_trim = debounced.read().trim().to_string();
            let cache_now = body_cache.read().clone();
            if needle_trim.is_empty() || cache_now.is_none() {
                results_setter.set((Vec::new(), Vec::new()));
                return;
            }
            let cache = cache_now.as_ref().unwrap().0.clone();
            let cache_for_loader = cache.clone();
            let loader =
                move |id: Uuid| -> Option<String> { cache_for_loader.get(&id).cloned() };
            match search_repo_for_compute.search(
                &needle_trim,
                true,
                DEFAULT_SEARCH_LIMIT,
                &loader,
            ) {
                Ok(hits) => {
                    let needle_lower = needle_trim.to_lowercase();
                    let mut projects = Vec::new();
                    let mut notes = Vec::new();
                    for h in hits {
                        match h.kind {
                            SearchKind::Project => {
                                projects.push(ProjectHit {
                                    project_id: h.id,
                                    name: h.title,
                                });
                            }
                            SearchKind::Note => {
                                let line_matches = if h.snippet.is_some() {
                                    cache
                                        .get(&h.id)
                                        .map(|body| collect_line_matches(body, &needle_lower))
                                        .unwrap_or_default()
                                } else {
                                    Vec::new()
                                };
                                notes.push(NoteFileHit {
                                    note_id: h.id,
                                    project_id: h.project_id,
                                    breadcrumb: h.breadcrumb,
                                    line_matches,
                                });
                            }
                        }
                    }
                    results_setter.set((projects, notes));
                }
                Err(e) => {
                    eprintln!("operon: local-search failed: {e}");
                    results_setter.set((Vec::new(), Vec::new()));
                }
            }
        });
    }

    let needle_trim = debounced.read().trim().to_string();
    let cache_now = body_cache.read().clone();
    let (project_hits, note_hits) = results.read().clone();
    let total_files = project_hits.len() + note_hits.len();
    let total_lines: usize = note_hits.iter().map(|n| n.line_matches.len()).sum();

    // ===== Click handlers =====
    let on_project_pick = make_project_pick(selected_project, selected_note, workspace_open, tree_queue);

    // Title + kind lookup for the click handler. Rebuilt only when the
    // project / note version signals bump — not on every keystroke.
    let note_meta_signal: Signal<HashMap<Uuid, (String, NoteKind)>> =
        use_signal(HashMap::new);
    {
        let mut meta_setter = note_meta_signal;
        let nr = note_repo.clone();
        let pr = project_repo.clone();
        use_effect(move || {
            let _ = project_version.read();
            let _ = note_version.read();
            meta_setter.set(build_note_meta(&nr, &pr));
        });
    }
    let note_meta = note_meta_signal.read().clone();
    let on_note_pick = make_note_pick(
        tabs,
        save_scheduler.clone(),
        selected_note,
        selected_project,
        workspace_open,
        tree_queue,
        note_meta,
        persistence.clone(),
    );

    let mut collapsed_setter = collapsed;
    let collapsed_now = collapsed.read().clone();

    let cache_loading = cache_now.is_none();

    rsx! {
        div {
            class: "notes-explorer-panel",
            "data-testid": "local-search-panel",
            div {
                class: "notes-explorer-heading",
                style: "font-size: 11px; text-transform: uppercase; letter-spacing: 0.05em; padding: 0 0 6px 0; opacity: 0.7;",
                "Search"
            }
            div {
                class: "notes-explorer-search-form",
                "data-testid": "local-search-form",
                input {
                    r#type: "search",
                    class: "notes-explorer-search-input",
                    "data-testid": "local-search-input",
                    placeholder: "Search across all notes",
                    value: "{query.read().clone()}",
                    onmounted: move |evt| {
                        let mut handle_setter = input_handle;
                        handle_setter.set(Some(evt.data()));
                        drop(evt.set_focus(true));
                    },
                    oninput: {
                        let mut q_setter = query;
                        move |evt| q_setter.set(evt.value())
                    },
                    onkeydown: {
                        let mut q_setter = query;
                        move |evt| {
                            if evt.key().to_string() == "Escape" {
                                evt.prevent_default();
                                q_setter.set(String::new());
                            }
                        }
                    },
                }
            }
            if cache_loading {
                div {
                    class: "px-3 py-6 text-xs opacity-60 text-center",
                    "data-testid": "local-search-loading",
                    "Indexing notes…"
                }
            } else if needle_trim.is_empty() {
                div {
                    class: "px-3 py-6 text-xs opacity-60 text-center",
                    "data-testid": "local-search-prompt",
                    "Type to search note titles and bodies."
                }
            } else if total_files == 0 {
                div {
                    class: "px-3 py-6 text-xs opacity-60 text-center",
                    "data-testid": "local-search-empty",
                    "No matches"
                }
            } else {
                div {
                    class: "px-3 py-2 text-xs opacity-70",
                    "data-testid": "local-search-summary",
                    {format!(
                        "{} match{} across {} file{}",
                        total_lines.max(total_files),
                        if total_lines.max(total_files) == 1 { "" } else { "es" },
                        total_files,
                        if total_files == 1 { "" } else { "s" },
                    )}
                }
                div {
                    class: "flex-1 overflow-y-auto",
                    "data-testid": "local-search-results",
                    for proj in project_hits.iter().cloned() {
                        {
                            let pid = proj.project_id;
                            let name = proj.name.clone();
                            let on_proj = on_project_pick;
                            rsx! {
                                button {
                                    r#type: "button",
                                    class: "w-full text-left px-3 py-2 text-sm hover:bg-[var(--operon-hover)] flex items-center gap-2",
                                    "data-testid": "local-search-project-row",
                                    "data-project-id": "{pid}",
                                    onclick: move |_| on_proj.call(pid),
                                    span { class: "opacity-60", "Project:" }
                                    span { class: "truncate", "{name}" }
                                }
                            }
                        }
                    }
                    for note in note_hits.iter().cloned() {
                        {
                            let nid = note.note_id;
                            let is_collapsed = collapsed_now.contains(&nid);
                            let line_count = note.line_matches.len();
                            let breadcrumb = note.breadcrumb.clone();
                            let on_note = on_note_pick;
                            let proj = note.project_id;
                            let needle_lower_for_render = needle_trim.to_lowercase();
                            rsx! {
                                div {
                                    class: "flex flex-col",
                                    "data-testid": "local-search-file-row",
                                    "data-note-id": "{nid}",
                                    div {
                                        class: "flex items-center gap-1 px-2 py-1 text-sm hover:bg-[var(--operon-hover)] cursor-pointer",
                                        onclick: move |_| on_note.call((nid, proj)),
                                        button {
                                            r#type: "button",
                                            class: "px-1 opacity-70 hover:opacity-100",
                                            title: if is_collapsed { "Expand" } else { "Collapse" },
                                            onclick: move |evt| {
                                                evt.stop_propagation();
                                                collapsed_setter.with_mut(|s| {
                                                    if !s.insert(nid) { s.remove(&nid); }
                                                });
                                            },
                                            if is_collapsed { "▸" } else { "▾" }
                                        }
                                        span {
                                            class: "truncate flex-1",
                                            "data-testid": "local-search-file-breadcrumb",
                                            "{breadcrumb}"
                                        }
                                        if line_count > 0 {
                                            span {
                                                class: "text-xs opacity-60",
                                                "data-testid": "local-search-file-count",
                                                "{line_count}"
                                            }
                                        }
                                    }
                                    if !is_collapsed && line_count > 0 {
                                        div {
                                            class: "flex flex-col",
                                            for lm in note.line_matches.iter().cloned() {
                                                {
                                                    let line_number = lm.line_number;
                                                    let segments = build_segments(&lm.line_text, &lm.match_ranges);
                                                    let on_note_inner = on_note;
                                                    let mut reveal_setter = reveal_request;
                                                    let nid_for_reveal = nid;
                                                    let _ = needle_lower_for_render.clone();
                                                    rsx! {
                                                        button {
                                                            r#type: "button",
                                                            class: "w-full text-left pl-8 pr-2 py-1 text-xs font-mono hover:bg-[var(--operon-hover)] flex gap-2",
                                                            "data-testid": "local-search-line-row",
                                                            "data-line-number": "{line_number}",
                                                            onclick: move |_| {
                                                                // Set reveal-line BEFORE opening the
                                                                // tab so the editor host's mount
                                                                // effect picks up the request on
                                                                // first paint.
                                                                reveal_setter.set(Some((
                                                                    nid_for_reveal.to_string(),
                                                                    line_number as u32,
                                                                )));
                                                                on_note_inner.call((nid, proj));
                                                            },
                                                            span {
                                                                class: "opacity-50 select-none",
                                                                style: "min-width: 2.5rem; text-align: right;",
                                                                "{line_number}"
                                                            }
                                                            span {
                                                                class: "truncate",
                                                                for seg in segments.iter().cloned() {
                                                                    if seg.is_match {
                                                                        mark {
                                                                            class: "bg-[var(--operon-search-match,_rgba(255,200,0,0.35))] text-inherit",
                                                                            "{seg.text}"
                                                                        }
                                                                    } else {
                                                                        span { "{seg.text}" }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            if line_count >= MAX_LINE_HITS_PER_NOTE {
                                                div {
                                                    class: "pl-8 pr-2 py-1 text-xs opacity-60",
                                                    "+ more matches in this note"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone)]
struct Segment {
    text: String,
    is_match: bool,
}

/// Slice `text` according to `ranges` (char offsets) into alternating
/// `is_match=false`/`is_match=true` segments so the caller can wrap matches
/// in a `<mark>`.
fn build_segments(text: &str, ranges: &[(usize, usize)]) -> Vec<Segment> {
    if ranges.is_empty() {
        return vec![Segment {
            text: text.to_string(),
            is_match: false,
        }];
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<Segment> = Vec::with_capacity(ranges.len() * 2 + 1);
    let mut cursor = 0usize;
    for &(s, e) in ranges {
        let s = s.min(chars.len());
        let e = e.min(chars.len()).max(s);
        if cursor < s {
            out.push(Segment {
                text: chars[cursor..s].iter().collect(),
                is_match: false,
            });
        }
        if s < e {
            out.push(Segment {
                text: chars[s..e].iter().collect(),
                is_match: true,
            });
        }
        cursor = e;
    }
    if cursor < chars.len() {
        out.push(Segment {
            text: chars[cursor..].iter().collect(),
            is_match: false,
        });
    }
    out
}

fn build_note_meta(
    note_repo: &Arc<dyn operon_store::repos::LocalNoteRepository>,
    project_repo: &Arc<dyn operon_store::repos::LocalProjectRepository>,
) -> HashMap<Uuid, (String, NoteKind)> {
    let mut out: HashMap<Uuid, (String, NoteKind)> = HashMap::new();
    let Ok(projects) = project_repo.list() else {
        return out;
    };
    for p in projects {
        if let Ok(rows) = note_repo.list_for_project(p.id) {
            for r in rows {
                out.insert(r.id, (r.title, r.kind));
            }
        }
    }
    out
}

fn make_project_pick(
    selected_project: Signal<Option<Uuid>>,
    selected_note: Signal<Option<Uuid>>,
    workspace_open: Signal<HashMap<String, bool>>,
    tree_queue: Signal<crate::local_mode::explorer::TreeStateQueue>,
) -> Callback<Uuid> {
    let mut sp = selected_project;
    let mut sn = selected_note;
    let mut wo = workspace_open;
    Callback::new(move |pid: Uuid| {
        sp.set(Some(pid));
        sn.set(None);
        wo.with_mut(|m| {
            m.insert(pid.to_string(), true);
        });
        tree_queue
            .read()
            .enqueue(SCOPE_WORKSPACE, pid.to_string(), true);
    })
}

#[allow(clippy::too_many_arguments)]
fn make_note_pick(
    tabs: Signal<TabManager>,
    save_scheduler: SaveScheduler,
    selected_note: Signal<Option<Uuid>>,
    selected_project: Signal<Option<Uuid>>,
    workspace_open: Signal<HashMap<String, bool>>,
    tree_queue: Signal<crate::local_mode::explorer::TreeStateQueue>,
    note_meta: HashMap<Uuid, (String, NoteKind)>,
    persistence: Arc<dyn Persistence>,
) -> Callback<(Uuid, Option<Uuid>)> {
    let mut tabs_handle = tabs;
    let mut sn = selected_note;
    let mut sp = selected_project;
    let mut wo = workspace_open;
    Callback::new(move |(id, project_id): (Uuid, Option<Uuid>)| {
        sn.set(Some(id));
        if let Some(pid) = project_id {
            sp.set(Some(pid));
            wo.with_mut(|m| {
                m.insert(pid.to_string(), true);
            });
            tree_queue
                .read()
                .enqueue(SCOPE_WORKSPACE, pid.to_string(), true);
        }
        let (title, kind) = note_meta
            .get(&id)
            .cloned()
            .unwrap_or_else(|| (id.to_string(), NoteKind::Markdown));
        let id_str = id.to_string();
        #[cfg(not(target_arch = "wasm32"))]
        let initial_content = match futures::executor::block_on(persistence.load(&id_str)) {
            Ok(bytes) => String::from_utf8(bytes).unwrap_or_default(),
            Err(crate::persistence::PersistError::NotFound) => String::new(),
            Err(e) => {
                eprintln!("operon: local-search open load error note={id_str}: {e:?}");
                String::new()
            }
        };
        #[cfg(target_arch = "wasm32")]
        let initial_content = String::new();

        let new_tab_id = open_local_note_tab(
            tabs_handle,
            save_scheduler.clone(),
            id,
            title,
            initial_content,
            kind,
        );

        #[cfg(target_arch = "wasm32")]
        {
            let pers = persistence.clone();
            let mut tabs_h = tabs_handle;
            spawn(async move {
                match pers.load(&id_str).await {
                    Ok(bytes) => {
                        if let Ok(content) = String::from_utf8(bytes) {
                            tabs_h.write().set_content(new_tab_id, content);
                        }
                    }
                    Err(crate::persistence::PersistError::NotFound) => {}
                    Err(e) => eprintln!(
                        "operon: local-search open load error note={id_str}: {e:?}"
                    ),
                }
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = new_tab_id;

        let _ = tabs_handle.write();
    })
}
