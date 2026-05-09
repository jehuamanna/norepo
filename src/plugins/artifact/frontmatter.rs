//! Artifact frontmatter parser.
//!
//! Artifacts are notes produced by SDLC-pipeline skill runs (BA →
//! Architect → Engineer phases). The note body is markdown; the
//! frontmatter declares the artifact's place in the pipeline:
//!
//! ```text
//! ---
//! artifact_kind: epic           # epic | feature | story | task | plan | test_cases | summary | requirements
//! status: pending               # pending | approved | rejected | dirty | running | error
//! source_artifact_id: <uuid>    # the artifact this was derived from (root artifacts have None)
//! source_skill_id: <uuid>       # the skill that produced this
//! input_hash: <sha256>          # snapshot of inputs; lets us detect dirty descendants
//! ---
//!
//! # The body the user reads + edits
//! ```
//!
//! Fields are optional; the parser returns `None` for any missing key.
//! Status mutations rewrite the frontmatter block in place, leaving
//! the body untouched.
//!
//! We deliberately reuse `crate::plugins::skill::frontmatter::split`
//! rather than a YAML crate — the field set is small and the value
//! grammar is "string" or "uuid", trivially tokenized.
//!
//! See `~/.claude/plans/a-new-note-in-lovely-acorn.md` (Phase 2 follow-up
//! design discussion) for the full pipeline rationale.

use uuid::Uuid;

use crate::plugins::skill::frontmatter::{field, split};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactKind {
    /// The user-authored seed: a Markdown note titled "Requirements"
    /// (or any markdown body) that the BA pipeline starts from. This
    /// kind is observed on synthetic root artifacts created when a
    /// non-Artifact note is used as the cascade entry.
    Requirements,
    Epic,
    Feature,
    Story,
    Task,
    Plan,
    Implementation,
    TestCases,
    TestResults,
    Summary,
    /// Aggregated cross-task backlog produced by a prioritization
    /// skill (e.g. `04b-pm-prioritize-tasks-coarse`). Body holds a
    /// priority-ordered list of every Task under the seed plus a
    /// dependency rationale; a sibling Workflow note carries the
    /// React Flow DAG snapshot.
    PrioritizedBacklog,
    /// Catch-all so a skill can declare a custom kind without forcing
    /// a code change here. Downstream code that wants typed handling
    /// matches on the named variants and falls back to display-only
    /// behavior for `Other`.
    Other(String),
}

impl ArtifactKind {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Requirements => "requirements",
            Self::Epic => "epic",
            Self::Feature => "feature",
            Self::Story => "story",
            Self::Task => "task",
            Self::Plan => "plan",
            Self::Implementation => "implementation",
            Self::TestCases => "test_cases",
            Self::TestResults => "test_results",
            Self::Summary => "summary",
            Self::PrioritizedBacklog => "prioritized_backlog",
            Self::Other(s) => s.as_str(),
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.trim() {
            "requirements" => Self::Requirements,
            "epic" => Self::Epic,
            "feature" => Self::Feature,
            "story" => Self::Story,
            "task" => Self::Task,
            "plan" => Self::Plan,
            "implementation" => Self::Implementation,
            "test_cases" => Self::TestCases,
            "test_results" => Self::TestResults,
            "summary" => Self::Summary,
            "prioritized_backlog" => Self::PrioritizedBacklog,
            other => Self::Other(other.to_string()),
        }
    }

    pub fn display_name(&self) -> String {
        match self {
            Self::Requirements => "Requirements".into(),
            Self::Epic => "Epic".into(),
            Self::Feature => "Feature".into(),
            Self::Story => "Story".into(),
            Self::Task => "Task".into(),
            Self::Plan => "Plan".into(),
            Self::Implementation => "Implementation".into(),
            Self::TestCases => "Test Cases".into(),
            Self::TestResults => "Test Results".into(),
            Self::Summary => "Summary".into(),
            Self::PrioritizedBacklog => "Prioritized Backlog".into(),
            Self::Other(s) => s.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactStatus {
    Pending,
    Approved,
    Rejected,
    Dirty,
    Running,
    Error,
}

impl ArtifactStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Dirty => "dirty",
            Self::Running => "running",
            Self::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.trim() {
            "approved" => Self::Approved,
            "rejected" => Self::Rejected,
            "dirty" => Self::Dirty,
            "running" => Self::Running,
            "error" => Self::Error,
            // Default to pending for any unknown / missing value so a
            // freshly imported artifact without an explicit status
            // still shows up correctly in the UI.
            _ => Self::Pending,
        }
    }
}

impl Default for ArtifactStatus {
    fn default() -> Self {
        Self::Pending
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactFrontmatter {
    pub artifact_kind: Option<ArtifactKind>,
    pub status: ArtifactStatus,
    pub source_artifact_id: Option<Uuid>,
    pub source_skill_id: Option<Uuid>,
    pub input_hash: Option<String>,
}

impl Default for ArtifactFrontmatter {
    fn default() -> Self {
        Self {
            artifact_kind: None,
            status: ArtifactStatus::Pending,
            source_artifact_id: None,
            source_skill_id: None,
            input_hash: None,
        }
    }
}

/// Parse a note's body into the typed artifact frontmatter view.
/// Always succeeds — fields that are missing or unparseable land as
/// `None` / defaults so a half-written note still renders.
pub fn parse(body: &str) -> ArtifactFrontmatter {
    let (lines_opt, _) = split(body);
    let lines = match lines_opt {
        Some(l) => l,
        None => return ArtifactFrontmatter::default(),
    };
    let artifact_kind = field(&lines, "artifact_kind").map(ArtifactKind::parse);
    let status = field(&lines, "status")
        .map(ArtifactStatus::parse)
        .unwrap_or_default();
    let source_artifact_id = field(&lines, "source_artifact_id")
        .and_then(|s| Uuid::parse_str(s).ok());
    let source_skill_id = field(&lines, "source_skill_id")
        .and_then(|s| Uuid::parse_str(s).ok());
    let input_hash = field(&lines, "input_hash").map(str::to_string);
    ArtifactFrontmatter {
        artifact_kind,
        status,
        source_artifact_id,
        source_skill_id,
        input_hash,
    }
}

/// Replace the frontmatter block in `body` with one rebuilt from
/// `next`. Preserves the body content after the closing `---`. Used
/// by Approve / Reject / Re-run actions to flip just the status (or
/// any subset of fields) without touching the markdown the user has
/// edited.
pub fn rewrite(body: &str, next: &ArtifactFrontmatter) -> String {
    let (existing, body_only) = split(body);
    let mut out = String::new();
    out.push_str("---\n");
    if let Some(kind) = next.artifact_kind.as_ref() {
        out.push_str(&format!("artifact_kind: {}\n", kind.as_str()));
    }
    out.push_str(&format!("status: {}\n", next.status.as_str()));
    if let Some(id) = next.source_artifact_id {
        out.push_str(&format!("source_artifact_id: {id}\n"));
    }
    if let Some(id) = next.source_skill_id {
        out.push_str(&format!("source_skill_id: {id}\n"));
    }
    if let Some(h) = next.input_hash.as_ref() {
        out.push_str(&format!("input_hash: {h}\n"));
    }
    // Preserve any *other* keys the skill emitted that we don't
    // model here (e.g. acceptance_criteria, dependencies). They
    // round-trip verbatim so the artifact view's "Edit body" path
    // doesn't blow them away.
    if let Some(prev_lines) = existing {
        const KNOWN: &[&str] = &[
            "artifact_kind",
            "status",
            "source_artifact_id",
            "source_skill_id",
            "input_hash",
        ];
        for line in &prev_lines {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some((k, _)) = trimmed.split_once(':') {
                if KNOWN.contains(&k.trim()) {
                    continue;
                }
            }
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str("---\n");
    out.push_str(body_only.trim_start_matches('\n'));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_extracts_artifact_kind_and_status() {
        let body = "---\nartifact_kind: epic\nstatus: approved\n---\n\nhello";
        let fm = parse(body);
        assert_eq!(fm.artifact_kind, Some(ArtifactKind::Epic));
        assert_eq!(fm.status, ArtifactStatus::Approved);
    }

    #[test]
    fn parse_defaults_status_to_pending() {
        let body = "---\nartifact_kind: feature\n---\nbody";
        let fm = parse(body);
        assert_eq!(fm.status, ArtifactStatus::Pending);
    }

    #[test]
    fn parse_extracts_implementation_kind() {
        let body = "---\nartifact_kind: implementation\nstatus: approved\n---\nbody";
        let fm = parse(body);
        assert_eq!(fm.artifact_kind, Some(ArtifactKind::Implementation));
        assert_eq!(
            ArtifactKind::Implementation.as_str(),
            "implementation"
        );
        assert_eq!(
            ArtifactKind::Implementation.display_name(),
            "Implementation"
        );
    }

    #[test]
    fn parse_extracts_test_results_kind() {
        let body = "---\nartifact_kind: test_results\nstatus: approved\n---\nbody";
        let fm = parse(body);
        assert_eq!(fm.artifact_kind, Some(ArtifactKind::TestResults));
        assert_eq!(ArtifactKind::TestResults.as_str(), "test_results");
        assert_eq!(
            ArtifactKind::TestResults.display_name(),
            "Test Results"
        );
    }

    #[test]
    fn parse_unrecognized_kind_falls_back_to_other() {
        let body = "---\nartifact_kind: design_doc\n---\nbody";
        let fm = parse(body);
        assert_eq!(
            fm.artifact_kind,
            Some(ArtifactKind::Other("design_doc".into()))
        );
    }

    #[test]
    fn parse_handles_no_frontmatter() {
        let body = "no frontmatter here, just text";
        let fm = parse(body);
        assert_eq!(fm.artifact_kind, None);
        assert_eq!(fm.status, ArtifactStatus::Pending);
    }

    #[test]
    fn rewrite_preserves_unknown_fields_and_body() {
        let body = "---\n\
            artifact_kind: epic\n\
            status: pending\n\
            acceptance_criteria: [\"a\", \"b\"]\n\
            ---\n\
            \n\
            # Body\n\
            real content";
        let mut fm = parse(body);
        fm.status = ArtifactStatus::Approved;
        let next = rewrite(body, &fm);
        // Status flipped:
        assert!(next.contains("status: approved"));
        // Body preserved:
        assert!(next.contains("# Body"));
        assert!(next.contains("real content"));
        // Unknown field preserved:
        assert!(next.contains("acceptance_criteria"));
    }

    #[test]
    fn rewrite_round_trips_uuids() {
        let id = Uuid::new_v4();
        let mut fm = ArtifactFrontmatter::default();
        fm.artifact_kind = Some(ArtifactKind::Story);
        fm.source_artifact_id = Some(id);
        let body = rewrite("", &fm);
        let parsed = parse(&body);
        assert_eq!(parsed.source_artifact_id, Some(id));
    }
}
