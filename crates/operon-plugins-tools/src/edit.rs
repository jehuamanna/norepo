//! `edit` tool — exact-string replace with strict uniqueness check.
//!
//! Ported in spirit from opencode's `edit.ts`. The contract:
//!  - `old_string` MUST appear exactly once unless `replace_all` is true.
//!  - If `old_string` is missing → error.
//!  - `new_string` may be empty (i.e., delete the match).

use operon_core::error::{OperonError, OperonResult};
use operon_core::traits::{
    CancellationToken, Capabilities, Plugin, ToolDef, ToolPlugin,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;
use tokio::fs;
use tokio::io::AsyncReadExt;

#[derive(Deserialize)]
struct EditInput {
    path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

pub struct EditTool;

#[async_trait]
impl Plugin for EditTool {
    fn name(&self) -> &str { "edit" }
    fn version(&self) -> &str { env!("CARGO_PKG_VERSION") }
    fn capabilities(&self) -> Capabilities { Capabilities::empty() }
}

#[async_trait]
impl ToolPlugin for EditTool {
    fn schema(&self) -> ToolDef {
        ToolDef {
            name: "edit".into(),
            description: "Replace an exact string in a file. \
                          old_string must appear exactly once unless replace_all is true."
                          .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path":        { "type": "string", "description": "Absolute path to the file." },
                    "old_string":  { "type": "string", "description": "The exact text to replace." },
                    "new_string":  { "type": "string", "description": "Replacement text (may be empty)." },
                    "replace_all": { "type": "boolean", "default": false,
                                     "description": "Replace every occurrence." }
                },
                "required": ["path", "old_string", "new_string"]
            }),
        }
    }

    async fn invoke(
        &self,
        args: serde_json::Value,
        _ct: CancellationToken,
    ) -> OperonResult<serde_json::Value> {
        let input: EditInput = serde_json::from_value(args).map_err(|e| OperonError::Plugin {
            plugin: "edit".into(),
            source: Box::new(e),
        })?;

        let path = PathBuf::from(&input.path);
        if !path.is_absolute() {
            return Ok(json!({ "error": "path must be absolute", "path": input.path }));
        }
        if !path.exists() {
            return Ok(json!({ "error": "file not found", "path": input.path }));
        }
        if input.old_string.is_empty() {
            return Ok(json!({ "error": "old_string must not be empty" }));
        }

        let mut content = String::new();
        fs::File::open(&path)
            .await
            .map_err(OperonError::Io)?
            .read_to_string(&mut content)
            .await
            .map_err(OperonError::Io)?;

        // Count occurrences before replacing so we can error on ambiguous edits.
        let occurrences = content.matches(&input.old_string).count();
        if occurrences == 0 {
            return Ok(json!({
                "error": "old_string not found in file",
                "path": input.path,
            }));
        }
        if occurrences > 1 && !input.replace_all {
            return Ok(json!({
                "error": format!("old_string found {occurrences} times; pass replace_all=true or supply more context"),
                "occurrences": occurrences,
                "path": input.path,
            }));
        }

        let new_content = if input.replace_all {
            content.replace(&input.old_string, &input.new_string)
        } else {
            content.replacen(&input.old_string, &input.new_string, 1)
        };
        let replaced_count = if input.replace_all { occurrences } else { 1 };
        fs::write(&path, new_content.as_bytes())
            .await
            .map_err(OperonError::Io)?;

        Ok(json!({
            "path": input.path,
            "replacements": replaced_count,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn fixture(content: &str) -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("file.txt");
        fs::write(&p, content.as_bytes()).await.unwrap();
        (tmp, p)
    }

    #[tokio::test]
    async fn replaces_unique_occurrence() {
        let (_tmp, p) = fixture("alpha\nBETA\ngamma\n").await;
        let r = EditTool
            .invoke(
                json!({ "path": p.to_str().unwrap(), "old_string": "BETA", "new_string": "delta" }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(r.get("replacements").and_then(|v| v.as_u64()), Some(1));
        let on_disk = fs::read_to_string(&p).await.unwrap();
        assert_eq!(on_disk, "alpha\ndelta\ngamma\n");
    }

    #[tokio::test]
    async fn ambiguous_replacement_errors_without_replace_all() {
        let (_tmp, p) = fixture("foo bar foo bar foo\n").await;
        let r = EditTool
            .invoke(
                json!({ "path": p.to_str().unwrap(), "old_string": "foo", "new_string": "X" }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(r.get("error").is_some());
        assert_eq!(r.get("occurrences").and_then(|v| v.as_u64()), Some(3));
        // File unchanged.
        let on_disk = fs::read_to_string(&p).await.unwrap();
        assert_eq!(on_disk, "foo bar foo bar foo\n");
    }

    #[tokio::test]
    async fn replace_all_replaces_everything() {
        let (_tmp, p) = fixture("foo bar foo bar foo\n").await;
        let r = EditTool
            .invoke(
                json!({
                    "path": p.to_str().unwrap(),
                    "old_string": "foo",
                    "new_string": "X",
                    "replace_all": true
                }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(r.get("replacements").and_then(|v| v.as_u64()), Some(3));
        let on_disk = fs::read_to_string(&p).await.unwrap();
        assert_eq!(on_disk, "X bar X bar X\n");
    }

    #[tokio::test]
    async fn missing_old_string_errors() {
        let (_tmp, p) = fixture("hello\n").await;
        let r = EditTool
            .invoke(
                json!({ "path": p.to_str().unwrap(), "old_string": "absent", "new_string": "x" }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(r.get("error").is_some());
    }

    #[tokio::test]
    async fn empty_old_string_rejected() {
        let (_tmp, p) = fixture("hello\n").await;
        let r = EditTool
            .invoke(
                json!({ "path": p.to_str().unwrap(), "old_string": "", "new_string": "x" }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(r.get("error").is_some());
    }

    #[tokio::test]
    async fn empty_new_string_deletes_match() {
        let (_tmp, p) = fixture("aXa").await;
        let r = EditTool
            .invoke(
                json!({ "path": p.to_str().unwrap(), "old_string": "X", "new_string": "" }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(r.get("replacements").and_then(|v| v.as_u64()), Some(1));
        let on_disk = fs::read_to_string(&p).await.unwrap();
        assert_eq!(on_disk, "aa");
    }
}
