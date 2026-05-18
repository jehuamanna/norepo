//! "Creatable kind" — the unified pick the explorer offers when the user
//! adds a new note. Wraps both:
//!
//! - **Plain note kinds** (`NoteKind::Markdown`, `Mdx`, `Code`, `Skill`,
//!   `Workflow`, `Image`, …): create an empty note of that file-level
//!   kind. Same flow that existed before this module landed.
//! - **Typed artifacts** (`ArtifactKind::Epic`, `Story`, `Task`, …):
//!   create a `NoteKind::Artifact` *and* seed its body with frontmatter
//!   (`artifact_kind: <kind>`) plus the `##` section headers the
//!   corresponding cascade skill produces, so the manually-created note
//!   slots directly into the pipeline.
//!
//! The single source of truth for the menu shown by the right-click
//! "Add child / sibling note" entries (`note_row.rs:313-341`), the
//! per-row hover "+" dropdown (`note_row.rs:1031-1044`), and the
//! project-level "+" dropdown (`project_row.rs:759`) is
//! [`build_creatable_menu`]. Editing the shape here lights up everywhere.

#![cfg(not(target_arch = "wasm32"))]

use dioxus::prelude::*;
use operon_store::repos::NoteKind;

use crate::local_mode::ui::context_menu::ContextMenuItem;
use crate::plugins::artifact::frontmatter::ArtifactKind;
use crate::plugins::artifact::view::current_iso_date;

/// What the user picked from a creation menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreatableKind {
    /// A plain note of the given file-level kind. Body starts empty;
    /// the explorer triggers inline rename so the user names it first.
    Plain(NoteKind),
    /// A `NoteKind::Artifact` note seeded with the matching scaffold
    /// body (frontmatter + section headers). The caller is responsible
    /// for writing the scaffold to disk via `Persistence::save` after
    /// `create_with_kind` returns.
    Artifact(ArtifactKind),
}

impl CreatableKind {
    /// The cascade pipeline kinds in the order the user listed them.
    /// Drives the `Artifact ▶` submenu, the scaffold tests, and the
    /// integration roundtrip. Anything outside this list (`Plan`,
    /// `Summary`, `Bug`, `Clarification`, `PrioritizedBacklog`,
    /// `Other(_)`) is deliberately not exposed in the menu — those are
    /// cascade-internal kinds or open-ended escape hatches.
    pub fn pipeline_artifacts() -> &'static [ArtifactKind] {
        // Can't be `const` because `ArtifactKind` carries an `Other(String)`
        // variant whose presence forces non-const. Static lifetime is
        // fine — these variants don't allocate.
        static KINDS: &[ArtifactKind] = &[
            ArtifactKind::MasterRequirement,
            ArtifactKind::Requirements,
            ArtifactKind::Epic,
            ArtifactKind::Story,
            ArtifactKind::Task,
            ArtifactKind::Architecture,
            ArtifactKind::ImplementationPlan,
            ArtifactKind::Implementation,
            ArtifactKind::TestCases,
            ArtifactKind::TestResults,
        ];
        KINDS
    }

    pub fn display_name(&self) -> String {
        match self {
            Self::Plain(k) => k.display_name().to_string(),
            Self::Artifact(k) => k.display_name(),
        }
    }
}

/// Build the body of a freshly-created artifact note. Output begins with
/// the YAML frontmatter delimiter so the cascade's `ArtifactKind::parse`
/// (in `src/plugins/artifact/frontmatter.rs`) round-trips it back to the
/// same `kind`. Section headers mirror the matching cascade skill's
/// "Required body sections" block in `seed-skills-updated/0{2..9}-*.md`.
///
/// The `## Revision history` table is seeded with row 1 dated `<today>`
/// (`current_iso_date`) and `Derived from = manual` so re-runs of
/// downstream skills can extend it without losing provenance.
pub fn scaffold_body(kind: &ArtifactKind) -> String {
    let today = current_iso_date();
    let history = format!(
        "| Revision | Date       | Derived from | Summary                              |\n\
         |----------|------------|--------------|--------------------------------------|\n\
         | 1        | {today}    | manual       | Manually created via Operon explorer. |\n"
    );
    let frontmatter = format!("---\nartifact_kind: {}\n---\n\n", kind.as_str());

    let sections = match kind {
        ArtifactKind::MasterRequirement => {
            "# Master Requirement: \n\n\
             ## Outcome\n\n\
             ## Constraints\n\n\
             ## Stakeholders\n\n\
             ## Success criteria\n\n"
                .to_string()
        }
        ArtifactKind::Requirements => {
            "# Requirement: \n\n\
             ## Description\n\n\
             ## Stakeholders\n\n\
             ## Acceptance criteria\n\n"
                .to_string()
        }
        ArtifactKind::Epic => {
            "# Epic: \n\n\
             ## Outcome\n\n\
             ## Why now\n\n\
             ## Satisfies Requirements\n\n\
             ## Scope\n\n\
             ## Out of scope\n\n\
             ## Success metric\n\n\
             ## Risks\n\n\
             ## Depends on\nNone (parallel-safe)\n\n"
                .to_string()
        }
        ArtifactKind::Story => {
            "# Story: \n\n\
             ## Parent Epic\n\n\
             ## Narrative\nAs a … I want … so that …\n\n\
             ## Acceptance criteria\n\n\
             ## UX notes\n\n\
             ## Edge cases\n\n\
             ## Definition of done\n- Tests pass\n- Approved by reviewer\n\n\
             ## Depends on\nNone (parallel-safe)\n\n"
                .to_string()
        }
        ArtifactKind::Task => {
            "# Task: \n\n\
             ## Parent Story\n\n\
             ## What changes\n\n\
             ## Why\n\n\
             ## Depends on\nNone (parallel-safe)\n\n\
             ## Acceptance check\n\n\
             ## Estimated size\n\n"
                .to_string()
        }
        ArtifactKind::Architecture => {
            // Indented `flowchart` is intentional — Markdown allows
            // it inside a fenced block. Kept short so the user sees
            // *a* diagram on open and edits from there.
            "# Architecture: \n\n\
             ## Context\n\n\
             ## Goals & non-goals\n\n\
             ## Constraints\n\n\
             ## Stakeholder views\n\n\
             ## High-level component map\n\n\
             ## Architecture diagram\n\
             ```mermaid\n\
             flowchart LR\n  \
             A[Component] --> B[Component]\n\
             ```\n\n\
             ## Data model\n\n\
             ## Public contracts\n\n\
             ## Tech stack choices\n\n\
             ## Cross-cutting concerns\n\n\
             ## Risks & mitigations\n\n\
             ## Rollout strategy\n\n\
             ## Open questions\n\n"
                .to_string()
        }
        ArtifactKind::ImplementationPlan => {
            "# Implementation Plan: \n\n\
             ## Parent Task\n\n\
             ## Inherited from Architecture\n\n\
             ## Approach\n\n\
             ## Files to change\n\n\
             ## Test cues\n\n\
             ## Risks\nNone\n\n\
             ## Open questions\nNone\n\n"
                .to_string()
        }
        ArtifactKind::Implementation => {
            "# Implementation: \n\n\
             ## Parent Plan\n\n\
             ## Inherited from Architecture\n\n\
             ## What I changed\n\n\
             ## Commit\n\n\
             ## Test cues\n\n\
             ## Follow-ups\nNone\n\n\
             ## Open questions\nNone\n\n"
                .to_string()
        }
        ArtifactKind::TestCases => {
            "# Test Cases: \n\n\
             ## Parent Task\n\n\
             ## Parent Implementation\n\n\
             ## Test framework\n\n\
             ## Test files\n\n\
             ## Test code\n\n\
             ## How to run\n\n\
             ## Coverage notes\n\n"
                .to_string()
        }
        ArtifactKind::TestResults => {
            "# Test Results: \n\n\
             ## Parent Test Cases\n\n\
             ## Command\n\n\
             ## Outcome\n\n\
             ## Failing tests\nNone\n\n\
             ## Raw output (truncated)\n\n\
             ## Verdict\n\n"
                .to_string()
        }
        // The non-pipeline variants — `Plan`, `Summary`, `Bug`,
        // `Clarification`, `PrioritizedBacklog`, `Other(_)` — aren't
        // surfaced in the menu (see `pipeline_artifacts`). If a caller
        // ever asks for one anyway, fall back to a minimal header +
        // revision history so the note still has typed frontmatter.
        _ => format!("# {}: \n\n", kind.display_name()),
    };

    format!("{frontmatter}{sections}## Revision history\n{history}")
}

/// Pure-data description of one menu entry. Used by
/// [`creatable_menu_layout`] (which `build_creatable_menu` consumes to
/// wire `Callback`s onto). Split out so unit tests can assert the
/// menu's shape without needing a Dioxus runtime — `Callback::new`
/// panics outside one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuNode {
    /// A leaf that creates a plain note of this kind.
    Plain(NoteKind),
    /// The "Artifact" anchor with `pipeline_artifacts()` as its children.
    /// All kinds always present — there's no per-context filtering.
    ArtifactSubmenu,
}

/// Pure structural layout of the menu, in display order. Top-level is
/// `NoteKind::all_creatable()` with `Artifact` replaced by an
/// `ArtifactSubmenu` anchor in the same position. Callers wrap this
/// in `Callback`s via [`build_creatable_menu`] at component render
/// time; tests assert against this directly.
pub fn creatable_menu_layout() -> Vec<MenuNode> {
    NoteKind::all_creatable()
        .iter()
        .map(|k| {
            if matches!(k, NoteKind::Artifact) {
                MenuNode::ArtifactSubmenu
            } else {
                MenuNode::Plain(*k)
            }
        })
        .collect()
}

/// Build the menu shown by "Add child / sibling note" submenus and the
/// hover "+" / project "+" dropdowns. Top-level entries are the plain
/// note kinds from `NoteKind::all_creatable()` *with `Artifact` replaced
/// by* a nested "Artifact" submenu listing the pipeline kinds.
///
/// `on_pick` is invoked with whichever leaf the user clicks.
pub fn build_creatable_menu(on_pick: Callback<CreatableKind>) -> Vec<ContextMenuItem> {
    creatable_menu_layout()
        .into_iter()
        .map(|node| match node {
            MenuNode::Plain(kind) => ContextMenuItem::new(
                kind.display_name(),
                Callback::new(move |_| {
                    on_pick.call(CreatableKind::Plain(kind));
                }),
            ),
            MenuNode::ArtifactSubmenu => {
                let children: Vec<ContextMenuItem> = CreatableKind::pipeline_artifacts()
                    .iter()
                    .cloned()
                    .map(|akind| {
                        let display = akind.display_name();
                        ContextMenuItem::new(
                            display,
                            Callback::new(move |_| {
                                on_pick.call(CreatableKind::Artifact(akind.clone()));
                            }),
                        )
                    })
                    .collect();
                ContextMenuItem::submenu("Artifact", children)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_artifacts_order_matches_user_request() {
        // Cascade pipeline kinds in their SDLC order. Locking it
        // down with a test so future edits to `ArtifactKind`'s
        // variant list don't silently reorder the menu (the
        // cascade-pipeline order is a UX contract, not just an
        // enum derivation). `implementation_plan` was added when
        // skill 07 was split into 07a (plan) + 07b (execute).
        let got: Vec<&'static str> = CreatableKind::pipeline_artifacts()
            .iter()
            .map(|k| k.as_str())
            .collect();
        assert_eq!(
            got,
            vec![
                "master_requirement",
                "requirements",
                "epic",
                "story",
                "task",
                "architecture",
                "implementation_plan",
                "implementation",
                "test_cases",
                "test_results",
            ]
        );
    }

    #[test]
    fn scaffold_body_starts_with_frontmatter_for_each_pipeline_kind() {
        for kind in CreatableKind::pipeline_artifacts() {
            let body = scaffold_body(kind);
            let expected_prefix = format!("---\nartifact_kind: {}\n---", kind.as_str());
            assert!(
                body.starts_with(&expected_prefix),
                "scaffold for {:?} should start with frontmatter; got: {:.80}…",
                kind,
                body
            );
            assert!(
                body.contains("\n## "),
                "scaffold for {:?} should contain at least one `## ` section header",
                kind
            );
            assert!(
                body.contains("## Revision history"),
                "scaffold for {:?} should include a Revision history section",
                kind
            );
        }
    }

    #[test]
    fn scaffold_body_round_trips_through_artifact_kind_parser() {
        for kind in CreatableKind::pipeline_artifacts() {
            let body = scaffold_body(kind);
            // Extract the frontmatter block (between the two `---` lines)
            // and parse `artifact_kind:` back. Same parsing the cascade
            // engine uses at `src/plugins/artifact/frontmatter.rs`.
            let after_first = body
                .strip_prefix("---\n")
                .expect("scaffold begins with frontmatter delimiter");
            let (fm, _rest) = after_first
                .split_once("\n---\n")
                .expect("scaffold has closing frontmatter delimiter");
            let value = fm
                .lines()
                .find_map(|line| line.strip_prefix("artifact_kind: "))
                .expect("frontmatter has artifact_kind key");
            let parsed = ArtifactKind::parse(value);
            assert_eq!(
                parsed.as_str(),
                kind.as_str(),
                "round-trip mismatch for {:?}: parsed back as {:?}",
                kind,
                parsed
            );
        }
    }

    #[test]
    fn scaffold_body_dates_revision_row_with_today() {
        let body = scaffold_body(&ArtifactKind::Epic);
        let today = current_iso_date();
        assert!(
            body.contains(&today),
            "scaffold should stamp today's date ({today}) into the revision history"
        );
        let parsed = crate::plugins::artifact::revision_table::parse_revision_table(&body)
            .expect("scaffold body contains a parseable revision table");
        assert_eq!(parsed.rows.len(), 1);
        assert_eq!(
            parsed.rows[0].derived_from, "manual",
            "scaffold's revision row should mark `Derived from = manual`"
        );
    }

    // NB: `build_creatable_menu` itself can't be unit-tested because
    // `Callback::new` panics outside a Dioxus runtime (same constraint
    // documented in `ui/context_menu.rs:367-369`). Instead, tests below
    // assert against `creatable_menu_layout` — the pure-data
    // intermediate that `build_creatable_menu` consumes. The
    // callback-wiring layer is exercised by integration tests at the
    // `explorer/mod.rs` level (Slice 4 of the typed-artifact plan).

    #[test]
    fn menu_layout_replaces_bare_artifact_leaf_with_submenu_anchor() {
        let layout = creatable_menu_layout();
        // No bare `MenuNode::Plain(Artifact)` — the typed submenu
        // displaces it.
        for node in &layout {
            if let MenuNode::Plain(k) = node {
                assert_ne!(
                    *k,
                    NoteKind::Artifact,
                    "Artifact should appear as an ArtifactSubmenu, not a Plain leaf"
                );
            }
        }
        // Exactly one `ArtifactSubmenu` anchor in the layout.
        let submenu_count = layout
            .iter()
            .filter(|n| matches!(n, MenuNode::ArtifactSubmenu))
            .count();
        assert_eq!(submenu_count, 1, "expected one ArtifactSubmenu anchor");
    }

    #[test]
    fn menu_layout_preserves_position_of_artifact_in_all_creatable() {
        // The submenu anchor must sit at the same index where
        // `NoteKind::Artifact` currently lives in
        // `NoteKind::all_creatable()` — otherwise users who relied on
        // the previous ordering would see the menu jump around.
        let creatable_position = NoteKind::all_creatable()
            .iter()
            .position(|k| matches!(k, NoteKind::Artifact))
            .expect("Artifact must be in all_creatable for the menu to surface it");
        let layout = creatable_menu_layout();
        assert!(
            matches!(layout[creatable_position], MenuNode::ArtifactSubmenu),
            "ArtifactSubmenu should sit at the same index where NoteKind::Artifact \
             lives in NoteKind::all_creatable() (expected index {creatable_position})"
        );
    }

    #[test]
    fn menu_layout_preserves_plain_kinds_other_than_artifact() {
        let layout = creatable_menu_layout();
        let plain_kinds: Vec<NoteKind> = layout
            .iter()
            .filter_map(|n| match n {
                MenuNode::Plain(k) => Some(*k),
                MenuNode::ArtifactSubmenu => None,
            })
            .collect();
        // Every plain kind from `NoteKind::all_creatable()` except
        // `Artifact` should appear in the layout.
        for k in NoteKind::all_creatable() {
            if matches!(k, NoteKind::Artifact) {
                continue;
            }
            assert!(
                plain_kinds.contains(k),
                "plain kind {:?} should appear in the menu layout",
                k
            );
        }
    }
}
