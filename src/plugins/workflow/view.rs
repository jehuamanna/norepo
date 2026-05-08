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
use crate::persistence::Persistence;
use crate::plugins::workflow::engine::{propagate_dirty, topo_order_dirty, SkillBag, SkillSnapshot, hash_body};
use crate::plugins::workflow::executor::{collect_upstream_outputs, run_node, RunArtifact};
use crate::plugins::workflow::state::{Node, NodeId, NodeStatus, WorkflowGraph};
use crate::shell::companion_state::ClaudeCodePluginCtx;

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

#[component]
fn WorkflowCanvas(props: WorkflowCanvasProps) -> Element {
    let g = props.graph;
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
            label: skill_label(n),
            status: n.status.clone(),
        })
        .collect();
    let edges: Vec<EdgeRender> = g
        .edges
        .iter()
        .filter_map(|e| {
            let from = nodes.iter().find(|n| n.id == e.from)?;
            let to = nodes.iter().find(|n| n.id == e.to)?;
            Some(EdgeRender {
                from_x: from.x + NODE_W,
                from_y: from.y + NODE_H / 2.0,
                to_x: to.x,
                to_y: to.y + NODE_H / 2.0,
            })
        })
        .collect();

    let (min_x, min_y, max_x, max_y) = bounds(&nodes);
    let viewbox = format!("{min_x} {min_y} {} {}", max_x - min_x, max_y - min_y);

    rsx! {
        div { class: "operon-workflow-canvas",
            "data-testid": "workflow-canvas",
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
                    "viewBox": "{viewbox}",
                    preserve_aspect_ratio: "xMidYMid meet",
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
                    for e in edges.iter() {
                        path {
                            class: "operon-workflow-edge",
                            d: "M {e.from_x} {e.from_y} C {e.from_x + 60.0} {e.from_y}, {e.to_x - 60.0} {e.to_y}, {e.to_x} {e.to_y}",
                            "marker-end": "url(#operon-workflow-arrow)",
                        }
                    }
                    for n in nodes.iter() {
                        {
                            let node_id_for_run = n.id;
                            let note_id_for_run = props.note_id.clone();
                            let apply_for_run = props.apply_graph;
                            let graph_for_run = g.clone();
                            let on_run_node = move |evt: dioxus::events::MouseEvent| {
                                evt.stop_propagation();
                                spawn_run_node(
                                    note_id_for_run.clone(),
                                    node_id_for_run,
                                    graph_for_run.clone(),
                                    apply_for_run,
                                );
                            };
                            rsx! {
                                g {
                                    class: "operon-workflow-node-group",
                                    "data-node-id": "{n.id}",
                                    transform: "translate({n.x}, {n.y})",
                                    rect {
                                        class: status_class(&n.status),
                                        "data-testid": "workflow-node",
                                        width: "{NODE_W}",
                                        height: "{NODE_H}",
                                        rx: "8",
                                        ry: "8",
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
                                    g {
                                        class: "operon-workflow-node-run",
                                        "data-testid": "workflow-node-run",
                                        transform: "translate({NODE_W - 28.0}, 8.0)",
                                        onclick: on_run_node,
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
                }
            }
        }
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
    let LocalProjectRepo(_project_repo) = use_context();
    let note_repo_for_picker = note_repo.clone();
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
        let initial = serde_json::from_str::<WorkflowGraph>(&graph_text_for_run)
            .unwrap_or_default();
        spawn_run_cascade(note_id_for_run.clone(), initial, apply_graph);
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
                "data-testid": "workflow-toolbar-run",
                title: "Run every Dirty node in topological order",
                onclick: on_run_all,
                "\u{25B6} Run all dirty"
            }
            if *picker_open.read() {
                div {
                    class: "operon-workflow-skill-picker",
                    "data-testid": "workflow-skill-picker",
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
}

#[derive(Clone, PartialEq)]
struct EdgeRender {
    from_x: f64,
    from_y: f64,
    to_x: f64,
    to_y: f64,
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

fn skill_label(n: &Node) -> String {
    // Prefer a short prefix of the skill_note_id for now; M3c will
    // join with the actual skill title.
    let id = n.skill_note_id.to_string();
    let head: String = id.chars().take(8).collect();
    format!("skill {head}")
}

fn status_class(s: &NodeStatus) -> &'static str {
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

fn bounds(nodes: &[NodeRender]) -> (f64, f64, f64, f64) {
    let mut min_x = 0.0f64;
    let mut min_y = 0.0f64;
    let mut max_x = 600.0f64;
    let mut max_y = 320.0f64;
    for n in nodes {
        min_x = min_x.min(n.x - 20.0);
        min_y = min_y.min(n.y - 20.0);
        max_x = max_x.max(n.x + NODE_W + 20.0);
        max_y = max_y.max(n.y + NODE_H + 20.0);
    }
    (min_x, min_y, max_x, max_y)
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
    spawn(async move {
        // 1. Resolve project + repo path.
        let Some((operon_session, repo_path)) =
            resolve_project_session(workflow_id, &note_repo, &project_repo)
        else {
            return;
        };
        plugin.bind_session(operon_session, repo_path.clone());

        // 2. Order the dirty subset.
        let order = match topo_order_dirty(&graph) {
            Ok(o) => o,
            Err(_) => return,
        };

        for node_id in order {
            if let Err(e) = run_one_node(
                &mut graph,
                node_id,
                workflow_id,
                operon_session,
                &repo_path,
                plugin.clone(),
                &persistence,
            )
            .await
            {
                if let Some(n) = graph.nodes.get_mut(&node_id) {
                    n.status = NodeStatus::Error(format!("{e}"));
                }
                apply_graph.call(graph.clone());
                break;
            }
            apply_graph.call(graph.clone());
        }
    });
}

/// Spawn a single-node run. Same plumbing as cascade but for one node.
fn spawn_run_node(
    note_id_str: String,
    node_id: NodeId,
    mut graph: WorkflowGraph,
    apply_graph: Callback<WorkflowGraph>,
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
    spawn(async move {
        let Some((operon_session, repo_path)) =
            resolve_project_session(workflow_id, &note_repo, &project_repo)
        else {
            return;
        };
        plugin.bind_session(operon_session, repo_path.clone());
        if let Err(e) = run_one_node(
            &mut graph,
            node_id,
            workflow_id,
            operon_session,
            &repo_path,
            plugin.clone(),
            &persistence,
        )
        .await
        {
            if let Some(n) = graph.nodes.get_mut(&node_id) {
                n.status = NodeStatus::Error(format!("{e}"));
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

async fn run_one_node(
    graph: &mut WorkflowGraph,
    node_id: NodeId,
    workflow_id: Uuid,
    operon_session: Uuid,
    repo_path: &std::path::Path,
    plugin: Arc<operon_plugins_claude_code::ClaudeCodeChatPlugin>,
    persistence: &Arc<dyn Persistence>,
) -> Result<(), String> {
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
    let skill_body_bytes = persistence
        .load(&node_snapshot.skill_note_id.to_string())
        .await
        .map_err(|e| format!("load skill {}: {e}", node_snapshot.skill_note_id))?;
    let skill_body = String::from_utf8(skill_body_bytes)
        .map_err(|e| format!("skill body utf8: {e}"))?;
    let (frontmatter, _body) = crate::plugins::skill::frontmatter::split(&skill_body);
    let skill_version = frontmatter
        .as_ref()
        .and_then(|fm| crate::plugins::skill::frontmatter::field(fm, "skill_version"))
        .unwrap_or("")
        .to_string();

    // Gather upstream outputs from disk (already-Fresh upstreams have
    // a `cached_output_path` we can read).
    let upstream =
        collect_upstream_outputs(graph, node_id).map_err(|e| format!("upstream: {e}"))?;

    // Snapshot the graph for hashing (the executor's run_node hashes
    // against this view).
    let graph_for_hash = graph.clone();

    let artifact: RunArtifact = run_node(
        plugin,
        operon_session,
        repo_path.to_path_buf(),
        workflow_id,
        node_id,
        &node_snapshot,
        &skill_body,
        &skill_version,
        &upstream,
        &graph_for_hash,
    )
    .await
    .map_err(|e| format!("{e}"))?;

    // Commit results and propagate dirty downstream.
    if let Some(n) = graph.nodes.get_mut(&node_id) {
        n.cached_output_path = Some(artifact.output_path);
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
    Ok(())
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
            },
        );
        let map = layout(&g);
        assert!(map.contains_key(&a));
        assert!(!map.contains_key(&b));
    }
}
