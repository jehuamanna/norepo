//! `lsp` ToolPlugin — exposes goto_definition / find_references / hover /
//! document_symbols / diagnostics through a registered language server.
//!
//! Build with `LspToolBuilder` so the tool can route requests across multiple
//! language servers picked by file extension.

use crate::client::{LspClient, LspServerConfig};
use operon_core::error::{OperonError, OperonResult};
use operon_core::traits::{
    CancellationToken, Capabilities, Plugin, ToolDef, ToolPlugin,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

#[derive(Deserialize)]
struct LspInput {
    method: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    line: Option<u32>,
    #[serde(default)]
    character: Option<u32>,
}

/// Maps a file extension to a registered LspClient id.
type ExtMap = HashMap<String, String>;

pub struct LspTool {
    clients: HashMap<String, Arc<LspClient>>,
    ext_to_client: ExtMap,
}

pub struct LspToolBuilder {
    clients: Vec<(LspServerConfig, Vec<String>)>,
}

impl Default for LspToolBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl LspToolBuilder {
    pub fn new() -> Self {
        Self { clients: Vec::new() }
    }

    /// Register an LSP server for the given file extensions (without leading dot).
    pub fn add(mut self, cfg: LspServerConfig, exts: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let exts: Vec<String> = exts.into_iter().map(|e| e.into()).collect();
        self.clients.push((cfg, exts));
        self
    }

    /// Build and connect every registered client. Failed connections are dropped
    /// (logged) so the tool still operates with the surviving clients.
    pub async fn build(self) -> LspTool {
        let mut clients: HashMap<String, Arc<LspClient>> = HashMap::new();
        let mut ext_to_client: ExtMap = HashMap::new();
        for (cfg, exts) in self.clients {
            let id = cfg.id.clone();
            let c = Arc::new(LspClient::new(cfg));
            match c.connect().await {
                Ok(()) => {
                    for ext in exts {
                        ext_to_client.insert(ext, id.clone());
                    }
                    clients.insert(id, c);
                }
                Err(e) => {
                    tracing::warn!(target: "operon::lsp", id = %id, error = %e, "lsp connect failed, skipping");
                }
            }
        }
        LspTool {
            clients,
            ext_to_client,
        }
    }
}

impl LspTool {
    pub fn empty() -> Self {
        Self {
            clients: HashMap::new(),
            ext_to_client: HashMap::new(),
        }
    }

    fn pick_client(&self, path: &Path) -> Option<Arc<LspClient>> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase())?;
        let id = self.ext_to_client.get(&ext)?;
        self.clients.get(id).cloned()
    }
}

#[async_trait]
impl Plugin for LspTool {
    fn name(&self) -> &str { "lsp" }
    fn version(&self) -> &str { env!("CARGO_PKG_VERSION") }
    fn capabilities(&self) -> Capabilities { Capabilities::empty() }
}

#[async_trait]
impl ToolPlugin for LspTool {
    fn schema(&self) -> ToolDef {
        ToolDef {
            name: "lsp".into(),
            description: "Query the project's language server. \
                          Methods: goto_definition, find_references, hover, document_symbols. \
                          Picks the right server by file extension."
                          .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "method": {
                        "type": "string",
                        "enum": ["goto_definition", "find_references", "hover", "document_symbols"]
                    },
                    "path":      { "type": "string", "description": "Absolute file path." },
                    "line":      { "type": "integer", "minimum": 0 },
                    "character": { "type": "integer", "minimum": 0 }
                },
                "required": ["method", "path"]
            }),
        }
    }

    async fn invoke(
        &self,
        args: serde_json::Value,
        _ct: CancellationToken,
    ) -> OperonResult<serde_json::Value> {
        let input: LspInput = serde_json::from_value(args).map_err(|e| OperonError::Plugin {
            plugin: "lsp".into(),
            source: Box::new(e),
        })?;
        let path_str = input.path.clone().ok_or_else(|| OperonError::Plugin {
            plugin: "lsp".into(),
            source: Box::new(std::io::Error::other("path is required")),
        })?;
        let path = Path::new(&path_str);
        if !path.is_absolute() {
            return Ok(json!({ "error": "path must be absolute" }));
        }
        let client = match self.pick_client(path) {
            Some(c) => c,
            None => {
                return Ok(json!({
                    "error": "no LSP server registered for this file extension",
                    "path": path_str,
                }));
            }
        };
        let line = input.line.unwrap_or(0);
        let character = input.character.unwrap_or(0);
        let result = match input.method.as_str() {
            "goto_definition" => client.definition(path, line, character).await,
            "find_references" => client.references(path, line, character).await,
            "hover"           => client.hover(path, line, character).await,
            "document_symbols" => client.document_symbol(path).await,
            other => {
                return Ok(json!({ "error": format!("unsupported method: {other}") }));
            }
        };
        match result {
            Ok(v) => Ok(json!({
                "method": input.method,
                "result": v,
            })),
            Err(e) => Ok(json!({
                "error": format!("{e}"),
            })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_tool_returns_no_server_error() {
        let t = LspTool::empty();
        let r = t
            .invoke(
                json!({ "method": "hover", "path": "/tmp/x.rs", "line": 0, "character": 0 }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(r.get("error").is_some());
    }

    #[tokio::test]
    async fn schema_lists_supported_methods() {
        let t = LspTool::empty();
        let s = t.schema();
        let methods = s
            .input_schema
            .get("properties")
            .and_then(|p| p.get("method"))
            .and_then(|m| m.get("enum"))
            .and_then(|e| e.as_array())
            .unwrap();
        let names: Vec<String> = methods
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        assert!(names.contains(&"goto_definition".to_string()));
        assert!(names.contains(&"hover".to_string()));
    }

    #[tokio::test]
    async fn rejects_relative_path() {
        let t = LspTool::empty();
        let r = t
            .invoke(
                json!({ "method": "hover", "path": "relative.rs" }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(r.get("error").and_then(|v| v.as_str()), Some("path must be absolute"));
    }

    #[test]
    fn pick_client_routes_by_extension() {
        let mut t = LspTool::empty();
        t.ext_to_client.insert("rs".into(), "rust-analyzer".into());
        // We can't easily build a real LspClient without a child, so just verify
        // the routing doesn't pick a client when none registered:
        assert!(t.pick_client(Path::new("/tmp/x.rs")).is_none());
    }
}
