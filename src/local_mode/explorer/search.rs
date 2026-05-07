//! Phase-5 in-explorer search. Renders a title-only quick-filter input at the
//! top of the explorer panel and a flat results list when the query is
//! non-empty. Cross-note content search lives in the dedicated
//! [`crate::plugins::local_search`] activity panel — the explorer's input
//! never scans bodies, so there is no body-cache lifecycle here.
//!
//! Wiring:
//! - The query/focus signals live at [`crate::local_mode::explorer::ExplorerPanel`]
//!   scope so the panel can swap between the tree view and the results list.
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

const SEARCH_DEBOUNCE_MS: u64 = 150;
const SCOPE_WORKSPACE: &str = "workspace";

/// Header search input. Pushes value into the shared `query` signal; the
/// panel debounces consumption and swaps the tree for the results list when
/// the query is non-empty. Title-only — content search lives in the
/// [`crate::plugins::local_search`] panel.
#[component]
pub fn ExplorerSearch(query: Signal<String>, on_clear: Callback<()>) -> Element {
    let mut query_setter = query;
    let local_value = query.read().clone();

    rsx! {
        div {
            class: "notes-explorer-search-form",
            "data-testid": "explorer-search-form",
            input {
                r#type: "search",
                class: "notes-explorer-search-input",
                "data-testid": "explorer-search-input",
                placeholder: "Filter projects and notes",
                value: "{local_value}",
                onmounted: move |evt| {
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
        }
    }
}

/// Snapshot of the body cache keyed by note id. Built lazily by the
/// [`crate::plugins::local_search`] panel on first mount; the explorer
/// quick-filter never populates it.
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
    pub on_pick: Callback<(SearchKind, Uuid, Option<Uuid>)>,
}

#[component]
pub fn ResultsList(props: ResultsListProps) -> Element {
    let search_repo: ExplorerSearchRepo = use_context();
    let cap = DEFAULT_SEARCH_LIMIT;
    let needle = props.query.trim().to_string();

    // Title-only quick filter — the body loader is never invoked because
    // `in_content == false`.
    let hits: Vec<SearchHit> = if needle.is_empty() {
        Vec::new()
    } else {
        let loader = |_id: Uuid| -> Option<String> { None };
        match search_repo.0.search(&needle, false, cap, &loader) {
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
                    "Type to filter"
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
                        }
                    }
                }
            }
            if truncated {
                div {
                    class: "px-3 py-2 text-xs opacity-60 text-center",
                    "data-testid": "search-result-truncated",
                    "+ more matches; refine your filter"
                }
            }
        }
    }
}

/// Newtype for context lookup. Mounted by the panel from the shared store.
#[derive(Clone)]
pub struct ExplorerSearchRepo(pub Arc<dyn LocalSearchRepository>);

/// Build the body cache by loading every note body via `Persistence`. Spawned
/// from a `use_effect` in [`crate::plugins::local_search::LocalSearchPanel`]
/// when the panel first mounts.
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
        },
    )
}

/// Lightweight async debounce: returns a future that resolves after
/// `SEARCH_DEBOUNCE_MS` milliseconds. The caller cancels by dropping the spawn.
pub async fn debounce_window() {
    futures_timer::Delay::new(Duration::from_millis(SEARCH_DEBOUNCE_MS)).await;
}
