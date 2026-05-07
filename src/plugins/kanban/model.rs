//! Data model for the Kanban plugin. The on-disk shape is JSON; an empty
//! body parses as an empty board so first-open of a fresh Kanban note
//! shows a "+ Add column" affordance instead of an error.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KanbanCard {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KanbanColumn {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub cards: Vec<KanbanCard>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct KanbanBoard {
    #[serde(default)]
    pub columns: Vec<KanbanColumn>,
}

impl KanbanBoard {
    /// Empty content → empty board. Malformed JSON also collapses to empty
    /// (logged) so a corrupted note still renders an editor instead of a
    /// blank page; users can fix the body via the Markdown viewer or undo.
    pub fn parse(raw: &str) -> Self {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Self::default();
        }
        match serde_json::from_str::<KanbanBoard>(trimmed) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("operon: kanban parse failed: {e}");
                Self::default()
            }
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{\"columns\":[]}".into())
    }

    pub fn fresh_id() -> String {
        uuid::Uuid::new_v4().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_parses_to_empty_board() {
        assert_eq!(KanbanBoard::parse("").columns.len(), 0);
        assert_eq!(KanbanBoard::parse("   ").columns.len(), 0);
    }

    #[test]
    fn malformed_json_collapses_to_empty() {
        assert_eq!(KanbanBoard::parse("not-json").columns.len(), 0);
    }

    #[test]
    fn round_trip_preserves_columns_and_cards() {
        let b = KanbanBoard {
            columns: vec![
                KanbanColumn {
                    id: "c1".into(),
                    title: "Todo".into(),
                    cards: vec![KanbanCard {
                        id: "a".into(),
                        text: "Buy milk".into(),
                    }],
                },
                KanbanColumn {
                    id: "c2".into(),
                    title: "Done".into(),
                    cards: vec![],
                },
            ],
        };
        let json = b.to_json();
        let back = KanbanBoard::parse(&json);
        assert_eq!(back, b);
    }
}
