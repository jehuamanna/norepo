//! `git` tool — safe git subcommands via `git2-rs`.
//!
//! Allowed: status, diff, log, branch_list, add, commit. **No** push, force,
//! reset --hard. The agent must use the `shell` tool (with explicit user
//! approval) for anything destructive.

use operon_core::error::{OperonError, OperonResult};
use operon_core::traits::{
    CancellationToken, Capabilities, Plugin, ToolDef, ToolPlugin,
};
use async_trait::async_trait;
use git2::{Repository, Status, StatusOptions};
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;

#[derive(Deserialize)]
struct GitInput {
    subcommand: String,
    #[serde(default)]
    args: serde_json::Value,
    #[serde(default)]
    cwd: Option<String>,
}

pub struct GitTool;

#[async_trait]
impl Plugin for GitTool {
    fn name(&self) -> &str { "git" }
    fn version(&self) -> &str { env!("CARGO_PKG_VERSION") }
    fn capabilities(&self) -> Capabilities { Capabilities::empty() }
}

#[async_trait]
impl ToolPlugin for GitTool {
    fn schema(&self) -> ToolDef {
        ToolDef {
            name: "git".into(),
            description: "Run a safe git subcommand. \
                          Allowed: status, diff (HEAD vs working tree), log (--oneline, count), \
                          branch_list, add, commit. No push / force / reset --hard. \
                          For those, use the shell tool with explicit user approval."
                          .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "subcommand": {
                        "type": "string",
                        "enum": ["status", "diff", "log", "branch_list", "add", "commit"]
                    },
                    "args":        { "type": "object", "description": "Subcommand-specific args (e.g. {paths: [...]}, {message: '...'})." },
                    "cwd":         { "type": "string", "description": "Absolute path to the repo root." }
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
        let input: GitInput = serde_json::from_value(args).map_err(|e| OperonError::Plugin {
            plugin: "git".into(),
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

        let sub = input.subcommand.clone();
        let sub_args = input.args.clone();
        // git2 is sync; offload to spawn_blocking.
        let out = tokio::task::spawn_blocking(move || run_subcommand(&cwd, &sub, &sub_args))
            .await
            .map_err(|e| OperonError::Plugin {
                plugin: "git".into(),
                source: Box::new(std::io::Error::other(format!("join: {e}"))),
            })??;
        Ok(out)
    }
}

fn run_subcommand(
    cwd: &PathBuf,
    sub: &str,
    args: &serde_json::Value,
) -> OperonResult<serde_json::Value> {
    let repo = Repository::discover(cwd).map_err(|e| OperonError::Plugin {
        plugin: "git".into(),
        source: Box::new(std::io::Error::other(format!("not a git repo at {}: {e}", cwd.display()))),
    })?;

    match sub {
        "status" => {
            let mut opts = StatusOptions::new();
            opts.include_untracked(true).renames_head_to_index(true);
            let statuses = repo.statuses(Some(&mut opts)).map_err(map_git_err)?;
            let mut entries = Vec::new();
            for s in statuses.iter() {
                let path = s.path().unwrap_or("<non-utf8>").to_string();
                entries.push(json!({
                    "path": path,
                    "wt_new":      s.status().contains(Status::WT_NEW),
                    "wt_modified": s.status().contains(Status::WT_MODIFIED),
                    "wt_deleted":  s.status().contains(Status::WT_DELETED),
                    "index_new":      s.status().contains(Status::INDEX_NEW),
                    "index_modified": s.status().contains(Status::INDEX_MODIFIED),
                    "index_deleted":  s.status().contains(Status::INDEX_DELETED),
                }));
            }
            Ok(json!({ "entries": entries }))
        }
        "diff" => {
            // HEAD ↔ working tree (combines staged + unstaged).
            let head = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
            let diff = repo
                .diff_tree_to_workdir_with_index(head.as_ref(), None)
                .map_err(map_git_err)?;
            let mut text = String::new();
            diff.print(git2::DiffFormat::Patch, |_d, _h, l| {
                let prefix = match l.origin() {
                    '+' | '-' | ' ' => format!("{}", l.origin()),
                    _ => String::new(),
                };
                text.push_str(&prefix);
                text.push_str(std::str::from_utf8(l.content()).unwrap_or(""));
                true
            })
            .map_err(map_git_err)?;
            Ok(json!({ "diff": text }))
        }
        "log" => {
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
            let mut walk = repo.revwalk().map_err(map_git_err)?;
            walk.push_head().map_err(map_git_err)?;
            let mut entries = Vec::new();
            for (i, oid) in walk.enumerate() {
                if i >= limit {
                    break;
                }
                let oid = oid.map_err(map_git_err)?;
                let commit = repo.find_commit(oid).map_err(map_git_err)?;
                let summary = commit.summary().unwrap_or("").to_string();
                let author = commit.author();
                entries.push(json!({
                    "sha": oid.to_string(),
                    "short": &oid.to_string()[..7.min(oid.to_string().len())],
                    "summary": summary,
                    "author": format!("{} <{}>", author.name().unwrap_or(""), author.email().unwrap_or("")),
                    "time": commit.time().seconds(),
                }));
            }
            Ok(json!({ "entries": entries }))
        }
        "branch_list" => {
            let mut entries = Vec::new();
            for b in repo.branches(None).map_err(map_git_err)? {
                let (br, kind) = b.map_err(map_git_err)?;
                let name = br.name().map_err(map_git_err)?.unwrap_or("").to_string();
                let is_head = br.is_head();
                entries.push(json!({
                    "name": name,
                    "kind": format!("{kind:?}"),
                    "is_head": is_head,
                }));
            }
            Ok(json!({ "entries": entries }))
        }
        "add" => {
            let paths: Vec<String> = args
                .get("paths")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default();
            if paths.is_empty() {
                return Ok(json!({ "error": "args.paths must be a non-empty array of strings" }));
            }
            let mut idx = repo.index().map_err(map_git_err)?;
            for p in &paths {
                idx.add_path(std::path::Path::new(p)).map_err(map_git_err)?;
            }
            idx.write().map_err(map_git_err)?;
            Ok(json!({ "added": paths }))
        }
        "commit" => {
            let message = match args.get("message").and_then(|v| v.as_str()) {
                Some(m) if !m.trim().is_empty() => m.to_string(),
                _ => return Ok(json!({ "error": "args.message is required and non-empty" })),
            };
            let sig = repo.signature().map_err(map_git_err)?;
            let mut idx = repo.index().map_err(map_git_err)?;
            let tree_oid = idx.write_tree().map_err(map_git_err)?;
            let tree = repo.find_tree(tree_oid).map_err(map_git_err)?;
            let parents: Vec<git2::Commit> = match repo.head().ok().and_then(|h| h.peel_to_commit().ok()) {
                Some(c) => vec![c],
                None => Vec::new(),
            };
            let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
            let oid = repo
                .commit(Some("HEAD"), &sig, &sig, &message, &tree, &parent_refs)
                .map_err(map_git_err)?;
            Ok(json!({ "sha": oid.to_string(), "message": message }))
        }
        other => Ok(json!({ "error": format!("subcommand not allowed: {other}") })),
    }
}

fn map_git_err(e: git2::Error) -> OperonError {
    OperonError::Plugin {
        plugin: "git".into(),
        source: Box::new(std::io::Error::other(format!("git: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn init_repo(tmp: &TempDir) -> Repository {
        let repo = Repository::init(tmp.path()).unwrap();
        // Required so commit() can build a signature.
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Test").unwrap();
        config.set_str("user.email", "test@example.com").unwrap();
        repo
    }

    #[tokio::test]
    async fn status_on_empty_repo() {
        let tmp = TempDir::new().unwrap();
        init_repo(&tmp);
        let r = GitTool
            .invoke(
                json!({ "subcommand": "status", "cwd": tmp.path().to_str().unwrap() }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(r.get("entries").is_some());
    }

    #[tokio::test]
    async fn status_with_untracked_file() {
        let tmp = TempDir::new().unwrap();
        init_repo(&tmp);
        std::fs::write(tmp.path().join("new.txt"), b"x").unwrap();
        let r = GitTool
            .invoke(
                json!({ "subcommand": "status", "cwd": tmp.path().to_str().unwrap() }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let entries = r.get("entries").and_then(|v| v.as_array()).unwrap();
        let new = entries.iter().find(|e| e.get("path").and_then(|v| v.as_str()) == Some("new.txt"));
        assert!(new.is_some());
        assert_eq!(new.unwrap().get("wt_new").and_then(|v| v.as_bool()), Some(true));
    }

    #[tokio::test]
    async fn add_then_commit_round_trip() {
        let tmp = TempDir::new().unwrap();
        init_repo(&tmp);
        std::fs::write(tmp.path().join("a.txt"), b"hello").unwrap();
        let _ = GitTool
            .invoke(
                json!({ "subcommand": "add", "args": { "paths": ["a.txt"] }, "cwd": tmp.path().to_str().unwrap() }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let r = GitTool
            .invoke(
                json!({ "subcommand": "commit", "args": { "message": "first" }, "cwd": tmp.path().to_str().unwrap() }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(r.get("sha").is_some());

        // log shows the new commit
        let log = GitTool
            .invoke(
                json!({ "subcommand": "log", "cwd": tmp.path().to_str().unwrap() }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let entries = log.get("entries").and_then(|v| v.as_array()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].get("summary").and_then(|v| v.as_str()), Some("first"));
    }

    #[tokio::test]
    async fn rejects_unsafe_subcommand() {
        let tmp = TempDir::new().unwrap();
        init_repo(&tmp);
        // Unknown variant rejected at deserialize time? No, the field is a String;
        // but our schema enum filters at agent level — runtime should also reject.
        // Try a bypass via raw JSON.
        let r = GitTool
            .invoke(
                json!({ "subcommand": "push", "cwd": tmp.path().to_str().unwrap() }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(r.get("error").and_then(|v| v.as_str()).unwrap().contains("not allowed"));
    }

    #[tokio::test]
    async fn empty_commit_message_rejected() {
        let tmp = TempDir::new().unwrap();
        init_repo(&tmp);
        let r = GitTool
            .invoke(
                json!({ "subcommand": "commit", "args": { "message": "" }, "cwd": tmp.path().to_str().unwrap() }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(r.get("error").is_some());
    }
}
