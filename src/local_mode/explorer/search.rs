//! Phase-5 in-explorer search. Renders a search box (with optional "in
//! content" checkbox) at the top of the explorer panel and a flat results
//! list when the query is non-empty.
//!
//! Wiring:
//! - The query/in_content/focus signals live at [`crate::local_mode::explorer::ExplorerPanel`]
//!   scope so the panel can swap between the tree view and the results list.
//! - Body matching is opt-in. When the user toggles "in content" we spawn a
//!   one-shot pre-load over `Persistence` that builds a `HashMap<Uuid, String>`,
//!   then pass a closure that reads from the snapshot to
//!   [`LocalSearchRepository::search`].
//! - Clicking a result opens the matching note (`SelectedNote` + tab) and
//!   ensures the owning project is open in the workspace tree-state, then
//!   clears the query so the tree reappears with the right context.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use dioxus::prelude::*;
use operon_store::repos::{LocalSearchRepository, SearchHit, SearchKind, DEFAULT_SEARCH_LIMIT};
use uuid::Uuid;

use crate::local_mode::editor::open_local_note_tab;
use crate::local_mode::explorer::tree_state::TreeStateQueue;
use crate::persistence::Persistence;
use crate::tabs::{SaveScheduler, TabManager};
use operon_store::repos::NoteKind;

/// Signal flipped by the `Ctrl+Shift+F` global shortcut. The search input
/// component watches this and steals focus when bumped.
#[derive(Clone, Copy)]
pub struct ExplorerSearchFocus(pub Signal<u64>);

const SEARCH_DEBOUNCE_MS: u64 = 150;
const SCOPE_WORKSPACE: &str = "workspace";

/// Header search input + "in content" toggle. Pushes value into the shared
/// `query`/`in_content` signals; debounced inside the consumer (the panel
/// only triggers `search()` after the same debounce window).
#[component]
pub fn ExplorerSearch(
    query: Signal<String>,
    in_content: Signal<bool>,
    focus_tick: Signal<u64>,
    on_clear: Callback<()>,
) -> Element {
    let mut query_setter = query;
    let mut in_content_setter = in_content;

    let local_value = query.read().clone();
    let checked = *in_content.read();

    rsx! {
        div {
            class: "notes-explorer-search-form",
            "data-testid": "explorer-search-form",
            input {
                r#type: "search",
                class: "notes-explorer-search-input",
                "data-testid": "explorer-search-input",
                placeholder: "Search projects and notes",
                value: "{local_value}",
                onmounted: move |evt| {
                    // Watch the focus tick so Ctrl+Shift+F (which bumps it) re-focuses
                    // this input. The mount handler re-runs whenever Dioxus remounts,
                    // and the explicit set_focus inside the focus effect below handles
                    // subsequent triggers via element-id lookup. Here we just register
                    // an initial focusable element if the signal already has a value.
                    let _ = focus_tick.read();
                    drop(evt.set_focus(true));
                },
                oninput: move |evt| query_setter.set(evt.value()),
                onkeydown: move |evt| {
                    let key = evt.key().to_string();
                    if key == "Escape" {
                        evt.prevent_default();
                        on_clear.call(());
                    }
                },
            }
            label {
                class: "notes-explorer-search-incontent",
                input {
                    r#type: "checkbox",
                    "data-testid": "explorer-search-in-content",
                    checked: "{checked}",
                    onchange: move |evt| {
                        in_content_setter.set(evt.value() == "true" || evt.value() == "on");
                    },
                }
                span { "in content" }
            }
        }
    }
}

/// Snapshot of the body cache keyed by note id. Built lazily when
/// `in_content` flips on; cleared when it flips off.
#[derive(Clone, Default)]
pub struct BodyCache(pub Arc<HashMap<Uuid, String>>);

impl PartialEq for BodyCache {
    fn eq(&self, other: &Self) -> bool {
        // Pointer-identity is enough for prop-equality (Dioxus uses this to
        // skip rerendering). The cache only reassigns wholesale.
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct ResultsListProps {
    pub query: String,
    pub in_content: bool,
    pub body_cache: BodyCache,
    pub on_pick: Callback<(SearchKind, Uuid, Option<Uuid>)>,
}

#[component]
pub fn ResultsList(props: ResultsListProps) -> Element {
    let search_repo: ExplorerSearchRepo = use_context();
    let cap = DEFAULT_SEARCH_LIMIT;
    let needle = props.query.trim().to_string();
    let in_content = props.in_content;
    let cache = props.body_cache.0.clone();

    // Run the search synchronously — it's a single SQLite read with at most
    // ~250 rows in practice; well below 16ms.
    let hits: Vec<SearchHit> = if needle.is_empty() {
        Vec::new()
    } else {
        let cache_for_loader = cache.clone();
        let loader = move |id: Uuid| -> Option<String> { cache_for_loader.get(&id).cloned() };
        match search_repo.0.search(&needle, in_content, cap, &loader) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("operon: search failed: {e}");
                Vec::new()
            }
        }
    };

    if hits.is_empty() {
        return rsx! {
            div {
                class: "px-3 py-6 text-xs opacity-60 text-center",
                "data-testid": "search-result-empty",
                if needle.is_empty() {
                    "Type to search"
                } else {
                    "No matches"
                }
            }
        };
    }

    let on_pick = props.on_pick;
    let truncated = hits.len() >= cap;

    rsx! {
        div {
            class: "flex-1 overflow-y-auto",
            "data-testid": "search-results",
            for hit in hits.iter().cloned() {
                {
                    let kind = hit.kind.clone();
                    let id = hit.id;
                    let project_id = hit.project_id;
                    let breadcrumb = hit.breadcrumb.clone();
                    let id_str = id.to_string();
                    let snippet_html = hit.snippet.clone();
                    rsx! {
                        button {
                            r#type: "button",
                            class: "w-full text-left px-3 py-2 text-sm hover:bg-[var(--operon-hover)] border-b border-[var(--operon-border)] flex flex-col gap-1",
                            "data-testid": "search-result-row",
                            "data-result-id": "{id_str}",
                            "data-result-kind": match kind { SearchKind::Project => "project", SearchKind::Note => "note" },
                            onclick: move |_| {
                                on_pick.call((kind.clone(), id, project_id));
                            },
                            span {
                                class: "truncate text-xs opacity-80",
                                "data-testid": "search-result-breadcrumb",
                                "{breadcrumb}"
                            }
                            if let Some(snippet) = snippet_html.clone() {
                                span {
                                    class: "text-xs opacity-60 truncate",
                                    "data-testid": "search-result-snippet",
                                    "{snippet}"
                                }
                            }
                        }
                    }
                }
            }
            if truncated {
                div {
                    class: "px-3 py-2 text-xs opacity-60 text-center",
                    "data-testid": "search-result-truncated",
                    "+ more matches; refine your query"
                }
            }
        }
    }
}

/// Newtype for context lookup. Mounted by the panel from the shared store.
#[derive(Clone)]
pub struct ExplorerSearchRepo(pub Arc<dyn LocalSearchRepository>);

/// Build the body cache by loading every note body via `Persistence`. Spawned
/// from a `use_effect` in the panel when the user toggles `in_content` on.
pub async fn load_body_cache(
    note_ids: Vec<Uuid>,
    persistence: Arc<dyn Persistence>,
) -> HashMap<Uuid, String> {
    let mut out: HashMap<Uuid, String> = HashMap::with_capacity(note_ids.len());
    for nid in note_ids {
        let key = nid.to_string();
        match persistence.load(&key).await {
            Ok(bytes) => {
                if let Ok(s) = String::from_utf8(bytes) {
                    out.insert(nid, s);
                }
            }
            Err(_) => {
                // Note never saved to disk yet; skip.
            }
        }
    }
    out
}

/// Re-export so the panel can pass it to a result-click handler.
#[allow(clippy::too_many_arguments)]
pub fn click_handler(
    mut tabs: Signal<TabManager>,
    save_scheduler: SaveScheduler,
    mut selected_note: Signal<Option<Uuid>>,
    mut selected_project: Signal<Option<Uuid>>,
    mut workspace_open: Signal<HashMap<String, bool>>,
    tree_queue: Signal<TreeStateQueue>,
    note_meta: Signal<HashMap<Uuid, (String, NoteKind)>>,
    persistence: Arc<dyn Persistence>,
    mut query: Signal<String>,
    mut in_content: Signal<bool>,
) -> Callback<(SearchKind, Uuid, Option<Uuid>)> {
    Callback::new(
        move |(kind, id, project_id): (SearchKind, Uuid, Option<Uuid>)| {
            match kind {
                SearchKind::Project => {
                    selected_project.set(Some(id));
                    selected_note.set(None);
                    // Open the project in the workspace tree-state so the panel
                    // shows it expanded once we clear the query.
                    workspace_open.with_mut(|m| {
                        m.insert(id.to_string(), true);
                    });
                    tree_queue
                        .read()
                        .enqueue(SCOPE_WORKSPACE, id.to_string(), true);
                }
                SearchKind::Note => {
                    selected_note.set(Some(id));
                    if let Some(pid) = project_id {
                        selected_project.set(Some(pid));
                        workspace_open.with_mut(|m| {
                            m.insert(pid.to_string(), true);
                        });
                        tree_queue
                            .read()
                            .enqueue(SCOPE_WORKSPACE, pid.to_string(), true);
                    }
                    let (title, kind) = note_meta
                        .read()
                        .get(&id)
                        .cloned()
                        .unwrap_or_else(|| (id.to_string(), NoteKind::Markdown));
                    // Hydrate the tab buffer from disk before opening, mirroring
                    // the explorer click path (`on_select_note` in mod.rs). On
                    // desktop, `FilesystemPersistence::load` wraps a synchronous
                    // `std::fs::read`, so `block_on` resolves in one poll. On
                    // wasm, fall back to opening empty + async-set_content so we
                    // don't deadlock the browser thread.
                    let id_str = id.to_string();
                    #[cfg(not(target_arch = "wasm32"))]
                    let initial_content = match futures::executor::block_on(
                        persistence.load(&id_str),
                    ) {
                        Ok(bytes) => String::from_utf8(bytes).unwrap_or_default(),
                        Err(crate::persistence::PersistError::NotFound) => String::new(),
                        Err(e) => {
                            eprintln!("operon: search-open load error note={id_str}: {e:?}");
                            String::new()
                        }
                    };
                    #[cfg(target_arch = "wasm32")]
                    let initial_content = String::new();

                    let new_tab_id = open_local_note_tab(
                        tabs,
                        save_scheduler.clone(),
                        id,
                        title,
                        initial_content,
                        kind,
                    );

                    #[cfg(target_arch = "wasm32")]
                    {
                        let pers = persistence.clone();
                        let mut tabs_handle = tabs;
                        spawn(async move {
                            match pers.load(&id_str).await {
                                Ok(bytes) => {
                                    if let Ok(content) = String::from_utf8(bytes) {
                                        tabs_handle.write().set_content(new_tab_id, content);
                                    }
                                }
                                Err(crate::persistence::PersistError::NotFound) => {}
                                Err(e) => eprintln!(
                                    "operon: search-open load error note={id_str}: {e:?}"
                                ),
                            }
                        });
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    let _ = new_tab_id;

                    let _ = tabs.write();
                }
            }
            // Clear the query so the tree re-appears with the right project expanded.
            query.set(String::new());
            in_content.set(false);
        },
    )
}

/// Lightweight async debounce: returns a future that resolves after
/// `SEARCH_DEBOUNCE_MS` milliseconds. The caller cancels by dropping the spawn.
pub async fn debounce_window() {
    futures_timer::Delay::new(Duration::from_millis(SEARCH_DEBOUNCE_MS)).await;
}
