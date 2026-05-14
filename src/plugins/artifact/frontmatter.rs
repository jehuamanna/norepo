//! Artifact frontmatter parser.
//!
//! Artifacts are notes produced by SDLC-pipeline skill runs (BA →
//! Architect → Engineer phases). The note body is markdown; the
//! frontmatter declares the artifact's place in the pipeline:
//!
//! ```text
//! ---
//! artifact_kind: epic           # epic | feature | story | task | plan | implementation_plan | implementation | test_cases | summary | requirements
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
    /// Project root in the updated SDLC chain: holds the high-level
    /// charter ("build a portfolio management system") plus any
    /// CE-team inputs. The Play button is restricted to this kind so
    /// every cascade starts from master. `01-ba-aggregate-requirements`
    /// fans this out into multiple `Requirements` children.
    MasterRequirement,
    /// The user-authored seed: a Markdown note titled "Requirements"
    /// (or any markdown body) that the BA pipeline starts from. In
    /// the legacy chain this was the project root; in the updated
    /// chain it's one of many detailed requirement children under a
    /// `MasterRequirement`. Also observed on synthetic root artifacts
    /// created when a non-Artifact note is used as the cascade entry.
    Requirements,
    Epic,
    Feature,
    Story,
    Task,
    Plan,
    /// SDE plan-only artifact produced by `07a-sde-plan-task`. Captures
    /// the implementation approach (files to touch, design notes, test
    /// cues) without any code edits or commit. The Play button on this
    /// kind drives the actual code work via `07b-sde-execute-implementation`,
    /// which produces the downstream `Implementation` artifact.
    ImplementationPlan,
    Implementation,
    TestCases,
    TestResults,
    Summary,
    /// SA's single architecture note, iteratively revised in-place
    /// from the project's `MasterRequirement`. Inherited by SDE skills
    /// to scope implementation work.
    Architecture,
    /// Phase E: SA-authored review note flagging concerns that a new
    /// phase's requirements may raise against the existing
    /// Architecture. Always a direct child of an Architecture
    /// artifact; produced by `11-sa-review-architecture` either
    /// auto-fired after a non-first-phase cascade or manually
    /// triggered from the architecture's skill picker. Approving the
    /// review (or rejecting it) clears the parent architecture's
    /// `needs_review` flag once no Pending / Dirty reviews remain.
    ArchitectureReview,
    /// SDE-filed bug pointing at a specific Implementation. Consumed by
    /// the bug-fix skill, which produces a new Implementation revision.
    Bug,
    /// Cross-level discrepancy question emitted by the coherence-check
    /// skill. Body lists single/multi-choice options the user resolves
    /// in-place; the cascade halts until the artifact is Approved.
    Clarification,
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
            Self::MasterRequirement => "master_requirement",
            Self::Requirements => "requirements",
            Self::Epic => "epic",
            Self::Feature => "feature",
            Self::Story => "story",
            Self::Task => "task",
            Self::Plan => "plan",
            Self::ImplementationPlan => "implementation_plan",
            Self::Implementation => "implementation",
            Self::TestCases => "test_cases",
            Self::TestResults => "test_results",
            Self::Summary => "summary",
            Self::Architecture => "architecture",
            Self::ArchitectureReview => "architecture_review",
            Self::Bug => "bug",
            Self::Clarification => "clarification",
            Self::PrioritizedBacklog => "prioritized_backlog",
            Self::Other(s) => s.as_str(),
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.trim() {
            "master_requirement" => Self::MasterRequirement,
            "requirements" => Self::Requirements,
            "epic" => Self::Epic,
            "feature" => Self::Feature,
            "story" => Self::Story,
            "task" => Self::Task,
            "plan" => Self::Plan,
            "implementation_plan" => Self::ImplementationPlan,
            "implementation" => Self::Implementation,
            "test_cases" => Self::TestCases,
            "test_results" => Self::TestResults,
            "summary" => Self::Summary,
            "architecture" => Self::Architecture,
            "architecture_review" => Self::ArchitectureReview,
            "bug" => Self::Bug,
            "clarification" => Self::Clarification,
            "prioritized_backlog" => Self::PrioritizedBacklog,
            other => Self::Other(other.to_string()),
        }
    }

    pub fn display_name(&self) -> String {
        match self {
            Self::MasterRequirement => "Master Requirement".into(),
            Self::Requirements => "Requirements".into(),
            Self::Epic => "Epic".into(),
            Self::Feature => "Feature".into(),
            Self::Story => "Story".into(),
            Self::Task => "Task".into(),
            Self::Plan => "Plan".into(),
            Self::ImplementationPlan => "Implementation Plan".into(),
            Self::Implementation => "Implementation".into(),
            Self::TestCases => "Test Cases".into(),
            Self::TestResults => "Test Results".into(),
            Self::Summary => "Summary".into(),
            Self::Architecture => "Architecture".into(),
            Self::ArchitectureReview => "Architecture Review".into(),
            Self::Bug => "Bug".into(),
            Self::Clarification => "Clarification".into(),
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

    /// `true` when the cascade orchestrator (and the runner's
    /// pipeline gate) should let this artifact serve as a *source*
    /// for a downstream skill run. `Approved` artifacts execute
    /// downstream skills the normal "first time" way; `Dirty`
    /// artifacts trigger a re-execution that preserves the existing
    /// children in place (revision-history append) and marks their
    /// existing descendants Dirty so the wave propagates downward
    /// on the next BFS pop. Everything else (Pending, Rejected,
    /// Running, Error) blocks downstream execution: the user hasn't
    /// approved the source body yet, or it's mid-flight, or it
    /// failed. Shared by `cascade.rs` and `runner.rs` so both gates
    /// agree on the rule.
    pub fn is_runnable_source(&self) -> bool {
        matches!(self, Self::Approved | Self::Dirty)
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
    /// Free-form user feedback that should be inlined into the next
    /// re-run's prompt. Lets the user say "regenerate this Epic but
    /// emphasize X" without polluting the artifact body. Auto-cleared
    /// by the runner after a successful regeneration so old feedback
    /// doesn't replay on subsequent runs.
    ///
    /// Persisted on a single line as `revision_notes: <inline string>`.
    /// Newlines in the user's input are escaped to `\n` on serialize
    /// and restored on parse so multi-line notes round-trip cleanly
    /// without breaking the YAML-ish single-line frontmatter format
    /// the rest of this parser expects.
    pub revision_notes: Option<String>,
    /// Phase E: marker set on Architecture artifacts when a non-first
    /// phase's cascade has produced an `architecture_review` child
    /// that hasn't been Approved or Rejected yet. The explorer + the
    /// workflow canvas render a ⚠ badge on flagged architectures so
    /// the user notices without opening the note; the architecture's
    /// own view shows a banner listing the pending review children
    /// with click-through links. Cleared automatically when the last
    /// Pending / Dirty review is resolved.
    ///
    /// Serialized only when `true` to keep diffs minimal on artifacts
    /// without reviews.
    pub needs_review: bool,
}

impl Default for ArtifactFrontmatter {
    fn default() -> Self {
        Self {
            artifact_kind: None,
            status: ArtifactStatus::Pending,
            source_artifact_id: None,
            source_skill_id: None,
            input_hash: None,
            revision_notes: None,
            needs_review: false,
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
    // Custom parse path — the shared `field()` helper trims and
    // greedy-strips outer `"`/`'` chars, which would eat both our
    // wrapping quotes AND any user-typed escaped `\"` sitting at the
    // edges. We need to remove exactly one wrapping `"` from each side
    // and leave inner escapes alone. Falls back to `field()`'s
    // behaviour for legacy unquoted values written before the wrapping
    // format landed.
    let revision_notes = revision_notes_from_lines(&lines)
        .filter(|s| !s.is_empty());
    let needs_review = field(&lines, "needs_review")
        .map(|s| matches!(s.trim().to_ascii_lowercase().as_str(), "true" | "yes" | "1"))
        .unwrap_or(false);
    ArtifactFrontmatter {
        artifact_kind,
        status,
        source_artifact_id,
        source_skill_id,
        input_hash,
        revision_notes,
        needs_review,
    }
}

/// Pull the `revision_notes` value out of frontmatter lines without
/// going through the shared `field()` helper. We need this because:
///
/// * `field()` greedy-strips outer `"`/`'` chars via `trim_matches`,
///   which also eats the trailing `\"` of an escaped inner quote.
/// * `field()` calls `v.trim()` first, which would eat the trailing
///   space the user just typed in the controlled textarea.
///
/// This parser strips exactly ONE wrapping `"` from each side (when
/// present) and leaves the inner escape sequences alone for
/// `unescape_inline` to decode. For legacy bodies written before the
/// quote-wrapping format landed, it falls back to the old behaviour
/// (trim + decode).
fn revision_notes_from_lines(lines: &[&str]) -> Option<String> {
    for line in lines {
        let body = line.trim_start();
        let Some(rest) = body.strip_prefix("revision_notes:") else {
            continue;
        };
        // YAML idiom: one space after the colon. Strip it if present
        // but don't trim the rest — we need the user's leading
        // whitespace inside the value to survive.
        let rest = rest.strip_prefix(' ').unwrap_or(rest);
        // `lines()` already stripped the line ending, but be defensive
        // about a stray `\r` on CRLF inputs.
        let rest = rest.trim_end_matches('\r');
        // Quoted form: `"...escape_inline(value)..."`. Strip exactly
        // one `"` from each end.
        if rest.starts_with('"') && rest.ends_with('"') && rest.len() >= 2 {
            let inner = &rest[1..rest.len() - 1];
            return Some(unescape_inline(inner));
        }
        // Legacy unquoted form — match the prior behaviour so old
        // notes still parse cleanly.
        let inner = rest.trim();
        if inner.is_empty() {
            return None;
        }
        return Some(unescape_inline(inner));
    }
    None
}

/// Escape a user-typed string into a single-line frontmatter value:
/// newlines become `\n`, backslashes become `\\`, double quotes become
/// `\"`. Mirrors `unescape_inline`'s decoder. Double-quote escaping
/// matters because `rewrite` wraps the serialized value in `"..."` so
/// the custom revision-notes parser can strip exactly one wrapping
/// quote from each side.
fn escape_inline(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '"' => out.push_str("\\\""),
            other => out.push(other),
        }
    }
    out
}

/// Reverse of `escape_inline`: turn `\n` / `\r` / `\\` / `\"` sequences
/// back into the literal characters. Unknown escapes pass through
/// verbatim so user-typed backslashes don't get eaten if the field was
/// hand-edited in raw form.
fn unescape_inline(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.peek() {
                Some('n') => {
                    chars.next();
                    out.push('\n');
                }
                Some('r') => {
                    chars.next();
                    out.push('\r');
                }
                Some('\\') => {
                    chars.next();
                    out.push('\\');
                }
                Some('"') => {
                    chars.next();
                    out.push('"');
                }
                _ => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
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
    if let Some(notes) = next.revision_notes.as_ref() {
        // Don't trim — the controlled textarea sends every keystroke
        // through here, so stripping trailing whitespace would eat the
        // space the user just typed. We do skip serialization for
        // all-whitespace notes (they round-trip as `None` via parse's
        // `is_empty` filter on the unescaped form anyway).
        if !notes.chars().all(char::is_whitespace) {
            // Wrap in double quotes so the shared `field()` parser's
            // `v.trim()` step has whitespace to chew through *outside*
            // the quotes — preserving any leading/trailing whitespace
            // the user actually typed. `escape_inline` escapes `"` so
            // user-typed quotes don't break the wrapping.
            out.push_str(&format!(
                "revision_notes: \"{}\"\n",
                escape_inline(notes)
            ));
        }
    }
    // Phase E: only emit `needs_review` when true. Keeps the diff
    // empty on the vast majority of artifacts that never get flagged.
    if next.needs_review {
        out.push_str("needs_review: true\n");
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
            "revision_notes",
            "needs_review",
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
    fn is_runnable_source_accepts_approved_and_dirty() {
        // The cascade's approval gate + the runner's pipeline gate
        // both consult this. Approved is the normal first-time
        // execution path; Dirty triggers the preserve-and-mark
        // regen path. Anything else blocks downstream execution.
        assert!(ArtifactStatus::Approved.is_runnable_source());
        assert!(ArtifactStatus::Dirty.is_runnable_source());
    }

    #[test]
    fn is_runnable_source_blocks_pending_rejected_running_error() {
        assert!(!ArtifactStatus::Pending.is_runnable_source());
        assert!(!ArtifactStatus::Rejected.is_runnable_source());
        assert!(!ArtifactStatus::Running.is_runnable_source());
        assert!(!ArtifactStatus::Error.is_runnable_source());
    }

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
    fn parse_extracts_implementation_plan_kind() {
        let body =
            "---\nartifact_kind: implementation_plan\nstatus: pending\n---\nbody";
        let fm = parse(body);
        assert_eq!(fm.artifact_kind, Some(ArtifactKind::ImplementationPlan));
        assert_eq!(
            ArtifactKind::ImplementationPlan.as_str(),
            "implementation_plan"
        );
        assert_eq!(
            ArtifactKind::ImplementationPlan.display_name(),
            "Implementation Plan"
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
    fn parse_extracts_master_requirement_kind() {
        let body = "---\nartifact_kind: master_requirement\nstatus: approved\n---\nbody";
        let fm = parse(body);
        assert_eq!(fm.artifact_kind, Some(ArtifactKind::MasterRequirement));
        assert_eq!(ArtifactKind::MasterRequirement.as_str(), "master_requirement");
        assert_eq!(
            ArtifactKind::MasterRequirement.display_name(),
            "Master Requirement"
        );
    }

    #[test]
    fn parse_extracts_architecture_kind() {
        let body = "---\nartifact_kind: architecture\nstatus: pending\n---\nbody";
        let fm = parse(body);
        assert_eq!(fm.artifact_kind, Some(ArtifactKind::Architecture));
        assert_eq!(ArtifactKind::Architecture.as_str(), "architecture");
        assert_eq!(ArtifactKind::Architecture.display_name(), "Architecture");
    }

    #[test]
    fn parse_extracts_architecture_review_kind() {
        let body =
            "---\nartifact_kind: architecture_review\nstatus: pending\n---\nbody";
        let fm = parse(body);
        assert_eq!(fm.artifact_kind, Some(ArtifactKind::ArchitectureReview));
        assert_eq!(
            ArtifactKind::ArchitectureReview.as_str(),
            "architecture_review"
        );
        assert_eq!(
            ArtifactKind::ArchitectureReview.display_name(),
            "Architecture Review"
        );
    }

    #[test]
    fn needs_review_round_trips() {
        let body = "---\nartifact_kind: architecture\nstatus: approved\nneeds_review: true\n---\nbody";
        let fm = parse(body);
        assert!(fm.needs_review);
        // Re-serialize and re-parse to confirm the field survives the
        // round-trip via `rewrite`.
        let rebuilt = rewrite(body, &fm);
        assert!(rebuilt.contains("needs_review: true"));
        let reparsed = parse(&rebuilt);
        assert!(reparsed.needs_review);
    }

    #[test]
    fn needs_review_omitted_when_false() {
        let body = "---\nartifact_kind: architecture\nstatus: approved\n---\nbody";
        let fm = parse(body);
        assert!(!fm.needs_review);
        let rebuilt = rewrite(body, &fm);
        assert!(!rebuilt.contains("needs_review"));
    }

    #[test]
    fn parse_extracts_bug_and_clarification_kinds() {
        let bug = parse("---\nartifact_kind: bug\n---\nbody");
        assert_eq!(bug.artifact_kind, Some(ArtifactKind::Bug));
        assert_eq!(ArtifactKind::Bug.as_str(), "bug");
        let clar = parse("---\nartifact_kind: clarification\n---\nbody");
        assert_eq!(clar.artifact_kind, Some(ArtifactKind::Clarification));
        assert_eq!(ArtifactKind::Clarification.as_str(), "clarification");
        assert_eq!(
            ArtifactKind::Clarification.display_name(),
            "Clarification"
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
    fn parse_handles_double_frontmatter_blocks() {
        // The exact bug we hit live: user pasted the new body on top
        // of an existing `---\nstatus: approved\n---` block, ending up
        // with two consecutive blocks. With the lenient `split`, both
        // blocks are folded so `artifact_kind` is still discoverable.
        // Without this, `read_kind` would have returned None and the
        // cascade would silently process zero skills.
        let body = "---\n\
            status: approved\n\
            ---\n\
            ---\n\
            artifact_kind: requirements\n\
            status: approved\n\
            ---\n\
            \n\
            # Requirements: Pomofocus (web)\n\
            body here";
        let fm = parse(body);
        assert_eq!(fm.artifact_kind, Some(ArtifactKind::Requirements));
        assert_eq!(fm.status, ArtifactStatus::Approved);
    }

    #[test]
    fn rewrite_self_heals_double_frontmatter() {
        // After parsing a double-block body and rewriting (e.g. via
        // approve / re-run path), the output collapses back to a
        // single canonical block. Side effect: any stale double-block
        // body in the wild gets normalized on its next mutation.
        let body = "---\n\
            status: approved\n\
            ---\n\
            ---\n\
            artifact_kind: requirements\n\
            status: approved\n\
            ---\n\
            \n\
            # Body";
        let fm = parse(body);
        let next = rewrite(body, &fm);
        // Exactly one opening + one closing fence at the top.
        assert_eq!(next.matches("---").count(), 2);
        assert!(next.contains("artifact_kind: requirements"));
        assert!(next.contains("# Body"));
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

    #[test]
    fn revision_notes_round_trip_single_line() {
        let mut fm = ArtifactFrontmatter::default();
        fm.artifact_kind = Some(ArtifactKind::Epic);
        fm.revision_notes = Some("Emphasize observability concerns".into());
        let body = rewrite("", &fm);
        assert!(body.contains("revision_notes: \"Emphasize observability concerns\""));
        let parsed = parse(&body);
        assert_eq!(
            parsed.revision_notes.as_deref(),
            Some("Emphasize observability concerns")
        );
    }

    #[test]
    fn revision_notes_round_trip_multi_line() {
        // Newlines have to be escaped so the single-line frontmatter
        // serializer doesn't split the value across multiple keys.
        let mut fm = ArtifactFrontmatter::default();
        fm.artifact_kind = Some(ArtifactKind::Feature);
        fm.revision_notes = Some(
            "Drop the analytics epic.\nAdd an SLO epic instead.".into(),
        );
        let body = rewrite("", &fm);
        // Stored escaped + quoted on disk:
        assert!(body.contains(
            "revision_notes: \"Drop the analytics epic.\\nAdd an SLO epic instead.\""
        ));
        // Decoded back when parsing:
        let parsed = parse(&body);
        assert_eq!(
            parsed.revision_notes.as_deref(),
            Some("Drop the analytics epic.\nAdd an SLO epic instead.")
        );
    }

    #[test]
    fn revision_notes_round_trip_preserves_trailing_space() {
        // Regression: a trailing space mid-typing must survive the
        // round-trip — otherwise the controlled textarea snaps the
        // cursor back and the user can never type a real space.
        let mut fm = ArtifactFrontmatter::default();
        fm.artifact_kind = Some(ArtifactKind::Epic);
        fm.revision_notes = Some("hello ".into());
        let body = rewrite("", &fm);
        let parsed = parse(&body);
        assert_eq!(parsed.revision_notes.as_deref(), Some("hello "));
    }

    #[test]
    fn revision_notes_round_trip_preserves_trailing_newline() {
        // Same regression for newlines: pressing Enter at the end
        // must not be eaten by the serializer.
        let mut fm = ArtifactFrontmatter::default();
        fm.artifact_kind = Some(ArtifactKind::Epic);
        fm.revision_notes = Some("line1\n".into());
        let body = rewrite("", &fm);
        let parsed = parse(&body);
        assert_eq!(parsed.revision_notes.as_deref(), Some("line1\n"));
    }

    #[test]
    fn revision_notes_round_trip_preserves_embedded_double_quotes() {
        // The quote-wrapping serializer escapes inner `"` so user
        // content like `say "hi"` survives the round-trip.
        let mut fm = ArtifactFrontmatter::default();
        fm.artifact_kind = Some(ArtifactKind::Epic);
        fm.revision_notes = Some(r#"say "hi""#.into());
        let body = rewrite("", &fm);
        let parsed = parse(&body);
        assert_eq!(parsed.revision_notes.as_deref(), Some(r#"say "hi""#));
    }

    #[test]
    fn revision_notes_legacy_unquoted_value_still_parses() {
        // Older artifact files on disk used the unquoted form. They
        // must continue to parse cleanly so this format change is
        // backwards-compatible.
        let body = "---\nartifact_kind: epic\nstatus: pending\nrevision_notes: legacy plain value\n---\nbody";
        let parsed = parse(body);
        assert_eq!(parsed.revision_notes.as_deref(), Some("legacy plain value"));
    }

    #[test]
    fn revision_notes_empty_string_is_dropped() {
        // Empty / whitespace-only notes should NOT serialize a blank
        // `revision_notes:` line — that would be visual noise on the
        // common case.
        let mut fm = ArtifactFrontmatter::default();
        fm.artifact_kind = Some(ArtifactKind::Task);
        fm.revision_notes = Some("   ".into());
        let body = rewrite("", &fm);
        assert!(!body.contains("revision_notes"));
    }

    #[test]
    fn revision_notes_absent_in_legacy_bodies() {
        // Existing artifact frontmatter without the field parses
        // cleanly with `revision_notes: None`.
        let body = "---\nartifact_kind: epic\nstatus: approved\n---\nbody";
        let fm = parse(body);
        assert_eq!(fm.revision_notes, None);
    }
}
