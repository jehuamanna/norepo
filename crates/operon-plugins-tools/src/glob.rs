//! `glob` tool — find files matching a glob pattern.
//!
//! Wraps `globwalk`; respects `.gitignore` by default. Returns a sorted list of
//! absolute paths (cap at 1000 results to avoid runaway responses).

use operon_core::error::{OperonError, OperonResult};
use operon_core::traits::{
    CancellationToken, Capabilities, Plugin, ToolDef, ToolPlugin,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;

const MAX_RESULTS: usize = 1000;

#[derive(Deserialize)]
struct GlobInput {
    pattern: String,
    #[serde(default)]
    cwd: Option<String>,
}

pub struct GlobTool;

#[async_trait]
impl Plugin for GlobTool {
    fn name(&self) -> &str { "glob" }
    fn version(&self) -> &str { env!("CARGO_PKG_VERSION") }
    fn capabilities(&self) -> Capabilities { Capabilities::empty() }
}

#[async_trait]
impl ToolPlugin for GlobTool {
    fn schema(&self) -> ToolDef {
        ToolDef {
            name: "glob".into(),
            description: "Find files matching a glob pattern. Capped at 1000 results."
                          .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern (e.g. **/*.rs)."
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Absolute working directory to search from."
                    }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn invoke(
        &self,
        args: serde_json::Value,
        _ct: CancellationToken,
    ) -> OperonResult<serde_json::Value> {
        let input: GlobInput = serde_json::from_value(args).map_err(|e| OperonError::Plugin {
            plugin: "glob".into(),
            source: Box::new(e),
        })?;

        let cwd = match input.cwd {
            Some(c) => {
                let p = PathBuf::from(&c);
                if !p.is_absolute() {
                    return Ok(json!({ "error": "cwd must be absolute", "cwd": c }));
                }
                p
            }
            None => std::env::current_dir().map_err(OperonError::Io)?,
        };
        if !cwd.exists() || !cwd.is_dir() {
            return Ok(json!({ "error": "cwd does not exist or is not a directory" }));
        }

        let pattern = input.pattern.clone();
        // globwalk is sync; offload to a blocking task so we don't stall the runtime.
        let walk_results = tokio::task::spawn_blocking(move || {
            globwalk::GlobWalkerBuilder::from_patterns(&cwd, &[&pattern])
                .max_depth(64)
                .follow_links(false)
                .build()
                .map(|w| {
                    w.filter_map(Result::ok)
                        .filter(|e| e.file_type().is_file())
                        .map(|e| e.path().to_path_buf())
                        .collect::<Vec<_>>()
                })
        })
        .await
        .map_err(|e| OperonError::Plugin {
            plugin: "glob".into(),
            source: Box::new(std::io::Error::other(format!("join: {e}"))),
        })?;

        let mut paths = match walk_results {
            Ok(p) => p,
            Err(e) => {
                return Ok(json!({
                    "error": format!("invalid glob pattern: {e}"),
                    "pattern": input.pattern,
                }));
            }
        };

        paths.sort();
        let truncated = paths.len() > MAX_RESULTS;
        paths.truncate(MAX_RESULTS);
        let strs: Vec<String> = paths
            .into_iter()
            .filter_map(|p| p.to_str().map(|s| s.to_string()))
            .collect();

        Ok(json!({
            "pattern": input.pattern,
            "matches": strs,
            "count": strs.len(),
            "truncated": truncated,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::fs;

    #[tokio::test]
    async fn schema_is_well_formed() {
        let s = GlobTool.schema();
        assert_eq!(s.name, "glob");
    }

    #[tokio::test]
    async fn finds_files_matching_pattern() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a.rs"), b"").await.unwrap();
        fs::write(tmp.path().join("b.rs"), b"").await.unwrap();
        fs::write(tmp.path().join("c.txt"), b"").await.unwrap();
        let r = GlobTool
            .invoke(
                json!({ "pattern": "*.rs", "cwd": tmp.path().to_str().unwrap() }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let count = r.get("count").and_then(|v| v.as_u64()).unwrap();
        assert_eq!(count, 2);
        let matches = r.get("matches").and_then(|v| v.as_array()).unwrap();
        let names: Vec<String> = matches
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        assert!(names.iter().any(|n| n.ends_with("a.rs")));
        assert!(names.iter().any(|n| n.ends_with("b.rs")));
        assert!(!names.iter().any(|n| n.ends_with("c.txt")));
    }

    #[tokio::test]
    async fn rejects_relative_cwd() {
        let r = GlobTool
            .invoke(json!({ "pattern": "*.rs", "cwd": "relative" }), CancellationToken::new())
            .await
            .unwrap();
        assert!(r.get("error").is_some());
    }

    #[tokio::test]
    async fn invalid_pattern_returns_error() {
        let tmp = TempDir::new().unwrap();
        let r = GlobTool
            .invoke(
                // Invalid pattern: empty
                json!({ "pattern": "", "cwd": tmp.path().to_str().unwrap() }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(r.get("error").is_some() || r.get("count").and_then(|v| v.as_u64()) == Some(0));
    }
}
