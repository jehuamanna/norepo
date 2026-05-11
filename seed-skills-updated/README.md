# Seed skills (updated variant) — master_requirement, multi-level gates, iterative refinement

A redesigned seed-skill chain for full-SDLC work: BA decomposes a
project from a **master_requirement** down through detailed
Requirements → Epics → Features → Stories → Tasks; SA produces a
single, iteratively-revised Architecture; SDE implements, tests, and
patches bugs against that Architecture.

Compared to `seed-skills-employee/`:

- A new **`master_requirement`** artifact sits at the root. It holds
  the high-level charter ("build a portfolio management system") plus
  any CE-team inputs. The **Play button lives only here** — every
  cascade starts from master.
- BA produces **multiple detailed Requirements (A0)** beneath master,
  rather than collapsing everything into one Requirements note.
- Every level transition (A0 → A1 → A2 → A3 → A4) is a **hard gate**
  (`gate: approval`, `cascade_stop: true`) — the user reviews each
  level before the next runs.
- **Format normalizers** (`02n` / `03n` / `04n` / `05n`) restructure
  manually-added Epics / Features / Stories / Tasks into the
  canonical shape so downstream skills can consume them.
- A **coherence-check** skill (`00`) walks the whole tree on every
  Play and emits `clarification` artifacts for cross-level
  contradictions. The user answers in-place (single/multi choice
  with custom-input slots), and the cascade resumes.
- **Revision history is preserved** in every artifact body: each
  re-run appends a new `## Revision N (YYYY-MM-DD)` row and tucks the
  prior body under a collapsed `<details>` block.
- **SA** produces **one** `architecture` artifact, iteratively
  revised in-place from `master_requirement`. No HLD / LLD split.
- **SDE** consumes a Task + inherits the latest Architecture revision
  to produce an Implementation. Tests are generated and executed by
  the SDE persona (not a separate TST persona). Bugs are first-class
  `bug` artifacts; the bug-fix skill emits a new Implementation
  revision.

```
master_requirement   [PROJECT ROOT — the only Play button]
├── requirements × N           [A0, BA-produced from master + CE inputs]
│   └── epic × N                [A1]
│       └── feature × N          [A2]
│           └── story × N         [A3]
│               └── task × N      [A4]
│                   └── implementation
│                       ├── test_cases
│                       │   └── test_results
│                       └── bug × N
│                           └── implementation (revision, via 10-sde-fix-bug)
└── architecture                 [SA, iteratively revised in-place]
```

## Pipeline at a glance

| Order | Skill | Phase | input_kind | output_kind | Count | Gate | Persona |
|---|---|---|---|---|---|---|---|
| 00 | `00-coherence-check` | BA | master_requirement (aggregator over `task`) | clarification | many (0–N) | approval | BA |
| 01 | `01-ba-aggregate-requirements` | BA | master_requirement | requirements | many (3–8) | approval | BA |
| 02 | `02-ba-discover-epics` | BA | master_requirement (aggregator over `requirements`) | epic | many (3–8) | approval | BA |
| 02n | `02n-ba-normalize-epics` | BA | epic | epic | one | auto | BA |
| 03 | `03-ba-decompose-features` | BA | epic | feature | many (3–5) | approval | BA |
| 03n | `03n-ba-normalize-features` | BA | feature | feature | one | auto | BA |
| 04 | `04-ba-decompose-stories` | BA | feature | story | many (2–4) | approval | BA |
| 04n | `04n-ba-normalize-stories` | BA | story | story | one | auto | BA |
| 05 | `05-ba-decompose-tasks` | BA | story | task | many (3–5) | approval | BA |
| 06 | `06-sa-draft-architecture` | BA | master_requirement (aggregator over `requirements`) | architecture | one | approval | SA |
| 05n | `05n-sde-normalize-tasks` | SDE | task | task | one | auto | SDE |
| 07 | `07-sde-implement-task` | SDE | task (inherits `architecture`) | implementation | one | approval | SDE |
| 08 | `08-sde-generate-tests` | SDE | implementation | test_cases | one | approval | SDE |
| 09 | `09-sde-execute-tests` | SDE | test_cases | test_results | one | approval | SDE |
| 10 | `10-sde-fix-bug` | SDE | bug (inherits `architecture`) | implementation | one | approval | SDE |

## Two-phase workflow

The pipeline splits at the Architecture boundary. Which skills run
is determined entirely by where the user clicks Play:

**BA phase — triggered by Play on a `master_requirement` artifact.**
Runs every skill whose `input_kind` is `master_requirement` /
`requirements` / `epic` / `feature` / `story`. The chain produces
requirements → epics → features → stories → tasks, plus the
architecture artifact. The cascade naturally stops once tasks and
architecture exist — no BA-phase skill has `input_kind: task`, so
the orchestrator finds nothing to enqueue against the produced tasks
and ends. At every level transition (A0→A1→A2→A3→A4) `cascade_stop:
true` pauses for human review.

**SDE phase — triggered by Play on a `task` artifact.**
Runs every skill whose `input_kind` is `task` / `implementation` /
`test_cases` / `test_results` / `bug`. Per-task chain:
`task → implementation → test_cases → test_results`, with
`05n-sde-normalize-tasks` running first as a pre-flight reformat
(idempotent for tasks already produced by `05-ba-decompose-tasks`).
For bug-fix work: file a `bug` artifact under the buggy
implementation, click Play on the parent task — `10-sde-fix-bug`
emits a new implementation revision, the dirty cascade regenerates
the downstream test_cases + test_results.

Only `master_requirement` and `task` artifacts show the Play /
Generate Cascade / Run skill toolbar. Every other artifact
(Requirements, Epics, Features, Stories, Architecture,
Implementation, Test Cases, Test Results, Bug, Clarification) keeps
just Approve / Reject / Mark dirty / Revise — the workflow is
fully driven by master Play (BA phase) and per-task Play (SDE
phase).

## Required code-level changes

All eight items below have landed in-tree; the chain runs at full
fidelity.

| # | Status | Change | Files | Notes |
|---|--------|--------|-------|-------|
| 1 | shipped | Add `master_requirement` to `ArtifactKind` enum | `src/plugins/artifact/frontmatter.rs` | New variant + parser tests. |
| 2 | shipped | Add `architecture`, `bug`, `clarification` to `ArtifactKind` | same file | Three new variants. |
| 3 | shipped | Restrict the Play button to `master_requirement` (plus legacy `Requirements` root for backwards compat) | `src/plugins/artifact/view.rs` (`is_cascade_root` gate around `CascadePlayButton`) | Revise / Approve / Reject / Mark-dirty remain available on every kind. |
| 4 | shipped | New `ClarificationPanel` UI component | `src/shell/clarification_prompt.rs` (new file) + `view.rs` wiring | Single-choice radios + multi-choice checkboxes + per-option `Other: ___` text field. On submit: appends `## Answer (YYYY-MM-DD)` block, flips status Approved, writes resolved direction into each `## Resolution target`'s `revision_notes` and marks them Dirty for the next-Play regen. |
| 5 | shipped | Cascade halts on unresolved `clarification` artifacts | `src/plugins/artifact/cascade.rs` (`unresolved_clarification_titles` + step-0 gate in `run_cascade`) | Any `Pending` clarification in the project blocks the cascade with a `SkillRun` error listing the unanswered ones. |
| 6 | shipped | Runner support for in-place artifact rewrite (normalizer) | `src/plugins/artifact/runner.rs` (`is_normalizer_contract` + `import_normalizer_rewrite`) | When `output_kind == input_kind` + `output_count: one`, overwrites source body in place instead of creating a child. Preserves the source's existing parent linkage. |
| 7 | shipped | Revision-history-aware regen prompt | `src/plugins/artifact/runner.rs` (`build_prompt` `previous_outputs` param) + `cascade.rs` capture-before-wipe | Prior `(title, body)` pairs of dirty children are inlined under `--- previous revisions to preserve ---` so Claude can append `## Revision N` blocks rather than discard history. |
| 8 | shipped | BA / SA / SDE personas registered | `crates/operon-core/src/persona.rs` (three new builtins) + `agent_persona:` in each seed skill | BA is Plan-mode with read-only tools (read/glob/grep/lsp/web_*). SA same shape. SDE is Build-mode with full edit/shell/git access. Once the new agent runtime starts consuming `SkillAgentConfig`, each skill auto-picks its persona without further changes. |

## How to install

These files are templates, not auto-imported. For each `.md`:

1. Open Operon, navigate to the project's note tree.
2. Right-click the project → **Add note ▶** → **Skill**.
3. **Title the note exactly after the file's basename** — including
   the numeric prefix (e.g. `02-ba-discover-epics`). The numeric
   prefix wires the skill into the right slot in the cascade
   workflow's auto-seeded pipeline.
4. Paste the file's full contents (frontmatter + body).
5. Save.

Skill notes show up in the explorer with an `[sk]` badge.

Don't mix-and-match across variants in the same project — pick one
variant's chain per project so the cascade's auto-seeder doesn't see
two skills for the same `input_kind` slot.

## Where to put the master_requirement seed

The pipeline's first artifact is a `master_requirement` note. To
create one:

1. Right-click the project → **Add note ▶** → **Artifact**.
2. Title it `Master Requirement` (or anything — the title doesn't
   matter, only the frontmatter does).
3. Paste this header at the top, then your high-level charter and any
   CE-team inputs below:

   ```markdown
   ---
   artifact_kind: master_requirement
   status: approved
   ---

   # Master Requirement: <project name>

   ## Charter

   <your high-level prose: what the product is, who it's for, what
   problem it solves...>

   ## Inputs from CE

   <pasted CE-team notes, stakeholder transcripts, RFP excerpts,
   integration constraints, anything material to scope...>
   ```

   (`status: approved` lets `01-ba-aggregate-requirements` run
   immediately on Play.)

4. Save. The artifact view's **Play** button is now enabled (once
   change #3 above lands; until then, the Play button shows on any
   artifact and you can run it from the master manually).

## Per-tier dep ordering (engine-enforced)

Every BA artifact (Requirement, Epic, Feature, Story, Task) carries a
`## Depends on` section with sibling slugs (or
`None (parallel-safe)`). The cascade engine reads these and sequences
work topologically. Same engine semantics as `seed-skills-employee/`:

- Before running any skill on artifact A, the engine checks A's
  declared deps. If any dep hasn't been processed (or wasn't already
  Approved before the cascade started), A defers until they are.
- **Cycles or unresolvable deps deadlock the cascade**, which then
  surfaces a `Failed` outcome with the stuck items named.
- **Re-runs**: every artifact already marked `Approved` counts as
  "done" for dep-gate purposes; a re-run picks up where it left off
  rather than re-blocking on finished work.

## Revision history convention

Every artifact in this directory writes a `## Revision history`
table:

```markdown
## Revision history

| Revision | Date         | Derived from               | Summary                          |
|----------|--------------|----------------------------|----------------------------------|
| 3        | 2026-05-11   | bug-02-zero-day-payouts    | Fixed off-by-one in pay period.  |
| 2        | 2026-05-09   | story-03-…                 | Added pagination per UX review.  |
| 1        | 2026-05-07   | story-03-…                 | Initial draft.                   |
```

When a skill re-runs against an existing artifact, it appends a new
row, generates the new body **above** the old content, and stashes
the prior body inside a `<details><summary>Revision N (date)</summary>`
block at the bottom. The audit trail is what makes
`00-coherence-check` and human reviewers able to trace how the
project's understanding evolved.

This convention works today **without code changes** — every skill in
this directory instructs Claude to do the append-and-stash dance. The
runner change #7 above makes it cleaner by inlining the prior body
automatically; without it, Claude regenerates from the current body
only (the history rows stay, but content collapsed into
`<details>` blocks gets re-emitted verbatim from what's already in
the body).

## Tuning

Skill bodies are prompts. Adjust calibration sections (Requirement
count, Epic count, Feature count, Story count, Task count) to match
your team's norms. Each re-run reads the current skill body — no
rebuild needed.

If you want lighter gating (e.g. auto-approve through A0 → A1, gate
only at A2), flip `gate: approval` to `gate: auto` and
`cascade_stop: true` to `cascade_stop: false` on the skills you want
to wave through. The chain still runs end-to-end.
