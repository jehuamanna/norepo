//! `read` tool — read a file from the local filesystem.
//!
//! Schema: `{ path: string, offset?: int >= 0, limit?: int >= 1 }`. Returns the
//! requested slice with line numbering, plus total line count metadata.
//!
//! Paths are resolved as absolute. Callers are expected to pass absolute paths
//! (the agent loop binds `cwd` per-session and will rewrite relative paths in
//! Slice A14). For now relative paths are rejected.

use operon_core::error::OperonResult;
use operon_core::traits::{
    CancellationToken, Capabilities, Plugin, ToolDef, ToolPlugin,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;
use tokio::fs;
use tokio::io::AsyncReadExt;

/// Default maximum number of lines returned when `limit` is not specified.
/// Mirrors opencode's `read.ts` default; keeps token use bounded for big files.
const DEFAULT_LIMIT: usize = 2000;

#[derive(Deserialize)]
struct ReadInput {
    path: String,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

pub struct ReadTool;

#[async_trait]
impl Plugin for ReadTool {
    fn name(&self) -> &str { "read" }
    fn version(&self) -> &str { env!("CARGO_PKG_VERSION") }
    fn capabilities(&self) -> Capabilities { Capabilities::empty() }
}

#[async_trait]
impl ToolPlugin for ReadTool {
    fn schema(&self) -> ToolDef {
        ToolDef {
            name: "read".into(),
            description: "Read a file from the local filesystem. \
                          Returns content with 1-indexed line numbers. \
                          Uses offset/limit to page through large files (default limit 2000 lines)."
                          .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path":   { "type": "string", "description": "Absolute path to the file." },
                    "offset": { "type": "integer", "minimum": 0, "description": "0-indexed line to start reading at." },
                    "limit":  { "type": "integer", "minimum": 1, "description": "Max number of lines to return (default 2000)." }
                },
                "required": ["path"]
            }),
        }
    }

    async fn invoke(
        &self,
        args: serde_json::Value,
        _ct: CancellationToken,
    ) -> OperonResult<serde_json::Value> {
        let input: ReadInput = serde_json::from_value(args)
            .map_err(|e| operon_core::error::OperonError::Plugin {
                plugin: "read".into(),
                source: Box::new(e),
            })?;

        let path = PathBuf::from(&input.path);
        if !path.is_absolute() {
            return Ok(json!({ "error": "path must be absolute", "path": input.path }));
        }
        if !path.exists() {
            return Ok(json!({ "error": "file not found", "path": input.path }));
        }
        if path.is_dir() {
            return Ok(json!({ "error": "path is a directory", "path": input.path }));
        }

        let metadata = fs::metadata(&path).await.map_err(|e| operon_core::error::OperonError::Io(e))?;
        // Refuse pathologically large files outright (>= 100 MiB) to avoid OOM.
        const HARD_LIMIT_BYTES: u64 = 100 * 1024 * 1024;
        if metadata.len() >= HARD_LIMIT_BYTES {
            return Ok(json!({
                "error": "file too large to read in full (>= 100 MiB); pass offset+limit to page",
                "path": input.path,
                "size_bytes": metadata.len(),
            }));
        }

        let mut content = String::new();
        let mut file = fs::File::open(&path).await.map_err(|e| operon_core::error::OperonError::Io(e))?;
        file.read_to_string(&mut content)
            .await
            .map_err(|e| operon_core::error::OperonError::Io(e))?;

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();
        let offset = input.offset.unwrap_or(0);
        let limit = input.limit.unwrap_or(DEFAULT_LIMIT);

        if offset >= total_lines && total_lines > 0 {
            return Ok(json!({
                "error": "offset past end of file",
                "path": input.path,
                "total_lines": total_lines,
                "offset": offset,
            }));
        }

        let end = (offset + limit).min(total_lines);
        let mut numbered = String::new();
        for (i, line) in lines.iter().enumerate().skip(offset).take(end - offset) {
            numbered.push_str(&format!("{:>6}\t{}\n", i + 1, line));
        }

        Ok(json!({
            "path": input.path,
            "content": numbered,
            "total_lines": total_lines,
            "offset": offset,
            "lines_returned": end - offset,
            "truncated": end < total_lines,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::io::AsyncWriteExt;

    async fn write_fixture(dir: &TempDir, name: &str, content: &str) -> PathBuf {
        let p = dir.path().join(name);
        let mut f = fs::File::create(&p).await.unwrap();
        f.write_all(content.as_bytes()).await.unwrap();
        f.flush().await.unwrap();
        p
    }

    #[tokio::test]
    async fn schema_is_well_formed() {
        let s = ReadTool.schema();
        assert_eq!(s.name, "read");
        assert!(s.input_schema.get("properties").is_some());
        assert!(s.input_schema.get("required").is_some());
    }

    #[tokio::test]
    async fn rejects_relative_path() {
        let r = ReadTool.invoke(json!({ "path": "relative/file.txt" }), CancellationToken::new()).await.unwrap();
        assert_eq!(r.get("error").and_then(|v| v.as_str()), Some("path must be absolute"));
    }

    #[tokio::test]
    async fn rejects_missing_path() {
        let r = ReadTool.invoke(json!({ "path": "/no/such/path/here" }), CancellationToken::new()).await.unwrap();
        assert_eq!(r.get("error").and_then(|v| v.as_str()), Some("file not found"));
    }

    #[tokio::test]
    async fn reads_full_file_with_line_numbers() {
        let tmp = TempDir::new().unwrap();
        let p = write_fixture(&tmp, "hello.txt", "alpha\nbeta\ngamma\n").await;
        let r = ReadTool.invoke(json!({ "path": p.to_str().unwrap() }), CancellationToken::new()).await.unwrap();
        let content = r.get("content").and_then(|v| v.as_str()).unwrap();
        assert!(content.contains("     1\talpha"));
        assert!(content.contains("     2\tbeta"));
        assert!(content.contains("     3\tgamma"));
        assert_eq!(r.get("total_lines").and_then(|v| v.as_u64()), Some(3));
        assert_eq!(r.get("truncated").and_then(|v| v.as_bool()), Some(false));
    }

    #[tokio::test]
    async fn offset_and_limit_paginate() {
        let tmp = TempDir::new().unwrap();
        let body: String = (1..=20).map(|i| format!("line-{i}\n")).collect();
        let p = write_fixture(&tmp, "long.txt", &body).await;
        let r = ReadTool
            .invoke(
                json!({ "path": p.to_str().unwrap(), "offset": 5, "limit": 3 }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let content = r.get("content").and_then(|v| v.as_str()).unwrap();
        assert!(content.contains("     6\tline-6"));
        assert!(content.contains("     7\tline-7"));
        assert!(content.contains("     8\tline-8"));
        assert!(!content.contains("line-9"));
        assert_eq!(r.get("lines_returned").and_then(|v| v.as_u64()), Some(3));
        assert_eq!(r.get("truncated").and_then(|v| v.as_bool()), Some(true));
    }

    #[tokio::test]
    async fn directory_path_returns_error() {
        let tmp = TempDir::new().unwrap();
        let r = ReadTool.invoke(json!({ "path": tmp.path().to_str().unwrap() }), CancellationToken::new()).await.unwrap();
        assert_eq!(r.get("error").and_then(|v| v.as_str()), Some("path is a directory"));
    }
}
