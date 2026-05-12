//! `apply_patch` tool — apply a list of hunks to a file.
//!
//! Schema is *simpler* than full unified diff: each hunk is a `before` /
//! `after` pair where `before` must appear exactly once in the file. The
//! agent gets multi-line replacement in a single round-trip without us
//! having to implement fuzzy line-number alignment.
//!
//! For multi-file patches, call the tool once per file. For a free-form
//! unified diff, the agent should use `shell` to run `patch` itself.

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
struct Hunk {
    before: String,
    after: String,
}

#[derive(Deserialize)]
struct ApplyPatchInput {
    path: String,
    hunks: Vec<Hunk>,
}

pub struct ApplyPatchTool;

#[async_trait]
impl Plugin for ApplyPatchTool {
    fn name(&self) -> &str { "apply_patch" }
    fn version(&self) -> &str { env!("CARGO_PKG_VERSION") }
    fn capabilities(&self) -> Capabilities { Capabilities::empty() }
}

#[async_trait]
impl ToolPlugin for ApplyPatchTool {
    fn schema(&self) -> ToolDef {
        ToolDef {
            name: "apply_patch".into(),
            description: "Apply a list of replacement hunks to a single file. \
                          Each hunk's `before` text must appear exactly once. \
                          Use this for multi-line edits in one round-trip; \
                          use `edit` for a single small replacement."
                          .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path":  { "type": "string", "description": "Absolute path to the file." },
                    "hunks": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "before": { "type": "string" },
                                "after":  { "type": "string" }
                            },
                            "required": ["before", "after"]
                        }
                    }
                },
                "required": ["path", "hunks"]
            }),
        }
    }

    async fn invoke(
        &self,
        args: serde_json::Value,
        _ct: CancellationToken,
    ) -> OperonResult<serde_json::Value> {
        let input: ApplyPatchInput = serde_json::from_value(args).map_err(|e| OperonError::Plugin {
            plugin: "apply_patch".into(),
            source: Box::new(e),
        })?;

        let path = PathBuf::from(&input.path);
        if !path.is_absolute() {
            return Ok(json!({ "error": "path must be absolute", "path": input.path }));
        }
        if !path.exists() {
            return Ok(json!({ "error": "file not found", "path": input.path }));
        }
        if input.hunks.is_empty() {
            return Ok(json!({ "error": "hunks must be a non-empty array" }));
        }

        let mut content = String::new();
        fs::File::open(&path)
            .await
            .map_err(OperonError::Io)?
            .read_to_string(&mut content)
            .await
            .map_err(OperonError::Io)?;

        // Apply hunks in order. Each `before` must match exactly once in the
        // *current* state of the buffer. If any hunk fails, abort without
        // writing.
        let original = content.clone();
        for (i, h) in input.hunks.iter().enumerate() {
            if h.before.is_empty() {
                return Ok(json!({
                    "error": format!("hunk #{}: before must not be empty", i + 1),
                    "applied": 0,
                }));
            }
            let occurrences = content.matches(&h.before).count();
            if occurrences == 0 {
                return Ok(json!({
                    "error": format!("hunk #{}: before not found", i + 1),
                    "applied": 0,
                    "preview_before": head(&h.before, 200),
                }));
            }
            if occurrences > 1 {
                return Ok(json!({
                    "error": format!("hunk #{}: before matches {occurrences} times — disambiguate with more context", i + 1),
                    "applied": 0,
                }));
            }
            content = content.replacen(&h.before, &h.after, 1);
        }

        if content == original {
            return Ok(json!({
                "applied": input.hunks.len(),
                "changed": false,
                "path": input.path,
            }));
        }

        fs::write(&path, content.as_bytes())
            .await
            .map_err(OperonError::Io)?;
        Ok(json!({
            "applied": input.hunks.len(),
            "changed": true,
            "path": input.path,
        }))
    }
}

fn head(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
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
    async fn applies_single_hunk() {
        let (_tmp, p) = fixture("alpha\nBETA\ngamma\n").await;
        let r = ApplyPatchTool
            .invoke(
                json!({
                    "path": p.to_str().unwrap(),
                    "hunks": [{ "before": "BETA", "after": "delta" }]
                }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(r["applied"].as_u64(), Some(1));
        assert_eq!(r["changed"].as_bool(), Some(true));
        let on_disk = fs::read_to_string(&p).await.unwrap();
        assert_eq!(on_disk, "alpha\ndelta\ngamma\n");
    }

    #[tokio::test]
    async fn applies_multiple_hunks_in_order() {
        let (_tmp, p) = fixture("first\nsecond\nthird\n").await;
        let r = ApplyPatchTool
            .invoke(
                json!({
                    "path": p.to_str().unwrap(),
                    "hunks": [
                        { "before": "first", "after": "FIRST" },
                        { "before": "third", "after": "THIRD" }
                    ]
                }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(r["applied"].as_u64(), Some(2));
        let on_disk = fs::read_to_string(&p).await.unwrap();
        assert_eq!(on_disk, "FIRST\nsecond\nTHIRD\n");
    }

    #[tokio::test]
    async fn ambiguous_before_aborts_without_writing() {
        let (_tmp, p) = fixture("foo bar foo\n").await;
        let r = ApplyPatchTool
            .invoke(
                json!({
                    "path": p.to_str().unwrap(),
                    "hunks": [{ "before": "foo", "after": "X" }]
                }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(r.get("error").is_some());
        // File untouched.
        let on_disk = fs::read_to_string(&p).await.unwrap();
        assert_eq!(on_disk, "foo bar foo\n");
    }

    #[tokio::test]
    async fn missing_before_aborts() {
        let (_tmp, p) = fixture("hello\n").await;
        let r = ApplyPatchTool
            .invoke(
                json!({
                    "path": p.to_str().unwrap(),
                    "hunks": [{ "before": "absent", "after": "x" }]
                }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(r.get("error").is_some());
    }

    #[tokio::test]
    async fn second_hunk_failure_rolls_back_first_hunk() {
        // First hunk would succeed, second fails. We should NOT have written
        // the partially-modified content to disk.
        let (_tmp, p) = fixture("first\nsecond\n").await;
        let r = ApplyPatchTool
            .invoke(
                json!({
                    "path": p.to_str().unwrap(),
                    "hunks": [
                        { "before": "first", "after": "FIRST" },
                        { "before": "absent", "after": "x" }
                    ]
                }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(r.get("error").is_some());
        // First hunk's edit must NOT be on disk.
        let on_disk = fs::read_to_string(&p).await.unwrap();
        assert_eq!(on_disk, "first\nsecond\n");
    }

    #[tokio::test]
    async fn empty_hunks_array_rejected() {
        let (_tmp, p) = fixture("x").await;
        let r = ApplyPatchTool
            .invoke(
                json!({ "path": p.to_str().unwrap(), "hunks": [] }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(r.get("error").is_some());
    }

    #[tokio::test]
    async fn multi_line_replacement_works() {
        let (_tmp, p) = fixture("fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n").await;
        let r = ApplyPatchTool
            .invoke(
                json!({
                    "path": p.to_str().unwrap(),
                    "hunks": [{
                        "before": "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}",
                        "after": "fn add(a: i64, b: i64) -> i64 {\n    a.checked_add(b).expect(\"overflow\")\n}"
                    }]
                }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(r["applied"].as_u64(), Some(1));
        let on_disk = fs::read_to_string(&p).await.unwrap();
        assert!(on_disk.contains("checked_add"));
        assert!(!on_disk.contains("a + b"));
    }
}
