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
- **Inline clarifications** — any BA/SA/SDE skill from `02` through
  `09` may also pause itself mid-run when its inputs are too
  ambiguous to best-guess, by emitting a
  `clarification-NN-<topic>.mdx` file (note the `.mdx` extension —
  the visual signal "raised mid-skill") under its source artifact.
  The cascade hard-halts on Pending clarifications; the user answers
  via the same `ClarificationPanel`; the next Play re-runs the
  producing skill with the answer inlined under
  `--- refinement notes from user ---`. Same `artifact_kind:
  clarification`, same answer flow as `00`'s output — just raised by
  the decomposition / planning / test skill itself when it can't
  proceed responsibly.
- **Revision history is preserved** in every artifact body: each
  re-run appends a new `## Revision N (YYYY-MM-DD)` row and tucks the
  prior body under a collapsed `<details>` block.
- **SA** produces **one** `architecture` artifact, iteratively
  revised in-place from `master_requirement`. No HLD / LLD split.
- **SDE** consumes a Task in **two stages**: first `07a-sde-plan-task`
  writes an Implementation Plan (no code), the user reviews it, then
  `07b-sde-execute-implementation` does the actual code edits + commit.
  Both stages inherit the latest Architecture revision. Tests are
  generated and executed by the SDE persona (not a separate TST
  persona). Bugs are first-class `bug` artifacts; the bug-fix skill
  emits a new Implementation revision.

```
master_requirement   [PROJECT ROOT — the only Play button]
├── requirements × N           [A0, BA-produced from master + CE inputs]
│   └── epic × N                [A1]
│       └── feature × N          [A2]
│           └── story × N         [A3]
│               └── task × N      [A4]
│                   └── implementation_plan         [07a, plan only — no code]
│                       └── implementation         [07b, code edits + commit]
│                           ├── test_cases
│                           │   └── test_results
│                           └── bug × N
│                               └── implementation (revision, via 10-sde-fix-bug)
└── architecture                 [SA, iteratively revised in-place]
```

## Pipeline at a glance

| Order | Skill | Phase | input_kind | output_kind | Count | Gate | Persona |
|---|---|---|---|---|---|---|---|
| 00 | `00-coherence-check` | BA | master_requirement (aggregator over `task`) | clarification | many (0–N) | approval | BA |
| 02 | `02-ba-discover-epics` | BA | master_requirement (aggregator over `requirements`) | epic *or* clarification¹ | many (1–3) | approval | BA |
| 02n | `02n-ba-normalize-epics` | BA | epic | epic | one | auto | BA |
| 03 | `03-ba-decompose-features` | BA | epic | feature *or* clarification¹ | many (1–2) | approval | BA |
| 03n | `03n-ba-normalize-features` | BA | feature | feature | one | auto | BA |
| 04 | `04-ba-decompose-stories` | BA | feature | story *or* clarification¹ | many (1–3) | approval | BA |
| 04n | `04n-ba-normalize-stories` | BA | story | story | one | auto | BA |
| 05 | `05-ba-decompose-tasks` | BA | story | task *or* clarification¹ | many (1–3) | approval | BA |
| 06 | `06-sa-draft-architecture` | BA | master_requirement (aggregator over `requirements`) | architecture *or* clarification¹ | one | approval | SA |
| 05n | `05n-sde-normalize-tasks` | SDE | task | task | one | auto | SDE |
| 07a | `07a-sde-plan-task` | SDE | task (inherits `architecture`) | implementation_plan *or* clarification¹ | one | approval | SDE |
| 07b | `07b-sde-execute-implementation` | SDE | implementation_plan (inherits `architecture`) | implementation *or* clarification¹ | one | approval | SDE |
| 08 | `08-sde-generate-tests` | SDE | implementation | test_cases *or* clarification¹ | one | approval | SDE |
| 09 | `09-sde-execute-tests` | SDE | test_cases | test_results *or* clarification¹ | one | approval | SDE |
| 10 | `10-sde-fix-bug` | SDE | bug (inherits `architecture`) | implementation | one | approval | SDE |

¹ When inputs are ambiguous (per each skill's `## Raising
clarifications` rubric), the skill emits `clarification-NN-*.mdx`
files instead of its primary output, hard-halting the cascade. The
user answers via the `ClarificationPanel`; the next Play re-runs the
skill with the answer inlined under
`--- refinement notes from user ---`. See the per-skill bodies for
specific triggers.

## Two-phase workflow

The pipeline splits at the Architecture boundary. Which skills run
is determined entirely by where the user clicks Play.

**Authoring step (before BA phase Play).** The BA hand-creates each
detailed requirement under the project's `master_requirement` — there
is no AI step that produces `requirements` artifacts. Workflow:

1. Right-click the `master_requirement` in the explorer.
2. **Add child note** → **Markdown**.
3. Open the new note and set its frontmatter:
   ```markdown
   ---
   artifact_kind: requirements
   status: approved
   ---
   ```
4. Author the body (sections like `## Capability`, `## Acceptance
   criteria`, `## Constraints`, `## Stakeholders`, `## Out of scope`,
   `## Depends on`, `## Revision history` — see existing skills for
   the canonical shape).
5. Repeat for every coherent capability the project needs.
6. Then click Play on `master_requirement` to start the BA cascade.

If the BA clicks Play before authoring any requirements, the cascade
halts immediately with: *"No `requirements` artifacts exist under
this master_requirement…"* The error surfaces in the cascade-status
row of the master_requirement view; fix by following step 1–5 above
and clicking Play again.

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
Runs every skill whose `input_kind` is `task` / `implementation_plan`
/ `implementation` / `test_cases` / `test_results` / `bug`. Per-task
chain:
`task → implementation_plan → implementation → test_cases → test_results`,
with `05n-sde-normalize-tasks` running first as a pre-flight reformat
(idempotent for tasks already produced by `05-ba-decompose-tasks`).

**Task Play stops at the plan.** When you click Play on a task, the
runtime fires `07a-sde-plan-task` and **stops** — it writes an
Implementation Plan note (files to change, approach, test cues) but
does NOT touch source code, does NOT run tests, does NOT make a
commit. The user reviews the plan, then clicks Play on the
Implementation Plan to execute it (see below).

**Implementation Plan Play executes + tests.** When you click Play
on an Implementation Plan (Approved or Dirty), the runtime fires:
1. `07b-sde-execute-implementation` — does the actual code edits,
   runs the project's tests as a sanity check, makes one commit,
   and writes an Implementation note.
2. `08-sde-generate-tests` — generates TestCases against the
   freshly-implemented code.
3. `09-sde-execute-tests` — runs those TestCases and writes a
   TestResults note.

Dirty plans (the user edited the plan body, or added an inline
`## Bug` section) trigger the cascade's revision-history machinery
automatically — `07b` sees the prior Implementation body inlined
and appends a new `## Revision N` row.

**Implementation Play regenerates + reruns tests.** When you click
Play on the executed Implementation note (Approved or Dirty), the
runtime fires only `08-sde-generate-tests` + `09-sde-execute-tests`:
regenerate TestCases against the current Implementation body, then
run them. Code work is NOT repeated — to re-execute the code, go
back to the Implementation Plan and Play there.

When an Implementation is Dirty a second button — **"Create test
cases"** — appears next to Play. Clicking it runs `08` only,
regenerating TestCases without running them. Use this when you want
to review the regenerated test bodies before they fire.

**Inline `## Bug` flow.** To request a bug fix, paste a `## Bug`
section at the top of the Implementation Plan's body describing
what's broken. The auto-mark-Dirty save path flips the plan to
Dirty, and pressing Play on the plan re-runs `07b` (which treats
the `## Bug` section as the primary change driver) + `08` + `09`.
The legacy standalone `bug` artifact path (`10-sde-fix-bug`) still
works for back-compat but is deprecated.

Only `master_requirement` and `task` artifacts show the full Play /
Generate Cascade / Run skill toolbar. **Implementation Plan** and
**Implementation** each show just a Play button (Implementation
also surfaces the conditional "Create test cases" button on Dirty).
Every other artifact (Requirements, Epics, Features, Stories,
Architecture, Test Cases, Test Results, Bug, Clarification) keeps
just Approve / Reject / Mark dirty / Revise.

## Design pickup (Figma)

Users can paste Figma URLs (host `figma.com` or `www.figma.com`)
into any artifact in the BA chain — the master_requirement, any
detailed Requirement, an Epic, a Feature, a Story, or a Task.
On the next cascade step, the downstream skill scans its parent
body for those URLs, calls the Figma MCP "get figma data" tool
(name: `mcp__<server>__get_figma_data` — `<server>` depends on
the user's config, commonly `figma`, `figma-mcp`, or
`figma-developer-mcp`), and folds the returned frame inventory /
text into how it decomposes:

- design boundaries → Epic boundaries (in `02-ba-discover-epics`)
- screens / flows → Story boundaries (in `04-ba-decompose-stories`)
- specific frames / components → Task scope (in
  `05-ba-decompose-tasks`)

Each output artifact carries a `## Design references` section
listing the Figma URLs relevant to it with one-line per-URL notes.

**Setup.** The Figma MCP server must be configured in the user's
Claude Code MCP config (e.g. `claude mcp add figma -- npx -y
figma-developer-mcp --stdio`) so a `mcp__<server>__get_figma_data`
tool appears in the model's tool list. The skill looks up the tool
by suffix (`get_figma_data`) and works regardless of whether the
user registered the server as `figma`, `figma-mcp`, or
`figma-developer-mcp`. See <https://github.com/GLips/Figma-Context-MCP>
(or your org's pinned fork) for setup.

**Failure handling.** If the MCP tool isn't available or a URL is
unreachable, the skill does NOT block decomposition. It:

1. Prints a single `WARNING: …` line to the user during the run
   (`WARNING: Figma MCP not configured — <skill> proceeded without
   design context.` or `WARNING: Figma URL <url> unreachable —
   check sharing permissions.`).
2. Lists the affected URL under `## Design references` of the
   produced artifact with a tag —
   `_(Figma MCP not configured)_` or `_(link unreachable)_` — so
   the gap is preserved in the artifact body and the next reviewer
   sees what was skipped.

**Normalizers (`02n` / `03n` / `04n` / `05n`)** preserve any Figma
URLs they find in hand-authored artifacts by gathering them into
`## Design references`, but they do NOT call MCP — they leave
fetching to the next decomposition skill in the chain.

**SDE phase.** Both `07a-sde-plan-task` and
`07b-sde-execute-implementation` re-fetch Figma URLs to inform the
work — `07a` reads them to plan which frames map to which files;
`07b` re-reads the same URLs at execute time to pick up exact
component / layout / copy values, and uses
`mcp__<server>__download_figma_images` to pull assets the design
owns (logos, illustrations, exported PNG / SVG) into the appropriate
`assets/` directory (asset downloads happen only at execute time,
not at planning time). The Plan and Implementation notes each
record consulted URLs under `## Design references` with per-URL
notes about how the design informed the work. Failure handling is
the same warn-and-continue pattern as BA-phase skills. Test
generation (`08`), test execution (`09`), and bug-fix (`10`) do
not consume Figma — tests should validate the implementation, and
acceptance criteria from the Story already encode the testable
behavior.

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
