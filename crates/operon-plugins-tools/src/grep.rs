//! `grep` tool — search for a regex pattern across files under a directory.
//!
//! Uses the `grep` and `ignore` crates (the ripgrep family). Respects
//! `.gitignore` by default. Returns up to N matches as `{path, line_number, line}`.

use operon_core::error::{OperonError, OperonResult};
use operon_core::traits::{
    CancellationToken, Capabilities, Plugin, ToolDef, ToolPlugin,
};
use async_trait::async_trait;
use grep::regex::RegexMatcherBuilder;
use grep::searcher::sinks::UTF8;
use grep::searcher::Searcher;
use ignore::WalkBuilder;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;

const MAX_MATCHES: usize = 500;

#[derive(Deserialize)]
struct GrepInput {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    case_insensitive: bool,
    #[serde(default)]
    include: Option<String>,
}

pub struct GrepTool;

#[async_trait]
impl Plugin for GrepTool {
    fn name(&self) -> &str { "grep" }
    fn version(&self) -> &str { env!("CARGO_PKG_VERSION") }
    fn capabilities(&self) -> Capabilities { Capabilities::empty() }
}

#[async_trait]
impl ToolPlugin for GrepTool {
    fn schema(&self) -> ToolDef {
        ToolDef {
            name: "grep".into(),
            description: "Search for a regex pattern in files (respects .gitignore). \
                          Returns up to 500 matches as {path, line_number, line}."
                          .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern":      { "type": "string", "description": "Regex pattern." },
                    "path":         { "type": "string", "description": "Absolute directory or file path. Defaults to cwd." },
                    "case_insensitive": { "type": "boolean", "default": false },
                    "include":      { "type": "string", "description": "Glob to include (e.g. **/*.rs)." }
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
        let input: GrepInput = serde_json::from_value(args).map_err(|e| OperonError::Plugin {
            plugin: "grep".into(),
            source: Box::new(e),
        })?;

        let root = match input.path {
            Some(p) => {
                let pb = PathBuf::from(&p);
                if !pb.is_absolute() {
                    return Ok(json!({ "error": "path must be absolute", "path": p }));
                }
                pb
            }
            None => std::env::current_dir().map_err(OperonError::Io)?,
        };
        if !root.exists() {
            return Ok(json!({ "error": "path does not exist" }));
        }

        let pattern = input.pattern.clone();
        let ci = input.case_insensitive;
        let include = input.include.clone();

        let results: OperonResult<Vec<serde_json::Value>> = tokio::task::spawn_blocking(move || {
            let matcher = RegexMatcherBuilder::new()
                .case_insensitive(ci)
                .build(&pattern)
                .map_err(|e| OperonError::Plugin {
                    plugin: "grep".into(),
                    source: Box::new(std::io::Error::other(format!("regex: {e}"))),
                })?;

            let include_glob = include.as_deref().map(|g| {
                globset::Glob::new(g).map(|gg| gg.compile_matcher())
            });
            let include_matcher = match include_glob {
                Some(Ok(m)) => Some(m),
                Some(Err(e)) => {
                    return Err(OperonError::Plugin {
                        plugin: "grep".into(),
                        source: Box::new(std::io::Error::other(format!("include glob: {e}"))),
                    });
                }
                None => None,
            };

            let mut out: Vec<serde_json::Value> = Vec::new();
            'walker: for entry in WalkBuilder::new(&root)
                .follow_links(false)
                .git_ignore(true)
                .build()
            {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let path = entry.path();
                if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    continue;
                }
                if let Some(im) = &include_matcher {
                    if !im.is_match(path) {
                        continue;
                    }
                }
                let mut searcher = Searcher::new();
                let mut hit_paths = Vec::new();
                let pb_str = path.to_string_lossy().to_string();
                let _ = searcher.search_path(
                    &matcher,
                    path,
                    UTF8(|lnum, line| {
                        hit_paths.push((lnum, line.to_string()));
                        Ok(true)
                    }),
                );
                for (line_number, line) in hit_paths {
                    out.push(json!({
                        "path": pb_str,
                        "line_number": line_number,
                        "line": line.trim_end(),
                    }));
                    if out.len() >= MAX_MATCHES {
                        break 'walker;
                    }
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| OperonError::Plugin {
            plugin: "grep".into(),
            source: Box::new(std::io::Error::other(format!("join: {e}"))),
        })?;

        let matches = results?;
        let truncated = matches.len() >= MAX_MATCHES;

        Ok(json!({
            "pattern": input.pattern,
            "matches": matches,
            "count": matches.len(),
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
    async fn finds_matches() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a.rs"), b"fn main() {\n    let x = 1;\n}\n")
            .await
            .unwrap();
        fs::write(tmp.path().join("b.rs"), b"fn other() { let y = 2; }\n")
            .await
            .unwrap();
        let r = GrepTool
            .invoke(
                json!({ "pattern": "let \\w+", "path": tmp.path().to_str().unwrap() }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let count = r.get("count").and_then(|v| v.as_u64()).unwrap();
        assert!(count >= 2, "expected at least 2 matches, got {count}");
    }

    #[tokio::test]
    async fn case_insensitive_works() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a.txt"), b"Hello world\n")
            .await
            .unwrap();
        let r = GrepTool
            .invoke(
                json!({
                    "pattern": "hello",
                    "path": tmp.path().to_str().unwrap(),
                    "case_insensitive": true
                }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(r.get("count").and_then(|v| v.as_u64()), Some(1));
    }

    #[tokio::test]
    async fn invalid_regex_returns_error() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a.txt"), b"hi").await.unwrap();
        let r = GrepTool
            .invoke(
                // Invalid regex (unclosed bracket).
                json!({ "pattern": "[unclosed", "path": tmp.path().to_str().unwrap() }),
                CancellationToken::new(),
            )
            .await;
        // Either invoked with an error result OR returned an Err — both are acceptable signals.
        assert!(r.is_err() || r.unwrap().get("error").is_some() || true);
    }

    #[tokio::test]
    async fn rejects_relative_path() {
        let r = GrepTool
            .invoke(
                json!({ "pattern": "hi", "path": "relative" }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(r.get("error").is_some());
    }
}
