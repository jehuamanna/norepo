//! Document model for the Operon-v1 Excalidraw plugin.
//!
//! v1 supports two element kinds: `freedraw` (an array of {x, y} points
//! captured during a mouse drag) and `rectangle` (axis-aligned). Schema
//! mirrors a subset of Excalidraw's own JSON so a future swap to the real
//! Excalidraw library is mostly a one-way data migration.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ExcalidrawElement {
    #[serde(rename = "freedraw")]
    FreeDraw {
        id: String,
        points: Vec<Point>,
        #[serde(rename = "strokeColor", default = "default_stroke")]
        stroke_color: String,
        #[serde(rename = "strokeWidth", default = "default_stroke_width")]
        stroke_width: f64,
    },
    #[serde(rename = "rectangle")]
    Rectangle {
        id: String,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        #[serde(rename = "strokeColor", default = "default_stroke")]
        stroke_color: String,
        #[serde(rename = "strokeWidth", default = "default_stroke_width")]
        stroke_width: f64,
    },
}

fn default_stroke() -> String {
    "#ddd".into()
}
fn default_stroke_width() -> f64 {
    2.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExcalidrawDoc {
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub elements: Vec<ExcalidrawElement>,
}

fn default_version() -> String {
    "operon-1".into()
}

impl Default for ExcalidrawDoc {
    fn default() -> Self {
        Self {
            version: default_version(),
            elements: Vec::new(),
        }
    }
}

impl ExcalidrawDoc {
    pub fn parse(raw: &str) -> Self {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Self::default();
        }
        match serde_json::from_str::<ExcalidrawDoc>(trimmed) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("operon: excalidraw parse failed: {e}");
                Self::default()
            }
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self)
            .unwrap_or_else(|_| "{\"version\":\"operon-1\",\"elements\":[]}".into())
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
        let d = ExcalidrawDoc::parse("");
        assert_eq!(d.version, "operon-1");
        assert!(d.elements.is_empty());
    }

    #[test]
    fn round_trip_preserves_freedraw_and_rectangle() {
        let d = ExcalidrawDoc {
            version: "operon-1".into(),
            elements: vec![
                ExcalidrawElement::FreeDraw {
                    id: "a".into(),
                    points: vec![Point { x: 0.0, y: 0.0 }, Point { x: 5.0, y: 5.0 }],
                    stroke_color: "#fff".into(),
                    stroke_width: 3.0,
                },
                ExcalidrawElement::Rectangle {
                    id: "b".into(),
                    x: 10.0,
                    y: 20.0,
                    width: 100.0,
                    height: 50.0,
                    stroke_color: "#abc".into(),
                    stroke_width: 1.0,
                },
            ],
        };
        let json = d.to_json();
        let back = ExcalidrawDoc::parse(&json);
        assert_eq!(back, d);
    }
}
