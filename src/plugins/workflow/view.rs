//! Workflow note canvas + JSON editor (M3b).
//!
//! V1 scope is intentionally narrow: a read-only SVG canvas that
//! visualises a `WorkflowGraph`, plus an Edit mode that pairs that
//! canvas with a JSON textarea so a BA can hand-edit the graph and
//! watch it light up. Drag-to-position + click-to-edit is M3b.5;
//! cascade execution is M3c.

use dioxus::prelude::*;
use keyboard_types::Modifiers;
use operon_store::repos::{LocalNote, NoteKind};
use operon_store::repos::{LocalNoteRepository, LocalProjectRepository};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::local_mode::desktop::{LocalNoteRepo, LocalProjectRepo};
use crate::local_mode::explorer::{FocusedNode as ExplorerFocused, LocalNoteVersion, NodeKey, SelectedNote};
use crate::editor::EditorMode;
use crate::local_mode::editor::open_local_note_tab;
use crate::persistence::{PersistError, Persistence};
use crate::tabs::{SaveScheduler, TabManager};
use crate::plugins::markdown::MarkdownView;
use crate::plugins::artifact::frontmatter::ArtifactStatus;
use crate::plugins::artifact::view::{mark_descendants_dirty, patch_status_text};
use crate::plugins::workflow::engine::{propagate_dirty, topo_order_dirty, SkillBag, SkillSnapshot, hash_body};
use crate::plugins::workflow::executor::{
    collect_upstream_outputs, run_node, CascadeTranscriptSink, RunArtifact,
};
use crate::plugins::workflow::state::{Edge, EdgeId, Node, NodeId, NodeStatus, WorkflowGraph};
use crate::shell::companion_state::{
    ActiveChatScope, ActiveChatSession, ChatMessageRepo, ChatSessionRepo, ChatSessionVersion,
    ClaudeCodePluginCtx, NodeLiveState, NODE_LIVE_STATE,
};
use operon_store::repos::ChatScope;

/// Node card dimensions. Sized for a leadership demo: comfortable
/// reading width, and tall enough to fit five rows of in-tile
/// controls (header, name, artifact-action strip, NodeStatus pills,
/// footer icons) without text overlap. The auto-arrange `COL_GAP` /
/// `ROW_GAP` constants are tuned around these so columns don't bleed.
const NODE_W: f64 = 260.0;
const NODE_H: f64 = 210.0;
/// Vertical row baselines inside a node (in node-local coords). Used
/// by the SVG render and by the edge-anchor distribution to keep
/// child layouts aligned with the visible row structure.
const NODE_ROW1_Y: f64 = 22.0; // header strip (chevron + kind + ≡ ▶ ✏)
const NODE_ROW2_Y: f64 = 64.0; // name / title text baseline
const NODE_ROW_ACTIONS_Y: f64 = 92.0; // artifact action strip (Approve / Reject / …)
const NODE_ROW_LIVE_Y: f64 = 112.0; // M3 live-activity readout (skill nodes only)
const NODE_ROW_STATUS_Y: f64 = 132.0; // NodeStatus pills (Dirty / Running / …)
const NODE_ROW_FOOTER_Y: f64 = 172.0; // 👁 view + 🗑 delete icons
/// Top of each in-tile control band — used as the rect `y` for
/// inline buttons / pills. Distinct from the *baseline* values above
/// (which position SVG `<text>`).
const NODE_BTN_H: f64 = 22.0;

#[derive(Props, Clone, PartialEq)]
pub struct WorkflowEditorProps {
    pub note_id: String,
    pub content: String,
    pub on_change: EventHandler<String>,
}

#[component]
pub fn WorkflowEditor(props: WorkflowEditorProps) -> Element {
    let initial_graph = parse_or_default(&props.content);
    let initial_text = serialize(&initial_graph);
    let text = use_signal(|| initial_text.clone());
    let on_change = props.on_change;
    let note_id = props.note_id.clone();
    // Toggle for the JSON-tree pane to the right of the canvas. Default
    // hidden — most BAs work entirely off the canvas, and the textarea
    // takes screen real-estate that the canvas can use. Toolbar exposes
    // a button that flips this.
    let json_visible: Signal<bool> = use_signal(|| false);

    // Phase C: per-phase canvas filter. When `true` (the default), the
    // canvas drops every edge whose endpoints belong to different
    // `NoteKind::Phase` ancestors. Cross-phase dependencies are allowed
    // in the data model (e.g. a Phase 1 epic depending on Phase 0
    // infrastructure) but they clutter the visualisation for the
    // common case of one-phase-at-a-time review. Toolbar exposes a
    // button to flip this so the user can see the full DAG when
    // debugging cross-phase wiring.
    let hide_cross_phase_edges: Signal<bool> = use_signal(|| true);

    // Undo / redo / clipboard — editor-scope so they survive canvas
    // re-renders. History caps at HISTORY_CAP entries (oldest dropped
    // when full) to avoid unbounded memory growth on long sessions.
    let history: Signal<Vec<WorkflowGraph>> = use_signal(Vec::new);
    let redo_stack: Signal<Vec<WorkflowGraph>> = use_signal(Vec::new);
    // Clipboard for cut/copy/paste. Stores nodes by value (not id) so
    // a paste mints fresh UUIDs and offsets positions; edges internal
    // to the copied set get re-keyed alongside.
    let clipboard: Signal<Option<WorkflowClipboard>> = use_signal(|| None);
    // Expand/collapse set — lifted to editor scope so the toolbar's
    // "Expand all" / "Collapse all" buttons share state with the
    // canvas's per-node chevrons. Hydrated on first mount from the
    // workflow note's persisted `view_state.expanded_nodes`, so closing
    // and reopening the note restores the exact level-by-level drill-
    // down the user was looking at. New workflows have an empty
    // expanded set → only root nodes show, matching the demo flow.
    let expanded: Signal<std::collections::BTreeSet<NodeId>> = {
        let initial: std::collections::BTreeSet<NodeId> = initial_graph
            .view_state
            .expanded_nodes
            .iter()
            .copied()
            .collect();
        use_signal(move || initial.clone())
    };

    // Live-parse from the textarea so the canvas updates with every
    // keystroke. Bad JSON renders the previous good graph + a parse
    // error banner.
    let snapshot = text.read().clone();
    let parse_outcome = serde_json::from_str::<WorkflowGraph>(&snapshot);
    let graph_for_canvas = parse_outcome
        .as_ref()
        .ok()
        .cloned()
        .unwrap_or_else(|| initial_graph.clone());
    let parse_error = parse_outcome.err().map(|e| e.to_string());

    // Apply-graph callback: serialize + push through `text` Signal +
    // fire `on_change`. Used by the toolbar (add-node, cascade) and by
    // per-node Run buttons.
    let apply_graph: Callback<WorkflowGraph> = {
        let mut text_setter = text;
        let on_change = on_change;
        Callback::new(move |g: WorkflowGraph| {
            let s = serialize(&g);
            text_setter.set(s.clone());
            on_change.call(s);
        })
    };

    // Callback every chevron / Expand-all / Collapse-all path calls
    // after writing to `expanded`. Merges the new set into the graph's
    // `view_state` and pushes the rewritten JSON via `apply_graph`
    // (raw — not undoable; expand state isn't a graph edit).
    let persist_expanded: Callback<std::collections::BTreeSet<NodeId>> = {
        let snapshot_text = text;
        let apply_graph_inner = apply_graph;
        Callback::new(move |new_set: std::collections::BTreeSet<NodeId>| {
            let cur_text = snapshot_text.peek().clone();
            let Ok(mut g) = serde_json::from_str::<WorkflowGraph>(&cur_text) else {
                return;
            };
            g.view_state.expanded_nodes = new_set.iter().copied().collect();
            apply_graph_inner.call(g);
        })
    };

    // Live-refresh hook: subscribe to `WORKFLOW_GRAPH_VERSION` keyed on
    // this workflow note's id. When the cascade graph writer flushes
    // a new revision to disk, the bump triggers this effect; we async-
    // load the body, auto-arrange so the new artifact-snapshot tiles
    // don't pile on top of existing ones, and auto-expand any
    // artifact-snapshot parents so newly-produced child nodes appear
    // immediately in the canvas. Idempotent — last-seen-version guard
    // prevents re-firing on unchanged renders.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let workflow_id_for_effect = note_id.clone();
        let persistence_for_effect: std::sync::Arc<dyn crate::persistence::Persistence> =
            use_context();
        let mut last_seen_version: Signal<u64> = use_signal(|| 0);
        let mut expanded_for_effect = expanded;
        let apply_graph_for_effect = apply_graph;
        use_effect(move || {
            let Ok(wf_id) = Uuid::parse_str(&workflow_id_for_effect) else {
                return;
            };
            let cur_version = crate::shell::companion_state::WORKFLOW_GRAPH_VERSION
                .read()
                .get(&wf_id)
                .copied()
                .unwrap_or(0);
            if cur_version == 0 || cur_version <= *last_seen_version.peek() {
                return;
            }
            last_seen_version.set(cur_version);
            let persistence = persistence_for_effect.clone();
            let id_str = wf_id.to_string();
            spawn(async move {
                let body_bytes = match persistence.load(&id_str).await {
                    Ok(b) => b,
                    Err(e) => {
                        eprintln!("operon: workflow live-refresh load failed: {e:?}");
                        return;
                    }
                };
                let body = match String::from_utf8(body_bytes) {
                    Ok(s) => s,
                    Err(_) => return,
                };
                let parsed: WorkflowGraph = match serde_json::from_str(&body) {
                    Ok(g) => g,
                    Err(e) => {
                        eprintln!("operon: workflow live-refresh parse failed: {e}");
                        return;
                    }
                };
                let arranged = auto_arrange(&parsed);
                let mut new_expanded: std::collections::BTreeSet<NodeId> =
                    expanded_for_effect.peek().clone();
                for (id, node) in arranged.nodes.iter() {
                    if !node.is_artifact_snapshot {
                        continue;
                    }
                    let has_out = arranged.edges.iter().any(|e| e.from == *id);
                    if has_out {
                        new_expanded.insert(*id);
                    }
                }
                let mut g_final = arranged;
                g_final.view_state.expanded_nodes =
                    new_expanded.iter().copied().collect();
                expanded_for_effect.set(new_expanded);
                apply_graph_for_effect.call(g_final);
            });
        });
    }
    // Same idea for viewport pan / zoom. Called from canvas-scope
    // gestures (pan-drag mouseup, zoom buttons, ctrl+wheel zoom) so
    // closing and reopening the workflow lands the viewport exactly
    // where the user left it.
    let persist_view: Callback<(f64, f64, f64)> = {
        let snapshot_text = text;
        let apply_graph_inner = apply_graph;
        Callback::new(move |(px, py, z): (f64, f64, f64)| {
            let cur_text = snapshot_text.peek().clone();
            let Ok(mut g) = serde_json::from_str::<WorkflowGraph>(&cur_text) else {
                return;
            };
            g.view_state.pan_x = px;
            g.view_state.pan_y = py;
            g.view_state.zoom = z;
            apply_graph_inner.call(g);
        })
    };

    // Undo-aware variant — captures the current parsed graph before
    // forwarding to `apply_graph`, so Ctrl+Z on the canvas can revert
    // it. Caps at HISTORY_CAP to bound memory; clears `redo_stack` on
    // every new edit (standard branching-on-edit semantics).
    let apply_with_undo: Callback<WorkflowGraph> = {
        let snapshot_text = text;
        let mut history_setter = history;
        let mut redo_setter = redo_stack;
        let apply_graph_inner = apply_graph;
        Callback::new(move |g: WorkflowGraph| {
            let cur_text = snapshot_text.peek().clone();
            if let Ok(prev) = serde_json::from_str::<WorkflowGraph>(&cur_text) {
                history_setter.with_mut(|h| {
                    h.push(prev);
                    if h.len() > HISTORY_CAP {
                        h.remove(0);
                    }
                });
                redo_setter.with_mut(|r| r.clear());
            }
            apply_graph_inner.call(g);
        })
    };

    rsx! {
        div { class: "operon-workflow-surface operon-workflow-surface-edit",
            "data-testid": "workflow-editor",
            WorkflowToolbar {
                note_id: note_id.clone(),
                graph_text: snapshot.clone(),
                on_apply: Callback::new({
                    let mut text_setter = text;
                    let on_change = on_change;
                    move |new_text: String| {
                        text_setter.set(new_text.clone());
                        on_change.call(new_text);
                    }
                }),
                apply_graph: apply_graph,
                apply_with_undo: apply_with_undo,
                json_visible: json_visible,
                hide_cross_phase_edges: hide_cross_phase_edges,
                expanded: expanded,
                persist_expanded: persist_expanded,
            }
            div { class: "operon-workflow-pane",
                WorkflowCanvas {
                    graph: graph_for_canvas,
                    note_id: note_id.clone(),
                    apply_graph: apply_graph,
                    apply_with_undo: apply_with_undo,
                    history: history,
                    redo_stack: redo_stack,
                    clipboard: clipboard,
                    expanded: expanded,
                    persist_expanded: persist_expanded,
                    persist_view: persist_view,
                    hide_cross_phase_edges: hide_cross_phase_edges,
                }
                if *json_visible.read() {
                    div { class: "operon-workflow-json",
                        if let Some(err) = parse_error.as_ref() {
                            div {
                                class: "operon-workflow-parse-error",
                                "data-testid": "workflow-parse-error",
                                "Invalid JSON \u{2014} canvas shows last good graph: {err}"
                            }
                        }
                        textarea {
                            class: "operon-workflow-textarea",
                            "data-testid": "workflow-textarea",
                            spellcheck: "false",
                            value: "{text}",
                            oninput: move |e| {
                                let mut text_setter = text;
                                text_setter.set(e.value());
                                on_change.call(e.value());
                            },
                        }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct WorkflowViewProps {
    pub content: String,
}

#[component]
pub fn WorkflowView(props: WorkflowViewProps) -> Element {
    let graph = parse_or_default(&props.content);
    // No-op apply for read-only View mode — Run buttons in the canvas
    // are still rendered but produce no graph mutations.
    let noop: Callback<WorkflowGraph> = Callback::new(|_| {});
    // View-mode placeholders for undo / clipboard infra. These signals
    // are inert because there's no apply path to drive them.
    let history: Signal<Vec<WorkflowGraph>> = use_signal(Vec::new);
    let redo_stack: Signal<Vec<WorkflowGraph>> = use_signal(Vec::new);
    let clipboard: Signal<Option<WorkflowClipboard>> = use_signal(|| None);
    let expanded: Signal<std::collections::BTreeSet<NodeId>> = {
        let initial: std::collections::BTreeSet<NodeId> = graph
            .view_state
            .expanded_nodes
            .iter()
            .copied()
            .collect();
        use_signal(move || initial.clone())
    };
    let persist_noop: Callback<std::collections::BTreeSet<NodeId>> =
        Callback::new(|_| {});
    let persist_view_noop: Callback<(f64, f64, f64)> = Callback::new(|_| {});
    let hide_cross_phase_edges: Signal<bool> = use_signal(|| true);
    rsx! {
        div { class: "operon-workflow-surface",
            "data-testid": "workflow-view",
            WorkflowCanvas {
                graph,
                note_id: String::new(),
                apply_graph: noop,
                apply_with_undo: noop,
                history: history,
                redo_stack: redo_stack,
                clipboard: clipboard,
                expanded: expanded,
                persist_expanded: persist_noop,
                persist_view: persist_view_noop,
                hide_cross_phase_edges: hide_cross_phase_edges,
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct WorkflowCanvasProps {
    graph: WorkflowGraph,
    note_id: String,
    /// Raw apply — overwrites the JSON text without recording an
    /// undoable history entry. Used for drag positions, ghost edges,
    /// inspector live edits, per-node run state.
    apply_graph: Callback<WorkflowGraph>,
    /// Undo-aware apply — pushes the *previous* graph onto `history`,
    /// clears `redo_stack`, then forwards to `apply_graph`. Used for
    /// discrete keyboard actions: Delete, Cut, Paste, Auto-arrange.
    apply_with_undo: Callback<WorkflowGraph>,
    /// Editor-scope undo / redo / clipboard signals so the canvas
    /// keyboard handler can manipulate them.
    history: Signal<Vec<WorkflowGraph>>,
    redo_stack: Signal<Vec<WorkflowGraph>>,
    clipboard: Signal<Option<WorkflowClipboard>>,
    /// Editor-scope expand/collapse set, shared with the toolbar's
    /// Expand-all / Collapse-all buttons.
    expanded: Signal<std::collections::BTreeSet<NodeId>>,
    /// Persistence hook — invoked after every mutation of `expanded`
    /// so the workflow note's `view_state` stays in sync with the UI.
    persist_expanded: Callback<std::collections::BTreeSet<NodeId>>,
    /// Persistence hook for viewport pan / zoom. Called on pan-drag
    /// mouseup, zoom-button click, and ctrl+wheel zoom so the
    /// workflow note remembers the last viewport across close/reopen.
    persist_view: Callback<(f64, f64, f64)>,
    /// Phase C: when `true`, edges whose endpoints belong to different
    /// `NoteKind::Phase` ancestors are dropped from the rendered set.
    /// Toggled from the toolbar.
    hide_cross_phase_edges: Signal<bool>,
}

#[derive(Clone, Debug, PartialEq)]
struct DragState {
    /// The node the user grabbed — drives DragState lifecycle but the
    /// translation applies to every entry in `start_positions`.
    node: NodeId,
    /// Mouse client position at drag start, so subsequent mousemove
    /// events can compute a delta against it (we update each node's
    /// position by that delta, divided by zoom).
    start_client_x: f64,
    start_client_y: f64,
    /// (id, original_x, original_y) for *every* selected node at drag
    /// start. On mousemove we translate all of them by the same delta
    /// so a multi-selection moves as a group. When the user grabs an
    /// unselected node the set is just (node, n_x, n_y).
    start_positions: Vec<(NodeId, f64, f64)>,
}

/// Active edge-creation drag. The user mousedowns on a node's output
/// handle, the cursor follows in the SVG until they mouseup on a
/// target node's input handle (commit) or anywhere else (cancel).
/// Coordinates are tracked the same way `DragState` tracks node drags:
/// keep the start client position + the source handle's world coords,
/// then add the client delta (divided by current zoom) on every
/// mousemove to get the current cursor in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
struct EdgeDragState {
    from: NodeId,
    from_x: f64,
    from_y: f64,
    cursor_x: f64,
    cursor_y: f64,
    start_client_x: f64,
    start_client_y: f64,
}

/// Active canvas pan (middle-click or shift+drag). Tracks the start
/// pan offset and the start client position; on every mousemove we
/// shift `pan_x/pan_y` by the client delta. Pan is in screen space,
/// so no zoom-scaling is applied here.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PanDragState {
    start_client_x: f64,
    start_client_y: f64,
    start_pan_x: f64,
    start_pan_y: f64,
}

/// Cap on the undo / redo history so long sessions don't grow
/// unbounded. 100 discrete actions ≈ 100 graph clones; for the sizes
/// we typically see (≤ a few hundred nodes) that's small.
const HISTORY_CAP: usize = 100;

/// Cut / copy clipboard payload. Holds nodes by value (with their
/// original ids — paste mints fresh UUIDs) and the edges between them.
/// Cross-set edges (one endpoint inside the copied set, the other
/// outside) are dropped.
#[derive(Clone, Debug, PartialEq)]
struct WorkflowClipboard {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
}

/// Marquee (rubber-band) selection state. `start_*` are world-space
/// SVG coords captured on mousedown; `cur_*` are the world-space coords
/// of the cursor's current position. The render reads both and draws a
/// dashed `<rect>`; on mouseup we test every node card against the
/// normalized bounding rect and write the set into `selected_nodes`.
/// `additive` mirrors the modifier the user held on mousedown — Shift
/// adds the marquee result to the existing selection; no modifier
/// replaces it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct MarqueeState {
    start_x: f64,
    start_y: f64,
    cur_x: f64,
    cur_y: f64,
    /// Client-space anchor — needed to map subsequent mousemove client
    /// deltas back to world space (mirrors the pattern used by node /
    /// edge drag states).
    start_client_x: f64,
    start_client_y: f64,
    additive: bool,
}

#[component]
fn WorkflowCanvas(props: WorkflowCanvasProps) -> Element {
    let g = props.graph.clone();
    // Hook-based reads of the rail signals so the per-node ▶ button
    // can switch the companion to the cascade session via Signals
    // bound to a live runtime scope. `try_consume_context` from
    // inside the click handler returns handles whose writes are
    // silently dropped.
    let active_session_signal: Option<Signal<Option<Uuid>>> =
        try_consume_context::<ActiveChatSession>().map(|c| c.0);
    let active_scope_signal: Option<Signal<ChatScope>> =
        try_consume_context::<ActiveChatScope>().map(|c| c.0);
    // Resolve skill-id → title once per render so node tiles can show
    // the human skill name (e.g. `ba-discover-epics`) instead of the
    // raw UUID prefix. Falls back to an empty map when LocalNoteRepo
    // isn't in scope (read-only `WorkflowView`) or the workflow note
    // can't be resolved to a project.
    // Project-wide note title map. Used by `node_label` for both skill
    // nodes (resolved by `skill_note_id`) and artifact-snapshot nodes
    // whose `artifact_title` is missing — instead of falling back to
    // the kind+UUID-prefix string ("Artifact c7752c7e"), look up the
    // referenced note's actual title (e.g. "Requirements") so the
    // canvas tile reads like the explorer row.
    let skill_titles: HashMap<Uuid, String> = {
        let mut out = HashMap::new();
        if let (Ok(workflow_id), Some(LocalNoteRepo(repo))) = (
            Uuid::parse_str(&props.note_id),
            try_consume_context::<LocalNoteRepo>(),
        ) {
            if let Ok(Some(project_id)) = repo.find_project_for_note(workflow_id) {
                if let Ok(rows) = repo.list_for_project(project_id) {
                    for row in rows {
                        // Include every note kind; previously this was
                        // gated on `NoteKind::Skill`, which left
                        // artifact and markdown notes unresolvable.
                        out.insert(row.id, row.title);
                    }
                }
            }
        }
        out
    };
    // Parallel kind map keyed by note id — needed by the inspector's
    // View button so it knows which `format_id` to pass to
    // `open_local_note_tab`. Built from the same project scan above
    // so the (title, kind) pair is consistent.
    let note_kinds: HashMap<Uuid, NoteKind> = {
        let mut out = HashMap::new();
        if let (Ok(workflow_id), Some(LocalNoteRepo(repo))) = (
            Uuid::parse_str(&props.note_id),
            try_consume_context::<LocalNoteRepo>(),
        ) {
            if let Ok(Some(project_id)) = repo.find_project_for_note(workflow_id) {
                if let Ok(rows) = repo.list_for_project(project_id) {
                    for row in rows {
                        out.insert(row.id, row.kind);
                    }
                }
            }
        }
        out
    };
    // Phase C: each note's phase ancestor id (or None if no phase
    // ancestor exists). Used by the edge filter to drop cross-phase
    // edges when the toolbar toggle is on. Built by walking parents
    // for every project note up to depth 32 — small enough to be a
    // single-pass map at render time.
    let note_to_phase: HashMap<Uuid, Option<Uuid>> = {
        let mut out: HashMap<Uuid, Option<Uuid>> = HashMap::new();
        if let (Ok(workflow_id), Some(LocalNoteRepo(repo))) = (
            Uuid::parse_str(&props.note_id),
            try_consume_context::<LocalNoteRepo>(),
        ) {
            if let Ok(Some(project_id)) = repo.find_project_for_note(workflow_id) {
                if let Ok(rows) = repo.list_for_project(project_id) {
                    let parent_by_id: HashMap<Uuid, Option<Uuid>> =
                        rows.iter().map(|n| (n.id, n.parent_id)).collect();
                    let kind_by_id: HashMap<Uuid, NoteKind> =
                        rows.iter().map(|n| (n.id, n.kind)).collect();
                    for row in &rows {
                        let mut cursor = row.parent_id;
                        let mut steps = 0;
                        let mut found: Option<Uuid> = None;
                        while let Some(id) = cursor {
                            if steps > 32 {
                                break;
                            }
                            steps += 1;
                            if matches!(kind_by_id.get(&id), Some(NoteKind::Phase)) {
                                found = Some(id);
                                break;
                            }
                            cursor = parent_by_id.get(&id).copied().flatten();
                        }
                        out.insert(row.id, found);
                    }
                }
            }
        }
        out
    };
    // Tabs / save-scheduler / persistence handles for the View button
    // — pulled here so the canvas-scope click handler doesn't need to
    // reach back through use_context (which would return detached
    // signals from inside an event closure). All three optional so
    // read-only `WorkflowView` (no edit infra) still compiles.
    let tabs_for_view: Option<Signal<TabManager>> =
        try_consume_context::<Signal<TabManager>>();
    let scheduler_for_view: Option<SaveScheduler> = try_consume_context::<SaveScheduler>();
    let persistence_for_view: Option<std::sync::Arc<dyn Persistence>> =
        try_consume_context::<std::sync::Arc<dyn Persistence>>();
    // For in-tile Approve / Reject / Mark dirty / Revise — these
    // mutate the artifact note's frontmatter via Persistence::save
    // and bump LocalNoteVersion so the explorer + workflow re-render.
    let note_repo_for_actions: Option<std::sync::Arc<dyn LocalNoteRepository>> =
        try_consume_context::<LocalNoteRepo>().map(|r| r.0);
    let note_version_for_actions: Option<Signal<u64>> =
        try_consume_context::<LocalNoteVersion>().map(|v| v.0);
    // Per-artifact frontmatter status, keyed by the artifact note's
    // id. Read once per render and re-read whenever `LocalNoteVersion`
    // bumps (we subscribe to it below) so the workflow card's
    // Approve / Reject / Mark dirty buttons mirror the artifact view's
    // state. Only populated for `is_artifact_snapshot` nodes with an
    // `artifact_ref`. One Persistence::load per artifact-snapshot per
    // render — fine for demo workflows; if a project ever scales past
    // a few hundred artifacts we'd switch to a use_memo with the
    // artifact body byte hashes as the dep.
    let artifact_statuses: HashMap<Uuid, ArtifactStatus> = {
        // Subscribe to LocalNoteVersion (and the global mirror) so a
        // status flip elsewhere immediately re-renders the canvas.
        if let Some(mut v) = note_version_for_actions {
            let _ = *v.read();
        }
        let _ = *crate::shell::companion_state::LOCAL_NOTE_VERSION.read();
        let mut out = HashMap::new();
        if let Some(pers) = persistence_for_view.as_ref() {
            // `g` here is the parsed-from-text graph used by the
            // canvas; iterate every snapshot's `artifact_ref`, load the
            // body, parse the frontmatter, take its `status`.
            for (_, node) in g.nodes.iter() {
                if !node.is_artifact_snapshot {
                    continue;
                }
                let Some(aref) = node.artifact_ref else { continue };
                let id_str = aref.to_string();
                #[cfg(not(target_arch = "wasm32"))]
                {
                    if let Ok(bytes) = futures::executor::block_on(pers.load(&id_str)) {
                        let body = String::from_utf8(bytes).unwrap_or_default();
                        let fm =
                            crate::plugins::artifact::frontmatter::parse(&body);
                        out.insert(aref, fm.status);
                    }
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let _ = id_str;
                }
            }
        }
        out
    };
    // Cascade plumbing for the workflow card's ▶ button on artifact-
    // snapshot tiles. Pulled here (not try_consume'd inside the
    // click closure — those writes go to detached signals) so each
    // ▶ click can spawn a depth-1 cascade rooted on the snapshot's
    // referenced artifact, advancing the SDLC pipeline one level.
    let project_repo_for_cascade: Option<std::sync::Arc<dyn LocalProjectRepository>> =
        try_consume_context::<LocalProjectRepo>().map(|r| r.0);
    let plugin_for_cascade: Option<std::sync::Arc<operon_plugins_claude_code::ClaudeCodeChatPlugin>> =
        try_consume_context::<ClaudeCodePluginCtx>().map(|c| c.0);
    let chat_session_repo_for_cascade: Option<
        std::sync::Arc<dyn operon_store::repos::ChatSessionRepository>,
    > = try_consume_context::<ChatSessionRepo>().map(|r| r.0);
    let chat_message_repo_for_cascade: Option<
        std::sync::Arc<dyn operon_store::repos::ChatMessageRepository>,
    > = try_consume_context::<ChatMessageRepo>().map(|r| r.0);
    #[cfg(not(target_arch = "wasm32"))]
    let vault_signal_for_cascade: Option<Signal<Option<crate::local_mode::vault::VaultRoot>>> =
        try_consume_context::<crate::local_mode::desktop::CurrentVaultRoot>().map(|c| c.0);
    let chat_session_version_for_cascade: Option<Signal<u64>> =
        try_consume_context::<ChatSessionVersion>().map(|v| v.0);
    let mut drag = use_signal::<Option<DragState>>(|| None);
    let mut edge_drag = use_signal::<Option<EdgeDragState>>(|| None);
    // Multi-select model. Plain click replaces the set, Ctrl/Cmd+click
    // toggles, Shift+click adds, Ctrl+A selects all, Esc clears, the
    // marquee (left-drag on empty canvas) sets it to nodes intersecting
    // the rubber-band rectangle. The inspector shows only when exactly
    // one node is selected.
    let mut selected_nodes =
        use_signal::<std::collections::BTreeSet<NodeId>>(std::collections::BTreeSet::new);
    let mut selected_edge = use_signal::<Option<EdgeId>>(|| None);
    // Marquee (rubber-band) area select. Started by left mousedown on
    // empty canvas (no modifier = replace selection on release; shift =
    // add to selection); coordinates are in SVG client space and
    // converted to world space when we test node intersection.
    let mut marquee = use_signal::<Option<MarqueeState>>(|| None);
    // Editor-scope signals lifted via props so the toolbar can also
    // poke them.
    let mut expanded = props.expanded;
    let persist_expanded = props.persist_expanded;
    // Hover highlight: when the cursor is over an edge, we boost the
    // edge's stroke and pulse-tint its source / target nodes so the
    // user can trace fan-out at a glance during demos.
    let mut hovered_edge = use_signal::<Option<EdgeId>>(|| None);
    // Extra-instructions popover state. `Some(node_id)` while the
    // popover is open. Single instance — clicking ✏ on a different
    // node replaces the open one. The popover renders an overlay
    // anchored to the target node with a textarea + OK / Clear /
    // Cancel buttons.
    let mut extra_instructions_open = use_signal::<Option<NodeId>>(|| None);
    // Locally-edited buffer for the popover textarea — kept in a
    // separate signal so Cancel can discard without touching the
    // graph and Clear can wipe the textarea without committing.
    let mut extra_instructions_draft = use_signal::<String>(String::new);
    // Phase-3: app-scope signal the explorer watches; setting it
    // marks the note as the current selection in the explorer panel.
    // Combined with the explorer-scope `FocusedNode` (next line) this
    // gives the inspector's "View" button a "reveal in explorer"
    // action — the note's row gets highlighted *and* scrolled into
    // view, without spawning a tab.
    let selected_note_app = try_consume_context::<SelectedNote>().map(|s| s.0);
    let focused_node_app = try_consume_context::<ExplorerFocused>().map(|f| f.0);
    // Viewport pan/zoom. The SVG itself uses CSS pixel coords (no
    // viewBox), and an inner <g> applies `translate(pan) scale(zoom)`
    // so node positions / edge math stay in "world" coords while the
    // visible window can pan and zoom. Wheel + ctrl/cmd zooms
    // centered on the cursor; plain wheel pans; middle-click drag
    // pans too.
    // Hydrate pan/zoom from the workflow note's persisted view state
    // on first mount. `use_signal` runs the init closure once, so
    // subsequent re-renders don't clobber user-driven pan / zoom that
    // hasn't yet been written back. Zoom of 0.0 (the serde default
    // for "view_state was absent") falls back to 1.0 = 100%.
    let initial_view_state = props.graph.view_state.clone();
    let mut pan_x = {
        let v = initial_view_state.pan_x;
        use_signal(move || v)
    };
    let mut pan_y = {
        let v = initial_view_state.pan_y;
        use_signal(move || v)
    };
    let zoom = {
        let v = if initial_view_state.zoom == 0.0 {
            1.0
        } else {
            initial_view_state.zoom
        };
        use_signal(move || v)
    };
    let mut pan_drag = use_signal::<Option<PanDragState>>(|| None);
    // Auto-place nodes whose `position` is exactly (0, 0) — keeps the
    // hand-edited "create a node by adding to JSON" flow usable without
    // having to do math. Stable id-sorted layout.
    let positions = layout(&g);
    // Visibility set = roots + every node reachable from a root through
    // a chain of *expanded* nodes. Roots are nodes with no incoming
    // edges (so they always show). A node renders only when its id is
    // in `visible_set`; edges render only when both endpoints are.
    let expanded_snap = expanded.read().clone();
    let visible_set = compute_visible(&g, &expanded_snap);
    // Track which nodes have at least one outgoing edge so the chevron
    // (▶ collapsed / ▼ expanded) only shows on actual parents.
    let has_children_set: std::collections::BTreeSet<NodeId> = g
        .edges
        .iter()
        .map(|e| e.from)
        .collect();
    // Count outgoing / incoming edges per node and assign each edge
    // its fan index in *both* directions. The render uses these to:
    //   1. Color fan-out edges by `outgoing_index` (palette of 8).
    //   2. Place edge anchor points along the vertical extent of the
    //      source's right border and the target's left border, so
    //      multiple edges don't bunch at a single mid-height point.
    // Anchor order on each side is by the *other endpoint's*
    // canonical kind priority + title — so an Epic's outputs
    // anchor in the order Story (top) → Plan (middle) → Backlog
    // (bottom), reading naturally from primary work to rollup.
    // Counts come from the *unfiltered* edge list so anchor positions
    // stay stable as siblings collapse / expand.
    let endpoint_key = |id: NodeId| -> (i32, String) {
        match g.nodes.get(&id) {
            Some(n) => (
                kind_priority(n.artifact_kind_label.as_deref()),
                n.artifact_title.clone().unwrap_or_default(),
            ),
            None => (i32::MAX, String::new()),
        }
    };
    let mut outgoing_index: HashMap<EdgeId, usize> = HashMap::new();
    let mut incoming_index: HashMap<EdgeId, usize> = HashMap::new();
    let mut outgoing_count: HashMap<NodeId, usize> = HashMap::new();
    let mut incoming_count: HashMap<NodeId, usize> = HashMap::new();
    {
        // Group by source, sort siblings by target's (kind, title),
        // then assign 0..N.
        let mut by_source: HashMap<NodeId, Vec<EdgeId>> = HashMap::new();
        let mut by_target: HashMap<NodeId, Vec<EdgeId>> = HashMap::new();
        for e in &g.edges {
            by_source.entry(e.from).or_default().push(e.id);
            by_target.entry(e.to).or_default().push(e.id);
        }
        let edge_target: HashMap<EdgeId, NodeId> =
            g.edges.iter().map(|e| (e.id, e.to)).collect();
        let edge_source: HashMap<EdgeId, NodeId> =
            g.edges.iter().map(|e| (e.id, e.from)).collect();
        for (src, mut ids) in by_source {
            ids.sort_by(|a, b| {
                let ta = edge_target.get(a).copied().unwrap_or(src);
                let tb = edge_target.get(b).copied().unwrap_or(src);
                endpoint_key(ta).cmp(&endpoint_key(tb)).then_with(|| a.cmp(b))
            });
            outgoing_count.insert(src, ids.len());
            for (i, eid) in ids.iter().enumerate() {
                outgoing_index.insert(*eid, i);
            }
        }
        for (tgt, mut ids) in by_target {
            ids.sort_by(|a, b| {
                let sa = edge_source.get(a).copied().unwrap_or(tgt);
                let sb = edge_source.get(b).copied().unwrap_or(tgt);
                endpoint_key(sa).cmp(&endpoint_key(sb)).then_with(|| a.cmp(b))
            });
            incoming_count.insert(tgt, ids.len());
            for (i, eid) in ids.iter().enumerate() {
                incoming_index.insert(*eid, i);
            }
        }
    }
    // Distributes anchor index `i` across `total` slots along a node's
    // vertical extent so edges land at different y-coords. With one
    // edge it picks the middle (0.5); with N edges it picks
    // `(i + 1) / (N + 1)` — evenly spaced, never touching the corners.
    let anchor_y = |i: usize, total: usize| -> f64 {
        let total = total.max(1) as f64;
        NODE_H * (i as f64 + 1.0) / (total + 1.0)
    };
    // Per-node lists of anchor y-offsets, one per incident edge. The
    // node renderer walks these to draw a small dot at each edge's
    // landing point on the border (visual cue for the user that
    // multiple edges connect cleanly to distinct points).
    let mut node_input_anchors: HashMap<NodeId, Vec<f64>> = HashMap::new();
    let mut node_output_anchors: HashMap<NodeId, Vec<f64>> = HashMap::new();
    for e in &g.edges {
        let s_idx = outgoing_index.get(&e.id).copied().unwrap_or(0);
        let s_total = outgoing_count.get(&e.from).copied().unwrap_or(1);
        let t_idx = incoming_index.get(&e.id).copied().unwrap_or(0);
        let t_total = incoming_count.get(&e.to).copied().unwrap_or(1);
        node_output_anchors
            .entry(e.from)
            .or_default()
            .push(anchor_y(s_idx, s_total));
        node_input_anchors
            .entry(e.to)
            .or_default()
            .push(anchor_y(t_idx, t_total));
    }
    let nodes: Vec<NodeRender> = g
        .nodes
        .iter()
        .filter(|(id, _)| visible_set.contains(id))
        .map(|(id, n)| NodeRender {
            id: *id,
            x: positions.get(id).map(|p| p.0).unwrap_or(n.position.0),
            y: positions.get(id).map(|p| p.1).unwrap_or(n.position.1),
            label: node_label(n, &skill_titles),
            status: n.status.clone(),
            is_artifact_snapshot: n.is_artifact_snapshot,
            kind_label: n.artifact_kind_label.clone(),
            has_children: has_children_set.contains(id),
            is_expanded: expanded_snap.contains(id),
        })
        .collect();
    let hide_cross_phase = *props.hide_cross_phase_edges.read();
    let edges: Vec<EdgeRender> = g
        .edges
        .iter()
        .filter_map(|e| {
            let from = nodes.iter().find(|n| n.id == e.from)?;
            let to = nodes.iter().find(|n| n.id == e.to)?;
            // Phase C cross-phase filter. Look up each endpoint's
            // phase ancestor via `note_to_phase` (keyed by the node's
            // `artifact_ref`). If both ends sit in different phases,
            // drop the edge when the toolbar toggle is on. Nodes
            // without an `artifact_ref` (skill nodes, plain Markdown
            // tiles) have no phase concept and are treated as
            // matching everything — same as legacy projects with no
            // phase notes at all.
            if hide_cross_phase {
                let from_phase = g
                    .nodes
                    .get(&e.from)
                    .and_then(|n| n.artifact_ref)
                    .and_then(|aref| note_to_phase.get(&aref).copied().flatten());
                let to_phase = g
                    .nodes
                    .get(&e.to)
                    .and_then(|n| n.artifact_ref)
                    .and_then(|aref| note_to_phase.get(&aref).copied().flatten());
                if let (Some(f), Some(t)) = (from_phase, to_phase) {
                    if f != t {
                        return None;
                    }
                }
            }
            let s_idx = outgoing_index.get(&e.id).copied().unwrap_or(0);
            let s_total = outgoing_count.get(&e.from).copied().unwrap_or(1);
            let t_idx = incoming_index.get(&e.id).copied().unwrap_or(0);
            let t_total = incoming_count.get(&e.to).copied().unwrap_or(1);
            Some(EdgeRender {
                id: e.id,
                from_id: e.from,
                to_id: e.to,
                from_x: from.x + NODE_W,
                from_y: from.y + anchor_y(s_idx, s_total),
                to_x: to.x,
                to_y: to.y + anchor_y(t_idx, t_total),
                edge_kind: e.edge_kind.clone(),
                fan_index: s_idx,
            })
        })
        .collect();

    // Resolve the currently-hovered edge to its endpoint nodes so the
    // node loop below can apply the connecting-node hover class.
    let hover_endpoints: std::collections::BTreeSet<NodeId> =
        match *hovered_edge.read() {
            Some(eid) => edges
                .iter()
                .find(|e| e.id == eid)
                .map(|e| {
                    let mut s = std::collections::BTreeSet::new();
                    s.insert(e.from_id);
                    s.insert(e.to_id);
                    s
                })
                .unwrap_or_default(),
            None => std::collections::BTreeSet::new(),
        };

    // No viewBox: the SVG draws in CSS pixel coords, and an inner
    // <g> applies pan/zoom. This gives an effectively infinite
    // canvas (panning extends arbitrarily) without auto-fit
    // surprises when nodes are added.
    let pan_xv = *pan_x.read();
    let pan_yv = *pan_y.read();
    let zoomv = *zoom.read();
    let world_transform = format!("translate({pan_xv} {pan_yv}) scale({zoomv})");

    // Inspector readout for a single-selected node — extra_instructions
    // live edits and a skill-id readout (typed_fields edit lives in the
    // JSON pane to keep schema-validation consistent). Multi-selection
    // hides the inspector to avoid the "edit which one?" ambiguity.
    let selection_snap = selected_nodes.read().clone();
    let selected_count = selection_snap.len();
    let single_selected: Option<NodeId> = if selected_count == 1 {
        selection_snap.iter().next().copied()
    } else {
        None
    };
    let selected_node_view = single_selected.and_then(|sid| g.nodes.get(&sid).cloned());

    // Per-node live state snapshot. Subscribing here (rather than
    // per-tile) re-renders the whole canvas when ANY node's live
    // state changes — fine because publishes are bounded by
    // tool-call / thinking-block frequency, not streaming-text
    // frequency (executor.rs intentionally omits Text from the
    // publish path).
    let node_live_snap: HashMap<Uuid, NodeLiveState> =
        NODE_LIVE_STATE.read().clone();

    let persist_view = props.persist_view;
    let reset_view = {
        let mut pan_x_setter = pan_x;
        let mut pan_y_setter = pan_y;
        let mut zoom_setter = zoom;
        move |_| {
            pan_x_setter.set(0.0);
            pan_y_setter.set(0.0);
            zoom_setter.set(1.0);
            persist_view.call((0.0, 0.0, 1.0));
        }
    };
    // Explicit zoom buttons — the wheel/ctrl-wheel pan/zoom path is
    // there for power users; these surface the same affordance for
    // anyone on a trackpad / touchscreen who can't easily produce
    // ctrl+wheel. Each button steps zoom by 1.2× (in) or 1/1.2 (out)
    // and clamps to the same bounds the wheel handler uses.
    let zoom_in = {
        let mut zoom_setter = zoom;
        move |_| {
            let cur = *zoom_setter.read();
            let next = (cur * 1.2).clamp(0.2, 5.0);
            zoom_setter.set(next);
            persist_view.call((*pan_x.peek(), *pan_y.peek(), next));
        }
    };
    let zoom_out = {
        let mut zoom_setter = zoom;
        move |_| {
            let cur = *zoom_setter.read();
            let next = (cur / 1.2).clamp(0.2, 5.0);
            zoom_setter.set(next);
            persist_view.call((*pan_x.peek(), *pan_y.peek(), next));
        }
    };

    // Cascade-paused banner: visible whenever this workflow's root
    // artifact is in `CascadePhase::Paused` (set when a `cascade_stop`
    // skill produces a checkpoint artifact). Without this surface,
    // a paused cascade looks identical to a hung run — the user
    // wouldn't know that re-clicking ▶ Play after approving the
    // checkpoint is what continues the cascade.
    let cascade_pause_banner: Option<(Uuid, String)> = (|| {
        let root_id = find_cascade_root_artifact(&g)?;
        // Subscribe to CASCADE_STATE so the banner appears/clears
        // reactively as phases transition.
        let phase = crate::shell::companion_state::CASCADE_STATE
            .read()
            .get(&root_id)
            .cloned()?;
        if let crate::shell::companion_state::CascadePhase::Paused {
            artifact_id, ..
        } = phase
        {
            let title = g
                .nodes
                .values()
                .find(|n| n.artifact_ref == Some(artifact_id))
                .and_then(|n| n.artifact_title.clone())
                .unwrap_or_else(|| artifact_id.to_string());
            Some((artifact_id, title))
        } else {
            None
        }
    })();

    rsx! {
        if let Some((_pause_artifact_id, pause_title)) = cascade_pause_banner.clone() {
            div {
                class: "operon-cascade-pause-banner",
                "data-testid": "cascade-pause-banner",
                span { class: "operon-cascade-pause-icon", "⏸" }
                span { class: "operon-cascade-pause-text",
                    "Cascade paused at checkpoint: "
                    strong { "{pause_title}" }
                    ". Approve the artifact and click ▶ Play on Requirements to continue."
                }
            }
        }
        div { class: "operon-workflow-canvas",
            "data-testid": "workflow-canvas",
            // tabindex=0 + onkeydown — keyboard shortcuts (Ctrl+A, Del,
            // Ctrl+C/X/V, Ctrl+Z/Y, Esc) only fire when the canvas has
            // focus. Browsers auto-focus the nearest tabindex=0
            // ancestor when the user clicks anywhere inside, so this
            // works without extra JS plumbing.
            tabindex: "0",
            onkeydown: {
                let g_for_keys = g.clone();
                let nodes_for_keys = nodes.clone();
                let apply_graph = props.apply_graph;
                let apply_with_undo = props.apply_with_undo;
                let mut history = props.history;
                let mut redo_stack = props.redo_stack;
                let mut clipboard = props.clipboard;
                move |evt: dioxus::events::KeyboardEvent| {
                    let key = evt.key().to_string();
                    let mods = evt.modifiers();
                    let ctrl = mods.contains(Modifiers::CONTROL)
                        || mods.contains(Modifiers::META);
                    let shift = mods.contains(Modifiers::SHIFT);

                    // Esc — clear selection.
                    if key == "Escape" {
                        evt.prevent_default();
                        selected_nodes.set(std::collections::BTreeSet::new());
                        selected_edge.set(None);
                        return;
                    }

                    // Ctrl/Cmd+A — select every node.
                    if ctrl && !shift && key.eq_ignore_ascii_case("a") {
                        evt.prevent_default();
                        let mut all = std::collections::BTreeSet::new();
                        for n in &nodes_for_keys {
                            all.insert(n.id);
                        }
                        selected_nodes.set(all);
                        return;
                    }

                    // Delete / Backspace — remove every selected node
                    // (and any edge incident to one). Plain key, no
                    // modifiers — Ctrl+Backspace on inputs is the
                    // "delete word" shortcut, so we deliberately don't
                    // claim it.
                    if (key == "Delete" || key == "Backspace") && !ctrl && !shift {
                        let snap = selected_nodes.read().clone();
                        if !snap.is_empty() {
                            evt.prevent_default();
                            let mut next = g_for_keys.clone();
                            for id in &snap {
                                next.nodes.remove(id);
                            }
                            next.edges
                                .retain(|e| !snap.contains(&e.from) && !snap.contains(&e.to));
                            next.version = next.version.saturating_add(1);
                            apply_with_undo.call(next);
                            selected_nodes
                                .set(std::collections::BTreeSet::new());
                        }
                        return;
                    }

                    // Ctrl+C — copy selected nodes + their internal
                    // edges (edges that cross the selection boundary
                    // are dropped on copy; paste won't reconnect to
                    // outside nodes).
                    if ctrl && !shift && key.eq_ignore_ascii_case("c") {
                        let snap = selected_nodes.read().clone();
                        if !snap.is_empty() {
                            evt.prevent_default();
                            let nodes_v: Vec<Node> = snap
                                .iter()
                                .filter_map(|id| g_for_keys.nodes.get(id).cloned())
                                .collect();
                            let edges_v: Vec<Edge> = g_for_keys
                                .edges
                                .iter()
                                .filter(|e| {
                                    snap.contains(&e.from) && snap.contains(&e.to)
                                })
                                .cloned()
                                .collect();
                            clipboard.set(Some(WorkflowClipboard {
                                nodes: nodes_v,
                                edges: edges_v,
                            }));
                        }
                        return;
                    }

                    // Ctrl+X — cut = copy + delete in one step.
                    if ctrl && !shift && key.eq_ignore_ascii_case("x") {
                        let snap = selected_nodes.read().clone();
                        if !snap.is_empty() {
                            evt.prevent_default();
                            let nodes_v: Vec<Node> = snap
                                .iter()
                                .filter_map(|id| g_for_keys.nodes.get(id).cloned())
                                .collect();
                            let edges_v: Vec<Edge> = g_for_keys
                                .edges
                                .iter()
                                .filter(|e| {
                                    snap.contains(&e.from) && snap.contains(&e.to)
                                })
                                .cloned()
                                .collect();
                            clipboard.set(Some(WorkflowClipboard {
                                nodes: nodes_v,
                                edges: edges_v,
                            }));
                            let mut next = g_for_keys.clone();
                            for id in &snap {
                                next.nodes.remove(id);
                            }
                            next.edges
                                .retain(|e| !snap.contains(&e.from) && !snap.contains(&e.to));
                            next.version = next.version.saturating_add(1);
                            apply_with_undo.call(next);
                            selected_nodes
                                .set(std::collections::BTreeSet::new());
                        }
                        return;
                    }

                    // Ctrl+V — paste. Mints fresh UUIDs for every node
                    // and edge so a paste-into-the-same-graph doesn't
                    // collide with the originals; offsets positions by
                    // PASTE_OFFSET so the pasted group lands visibly
                    // adjacent rather than directly on top.
                    if ctrl && !shift && key.eq_ignore_ascii_case("v") {
                        let cb = clipboard.read().clone();
                        if let Some(cb) = cb {
                            if !cb.nodes.is_empty() {
                                evt.prevent_default();
                                const PASTE_OFFSET: f64 = 40.0;
                                let mut next = g_for_keys.clone();
                                let mut id_map: HashMap<NodeId, NodeId> =
                                    HashMap::new();
                                let mut new_selection =
                                    std::collections::BTreeSet::new();
                                for n in &cb.nodes {
                                    let new_id = Uuid::new_v4();
                                    id_map.insert(n.id, new_id);
                                    let mut clone = n.clone();
                                    clone.id = new_id;
                                    clone.position = (
                                        n.position.0 + PASTE_OFFSET,
                                        n.position.1 + PASTE_OFFSET,
                                    );
                                    next.nodes.insert(new_id, clone);
                                    new_selection.insert(new_id);
                                }
                                for e in &cb.edges {
                                    if let (Some(&new_from), Some(&new_to)) =
                                        (id_map.get(&e.from), id_map.get(&e.to))
                                    {
                                        let mut clone = e.clone();
                                        clone.id = Uuid::new_v4();
                                        clone.from = new_from;
                                        clone.to = new_to;
                                        next.edges.push(clone);
                                    }
                                }
                                next.version = next.version.saturating_add(1);
                                apply_with_undo.call(next);
                                selected_nodes.set(new_selection);
                            }
                        }
                        return;
                    }

                    // Ctrl+Z (undo) and Ctrl+Shift+Z / Ctrl+Y (redo).
                    // We bypass `apply_with_undo` here and call the raw
                    // `apply_graph` because we're managing the history
                    // stacks manually (would otherwise loop).
                    if ctrl && key.eq_ignore_ascii_case("z") {
                        evt.prevent_default();
                        if shift {
                            let next = redo_stack.write().pop();
                            if let Some(n) = next {
                                history.with_mut(|h| {
                                    h.push(g_for_keys.clone());
                                    if h.len() > HISTORY_CAP {
                                        h.remove(0);
                                    }
                                });
                                apply_graph.call(n);
                            }
                        } else {
                            let prev = history.write().pop();
                            if let Some(p) = prev {
                                redo_stack
                                    .with_mut(|r| r.push(g_for_keys.clone()));
                                apply_graph.call(p);
                            }
                        }
                        return;
                    }
                    if ctrl && !shift && key.eq_ignore_ascii_case("y") {
                        evt.prevent_default();
                        let next = redo_stack.write().pop();
                        if let Some(n) = next {
                            history.with_mut(|h| {
                                h.push(g_for_keys.clone());
                                if h.len() > HISTORY_CAP {
                                    h.remove(0);
                                }
                            });
                            apply_graph.call(n);
                        }
                    }
                }
            },
            // Floating viewport controls — explicit zoom in / zoom
            // out / reset. Wheel + ctrl+wheel still work for power
            // users; these buttons make the same affordance reachable
            // on trackpads / touchscreens / no-modifier inputs.
            div { class: "operon-workflow-viewport-controls",
                "data-testid": "workflow-viewport-controls",
                button {
                    r#type: "button",
                    class: "operon-workflow-viewport-button",
                    "data-testid": "workflow-zoom-out",
                    title: "Zoom out (or scroll with Ctrl/Cmd)",
                    onclick: zoom_out,
                    "\u{2212}"
                }
                button {
                    r#type: "button",
                    class: "operon-workflow-viewport-button",
                    "data-testid": "workflow-zoom-in",
                    title: "Zoom in (or scroll with Ctrl/Cmd)",
                    onclick: zoom_in,
                    "+"
                }
                button {
                    r#type: "button",
                    class: "operon-workflow-viewport-button",
                    "data-testid": "workflow-reset-view",
                    title: "Reset pan/zoom (press to recenter)",
                    onclick: reset_view,
                    "Reset"
                }
            }
            // Extra-instructions popover. Rendered above the SVG when
            // a card's ✏ button is clicked; one instance at a time.
            // Pure HTML overlay (positioned absolute) so the textarea
            // gets native typing affordance without SVG foreignObject
            // gymnastics. Anchored to the viewport — pan/zoom doesn't
            // chase the popover; the user sees a stable dialog.
            if let Some(target_id) = *extra_instructions_open.read() {
                {
                    let apply_extra = props.apply_with_undo;
                    let g_for_extra_apply = g.clone();
                    let on_ok = move |_| {
                        let mut next = g_for_extra_apply.clone();
                        let new_value = extra_instructions_draft.peek().clone();
                        if let Some(node) = next.nodes.get_mut(&target_id) {
                            node.extra_instructions = new_value;
                        }
                        next.version = next.version.saturating_add(1);
                        apply_extra.call(next);
                        extra_instructions_open.set(None);
                    };
                    let on_clear = move |_| {
                        extra_instructions_draft.set(String::new());
                    };
                    let on_cancel = move |_| {
                        extra_instructions_open.set(None);
                    };
                    let cur_value = extra_instructions_draft.read().clone();
                    rsx! {
                        div {
                            class: "operon-workflow-extra-popover-scrim",
                            "data-testid": "workflow-extra-popover-scrim",
                            onclick: move |_| {
                                // Click outside closes without saving.
                                extra_instructions_open.set(None);
                            },
                            onkeydown: move |evt: dioxus::events::KeyboardEvent| {
                                if evt.key().to_string() == "Escape" {
                                    extra_instructions_open.set(None);
                                }
                            },
                            div {
                                class: "operon-workflow-extra-popover",
                                "data-testid": "workflow-extra-popover",
                                onclick: move |evt: dioxus::events::MouseEvent| {
                                    evt.stop_propagation();
                                },
                                div { class: "operon-workflow-extra-popover-title",
                                    "Extra instructions"
                                }
                                textarea {
                                    class: "operon-workflow-extra-popover-textarea",
                                    "data-testid": "workflow-extra-popover-textarea",
                                    autofocus: true,
                                    value: "{cur_value}",
                                    oninput: move |e| {
                                        extra_instructions_draft.set(e.value());
                                    },
                                }
                                div { class: "operon-workflow-extra-popover-actions",
                                    button {
                                        r#type: "button",
                                        class: "operon-workflow-extra-popover-button operon-workflow-extra-popover-button-ok",
                                        "data-testid": "workflow-extra-popover-ok",
                                        onclick: on_ok,
                                        "OK"
                                    }
                                    button {
                                        r#type: "button",
                                        class: "operon-workflow-extra-popover-button",
                                        "data-testid": "workflow-extra-popover-clear",
                                        onclick: on_clear,
                                        "Clear"
                                    }
                                    button {
                                        r#type: "button",
                                        class: "operon-workflow-extra-popover-button",
                                        "data-testid": "workflow-extra-popover-cancel",
                                        onclick: on_cancel,
                                        "Cancel"
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if nodes.is_empty() {
                div { class: "operon-workflow-canvas-empty",
                    "data-testid": "workflow-canvas-empty",
                    "Empty workflow \u{2014} click \u{201C}Add skill node\u{201D} or paste JSON to begin."
                }
            } else {
                svg {
                    class: "operon-workflow-svg",
                    width: "100%",
                    height: "100%",
                    // No viewBox: SVG draws in CSS pixel coords; the
                    // inner <g> handles pan + zoom.
                    onwheel: {
                        let mut pan_x_setter = pan_x;
                        let mut pan_y_setter = pan_y;
                        let mut zoom_setter = zoom;
                        move |evt: dioxus::events::WheelEvent| {
                            evt.prevent_default();
                            evt.stop_propagation();
                            let delta = evt.delta().strip_units();
                            let mods = evt.modifiers();
                            let zoom_intent = mods.contains(Modifiers::CONTROL)
                                || mods.contains(Modifiers::META);
                            if zoom_intent {
                                // Zoom centered on cursor. Convert
                                // cursor (element coords ≈ SVG-local
                                // CSS px) to world space, change the
                                // zoom, then adjust pan so the same
                                // world point stays under the cursor.
                                let coords = evt.element_coordinates();
                                let cur_zoom = *zoom_setter.read();
                                let cur_pan_x = *pan_x_setter.read();
                                let cur_pan_y = *pan_y_setter.read();
                                let world_x = (coords.x - cur_pan_x) / cur_zoom;
                                let world_y = (coords.y - cur_pan_y) / cur_zoom;
                                // Pinch / wheel: positive delta_y = zoom out.
                                let factor = if delta.y < 0.0 { 1.1 } else { 1.0 / 1.1 };
                                let new_zoom = (cur_zoom * factor).clamp(0.2, 5.0);
                                let new_pan_x = coords.x - world_x * new_zoom;
                                let new_pan_y = coords.y - world_y * new_zoom;
                                pan_x_setter.set(new_pan_x);
                                pan_y_setter.set(new_pan_y);
                                zoom_setter.set(new_zoom);
                                persist_view.call((new_pan_x, new_pan_y, new_zoom));
                            } else {
                                // Plain wheel = pan in screen space.
                                pan_x_setter.with_mut(|v| *v -= delta.x);
                                pan_y_setter.with_mut(|v| *v -= delta.y);
                                persist_view.call((
                                    *pan_x_setter.peek(),
                                    *pan_y_setter.peek(),
                                    *zoom_setter.peek(),
                                ));
                            }
                        }
                    },
                    onmousedown: move |evt: dioxus::events::MouseEvent| {
                        // Middle-click drag = pan. Caught at the SVG
                        // level so nodes / handles aren't involved.
                        if evt.trigger_button() == Some(dioxus::html::input_data::MouseButton::Auxiliary) {
                            evt.prevent_default();
                            let coords = evt.client_coordinates();
                            pan_drag.set(Some(PanDragState {
                                start_client_x: coords.x,
                                start_client_y: coords.y,
                                start_pan_x: *pan_x.read(),
                                start_pan_y: *pan_y.read(),
                            }));
                            return;
                        }
                        // Left-click on empty SVG: start marquee (rubber
                        // band) selection. Node mousedowns stop_propagation
                        // so this fires only on empty canvas. Shift = add
                        // to current selection on release; no modifier =
                        // replace.
                        if evt.trigger_button()
                            == Some(dioxus::html::input_data::MouseButton::Primary)
                        {
                            let mods = evt.modifiers();
                            let elem = evt.element_coordinates();
                            let client = evt.client_coordinates();
                            let cx = *pan_x.read();
                            let cy = *pan_y.read();
                            let cz = *zoom.read();
                            let wx = (elem.x - cx) / cz;
                            let wy = (elem.y - cy) / cz;
                            marquee.set(Some(MarqueeState {
                                start_x: wx,
                                start_y: wy,
                                cur_x: wx,
                                cur_y: wy,
                                start_client_x: client.x,
                                start_client_y: client.y,
                                additive: mods.contains(Modifiers::SHIFT),
                            }));
                        }
                    },
                    // Track drags at the SVG level so the cursor
                    // doesn't have to stay perfectly inside the node
                    // rect during a fast drag.
                    onmousemove: {
                        let apply = props.apply_graph;
                        let mut graph = g.clone();
                        move |e: dioxus::events::MouseEvent| {
                            let coords = e.client_coordinates();
                            // Pan drag has highest priority — it's a
                            // global gesture and shouldn't share with
                            // node / edge drags.
                            if let Some(cur) = *pan_drag.read() {
                                let dx = coords.x - cur.start_client_x;
                                let dy = coords.y - cur.start_client_y;
                                pan_x.set(cur.start_pan_x + dx);
                                pan_y.set(cur.start_pan_y + dy);
                                return;
                            }
                            let cur_zoom = *zoom.read();
                            // Marquee drag: update the rubber-band's
                            // current corner in world coords.
                            let marquee_now = *marquee.read();
                            if let Some(cur) = marquee_now {
                                let dx = (coords.x - cur.start_client_x) / cur_zoom;
                                let dy = (coords.y - cur.start_client_y) / cur_zoom;
                                marquee.set(Some(MarqueeState {
                                    cur_x: cur.start_x + dx,
                                    cur_y: cur.start_y + dy,
                                    ..cur
                                }));
                                return;
                            }
                            // Node-position drag: mutate graph + push.
                            // Divide the client delta by zoom because
                            // node positions are in world coords and
                            // the inner <g> scales them by `zoom` for
                            // display.
                            let drag_now = drag.read().clone();
                            if let Some(cur) = drag_now {
                                let dx = (coords.x - cur.start_client_x) / cur_zoom;
                                let dy = (coords.y - cur.start_client_y) / cur_zoom;
                                // Translate every node in the captured
                                // start_positions set so a multi-selection
                                // moves as a group. Single-node grabs only
                                // have one entry, so this collapses to
                                // the original behavior.
                                for (id, sx, sy) in &cur.start_positions {
                                    if let Some(node) = graph.nodes.get_mut(id) {
                                        node.position = (sx + dx, sy + dy);
                                    }
                                }
                                apply.call(graph.clone());
                                return;
                            }
                            // Edge-creation drag: only update ghost
                            // cursor (no graph mutation until commit).
                            // Same zoom-scaling so the ghost line
                            // tracks the cursor in world space.
                            let edge_cur = *edge_drag.read();
                            if let Some(cur) = edge_cur {
                                let dx = (coords.x - cur.start_client_x) / cur_zoom;
                                let dy = (coords.y - cur.start_client_y) / cur_zoom;
                                edge_drag.set(Some(EdgeDragState {
                                    cursor_x: cur.from_x + dx,
                                    cursor_y: cur.from_y + dy,
                                    ..cur
                                }));
                            }
                        }
                    },
                    onmouseup: {
                        // Proximity-snap on release: r=6 input handles
                        // are tiny targets, so most users will release
                        // *near* but not exactly on one. If an
                        // edge_drag is active and the cursor's last
                        // tracked position is within SNAP_TOL viewBox
                        // units of any other node's input handle,
                        // commit the edge to that nearest node.
                        let apply = props.apply_graph;
                        let nodes_snap = nodes.clone();
                        let graph_snap = g.clone();
                        let nodes_for_marquee = nodes.clone();
                        move |_| {
                            // Persist viewport when a pan-drag ends —
                            // doing it here (not on every mousemove)
                            // keeps the JSON write at one save per
                            // gesture instead of one per pixel.
                            let was_panning = pan_drag.peek().is_some();
                            drag.set(None);
                            pan_drag.set(None);
                            if was_panning {
                                persist_view.call((
                                    *pan_x.peek(),
                                    *pan_y.peek(),
                                    *zoom.peek(),
                                ));
                            }
                            // Marquee commit: compute hit set, write to
                            // selected_nodes. A near-zero-size marquee is
                            // treated as a click on empty canvas → clear
                            // selection.
                            let final_marquee = *marquee.read();
                            marquee.set(None);
                            if let Some(m) = final_marquee {
                                let x0 = m.start_x.min(m.cur_x);
                                let y0 = m.start_y.min(m.cur_y);
                                let x1 = m.start_x.max(m.cur_x);
                                let y1 = m.start_y.max(m.cur_y);
                                const CLICK_TOL: f64 = 4.0;
                                if (x1 - x0) < CLICK_TOL && (y1 - y0) < CLICK_TOL {
                                    if !m.additive {
                                        selected_nodes
                                            .set(std::collections::BTreeSet::new());
                                        selected_edge.set(None);
                                    }
                                    return;
                                }
                                let mut hit = std::collections::BTreeSet::new();
                                for n in nodes_for_marquee.iter() {
                                    let nx0 = n.x;
                                    let ny0 = n.y;
                                    let nx1 = n.x + NODE_W;
                                    let ny1 = n.y + NODE_H;
                                    if nx0 < x1 && nx1 > x0 && ny0 < y1 && ny1 > y0 {
                                        hit.insert(n.id);
                                    }
                                }
                                if m.additive {
                                    let mut existing = selected_nodes.read().clone();
                                    existing.extend(hit);
                                    selected_nodes.set(existing);
                                } else {
                                    selected_nodes.set(hit);
                                }
                                selected_edge.set(None);
                                return;
                            }
                            let cur = match *edge_drag.read() {
                                Some(d) => d,
                                None => return,
                            };
                            edge_drag.set(None);
                            const SNAP_TOL: f64 = 30.0;
                            let mut best: Option<(NodeId, f64)> = None;
                            for n in nodes_snap.iter() {
                                if n.id == cur.from {
                                    continue;
                                }
                                let hx = n.x;
                                let hy = n.y + NODE_H / 2.0;
                                let dxh = hx - cur.cursor_x;
                                let dyh = hy - cur.cursor_y;
                                let dist2 = dxh * dxh + dyh * dyh;
                                if dist2 <= SNAP_TOL * SNAP_TOL
                                    && best.map_or(true, |(_, bd)| dist2 < bd)
                                {
                                    best = Some((n.id, dist2));
                                }
                            }
                            eprintln!(
                                "operon: edge-drag mouseup cursor=({:.0},{:.0}) best={:?}",
                                cur.cursor_x, cur.cursor_y, best
                            );
                            if let Some((target, _)) = best {
                                if let Some(g_next) =
                                    add_edge_if_new(&graph_snap, cur.from, target)
                                {
                                    apply.call(g_next);
                                    // Auto-expand the upstream so the
                                    // newly-wired downstream stays
                                    // visible. Without this, wiring
                                    // tile→01 makes 01 disappear
                                    // because the tile becomes the
                                    // new indegree-0 root and isn't
                                    // in `expanded` yet.
                                    expanded.with_mut(|s| {
                                        s.insert(cur.from);
                                    });
                                    let snap = expanded.read().clone();
                                    persist_expanded.call(snap);
                                }
                            }
                        }
                    },
                    onmouseleave: move |_| {
                        drag.set(None);
                        edge_drag.set(None);
                        pan_drag.set(None);
                    },
                    defs {
                        marker {
                            id: "operon-workflow-arrow",
                            "viewBox": "0 0 10 10",
                            "refX": "9",
                            "refY": "5",
                            "markerWidth": "8",
                            "markerHeight": "8",
                            orient: "auto-start-reverse",
                            path { d: "M0,0 L10,5 L0,10 z", fill: "currentColor" }
                        }
                    }
                    // World-space group: pan + zoom apply here so all
                    // node / edge math stays in unscaled coords.
                    g {
                        class: "operon-workflow-world",
                        transform: "{world_transform}",
                    for e in edges.iter() {
                        {
                            let edge_id = e.id;
                            let is_selected_edge = *selected_edge.read() == Some(edge_id);
                            let is_hovered_edge = *hovered_edge.read() == Some(edge_id);
                            // Cascade-visualization: amber stroke for
                            // sibling Depends-on cross-edges; default
                            // black for skill-DAG and parent-child
                            // edges.
                            let kind_suffix = match e.edge_kind.as_deref() {
                                Some("depends_on") => " operon-workflow-edge-depends-on",
                                Some("parent_child") => " operon-workflow-edge-parent-child",
                                _ => "",
                            };
                            // Multi-color fan-out — every edge from the
                            // same source picks a palette slot via
                            // `fan_index % FAN_COLORS`. Lets the user
                            // trace which downstream came from which
                            // upstream when one node fans out to many.
                            let fan_class =
                                format!(" operon-workflow-edge-fan-{}", e.fan_index % 8);
                            let hover_class = if is_hovered_edge {
                                " operon-workflow-edge-hovered"
                            } else {
                                ""
                            };
                            let edge_class = if is_selected_edge {
                                format!("operon-workflow-edge operon-workflow-edge-selected{kind_suffix}{fan_class}{hover_class}")
                            } else {
                                format!("operon-workflow-edge{kind_suffix}{fan_class}{hover_class}")
                            };
                            // Wider invisible hit-area path layered
                            // beneath the visible stroke — cubic
                            // beziers are 2-3 px wide visually but
                            // need a fatter target so a direct click
                            // selects reliably.
                            rsx! {
                                path {
                                    class: "operon-workflow-edge-hit",
                                    "data-testid": "workflow-edge-hit",
                                    "data-edge-id": "{edge_id}",
                                    d: "M {e.from_x} {e.from_y} C {e.from_x + 60.0} {e.from_y}, {e.to_x - 60.0} {e.to_y}, {e.to_x} {e.to_y}",
                                    onmousedown: move |evt: dioxus::events::MouseEvent| {
                                        evt.stop_propagation();
                                    },
                                    onclick: move |evt: dioxus::events::MouseEvent| {
                                        evt.stop_propagation();
                                        selected_edge.set(Some(edge_id));
                                        selected_nodes.set(std::collections::BTreeSet::new());
                                    },
                                    // Hover targets the wider hit-area
                                    // path so the highlight kicks in
                                    // even when the cursor is near the
                                    // visible stroke but not exactly on
                                    // it.
                                    onmouseenter: move |_| {
                                        hovered_edge.set(Some(edge_id));
                                    },
                                    onmouseleave: move |_| {
                                        if *hovered_edge.peek() == Some(edge_id) {
                                            hovered_edge.set(None);
                                        }
                                    },
                                }
                                path {
                                    class: "{edge_class}",
                                    "data-testid": "workflow-edge",
                                    "data-edge-id": "{edge_id}",
                                    "data-selected": if is_selected_edge { "true" } else { "false" },
                                    d: "M {e.from_x} {e.from_y} C {e.from_x + 60.0} {e.from_y}, {e.to_x - 60.0} {e.to_y}, {e.to_x} {e.to_y}",
                                    "marker-end": "url(#operon-workflow-arrow)",
                                }
                            }
                        }
                    }
                    for n in nodes.iter() {
                        {
                            let node_id = n.id;
                            let n_x = n.x;
                            let n_y = n.y;
                            let note_id_for_run = props.note_id.clone();
                            let apply_for_run = props.apply_graph;
                            let graph_for_run = g.clone();
                            // Snapshot artifact-snapshot status + ref so
                            // the click handler can route to the cascade
                            // play (one level) for artifact tiles, or to
                            // the legacy per-skill `spawn_run_node` for
                            // hand-built skill DAG nodes.
                            let is_snapshot_for_run = n.is_artifact_snapshot;
                            let artifact_ref_for_run =
                                g.nodes.get(&node_id).and_then(|gn| gn.artifact_ref);
                            let cascade_note_repo = note_repo_for_actions.clone();
                            let cascade_project_repo = project_repo_for_cascade.clone();
                            let cascade_persistence = persistence_for_view.clone();
                            let cascade_plugin = plugin_for_cascade.clone();
                            let cascade_session_repo =
                                chat_session_repo_for_cascade.clone();
                            let cascade_message_repo =
                                chat_message_repo_for_cascade.clone();
                            #[cfg(not(target_arch = "wasm32"))]
                            let cascade_vault_signal = vault_signal_for_cascade;
                            let mut cascade_note_version = note_version_for_actions;
                            let mut cascade_session_version =
                                chat_session_version_for_cascade;
                            let on_run_node = move |evt: dioxus::events::MouseEvent| {
                                evt.stop_propagation();
                                // Artifact-snapshot tiles → one-level
                                // cascade rooted on the underlying
                                // artifact note. Each click advances the
                                // SDLC pipeline by exactly one step
                                // (Requirements → Epics, then click on
                                // Epic → its Features, etc.).
                                if is_snapshot_for_run {
                                    let Some(aref) = artifact_ref_for_run else { return };
                                    let Some(note_repo) = cascade_note_repo.clone() else {
                                        eprintln!(
                                            "operon: cascade-play BAIL — LocalNoteRepo missing"
                                        );
                                        return;
                                    };
                                    let Some(project_repo) = cascade_project_repo.clone()
                                    else {
                                        eprintln!(
                                            "operon: cascade-play BAIL — LocalProjectRepo missing"
                                        );
                                        return;
                                    };
                                    let Some(persistence) = cascade_persistence.clone()
                                    else {
                                        eprintln!(
                                            "operon: cascade-play BAIL — Persistence missing"
                                        );
                                        return;
                                    };
                                    let Some(plugin) = cascade_plugin.clone() else {
                                        eprintln!(
                                            "operon: cascade-play BAIL — ClaudeCode plugin missing"
                                        );
                                        return;
                                    };
                                    let Some(session_repo) = cascade_session_repo.clone()
                                    else {
                                        eprintln!(
                                            "operon: cascade-play BAIL — ChatSessionRepo missing"
                                        );
                                        return;
                                    };
                                    let Some(message_repo) = cascade_message_repo.clone()
                                    else {
                                        eprintln!(
                                            "operon: cascade-play BAIL — ChatMessageRepo missing"
                                        );
                                        return;
                                    };
                                    let (
                                        Some(mut active_session),
                                        Some(mut active_scope),
                                        Some(mut note_version),
                                        Some(mut session_version),
                                    ) = (
                                        active_session_signal,
                                        active_scope_signal,
                                        cascade_note_version,
                                        cascade_session_version,
                                    ) else {
                                        eprintln!(
                                            "operon: cascade-play BAIL — rail signals missing"
                                        );
                                        return;
                                    };
                                    #[cfg(not(target_arch = "wasm32"))]
                                    let cascade_vault_snapshot = cascade_vault_signal
                                        .as_ref()
                                        .and_then(|s| s.read().clone());
                                    crate::plugins::artifact::view::spawn_cascade(
                                        aref,
                                        note_repo,
                                        project_repo,
                                        persistence,
                                        plugin,
                                        session_repo,
                                        message_repo,
                                        #[cfg(not(target_arch = "wasm32"))]
                                        cascade_vault_snapshot,
                                        &mut note_version,
                                        &mut session_version,
                                        &mut active_session,
                                        &mut active_scope,
                                        Some(1), // one level per click
                                        // Workflow canvas's per-node ▶ is the
                                        // generic step-through — no
                                        // SDE sub-filter applies here.
                                        crate::plugins::artifact::cascade::RunMode::Full,
                                    );
                                    return;
                                }
                                // Skill DAG nodes (hand-built workflows):
                                // keep the legacy per-skill run.
                                let (Some(active_session), Some(active_scope)) =
                                    (active_session_signal, active_scope_signal)
                                else {
                                    eprintln!(
                                        "operon: per-node run BAIL — \
                                         ActiveChatSession/Scope context missing"
                                    );
                                    return;
                                };
                                spawn_run_node(
                                    note_id_for_run.clone(),
                                    node_id,
                                    graph_for_run.clone(),
                                    apply_for_run,
                                    active_session,
                                    active_scope,
                                );
                            };
                            let nodes_for_drag = nodes.clone();
                            let on_node_mousedown = move |evt: dioxus::events::MouseEvent| {
                                evt.stop_propagation();
                                let coords = evt.client_coordinates();
                                let mods = evt.modifiers();
                                let ctrl = mods.contains(Modifiers::CONTROL)
                                    || mods.contains(Modifiers::META);
                                let shift = mods.contains(Modifiers::SHIFT);
                                let mut set = selected_nodes.read().clone();
                                if ctrl {
                                    if !set.remove(&node_id) {
                                        set.insert(node_id);
                                    }
                                } else if shift {
                                    set.insert(node_id);
                                } else if !set.contains(&node_id) {
                                    // Plain click on a non-selected node:
                                    // replace the selection. If the node
                                    // is already in a multi-selection,
                                    // keep the rest so the user can drag
                                    // the whole group.
                                    set.clear();
                                    set.insert(node_id);
                                }
                                // Capture every selected node's start
                                // position so the drag move loop can
                                // translate the group together.
                                let start_positions: Vec<(NodeId, f64, f64)> = if set.len() >= 2
                                    && set.contains(&node_id)
                                {
                                    nodes_for_drag
                                        .iter()
                                        .filter(|n| set.contains(&n.id))
                                        .map(|n| (n.id, n.x, n.y))
                                        .collect()
                                } else {
                                    vec![(node_id, n_x, n_y)]
                                };
                                drag.set(Some(DragState {
                                    node: node_id,
                                    start_client_x: coords.x,
                                    start_client_y: coords.y,
                                    start_positions,
                                }));
                                selected_nodes.set(set);
                                selected_edge.set(None);
                            };
                            // Output-handle mousedown: start an edge
                            // drag from this node. stop_propagation so
                            // the parent node's mousedown doesn't fire
                            // a node-position drag at the same time.
                            let on_output_mousedown = move |evt: dioxus::events::MouseEvent| {
                                evt.stop_propagation();
                                let coords = evt.client_coordinates();
                                let from_x = n_x + NODE_W;
                                let from_y = n_y + NODE_H / 2.0;
                                edge_drag.set(Some(EdgeDragState {
                                    from: node_id,
                                    from_x,
                                    from_y,
                                    cursor_x: from_x,
                                    cursor_y: from_y,
                                    start_client_x: coords.x,
                                    start_client_y: coords.y,
                                }));
                            };
                            // Input-handle mouseup: commit the edge if
                            // an edge_drag is in flight and the source
                            // is a different node and no duplicate
                            // (from, to) edge already exists.
                            let apply_for_edge = props.apply_graph;
                            let graph_for_edge = g.clone();
                            let on_input_mouseup = move |evt: dioxus::events::MouseEvent| {
                                evt.stop_propagation();
                                let cur = match *edge_drag.read() {
                                    Some(d) => d,
                                    None => return,
                                };
                                if cur.from == node_id {
                                    edge_drag.set(None);
                                    return;
                                }
                                let next = add_edge_if_new(&graph_for_edge, cur.from, node_id);
                                if let Some(g_next) = next {
                                    apply_for_edge.call(g_next);
                                    // Auto-expand the upstream so the
                                    // newly-wired downstream stays
                                    // visible (parity with the canvas-
                                    // level edge-drag mouseup handler).
                                    expanded.with_mut(|s| {
                                        s.insert(cur.from);
                                    });
                                    let snap = expanded.read().clone();
                                    persist_expanded.call(snap);
                                }
                                edge_drag.set(None);
                            };
                            // Input-handle mousedown still has to
                            // stop_propagation so it doesn't trigger
                            // a node-position drag.
                            let on_input_mousedown = move |evt: dioxus::events::MouseEvent| {
                                evt.stop_propagation();
                            };
                            let is_selected = selected_nodes.read().contains(&node_id);
                            // Hover endpoint: the cursor is on an edge
                            // whose source or target is this node. Used
                            // to pulse-tint connecting nodes alongside
                            // the highlighted edge so the user can
                            // trace fan-out at a glance.
                            let is_hover_endpoint = hover_endpoints.contains(&node_id);
                            let mut group_class = String::from("operon-workflow-node-group");
                            if is_selected {
                                group_class.push_str(" operon-workflow-node-group-selected");
                            }
                            if is_hover_endpoint {
                                group_class.push_str(" operon-workflow-node-group-edge-hover");
                            }
                            // Slug for the kind-color CSS hook
                            // (`[data-artifact-kind="epic"]` etc).
                            // Lower-cased + spaces collapsed to underscores
                            // so display names like "Test Cases" become
                            // "test_cases" and the selector is stable.
                            let kind_slug: String = n
                                .kind_label
                                .as_deref()
                                .map(|s| s.to_lowercase().replace(' ', "_"))
                                .unwrap_or_default();
                            rsx! {
                                g {
                                    class: "{group_class}",
                                    "data-node-id": "{n.id}",
                                    "data-selected": if is_selected { "true" } else { "false" },
                                    transform: "translate({n.x}, {n.y})",
                                    onmousedown: on_node_mousedown,
                                    rect {
                                        class: status_class(&n.status, n.is_artifact_snapshot),
                                        "data-testid": "workflow-node",
                                        "data-node-kind": if n.is_artifact_snapshot { "artifact" } else { "skill" },
                                        "data-artifact-kind": "{kind_slug}",
                                        width: "{NODE_W}",
                                        height: "{NODE_H}",
                                        rx: "8",
                                        ry: "8",
                                    }
                                    // Row 1: kind badge inline next to
                                    // the chevron. The chevron itself is
                                    // rendered later (see below) at the
                                    // far-left of the controls strip.
                                    if n.is_artifact_snapshot {
                                        text {
                                            class: "operon-workflow-node-kind-badge",
                                            x: "36",
                                            y: "{NODE_ROW1_Y + 4.0}",
                                            "{n.kind_label.clone().unwrap_or_default()}"
                                        }
                                    }
                                    // Row 2: name / title.
                                    text {
                                        class: "operon-workflow-node-title",
                                        x: "16",
                                        y: "{NODE_ROW2_Y}",
                                        "{n.label}"
                                    }
                                    // Row 3 (skill nodes only) — live activity
                                    // readout fed by the executor's AgentEvent
                                    // stream via NODE_LIVE_STATE. Shows current
                                    // tool, thinking pulse, last write, or
                                    // last error. Cleared on Done; sticky on
                                    // Error so the failure reason survives the
                                    // run's terminal event.
                                    {
                                        let live_render: Option<(&'static str, String, &'static str)> = if n.is_artifact_snapshot {
                                            None
                                        } else {
                                            node_live_snap.get(&n.id).map(describe_live_state)
                                        };
                                        if let Some((icon, label, css)) = live_render {
                                            rsx! {
                                                text {
                                                    class: "operon-workflow-node-live operon-workflow-node-live-{css}",
                                                    "data-testid": "workflow-node-live",
                                                    "data-live-kind": "{css}",
                                                    x: "16",
                                                    y: "{NODE_ROW_LIVE_Y}",
                                                    "{icon} {label}"
                                                }
                                            }
                                        } else {
                                            rsx! {}
                                        }
                                    }
                                    // (Status moved to the row-4 pill
                                    // strip — the pill that matches
                                    // `n.status` is highlighted.)
                                    // Input handle (left edge,
                                    // mid-height): visible 8-radius
                                    // dot + invisible 14-radius hit
                                    // circle for forgiving release
                                    // targets. Drop target for an
                                    // active edge_drag.
                                    circle {
                                        class: "operon-workflow-handle-hit",
                                        cx: "0",
                                        cy: "{NODE_H / 2.0}",
                                        r: "14",
                                        onmousedown: on_input_mousedown,
                                        onmouseup: on_input_mouseup,
                                    }
                                    circle {
                                        class: "operon-workflow-handle operon-workflow-handle-input",
                                        "data-testid": "workflow-handle-input",
                                        cx: "0",
                                        cy: "{NODE_H / 2.0}",
                                        r: "8",
                                    }
                                    // Output handle (right edge,
                                    // mid-height): drag-source for new
                                    // edges.
                                    circle {
                                        class: "operon-workflow-handle-hit",
                                        cx: "{NODE_W}",
                                        cy: "{NODE_H / 2.0}",
                                        r: "14",
                                        onmousedown: on_output_mousedown,
                                    }
                                    circle {
                                        class: "operon-workflow-handle operon-workflow-handle-output",
                                        "data-testid": "workflow-handle-output",
                                        cx: "{NODE_W}",
                                        cy: "{NODE_H / 2.0}",
                                        r: "8",
                                    }
                                    // Per-edge anchor dots. One small
                                    // circle per incident edge, drawn
                                    // at the y-coord where that edge's
                                    // bezier actually lands. Lets the
                                    // user see how many fan-in / fan-
                                    // out connections a node has at a
                                    // glance, and matches the visual
                                    // style of the curves themselves.
                                    {
                                        let in_ys = node_input_anchors
                                            .get(&node_id)
                                            .cloned()
                                            .unwrap_or_default();
                                        let out_ys = node_output_anchors
                                            .get(&node_id)
                                            .cloned()
                                            .unwrap_or_default();
                                        rsx! {
                                            for (i, ya) in in_ys.iter().enumerate() {
                                                circle {
                                                    key: "in-{i}",
                                                    class: "operon-workflow-anchor operon-workflow-anchor-input",
                                                    cx: "0",
                                                    cy: "{ya}",
                                                    r: "3",
                                                }
                                            }
                                            for (i, ya) in out_ys.iter().enumerate() {
                                                circle {
                                                    key: "out-{i}",
                                                    class: "operon-workflow-anchor operon-workflow-anchor-output",
                                                    cx: "{NODE_W}",
                                                    cy: "{ya}",
                                                    r: "3",
                                                }
                                            }
                                        }
                                    }
                                    // Explicit "View" affordance —
                                    // clicking the rect already
                                    // selects the node (mousedown
                                    // Expand / collapse chevron — only
                                    // shown on parents (nodes with at
                                    // least one outgoing edge). Click
                                    // toggles the node's membership in
                                    // the `expanded` set, which is read
                                    // each render via `compute_visible`.
                                    if n.has_children {
                                        {
                                            let chevron_id = node_id;
                                            let glyph = if n.is_expanded { "\u{25BE}" } else { "\u{25B8}" };
                                            rsx! {
                                                g {
                                                    class: "operon-workflow-node-toggle",
                                                    "data-testid": "workflow-node-toggle",
                                                    "data-expanded": if n.is_expanded { "true" } else { "false" },
                                                    transform: "translate(8, 8)",
                                                    onclick: move |evt: dioxus::events::MouseEvent| {
                                                        evt.stop_propagation();
                                                        let mut set = expanded.read().clone();
                                                        if !set.remove(&chevron_id) {
                                                            set.insert(chevron_id);
                                                        }
                                                        expanded.set(set.clone());
                                                        persist_expanded.call(set);
                                                    },
                                                    onmousedown: move |evt: dioxus::events::MouseEvent| {
                                                        evt.stop_propagation();
                                                    },
                                                    rect {
                                                        width: "20",
                                                        height: "20",
                                                        rx: "4",
                                                        ry: "4",
                                                        class: "operon-workflow-node-toggle-bg",
                                                    }
                                                    text {
                                                        x: "10",
                                                        y: "14",
                                                        "text-anchor": "middle",
                                                        class: "operon-workflow-node-toggle-glyph",
                                                        "{glyph}"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    // handler), but a dedicated icon
                                    // makes the inspector
                                    // discoverable. stop_propagation
                                    // so the click doesn't ALSO start
                                    // a node drag.
                                    g {
                                        class: "operon-workflow-node-view",
                                        "data-testid": "workflow-node-view",
                                        transform: "translate({NODE_W - 56.0}, 8.0)",
                                        onclick: move |evt: dioxus::events::MouseEvent| {
                                            evt.stop_propagation();
                                            let mut set = std::collections::BTreeSet::new();
                                            set.insert(node_id);
                                            selected_nodes.set(set);
                                            selected_edge.set(None);
                                        },
                                        onmousedown: move |evt: dioxus::events::MouseEvent| {
                                            evt.stop_propagation();
                                        },
                                        rect {
                                            width: "20",
                                            height: "20",
                                            rx: "10",
                                            ry: "10",
                                            class: "operon-workflow-node-view-bg",
                                        }
                                        text {
                                            x: "10",
                                            y: "14",
                                            "text-anchor": "middle",
                                            class: "operon-workflow-node-view-glyph",
                                            "\u{2630}"
                                        }
                                    }
                                    g {
                                        class: "operon-workflow-node-run",
                                        "data-testid": "workflow-node-run",
                                        transform: "translate({NODE_W - 28.0}, 8.0)",
                                        onclick: on_run_node,
                                        onmousedown: move |evt: dioxus::events::MouseEvent| {
                                            evt.stop_propagation();
                                        },
                                        rect {
                                            width: "20",
                                            height: "20",
                                            rx: "10",
                                            ry: "10",
                                            class: "operon-workflow-node-run-bg",
                                        }
                                        if matches!(n.status, NodeStatus::Running) {
                                            // Cascade is currently running a
                                            // skill against this artifact.
                                            // Swap the ▶ glyph for ⟳ with a
                                            // CSS rotation animation so the
                                            // user can see at a glance which
                                            // tile is being processed.
                                            text {
                                                x: "10",
                                                y: "14",
                                                "text-anchor": "middle",
                                                class: "operon-workflow-node-run-glyph operon-workflow-node-run-glyph-running",
                                                "\u{27F3}"
                                            }
                                        } else {
                                            text {
                                                x: "10",
                                                y: "14",
                                                "text-anchor": "middle",
                                                class: "operon-workflow-node-run-glyph",
                                                "\u{25B6}"
                                            }
                                        }
                                    }
                                    // ✏ Extra-instructions trigger
                                    // (row 1, far right). Click hydrates
                                    // the popover draft from the node's
                                    // current `extra_instructions` and
                                    // opens the in-place dialog.
                                    {
                                        let g_for_extra = g.clone();
                                        rsx! {
                                            g {
                                                class: "operon-workflow-node-edit",
                                                "data-testid": "workflow-node-extra-instructions",
                                                transform: "translate({NODE_W - 84.0}, 8.0)",
                                                onclick: move |evt: dioxus::events::MouseEvent| {
                                                    evt.stop_propagation();
                                                    let cur = g_for_extra
                                                        .nodes
                                                        .get(&node_id)
                                                        .map(|n| n.extra_instructions.clone())
                                                        .unwrap_or_default();
                                                    extra_instructions_draft.set(cur);
                                                    extra_instructions_open
                                                        .set(Some(node_id));
                                                },
                                                onmousedown: move |evt: dioxus::events::MouseEvent| {
                                                    evt.stop_propagation();
                                                },
                                                rect {
                                                    width: "20",
                                                    height: "20",
                                                    rx: "10",
                                                    ry: "10",
                                                    class: "operon-workflow-node-edit-bg",
                                                }
                                                text {
                                                    x: "10",
                                                    y: "14",
                                                    "text-anchor": "middle",
                                                    class: "operon-workflow-node-edit-glyph",
                                                    "\u{270F}"
                                                }
                                            }
                                        }
                                    }
                                    // Row 3 — artifact action strip
                                    // (Approve / Reject / Mark dirty /
                                    // Revise). Only on cascade-snapshot
                                    // tiles; skill nodes don't have an
                                    // approval lifecycle.
                                    if n.is_artifact_snapshot {
                                        {
                                            let pers = persistence_for_view.clone();
                                            let mut version_sink = note_version_for_actions;
                                            let note_repo_for_revise =
                                                note_repo_for_actions.clone();
                                            let aref = g.nodes.get(&node_id).and_then(|gn| gn.artifact_ref);
                                            // Current artifact frontmatter status — drives both the
                                            // active-button highlight and the click-time short-circuit
                                            // (clicking Approve when already approved is a no-op).
                                            let current_status: Option<ArtifactStatus> =
                                                aref.and_then(|a| artifact_statuses.get(&a).cloned());
                                            let cur_for_factory = current_status.clone();
                                            // Helper closure factory: returns a click handler that
                                            // loads the artifact body, patches its frontmatter to
                                            // the chosen status, saves, mirrors the new body into
                                            // any open tab's buffer (so an artifact-view tab open
                                            // for the same note re-renders with the fresh status),
                                            // and bumps LocalNoteVersion. Skips early when the
                                            // artifact is already in `target`.
                                            let tabs_for_action = tabs_for_view;
                                            let patch_action = move |target: ArtifactStatus| {
                                                let pers = pers.clone();
                                                let cur = cur_for_factory.clone();
                                                move |evt: dioxus::events::MouseEvent| {
                                                    evt.stop_propagation();
                                                    if let Some(c) = cur.as_ref() {
                                                        if std::mem::discriminant(c)
                                                            == std::mem::discriminant(&target)
                                                        {
                                                            return;
                                                        }
                                                    }
                                                    let Some(aref) = aref else { return };
                                                    let Some(pers) = pers.clone() else { return };
                                                    let id_str = aref.to_string();
                                                    spawn(async move {
                                                        let bytes = match pers.load(&id_str).await {
                                                            Ok(b) => b,
                                                            Err(PersistError::NotFound) => Vec::new(),
                                                            Err(e) => {
                                                                eprintln!(
                                                                    "operon: workflow action load error note_id={id_str}: {e:?}"
                                                                );
                                                                return;
                                                            }
                                                        };
                                                        let body = String::from_utf8(bytes).unwrap_or_default();
                                                        let new_body =
                                                            patch_status_text(&body, target.clone());
                                                        if let Err(e) = pers
                                                            .save(&id_str, new_body.as_bytes())
                                                            .await
                                                        {
                                                            eprintln!(
                                                                "operon: workflow action save error note_id={id_str}: {e:?}"
                                                            );
                                                            return;
                                                        }
                                                        // Mirror the new body into every open tab
                                                        // for this note. Disk and the in-memory tab
                                                        // buffer were drifting — the artifact-view
                                                        // tab kept rendering its stale content prop
                                                        // even after the workflow card flipped the
                                                        // frontmatter on disk.
                                                        if let Some(mut tabs) = tabs_for_action {
                                                            let tab_ids: Vec<crate::tabs::TabId> = {
                                                                let snap = tabs.read();
                                                                let ids = snap
                                                                    .iter()
                                                                    .filter(|t| t.note_id == id_str)
                                                                    .map(|t| t.id)
                                                                    .collect();
                                                                ids
                                                            };
                                                            for tid in tab_ids {
                                                                tabs.write()
                                                                    .set_content(tid, new_body.clone());
                                                            }
                                                        }
                                                        if let Some(mut v) = version_sink {
                                                            v.with_mut(|x| {
                                                                *x = x.saturating_add(1)
                                                            });
                                                        }
                                                        crate::shell::companion_state::LOCAL_NOTE_VERSION
                                                            .with_mut(|x| {
                                                                *x = x.saturating_add(1)
                                                            });
                                                    });
                                                }
                                            };
                                            let on_approve =
                                                patch_action.clone()(ArtifactStatus::Approved);
                                            let on_reject =
                                                patch_action.clone()(ArtifactStatus::Rejected);
                                            let on_mark_dirty =
                                                patch_action(ArtifactStatus::Dirty);
                                            // Revise = walk descendants
                                            // and flip Approved → Dirty.
                                            // Reuses the artifact view's
                                            // helper.
                                            let on_revise = {
                                                let note_repo = note_repo_for_revise.clone();
                                                let pers_revise = persistence_for_view.clone();
                                                let mut version_sink_rv = note_version_for_actions;
                                                move |evt: dioxus::events::MouseEvent| {
                                                    evt.stop_propagation();
                                                    let Some(aref) = aref else { return };
                                                    let Some(repo) = note_repo.clone() else { return };
                                                    let Some(pers) = pers_revise.clone() else { return };
                                                    spawn(async move {
                                                        match mark_descendants_dirty(
                                                            &repo, &pers, aref,
                                                        )
                                                        .await
                                                        {
                                                            Ok(_n) => {}
                                                            Err(e) => eprintln!(
                                                                "operon: workflow Revise walk failed for {aref}: {e}"
                                                            ),
                                                        }
                                                        if let Some(mut v) = version_sink_rv {
                                                            v.with_mut(|x| {
                                                                *x = x.saturating_add(1)
                                                            });
                                                        }
                                                        crate::shell::companion_state::LOCAL_NOTE_VERSION
                                                            .with_mut(|x| {
                                                                *x = x.saturating_add(1)
                                                            });
                                                    });
                                                }
                                            };
                                            // Active = "this is the current ArtifactStatus";
                                            // we add `-active` to highlight the matching button
                                            // and `-disabled` so CSS can dim it + drop pointer
                                            // events. The click handler short-circuits inside
                                            // `patch_action` either way.
                                            let approve_active = matches!(current_status, Some(ArtifactStatus::Approved));
                                            let reject_active = matches!(current_status, Some(ArtifactStatus::Rejected));
                                            let dirty_active = matches!(current_status, Some(ArtifactStatus::Dirty));
                                            let approve_class = if approve_active {
                                                "operon-workflow-node-action-button operon-workflow-node-action-approve operon-workflow-node-action-active operon-workflow-node-action-disabled"
                                            } else {
                                                "operon-workflow-node-action-button operon-workflow-node-action-approve"
                                            };
                                            let reject_class = if reject_active {
                                                "operon-workflow-node-action-button operon-workflow-node-action-reject operon-workflow-node-action-active operon-workflow-node-action-disabled"
                                            } else {
                                                "operon-workflow-node-action-button operon-workflow-node-action-reject"
                                            };
                                            let dirty_class = if dirty_active {
                                                "operon-workflow-node-action-button operon-workflow-node-action-dirty operon-workflow-node-action-active operon-workflow-node-action-disabled"
                                            } else {
                                                "operon-workflow-node-action-button operon-workflow-node-action-dirty"
                                            };
                                            rsx! {
                                                g {
                                                    class: "operon-workflow-node-actions",
                                                    transform: "translate(0, {NODE_ROW_ACTIONS_Y - 11.0})",
                                                    g {
                                                        class: "{approve_class}",
                                                        "data-testid": "workflow-node-approve",
                                                        transform: "translate(14, 0)",
                                                        onclick: on_approve,
                                                        onmousedown: move |evt: dioxus::events::MouseEvent| { evt.stop_propagation(); },
                                                        rect { width: "56", height: "22", rx: "4", ry: "4", class: "operon-workflow-node-action-bg" }
                                                        text { x: "28", y: "15", "text-anchor": "middle", class: "operon-workflow-node-action-label", "Approve" }
                                                    }
                                                    g {
                                                        class: "{reject_class}",
                                                        "data-testid": "workflow-node-reject",
                                                        transform: "translate(76, 0)",
                                                        onclick: on_reject,
                                                        onmousedown: move |evt: dioxus::events::MouseEvent| { evt.stop_propagation(); },
                                                        rect { width: "56", height: "22", rx: "4", ry: "4", class: "operon-workflow-node-action-bg" }
                                                        text { x: "28", y: "15", "text-anchor": "middle", class: "operon-workflow-node-action-label", "Reject" }
                                                    }
                                                    g {
                                                        class: "{dirty_class}",
                                                        "data-testid": "workflow-node-mark-dirty",
                                                        transform: "translate(138, 0)",
                                                        onclick: on_mark_dirty,
                                                        onmousedown: move |evt: dioxus::events::MouseEvent| { evt.stop_propagation(); },
                                                        rect { width: "56", height: "22", rx: "4", ry: "4", class: "operon-workflow-node-action-bg" }
                                                        text { x: "28", y: "15", "text-anchor": "middle", class: "operon-workflow-node-action-label", "Dirty" }
                                                    }
                                                    g {
                                                        class: "operon-workflow-node-action-button operon-workflow-node-action-revise",
                                                        "data-testid": "workflow-node-revise",
                                                        transform: "translate(200, 0)",
                                                        onclick: on_revise,
                                                        onmousedown: move |evt: dioxus::events::MouseEvent| { evt.stop_propagation(); },
                                                        rect { width: "50", height: "22", rx: "4", ry: "4", class: "operon-workflow-node-action-bg" }
                                                        text { x: "25", y: "15", "text-anchor": "middle", class: "operon-workflow-node-action-label", "Revise" }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    // Row 4 — NodeStatus pill strip.
                                    // Click flips the node's status and
                                    // bumps the graph version through
                                    // `apply_with_undo` so Ctrl+Z reverts.
                                    {
                                        let apply_state = props.apply_with_undo;
                                        let g_state = g.clone();
                                        let cur_status = n.status.clone();
                                        let make_state_handler = move |target: NodeStatus| {
                                            let g_state = g_state.clone();
                                            move |evt: dioxus::events::MouseEvent| {
                                                evt.stop_propagation();
                                                let mut next = g_state.clone();
                                                if let Some(node) = next.nodes.get_mut(&node_id) {
                                                    node.status = target.clone();
                                                }
                                                next.version = next.version.saturating_add(1);
                                                apply_state.call(next);
                                            }
                                        };
                                        let active = |s: &NodeStatus| {
                                            std::mem::discriminant(s)
                                                == std::mem::discriminant(&cur_status)
                                        };
                                        let dirty_class = if active(&NodeStatus::Dirty) {
                                            "operon-workflow-node-state-pill operon-workflow-node-state-pill-active"
                                        } else {
                                            "operon-workflow-node-state-pill"
                                        };
                                        let running_class = if active(&NodeStatus::Running) {
                                            "operon-workflow-node-state-pill operon-workflow-node-state-pill-active"
                                        } else {
                                            "operon-workflow-node-state-pill"
                                        };
                                        let fresh_class = if active(&NodeStatus::Fresh) {
                                            "operon-workflow-node-state-pill operon-workflow-node-state-pill-active"
                                        } else {
                                            "operon-workflow-node-state-pill"
                                        };
                                        let error_class = if matches!(cur_status, NodeStatus::Error(_)) {
                                            "operon-workflow-node-state-pill operon-workflow-node-state-pill-active"
                                        } else {
                                            "operon-workflow-node-state-pill"
                                        };
                                        let on_dirty = make_state_handler.clone()(NodeStatus::Dirty);
                                        let on_running = make_state_handler.clone()(NodeStatus::Running);
                                        let on_fresh = make_state_handler.clone()(NodeStatus::Fresh);
                                        let on_error = make_state_handler(NodeStatus::Error("manual".into()));
                                        rsx! {
                                            g {
                                                class: "operon-workflow-node-states",
                                                transform: "translate(0, {NODE_ROW_STATUS_Y - 11.0})",
                                                g {
                                                    class: "{dirty_class}",
                                                    "data-testid": "workflow-node-state-dirty",
                                                    transform: "translate(14, 0)",
                                                    onclick: on_dirty,
                                                    onmousedown: move |evt: dioxus::events::MouseEvent| { evt.stop_propagation(); },
                                                    rect { width: "56", height: "22", rx: "11", ry: "11", class: "operon-workflow-node-state-bg" }
                                                    text { x: "28", y: "15", "text-anchor": "middle", class: "operon-workflow-node-state-label", "Dirty" }
                                                }
                                                g {
                                                    class: "{running_class}",
                                                    "data-testid": "workflow-node-state-running",
                                                    transform: "translate(76, 0)",
                                                    onclick: on_running,
                                                    onmousedown: move |evt: dioxus::events::MouseEvent| { evt.stop_propagation(); },
                                                    rect { width: "56", height: "22", rx: "11", ry: "11", class: "operon-workflow-node-state-bg" }
                                                    text { x: "28", y: "15", "text-anchor": "middle", class: "operon-workflow-node-state-label", "Running" }
                                                }
                                                g {
                                                    class: "{fresh_class}",
                                                    "data-testid": "workflow-node-state-fresh",
                                                    transform: "translate(138, 0)",
                                                    onclick: on_fresh,
                                                    onmousedown: move |evt: dioxus::events::MouseEvent| { evt.stop_propagation(); },
                                                    rect { width: "56", height: "22", rx: "11", ry: "11", class: "operon-workflow-node-state-bg" }
                                                    text { x: "28", y: "15", "text-anchor": "middle", class: "operon-workflow-node-state-label", "Fresh" }
                                                }
                                                g {
                                                    class: "{error_class}",
                                                    "data-testid": "workflow-node-state-error",
                                                    transform: "translate(200, 0)",
                                                    onclick: on_error,
                                                    onmousedown: move |evt: dioxus::events::MouseEvent| { evt.stop_propagation(); },
                                                    rect { width: "50", height: "22", rx: "11", ry: "11", class: "operon-workflow-node-state-bg" }
                                                    text { x: "25", y: "15", "text-anchor": "middle", class: "operon-workflow-node-state-label", "Error" }
                                                }
                                            }
                                        }
                                    }
                                    // Row 5 — footer icons (👁 view / 🗑 delete).
                                    {
                                        // View: open the node's note in
                                        // a tab + reveal in explorer.
                                        let sel_for_view = selected_note_app;
                                        let focus_for_view = focused_node_app;
                                        let tabs_handle = tabs_for_view;
                                        let scheduler_handle = scheduler_for_view.clone();
                                        let persistence_handle = persistence_for_view.clone();
                                        let titles_for_click = skill_titles.clone();
                                        let kinds_for_click = note_kinds.clone();
                                        // Look up the underlying graph node so we can read
                                        // `cached_output_note_id` / `artifact_ref` (those
                                        // fields aren't on the render-side `NodeRender`).
                                        let view_target_id = g.nodes.get(&node_id).and_then(|gn| {
                                            gn.cached_output_note_id.or(if gn.is_artifact_snapshot {
                                                gn.artifact_ref
                                            } else {
                                                None
                                            })
                                        });
                                        let on_eye = move |evt: dioxus::events::MouseEvent| {
                                            evt.stop_propagation();
                                            let Some(id) = view_target_id else { return };
                                            if let (Some(mut tabs), Some(scheduler)) =
                                                (tabs_handle, scheduler_handle.clone())
                                            {
                                                let id_str = id.to_string();
                                                let existing_edit = {
                                                    let snap = tabs.read();
                                                    let id_match = snap
                                                        .iter()
                                                        .find(|t| {
                                                            t.note_id == id_str
                                                                && matches!(t.mode, EditorMode::Edit)
                                                        })
                                                        .map(|t| t.id);
                                                    id_match
                                                };
                                                if let Some(tid) = existing_edit {
                                                    tabs.write().activate(tid);
                                                } else {
                                                    let title_str = titles_for_click
                                                        .get(&id)
                                                        .cloned()
                                                        .unwrap_or_else(|| id.to_string());
                                                    let kind = kinds_for_click
                                                        .get(&id)
                                                        .copied()
                                                        .unwrap_or(NoteKind::Markdown);
                                                    let inherited = {
                                                        let snap = tabs.read();
                                                        let c = snap
                                                            .iter()
                                                            .find(|t| t.note_id == id_str)
                                                            .map(|t| t.content.clone());
                                                        c
                                                    };
                                                    #[cfg(not(target_arch = "wasm32"))]
                                                    let initial_content = match inherited {
                                                        Some(c) => c,
                                                        None => match persistence_handle
                                                            .as_ref()
                                                            .map(|p| {
                                                                futures::executor::block_on(
                                                                    p.load(&id_str),
                                                                )
                                                            })
                                                        {
                                                            Some(Ok(bytes)) => {
                                                                String::from_utf8(bytes)
                                                                    .unwrap_or_default()
                                                            }
                                                            Some(Err(PersistError::NotFound)) => {
                                                                String::new()
                                                            }
                                                            Some(Err(e)) => {
                                                                eprintln!(
                                                                    "operon: workflow eye load error note_id={id_str}: {e:?}"
                                                                );
                                                                String::new()
                                                            }
                                                            None => String::new(),
                                                        },
                                                    };
                                                    #[cfg(target_arch = "wasm32")]
                                                    let initial_content =
                                                        inherited.unwrap_or_default();
                                                    let _ = open_local_note_tab(
                                                        tabs,
                                                        scheduler,
                                                        id,
                                                        title_str,
                                                        initial_content,
                                                        kind,
                                                    );
                                                }
                                            }
                                            if let Some(mut sel) = sel_for_view {
                                                sel.set(Some(id));
                                            }
                                            if let Some(mut focus) = focus_for_view {
                                                focus.set(Some(NodeKey::Note(id)));
                                            }
                                        };
                                        // Delete: undoable.
                                        let apply_delete = props.apply_with_undo;
                                        let g_delete = g.clone();
                                        let on_trash = move |evt: dioxus::events::MouseEvent| {
                                            evt.stop_propagation();
                                            apply_delete.call(remove_node(&g_delete, node_id));
                                        };
                                        let view_disabled = view_target_id.is_none();
                                        rsx! {
                                            g {
                                                class: "operon-workflow-node-footer",
                                                transform: "translate(0, {NODE_ROW_FOOTER_Y - 11.0})",
                                                g {
                                                    class: if view_disabled
                                                        { "operon-workflow-node-icon-eye operon-workflow-node-icon-disabled" }
                                                        else { "operon-workflow-node-icon-eye" },
                                                    "data-testid": "workflow-node-eye",
                                                    transform: "translate({NODE_W - 56.0}, 0)",
                                                    onclick: on_eye,
                                                    onmousedown: move |evt: dioxus::events::MouseEvent| {
                                                        evt.stop_propagation();
                                                    },
                                                    rect { width: "20", height: "20", rx: "4", ry: "4",
                                                        class: "operon-workflow-node-icon-bg" }
                                                    text { x: "10", y: "14", "text-anchor": "middle",
                                                        class: "operon-workflow-node-icon-glyph",
                                                        "\u{1F441}" }
                                                }
                                                g {
                                                    class: "operon-workflow-node-icon-trash",
                                                    "data-testid": "workflow-node-trash",
                                                    transform: "translate({NODE_W - 28.0}, 0)",
                                                    onclick: on_trash,
                                                    onmousedown: move |evt: dioxus::events::MouseEvent| {
                                                        evt.stop_propagation();
                                                    },
                                                    rect { width: "20", height: "20", rx: "4", ry: "4",
                                                        class: "operon-workflow-node-icon-bg operon-workflow-node-icon-bg-danger" }
                                                    text { x: "10", y: "14", "text-anchor": "middle",
                                                        class: "operon-workflow-node-icon-glyph",
                                                        "\u{1F5D1}" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    // Marquee rectangle — rendered while a left-drag on
                    // empty canvas is in flight. Drawn last in the world
                    // group so it sits on top of nodes/edges. The
                    // pointer-events:none in CSS lets clicks pass through.
                    if let Some(m) = marquee.read().as_ref() {
                        {
                            let x = m.start_x.min(m.cur_x);
                            let y = m.start_y.min(m.cur_y);
                            let w = (m.cur_x - m.start_x).abs();
                            let h = (m.cur_y - m.start_y).abs();
                            rsx! {
                                rect {
                                    class: "operon-workflow-marquee",
                                    "data-testid": "workflow-marquee",
                                    x: "{x}",
                                    y: "{y}",
                                    width: "{w}",
                                    height: "{h}",
                                }
                            }
                        }
                    }
                    // Ghost edge — rendered while dragging from an
                    // output handle. Same cubic-bezier shape as a
                    // committed edge so the user gets a preview of
                    // the curvature, but with a dashed stroke so the
                    // unfinished state reads visually.
                    if let Some(d) = edge_drag.read().as_ref() {
                        path {
                            class: "operon-workflow-edge operon-workflow-edge-ghost",
                            "data-testid": "workflow-edge-ghost",
                            d: "M {d.from_x} {d.from_y} C {d.from_x + 60.0} {d.from_y}, {d.cursor_x - 60.0} {d.cursor_y}, {d.cursor_x} {d.cursor_y}",
                            "marker-end": "url(#operon-workflow-arrow)",
                        }
                    }
                    // Selected-edge delete button — rendered last so
                    // it draws above any node it overlaps. Only one
                    // edge can be selected at a time, so a single
                    // group is sufficient.
                    if let Some(sel_id) = *selected_edge.read() {
                        if let Some(e) = edges.iter().find(|e| e.id == sel_id) {
                            {
                                let mid_x = (e.from_x + e.to_x) / 2.0;
                                let mid_y = (e.from_y + e.to_y) / 2.0;
                                let apply = props.apply_graph;
                                let graph_for_edge_delete = g.clone();
                                rsx! {
                                    g {
                                        class: "operon-workflow-edge-delete",
                                        "data-testid": "workflow-edge-delete",
                                        "data-edge-id": "{sel_id}",
                                        transform: "translate({mid_x - 10.0}, {mid_y - 10.0})",
                                        onmousedown: move |evt: dioxus::events::MouseEvent| {
                                            evt.stop_propagation();
                                        },
                                        onclick: move |evt: dioxus::events::MouseEvent| {
                                            evt.stop_propagation();
                                            let next = remove_edge(&graph_for_edge_delete, sel_id);
                                            apply.call(next);
                                            selected_edge.set(None);
                                        },
                                        rect {
                                            class: "operon-workflow-edge-delete-bg",
                                            width: "20",
                                            height: "20",
                                            rx: "10",
                                            ry: "10",
                                        }
                                        text {
                                            class: "operon-workflow-edge-delete-glyph",
                                            x: "10",
                                            y: "14",
                                            "text-anchor": "middle",
                                            "\u{2715}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                    } // end of world-transform <g>
                }
            }
        }
    }
}

/// Inspector "last output" preview. Renders the markdown body claude
/// wrote to `path` for a workflow node's last successful run. Keyed
/// from the parent on `(path, graph.version)` so a cascade re-run
/// remounts this component and re-reads the file.
///
/// Read happens off the render thread via `use_future` +
/// `spawn_blocking` so a multi-KB output doesn't block the canvas.
#[derive(Props, Clone, PartialEq)]
struct InspectorOutputPanelProps {
    path: std::path::PathBuf,
}

#[component]
fn InspectorOutputPanel(props: InspectorOutputPanelProps) -> Element {
    // None = still loading. Some(Ok) = file body. Some(Err) = error.
    let state: Signal<Option<Result<String, String>>> = use_signal(|| None);
    let path_for_load = props.path.clone();
    use_future(move || {
        let path = path_for_load.clone();
        let mut state_setter = state;
        async move {
            #[cfg(not(target_arch = "wasm32"))]
            {
                let res = tokio::task::spawn_blocking(move || std::fs::read_to_string(&path))
                    .await
                    .map_err(|e| format!("join: {e}"))
                    .and_then(|r| r.map_err(|e| format!("read: {e}")));
                state_setter.set(Some(res));
            }
            #[cfg(target_arch = "wasm32")]
            {
                // Wasm has no fs. Output panel is desktop-only; on
                // wasm we just surface a placeholder.
                let _ = path;
                state_setter.set(Some(Err("output preview is desktop-only".to_string())));
            }
        }
    });

    // Snapshot the current state into an owned value so the read
    // borrow doesn't outlive the match arms (the rsx blocks own
    // captures into the next render).
    let snapshot = state.read().clone();
    match snapshot {
        None => rsx! {
            div { class: "operon-workflow-inspector-output operon-workflow-inspector-output-loading",
                "data-testid": "workflow-inspector-output-loading",
                "Loading\u{2026}"
            }
        },
        Some(Err(msg)) => {
            // Distinguish "file not found" from other errors so the
            // user gets a useful prompt for the common re-run case.
            let is_missing = msg.contains("No such file") || msg.contains("not found");
            let label = if is_missing {
                "Output file is missing \u{2014} re-run to regenerate.".to_string()
            } else {
                format!("Couldn't read output: {msg}")
            };
            rsx! {
                div {
                    class: "operon-workflow-inspector-output operon-workflow-inspector-output-error",
                    "data-testid": "workflow-inspector-output-error",
                    "{label}"
                }
            }
        }
        Some(Ok(body)) if body.trim().is_empty() => rsx! {
            div { class: "operon-workflow-inspector-output operon-workflow-inspector-output-empty",
                "data-testid": "workflow-inspector-output-empty-body",
                "(empty)"
            }
        },
        Some(Ok(body)) => rsx! {
            div { class: "operon-workflow-inspector-output",
                "data-testid": "workflow-inspector-output-body",
                MarkdownView { content: body }
            }
        },
    }
}

#[derive(Props, Clone, PartialEq)]
struct WorkflowToolbarProps {
    note_id: String,
    graph_text: String,
    on_apply: Callback<String>,
    apply_graph: Callback<WorkflowGraph>,
    /// Undo-recording variant of `apply_graph` — used for discrete,
    /// user-initiated layout changes (currently Auto-arrange) so they
    /// can be reverted with Ctrl+Z.
    apply_with_undo: Callback<WorkflowGraph>,
    /// Visibility of the JSON-tree pane (the textarea to the right of the
    /// canvas). Toolbar renders a "{}" button that flips this signal.
    json_visible: Signal<bool>,
    /// Phase C: per-phase edge filter toggle. Toolbar renders a button
    /// that flips this signal; the canvas reads it when building the
    /// edge render list.
    hide_cross_phase_edges: Signal<bool>,
    /// Canvas-scope expand/collapse set. Toolbar buttons clear it
    /// ("Collapse all") or fill it with every node id ("Expand all").
    expanded: Signal<std::collections::BTreeSet<NodeId>>,
    /// Persistence hook — Expand-all / Collapse-all call this after
    /// updating `expanded` so the new state is written back into the
    /// workflow note's `view_state`.
    persist_expanded: Callback<std::collections::BTreeSet<NodeId>>,
}

#[component]
fn WorkflowToolbar(props: WorkflowToolbarProps) -> Element {
    let LocalNoteRepo(note_repo) = use_context();
    let LocalProjectRepo(project_repo) = use_context();
    // Hook-based reads: the resulting Signals are bound to this
    // component's runtime scope, which is what the spawn helpers'
    // `.set(...)` writes need to actually fire. Reading these via
    // `try_consume_context` from inside an event handler returns
    // detached handles whose writes are silently dropped.
    let ActiveChatSession(active_session_signal) = use_context();
    let ActiveChatScope(active_scope_signal) = use_context();
    // Additional contexts needed to launch `spawn_cascade` from the
    // toolbar's Run button (mirrors `CascadePlayButton`'s use_context
    // pattern in `plugins/artifact/view.rs`).
    let persistence: Arc<dyn Persistence> = use_context();
    let ClaudeCodePluginCtx(plugin) = use_context();
    let ChatSessionRepo(chat_session_repo) = use_context();
    let ChatMessageRepo(chat_message_repo) = use_context();
    let LocalNoteVersion(note_version) = use_context();
    let ChatSessionVersion(chat_session_version) = use_context();
    #[cfg(not(target_arch = "wasm32"))]
    let crate::local_mode::desktop::CurrentVaultRoot(vault_signal_toolbar) = use_context();
    let on_apply = props.on_apply;
    let graph_text = props.graph_text.clone();
    let apply_with_undo = props.apply_with_undo;
    let mut json_visible = props.json_visible;
    let mut expanded = props.expanded;
    let persist_expanded = props.persist_expanded;

    // Auto-arrange: parse the current graph, run a Sugiyama-style
    // layered layout, push the rewritten positions back through
    // `apply_with_undo` so Ctrl+Z reverts the move in one step.
    let graph_text_for_arrange = graph_text.clone();
    let on_auto_arrange = move |_| {
        let g: WorkflowGraph = match serde_json::from_str(&graph_text_for_arrange) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("operon: auto-arrange BAIL — graph_text parse: {e}");
                return;
            }
        };
        apply_with_undo.call(auto_arrange(&g));
    };

    // Expand / collapse all. Reading the current graph_text gives us
    // the full node set without holding a Signal handle to the parsed
    // graph (which lives one component up). Failures parse → no-op.
    // Both paths also call `persist_expanded` so the new state writes
    // back into the workflow note's `view_state` and survives a
    // close-and-reopen.
    let graph_text_for_expand = graph_text.clone();
    let on_expand_all = move |_| {
        let g: WorkflowGraph = match serde_json::from_str(&graph_text_for_expand) {
            Ok(g) => g,
            Err(_) => return,
        };
        let mut all: std::collections::BTreeSet<NodeId> =
            std::collections::BTreeSet::new();
        for id in g.nodes.keys() {
            all.insert(*id);
        }
        expanded.set(all.clone());
        persist_expanded.call(all);
    };
    let on_collapse_all = move |_| {
        let empty = std::collections::BTreeSet::new();
        expanded.set(empty.clone());
        persist_expanded.call(empty);
    };

    // Compute the current effective step-mode for label rendering.
    // Re-parses on every toolbar render — cheap (graph_text is a few
    // KB) and matches the rest of the toolbar's "trust the JSON,
    // re-derive" pattern.
    let current_step_mode = {
        let parsed: WorkflowGraph =
            serde_json::from_str(&graph_text).unwrap_or_default();
        crate::plugins::workflow::state::effective_step_mode(&parsed)
    };

    // Toggle handler: flip the persisted `view_state.step_mode`. We
    // always persist `Some(_)` (not `None`) so the user's explicit
    // choice sticks even if the graph shape later changes (e.g. they
    // delete every skill node — the heuristic would flip otherwise).
    let graph_text_for_step = graph_text.clone();
    let on_toggle_step_mode = move |_| {
        let mut graph: WorkflowGraph =
            match serde_json::from_str(&graph_text_for_step) {
                Ok(g) => g,
                Err(_) if graph_text_for_step.trim().is_empty() => {
                    WorkflowGraph::new()
                }
                Err(_) => return,
            };
        let next = !crate::plugins::workflow::state::effective_step_mode(&graph);
        graph.view_state.step_mode = Some(next);
        graph.version = graph.version.saturating_add(1);
        on_apply.call(serialize(&graph));
    };

    // Resolve the cascade's seed artifact tile from the current graph
    // text so the ▶ Run button can invoke `spawn_cascade` against the
    // originating master-requirement / Requirements artifact — the
    // same UUID `CascadePlayButton` on that artifact's tile would pass.
    // Cascades are always seeded with one such tile by
    // `cascade_graph::seed_cascade_workflow_root_only`; hand-built
    // workflows without one render Run disabled. Re-derived on every
    // render, mirroring `current_step_mode` above.
    let seed_artifact_id: Option<Uuid> = serde_json::from_str::<WorkflowGraph>(&graph_text)
        .ok()
        .and_then(|g| {
            g.nodes
                .values()
                .find(|n| n.is_artifact_snapshot && n.artifact_ref.is_some())
                .and_then(|n| n.artifact_ref)
        });

    rsx! {
        div { class: "operon-workflow-toolbar",
            "data-testid": "workflow-toolbar",
            button {
                r#type: "button",
                class: "operon-workflow-toolbar-button",
                "data-testid": "workflow-toolbar-run",
                disabled: seed_artifact_id.is_none(),
                title: if seed_artifact_id.is_some() {
                    "Run the SDLC pipeline from this cascade's seed artifact.".to_string()
                } else {
                    "This canvas has no seed artifact tile \u{2014} Run is disabled.".to_string()
                },
                onclick: {
                    let note_repo = note_repo.clone();
                    let project_repo = project_repo.clone();
                    let persistence = persistence.clone();
                    let plugin = plugin.clone();
                    let chat_session_repo = chat_session_repo.clone();
                    let chat_message_repo = chat_message_repo.clone();
                    let mut note_version_setter = note_version;
                    let mut chat_session_version_setter = chat_session_version;
                    let mut active_session_setter = active_session_signal;
                    let mut active_scope_setter = active_scope_signal;
                    move |_| {
                        let Some(root_id) = seed_artifact_id else { return; };
                        #[cfg(not(target_arch = "wasm32"))]
                        let vault_snapshot_toolbar = vault_signal_toolbar.read().clone();
                        crate::plugins::artifact::view::spawn_cascade(
                            root_id,
                            note_repo.clone(),
                            project_repo.clone(),
                            persistence.clone(),
                            plugin.clone(),
                            chat_session_repo.clone(),
                            chat_message_repo.clone(),
                            #[cfg(not(target_arch = "wasm32"))]
                            vault_snapshot_toolbar,
                            &mut note_version_setter,
                            &mut chat_session_version_setter,
                            &mut active_session_setter,
                            &mut active_scope_setter,
                            None,
                            crate::plugins::artifact::cascade::RunMode::Full,
                        );
                    }
                },
                "\u{25B6} Run"
            }
            button {
                r#type: "button",
                class: if current_step_mode {
                    "operon-workflow-toolbar-button operon-workflow-toolbar-button-active"
                } else {
                    "operon-workflow-toolbar-button"
                },
                "data-testid": "workflow-toolbar-step-mode",
                "aria-pressed": if current_step_mode { "true" } else { "false" },
                title: if current_step_mode {
                    "Step mode ON — cascade pauses after every skill so each stage can be reviewed independently. Click to disable and run continuously."
                } else {
                    "Continuous run — cascade only pauses at cascade_stop checkpoints (01b, 02b, etc.). Click to enable per-skill pauses."
                },
                onclick: on_toggle_step_mode,
                if current_step_mode { "\u{23F8} Step mode" } else { "\u{27A1} Continuous" }
            }
            button {
                r#type: "button",
                class: "operon-workflow-toolbar-button",
                "data-testid": "workflow-toolbar-auto-arrange",
                title: "Auto-arrange nodes left-to-right by topological rank, ordered to minimize edge crossings",
                onclick: on_auto_arrange,
                "\u{2630} Auto-arrange"
            }
            button {
                r#type: "button",
                class: "operon-workflow-toolbar-button",
                "data-testid": "workflow-toolbar-expand-all",
                title: "Expand every node — show every level of the cascade at once",
                onclick: on_expand_all,
                "\u{25BE} Expand all"
            }
            button {
                r#type: "button",
                class: "operon-workflow-toolbar-button",
                "data-testid": "workflow-toolbar-collapse-all",
                title: "Collapse every node — show only the root level",
                onclick: on_collapse_all,
                "\u{25B8} Collapse all"
            }
            {
                let mut hide_cross = props.hide_cross_phase_edges;
                let is_hiding = *hide_cross.read();
                rsx! {
                    button {
                        r#type: "button",
                        class: if is_hiding {
                            "operon-workflow-toolbar-button operon-workflow-toolbar-button-active"
                        } else {
                            "operon-workflow-toolbar-button"
                        },
                        "data-testid": "workflow-toolbar-toggle-cross-phase",
                        "aria-pressed": if is_hiding { "true" } else { "false" },
                        title: if is_hiding {
                            "Hiding edges that cross phase boundaries. Click to show every cross-phase dependency."
                        } else {
                            "Showing every edge, including cross-phase dependencies. Click to hide cross-phase edges and focus on one phase at a time."
                        },
                        onclick: move |_| {
                            let v = *hide_cross.peek();
                            hide_cross.set(!v);
                        },
                        if is_hiding { "\u{29C9} 1 phase" } else { "\u{29C9} All phases" }
                    }
                }
            }
            button {
                r#type: "button",
                class: if *json_visible.read() {
                    "operon-workflow-toolbar-button operon-workflow-toolbar-button-active"
                } else {
                    "operon-workflow-toolbar-button"
                },
                "data-testid": "workflow-toolbar-toggle-json",
                "aria-pressed": if *json_visible.read() { "true" } else { "false" },
                title: "Show / hide the raw JSON tree pane",
                onclick: move |_| {
                    let v = *json_visible.peek();
                    json_visible.set(!v);
                },
                if *json_visible.read() { "{{}} Hide JSON" } else { "{{}} Show JSON" }
            }
        }
    }
}

// ===== helpers =====

#[derive(Clone, PartialEq)]
struct NodeRender {
    id: NodeId,
    x: f64,
    y: f64,
    label: String,
    status: NodeStatus,
    /// Cascade-visualization marker propagated from `Node::is_artifact_snapshot`.
    /// Drives a CSS-class branch in `status_class` so the canvas tile
    /// looks visibly different from an editable skill node.
    is_artifact_snapshot: bool,
    /// Kind badge ("Epic" / "Story" / etc.) when this is an artifact
    /// snapshot. None for skill nodes.
    kind_label: Option<String>,
    /// `true` when this node has at least one outgoing edge in the
    /// underlying graph. Drives the chevron toggle visibility — a leaf
    /// node has nothing to expand / collapse.
    has_children: bool,
    /// `true` when the user has clicked the chevron and the node's
    /// children are visible. Default `false`; new nodes (paste, seed
    /// pipeline, cascade output) start collapsed.
    is_expanded: bool,
}

#[derive(Clone, PartialEq)]
struct EdgeRender {
    id: EdgeId,
    /// Source / target node ids — kept on the render-side projection so
    /// hover handlers can highlight the connecting nodes without a
    /// round-trip through the underlying graph.
    from_id: NodeId,
    to_id: NodeId,
    from_x: f64,
    from_y: f64,
    to_x: f64,
    to_y: f64,
    /// Cascade-visualization marker. `Some("depends_on")` is rendered
    /// in amber (CSS class `operon-workflow-edge-depends-on`) so the
    /// user can distinguish parent/child structure from inter-sibling
    /// dependencies.
    edge_kind: Option<String>,
    /// 0-based index of this edge within its source node's outgoing
    /// edges, used to pick a fan-color (`operon-workflow-edge-fan-N`).
    /// Stable across collapse / expand because we compute it from the
    /// full graph, not the visible subset.
    fan_index: usize,
}

fn parse_or_default(content: &str) -> WorkflowGraph {
    if content.trim().is_empty() {
        return WorkflowGraph::new();
    }
    serde_json::from_str(content).unwrap_or_default()
}

/// Compute the effective list of source artifact ids for a node when
/// it runs in the topo loop (Phase 6 fan-out). Three cases:
///
/// 1. Node has an explicit `source_artifact_id` (typically auto-set
///    when wired downstream of an artifact-snapshot tile, or set in
///    the inspector). Returns that single id — node runs once.
/// 2. Node has no explicit source but at least one upstream skill
///    node has `cached_produced_artifact_ids` populated. Returns the
///    union of all upstreams' ids — node fans out, running once per
///    upstream-produced artifact. Order is upstream-iteration order
///    so the canvas/topo sequence is preserved.
/// 3. Neither of the above. Returns an empty Vec — caller should
///    skip the node (no source to consume).
///
/// The fan-out only kicks in for nodes WITHOUT an explicit source.
/// Aggregator skills (e.g. `aggregate: task`, `input_kind:
/// requirements`) anchored to the Requirements root keep their
/// single-source semantic even when downstream of a many-output
/// upstream — their explicit `source_artifact_id` overrides fan-out.
fn effective_sources_for_node(graph: &WorkflowGraph, node_id: NodeId) -> Vec<Uuid> {
    if let Some(node) = graph.nodes.get(&node_id) {
        if let Some(explicit) = node.source_artifact_id {
            return vec![explicit];
        }
    } else {
        return Vec::new();
    }

    let mut seen: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
    let mut sources: Vec<Uuid> = Vec::new();
    for edge in &graph.edges {
        if edge.to != node_id {
            continue;
        }
        let Some(upstream) = graph.nodes.get(&edge.from) else {
            continue;
        };
        // Artifact-snapshot upstream: the tile's `artifact_ref` IS
        // the source. (Phase 2 normally auto-derives this onto the
        // downstream node, but a workflow JSON edited by hand may
        // skip that step.)
        if upstream.is_artifact_snapshot {
            if let Some(art) = upstream.artifact_ref {
                if seen.insert(art) {
                    sources.push(art);
                }
            }
            continue;
        }
        // Skill upstream: every artifact this upstream produced is
        // a separate source for the downstream node — implicit
        // fan-out.
        for art in &upstream.cached_produced_artifact_ids {
            if seen.insert(*art) {
                sources.push(*art);
            }
        }
    }
    sources
}

/// Pull the cascade root artifact id out of a workflow graph: the
/// artifact-snapshot tile that has no inbound `parent_child` edge.
/// `Cascade: <root>` notes (auto-generated by `CascadeGraphWriter`)
/// always have exactly one such tile — the artifact the user clicked
/// ▶ on. Returns `None` for hand-built workflow graphs that aren't
/// cascade snapshots (no artifact-snapshot tiles at all). When the
/// shape is ambiguous (multiple unrooted artifact tiles), prefer the
/// node with the smallest `position.1` (top-most on the canvas), which
/// matches the writer's level-0 layout convention
/// (`cascade_graph.rs:594-617`).
fn find_cascade_root_artifact(graph: &WorkflowGraph) -> Option<Uuid> {
    let mut has_parent_edge: std::collections::HashSet<NodeId> =
        std::collections::HashSet::new();
    for e in &graph.edges {
        let kind = e.edge_kind.as_deref().unwrap_or("");
        if kind == "parent_child" {
            has_parent_edge.insert(e.to);
        }
    }
    let mut candidates: Vec<&Node> = graph
        .nodes
        .values()
        .filter(|n| n.is_artifact_snapshot && n.artifact_ref.is_some())
        .filter(|n| !has_parent_edge.contains(&n.id))
        .collect();
    candidates.sort_by(|a, b| {
        a.position
            .1
            .partial_cmp(&b.position.1)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates.first().and_then(|n| n.artifact_ref)
}

fn serialize(graph: &WorkflowGraph) -> String {
    serde_json::to_string_pretty(graph).unwrap_or_else(|_| "{}".to_string())
}

/// Maximum visible characters in a node's title row before it gets a
/// trailing ellipsis. Tuned for `NODE_W = 260` + the 14px monospace
/// title font: keeps the card visually contained without truncating
/// the meaningful prefix (e.g. `epic-01-account-and-personalisation`
/// → `epic-01-account-and-personali…`).
const NODE_TITLE_MAX_CHARS: usize = 30;

fn truncate_for_card(s: &str) -> String {
    let count = s.chars().count();
    if count <= NODE_TITLE_MAX_CHARS {
        return s.to_string();
    }
    let mut out: String = s.chars().take(NODE_TITLE_MAX_CHARS - 1).collect();
    out.push('\u{2026}');
    out
}

/// Derive icon + body + CSS modifier from a node's live-state snapshot.
/// Priority: error > active_tool > thinking > last_write. The returned
/// css modifier ("error" / "tool" / "thinking" / "write" / "idle") is
/// appended to the base `operon-workflow-node-live` class so styles can
/// differentiate (e.g. red for error, dim for idle).
fn describe_live_state(s: &NodeLiveState) -> (&'static str, String, &'static str) {
    if let Some(err) = s.last_error.as_ref() {
        return ("\u{2716}", truncate_for_card(err.as_str()), "error");
    }
    if let Some(tool) = s.active_tool.as_ref() {
        return ("\u{25B8}", truncate_for_card(tool.summary.as_str()), "tool");
    }
    if s.thinking {
        return ("\u{2728}", "thinking\u{2026}".to_string(), "thinking");
    }
    if let Some(f) = s.last_write_file.as_ref() {
        return ("\u{1F4DD}", truncate_for_card(f.as_str()), "write");
    }
    ("\u{00B7}", String::new(), "idle")
}

fn node_label(n: &Node, skill_titles: &HashMap<Uuid, String>) -> String {
    let raw = if n.is_artifact_snapshot {
        // Cascade snapshot: prefer the artifact's cached title (e.g.
        // "epic-01-realtime-collaboration"). When that's missing —
        // e.g. on the seed root or after a snapshot was created
        // before titles were cached — look up the referenced note's
        // current title in the project-wide map. Last-resort fallback
        // is "<kind> <id-prefix>" so the tile is never blank.
        if let Some(title) = n.artifact_title.as_ref() {
            title.clone()
        } else if let Some(title) = n
            .artifact_ref
            .and_then(|aref| skill_titles.get(&aref))
        {
            title.clone()
        } else {
            let head: String = n
                .artifact_ref
                .map(|id| id.to_string().chars().take(8).collect())
                .unwrap_or_default();
            let kind = n
                .artifact_kind_label
                .clone()
                .unwrap_or_else(|| "Artifact".into());
            format!("{kind} {head}")
        }
    } else if let Some(title) = skill_titles.get(&n.skill_note_id) {
        // Skill node: prefer the resolved skill title.
        title.clone()
    } else {
        // Fall back to the UUID short prefix when the skill row isn't
        // in the lookup map (e.g. read-only WorkflowView, or skill
        // note was deleted).
        let id = n.skill_note_id.to_string();
        let head: String = id.chars().take(8).collect();
        format!("skill {head}")
    };
    truncate_for_card(&raw)
}

fn status_class(s: &NodeStatus, is_artifact_snapshot: bool) -> &'static str {
    if is_artifact_snapshot {
        // Cascade-snapshot tiles use a separate base class so they
        // can be styled distinctly (smaller, dimmer, kind-tinted)
        // without colliding with skill-node status colors. Status
        // is still surfaced for completeness.
        return match s {
            NodeStatus::Fresh => "operon-workflow-node operon-workflow-node-artifact",
            NodeStatus::Dirty => "operon-workflow-node operon-workflow-node-artifact operon-workflow-node-dirty",
            NodeStatus::Running => "operon-workflow-node operon-workflow-node-artifact operon-workflow-node-running",
            NodeStatus::Error(_) => "operon-workflow-node operon-workflow-node-artifact operon-workflow-node-error",
        };
    }
    match s {
        NodeStatus::Fresh => "operon-workflow-node operon-workflow-node-fresh",
        NodeStatus::Dirty => "operon-workflow-node operon-workflow-node-dirty",
        NodeStatus::Running => "operon-workflow-node operon-workflow-node-running",
        NodeStatus::Error(_) => "operon-workflow-node operon-workflow-node-error",
    }
}

fn status_label(s: &NodeStatus) -> &'static str {
    match s {
        NodeStatus::Fresh => "fresh",
        NodeStatus::Dirty => "dirty",
        NodeStatus::Running => "running…",
        NodeStatus::Error(_) => "error",
    }
}

/// Auto-layout: lay nodes out on a deterministic grid for any whose
/// `position` is `(0, 0)`. Sort by NodeId for stability.
fn layout(graph: &WorkflowGraph) -> HashMap<NodeId, (f64, f64)> {
    let mut placed: Vec<NodeId> = graph.nodes.keys().copied().collect();
    placed.sort();
    let mut out = HashMap::new();
    let cols = 3usize;
    for (i, id) in placed.iter().enumerate() {
        if let Some(node) = graph.nodes.get(id) {
            if node.position != (0.0, 0.0) {
                continue;
            }
            let row = i / cols;
            let col = i % cols;
            let x = 40.0 + (col as f64) * (NODE_W + 60.0);
            let y = 40.0 + (row as f64) * (NODE_H + 60.0);
            out.insert(*id, (x, y));
        }
    }
    out
}

/// Prioritized-backlog artifacts summarize a particular level of the
/// cascade — `prioritized-backlog-epics`, `…-stories`, etc. The
/// auto-arrange uses this to drop each backlog into the same column
/// as the level it covers, instead of stacking every backlog in a
/// single rightmost column. Returns the matching kind priority
/// (Epic = 2, Story = 4, Task = 5, Plan = 6), or `None` when the
/// title doesn't follow the convention.
fn infer_backlog_level(title: &str) -> Option<i32> {
    let lower = title.to_lowercase();
    // Order matters only for cosmetic precision: longest/most specific
    // word first so a custom name like "epic-story-rollup" doesn't
    // misroute. In practice the seed-pipeline names are clean and the
    // first hit wins.
    if lower.contains("epic") {
        return Some(2);
    }
    // `stor` covers both `story` and `stories`. Returns the *raw*
    // canonical priority — `auto_arrange` collapses Plan→Story for
    // the column index via `column_kind`, but the raw value is what
    // we want for the within-column sort key.
    if lower.contains("stor") {
        return Some(4);
    }
    if lower.contains("task") {
        return Some(5);
    }
    if lower.contains("plan") {
        return Some(6);
    }
    None
}

/// Canonical SDLC waterfall priority for an `artifact_kind_label` value
/// (matches the `ArtifactKind::display_name()` strings in
/// `plugins/artifact/frontmatter.rs:98-113`). Lower numbers are
/// upstream / earlier in the cascade; nodes with the same priority go
/// in the same horizontal swim lane.
///
/// `architecture` shares the Epic column (2) because both are
/// depth-1 children of the master requirements — fan-out siblings,
/// not a downstream chain. Without this, architecture defaulted to
/// the catch-all "12" and rendered to the right of every other kind,
/// which visually implied `Epic → Architecture` was a real
/// dependency.
fn kind_priority(label: Option<&str>) -> i32 {
    let key = label.map(|l| l.to_lowercase());
    match key.as_deref() {
        None | Some("artifact") => 0, // root seed / generic
        Some("requirements") | Some("master_requirement") => 1,
        Some("epic") | Some("architecture") => 2,
        Some("story") => 4,
        Some("task") => 5,
        Some("plan") => 6,
        Some("implementation") => 7,
        Some("test cases") => 8,
        Some("test results") => 9,
        Some("summary") => 10,
        Some("prioritized backlog") => 11,
        Some(_) => 12, // Other / unknown custom kind
    }
}

/// Maps a canonical kind priority (the value returned by
/// `kind_priority`) to its visual *column index* for `auto_arrange`.
/// Plan (6) collapses into Story's column (4) because Plans are the
/// technical expansion of a Story — the cascade emits one Plan per
/// Story, so visually grouping them keeps the level's items together
/// instead of leaving Plans in a mostly-empty column. Within the
/// merged column the within-column sort uses the *raw* priority, so
/// Stories (4) still appear above Plans (6) in reading order.
/// Every other kind keeps its own column.
fn column_kind(p: i32) -> i32 {
    match p {
        6 => 4, // Plan → Story column
        _ => p,
    }
}

/// Compute the visible-node set for the current expand state. A node
/// is visible iff it's a root (no incoming edges) OR it can be reached
/// from a root through a chain of expanded nodes. This lets the user
/// drill into the cascade one level at a time — clicking the chevron
/// on a parent surfaces only its direct children, not the whole
/// subtree, matching the "expand each level" demo flow.
///
/// Cycle-safe: BFS via `BTreeSet::insert` returning bool stops re-
/// traversal of already-visible nodes.
fn compute_visible(
    graph: &WorkflowGraph,
    expanded: &std::collections::BTreeSet<NodeId>,
) -> std::collections::BTreeSet<NodeId> {
    let mut visible = std::collections::BTreeSet::new();
    let mut indeg: HashMap<NodeId, usize> =
        graph.nodes.keys().map(|id| (*id, 0usize)).collect();
    for e in &graph.edges {
        if indeg.contains_key(&e.from) && indeg.contains_key(&e.to) {
            *indeg.entry(e.to).or_insert(0) += 1;
        }
    }
    // Roots: indegree 0 in the graph. Always visible.
    let mut frontier: Vec<NodeId> = indeg
        .iter()
        .filter(|(_, &c)| c == 0)
        .map(|(id, _)| *id)
        .collect();
    for r in &frontier {
        visible.insert(*r);
    }
    // BFS, but only follow `from -> to` edges when `from` is expanded.
    while let Some(n) = frontier.pop() {
        if !expanded.contains(&n) {
            continue;
        }
        for e in graph.edges.iter().filter(|e| e.from == n) {
            if !graph.nodes.contains_key(&e.to) {
                continue;
            }
            if visible.insert(e.to) {
                frontier.push(e.to);
            }
        }
    }
    visible
}

/// SDLC waterfall layout — **kind drives the column**, not topological
/// rank. All Epics share one column, all Features share the next, all
/// Stories the next, etc. The canonical priority order
/// (`kind_priority`) sets the left-to-right sequence; only kinds that
/// actually appear in the graph get a column, so empty levels don't
/// leave gaps. Within a column, nodes are stacked vertically and
/// sorted alphabetically by `artifact_title` for stable ordering.
fn auto_arrange(graph: &WorkflowGraph) -> WorkflowGraph {
    let mut next = graph.clone();
    if next.nodes.is_empty() {
        return next;
    }

    // Snapshot per-node placement keys. Stored as a 3-tuple so the
    // column index and within-column sort key can diverge:
    //   - `column` decides which vertical band the node lands in.
    //     Plan→Story column collapse and backlog→owner-column routing
    //     both happen here.
    //   - `sort_key` keeps the raw `kind_priority`, so Stories (4)
    //     still appear above Plans (6) inside the merged column, and
    //     PrioritizedBacklog (11) always falls to the bottom.
    //   - `title` is the alphabetical tiebreaker.
    let snapshot: HashMap<NodeId, (i32, i32, String)> = next
        .nodes
        .iter()
        .map(|(id, n)| {
            let title = n.artifact_title.clone().unwrap_or_default();
            let raw = kind_priority(n.artifact_kind_label.as_deref());
            // Sort key: keep raw — backlogs (11) sort to the bottom,
            // Stories (4) above Plans (6) within the merged column.
            let sort_key = raw;
            // Column key: route backlogs to their inferred level's
            // column (parsed from the title suffix — e.g.
            // `prioritized-backlog-stories` → Story level), then
            // collapse Plan→Story for the merged column.
            let owner = if raw == 11 {
                infer_backlog_level(&title).unwrap_or(raw)
            } else {
                raw
            };
            let column = column_kind(owner);
            (*id, (column, sort_key, title))
        })
        .collect();

    // Compress the column index: only include kinds present in the
    // graph, in canonical priority order. So a workflow with only
    // Epics, Features, Stories gets columns 0/1/2 — no holes for
    // missing Tasks/Plans.
    let mut present_kinds: Vec<i32> = snapshot
        .values()
        .map(|(c, _, _)| *c)
        .collect::<std::collections::BTreeSet<i32>>()
        .into_iter()
        .collect();
    present_kinds.sort();
    let kind_to_col: HashMap<i32, usize> = present_kinds
        .iter()
        .enumerate()
        .map(|(i, k)| (*k, i))
        .collect();

    // Group nodes into columns by kind. Each column is one homogeneous
    // band — all the Epics, all the Features, etc.
    let mut columns: Vec<Vec<NodeId>> = vec![Vec::new(); present_kinds.len()];
    for (id, (c, _, _)) in &snapshot {
        if let Some(col) = kind_to_col.get(c) {
            columns[*col].push(*id);
        }
    }
    // Within-column ordering. Process columns left-to-right so each
    // node's "primary ancestor slot" is already known by the time we
    // need it. Sort key per node, in order of importance:
    //
    //   1. Ancestor's slot in some earlier column. BFS up incoming
    //      edges, take the first parent that already has a slot
    //      assigned. Children of feature-01 cluster together, then
    //      children of feature-02, etc.
    //   2. Raw `sort_key` — Stories (4) above Plans (6) above
    //      Backlogs (11) within the same parent group.
    //   3. Title — alphabetical within each (parent, kind) bucket.
    //   4. Node id — deterministic when titles collide.
    //
    // Nodes whose BFS finds no ancestor in any earlier column (e.g.
    // backlog-features parented straight to the root) sort to the end
    // of their column via `usize::MAX`, which keeps backlogs as the
    // tail of every level visually.
    // Records each node's primary ancestor (if any) so the placement
    // pass below can center child groups on the parent's Y.
    let mut primary_parent: HashMap<NodeId, Option<NodeId>> = HashMap::new();
    let mut slot_in_column: HashMap<NodeId, usize> = HashMap::new();
    for col_idx in 0..columns.len() {
        // Precompute the sort key for every node in this column up
        // front. The key includes the ancestor's (col, slot) computed
        // by walking incoming edges via BFS — using a precomputed
        // HashMap avoids capturing closures whose borrows would clash
        // with `columns[col_idx].sort_by` taking a `&mut Vec`.
        let mut keys: HashMap<NodeId, (usize, usize, i32, String)> =
            HashMap::with_capacity(columns[col_idx].len());
        for id in columns[col_idx].iter() {
            // BFS up `incoming(*n)` for the first parent already
            // placed in an earlier column.
            let anc: Option<NodeId> = {
                let mut visited: std::collections::HashSet<NodeId> =
                    std::collections::HashSet::new();
                visited.insert(*id);
                let mut frontier: Vec<NodeId> = vec![*id];
                let mut found: Option<NodeId> = None;
                'outer: while !frontier.is_empty() {
                    let mut next_frontier: Vec<NodeId> = Vec::new();
                    for n in &frontier {
                        for e in next.edges.iter().filter(|e| e.to == *n) {
                            if !visited.insert(e.from) {
                                continue;
                            }
                            if slot_in_column.contains_key(&e.from)
                                && snapshot
                                    .get(&e.from)
                                    .and_then(|(c, _, _)| kind_to_col.get(c))
                                    .is_some()
                            {
                                found = Some(e.from);
                                break 'outer;
                            }
                            next_frontier.push(e.from);
                        }
                    }
                    frontier = next_frontier;
                }
                found
            };
            let (anc_col, anc_slot) = anc
                .and_then(|aid| {
                    let col = snapshot
                        .get(&aid)
                        .and_then(|(c, _, _)| kind_to_col.get(c).copied());
                    let slot = slot_in_column.get(&aid).copied();
                    match (col, slot) {
                        (Some(c), Some(s)) => Some((c, s)),
                        _ => None,
                    }
                })
                .unwrap_or((usize::MAX, usize::MAX));
            let (sort_key, title) = snapshot
                .get(id)
                .map(|(_, s, t)| (*s, t.clone()))
                .unwrap_or((0, String::new()));
            keys.insert(*id, (anc_col, anc_slot, sort_key, title));
            primary_parent.insert(*id, anc);
        }
        let empty_key = (usize::MAX, usize::MAX, 0i32, String::new());
        columns[col_idx].sort_by(|a, b| {
            let ka = keys.get(a).unwrap_or(&empty_key);
            let kb = keys.get(b).unwrap_or(&empty_key);
            ka.cmp(kb).then_with(|| a.cmp(b))
        });
        // Write slots for this column so the next column's primary-
        // ancestor lookup can find these nodes.
        for (slot, id) in columns[col_idx].iter().enumerate() {
            slot_in_column.insert(*id, slot);
        }
    }

    // Generous demo-friendly spacing. The big COL_GAP keeps long
    // titles visually inside their own column even with the
    // auto-truncate ellipsis at ~30 chars.
    const COL_GAP: f64 = 380.0;
    const ROW_GAP: f64 = 44.0;
    const BASE_X: f64 = 60.0;
    const BASE_Y: f64 = 60.0;
    let col_step = NODE_W + COL_GAP;
    let row_step = NODE_H + ROW_GAP;

    // Placement pass — *bottom-up centroid*. We process columns
    // RIGHT-TO-LEFT: the rightmost column is laid out top-down by
    // its existing within-column sort, then each preceding column
    // places each node at the *average Y of its children* (children
    // = direct outgoing-edge targets in any later column). Parents
    // land at the centroid of their children, so the connector lines
    // come out roughly horizontal — minimal curvature, the demo look.
    //
    // Within a column, after computing each node's ideal Y, we sort
    // by ideal Y (with the existing sort key as tiebreaker) and
    // sweep top-down enforcing `row_step` spacing — so two parents
    // whose ideal Ys overlap get nudged apart while preserving order.
    let mut node_y: HashMap<NodeId, f64> = HashMap::new();
    if columns.is_empty() {
        next.version = next.version.saturating_add(1);
        return next;
    }
    let last_col = columns.len() - 1;
    // Pre-index direct children for fast lookup. Stored as
    // `outgoing_targets[from] = Vec<to>`, scoped to nodes that
    // actually exist in the graph (defensive against dangling refs
    // mid-edit).
    let mut outgoing_targets: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for e in &next.edges {
        if next.nodes.contains_key(&e.from) && next.nodes.contains_key(&e.to) {
            outgoing_targets.entry(e.from).or_default().push(e.to);
        }
    }

    // Step 1 — rightmost column: stack from BASE_Y in within-column
    // sort order. This anchors the bottom-up cascade.
    {
        let x = BASE_X + last_col as f64 * col_step;
        for (slot, id) in columns[last_col].iter().enumerate() {
            let y = BASE_Y + slot as f64 * row_step;
            node_y.insert(*id, y);
            if let Some(node) = next.nodes.get_mut(id) {
                node.position = (x, y);
            }
        }
    }

    // Step 2 — every column to the LEFT of the rightmost, in
    // right-to-left order. Each node's ideal Y is the average of its
    // already-placed children's Ys. Nodes without placed children
    // (e.g. backlog-epics whose children sit elsewhere) get a
    // fallback Y that just appends them after the column's last
    // placed node, so they tail the column without crossing siblings.
    for col_idx in (0..last_col).rev() {
        let x = BASE_X + col_idx as f64 * col_step;
        if columns[col_idx].is_empty() {
            continue;
        }

        // Compute each node's ideal Y from its children.
        let mut ideal: HashMap<NodeId, Option<f64>> = HashMap::new();
        for id in &columns[col_idx] {
            let children = outgoing_targets.get(id);
            let avg = children.and_then(|kids| {
                let ys: Vec<f64> =
                    kids.iter().filter_map(|c| node_y.get(c).copied()).collect();
                if ys.is_empty() {
                    None
                } else {
                    Some(ys.iter().copied().sum::<f64>() / ys.len() as f64)
                }
            });
            ideal.insert(*id, avg);
        }

        // Sort the column by ideal Y (with the existing ordering as
        // tiebreaker). Nodes with no centroid sort to the bottom.
        let original_order: HashMap<NodeId, usize> = columns[col_idx]
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, i))
            .collect();
        let mut ordered: Vec<NodeId> = columns[col_idx].clone();
        ordered.sort_by(|a, b| {
            let ya = ideal.get(a).and_then(|o| *o);
            let yb = ideal.get(b).and_then(|o| *o);
            match (ya, yb) {
                (Some(va), Some(vb)) => va
                    .partial_cmp(&vb)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| {
                        original_order
                            .get(a)
                            .cmp(&original_order.get(b))
                    }),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => original_order.get(a).cmp(&original_order.get(b)),
            }
        });
        columns[col_idx] = ordered;

        // Sweep top-down: clamp each node's Y to (ideal, prev + step).
        let mut prev_y: Option<f64> = None;
        for id in &columns[col_idx] {
            let proposed = ideal
                .get(id)
                .and_then(|o| *o)
                .unwrap_or_else(|| match prev_y {
                    Some(p) => p + row_step,
                    None => BASE_Y,
                });
            let y = match prev_y {
                Some(p) => proposed.max(p + row_step),
                None => proposed.max(BASE_Y),
            };
            node_y.insert(*id, y);
            if let Some(node) = next.nodes.get_mut(id) {
                node.position = (x, y);
            }
            prev_y = Some(y);
        }
    }
    next.version = next.version.saturating_add(1);
    next
}

/// Remove `node_id` and any incident edges. Bumps the graph version
/// so cascade-runs see a different fingerprint.
fn remove_node(graph: &WorkflowGraph, node_id: NodeId) -> WorkflowGraph {
    let mut next = graph.clone();
    next.nodes.remove(&node_id);
    next.edges
        .retain(|e| e.from != node_id && e.to != node_id);
    next.version = next.version.saturating_add(1);
    next
}

/// Append a `from -> to` edge if it would be safe (no self-loop, not
/// a duplicate of an existing edge). Returns `None` when the edge is
/// rejected so the caller can skip the apply round-trip.
///
/// Side effect: when `from` is an artifact-snapshot tile (carries
/// `artifact_ref`) and `to` is a skill node without a
/// `source_artifact_id` yet, auto-populate `to.source_artifact_id`
/// with the upstream tile's `artifact_ref`. This is what binds a
/// freshly-wired skill node to its consumed artifact so the
/// workflow-canvas executor can route the run through
/// `runner::run_skill_on_source` (Phase 4) and inherit aggregate /
/// inherit / cascade_stop behavior.
fn add_edge_if_new(graph: &WorkflowGraph, from: NodeId, to: NodeId) -> Option<WorkflowGraph> {
    if from == to {
        return None;
    }
    if graph.edges.iter().any(|e| e.from == from && e.to == to) {
        return None;
    }
    let mut next = graph.clone();
    next.edges.push(Edge {
        id: Uuid::new_v4(),
        from,
        from_socket: "default".into(),
        to,
        to_socket: "default".into(),
        edge_kind: None,
    });

    // Auto-derive `source_artifact_id` on the destination skill node
    // when the upstream is an artifact-snapshot tile. Only fills when
    // the destination doesn't already have a source set (preserves
    // explicit user overrides).
    let upstream_artifact_ref = next
        .nodes
        .get(&from)
        .filter(|n| n.is_artifact_snapshot)
        .and_then(|n| n.artifact_ref);
    if let Some(art_id) = upstream_artifact_ref {
        if let Some(dest) = next.nodes.get_mut(&to) {
            if !dest.is_artifact_snapshot && dest.source_artifact_id.is_none() {
                dest.source_artifact_id = Some(art_id);
            }
        }
    }

    next.version = next.version.saturating_add(1);
    Some(next)
}

/// Drop the edge with the given id. No-op (returns the original) when
/// the id isn't present.
fn remove_edge(graph: &WorkflowGraph, edge_id: EdgeId) -> WorkflowGraph {
    let mut next = graph.clone();
    next.edges.retain(|e| e.id != edge_id);
    next.version = next.version.saturating_add(1);
    next
}

/// Pick a non-overlapping drop position for a freshly-added node.
/// Stacks new tiles diagonally to the bottom-right of whatever is
/// already on the canvas so picker drops don't pile on top of each
/// other (or each other plus existing seeded nodes). Strides match
/// `seed-pipeline`'s `SEED_X_STRIDE`/`SEED_Y_STRIDE` so the visual
/// gap rule is consistent across entry points.
fn next_drop_position(graph: &WorkflowGraph) -> (f64, f64) {
    const DROP_X_STRIDE: f64 = 340.0;
    const DROP_Y_STRIDE: f64 = 290.0;
    if graph.nodes.is_empty() {
        return (40.0, 40.0);
    }
    let max_x = graph
        .nodes
        .values()
        .map(|n| n.position.0)
        .fold(0.0_f64, f64::max);
    let max_y = graph
        .nodes
        .values()
        .map(|n| n.position.1)
        .fold(0.0_f64, f64::max);
    (max_x + DROP_X_STRIDE, max_y + DROP_Y_STRIDE)
}

/// Insert a fresh node referencing `skill_note_id` into the JSON-text
/// representation of a `WorkflowGraph`. Tolerates malformed input —
/// returns the input unchanged in that case so the user's pending
/// edits aren't blown away.
fn append_node_to_graph(graph_text: &str, skill_note_id: Uuid) -> String {
    let mut graph: WorkflowGraph = match serde_json::from_str(graph_text) {
        Ok(g) => g,
        Err(_) if graph_text.trim().is_empty() => WorkflowGraph::new(),
        Err(_) => return graph_text.to_string(),
    };
    let id = Uuid::new_v4();
    let position = next_drop_position(&graph);
    let node = Node {
        id,
        skill_note_id,
        typed_fields: serde_json::Value::Null,
        extra_instructions: String::new(),
        position,
        cached_output_path: None,
        cached_input_hash: None,
        status: NodeStatus::Dirty,
        cached_output_note_id: None,
        is_artifact_snapshot: false,
        artifact_ref: None,
        artifact_kind_label: None,
        artifact_title: None,
        source_artifact_id: None,
        cached_produced_artifact_ids: Vec::new(),
    };
    graph.nodes.insert(id, node);
    graph.version = graph.version.saturating_add(1);
    serialize(&graph)
}

/// Drop a read-only artifact-snapshot tile onto the canvas. Mirror of
/// `append_node_to_graph` but builds the artifact-tile shape: no
/// `skill_note_id`, `is_artifact_snapshot: true`, `artifact_ref` set,
/// `artifact_title` cached for badge rendering. Lets the user wire
/// this tile to a downstream skill node — `add_edge_if_new` then
/// auto-derives `source_artifact_id` on the skill, which unlocks the
/// SDLC executor route (`run_one_node_sdlc`) on the next "Run all
/// dirty". `artifact_kind_label` is left None here; the canvas's
/// reactive load loop fills it in on the next paint by reading the
/// artifact body's frontmatter (same path the cascade-driven tiles
/// use, so kind badges stay consistent across orchestrators).
fn append_artifact_tile_to_graph(
    graph_text: &str,
    artifact_note_id: Uuid,
    artifact_title: &str,
) -> String {
    let mut graph: WorkflowGraph = match serde_json::from_str(graph_text) {
        Ok(g) => g,
        Err(_) if graph_text.trim().is_empty() => WorkflowGraph::new(),
        Err(_) => return graph_text.to_string(),
    };
    let id = Uuid::new_v4();
    let position = next_drop_position(&graph);
    let node = Node {
        id,
        skill_note_id: Uuid::nil(),
        typed_fields: serde_json::Value::Null,
        extra_instructions: String::new(),
        position,
        cached_output_path: None,
        cached_input_hash: None,
        status: NodeStatus::Fresh,
        cached_output_note_id: None,
        is_artifact_snapshot: true,
        artifact_ref: Some(artifact_note_id),
        artifact_kind_label: None,
        artifact_title: Some(artifact_title.to_string()),
        source_artifact_id: None,
        cached_produced_artifact_ids: Vec::new(),
    };
    graph.nodes.insert(id, node);
    graph.version = graph.version.saturating_add(1);
    serialize(&graph)
}

// ===== executor wiring =====

/// Spawn a cascade run for the workflow at `note_id_str`. Walks dirty
/// nodes in topological order, applying the mutated `WorkflowGraph`
/// back to the editor signal after every step so the canvas reflects
/// each node's lifecycle (Dirty → Running → Fresh / Error).
///
/// Currently unused: the toolbar's old "▶ Run all dirty" button was
/// replaced by the master-requirement Run button, and there is no
/// other caller. Kept around because the topo-by-dirty executor wiring
/// here is non-trivial and may be useful if hand-authored workflows
/// regain a runner in the future. Remove if it's still dead at the
/// next major cleanup.
#[allow(dead_code)]
fn spawn_run_cascade(
    note_id_str: String,
    mut graph: WorkflowGraph,
    apply_graph: Callback<WorkflowGraph>,
    // Rail signals plumbed from the component body. `try_consume_context`
    // inside this function (an event handler context) returns Signal
    // handles that aren't reliably bound to a live runtime scope — so
    // their `.set(...)` calls were silently dropped, leaving the rail
    // on the user's previously-active session. Passing the Signals
    // explicitly from the toolbar's `use_context` hook fixes that.
    mut active_session_signal: Signal<Option<Uuid>>,
    mut active_scope_signal: Signal<ChatScope>,
) {
    eprintln!(
        "operon: spawn_run_cascade called note_id={} nodes={} edges={} dirty={}",
        note_id_str,
        graph.nodes.len(),
        graph.edges.len(),
        graph.dirty_nodes().len(),
    );
    let note_repo = match try_consume_context::<LocalNoteRepo>() {
        Some(LocalNoteRepo(r)) => r,
        None => {
            eprintln!("operon: cascade BAIL — LocalNoteRepo context missing");
            return;
        }
    };
    let project_repo = match try_consume_context::<LocalProjectRepo>() {
        Some(LocalProjectRepo(r)) => r,
        None => {
            eprintln!("operon: cascade BAIL — LocalProjectRepo context missing");
            return;
        }
    };
    let plugin = match try_consume_context::<ClaudeCodePluginCtx>() {
        Some(ClaudeCodePluginCtx(p)) => p,
        None => {
            eprintln!("operon: cascade BAIL — ClaudeCodePluginCtx context missing");
            return;
        }
    };
    let persistence = match try_consume_context::<Arc<dyn Persistence>>() {
        Some(p) => p,
        None => {
            eprintln!("operon: cascade BAIL — Arc<dyn Persistence> context missing");
            return;
        }
    };
    let workflow_id = match Uuid::parse_str(&note_id_str) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("operon: cascade BAIL — workflow id parse: {e}");
            return;
        }
    };
    // Phase-2 plumbing: the LocalNoteVersion signal is what wakes up
    // the explorer to re-fetch when notes change. Capture it here
    // (component scope is required) and bump it after each upsert.
    // Missing context is a soft-fail — the cascade still runs, the
    // explorer just won't auto-refresh until something else bumps it.
    let note_version: Option<Signal<u64>> =
        try_consume_context::<LocalNoteVersion>().map(|v| v.0);
    // Phase-4 plumbing: rail-backed cascade session. All three pieces
    // are required for rail integration; if any are missing, the
    // cascade still runs but skips chat persistence (the old behavior).
    let session_repo_opt: Option<Arc<dyn operon_store::repos::ChatSessionRepository>> =
        try_consume_context::<ChatSessionRepo>().map(|r| r.0);
    let chat_repo_opt: Option<Arc<dyn operon_store::repos::ChatMessageRepository>> =
        try_consume_context::<ChatMessageRepo>().map(|r| r.0);
    let session_version: Option<Signal<u64>> =
        try_consume_context::<ChatSessionVersion>().map(|v| v.0);
    spawn(async move {
        eprintln!("operon: cascade async START workflow_id={workflow_id}");
        // 1. Resolve project + repo path.
        let Some((_legacy_session, repo_path)) =
            resolve_project_session(workflow_id, &note_repo, &project_repo)
        else {
            eprintln!(
                "operon: cascade BAIL — resolve_project_session returned None \
                 (project not found OR repo_path not bound)"
            );
            return;
        };
        let project_id_opt = note_repo.find_project_for_note(workflow_id).ok().flatten();
        // Phase-4: derive (and ensure) the cascade chat-session. The
        // session id replaces what the cascade used to pass directly as
        // `operon_session` (the workflow id), so plugin.bind_session
        // and run_node now talk to the rail-backed session. The id is
        // deterministic in `workflow_id` so re-runs keep using the
        // same row (no rail spam).
        let cascade_session_id = cascade_session_id_for(workflow_id);
        let mut transcript_sink: Option<CascadeTranscriptSink> = None;
        if let (Some(session_repo), Some(chat_repo), Some(project_id)) =
            (session_repo_opt.as_ref(), chat_repo_opt.as_ref(), project_id_opt)
        {
            let exists = matches!(session_repo.get(cascade_session_id), Ok(Some(_)));
            if !exists {
                let title = lookup_note_title(&note_repo, project_id, workflow_id)
                    .unwrap_or_else(|| "Workflow".to_string());
                let label = format!("Cascade: {title}");
                if let Err(e) = session_repo.create_with_id(
                    cascade_session_id,
                    ChatScope::Project(project_id),
                    &label,
                ) {
                    eprintln!("operon: cascade chat-session create failed: {e}");
                }
            }
            // Bubble the row to the top of the rail by touching it.
            let _ = session_repo.touch(cascade_session_id);
            if let Some(mut sig) = session_version {
                sig.with_mut(|v| *v = v.saturating_add(1));
            }
            transcript_sink = Some(CascadeTranscriptSink {
                chat_session_id: cascade_session_id,
                chat_repo: chat_repo.clone(),
            });
        }
        eprintln!(
            "operon: cascade resolved cascade_session={cascade_session_id} repo={} project_id={:?} \
             rail_persist={}",
            repo_path.display(),
            project_id_opt,
            transcript_sink.is_some(),
        );

        // M2 — refresh <repo>/.claude/CLAUDE.md with the project's
        // current SDLC inventory before any node runs, so the first
        // turn of every per-node session sees up-to-date context.
        // Failure is non-fatal: the cascade still runs without the doc.
        if let Some(project_id) = project_id_opt {
            let project_name = project_repo
                .get(project_id)
                .ok()
                .flatten()
                .map(|p| p.name)
                .unwrap_or_else(|| "Unknown project".to_string());
            if let Err(e) = crate::plugins::artifact::claude_context::write_project_claude_md(
                &note_repo,
                &persistence,
                project_id,
                &project_name,
                &repo_path,
            )
            .await
            {
                eprintln!(
                    "operon: cascade CLAUDE.md refresh failed (non-fatal): {e}"
                );
            } else {
                eprintln!(
                    "operon: cascade CLAUDE.md refreshed at {}/.claude/CLAUDE.md",
                    repo_path.display()
                );
            }
        }

        // M5 — no upfront `plugin.bind_session(cascade_session_id, …)`
        // here. Each node mints its own fresh `operon_session` inside
        // the source-loop below so its `claude` subprocess starts
        // without `--resume`, eliminating reasoning bleed from prior
        // nodes. The cascade-wide `cascade_session_id` is only used
        // for the rail (chat_session row + transcript persistence).

        // Switch the companion's rail to the cascade's session so the
        // user sees the streaming transcript, "Claude is thinking…"
        // loader, and tool-call cards live as the cascade runs. The
        // rail tracks `cascade_session_id` (the chat_session row),
        // not any per-node operon_session, so the rail UX stays one
        // conversation per cascade even though each node spawns its
        // own claude subprocess.
        if let Some(project_id) = project_id_opt {
            active_scope_signal.set(ChatScope::Project(project_id));
        }
        active_session_signal.set(Some(cascade_session_id));
        eprintln!(
            "operon: cascade rail-switch active_session={cascade_session_id} \
             scope=Project({:?})",
            project_id_opt
        );

        // 2. Order the dirty subset.
        let order = match topo_order_dirty(&graph) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("operon: cascade BAIL — topo_order_dirty error: {e}");
                return;
            }
        };
        eprintln!("operon: cascade order={:?}", order);

        if order.is_empty() {
            eprintln!("operon: cascade no dirty nodes — nothing to run");
            return;
        }

        // Phase 5 resume: clear any prior Paused phase keyed on this
        // workflow's root artifact (or the workflow id fallback) so a
        // re-click of "Run all dirty" after the user approved a
        // checkpoint hides the pause banner before the next node
        // starts. Subsequent cascade_stop hits in this run will
        // re-Pause as expected.
        let resume_root =
            find_cascade_root_artifact(&graph).unwrap_or(workflow_id);
        crate::shell::companion_state::CASCADE_STATE.with_mut(|m| {
            m.remove(&resume_root);
        });

        // Outer loop: each node visited in topo order.
        // Inner loop (Phase 6): each source artifact in the effective
        // sources list. For SDLC nodes downstream of a many-output
        // upstream this fans out — N upstream artifacts → N
        // invocations of the downstream node, each parented to its
        // own source artifact.
        let mut paused_after_node = false;
        for node_id in order {
            if paused_after_node {
                break;
            }
            let sources = effective_sources_for_node(&graph, node_id);
            eprintln!(
                "operon: cascade visiting node {node_id} sources={}",
                sources.len()
            );
            if sources.is_empty() {
                eprintln!(
                    "operon: cascade node {node_id} has no source — skipping"
                );
                continue;
            }
            // Fan-out: run the node once per source. Accumulate
            // produced ids across invocations so downstream nodes
            // see the full set on their next visit.
            let mut accumulated_artifact_ids: Vec<Uuid> = Vec::new();
            let mut node_failed = false;
            for source_id in sources {
                // M5 — fresh per-node claude session. Mint a brand-new
                // UUID for every (node, source) iteration so claude's
                // `--resume` doesn't carry context across nodes.
                // `bind_session` is idempotent for a given UUID; per
                // node we generate a new one so it's effectively a
                // first-time bind. The transcript_sink keeps using
                // `cascade_session_id` so the rail sees one
                // conversation per cascade, not one per node.
                let node_operon_session = Uuid::new_v4();
                plugin.bind_session(node_operon_session, repo_path.clone());
                eprintln!(
                    "operon: cascade running node {node_id} (source={source_id}) \
                     fresh operon_session={node_operon_session}"
                );
                // Stamp the source on the node so run_one_node's
                // SDLC routing decision and run_one_node_sdlc both
                // see the right input.
                if let Some(n) = graph.nodes.get_mut(&node_id) {
                    n.source_artifact_id = Some(source_id);
                    n.status = NodeStatus::Running;
                }
                apply_graph.call(graph.clone());
                // `spawn_run_cascade` is dead code; pass a throwaway
                // outputs_base under the repo so it still typechecks
                // when someone resurrects it. Real callers go through
                // `spawn_run_node`, which derives the path from the vault.
                let outputs_base = repo_path.join("outputs");
                let result = run_one_node(
                    &mut graph,
                    node_id,
                    workflow_id,
                    node_operon_session,
                    &repo_path,
                    outputs_base,
                    plugin.clone(),
                    &persistence,
                    &note_repo,
                    transcript_sink.clone(),
                )
                .await;
                let mut node_paused = false;
                match result {
                    Err(e) => {
                        eprintln!("operon: cascade node {node_id} failed: {e}");
                        if let Some(n) = graph.nodes.get_mut(&node_id) {
                            n.status = NodeStatus::Error(format!("{e}"));
                        }
                        apply_graph.call(graph.clone());
                        node_failed = true;
                    }
                    Ok(NodeRunOk {
                        skill_note_id,
                        produced,
                        sdlc_artifact_ids,
                        cascade_stop_artifact,
                    }) => {
                        if let Some(ids) = sdlc_artifact_ids.as_ref() {
                            for id in ids {
                                if !accumulated_artifact_ids.contains(id) {
                                    accumulated_artifact_ids.push(*id);
                                }
                            }
                        }
                        eprintln!(
                            "operon: cascade node {node_id} (source={source_id}) \
                             produced={} sdlc={} stop={:?} (skill {skill_note_id})",
                            produced.len(),
                            sdlc_artifact_ids.as_ref().map(|v| v.len()).unwrap_or(0),
                            cascade_stop_artifact,
                        );
                        if sdlc_artifact_ids.is_some() {
                            if let Some(mut sig) = note_version {
                                sig.with_mut(|v| *v = v.saturating_add(1));
                            }
                        } else if let Some(project_id) = project_id_opt {
                            match upsert_output_notes(
                                &note_repo,
                                &persistence,
                                project_id,
                                &mut graph,
                                node_id,
                                skill_note_id,
                                &produced,
                            )
                            .await
                            {
                                Ok(ids) => {
                                    eprintln!(
                                        "operon: cascade imported {} output note(s)",
                                        ids.len()
                                    );
                                    if let Some(mut sig) = note_version {
                                        sig.with_mut(|v| *v = v.saturating_add(1));
                                    }
                                }
                                Err(e) => eprintln!(
                                    "operon: cascade output-note upsert failed: {e}"
                                ),
                            }
                        }
                        apply_graph.call(graph.clone());

                        // Pause condition: a `cascade_stop` skill
                        // produced a checkpoint artifact, OR step mode
                        // is on AND any artifact was produced. Step
                        // mode treats every successful skill run as a
                        // checkpoint so the user can review each
                        // stage independently before continuing.
                        let step_mode_on =
                            crate::plugins::workflow::state::effective_step_mode(&graph);
                        let step_pause_artifact = if step_mode_on
                            && cascade_stop_artifact.is_none()
                        {
                            sdlc_artifact_ids
                                .as_ref()
                                .and_then(|v| v.first().copied())
                        } else {
                            None
                        };
                        let pause_artifact = cascade_stop_artifact.or(step_pause_artifact);
                        if let Some(checkpoint_id) = pause_artifact {
                            let pause_root = find_cascade_root_artifact(&graph)
                                .unwrap_or(workflow_id);
                            crate::shell::companion_state::CASCADE_STATE
                                .with_mut(|m| {
                                    m.insert(
                                        pause_root,
                                        crate::shell::companion_state::CascadePhase::Paused {
                                            artifact_id: checkpoint_id,
                                            skill_id: skill_note_id,
                                            level: 0,
                                        },
                                    );
                                });
                            eprintln!(
                                "operon: cascade PAUSED at artifact {checkpoint_id} \
                                 (root {pause_root}) cascade_stop={} step_mode={}",
                                cascade_stop_artifact.is_some(),
                                step_mode_on,
                            );
                            node_paused = true;
                        }
                    }
                }
                if node_failed || node_paused {
                    if node_paused {
                        paused_after_node = true;
                    }
                    break;
                }
            }
            // After all sources for this node, stamp the accumulated
            // produced artifact ids so downstream nodes see the full
            // fan-out set on their next visit.
            if let Some(n) = graph.nodes.get_mut(&node_id) {
                n.cached_produced_artifact_ids = accumulated_artifact_ids;
            }
            apply_graph.call(graph.clone());
            if node_failed {
                break;
            }
        }
        eprintln!("operon: cascade DONE");
    });
}

/// Spawn a single-node run. Same plumbing as cascade but for one node.
fn spawn_run_node(
    note_id_str: String,
    node_id: NodeId,
    mut graph: WorkflowGraph,
    apply_graph: Callback<WorkflowGraph>,
    // Same rail-signal plumbing as `spawn_run_cascade` — see the long
    // comment there for why we can't `try_consume_context` these.
    mut active_session_signal: Signal<Option<Uuid>>,
    mut active_scope_signal: Signal<ChatScope>,
) {
    let note_repo = match try_consume_context::<LocalNoteRepo>() {
        Some(LocalNoteRepo(r)) => r,
        None => return,
    };
    let project_repo = match try_consume_context::<LocalProjectRepo>() {
        Some(LocalProjectRepo(r)) => r,
        None => return,
    };
    let plugin = match try_consume_context::<ClaudeCodePluginCtx>() {
        Some(ClaudeCodePluginCtx(p)) => p,
        None => return,
    };
    let persistence = match try_consume_context::<Arc<dyn Persistence>>() {
        Some(p) => p,
        None => return,
    };
    #[cfg(not(target_arch = "wasm32"))]
    let vault_snapshot_run_node: Option<crate::local_mode::vault::VaultRoot> =
        try_consume_context::<crate::local_mode::desktop::CurrentVaultRoot>()
            .and_then(|c| c.0.read().clone());
    let workflow_id = match Uuid::parse_str(&note_id_str) {
        Ok(u) => u,
        Err(_) => return,
    };
    let note_version: Option<Signal<u64>> =
        try_consume_context::<LocalNoteVersion>().map(|v| v.0);
    let session_repo_opt: Option<Arc<dyn operon_store::repos::ChatSessionRepository>> =
        try_consume_context::<ChatSessionRepo>().map(|r| r.0);
    let chat_repo_opt: Option<Arc<dyn operon_store::repos::ChatMessageRepository>> =
        try_consume_context::<ChatMessageRepo>().map(|r| r.0);
    let session_version: Option<Signal<u64>> =
        try_consume_context::<ChatSessionVersion>().map(|v| v.0);
    spawn(async move {
        let Some((_legacy_session, repo_path)) =
            resolve_project_session(workflow_id, &note_repo, &project_repo)
        else {
            return;
        };
        let project_id_opt = note_repo.find_project_for_note(workflow_id).ok().flatten();
        // Same Phase-4 derivation as the cascade — per-node ▶ runs go
        // into the same rail entry.
        let cascade_session_id = cascade_session_id_for(workflow_id);
        let mut transcript_sink: Option<CascadeTranscriptSink> = None;
        if let (Some(session_repo), Some(chat_repo), Some(project_id)) = (
            session_repo_opt.as_ref(),
            chat_repo_opt.as_ref(),
            project_id_opt,
        ) {
            let exists = matches!(session_repo.get(cascade_session_id), Ok(Some(_)));
            if !exists {
                let title = lookup_note_title(&note_repo, project_id, workflow_id)
                    .unwrap_or_else(|| "Workflow".to_string());
                let label = format!("Cascade: {title}");
                let _ = session_repo.create_with_id(
                    cascade_session_id,
                    ChatScope::Project(project_id),
                    &label,
                );
            }
            let _ = session_repo.touch(cascade_session_id);
            if let Some(mut sig) = session_version {
                sig.with_mut(|v| *v = v.saturating_add(1));
            }
            transcript_sink = Some(CascadeTranscriptSink {
                chat_session_id: cascade_session_id,
                chat_repo: chat_repo.clone(),
            });
        }
        let operon_session = cascade_session_id;

        // M2 — refresh <repo>/.claude/CLAUDE.md before binding so the
        // per-node ▶ run sees the up-to-date SDLC inventory on its
        // first turn. Non-fatal on failure.
        if let Some(project_id) = project_id_opt {
            let project_name = project_repo
                .get(project_id)
                .ok()
                .flatten()
                .map(|p| p.name)
                .unwrap_or_else(|| "Unknown project".to_string());
            if let Err(e) = crate::plugins::artifact::claude_context::write_project_claude_md(
                &note_repo,
                &persistence,
                project_id,
                &project_name,
                &repo_path,
            )
            .await
            {
                eprintln!(
                    "operon: per-node run CLAUDE.md refresh failed (non-fatal): {e}"
                );
            }
        }

        plugin.bind_session(operon_session, repo_path.clone());

        // Switch the rail to the cascade session so the user sees the
        // node's streaming transcript / "Claude is thinking…" /
        // tool-call cards live as the run progresses.
        if let Some(project_id) = project_id_opt {
            active_scope_signal.set(ChatScope::Project(project_id));
        }
        active_session_signal.set(Some(operon_session));
        eprintln!(
            "operon: per-node run rail-switch active_session={operon_session} \
             scope=Project({:?})",
            project_id_opt
        );
        #[cfg(not(target_arch = "wasm32"))]
        let outputs_base = match (vault_snapshot_run_node.as_ref(), project_id_opt) {
            (Some(v), Some(pid)) => v.project_outputs_dir(pid),
            _ => repo_path.join("outputs"),
        };
        #[cfg(target_arch = "wasm32")]
        let outputs_base = repo_path.join("outputs");
        match run_one_node(
            &mut graph,
            node_id,
            workflow_id,
            operon_session,
            &repo_path,
            outputs_base,
            plugin.clone(),
            &persistence,
            &note_repo,
            transcript_sink,
        )
        .await
        {
            Err(e) => {
                if let Some(n) = graph.nodes.get_mut(&node_id) {
                    n.status = NodeStatus::Error(format!("{e}"));
                }
            }
            Ok(NodeRunOk {
                skill_note_id,
                produced,
                sdlc_artifact_ids,
                cascade_stop_artifact: _,
            }) => {
                eprintln!(
                    "operon: per-node run produced={} sdlc={} (skill {skill_note_id})",
                    produced.len(),
                    sdlc_artifact_ids.as_ref().map(|v| v.len()).unwrap_or(0),
                );
                if sdlc_artifact_ids.is_some() {
                    if let Some(mut sig) = note_version {
                        sig.with_mut(|v| *v = v.saturating_add(1));
                    }
                } else if let Some(project_id) = project_id_opt {
                    match upsert_output_notes(
                        &note_repo,
                        &persistence,
                        project_id,
                        &mut graph,
                        node_id,
                        skill_note_id,
                        &produced,
                    )
                    .await
                    {
                        Ok(ids) => {
                            eprintln!(
                                "operon: per-node run imported {} output note(s)",
                                ids.len()
                            );
                            if let Some(mut sig) = note_version {
                                sig.with_mut(|v| *v = v.saturating_add(1));
                            }
                        }
                        Err(e) => eprintln!(
                            "operon: per-node output-note upsert failed: {e}"
                        ),
                    }
                }
            }
        }
        apply_graph.call(graph);
    });
}

/// Resolve the workflow's project + a derived operon session UUID +
/// the bound repo_path. Returns `None` when the project's repo isn't
/// bound (cascade is a no-op in that case).
fn resolve_project_session(
    workflow_id: Uuid,
    note_repo: &Arc<dyn LocalNoteRepository>,
    project_repo: &Arc<dyn LocalProjectRepository>,
) -> Option<(Uuid, std::path::PathBuf)> {
    let project_id = note_repo.find_project_for_note(workflow_id).ok().flatten()?;
    let repo_path = project_repo
        .list()
        .ok()?
        .into_iter()
        .find(|p| p.id == project_id)?
        .repo_path?;
    // Reuse the workflow note's UUID as the Operon session id so all
    // turns through cascade share one claude session (gets `--resume`'d
    // across nodes — prompt-cache hits compound).
    Some((workflow_id, repo_path))
}

/// Result of one successful node run, surfaced back to the cascade
/// driver so it can drive Phase-2 output-note upsert without a second
/// disk read.
struct NodeRunOk {
    /// The `skill_note_id` that produced this run — used to look up
    /// the skill's title for the auto-created note name (and as a
    /// fallback when a multi-output skill produces zero files —
    /// can't happen, but defensive logging keys off it).
    skill_note_id: Uuid,
    /// Every `.md` file claude produced this run, in lexicographic
    /// order. Single-output skills (`output_count: one`) yield 1
    /// element; multi-output skills (BA decompose, etc.) yield N.
    /// Each entry is `(absolute_path, body)`.
    produced: Vec<(std::path::PathBuf, String)>,
    /// `Some(ids)` when this run went through the SDLC path
    /// (`runner::run_skill_on_source`), which already imported each
    /// produced file as a `NoteKind::Artifact` row. The outer cascade
    /// driver uses this to (1) skip the legacy Outputs-note upsert
    /// and (2) stamp `Node::cached_produced_artifact_ids` for fan-out
    /// (Phase 6). `None` for the legacy free-form `executor::run_node`
    /// path, which still emits Outputs notes via `upsert_output_notes`.
    sdlc_artifact_ids: Option<Vec<Uuid>>,
    /// `Some(checkpoint_artifact_id)` when this SDLC run produced an
    /// artifact via a skill with `cascade_stop: true`. The cascade
    /// driver writes `CASCADE_STATE::Paused` and breaks the topo
    /// loop on this signal so the user reviews + approves the
    /// checkpoint before continuing. The pause banner already reads
    /// `CASCADE_STATE` and surfaces automatically (Phase 5).
    cascade_stop_artifact: Option<Uuid>,
}

#[allow(clippy::too_many_arguments)]
async fn run_one_node(
    graph: &mut WorkflowGraph,
    node_id: NodeId,
    workflow_id: Uuid,
    operon_session: Uuid,
    repo_path: &std::path::Path,
    outputs_base: std::path::PathBuf,
    plugin: Arc<operon_plugins_claude_code::ClaudeCodeChatPlugin>,
    persistence: &Arc<dyn Persistence>,
    note_repo: &Arc<dyn LocalNoteRepository>,
    transcript_sink: Option<CascadeTranscriptSink>,
) -> Result<NodeRunOk, String> {
    // Mark Running before the await so the canvas shows the spinner.
    if let Some(n) = graph.nodes.get_mut(&node_id) {
        n.status = NodeStatus::Running;
    }

    // Snapshot the node + load its skill.
    let node_snapshot = graph
        .nodes
        .get(&node_id)
        .cloned()
        .ok_or_else(|| format!("node {node_id} missing"))?;
    eprintln!(
        "operon: run_one_node [{node_id}] loading skill body for skill_note_id={}",
        node_snapshot.skill_note_id
    );
    let skill_body_bytes = persistence
        .load(&node_snapshot.skill_note_id.to_string())
        .await
        .map_err(|e| format!("load skill {}: {e}", node_snapshot.skill_note_id))?;
    eprintln!(
        "operon: run_one_node [{node_id}] skill body loaded ({} bytes)",
        skill_body_bytes.len()
    );
    let skill_body = String::from_utf8(skill_body_bytes)
        .map_err(|e| format!("skill body utf8: {e}"))?;
    let (frontmatter, _body) = crate::plugins::skill::frontmatter::split(&skill_body);
    let skill_version = frontmatter
        .as_ref()
        .and_then(|fm| crate::plugins::skill::frontmatter::field(fm, "skill_version"))
        .unwrap_or("")
        .to_string();
    let skill_contract = crate::plugins::skill::frontmatter::contract(
        frontmatter.as_deref().unwrap_or(&[]),
    );
    eprintln!(
        "operon: run_one_node [{node_id}] skill_version={:?} body_len={}",
        skill_version,
        skill_body.len()
    );

    // SDLC routing (Phase 4): when the node has a source artifact AND
    // the skill contract declares any of `input_kind`/`aggregate`/
    // `inherit`, route the run through `runner::run_skill_on_source`.
    // That codepath inlines aggregated descendants and inherited
    // ancestors, applies the gate (Pending vs Approved), and emits
    // real `NoteKind::Artifact` notes parented to the source —
    // identical to what ▶ Play on a Requirements artifact does.
    let is_sdlc_node = node_snapshot.source_artifact_id.is_some()
        && (skill_contract.input_kind.is_some()
            || skill_contract.aggregate.is_some()
            || skill_contract.inherit.is_some());
    if is_sdlc_node {
        return run_one_node_sdlc(
            graph,
            node_id,
            &node_snapshot,
            &skill_body,
            skill_version,
            &skill_contract,
            persistence,
            note_repo,
            plugin,
            operon_session,
        )
        .await;
    }

    // Gather upstream outputs from disk (already-Fresh upstreams have
    // a `cached_output_path` we can read).
    let upstream =
        collect_upstream_outputs(graph, node_id).map_err(|e| format!("upstream: {e}"))?;
    eprintln!(
        "operon: run_one_node [{node_id}] upstream_outputs={}",
        upstream.len()
    );

    // Snapshot the graph for hashing (the executor's run_node hashes
    // against this view).
    let graph_for_hash = graph.clone();

    // Compute the skill slug used for the output filename. Falls back
    // to the skill UUID prefix when the skill's note row can't be
    // looked up (deleted skill row case).
    let project_id_for_slug = note_repo.find_project_for_note(workflow_id).ok().flatten();
    let skill_title = project_id_for_slug
        .and_then(|pid| lookup_note_title(note_repo, pid, node_snapshot.skill_note_id))
        .unwrap_or_else(|| {
            let s = node_snapshot.skill_note_id.to_string();
            s.chars().take(8).collect()
        });
    let skill_slug = crate::plugins::skill::frontmatter::slugify(&skill_title);

    eprintln!(
        "operon: run_one_node [{node_id}] calling executor::run_node \
         (claude subprocess in {}) slug={}",
        repo_path.display(),
        skill_slug,
    );
    let artifact: RunArtifact = run_node(
        plugin,
        operon_session,
        outputs_base,
        workflow_id,
        node_id,
        &node_snapshot,
        &skill_body,
        &skill_version,
        &skill_slug,
        &upstream,
        &graph_for_hash,
        transcript_sink,
        Some(repo_path),
    )
    .await
    .map_err(|e| format!("{e}"))?;
    eprintln!(
        "operon: run_one_node [{node_id}] executor returned artifact: \
         lead_output={} produced_count={}",
        artifact.output_path.display(),
        artifact.produced.len(),
    );

    // Commit results and propagate dirty downstream. cached_output_path
    // tracks the LEAD output (first file lexicographically) so the
    // inspector's "last output" panel and the upstream reader still
    // work; multi-output skills surface every file via the per-node
    // Outputs notes that the caller imports next.
    if let Some(n) = graph.nodes.get_mut(&node_id) {
        n.cached_output_path = Some(artifact.output_path.clone());
        n.cached_input_hash = Some(artifact.input_hash);
        n.status = NodeStatus::Fresh;
    }
    let mut bag = SkillBag::new();
    bag.insert(
        node_snapshot.skill_note_id,
        SkillSnapshot {
            version: skill_version,
            body_hash: hash_body(&skill_body),
        },
    );
    let _ = propagate_dirty(node_id, graph, &bag);
    Ok(NodeRunOk {
        skill_note_id: node_snapshot.skill_note_id,
        produced: artifact.produced,
        sdlc_artifact_ids: None,
        cascade_stop_artifact: None,
    })
}

/// SDLC variant of `run_one_node` (Phase 4 of the workflow→cascade
/// parity port). Routes the run through
/// `runner::run_skill_on_source`, which handles aggregate / inherit /
/// gates / artifact emission identically to the cascade orchestrator.
/// Called when the node has a `source_artifact_id` and the skill
/// declares an SDLC contract (`input_kind` / `aggregate` / `inherit`).
///
/// `contract` is the parsed SkillContract — used here only to detect
/// `cascade_stop: true` so the SDLC route can surface a checkpoint
/// signal back to `spawn_run_cascade` (Phase 5).
#[allow(clippy::too_many_arguments)]
async fn run_one_node_sdlc(
    graph: &mut WorkflowGraph,
    node_id: NodeId,
    node_snapshot: &Node,
    skill_body: &str,
    skill_version: String,
    contract: &crate::plugins::skill::frontmatter::SkillContract,
    persistence: &Arc<dyn Persistence>,
    note_repo: &Arc<dyn LocalNoteRepository>,
    plugin: Arc<operon_plugins_claude_code::ClaudeCodeChatPlugin>,
    operon_session: Uuid,
) -> Result<NodeRunOk, String> {
    let source_id = node_snapshot
        .source_artifact_id
        .ok_or_else(|| "SDLC route requires source_artifact_id".to_string())?;
    let project_repo: Arc<dyn LocalProjectRepository> = match try_consume_context::<
        LocalProjectRepo,
    >() {
        Some(LocalProjectRepo(r)) => r,
        None => return Err("LocalProjectRepo context missing in SDLC node run".into()),
    };
    let chat_repo_opt: Option<Arc<dyn operon_store::repos::ChatMessageRepository>> =
        try_consume_context::<ChatMessageRepo>().map(|r| r.0);

    eprintln!(
        "operon: run_one_node_sdlc [{node_id}] source={source_id} \
         skill={} session={operon_session}",
        node_snapshot.skill_note_id,
    );

    // Workflow-canvas single-node runs don't yet have a Stop UI of
    // their own, so pass a fresh CancellationToken here. Once a
    // canvas-level Stop button lands, swap this for the wired
    // token. The runner's signature still requires SOMETHING — and
    // the plugin's drive_stream watches whatever it's given, so a
    // fresh token is a no-op cancel that compiles.
    let cancel = tokio_util::sync::CancellationToken::new();
    let outcome = crate::plugins::artifact::runner::run_skill_on_source(
        note_repo,
        &project_repo,
        persistence,
        &plugin,
        chat_repo_opt.as_ref(),
        operon_session,
        source_id,
        node_snapshot.skill_note_id,
        Some(node_id),
        cancel,
    )
    .await
    .map_err(|e| format!("run_skill_on_source: {e}"))?;

    eprintln!(
        "operon: run_one_node_sdlc [{node_id}] produced {} artifact(s) under {}",
        outcome.created_artifact_ids.len(),
        outcome.artifacts_dir.display(),
    );

    // Stamp the node so the dirty/version logic sees this run as
    // completed. `cached_produced_artifact_ids` accumulation across
    // multiple fan-out invocations (Phase 6) is handled by the topo
    // loop in `spawn_run_cascade` — this function only reports the
    // ids it produced via `NodeRunOk::sdlc_artifact_ids`.
    if let Some(n) = graph.nodes.get_mut(&node_id) {
        n.cached_output_path = Some(outcome.artifacts_dir.clone());
        n.cached_input_hash = Some(format!("sdlc-{}", node_id));
        n.status = NodeStatus::Fresh;
    }
    let mut bag = SkillBag::new();
    bag.insert(
        node_snapshot.skill_note_id,
        SkillSnapshot {
            version: skill_version,
            body_hash: hash_body(skill_body),
        },
    );
    let _ = propagate_dirty(node_id, graph, &bag);

    let cascade_stop_artifact = if contract.cascade_stop
        && !outcome.created_artifact_ids.is_empty()
    {
        outcome.created_artifact_ids.first().copied()
    } else {
        None
    };

    Ok(NodeRunOk {
        skill_note_id: node_snapshot.skill_note_id,
        // Empty `produced` because the SDLC path already imported
        // every file as an Artifact note via `import_produced_artifacts`
        // — the outer cascade loop won't re-run `upsert_output_notes`
        // when `sdlc_artifact_ids` is `Some`.
        produced: Vec::new(),
        sdlc_artifact_ids: Some(outcome.created_artifact_ids),
        cascade_stop_artifact,
    })
}

/// Phase-2 output surfacing: upsert a per-skill "Outputs/<title>-output"
/// markdown note in the explorer. Idempotent across runs:
/// - The "Outputs" folder is project-scoped — found by title at
///   project root and reused (or created if absent). All workflows
///   in the project share one folder.
/// - The per-node output note is keyed off `cached_output_note_id`
///   stamped on the node. On re-runs the stamped row is reused, the
///   body is overwritten, and the title is re-synced to whatever the
///   current skill is called.
///
/// Returns the note id so the caller can stamp it onto the node and
/// expose it via the inspector's "View in tab" button.
async fn upsert_output_notes(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
    graph: &mut WorkflowGraph,
    node_id: NodeId,
    skill_note_id: Uuid,
    produced: &[(std::path::PathBuf, String)],
) -> Result<Vec<Uuid>, String> {
    use crate::plugins::artifact::frontmatter::{
        parse as parse_artifact_fm, rewrite as rewrite_artifact_fm, ArtifactKind, ArtifactStatus,
    };

    // 1. One project-scoped Outputs folder. Found by title at root —
    //    co-opts a user-created folder if one exists with the same
    //    name; otherwise created.
    let folder_id = ensure_outputs_folder(note_repo, project_id)?;

    // 2. One Outputs note per produced file. Title is the file's stem
    //    (e.g. `epic-01-core-timer-engine.md` → `epic-01-core-timer-engine`).
    //    Note kind depends on the file's declared `artifact_kind`:
    //    Summary outputs land as `Markdown` notes (read-only narrative
    //    that the user reads top-to-bottom), every other kind
    //    (epic / feature / story / task / plan / implementation /
    //    test_cases / test_results) lands as `Artifact` so the artifact
    //    UI can pick them up — gate them by status, run downstream
    //    skills, surface kind chips, etc. Existing siblings of EITHER
    //    kind are reused on re-runs so the user doesn't accumulate
    //    duplicates if the kind contract changes.
    let existing_outputs: Vec<LocalNote> = note_repo
        .list_for_project(project_id)
        .unwrap_or_default()
        .into_iter()
        .filter(|n| {
            n.parent_id == Some(folder_id)
                && matches!(n.kind, NoteKind::Markdown | NoteKind::Artifact)
        })
        .collect();

    let mut imported: Vec<Uuid> = Vec::with_capacity(produced.len());
    for (idx, (path, body)) in produced.iter().enumerate() {
        let title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output")
            .to_string();

        // Parse the produced file's own frontmatter to decide the note
        // kind. The runtime prompt instructs Claude to start every
        // output with `artifact_kind: <kind>` matching the skill's
        // `output_kind`, so this read is well-defined for every seed
        // skill. Files missing frontmatter (a misbehaving skill) fall
        // through to `Artifact` so the user can fix them in-place
        // rather than getting an inert Markdown note.
        let fm = parse_artifact_fm(body);
        let is_summary = matches!(fm.artifact_kind, Some(ArtifactKind::Summary));
        let target_kind = if is_summary { NoteKind::Markdown } else { NoteKind::Artifact };

        let existing_id = existing_outputs.iter().find(|n| n.title == title).map(|n| n.id);
        let row_id = match existing_id {
            Some(id) => id,
            None => {
                let row = note_repo
                    .create_with_kind(project_id, Some(folder_id), &title, target_kind)
                    .map_err(|e| format!("create output note '{title}': {e}"))?;
                row.id
            }
        };

        // For Artifact-kind outputs, patch the frontmatter so the
        // engine's view fields are authoritative regardless of what
        // the skill wrote: ensure `status: approved` (workflow runs
        // are user-driven and don't gate downstream skills here),
        // stamp `source_skill_id` so the artifact view's chips can
        // link back, and let `artifact_kind` fall through if missing.
        // Summary outputs save the body verbatim — no frontmatter
        // patching since they render as plain Markdown.
        let body_to_save = if is_summary {
            body.clone()
        } else {
            let mut fm_patched = fm;
            if fm_patched.artifact_kind.is_none() {
                // Preserve a hint for downstream readers — if the skill
                // forgot frontmatter entirely, the artifact_kind will
                // still be None and the view falls back to "Artifact".
            }
            fm_patched.status = ArtifactStatus::Approved;
            fm_patched.source_skill_id = Some(skill_note_id);
            rewrite_artifact_fm(body, &fm_patched)
        };

        persistence
            .save(&row_id.to_string(), body_to_save.as_bytes())
            .await
            .map_err(|e| format!("save output body for '{title}': {e}"))?;
        imported.push(row_id);
        // Stamp the FIRST output as the node's anchor so the inspector
        // still has one note to surface via "View in tab".
        if idx == 0 {
            if let Some(node) = graph.nodes.get_mut(&node_id) {
                node.cached_output_note_id = Some(row_id);
            }
        }
    }
    Ok(imported)
}

/// Find the project's "Outputs" folder note (markdown root note titled
/// "Outputs"), or create it if absent. Reuses existing folders so
/// re-runs across cascades and per-node ▶ runs all land in the same
/// place.
fn ensure_outputs_folder(
    note_repo: &Arc<dyn LocalNoteRepository>,
    project_id: Uuid,
) -> Result<Uuid, String> {
    let existing = note_repo
        .list_for_project(project_id)
        .map_err(|e| format!("list project notes: {e}"))?
        .into_iter()
        .find(|n| n.parent_id.is_none() && n.title == "Outputs")
        .map(|n| n.id);
    if let Some(id) = existing {
        return Ok(id);
    }
    let folder = note_repo
        .create_with_kind(project_id, None, "Outputs", NoteKind::Markdown)
        .map_err(|e| format!("create Outputs folder: {e}"))?;
    Ok(folder.id)
}

/// O(N) scan of `list_for_project` to grab one note's title.
/// Acceptable here since each call is once-per-cascade-run / event.
fn lookup_note_title(
    note_repo: &Arc<dyn LocalNoteRepository>,
    project_id: Uuid,
    note_id: Uuid,
) -> Option<String> {
    note_repo
        .list_for_project(project_id)
        .ok()?
        .into_iter()
        .find(|n| n.id == note_id)
        .map(|n| n.title)
}

/// Phase-4: derive a stable v5 UUID for the cascade chat-session that
/// corresponds to a given workflow note. Same workflow → same id, so
/// the rail entry persists across runs and `claude --resume` keeps
/// working through the plugin's binding cache.
fn cascade_session_id_for(workflow_id: Uuid) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_OID,
        format!("operon-workflow-cascade:{workflow_id}").as_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_or_default_empty_returns_default() {
        let g = parse_or_default("");
        assert_eq!(g, WorkflowGraph::new());
    }

    #[test]
    fn parse_or_default_invalid_json_returns_default() {
        let g = parse_or_default("not json");
        assert_eq!(g, WorkflowGraph::new());
    }

    #[test]
    fn parse_or_default_round_trips_valid_json() {
        let mut g = WorkflowGraph::new();
        let id = Uuid::new_v4();
        g.nodes.insert(
            id,
            Node {
                id,
                skill_note_id: Uuid::new_v4(),
                typed_fields: serde_json::Value::Null,
                extra_instructions: String::new(),
                position: (10.0, 20.0),
                cached_output_path: None,
                cached_input_hash: None,
                status: NodeStatus::Fresh,
                cached_output_note_id: None,
                is_artifact_snapshot: false,
                artifact_ref: None,
                artifact_kind_label: None,
                artifact_title: None,
                source_artifact_id: None,
                cached_produced_artifact_ids: Vec::new(),
            },
        );
        let s = serialize(&g);
        let back = parse_or_default(&s);
        assert_eq!(back, g);
    }

    #[test]
    fn append_node_creates_a_fresh_dirty_node() {
        let g = WorkflowGraph::new();
        let s = serialize(&g);
        let skill = Uuid::new_v4();
        let next = append_node_to_graph(&s, skill);
        let parsed: WorkflowGraph = serde_json::from_str(&next).unwrap();
        assert_eq!(parsed.nodes.len(), 1);
        let node = parsed.nodes.values().next().unwrap();
        assert_eq!(node.skill_note_id, skill);
        assert!(matches!(node.status, NodeStatus::Dirty));
    }

    #[test]
    fn append_node_preserves_existing_nodes() {
        let mut g = WorkflowGraph::new();
        let existing = Uuid::new_v4();
        g.nodes.insert(
            existing,
            Node {
                id: existing,
                skill_note_id: Uuid::new_v4(),
                typed_fields: serde_json::Value::Null,
                extra_instructions: "first".into(),
                position: (0.0, 0.0),
                cached_output_path: None,
                cached_input_hash: None,
                status: NodeStatus::Fresh,
                cached_output_note_id: None,
                is_artifact_snapshot: false,
                artifact_ref: None,
                artifact_kind_label: None,
                artifact_title: None,
                source_artifact_id: None,
                cached_produced_artifact_ids: Vec::new(),
            },
        );
        let s = serialize(&g);
        let next = append_node_to_graph(&s, Uuid::new_v4());
        let parsed: WorkflowGraph = serde_json::from_str(&next).unwrap();
        assert_eq!(parsed.nodes.len(), 2);
        assert!(parsed.nodes.contains_key(&existing));
    }

    #[test]
    fn append_node_returns_input_when_invalid_and_nonempty() {
        let bad = "not actually json";
        let next = append_node_to_graph(bad, Uuid::new_v4());
        assert_eq!(next, bad);
    }

    #[test]
    fn append_artifact_tile_creates_snapshot_node() {
        let g = WorkflowGraph::new();
        let s = serialize(&g);
        let art = Uuid::new_v4();
        let next = append_artifact_tile_to_graph(&s, art, "Requirements: Auth");
        let parsed: WorkflowGraph = serde_json::from_str(&next).unwrap();
        assert_eq!(parsed.nodes.len(), 1);
        let node = parsed.nodes.values().next().unwrap();
        assert!(node.is_artifact_snapshot);
        assert_eq!(node.artifact_ref, Some(art));
        assert_eq!(node.artifact_title.as_deref(), Some("Requirements: Auth"));
        assert_eq!(node.skill_note_id, Uuid::nil());
        assert!(matches!(node.status, NodeStatus::Fresh));
    }

    #[test]
    fn append_artifact_tile_then_skill_node_then_edge_auto_derives_source() {
        // End-to-end of the new picker flow: drop an artifact tile,
        // then a skill node, then wire them — confirms the wiring
        // populates source_artifact_id so the SDLC executor route
        // takes effect on the next "Run all dirty".
        let s = serialize(&WorkflowGraph::new());
        let art = Uuid::new_v4();
        let s = append_artifact_tile_to_graph(&s, art, "Requirements");
        let skill = Uuid::new_v4();
        let s = append_node_to_graph(&s, skill);
        let mut g: WorkflowGraph = serde_json::from_str(&s).unwrap();
        let tile_id = *g
            .nodes
            .iter()
            .find(|(_, n)| n.is_artifact_snapshot)
            .map(|(id, _)| id)
            .unwrap();
        let skill_node_id = *g
            .nodes
            .iter()
            .find(|(_, n)| !n.is_artifact_snapshot)
            .map(|(id, _)| id)
            .unwrap();
        g = add_edge_if_new(&g, tile_id, skill_node_id).expect("edge added");
        assert_eq!(
            g.nodes.get(&skill_node_id).and_then(|n| n.source_artifact_id),
            Some(art),
            "skill node should inherit source from the upstream tile"
        );
    }

    #[test]
    fn remove_node_drops_node_and_incident_edges() {
        let mut g = WorkflowGraph::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        for id in [a, b, c] {
            g.nodes.insert(
                id,
                Node {
                    id,
                    skill_note_id: Uuid::new_v4(),
                    typed_fields: serde_json::Value::Null,
                    extra_instructions: String::new(),
                    position: (0.0, 0.0),
                    cached_output_path: None,
                    cached_input_hash: None,
                    status: NodeStatus::Fresh,
                    cached_output_note_id: None,
                    is_artifact_snapshot: false,
                    artifact_ref: None,
                    artifact_kind_label: None,
                    artifact_title: None,
                    source_artifact_id: None,
                    cached_produced_artifact_ids: Vec::new(),
                },
            );
        }
        g.edges.push(Edge {
            id: Uuid::new_v4(),
            from: a,
            from_socket: "default".into(),
            to: b,
            to_socket: "default".into(),
            edge_kind: None,
        });
        g.edges.push(Edge {
            id: Uuid::new_v4(),
            from: b,
            from_socket: "default".into(),
            to: c,
            to_socket: "default".into(),
            edge_kind: None,
        });
        g.edges.push(Edge {
            id: Uuid::new_v4(),
            from: a,
            from_socket: "default".into(),
            to: c,
            to_socket: "default".into(),
            edge_kind: None,
        });
        let prev_version = g.version;
        let next = remove_node(&g, b);
        assert!(!next.nodes.contains_key(&b));
        assert!(next.nodes.contains_key(&a));
        assert!(next.nodes.contains_key(&c));
        // Only the a→c edge survives (a→b and b→c both touched b).
        assert_eq!(next.edges.len(), 1);
        assert_eq!(next.edges[0].from, a);
        assert_eq!(next.edges[0].to, c);
        assert!(next.version > prev_version);
    }

    #[test]
    fn add_edge_if_new_rejects_self_loop_and_duplicates() {
        let mut g = WorkflowGraph::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        for id in [a, b] {
            g.nodes.insert(
                id,
                Node {
                    id,
                    skill_note_id: Uuid::new_v4(),
                    typed_fields: serde_json::Value::Null,
                    extra_instructions: String::new(),
                    position: (0.0, 0.0),
                    cached_output_path: None,
                    cached_input_hash: None,
                    status: NodeStatus::Fresh,
                    cached_output_note_id: None,
                    is_artifact_snapshot: false,
                    artifact_ref: None,
                    artifact_kind_label: None,
                    artifact_title: None,
                    source_artifact_id: None,
                    cached_produced_artifact_ids: Vec::new(),
                },
            );
        }
        // Self-loop rejected.
        assert!(add_edge_if_new(&g, a, a).is_none());
        // First insert succeeds.
        let g1 = add_edge_if_new(&g, a, b).expect("first insert");
        assert_eq!(g1.edges.len(), 1);
        // Duplicate rejected.
        assert!(add_edge_if_new(&g1, a, b).is_none());
    }

    #[test]
    fn cascade_session_id_is_deterministic_per_workflow() {
        let workflow = Uuid::new_v4();
        let other = Uuid::new_v4();
        // Same input → same output across calls (so re-runs find the
        // existing chat_session row instead of spawning duplicates).
        assert_eq!(
            cascade_session_id_for(workflow),
            cascade_session_id_for(workflow)
        );
        // Different workflow → different cascade session (so each
        // workflow owns its own rail entry).
        assert_ne!(
            cascade_session_id_for(workflow),
            cascade_session_id_for(other)
        );
    }

    #[test]
    fn remove_edge_drops_only_the_target_edge() {
        let mut g = WorkflowGraph::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let edge_keep = Edge {
            id: Uuid::new_v4(),
            from: a,
            from_socket: "default".into(),
            to: b,
            to_socket: "default".into(),
            edge_kind: None,
        };
        let edge_drop = Edge {
            id: Uuid::new_v4(),
            from: b,
            from_socket: "default".into(),
            to: a,
            to_socket: "default".into(),
            edge_kind: None,
        };
        let drop_id = edge_drop.id;
        g.edges.push(edge_keep.clone());
        g.edges.push(edge_drop);
        let next = remove_edge(&g, drop_id);
        assert_eq!(next.edges.len(), 1);
        assert_eq!(next.edges[0].id, edge_keep.id);
    }

    #[test]
    fn layout_assigns_positions_to_origin_nodes_only() {
        let mut g = WorkflowGraph::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        g.nodes.insert(
            a,
            Node {
                id: a,
                skill_note_id: Uuid::new_v4(),
                typed_fields: serde_json::Value::Null,
                extra_instructions: String::new(),
                position: (0.0, 0.0),
                cached_output_path: None,
                cached_input_hash: None,
                status: NodeStatus::Dirty,
                cached_output_note_id: None,
                is_artifact_snapshot: false,
                artifact_ref: None,
                artifact_kind_label: None,
                artifact_title: None,
                source_artifact_id: None,
                cached_produced_artifact_ids: Vec::new(),
            },
        );
        g.nodes.insert(
            b,
            Node {
                id: b,
                skill_note_id: Uuid::new_v4(),
                typed_fields: serde_json::Value::Null,
                extra_instructions: String::new(),
                position: (123.0, 456.0),
                cached_output_path: None,
                cached_input_hash: None,
                status: NodeStatus::Dirty,
                cached_output_note_id: None,
                is_artifact_snapshot: false,
                artifact_ref: None,
                artifact_kind_label: None,
                artifact_title: None,
                source_artifact_id: None,
                cached_produced_artifact_ids: Vec::new(),
            },
        );
        let map = layout(&g);
        assert!(map.contains_key(&a));
        assert!(!map.contains_key(&b));
    }

    /// Build a node with the given id, skill, and (optional)
    /// source_artifact_id. Test helper for the fan-out / source-
    /// resolution suite.
    fn skill_node(id: NodeId, source: Option<Uuid>, produced: Vec<Uuid>) -> Node {
        Node {
            id,
            skill_note_id: Uuid::new_v4(),
            typed_fields: serde_json::Value::Null,
            extra_instructions: String::new(),
            position: (0.0, 0.0),
            cached_output_path: None,
            cached_input_hash: None,
            status: NodeStatus::Fresh,
            cached_output_note_id: None,
            is_artifact_snapshot: false,
            artifact_ref: None,
            artifact_kind_label: None,
            artifact_title: None,
            source_artifact_id: source,
            cached_produced_artifact_ids: produced,
        }
    }

    fn artifact_tile(id: NodeId, art: Uuid) -> Node {
        Node {
            id,
            skill_note_id: Uuid::nil(),
            typed_fields: serde_json::Value::Null,
            extra_instructions: String::new(),
            position: (0.0, 0.0),
            cached_output_path: None,
            cached_input_hash: None,
            status: NodeStatus::Fresh,
            cached_output_note_id: None,
            is_artifact_snapshot: true,
            artifact_ref: Some(art),
            artifact_kind_label: None,
            artifact_title: None,
            source_artifact_id: None,
            cached_produced_artifact_ids: Vec::new(),
        }
    }

    fn plain_edge(from: NodeId, to: NodeId) -> Edge {
        Edge {
            id: Uuid::new_v4(),
            from,
            from_socket: "default".into(),
            to,
            to_socket: "default".into(),
            edge_kind: None,
        }
    }

    #[test]
    fn effective_sources_uses_explicit_source_artifact_id() {
        let mut g = WorkflowGraph::new();
        let n = Uuid::new_v4();
        let art = Uuid::new_v4();
        g.nodes.insert(n, skill_node(n, Some(art), vec![]));
        let sources = effective_sources_for_node(&g, n);
        assert_eq!(sources, vec![art]);
    }

    #[test]
    fn effective_sources_returns_empty_for_unbound_node() {
        let mut g = WorkflowGraph::new();
        let n = Uuid::new_v4();
        g.nodes.insert(n, skill_node(n, None, vec![]));
        let sources = effective_sources_for_node(&g, n);
        assert!(sources.is_empty());
    }

    #[test]
    fn effective_sources_pulls_from_artifact_snapshot_upstream() {
        let mut g = WorkflowGraph::new();
        let tile_id = Uuid::new_v4();
        let skill_id = Uuid::new_v4();
        let art = Uuid::new_v4();
        g.nodes.insert(tile_id, artifact_tile(tile_id, art));
        g.nodes.insert(skill_id, skill_node(skill_id, None, vec![]));
        g.edges.push(plain_edge(tile_id, skill_id));
        let sources = effective_sources_for_node(&g, skill_id);
        assert_eq!(sources, vec![art]);
    }

    #[test]
    fn effective_sources_fans_out_from_skill_upstream_with_many_outputs() {
        // Phase 6: when an upstream skill node has produced N
        // artifacts (output_count: many), the downstream node fans
        // out — sources list contains all N ids.
        let mut g = WorkflowGraph::new();
        let upstream = Uuid::new_v4();
        let downstream = Uuid::new_v4();
        let art_a = Uuid::new_v4();
        let art_b = Uuid::new_v4();
        let art_c = Uuid::new_v4();
        g.nodes.insert(
            upstream,
            skill_node(upstream, None, vec![art_a, art_b, art_c]),
        );
        g.nodes
            .insert(downstream, skill_node(downstream, None, vec![]));
        g.edges.push(plain_edge(upstream, downstream));
        let sources = effective_sources_for_node(&g, downstream);
        assert_eq!(sources, vec![art_a, art_b, art_c]);
    }

    #[test]
    fn effective_sources_explicit_source_overrides_upstream_fan_out() {
        // Aggregator skills (input_kind: requirements, anchored to
        // the seed root) keep their single-source semantic even when
        // downstream of a many-output upstream — explicit
        // source_artifact_id wins.
        let mut g = WorkflowGraph::new();
        let upstream = Uuid::new_v4();
        let aggregator = Uuid::new_v4();
        let req_root = Uuid::new_v4();
        let task_a = Uuid::new_v4();
        let task_b = Uuid::new_v4();
        g.nodes.insert(
            upstream,
            skill_node(upstream, None, vec![task_a, task_b]),
        );
        g.nodes.insert(
            aggregator,
            skill_node(aggregator, Some(req_root), vec![]),
        );
        g.edges.push(plain_edge(upstream, aggregator));
        let sources = effective_sources_for_node(&g, aggregator);
        assert_eq!(sources, vec![req_root]);
    }

    #[test]
    fn effective_sources_dedups_same_artifact_seen_via_multiple_upstreams() {
        let mut g = WorkflowGraph::new();
        let up_a = Uuid::new_v4();
        let up_b = Uuid::new_v4();
        let down = Uuid::new_v4();
        let shared_art = Uuid::new_v4();
        g.nodes
            .insert(up_a, skill_node(up_a, None, vec![shared_art]));
        g.nodes
            .insert(up_b, skill_node(up_b, None, vec![shared_art]));
        g.nodes.insert(down, skill_node(down, None, vec![]));
        g.edges.push(plain_edge(up_a, down));
        g.edges.push(plain_edge(up_b, down));
        let sources = effective_sources_for_node(&g, down);
        assert_eq!(sources, vec![shared_art]);
    }

    #[test]
    fn add_edge_auto_derives_source_from_artifact_snapshot_upstream() {
        // Phase 2: wiring an edge from an artifact-snapshot tile to
        // a fresh skill node should copy the tile's `artifact_ref`
        // onto the skill's `source_artifact_id`.
        let mut g = WorkflowGraph::new();
        let tile_id = Uuid::new_v4();
        let skill_id = Uuid::new_v4();
        let art = Uuid::new_v4();
        g.nodes.insert(tile_id, artifact_tile(tile_id, art));
        g.nodes.insert(skill_id, skill_node(skill_id, None, vec![]));
        let next = add_edge_if_new(&g, tile_id, skill_id)
            .expect("edge should be added");
        assert_eq!(
            next.nodes.get(&skill_id).and_then(|n| n.source_artifact_id),
            Some(art),
            "skill node should inherit the tile's artifact_ref"
        );
    }

    #[test]
    fn add_edge_does_not_overwrite_explicit_source() {
        // When the destination already has source_artifact_id set,
        // wiring a new artifact-tile upstream must NOT overwrite it
        // (preserves explicit user overrides).
        let mut g = WorkflowGraph::new();
        let tile_id = Uuid::new_v4();
        let skill_id = Uuid::new_v4();
        let tile_art = Uuid::new_v4();
        let explicit_art = Uuid::new_v4();
        g.nodes.insert(tile_id, artifact_tile(tile_id, tile_art));
        g.nodes
            .insert(skill_id, skill_node(skill_id, Some(explicit_art), vec![]));
        let next = add_edge_if_new(&g, tile_id, skill_id)
            .expect("edge should be added");
        assert_eq!(
            next.nodes.get(&skill_id).and_then(|n| n.source_artifact_id),
            Some(explicit_art),
            "explicit source must not be overwritten"
        );
    }

    #[test]
    fn add_edge_skill_to_skill_does_not_set_source() {
        // When the upstream is another skill (not an artifact tile),
        // no auto-derive should happen — the downstream node stays
        // unbound so the topo loop's fan-out logic kicks in.
        let mut g = WorkflowGraph::new();
        let s1 = Uuid::new_v4();
        let s2 = Uuid::new_v4();
        g.nodes.insert(s1, skill_node(s1, None, vec![]));
        g.nodes.insert(s2, skill_node(s2, None, vec![]));
        let next = add_edge_if_new(&g, s1, s2).expect("edge should be added");
        assert!(
            next.nodes
                .get(&s2)
                .and_then(|n| n.source_artifact_id)
                .is_none(),
            "skill→skill edge must not auto-derive source"
        );
    }

    #[test]
    fn find_cascade_root_picks_top_artifact_snapshot_with_no_inbound_parent_edge() {
        let mut g = WorkflowGraph::new();
        let root_tile = Uuid::new_v4();
        let child_tile = Uuid::new_v4();
        let root_art = Uuid::new_v4();
        let child_art = Uuid::new_v4();
        let mut root_node = artifact_tile(root_tile, root_art);
        root_node.position = (0.0, 0.0);
        let mut child_node = artifact_tile(child_tile, child_art);
        child_node.position = (0.0, 200.0);
        g.nodes.insert(root_tile, root_node);
        g.nodes.insert(child_tile, child_node);
        g.edges.push(Edge {
            id: Uuid::new_v4(),
            from: root_tile,
            from_socket: "default".into(),
            to: child_tile,
            to_socket: "default".into(),
            edge_kind: Some("parent_child".into()),
        });
        assert_eq!(find_cascade_root_artifact(&g), Some(root_art));
    }
}
