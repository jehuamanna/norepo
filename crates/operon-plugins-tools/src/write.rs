//! `write` tool — write a file to the local filesystem.
//!
//! Creates parent directories as needed. Overwrites existing files. The agent's
//! permission gate (Slice A3) decides whether the write is allowed.

use operon_core::error::{OperonError, OperonResult};
use operon_core::traits::{
    CancellationToken, Capabilities, Plugin, ToolDef, ToolPlugin,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;
use tokio::fs;
use tokio::io::AsyncWriteExt;

#[derive(Deserialize)]
struct WriteInput {
    path: String,
    content: String,
}

pub struct WriteTool;

#[async_trait]
impl Plugin for WriteTool {
    fn name(&self) -> &str { "write" }
    fn version(&self) -> &str { env!("CARGO_PKG_VERSION") }
    fn capabilities(&self) -> Capabilities { Capabilities::empty() }
}

#[async_trait]
impl ToolPlugin for WriteTool {
    fn schema(&self) -> ToolDef {
        ToolDef {
            name: "write".into(),
            description: "Write a file to the local filesystem. \
                          Overwrites if it exists. Creates parent directories as needed."
                          .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path":    { "type": "string", "description": "Absolute path to the file." },
                    "content": { "type": "string", "description": "File content (UTF-8)." }
                },
                "required": ["path", "content"]
            }),
        }
    }

    async fn invoke(
        &self,
        args: serde_json::Value,
        _ct: CancellationToken,
    ) -> OperonResult<serde_json::Value> {
        let input: WriteInput = serde_json::from_value(args).map_err(|e| OperonError::Plugin {
            plugin: "write".into(),
            source: Box::new(e),
        })?;

        let path = PathBuf::from(&input.path);
        if !path.is_absolute() {
            return Ok(json!({ "error": "path must be absolute", "path": input.path }));
        }
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).await.map_err(OperonError::Io)?;
            }
        }
        let mut f = fs::File::create(&path).await.map_err(OperonError::Io)?;
        f.write_all(input.content.as_bytes()).await.map_err(OperonError::Io)?;
        f.flush().await.map_err(OperonError::Io)?;
        let bytes = input.content.len();

        Ok(json!({
            "path": input.path,
            "bytes_written": bytes,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn schema_is_well_formed() {
        let s = WriteTool.schema();
        assert_eq!(s.name, "write");
    }

    #[tokio::test]
    async fn rejects_relative_path() {
        let r = WriteTool
            .invoke(json!({ "path": "x.txt", "content": "" }), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(r.get("error").and_then(|v| v.as_str()), Some("path must be absolute"));
    }

    #[tokio::test]
    async fn writes_file_and_creates_parents() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("nested/deep/file.txt");
        let r = WriteTool
            .invoke(
                json!({ "path": p.to_str().unwrap(), "content": "hello" }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(r.get("bytes_written").and_then(|v| v.as_u64()), Some(5));
        let on_disk = fs::read_to_string(&p).await.unwrap();
        assert_eq!(on_disk, "hello");
    }

    #[tokio::test]
    async fn write_overwrites_existing_file() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("file.txt");
        fs::write(&p, b"old").await.unwrap();
        let _ = WriteTool
            .invoke(
                json!({ "path": p.to_str().unwrap(), "content": "new" }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let on_disk = fs::read_to_string(&p).await.unwrap();
        assert_eq!(on_disk, "new");
    }
}
