//! `todo` tool — per-instance todo list management.
//!
//! Lets the agent track multi-step work explicitly. Backed by an in-memory
//! `Mutex<Vec<TodoItem>>`. Constructing one `TodoTool` per session keeps lists
//! isolated; the runtime is expected to do that.
//!
//! Subcommands: `add` / `complete` / `list` / `clear`.

use operon_core::error::{OperonError, OperonResult};
use operon_core::traits::{
    CancellationToken, Capabilities, Plugin, ToolDef, ToolPlugin,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Mutex;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: u64,
    pub content: String,
    pub status: TodoStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Deserialize)]
struct TodoInput {
    subcommand: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    status: Option<TodoStatus>,
}

pub struct TodoTool {
    items: Mutex<Vec<TodoItem>>,
    next_id: Mutex<u64>,
}

impl Default for TodoTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TodoTool {
    pub fn new() -> Self {
        Self {
            items: Mutex::new(Vec::new()),
            next_id: Mutex::new(1),
        }
    }

    fn alloc_id(&self) -> u64 {
        let mut g = self.next_id.lock().expect("todo id lock poisoned");
        let id = *g;
        *g += 1;
        id
    }
}

#[async_trait]
impl Plugin for TodoTool {
    fn name(&self) -> &str { "todo" }
    fn version(&self) -> &str { env!("CARGO_PKG_VERSION") }
    fn capabilities(&self) -> Capabilities { Capabilities::empty() }
}

#[async_trait]
impl ToolPlugin for TodoTool {
    fn schema(&self) -> ToolDef {
        ToolDef {
            name: "todo".into(),
            description: "Track multi-step work. Subcommands: \
                          add (content) → returns id; \
                          complete (id) → marks done; \
                          set (id, status) → set status to pending|in_progress|completed; \
                          list → returns all items; \
                          clear → drop everything."
                          .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "subcommand": {
                        "type": "string",
                        "enum": ["add", "complete", "set", "list", "clear"]
                    },
                    "content": { "type": "string", "description": "For `add`." },
                    "id":      { "type": "integer", "minimum": 1, "description": "For `complete` / `set`." },
                    "status":  {
                        "type": "string",
                        "enum": ["pending", "in_progress", "completed"],
                        "description": "For `set`."
                    }
                },
                "required": ["subcommand"]
            }),
        }
    }

    async fn invoke(
        &self,
        args: serde_json::Value,
        _ct: CancellationToken,
    ) -> OperonResult<serde_json::Value> {
        let input: TodoInput = serde_json::from_value(args).map_err(|e| OperonError::Plugin {
            plugin: "todo".into(),
            source: Box::new(e),
        })?;
        match input.subcommand.as_str() {
            "add" => {
                let content = match input.content {
                    Some(c) if !c.trim().is_empty() => c,
                    _ => return Ok(json!({ "error": "content is required and non-empty" })),
                };
                let id = self.alloc_id();
                let item = TodoItem {
                    id,
                    content,
                    status: TodoStatus::Pending,
                };
                self.items
                    .lock()
                    .expect("todo lock poisoned")
                    .push(item.clone());
                Ok(json!({ "added": { "id": item.id, "content": item.content } }))
            }
            "complete" => {
                let id = match input.id {
                    Some(i) => i,
                    None => return Ok(json!({ "error": "id is required" })),
                };
                let mut items = self.items.lock().expect("todo lock poisoned");
                let found = items.iter_mut().find(|it| it.id == id);
                match found {
                    Some(it) => {
                        it.status = TodoStatus::Completed;
                        Ok(json!({ "completed": id }))
                    }
                    None => Ok(json!({ "error": format!("no todo with id {id}") })),
                }
            }
            "set" => {
                let id = match input.id {
                    Some(i) => i,
                    None => return Ok(json!({ "error": "id is required" })),
                };
                let status = match input.status {
                    Some(s) => s,
                    None => return Ok(json!({ "error": "status is required" })),
                };
                let mut items = self.items.lock().expect("todo lock poisoned");
                let found = items.iter_mut().find(|it| it.id == id);
                match found {
                    Some(it) => {
                        it.status = status;
                        Ok(json!({ "id": id, "status": status }))
                    }
                    None => Ok(json!({ "error": format!("no todo with id {id}") })),
                }
            }
            "list" => {
                let items = self.items.lock().expect("todo lock poisoned").clone();
                let arr: Vec<serde_json::Value> = items
                    .iter()
                    .map(|it| {
                        json!({
                            "id": it.id,
                            "content": it.content,
                            "status": it.status,
                        })
                    })
                    .collect();
                Ok(json!({ "items": arr, "count": items.len() }))
            }
            "clear" => {
                let n = self.items.lock().expect("todo lock poisoned").len();
                self.items.lock().expect("todo lock poisoned").clear();
                Ok(json!({ "cleared": n }))
            }
            other => Ok(json!({ "error": format!("unknown subcommand: {other}") })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn add_returns_assigned_id() {
        let t = TodoTool::new();
        let r = t
            .invoke(
                json!({ "subcommand": "add", "content": "write tests" }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(r["added"]["id"].as_u64(), Some(1));
        assert_eq!(r["added"]["content"].as_str(), Some("write tests"));
    }

    #[tokio::test]
    async fn add_then_list_returns_one_pending_item() {
        let t = TodoTool::new();
        let _ = t
            .invoke(json!({ "subcommand": "add", "content": "a" }), CancellationToken::new())
            .await
            .unwrap();
        let r = t
            .invoke(json!({ "subcommand": "list" }), CancellationToken::new())
            .await
            .unwrap();
        let items = r["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["status"].as_str(), Some("pending"));
    }

    #[tokio::test]
    async fn complete_marks_status_completed() {
        let t = TodoTool::new();
        let added = t
            .invoke(json!({ "subcommand": "add", "content": "a" }), CancellationToken::new())
            .await
            .unwrap();
        let id = added["added"]["id"].as_u64().unwrap();
        let _ = t
            .invoke(
                json!({ "subcommand": "complete", "id": id }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let listed = t
            .invoke(json!({ "subcommand": "list" }), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(listed["items"][0]["status"].as_str(), Some("completed"));
    }

    #[tokio::test]
    async fn set_in_progress_status() {
        let t = TodoTool::new();
        let added = t
            .invoke(json!({ "subcommand": "add", "content": "a" }), CancellationToken::new())
            .await
            .unwrap();
        let id = added["added"]["id"].as_u64().unwrap();
        let _ = t
            .invoke(
                json!({ "subcommand": "set", "id": id, "status": "in_progress" }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let listed = t
            .invoke(json!({ "subcommand": "list" }), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(listed["items"][0]["status"].as_str(), Some("in_progress"));
    }

    #[tokio::test]
    async fn complete_unknown_id_errors() {
        let t = TodoTool::new();
        let r = t
            .invoke(
                json!({ "subcommand": "complete", "id": 999 }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(r.get("error").is_some());
    }

    #[tokio::test]
    async fn add_empty_content_rejected() {
        let t = TodoTool::new();
        let r = t
            .invoke(json!({ "subcommand": "add", "content": "  " }), CancellationToken::new())
            .await
            .unwrap();
        assert!(r.get("error").is_some());
    }

    #[tokio::test]
    async fn clear_drops_everything_and_reports_count() {
        let t = TodoTool::new();
        for s in ["a", "b", "c"] {
            let _ = t
                .invoke(json!({ "subcommand": "add", "content": s }), CancellationToken::new())
                .await
                .unwrap();
        }
        let cleared = t
            .invoke(json!({ "subcommand": "clear" }), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(cleared["cleared"].as_u64(), Some(3));
        let listed = t
            .invoke(json!({ "subcommand": "list" }), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(listed["count"].as_u64(), Some(0));
    }

    #[tokio::test]
    async fn ids_are_unique_and_monotonic_after_clear() {
        let t = TodoTool::new();
        let r1 = t
            .invoke(json!({ "subcommand": "add", "content": "a" }), CancellationToken::new())
            .await
            .unwrap();
        let _ = t
            .invoke(json!({ "subcommand": "clear" }), CancellationToken::new())
            .await
            .unwrap();
        let r2 = t
            .invoke(json!({ "subcommand": "add", "content": "b" }), CancellationToken::new())
            .await
            .unwrap();
        // Even after clear, ids keep counting up so consumers don't see id collisions.
        assert!(r2["added"]["id"].as_u64().unwrap() > r1["added"]["id"].as_u64().unwrap());
    }
}
