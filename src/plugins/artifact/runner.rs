//! Artifact-skill runner: spawn claude with a skill's prompt and a
//! source note's body, capture each `.md` file claude writes inside
//! the project's per-source artifacts directory, and import every
//! captured file as a new `NoteKind::Artifact` note linked back to
//! the source.
//!
//! This sits alongside the workflow-canvas executor (which runs
//! single-output skills against a static DAG). Same claude plugin,
//! same chat-message persistence shape; different ingestion model:
//! N output files → N artifact notes, parented to the source so the
//! explorer's tree mirrors the BA → Architect → Engineer hierarchy.

#![cfg(not(target_arch = "wasm32"))]

use futures::StreamExt;
use operon_plugins_claude_code::{ClaudeCodeChatPlugin, ClaudeCodeEvent};
use operon_store::repos::{
    ChatMessageKind, ChatMessageRepository, LocalNoteRepository, LocalProjectRepository, NoteKind,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::persistence::Persistence;
use crate::plugins::artifact::frontmatter::{
    rewrite as rewrite_artifact_fm, ArtifactKind, ArtifactStatus,
};
use crate::plugins::skill::frontmatter::{
    contract as parse_skill_contract, split as split_skill,
};

#[derive(Debug)]
pub enum RunnerError {
    NotFound(String),
    InvalidPath(String),
    Plugin(String),
    Io(std::io::Error),
}

impl std::fmt::Display for RunnerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(s) => write!(f, "not found: {s}"),
            Self::InvalidPath(s) => write!(f, "invalid path: {s}"),
            Self::Plugin(s) => write!(f, "claude: {s}"),
            Self::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl From<std::io::Error> for RunnerError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Outcome of one artifact-skill run.
#[derive(Debug, Clone)]
pub struct RunOutcome {
    /// Note ids of artifacts the run created (in import order).
    pub created_artifact_ids: Vec<Uuid>,
    /// Path under the repo where claude wrote the artifact files (for
    /// debugging — also accessible via `cached_output_path` on each
    /// imported note).
    pub artifacts_dir: PathBuf,
}

/// Entry point. The caller is responsible for having bound the
/// claude plugin to `chat_session_id` against the project's repo
/// path before invoking this — same convention as the workflow
/// executor.
#[allow(clippy::too_many_arguments)]
pub async fn run_skill_on_source(
    note_repo: &Arc<dyn LocalNoteRepository>,
    project_repo: &Arc<dyn LocalProjectRepository>,
    persistence: &Arc<dyn Persistence>,
    plugin: &Arc<ClaudeCodeChatPlugin>,
    chat_repo: Option<&Arc<dyn ChatMessageRepository>>,
    chat_session_id: Uuid,
    source_note_id: Uuid,
    skill_note_id: Uuid,
) -> Result<RunOutcome, RunnerError> {
    // 1. Resolve project + repo_path.
    let project_id = note_repo
        .find_project_for_note(source_note_id)
        .map_err(|e| RunnerError::Plugin(format!("find_project: {e}")))?
        .ok_or_else(|| RunnerError::NotFound(format!("source note {source_note_id} has no project")))?;
    let repo_path: PathBuf = project_repo
        .list()
        .map_err(|e| RunnerError::Plugin(format!("list projects: {e}")))?
        .into_iter()
        .find(|p| p.id == project_id)
        .ok_or_else(|| RunnerError::NotFound(format!("project {project_id}")))?
        .repo_path
        .ok_or_else(|| RunnerError::InvalidPath("project has no repo_path bound".into()))?;

    // 2. Load source body + skill body from persistence.
    let source_bytes = persistence
        .load(&source_note_id.to_string())
        .await
        .map_err(|e| RunnerError::NotFound(format!("source body: {e}")))?;
    let source_body =
        String::from_utf8(source_bytes).map_err(|e| RunnerError::Plugin(format!("utf8: {e}")))?;
    let skill_bytes = persistence
        .load(&skill_note_id.to_string())
        .await
        .map_err(|e| RunnerError::NotFound(format!("skill body: {e}")))?;
    let skill_body =
        String::from_utf8(skill_bytes).map_err(|e| RunnerError::Plugin(format!("utf8: {e}")))?;

    // 3. Parse skill contract — input/output kind, gate, etc.
    let (skill_fm_lines, _) = split_skill(&skill_body);
    let lines = skill_fm_lines.unwrap_or_default();
    let contract = parse_skill_contract(&lines);

    // 4. Where claude is going to write the new artifact files. One
    //    subdir per source so the engine can scan a known place after
    //    the run completes.
    let artifacts_dir = repo_path
        .join(".operon")
        .join("artifacts")
        .join(source_note_id.to_string());
    std::fs::create_dir_all(&artifacts_dir)?;
    let run_started_at = SystemTime::now();

    // 5. Pre-snapshot the directory so we only import files claude
    //    creates *during this run*, not pre-existing leftovers from a
    //    prior run that the user already imported.
    let existing: HashSet<PathBuf> = list_md_files(&artifacts_dir);

    // 6. Build the prompt that claude will see.
    let prompt = build_prompt(&source_body, &skill_body, &artifacts_dir, &contract, source_note_id, skill_note_id);

    // 7. Persist the prompt as a User message (transcript visibility).
    if let Some(repo) = chat_repo {
        let _ = repo.append(
            chat_session_id,
            ChatMessageKind::User,
            None,
            &serde_json::json!({ "text": prompt.clone() }),
        );
    }

    // 8. Run claude. The runner forces `acceptEdits` on this
    //    session so its automated Write tool calls don't hang
    //    waiting for stdin approval — even when the user's global
    //    permission picker is set to "default". Normal companion
    //    chats keep using whatever the user picked, since they
    //    don't set a per-session override.
    plugin.set_session_permission_mode(chat_session_id, Some("acceptEdits".into()));
    eprintln!(
        "operon: artifact runner start source={source_note_id} skill={skill_note_id} \
         dir={} prompt_len={}",
        artifacts_dir.display(),
        prompt.len(),
    );
    let ct = CancellationToken::new();
    let mut rx = plugin
        .send_rich(prompt, chat_session_id, ct)
        .await
        .map_err(|e| RunnerError::Plugin(format!("send_rich: {e}")))?;
    let mut assistant_buf = String::new();
    while let Some(ev) = rx.next().await {
        // Persist event to the rail's chat_message (mirroring the
        // workflow executor's pattern).
        if let Some(repo) = chat_repo {
            persist_event(repo, chat_session_id, &ev, &mut assistant_buf);
        }
        match ev {
            ClaudeCodeEvent::Done { .. } => break,
            ClaudeCodeEvent::Error(msg) => {
                if let Some(repo) = chat_repo {
                    let _ = repo.append(
                        chat_session_id,
                        ChatMessageKind::System,
                        None,
                        &serde_json::json!({ "text": format!("error: {msg}") }),
                    );
                }
                return Err(RunnerError::Plugin(msg));
            }
            _ => {}
        }
    }
    // Flush any leftover assistant text the persist helper buffered.
    if let Some(repo) = chat_repo {
        if !assistant_buf.is_empty() {
            let _ = repo.append(
                chat_session_id,
                ChatMessageKind::Assistant,
                None,
                &serde_json::json!({ "text": std::mem::take(&mut assistant_buf) }),
            );
        }
    }

    // 9. Scan the artifacts dir for files that are either new or
    //    have an mtime past `run_started_at` (claude may have
    //    overwritten an existing file on a re-run).
    let produced = scan_produced_files(&artifacts_dir, &existing, run_started_at);
    eprintln!(
        "operon: artifact runner produced {} file(s) in {}",
        produced.len(),
        artifacts_dir.display()
    );

    // 10. Import each produced file as an Artifact note under the
    //     source. Body is read from disk; frontmatter is patched so
    //     the engine's view fields (status, source linkage) are
    //     authoritative regardless of what claude wrote.
    let mut created_ids: Vec<Uuid> = Vec::new();
    for file in produced {
        let body = match std::fs::read_to_string(&file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("operon: artifact import skipped {} ({e})", file.display());
                continue;
            }
        };
        let title = file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("artifact")
            .to_string();
        let row = match note_repo.create_with_kind(
            project_id,
            Some(source_note_id),
            &title,
            NoteKind::Artifact,
        ) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("operon: artifact create_with_kind failed: {e}");
                continue;
            }
        };
        // Patch the artifact frontmatter so source_artifact_id /
        // source_skill_id / status are always correct, even if the
        // skill prompt forgot to emit them.
        let mut fm = crate::plugins::artifact::frontmatter::parse(&body);
        if fm.artifact_kind.is_none() {
            fm.artifact_kind = contract
                .output_kind
                .as_deref()
                .map(ArtifactKind::parse);
        }
        fm.status = match contract.gate {
            crate::plugins::skill::frontmatter::SkillGate::Auto => {
                ArtifactStatus::Approved
            }
            crate::plugins::skill::frontmatter::SkillGate::Approval => {
                ArtifactStatus::Pending
            }
        };
        fm.source_artifact_id = Some(source_note_id);
        fm.source_skill_id = Some(skill_note_id);
        let final_body = rewrite_artifact_fm(&body, &fm);
        if let Err(e) = persistence
            .save(&row.id.to_string(), final_body.as_bytes())
            .await
        {
            eprintln!("operon: artifact persistence save failed: {e}");
            continue;
        }
        created_ids.push(row.id);
    }

    Ok(RunOutcome {
        created_artifact_ids: created_ids,
        artifacts_dir,
    })
}

fn build_prompt(
    source_body: &str,
    skill_body: &str,
    artifacts_dir: &Path,
    contract: &crate::plugins::skill::frontmatter::SkillContract,
    source_id: Uuid,
    skill_id: Uuid,
) -> String {
    let mut buf = String::new();
    buf.push_str(
        "You are running an SDLC artifact-producing skill. Read the source\n\
         artifact below, follow the skill instructions, and write each output\n\
         artifact as a SEPARATE markdown file using the Write tool.\n\n",
    );

    buf.push_str(&format!(
        "Each output file must be written under the absolute directory:\n  {}\n\
         using a short kebab-case filename like `epic-user-auth.md`.\n\n",
        artifacts_dir.display()
    ));

    let kind_label = contract.output_kind.as_deref().unwrap_or("artifact");
    buf.push_str(&format!(
        "Each file MUST start with this YAML frontmatter (and nothing else\n\
         before it):\n\n\
         ```yaml\n\
         ---\n\
         artifact_kind: {kind_label}\n\
         status: pending\n\
         source_artifact_id: {source_id}\n\
         source_skill_id: {skill_id}\n\
         ---\n\
         ```\n\n\
         Then the artifact body in markdown. The first heading should match\n\
         the file name (in human-readable form).\n\n"
    ));

    buf.push_str("--- skill body ---\n");
    buf.push_str(skill_body.trim_end());
    buf.push_str("\n--- /skill body ---\n\n");

    buf.push_str("--- source artifact body ---\n");
    buf.push_str(source_body.trim_end());
    buf.push_str("\n--- /source artifact body ---\n\n");

    buf.push_str(
        "When done, do NOT echo the artifact contents back to the user — the\n\
         engine reads them from disk. A short summary of how many artifacts\n\
         you produced is enough.\n",
    );
    buf
}

/// List `.md` files (top-level only — no recursion) in `dir`.
/// Returns absolute, canonicalized paths so the diff against post-run
/// state works regardless of how `dir` was originally constructed.
fn list_md_files(dir: &Path) -> HashSet<PathBuf> {
    let mut out = HashSet::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(it) => it,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("md") {
            if let Ok(canon) = path.canonicalize() {
                out.insert(canon);
            } else {
                out.insert(path);
            }
        }
    }
    out
}

/// Find files in `dir` that are either NEW (not in `pre_existing`) or
/// were modified after `run_started_at` (the latter handles re-runs
/// that overwrite a prior file). Returned in lexicographic order so
/// imports are deterministic across runs.
fn scan_produced_files(
    dir: &Path,
    pre_existing: &HashSet<PathBuf>,
    run_started_at: SystemTime,
) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(it) => it,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !(path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("md")) {
            continue;
        }
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        let is_new = !pre_existing.contains(&canonical);
        let is_recent = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .map(|t| t >= run_started_at)
            .unwrap_or(false);
        if is_new || is_recent {
            out.push(canonical);
        }
    }
    out.sort();
    out
}

/// Mirror the workflow executor's chat-message-persist contract so
/// the rail entry reads the same as a regular companion chat:
/// User → Assistant → ToolCall → ToolResult flow. Text deltas are
/// buffered into a single Assistant row per turn (caller flushes
/// after the loop ends).
fn persist_event(
    repo: &Arc<dyn ChatMessageRepository>,
    chat_session_id: Uuid,
    ev: &ClaudeCodeEvent,
    assistant_buf: &mut String,
) {
    use ClaudeCodeEvent::*;
    let flush = |buf: &mut String| {
        if buf.is_empty() {
            return;
        }
        let _ = repo.append(
            chat_session_id,
            ChatMessageKind::Assistant,
            None,
            &serde_json::json!({ "text": std::mem::take(buf) }),
        );
    };
    match ev {
        Text(t) => assistant_buf.push_str(t),
        Thinking(t) => {
            flush(assistant_buf);
            let _ = repo.append(
                chat_session_id,
                ChatMessageKind::Thinking,
                None,
                &serde_json::json!({ "text": t }),
            );
        }
        ToolUse { id, name, input } => {
            flush(assistant_buf);
            let _ = repo.append(
                chat_session_id,
                ChatMessageKind::ToolCall,
                Some(id),
                &serde_json::json!({
                    "id": id,
                    "name": name,
                    "input": input,
                    "result": serde_json::Value::Null,
                }),
            );
        }
        ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            let _ = repo.update_tool_result(
                chat_session_id,
                tool_use_id,
                &serde_json::json!({
                    "id": tool_use_id,
                    "result": { "content": content, "is_error": is_error },
                }),
            );
        }
        Done { .. } | Error(_) => {
            flush(assistant_buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_md_files_returns_only_md() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.md"), "x").unwrap();
        std::fs::write(dir.path().join("b.txt"), "y").unwrap();
        std::fs::create_dir(dir.path().join("nested")).unwrap();
        let set = list_md_files(dir.path());
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn scan_produced_files_diffs_against_pre_existing() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join("stale.md");
        std::fs::write(&stale, "x").unwrap();
        let pre = list_md_files(dir.path());
        let started = SystemTime::now();
        // Sleep so the new file's mtime is provably after `started`.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let fresh = dir.path().join("fresh.md");
        std::fs::write(&fresh, "y").unwrap();
        let produced = scan_produced_files(dir.path(), &pre, started);
        assert_eq!(produced.len(), 1);
        assert_eq!(produced[0].file_name().unwrap(), "fresh.md");
    }

    #[test]
    fn build_prompt_inlines_source_and_skill_and_dir() {
        let dir = std::path::Path::new("/tmp/x");
        let mut contract = crate::plugins::skill::frontmatter::SkillContract::default();
        contract.output_kind = Some("epic".into());
        let prompt = build_prompt(
            "REQ_BODY",
            "SKILL_BODY",
            dir,
            &contract,
            Uuid::nil(),
            Uuid::nil(),
        );
        assert!(prompt.contains("REQ_BODY"));
        assert!(prompt.contains("SKILL_BODY"));
        assert!(prompt.contains("/tmp/x"));
        assert!(prompt.contains("artifact_kind: epic"));
    }
}
