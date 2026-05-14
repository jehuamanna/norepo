//! SDLC role bucket inferred from an artifact's `artifact_kind` (for
//! artifact notes) or from a skill's leading numeric prefix (for skill
//! notes). The explorer renders each row with a 3px left accent bar
//! colored by role so users can scan the tree and see which role owns
//! a stage at a glance.
//!
//! Mapping decisions live here (not in `frontmatter.rs`) because they
//! are a UI policy — they could change without affecting the underlying
//! artifact schema.

use crate::plugins::artifact::frontmatter::{ArtifactKind, ArtifactStatus};

/// Cached snapshot of an Artifact note's frontmatter that the explorer
/// needs for its decorations. Built once per `notes_by_project` change
/// by sync-loading + parsing each Artifact body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactMeta {
    pub kind: Option<ArtifactKind>,
    pub status: ArtifactStatus,
    /// Phase E: `true` when this artifact has the `needs_review` flag
    /// set in its frontmatter. The explorer row uses this to render a
    /// ⚠ next to the status dot. Only meaningful for Architecture
    /// artifacts today; cheap enough to thread through for all kinds
    /// in case future skills set the flag elsewhere.
    pub needs_review: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Business analyst — requirements/epics/features/stories/etc.
    Ba,
    /// Solution architect — architecture/plan.
    Sa,
    /// Software development engineer — tasks/impl/bugs/tests.
    Sde,
}

impl Role {
    /// CSS class fragment used on `.notes-explorer-row` to drive the
    /// role accent color. Matches selectors in `shell.css`.
    pub fn css_class(&self) -> &'static str {
        match self {
            Role::Ba => "operon-note-role-ba",
            Role::Sa => "operon-note-role-sa",
            Role::Sde => "operon-note-role-sde",
        }
    }
}

/// Map an artifact's `artifact_kind` to the SDLC role that owns it.
/// `None` means "not a typed SDLC artifact" — render with no accent.
pub fn role_for_artifact_kind(kind: &ArtifactKind) -> Option<Role> {
    match kind {
        ArtifactKind::MasterRequirement
        | ArtifactKind::Requirements
        | ArtifactKind::Epic
        | ArtifactKind::Feature
        | ArtifactKind::Story
        | ArtifactKind::Summary
        | ArtifactKind::Clarification
        | ArtifactKind::PrioritizedBacklog => Some(Role::Ba),
        ArtifactKind::Architecture
        | ArtifactKind::ArchitectureReview
        | ArtifactKind::Plan => Some(Role::Sa),
        ArtifactKind::Task
        | ArtifactKind::Implementation
        | ArtifactKind::ImplementationPlan
        | ArtifactKind::Bug
        | ArtifactKind::TestCases
        | ArtifactKind::TestResults => Some(Role::Sde),
        ArtifactKind::Other(s) => heuristic_other(s),
    }
}

/// Best-guess role for a user-defined `Other(s)` artifact kind. Match
/// substrings ordered SDE → SA → BA so the more specific
/// implementation keywords win over the broader ones. Returns `None`
/// for truly unrecognized kinds — the row renders uncolored.
fn heuristic_other(s: &str) -> Option<Role> {
    let lower = s.to_ascii_lowercase();
    if ["test", "bug", "impl", "task"]
        .iter()
        .any(|k| lower.contains(k))
    {
        return Some(Role::Sde);
    }
    if ["arch", "plan", "design"].iter().any(|k| lower.contains(k)) {
        return Some(Role::Sa);
    }
    if [
        "req", "spec", "epic", "feat", "story", "backlog", "summary", "clarif",
    ]
    .iter()
    .any(|k| lower.contains(k))
    {
        return Some(Role::Ba);
    }
    None
}

/// Infer a role from a skill note's title. Convention: titles start
/// with a numeric prefix (`01-`, `07-…`); 01-05 → BA, 06-08 → SA,
/// 09+ → SDE. Skills without a numeric prefix get no role color.
pub fn role_for_skill_title(title: &str) -> Option<Role> {
    let digits: String = title.chars().take_while(|c| c.is_ascii_digit()).collect();
    let n: u32 = digits.parse().ok()?;
    match n {
        1..=5 => Some(Role::Ba),
        6..=8 => Some(Role::Sa),
        _ => Some(Role::Sde),
    }
}
