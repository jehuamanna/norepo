//! Workflow note canvas + JSON editor (M3b).
//!
//! V1 scope is intentionally narrow: a read-only SVG canvas that
//! visualises a `WorkflowGraph`, plus an Edit mode that pairs that
//! canvas with a JSON textarea so a BA can hand-edit the graph and
//! watch it light up. Drag-to-position + click-to-edit is M3b.5;
//! cascade execution is M3c.

use dioxus::prelude::*;
use operon_store::repos::{LocalNote, NoteKind};
use operon_store::repos::{LocalNoteRepository, LocalProjectRepository};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::local_mode::desktop::{LocalNoteRepo, LocalProjectRepo};
use crate::local_mode::explorer::{LocalNoteVersion, SelectedNote};
use crate::persistence::Persistence;
use crate::plugins::markdown::MarkdownView;
use crate::plugins::workflow::engine::{propagate_dirty, topo_order_dirty, SkillBag, SkillSnapshot, hash_body};
use crate::plugins::workflow::executor::{
    collect_upstream_outputs, run_node, CascadeTranscriptSink, RunArtifact,
};
use crate::plugins::workflow::state::{Edge, EdgeId, Node, NodeId, NodeStatus, WorkflowGraph};
use crate::shell::companion_state::{
    ActiveChatScope, ActiveChatSession, ChatMessageRepo, ChatSessionRepo, ChatSessionVersion,
    ClaudeCodePluginCtx,
};
use operon_store::repos::ChatScope;

const NODE_W: f64 = 180.0;
const NODE_H: f64 = 64.0;

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
            }
            div { class: "operon-workflow-pane",
                WorkflowCanvas {
                    graph: graph_for_canvas,
                    note_id: note_id.clone(),
                    apply_graph: apply_graph,
                }
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
    rsx! {
        div { class: "operon-workflow-surface",
            "data-testid": "workflow-view",
            WorkflowCanvas {
                graph,
                note_id: String::new(),
                apply_graph: noop,
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct WorkflowCanvasProps {
    graph: WorkflowGraph,
    note_id: String,
    apply_graph: Callback<WorkflowGraph>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct DragState {
    node: NodeId,
    /// Mouse client position at drag start, so subsequent mousemove
    /// events can compute a delta against it (we update the node's
    /// position by that delta).
    start_client_x: f64,
    start_client_y: f64,
    /// Node position at drag start.
    start_node_x: f64,
    start_node_y: f64,
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
    let skill_titles: HashMap<Uuid, String> = {
        let mut out = HashMap::new();
        if let (Ok(workflow_id), Some(LocalNoteRepo(repo))) = (
            Uuid::parse_str(&props.note_id),
            try_consume_context::<LocalNoteRepo>(),
        ) {
            if let Ok(Some(project_id)) = repo.find_project_for_note(workflow_id) {
                if let Ok(rows) = repo.list_for_project(project_id) {
                    for row in rows {
                        if matches!(row.kind, NoteKind::Skill) {
                            out.insert(row.id, row.title);
                        }
                    }
                }
            }
        }
        out
    };
    let mut drag = use_signal::<Option<DragState>>(|| None);
    let mut edge_drag = use_signal::<Option<EdgeDragState>>(|| None);
    let mut selected_node = use_signal::<Option<NodeId>>(|| None);
    let mut selected_edge = use_signal::<Option<EdgeId>>(|| None);
    // Phase-3: app-scope signal the explorer watches; setting it
    // triggers the wired effect in editor_host that opens the note in
    // an Edit-mode tab. The "View in tab" button writes here.
    let selected_note_app = try_consume_context::<SelectedNote>().map(|s| s.0);
    // Viewport pan/zoom. The SVG itself uses CSS pixel coords (no
    // viewBox), and an inner <g> applies `translate(pan) scale(zoom)`
    // so node positions / edge math stay in "world" coords while the
    // visible window can pan and zoom. Wheel + ctrl/cmd zooms
    // centered on the cursor; plain wheel pans; middle-click drag
    // pans too.
    let mut pan_x = use_signal(|| 0.0f64);
    let mut pan_y = use_signal(|| 0.0f64);
    let zoom = use_signal(|| 1.0f64);
    let mut pan_drag = use_signal::<Option<PanDragState>>(|| None);
    // Auto-place nodes whose `position` is exactly (0, 0) — keeps the
    // hand-edited "create a node by adding to JSON" flow usable without
    // having to do math. Stable id-sorted layout.
    let positions = layout(&g);
    let nodes: Vec<NodeRender> = g
        .nodes
        .iter()
        .map(|(id, n)| NodeRender {
            id: *id,
            x: positions.get(id).map(|p| p.0).unwrap_or(n.position.0),
            y: positions.get(id).map(|p| p.1).unwrap_or(n.position.1),
            label: node_label(n, &skill_titles),
            status: n.status.clone(),
            is_artifact_snapshot: n.is_artifact_snapshot,
            kind_label: n.artifact_kind_label.clone(),
        })
        .collect();
    let edges: Vec<EdgeRender> = g
        .edges
        .iter()
        .filter_map(|e| {
            let from = nodes.iter().find(|n| n.id == e.from)?;
            let to = nodes.iter().find(|n| n.id == e.to)?;
            Some(EdgeRender {
                id: e.id,
                from_x: from.x + NODE_W,
                from_y: from.y + NODE_H / 2.0,
                to_x: to.x,
                to_y: to.y + NODE_H / 2.0,
                edge_kind: e.edge_kind.clone(),
            })
        })
        .collect();

    // No viewBox: the SVG draws in CSS pixel coords, and an inner
    // <g> applies pan/zoom. This gives an effectively infinite
    // canvas (panning extends arbitrarily) without auto-fit
    // surprises when nodes are added.
    let pan_xv = *pan_x.read();
    let pan_yv = *pan_y.read();
    let zoomv = *zoom.read();
    let world_transform = format!("translate({pan_xv} {pan_yv}) scale({zoomv})");

    // Inspector readout for the selected node — extra_instructions live
    // edits and a skill-id readout for now (typed_fields edit lives in
    // the JSON pane to keep schema-validation consistent).
    let selected = *selected_node.read();
    let selected_node_view = selected.and_then(|sid| g.nodes.get(&sid).cloned());

    let reset_view = {
        let mut pan_x_setter = pan_x;
        let mut pan_y_setter = pan_y;
        let mut zoom_setter = zoom;
        move |_| {
            pan_x_setter.set(0.0);
            pan_y_setter.set(0.0);
            zoom_setter.set(1.0);
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
        }
    };
    let zoom_out = {
        let mut zoom_setter = zoom;
        move |_| {
            let cur = *zoom_setter.read();
            let next = (cur / 1.2).clamp(0.2, 5.0);
            zoom_setter.set(next);
        }
    };

    rsx! {
        div { class: "operon-workflow-canvas",
            "data-testid": "workflow-canvas",
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
            if let Some(sn) = selected_node_view.as_ref() {
                {
                    let sid = sn.id;
                    let extra = sn.extra_instructions.clone();
                    let skill_id_str = sn.skill_note_id.to_string();
                    let apply = props.apply_graph;
                    let mut graph_for_inspector = g.clone();
                    let graph_for_delete = g.clone();
                    let current_status = sn.status.clone();
                    let graph_for_status = g.clone();
                    rsx! {
                        div { class: "operon-workflow-inspector",
                            "data-testid": "workflow-inspector",
                            "data-node-id": "{sid}",
                            div { class: "operon-workflow-inspector-row",
                                span { class: "operon-workflow-inspector-label", "skill id" }
                                code { class: "md-inline-code operon-workflow-inspector-code", "{skill_id_str}" }
                                {
                                    // Phase-3: open the auto-created
                                    // output note in a tab. Button is
                                    // disabled when the node has no
                                    // output yet.
                                    let out_note_id = sn.cached_output_note_id;
                                    let has_output = out_note_id.is_some()
                                        && selected_note_app.is_some();
                                    let title = if has_output {
                                        "Open this node's last output in a tab"
                                    } else {
                                        "Run this node first to view its output"
                                    };
                                    let sink = selected_note_app;
                                    rsx! {
                                        button {
                                            r#type: "button",
                                            class: "operon-workflow-inspector-view",
                                            "data-testid": "workflow-inspector-view-output",
                                            title: "{title}",
                                            disabled: !has_output,
                                            onclick: move |_| {
                                                if let (Some(id), Some(mut sig)) = (out_note_id, sink) {
                                                    sig.set(Some(id));
                                                }
                                            },
                                            "View in tab"
                                        }
                                    }
                                }
                                button {
                                    r#type: "button",
                                    class: "operon-workflow-inspector-delete",
                                    "data-testid": "workflow-inspector-delete-node",
                                    title: "Delete this node and any connected edges",
                                    onclick: move |_| {
                                        let next = remove_node(&graph_for_delete, sid);
                                        apply.call(next);
                                        selected_node.set(None);
                                    },
                                    "Delete"
                                }
                                button {
                                    r#type: "button",
                                    class: "operon-workflow-inspector-close",
                                    onclick: move |_| selected_node.set(None),
                                    "\u{2715}"
                                }
                            }
                            label { class: "operon-workflow-inspector-row",
                                span { class: "operon-workflow-inspector-label", "extra instructions" }
                                textarea {
                                    class: "operon-workflow-inspector-textarea",
                                    "data-testid": "workflow-inspector-extra",
                                    value: "{extra}",
                                    oninput: move |e| {
                                        if let Some(node) = graph_for_inspector.nodes.get_mut(&sid) {
                                            node.extra_instructions = e.value();
                                        }
                                        apply.call(graph_for_inspector.clone());
                                    },
                                }
                            }
                            // Status picker — flip a node between the
                            // four lifecycle states so the user can
                            // re-trigger a run (Fresh → Dirty), park
                            // a node mid-cascade (→ Running), or
                            // mark a problematic node Error to skip
                            // it. Bumps `graph.version` so dirty
                            // propagation re-evaluates downstream.
                            div { class: "operon-workflow-inspector-row",
                                "data-testid": "workflow-inspector-status-row",
                                span { class: "operon-workflow-inspector-label", "status" }
                                div { class: "operon-workflow-inspector-status-buttons",
                                    {
                                        let states: [(NodeStatus, &str, &str); 4] = [
                                            (NodeStatus::Dirty, "Dirty", "Mark as needing a re-run"),
                                            (NodeStatus::Running, "Running", "Mark as currently running"),
                                            (NodeStatus::Fresh, "Fresh", "Mark as up-to-date (skip on next run)"),
                                            (NodeStatus::Error("manual".into()), "Error", "Mark as failed"),
                                        ];
                                        rsx! {
                                            for (target, label, help) in states {
                                                {
                                                    let is_current = std::mem::discriminant(&current_status)
                                                        == std::mem::discriminant(&target);
                                                    let mut graph_for_apply = graph_for_status.clone();
                                                    let target_for_click = target.clone();
                                                    let class = if is_current {
                                                        "operon-workflow-inspector-status-button operon-workflow-inspector-status-button-active"
                                                    } else {
                                                        "operon-workflow-inspector-status-button"
                                                    };
                                                    rsx! {
                                                        button {
                                                            key: "{label}",
                                                            r#type: "button",
                                                            class: "{class}",
                                                            "data-testid": "workflow-inspector-status-{label}",
                                                            title: "{help}",
                                                            onclick: move |_| {
                                                                if let Some(node) = graph_for_apply.nodes.get_mut(&sid) {
                                                                    node.status = target_for_click.clone();
                                                                }
                                                                graph_for_apply.version =
                                                                    graph_for_apply.version.saturating_add(1);
                                                                apply.call(graph_for_apply.clone());
                                                            },
                                                            "{label}"
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            // Last-output panel. Reads the cached
                            // output file off disk via a `use_future`
                            // inside the child component so the read
                            // doesn't block render. Keyed on
                            // (path, graph.version) so a cascade
                            // re-run invalidates the cached read.
                            div { class: "operon-workflow-inspector-row",
                                "data-testid": "workflow-inspector-output-row",
                                span { class: "operon-workflow-inspector-label", "last output" }
                                if let Some(path) = sn.cached_output_path.as_ref() {
                                    InspectorOutputPanel {
                                        key: "{path.display()}-{g.version}",
                                        path: path.clone(),
                                    }
                                } else {
                                    div {
                                        class: "operon-workflow-inspector-output operon-workflow-inspector-output-empty",
                                        "data-testid": "workflow-inspector-output-empty",
                                        "No output yet \u{2014} click \u{25B6} on this node (or \u{25B6} Run all dirty) to produce one."
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
                                pan_x_setter.set(coords.x - world_x * new_zoom);
                                pan_y_setter.set(coords.y - world_y * new_zoom);
                                zoom_setter.set(new_zoom);
                            } else {
                                // Plain wheel = pan in screen space.
                                pan_x_setter.with_mut(|v| *v -= delta.x);
                                pan_y_setter.with_mut(|v| *v -= delta.y);
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
                            // Node-position drag: mutate graph + push.
                            // Divide the client delta by zoom because
                            // node positions are in world coords and
                            // the inner <g> scales them by `zoom` for
                            // display.
                            if let Some(cur) = *drag.read() {
                                let dx = (coords.x - cur.start_client_x) / cur_zoom;
                                let dy = (coords.y - cur.start_client_y) / cur_zoom;
                                if let Some(node) = graph.nodes.get_mut(&cur.node) {
                                    node.position = (
                                        cur.start_node_x + dx,
                                        cur.start_node_y + dy,
                                    );
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
                        move |_| {
                            drag.set(None);
                            pan_drag.set(None);
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
                            // Cascade-visualization: amber stroke for
                            // sibling Depends-on cross-edges; default
                            // black for skill-DAG and parent-child
                            // edges.
                            let kind_suffix = match e.edge_kind.as_deref() {
                                Some("depends_on") => " operon-workflow-edge-depends-on",
                                Some("parent_child") => " operon-workflow-edge-parent-child",
                                _ => "",
                            };
                            let edge_class = if is_selected_edge {
                                format!("operon-workflow-edge operon-workflow-edge-selected{kind_suffix}")
                            } else {
                                format!("operon-workflow-edge{kind_suffix}")
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
                                        selected_node.set(None);
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
                            let on_run_node = move |evt: dioxus::events::MouseEvent| {
                                evt.stop_propagation();
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
                            let on_node_mousedown = move |evt: dioxus::events::MouseEvent| {
                                evt.stop_propagation();
                                let coords = evt.client_coordinates();
                                drag.set(Some(DragState {
                                    node: node_id,
                                    start_client_x: coords.x,
                                    start_client_y: coords.y,
                                    start_node_x: n_x,
                                    start_node_y: n_y,
                                }));
                                selected_node.set(Some(node_id));
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
                                }
                                edge_drag.set(None);
                            };
                            // Input-handle mousedown still has to
                            // stop_propagation so it doesn't trigger
                            // a node-position drag.
                            let on_input_mousedown = move |evt: dioxus::events::MouseEvent| {
                                evt.stop_propagation();
                            };
                            let is_selected = *selected_node.read() == Some(node_id);
                            let group_class = if is_selected {
                                "operon-workflow-node-group operon-workflow-node-group-selected"
                            } else {
                                "operon-workflow-node-group"
                            };
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
                                        width: "{NODE_W}",
                                        height: "{NODE_H}",
                                        rx: "8",
                                        ry: "8",
                                    }
                                    if n.is_artifact_snapshot {
                                        text {
                                            class: "operon-workflow-node-kind-badge",
                                            x: "12",
                                            y: "14",
                                            "{n.kind_label.clone().unwrap_or_default()}"
                                        }
                                    }
                                    text {
                                        class: "operon-workflow-node-title",
                                        x: "12",
                                        y: "26",
                                        "{n.label}"
                                    }
                                    text {
                                        class: "operon-workflow-node-status",
                                        x: "12",
                                        y: "48",
                                        "{status_label(&n.status)}"
                                    }
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
                                    // Explicit "View" affordance —
                                    // clicking the rect already
                                    // selects the node (mousedown
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
                                            selected_node.set(Some(node_id));
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
                                        text {
                                            x: "10",
                                            y: "14",
                                            "text-anchor": "middle",
                                            class: "operon-workflow-node-run-glyph",
                                            "\u{25B6}"
                                        }
                                    }
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
    let note_repo_for_picker = note_repo.clone();
    let project_repo_for_seed = project_repo.clone();
    let on_apply = props.on_apply;
    let graph_text = props.graph_text.clone();
    let note_id_str = props.note_id.clone();
    let apply_graph = props.apply_graph;

    let mut picker_open = use_signal(|| false);
    let mut picker_options: Signal<Vec<LocalNote>> = use_signal(Vec::new);

    // Run-all-dirty: parse current graph, topo-sort dirty nodes, run
    // each sequentially via the executor, applying the mutated graph
    // after every node so the canvas reflects progress in real time.
    let note_id_for_run = note_id_str.clone();
    let graph_text_for_run = graph_text.clone();
    let on_run_all = move |_| {
        eprintln!(
            "operon: ▶ Run all dirty CLICKED note_id={} graph_text_len={}",
            note_id_for_run,
            graph_text_for_run.len()
        );
        let initial = match serde_json::from_str::<WorkflowGraph>(&graph_text_for_run) {
            Ok(g) => g,
            Err(e) => {
                eprintln!(
                    "operon: ▶ Run all dirty BAIL — graph_text JSON parse: {e}"
                );
                return;
            }
        };
        spawn_run_cascade(
            note_id_for_run.clone(),
            initial,
            apply_graph,
            active_session_signal,
            active_scope_signal,
        );
    };

    let open_picker = move |_| {
        let note_uuid = match Uuid::parse_str(&note_id_str) {
            Ok(u) => u,
            Err(_) => return,
        };
        let project_id = match note_repo_for_picker.find_project_for_note(note_uuid) {
            Ok(Some(p)) => p,
            _ => return,
        };
        let mut skills: Vec<LocalNote> = note_repo_for_picker
            .list_for_project(project_id)
            .unwrap_or_default()
            .into_iter()
            .filter(|n| matches!(n.kind, NoteKind::Skill))
            .collect();
        skills.sort_by(|a, b| a.title.cmp(&b.title));
        picker_options.set(skills);
        picker_open.set(true);
    };

    // Seed-pipeline: bulk-add every numbered skill in the project as a
    // chained Dirty node, sequentially connected by edges, in numeric
    // order. Reuses the same builder the cascade-workflow auto-seeder
    // uses (`cascade_graph::append_numbered_skill_chain`) so manual
    // and automatic seeding stay in sync.
    //
    // The seed is filtered by the project's cascade-stages selection
    // (`<repo>/.operon/cascade-stages.json`) — the same checkboxes
    // surfaced from a Requirements artifact's "Pipeline stages" panel.
    // So if the user has unchecked stages 5–10 there, "Seed pipeline"
    // here only adds 01–04. Absent sidecar = "all stages enabled" via
    // `stages_sidecar::resolve_or_all`'s fallback.
    let note_repo_for_seed = note_repo.clone();
    let note_id_for_seed = props.note_id.clone();
    let graph_text_for_seed = graph_text.clone();
    let on_seed_pipeline = move |_| {
        let note_uuid = match Uuid::parse_str(&note_id_for_seed) {
            Ok(u) => u,
            Err(_) => return,
        };
        let project_id = match note_repo_for_seed.find_project_for_note(note_uuid) {
            Ok(Some(p)) => p,
            _ => return,
        };
        let mut project_skills: Vec<LocalNote> = note_repo_for_seed
            .list_for_project(project_id)
            .unwrap_or_default()
            .into_iter()
            .filter(|n| matches!(n.kind, NoteKind::Skill))
            .collect();
        // Apply the cascade-stages.json filter — only seed checked
        // stages. We need the project's repo_path to find the sidecar;
        // when the project isn't repo-bound we fall through and seed
        // every numbered skill (matches the Play button's behavior in
        // the same situation).
        let repo_path: Option<std::path::PathBuf> = project_repo_for_seed
            .list()
            .ok()
            .and_then(|all| all.into_iter().find(|p| p.id == project_id))
            .and_then(|p| p.repo_path);
        if let Some(path) = repo_path.as_ref() {
            let all_ids: std::collections::HashSet<Uuid> =
                project_skills.iter().map(|n| n.id).collect();
            let enabled = crate::plugins::artifact::cascade::stages_sidecar::resolve_or_all(
                path, &all_ids,
            );
            project_skills.retain(|n| enabled.contains(&n.id));
        }
        let current = match serde_json::from_str::<WorkflowGraph>(&graph_text_for_seed) {
            Ok(g) => g,
            Err(_) if graph_text_for_seed.trim().is_empty() => WorkflowGraph::new(),
            Err(_) => return,
        };
        let next = crate::plugins::artifact::cascade_graph::append_numbered_skill_chain(
            current,
            &project_skills,
        );
        on_apply.call(serialize(&next));
    };

    rsx! {
        div { class: "operon-workflow-toolbar",
            "data-testid": "workflow-toolbar",
            button {
                r#type: "button",
                class: "operon-workflow-toolbar-button",
                "data-testid": "workflow-toolbar-add",
                onclick: open_picker,
                "+ Add skill node"
            }
            button {
                r#type: "button",
                class: "operon-workflow-toolbar-button",
                "data-testid": "workflow-toolbar-seed",
                title: "Add every numbered skill in the project as a chained Dirty node, in numeric order",
                onclick: on_seed_pipeline,
                "+ Seed pipeline"
            }
            button {
                r#type: "button",
                class: "operon-workflow-toolbar-button",
                "data-testid": "workflow-toolbar-run",
                title: "Run every Dirty node in topological order",
                onclick: on_run_all,
                "\u{25B6} Run all dirty"
            }
            if *picker_open.read() {
                // Click-outside dismissal: a transparent fixed-position
                // backdrop catches clicks anywhere outside the picker
                // panel. The panel itself stops propagation so clicks
                // on its options don't bubble up here and close it
                // before the option's onclick handler runs.
                div {
                    class: "operon-workflow-skill-picker-backdrop",
                    "data-testid": "workflow-skill-picker-backdrop",
                    onclick: move |_| picker_open.set(false),
                }
                div {
                    class: "operon-workflow-skill-picker",
                    "data-testid": "workflow-skill-picker",
                    onclick: move |evt: dioxus::events::MouseEvent| {
                        evt.stop_propagation();
                    },
                    div { class: "operon-workflow-skill-picker-header",
                        span { "Pick a skill to add" }
                        button {
                            r#type: "button",
                            class: "operon-workflow-skill-picker-close",
                            onclick: move |_| picker_open.set(false),
                            "\u{2715}"
                        }
                    }
                    if picker_options.read().is_empty() {
                        div { class: "operon-workflow-skill-picker-empty",
                            "No skill notes in this project yet \u{2014} create one with + \u{2192} Skill in the explorer."
                        }
                    } else {
                        ul { class: "operon-workflow-skill-picker-list",
                            for skill in picker_options.read().iter() {
                                {
                                    let skill_id = skill.id;
                                    let label = skill.title.clone();
                                    let graph_text = graph_text.clone();
                                    let on_apply = on_apply;
                                    rsx! {
                                        li {
                                            key: "{skill_id}",
                                            button {
                                                r#type: "button",
                                                class: "operon-workflow-skill-picker-item",
                                                onclick: move |_| {
                                                    let next = append_node_to_graph(&graph_text, skill_id);
                                                    on_apply.call(next);
                                                    picker_open.set(false);
                                                },
                                                "{label}"
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
    /// Kind badge ("Epic" / "Feature" / etc.) when this is an artifact
    /// snapshot. None for skill nodes.
    kind_label: Option<String>,
}

#[derive(Clone, PartialEq)]
struct EdgeRender {
    id: EdgeId,
    from_x: f64,
    from_y: f64,
    to_x: f64,
    to_y: f64,
    /// Cascade-visualization marker. `Some("depends_on")` is rendered
    /// in amber (CSS class `operon-workflow-edge-depends-on`) so the
    /// user can distinguish parent/child structure from inter-sibling
    /// dependencies.
    edge_kind: Option<String>,
}

fn parse_or_default(content: &str) -> WorkflowGraph {
    if content.trim().is_empty() {
        return WorkflowGraph::new();
    }
    serde_json::from_str(content).unwrap_or_default()
}

fn serialize(graph: &WorkflowGraph) -> String {
    serde_json::to_string_pretty(graph).unwrap_or_else(|_| "{}".to_string())
}

fn node_label(n: &Node, skill_titles: &HashMap<Uuid, String>) -> String {
    if n.is_artifact_snapshot {
        // Cascade snapshot: prefer the artifact's title (e.g.
        // "epic-01-realtime-collaboration"); fall back to the kind
        // label ("Epic") + UUID short prefix when title is missing.
        if let Some(title) = n.artifact_title.as_ref() {
            return title.clone();
        }
        let head: String = n
            .artifact_ref
            .map(|id| id.to_string().chars().take(8).collect())
            .unwrap_or_default();
        let kind = n.artifact_kind_label.clone().unwrap_or_else(|| "Artifact".into());
        return format!("{kind} {head}");
    }
    // Skill node: prefer the resolved skill title; fall back to the
    // UUID short prefix when the skill row isn't in the lookup map
    // (e.g. read-only WorkflowView, or skill note was deleted).
    if let Some(title) = skill_titles.get(&n.skill_note_id) {
        return title.clone();
    }
    let id = n.skill_note_id.to_string();
    let head: String = id.chars().take(8).collect();
    format!("skill {head}")
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
    let node = Node {
        id,
        skill_note_id,
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
        let operon_session = cascade_session_id;
        eprintln!(
            "operon: cascade resolved session={operon_session} repo={} project_id={:?} \
             rail_persist={}",
            repo_path.display(),
            project_id_opt,
            transcript_sink.is_some(),
        );
        plugin.bind_session(operon_session, repo_path.clone());

        // Switch the companion's rail to the cascade's session so the
        // user sees the streaming transcript, "Claude is thinking…"
        // loader, and tool-call cards live as the cascade runs.
        // Mirrors what the artifact view's spawn_runner does. The
        // signals are plumbed from the toolbar's component body so
        // the writes land on a runtime-bound Signal.
        if let Some(project_id) = project_id_opt {
            active_scope_signal.set(ChatScope::Project(project_id));
        }
        active_session_signal.set(Some(operon_session));
        eprintln!(
            "operon: cascade rail-switch active_session={operon_session} \
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

        for node_id in order {
            eprintln!("operon: cascade running node {node_id}");
            if let Some(n) = graph.nodes.get_mut(&node_id) {
                n.status = NodeStatus::Running;
            }
            apply_graph.call(graph.clone());
            match run_one_node(
                &mut graph,
                node_id,
                workflow_id,
                operon_session,
                &repo_path,
                plugin.clone(),
                &persistence,
                &note_repo,
                transcript_sink.clone(),
            )
            .await
            {
                Err(e) => {
                    eprintln!("operon: cascade node {node_id} failed: {e}");
                    if let Some(n) = graph.nodes.get_mut(&node_id) {
                        n.status = NodeStatus::Error(format!("{e}"));
                    }
                    apply_graph.call(graph.clone());
                    break;
                }
                Ok(NodeRunOk { skill_note_id, produced }) => {
                    eprintln!(
                        "operon: cascade node {node_id} completed \
                         produced={} (skill {skill_note_id})",
                        produced.len()
                    );
                    // Best-effort import of every produced file as its
                    // own Outputs note. Only when the workflow has a
                    // resolvable project.
                    if let Some(project_id) = project_id_opt {
                        match upsert_output_notes(
                            &note_repo,
                            &persistence,
                            project_id,
                            &mut graph,
                            node_id,
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
                }
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
        match run_one_node(
            &mut graph,
            node_id,
            workflow_id,
            operon_session,
            &repo_path,
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
            Ok(NodeRunOk { skill_note_id, produced }) => {
                eprintln!(
                    "operon: per-node run produced={} (skill {skill_note_id})",
                    produced.len()
                );
                if let Some(project_id) = project_id_opt {
                    match upsert_output_notes(
                        &note_repo,
                        &persistence,
                        project_id,
                        &mut graph,
                        node_id,
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
}

#[allow(clippy::too_many_arguments)]
async fn run_one_node(
    graph: &mut WorkflowGraph,
    node_id: NodeId,
    workflow_id: Uuid,
    operon_session: Uuid,
    repo_path: &std::path::Path,
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
    eprintln!(
        "operon: run_one_node [{node_id}] skill_version={:?} body_len={}",
        skill_version,
        skill_body.len()
    );

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
        repo_path.to_path_buf(),
        workflow_id,
        node_id,
        &node_snapshot,
        &skill_body,
        &skill_version,
        &skill_slug,
        &upstream,
        &graph_for_hash,
        transcript_sink,
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
    produced: &[(std::path::PathBuf, String)],
) -> Result<Vec<Uuid>, String> {
    // 1. One project-scoped Outputs folder. Found by title at root —
    //    co-opts a user-created folder if one exists with the same
    //    name; otherwise created.
    let folder_id = ensure_outputs_folder(note_repo, project_id)?;

    // 2. One Outputs note per produced file. Title is the file's stem
    //    (e.g. `epic-01-core-timer-engine.md` → `epic-01-core-timer-engine`).
    //    Existing siblings under the Outputs folder with the same title
    //    are reused — body overwritten — so re-runs don't pile up
    //    duplicates. The lead file's note id is also stamped on
    //    `cached_output_note_id` so the inspector's "View in tab"
    //    button still has a single anchor.
    let existing_outputs: Vec<LocalNote> = note_repo
        .list_for_project(project_id)
        .unwrap_or_default()
        .into_iter()
        .filter(|n| {
            n.parent_id == Some(folder_id) && matches!(n.kind, NoteKind::Markdown)
        })
        .collect();

    let mut imported: Vec<Uuid> = Vec::with_capacity(produced.len());
    for (idx, (path, body)) in produced.iter().enumerate() {
        let title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output")
            .to_string();
        let existing_id = existing_outputs.iter().find(|n| n.title == title).map(|n| n.id);
        let row_id = match existing_id {
            Some(id) => id,
            None => {
                let row = note_repo
                    .create_with_kind(
                        project_id,
                        Some(folder_id),
                        &title,
                        NoteKind::Markdown,
                    )
                    .map_err(|e| format!("create output note '{title}': {e}"))?;
                row.id
            }
        };
        persistence
            .save(&row_id.to_string(), body.as_bytes())
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
            },
        );
        let map = layout(&g);
        assert!(map.contains_key(&a));
        assert!(!map.contains_key(&b));
    }
}
