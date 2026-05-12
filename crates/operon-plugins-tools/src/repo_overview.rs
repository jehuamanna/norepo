//! `repo_overview` tool — fast, no-LLM summary of a repository.
//!
//! Walks the cwd respecting `.gitignore`, then reports:
//! - Total file count & total bytes.
//! - Top N file extensions by file count (with bytes).
//! - Top-level directory layout (one level deep).
//! - The README.md head (first ~80 lines) if present.
//!
//! Useful as the agent's first action on a new project: it's deterministic,
//! cheap, and gives enough surface to ground subsequent reads.

use operon_core::error::{OperonError, OperonResult};
use operon_core::traits::{
    CancellationToken, Capabilities, Plugin, ToolDef, ToolPlugin,
};
use async_trait::async_trait;
use ignore::WalkBuilder;
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;

const TOP_EXT_LIMIT: usize = 12;
const README_HEAD_LINES: usize = 80;

#[derive(Deserialize)]
struct Input {
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    max_files: Option<usize>,
}

pub struct RepoOverviewTool;

#[async_trait]
impl Plugin for RepoOverviewTool {
    fn name(&self) -> &str { "repo_overview" }
    fn version(&self) -> &str { env!("CARGO_PKG_VERSION") }
    fn capabilities(&self) -> Capabilities { Capabilities::empty() }
}

#[async_trait]
impl ToolPlugin for RepoOverviewTool {
    fn schema(&self) -> ToolDef {
        ToolDef {
            name: "repo_overview".into(),
            description: "Summarise a repo: file count, top extensions, \
                          top-level directory layout, README head. \
                          Respects .gitignore. No LLM call."
                          .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "cwd":       { "type": "string", "description": "Absolute path to the repo root. Defaults to current working dir." },
                    "max_files": { "type": "integer", "minimum": 1, "default": 50000,
                                   "description": "Cap on files walked. Stops once reached." }
                }
            }),
        }
    }

    async fn invoke(
        &self,
        args: serde_json::Value,
        _ct: CancellationToken,
    ) -> OperonResult<serde_json::Value> {
        let input: Input = serde_json::from_value(args).unwrap_or(Input {
            cwd: None,
            max_files: None,
        });
        let root = match input.cwd {
            Some(c) => {
                let p = PathBuf::from(&c);
                if !p.is_absolute() {
                    return Ok(json!({ "error": "cwd must be absolute", "cwd": c }));
                }
                p
            }
            None => std::env::current_dir().map_err(OperonError::Io)?,
        };
        if !root.exists() || !root.is_dir() {
            return Ok(json!({ "error": "cwd does not exist or is not a directory" }));
        }
        let max_files = input.max_files.unwrap_or(50_000);

        let root_for_walk = root.clone();
        let summary = tokio::task::spawn_blocking(move || compute(&root_for_walk, max_files))
            .await
            .map_err(|e| OperonError::Plugin {
                plugin: "repo_overview".into(),
                source: Box::new(std::io::Error::other(format!("join: {e}"))),
            })?;

        let readme_head = read_readme_head(&root);

        Ok(json!({
            "root": root.display().to_string(),
            "files": summary.files,
            "bytes": summary.bytes,
            "stopped_at_cap": summary.stopped_at_cap,
            "top_extensions": summary.top_extensions,
            "top_level_dirs": summary.top_level_dirs,
            "readme_head": readme_head,
        }))
    }
}

#[derive(Debug)]
struct OverviewSummary {
    files: u64,
    bytes: u64,
    stopped_at_cap: bool,
    top_extensions: Vec<serde_json::Value>,
    top_level_dirs: Vec<serde_json::Value>,
}

fn compute(root: &PathBuf, max_files: usize) -> OverviewSummary {
    let mut files: u64 = 0;
    let mut bytes: u64 = 0;
    let mut by_ext: BTreeMap<String, (u64, u64)> = BTreeMap::new(); // ext → (count, bytes)
    let mut top_dirs: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut stopped_at_cap = false;

    for entry in WalkBuilder::new(root)
        .git_ignore(true)
        .follow_links(false)
        .build()
        .filter_map(Result::ok)
    {
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            if files as usize >= max_files {
                stopped_at_cap = true;
                break;
            }
            let p = entry.path();
            let size = p.metadata().map(|m| m.len()).unwrap_or(0);
            files += 1;
            bytes += size;
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| s.to_lowercase())
                .unwrap_or_else(|| "<no-ext>".to_string());
            let e = by_ext.entry(ext).or_default();
            e.0 += 1;
            e.1 += size;
            // Top-level dir = first segment after `root`.
            if let Ok(rel) = p.strip_prefix(root) {
                if let Some(first) = rel.iter().next() {
                    let key = first.to_string_lossy().to_string();
                    let d = top_dirs.entry(key).or_default();
                    d.0 += 1;
                    d.1 += size;
                }
            }
        }
    }

    let mut ext_v: Vec<(String, u64, u64)> = by_ext.into_iter().map(|(k, (c, b))| (k, c, b)).collect();
    ext_v.sort_by(|a, b| b.1.cmp(&a.1));
    let top_extensions = ext_v
        .into_iter()
        .take(TOP_EXT_LIMIT)
        .map(|(ext, count, b)| json!({ "extension": ext, "files": count, "bytes": b }))
        .collect();

    let mut dir_v: Vec<(String, u64, u64)> = top_dirs.into_iter().map(|(k, (c, b))| (k, c, b)).collect();
    dir_v.sort_by(|a, b| b.1.cmp(&a.1));
    let top_level_dirs = dir_v
        .into_iter()
        .map(|(name, count, b)| json!({ "name": name, "files": count, "bytes": b }))
        .collect();

    OverviewSummary {
        files,
        bytes,
        stopped_at_cap,
        top_extensions,
        top_level_dirs,
    }
}

fn read_readme_head(root: &PathBuf) -> Option<String> {
    for name in ["README.md", "README", "README.rst", "Readme.md"] {
        let p = root.join(name);
        if let Ok(content) = std::fs::read_to_string(&p) {
            return Some(
                content
                    .lines()
                    .take(README_HEAD_LINES)
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn empty_dir_reports_zero_files() {
        let tmp = TempDir::new().unwrap();
        let r = RepoOverviewTool
            .invoke(
                json!({ "cwd": tmp.path().to_str().unwrap() }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(r["files"].as_u64(), Some(0));
        assert_eq!(r["bytes"].as_u64(), Some(0));
        assert!(r["readme_head"].is_null());
    }

    #[tokio::test]
    async fn counts_files_and_groups_by_extension() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), b"fn x() {}").unwrap();
        std::fs::write(tmp.path().join("b.rs"), b"fn y() {}").unwrap();
        std::fs::write(tmp.path().join("c.md"), b"# hello").unwrap();
        let r = RepoOverviewTool
            .invoke(
                json!({ "cwd": tmp.path().to_str().unwrap() }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(r["files"].as_u64(), Some(3));
        let exts = r["top_extensions"].as_array().unwrap();
        // First extension by bytes should be `rs` (more bytes total).
        assert_eq!(exts[0]["extension"].as_str(), Some("rs"));
        assert_eq!(exts[0]["files"].as_u64(), Some(2));
    }

    #[tokio::test]
    async fn reports_top_level_dirs() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("src")).unwrap();
        std::fs::create_dir(tmp.path().join("docs")).unwrap();
        std::fs::write(tmp.path().join("src/lib.rs"), b"pub fn x() {}").unwrap();
        std::fs::write(tmp.path().join("docs/intro.md"), b"# intro").unwrap();
        let r = RepoOverviewTool
            .invoke(
                json!({ "cwd": tmp.path().to_str().unwrap() }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let dirs = r["top_level_dirs"].as_array().unwrap();
        let names: Vec<String> = dirs
            .iter()
            .filter_map(|v| v["name"].as_str().map(|s| s.to_string()))
            .collect();
        assert!(names.iter().any(|n| n == "src"));
        assert!(names.iter().any(|n| n == "docs"));
    }

    #[tokio::test]
    async fn includes_readme_head() {
        let tmp = TempDir::new().unwrap();
        let readme = (1..=200)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(tmp.path().join("README.md"), readme.as_bytes()).unwrap();
        let r = RepoOverviewTool
            .invoke(
                json!({ "cwd": tmp.path().to_str().unwrap() }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let head = r["readme_head"].as_str().unwrap();
        // We capped at 80 lines.
        let line_count = head.lines().count();
        assert!(line_count <= 80, "expected ≤ 80 lines, got {line_count}");
        assert!(head.starts_with("line 1\n"));
        assert!(!head.contains("line 200"));
    }

    #[tokio::test]
    async fn rejects_relative_cwd() {
        let r = RepoOverviewTool
            .invoke(json!({ "cwd": "relative" }), CancellationToken::new())
            .await
            .unwrap();
        assert!(r.get("error").is_some());
    }

    #[tokio::test]
    async fn max_files_caps_walk() {
        let tmp = TempDir::new().unwrap();
        for i in 0..10 {
            std::fs::write(tmp.path().join(format!("f{i}.txt")), b"x").unwrap();
        }
        let r = RepoOverviewTool
            .invoke(
                json!({ "cwd": tmp.path().to_str().unwrap(), "max_files": 3 }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(r["files"].as_u64(), Some(3));
        assert_eq!(r["stopped_at_cap"].as_bool(), Some(true));
    }
}
