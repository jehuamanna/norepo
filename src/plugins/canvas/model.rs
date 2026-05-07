//! Canvas document model. Mirrors the Obsidian Canvas spec:
//!   { "nodes": [{id, type, x, y, width, height, text?}, …],
//!     "edges": [{id, fromNode, toNode}, …] }
//! v1 only emits and consumes `type: "text"` nodes; unknown node types are
//! preserved verbatim through serde so round-tripping a foreign Canvas file
//! doesn't lose data.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanvasNode {
    pub id: String,
    /// Obsidian uses "text", "file", "link", "group". v1 renders text only.
    #[serde(rename = "type", default = "default_node_type")]
    pub kind: String,
    pub x: f64,
    pub y: f64,
    #[serde(default = "default_width")]
    pub width: f64,
    #[serde(default = "default_height")]
    pub height: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

fn default_node_type() -> String {
    "text".into()
}
fn default_width() -> f64 {
    240.0
}
fn default_height() -> f64 {
    120.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanvasEdge {
    pub id: String,
    #[serde(rename = "fromNode")]
    pub from_node: String,
    #[serde(rename = "toNode")]
    pub to_node: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CanvasDoc {
    #[serde(default)]
    pub nodes: Vec<CanvasNode>,
    #[serde(default)]
    pub edges: Vec<CanvasEdge>,
}

impl CanvasDoc {
    pub fn parse(raw: &str) -> Self {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Self::default();
        }
        match serde_json::from_str::<CanvasDoc>(trimmed) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("operon: canvas parse failed: {e}");
                Self::default()
            }
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self)
            .unwrap_or_else(|_| "{\"nodes\":[],\"edges\":[]}".into())
    }

    pub fn fresh_id() -> String {
        uuid::Uuid::new_v4().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_parses_to_empty_doc() {
        assert!(CanvasDoc::parse("").nodes.is_empty());
    }

    #[test]
    fn round_trip_preserves_nodes_and_edges() {
        let d = CanvasDoc {
            nodes: vec![CanvasNode {
                id: "n1".into(),
                kind: "text".into(),
                x: 10.0,
                y: 20.0,
                width: 200.0,
                height: 100.0,
                text: Some("hello".into()),
            }],
            edges: vec![CanvasEdge {
                id: "e1".into(),
                from_node: "n1".into(),
                to_node: "n1".into(),
            }],
        };
        let json = d.to_json();
        let back = CanvasDoc::parse(&json);
        assert_eq!(back, d);
    }
}
