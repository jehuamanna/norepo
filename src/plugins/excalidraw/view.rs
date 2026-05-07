//! SVG-backed sketcher. Pen + Rectangle + Erase tools, freehand strokes
//! captured as point lists during a mouse drag and committed as a
//! `freedraw` element on mouseup.

use dioxus::prelude::*;

use super::model::{ExcalidrawDoc, ExcalidrawElement, Point};

const SVG_BG: &str = "background: #181818; cursor: crosshair; width: 100%; height: 100%; display: block;";

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Tool {
    Pen,
    Rectangle,
    Eraser,
}

#[component]
pub fn ExcalidrawView(doc: ExcalidrawDoc) -> Element {
    rsx! {
        div {
            class: "operon-excalidraw-view",
            "data-testid": "excalidraw-view",
            style: "width: 100%; height: 100%; overflow: auto; background: #181818;",
            svg {
                xmlns: "http://www.w3.org/2000/svg",
                style: "{SVG_BG}",
                {render_elements(&doc.elements)}
            }
        }
    }
}

#[component]
pub fn ExcalidrawEditor(initial: String, on_change: EventHandler<String>) -> Element {
    let mut doc: Signal<ExcalidrawDoc> = use_signal(|| ExcalidrawDoc::parse(&initial));
    let mut tool: Signal<Tool> = use_signal(|| Tool::Pen);
    // In-progress freehand stroke or rectangle drag. The element shape
    // matches a tool: Pen captures `points`; Rectangle captures
    // (start_x, start_y, current_x, current_y).
    let mut pen_points: Signal<Vec<Point>> = use_signal(Vec::new);
    let mut rect_drag: Signal<Option<(f64, f64, f64, f64)>> = use_signal(|| None);

    let push = move |d: &ExcalidrawDoc| on_change.call(d.to_json());

    let on_svg_mousedown = move |evt: Event<MouseData>| {
        let coords = evt.element_coordinates();
        match *tool.read() {
            Tool::Pen => {
                pen_points.set(vec![Point {
                    x: coords.x,
                    y: coords.y,
                }]);
            }
            Tool::Rectangle => {
                rect_drag.set(Some((coords.x, coords.y, coords.x, coords.y)));
            }
            Tool::Eraser => {
                // Eraser removes the topmost element whose bounding box
                // contains the cursor.
                doc.with_mut(|d| remove_at(d, coords.x, coords.y));
                let snap = doc.read().clone();
                push(&snap);
            }
        }
    };

    let on_svg_mousemove = move |evt: Event<MouseData>| {
        let coords = evt.element_coordinates();
        match *tool.read() {
            Tool::Pen => {
                if !pen_points.read().is_empty() {
                    pen_points.with_mut(|p| {
                        p.push(Point {
                            x: coords.x,
                            y: coords.y,
                        });
                    });
                }
            }
            Tool::Rectangle => {
                let snap = *rect_drag.read();
                if let Some((sx, sy, _, _)) = snap {
                    rect_drag.set(Some((sx, sy, coords.x, coords.y)));
                }
            }
            Tool::Eraser => {}
        }
    };

    let on_svg_mouseup = move |_: Event<MouseData>| {
        match *tool.read() {
            Tool::Pen => {
                let pts = pen_points.read().clone();
                if pts.len() > 1 {
                    doc.with_mut(|d| {
                        d.elements.push(ExcalidrawElement::FreeDraw {
                            id: ExcalidrawDoc::fresh_id(),
                            points: pts,
                            stroke_color: "#ddd".into(),
                            stroke_width: 2.0,
                        });
                    });
                    let snap = doc.read().clone();
                    push(&snap);
                }
                pen_points.set(Vec::new());
            }
            Tool::Rectangle => {
                if let Some((sx, sy, ex, ey)) = *rect_drag.read() {
                    let x = sx.min(ex);
                    let y = sy.min(ey);
                    let w = (ex - sx).abs();
                    let h = (ey - sy).abs();
                    if w > 2.0 && h > 2.0 {
                        doc.with_mut(|d| {
                            d.elements.push(ExcalidrawElement::Rectangle {
                                id: ExcalidrawDoc::fresh_id(),
                                x,
                                y,
                                width: w,
                                height: h,
                                stroke_color: "#ddd".into(),
                                stroke_width: 2.0,
                            });
                        });
                        let snap = doc.read().clone();
                        push(&snap);
                    }
                }
                rect_drag.set(None);
            }
            Tool::Eraser => {}
        }
    };

    let clear_all = move |_| {
        doc.with_mut(|d| d.elements.clear());
        let snap = doc.read().clone();
        push(&snap);
    };

    let tool_now = *tool.read();
    let tool_btn_style = |t: Tool| -> &'static str {
        if tool_now == t {
            "padding: 0.35rem 0.6rem; border-radius: 0.25rem; cursor: pointer; background: var(--operon-accent, #4a7); color: #000; border: 0;"
        } else {
            "padding: 0.35rem 0.6rem; border-radius: 0.25rem; cursor: pointer; background: var(--operon-panel, #222); color: inherit; border: 1px solid var(--operon-border, #333);"
        }
    };

    rsx! {
        div {
            class: "operon-excalidraw-editor",
            "data-testid": "excalidraw-editor",
            style: "display: flex; flex-direction: column; height: 100%;",
            div {
                class: "operon-excalidraw-toolbar",
                style: "display: flex; gap: 0.4rem; padding: 0.35rem 0.5rem; border-bottom: 1px solid var(--operon-border, #333); align-items: center;",
                button {
                    r#type: "button",
                    "data-testid": "excalidraw-tool-pen",
                    "aria-pressed": if tool_now == Tool::Pen { "true" } else { "false" },
                    style: "{tool_btn_style(Tool::Pen)}",
                    onclick: move |_| tool.set(Tool::Pen),
                    "Pen"
                }
                button {
                    r#type: "button",
                    "data-testid": "excalidraw-tool-rect",
                    "aria-pressed": if tool_now == Tool::Rectangle { "true" } else { "false" },
                    style: "{tool_btn_style(Tool::Rectangle)}",
                    onclick: move |_| tool.set(Tool::Rectangle),
                    "Rectangle"
                }
                button {
                    r#type: "button",
                    "data-testid": "excalidraw-tool-eraser",
                    "aria-pressed": if tool_now == Tool::Eraser { "true" } else { "false" },
                    style: "{tool_btn_style(Tool::Eraser)}",
                    onclick: move |_| tool.set(Tool::Eraser),
                    "Eraser"
                }
                span { style: "flex: 1;" }
                button {
                    r#type: "button",
                    "data-testid": "excalidraw-clear",
                    style: "padding: 0.35rem 0.6rem; border-radius: 0.25rem; cursor: pointer; opacity: 0.7;",
                    onclick: clear_all,
                    "Clear"
                }
            }
            div {
                style: "flex: 1; min-height: 0; overflow: hidden;",
                svg {
                    xmlns: "http://www.w3.org/2000/svg",
                    style: "{SVG_BG}",
                    onmousedown: on_svg_mousedown,
                    onmousemove: on_svg_mousemove,
                    onmouseup: on_svg_mouseup,
                    onmouseleave: on_svg_mouseup,
                    {render_elements(&doc.read().elements)}
                    // Live preview of the in-flight stroke / rectangle.
                    {render_in_flight(&pen_points.read(), *rect_drag.read())}
                }
            }
        }
    }
}

fn render_elements(elements: &[ExcalidrawElement]) -> Element {
    rsx! {
        for el in elements.iter() {
            {
                match el {
                    ExcalidrawElement::FreeDraw { id, points, stroke_color, stroke_width } => {
                        let d = points_to_path(points);
                        let id = id.clone();
                        let color = stroke_color.clone();
                        let w = *stroke_width;
                        rsx! {
                            path {
                                key: "{id}",
                                d: "{d}",
                                stroke: "{color}",
                                "stroke-width": "{w}",
                                fill: "none",
                                "stroke-linecap": "round",
                                "stroke-linejoin": "round",
                            }
                        }
                    }
                    ExcalidrawElement::Rectangle { id, x, y, width, height, stroke_color, stroke_width } => {
                        let id = id.clone();
                        let color = stroke_color.clone();
                        let w = *stroke_width;
                        rsx! {
                            rect {
                                key: "{id}",
                                x: "{x}",
                                y: "{y}",
                                width: "{width}",
                                height: "{height}",
                                stroke: "{color}",
                                "stroke-width": "{w}",
                                fill: "none",
                            }
                        }
                    }
                }
            }
        }
    }
}

fn render_in_flight(pen_points: &[Point], rect: Option<(f64, f64, f64, f64)>) -> Element {
    let pen = if pen_points.len() > 1 {
        let d = points_to_path(pen_points);
        rsx! {
            path {
                d: "{d}",
                stroke: "#ddd",
                "stroke-width": "2",
                fill: "none",
                "stroke-linecap": "round",
                "stroke-linejoin": "round",
                opacity: "0.8",
            }
        }
    } else {
        rsx! {}
    };
    let rect_el = if let Some((sx, sy, ex, ey)) = rect {
        let x = sx.min(ex);
        let y = sy.min(ey);
        let w = (ex - sx).abs();
        let h = (ey - sy).abs();
        rsx! {
            rect {
                x: "{x}",
                y: "{y}",
                width: "{w}",
                height: "{h}",
                stroke: "#ddd",
                "stroke-width": "2",
                fill: "none",
                opacity: "0.6",
            }
        }
    } else {
        rsx! {}
    };
    rsx! {
        {pen}
        {rect_el}
    }
}

fn points_to_path(points: &[Point]) -> String {
    let mut out = String::new();
    for (i, p) in points.iter().enumerate() {
        if i == 0 {
            out.push_str(&format!("M {:.2} {:.2}", p.x, p.y));
        } else {
            out.push_str(&format!(" L {:.2} {:.2}", p.x, p.y));
        }
    }
    out
}

fn remove_at(doc: &mut ExcalidrawDoc, x: f64, y: f64) {
    let radius_sq = 12.0_f64.powi(2);
    let mut to_remove: Option<usize> = None;
    for (i, el) in doc.elements.iter().enumerate().rev() {
        match el {
            ExcalidrawElement::Rectangle {
                x: rx,
                y: ry,
                width,
                height,
                ..
            } => {
                if x >= *rx && x <= *rx + *width && y >= *ry && y <= *ry + *height {
                    to_remove = Some(i);
                    break;
                }
            }
            ExcalidrawElement::FreeDraw { points, .. } => {
                if points
                    .iter()
                    .any(|p| (p.x - x).powi(2) + (p.y - y).powi(2) <= radius_sq)
                {
                    to_remove = Some(i);
                    break;
                }
            }
        }
    }
    if let Some(i) = to_remove {
        doc.elements.remove(i);
    }
}
