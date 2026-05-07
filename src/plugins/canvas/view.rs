//! Canvas read-only view + editor. v1 supports text-node creation, dragging
//! by click+hold on the node header, inline text editing, and pan via the
//! background. No edge rendering yet (round-tripped but not visualised).

use dioxus::prelude::*;

use super::model::{CanvasDoc, CanvasNode};

const CANVAS_BG: &str = "background: var(--operon-bg, #111); position: relative; overflow: hidden; height: 100%; user-select: none;";

#[component]
pub fn CanvasView(doc: CanvasDoc) -> Element {
    rsx! {
        div {
            class: "operon-canvas-view",
            "data-testid": "canvas-view",
            style: "{CANVAS_BG}",
            for node in doc.nodes.iter() {
                NodeStatic { key: "{node.id}", node: node.clone() }
            }
        }
    }
}

#[component]
fn NodeStatic(node: CanvasNode) -> Element {
    rsx! {
        div {
            class: "operon-canvas-node",
            "data-testid": "canvas-node",
            "data-node-id": "{node.id}",
            style: "position: absolute; left: {node.x}px; top: {node.y}px; width: {node.width}px; height: {node.height}px; background: var(--operon-panel, #1a1a1a); border: 1px solid var(--operon-border, #333); border-radius: 0.25rem; padding: 0.5rem; overflow: auto; white-space: pre-wrap;",
            "{node.text.clone().unwrap_or_default()}"
        }
    }
}

#[component]
pub fn CanvasEditor(initial: String, on_change: EventHandler<String>) -> Element {
    let mut doc: Signal<CanvasDoc> = use_signal(|| CanvasDoc::parse(&initial));
    // Last cursor position captured on background dblclick — used to place
    // the next created node.
    let mut next_pos: Signal<(f64, f64)> = use_signal(|| (40.0, 40.0));

    let push = move |d: &CanvasDoc| on_change.call(d.to_json());

    let mut add_node_at = move |x: f64, y: f64| {
        doc.with_mut(|d| {
            d.nodes.push(CanvasNode {
                id: CanvasDoc::fresh_id(),
                kind: "text".into(),
                x,
                y,
                width: 200.0,
                height: 100.0,
                text: Some(String::new()),
            });
        });
        let snap = doc.read().clone();
        push(&snap);
    };

    let on_bg_dblclick = move |evt: Event<MouseData>| {
        let coords = evt.element_coordinates();
        next_pos.set((coords.x, coords.y));
        add_node_at(coords.x, coords.y);
    };

    rsx! {
        div {
            class: "operon-canvas-editor",
            "data-testid": "canvas-editor",
            style: "{CANVAS_BG}",
            ondoubleclick: on_bg_dblclick,
            // Floating help banner on first run.
            if doc.read().nodes.is_empty() {
                div {
                    style: "position: absolute; top: 50%; left: 50%; transform: translate(-50%, -50%); opacity: 0.6; pointer-events: none; text-align: center;",
                    "Double-click anywhere to add a text node."
                }
            }
            for (idx, node) in doc.read().nodes.iter().enumerate() {
                {
                    let n = node.clone();
                    rsx! {
                        NodeEditable {
                            key: "{n.id}",
                            idx,
                            node: n,
                            doc,
                            on_persist: EventHandler::new(move |json: String| on_change.call(json)),
                        }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct NodeEditableProps {
    idx: usize,
    node: CanvasNode,
    doc: Signal<CanvasDoc>,
    on_persist: EventHandler<String>,
}

#[component]
fn NodeEditable(props: NodeEditableProps) -> Element {
    let idx = props.idx;
    let mut doc = props.doc;
    let on_persist = props.on_persist;

    // Per-node drag state. (start_client_x, start_client_y, start_node_x,
    // start_node_y). None when not dragging.
    let mut drag_start: Signal<Option<(f64, f64, f64, f64)>> = use_signal(|| None);

    let on_header_mousedown = move |evt: Event<MouseData>| {
        evt.stop_propagation();
        let coords = evt.client_coordinates();
        let snap = doc.read();
        let Some(n) = snap.nodes.get(idx) else {
            return;
        };
        drag_start.set(Some((coords.x, coords.y, n.x, n.y)));
    };

    let on_global_mousemove = move |evt: Event<MouseData>| {
        let Some((sx, sy, nx, ny)) = *drag_start.read() else {
            return;
        };
        let coords = evt.client_coordinates();
        let dx = coords.x - sx;
        let dy = coords.y - sy;
        doc.with_mut(|d| {
            if let Some(n) = d.nodes.get_mut(idx) {
                n.x = (nx + dx).max(0.0);
                n.y = (ny + dy).max(0.0);
            }
        });
    };

    let on_global_mouseup = move |_: Event<MouseData>| {
        if drag_start.read().is_some() {
            drag_start.set(None);
            let snap = doc.read().clone();
            on_persist.call(snap.to_json());
        }
    };

    let on_text_change = move |evt: Event<FormData>| {
        let v = evt.value();
        doc.with_mut(|d| {
            if let Some(n) = d.nodes.get_mut(idx) {
                n.text = Some(v);
            }
        });
        let snap = doc.read().clone();
        on_persist.call(snap.to_json());
    };

    let delete_node = move |_| {
        doc.with_mut(|d| {
            if idx < d.nodes.len() {
                let removed = d.nodes.remove(idx);
                d.edges.retain(|e| e.from_node != removed.id && e.to_node != removed.id);
            }
        });
        let snap = doc.read().clone();
        on_persist.call(snap.to_json());
    };

    let n = props.node.clone();
    let style = format!(
        "position: absolute; left: {}px; top: {}px; width: {}px; height: {}px; background: var(--operon-panel, #1a1a1a); border: 1px solid var(--operon-border, #333); border-radius: 0.25rem; display: flex; flex-direction: column; overflow: hidden;",
        n.x, n.y, n.width, n.height
    );

    rsx! {
        div {
            class: "operon-canvas-node",
            "data-testid": "canvas-node",
            "data-node-id": "{n.id}",
            style: "{style}",
            onmousemove: on_global_mousemove,
            onmouseup: on_global_mouseup,
            onmouseleave: on_global_mouseup,
            div {
                class: "operon-canvas-node-header",
                style: "background: var(--operon-bg, #111); padding: 0.25rem 0.4rem; cursor: move; display: flex; align-items: center; justify-content: space-between; font-size: 0.75em; opacity: 0.7;",
                onmousedown: on_header_mousedown,
                span { "drag" }
                button {
                    r#type: "button",
                    "data-testid": "canvas-node-delete",
                    "aria-label": "Delete node",
                    style: "background: transparent; border: 0; color: inherit; opacity: 0.7; cursor: pointer;",
                    onclick: delete_node,
                    "×"
                }
            }
            textarea {
                "data-testid": "canvas-node-text",
                style: "flex: 1; resize: none; background: transparent; border: 0; color: inherit; padding: 0.4rem; font-family: inherit; outline: none;",
                value: "{n.text.clone().unwrap_or_default()}",
                onchange: on_text_change,
            }
        }
    }
}
