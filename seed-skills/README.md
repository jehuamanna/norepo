# Seed skills for the SDLC artifact pipeline

Nine sample skills that drive the
`Requirements → Epic → Story → Task → HLD/LLD → Code → Tests → Test Results → Summary`
pipeline on top of the artifact engine in `src/plugins/artifact/`.

## How to install

These are templates, not auto-imported. For each `.md` file:

1. Open Operon, navigate to the project's note tree.
2. Right-click the project → **Add note ▶** → **Skill** (the arrow opens a
   submenu listing all creatable kinds — Markdown, MDX, Code, Skill,
   Workflow, Artifact, etc.). You can also click the **+** button on the
   project header for the same submenu.
3. **Title the note exactly after the file's basename** — including the
   numeric prefix (e.g. `02-ba-decompose-stories`). The numeric
   prefix is what wires the skill into the right slot in the cascade
   workflow's auto-seeded pipeline; without it, the skill is still
   usable from the manual picker but won't be in the default flow.
4. Paste the file's full contents (frontmatter + body).
5. Save.

Skill notes show up in the explorer with an `[sk]` badge.

Once all nine exist, the skill picker on every artifact will offer the
ones whose `input_kind` matches that artifact's `artifact_kind`. The
Cascade workflow note auto-created from a Requirements artifact will
contain a chained skill node per numbered skill, in numeric order.

## Where to put the Requirements seed

The pipeline's first artifact is a `Requirements` note (free-form prose
about what to build). To create one:

1. Right-click the project → **Add note ▶** → **Artifact**.
2. Title it `Requirements` (or anything — the title doesn't matter, only
   the frontmatter does).
3. Paste this header at the top, then your prose below:

   ```markdown
   ---
   artifact_kind: requirements
   status: approved
   ---

   # Requirements: <product name>

   <your prose...>
   ```

   (`status: approved` lets you immediately run `01-ba-discover-epics`
   on it. Without it, the gate will block the run since the default
   is `pending`.)

4. Save. The artifact view's **Run skill…** button is now enabled, and
   `01-ba-discover-epics` will be the matching skill (input_kind:
   requirements).

## Pipeline at a glance

| Order | Skill | input_kind | output_kind | Persona |
|---|---|---|---|---|
| 01 | `01-ba-discover-epics` | requirements | epic | BA |
| 01b | `01b-pm-prioritize-epics` | requirements (aggregator over `epic`) | prioritized_backlog | PM |
| 02 | `02-ba-decompose-stories` | epic | story | BA |
| 02b | `02b-pm-prioritize-stories` | epic (aggregator over `story`) | prioritized_backlog | PM |
| 03 | `03-ba-decompose-tasks` | story | task | BA |
| 03b | `03b-pm-prioritize-tasks-coarse` | requirements (aggregator over `task`) | prioritized_backlog | PM |
| 04 | `04-sa-design-epic-hld` | epic | plan | SA |
| 05 | `05-sa-design-story-lld` | story | plan | SA |
| 05b | `05b-pm-prioritize-tasks-refined` | requirements (aggregator over `task`) | prioritized_backlog | PM |
| 05c | `05c-sa-prioritize-plans` | requirements (aggregator over `plan`) | prioritized_backlog | SA |
| 06 | `06-sde-implement-task` | task | implementation | SDE |
| 07 | `07-tst-write-tests` | implementation | test_cases | TST |
| 08 | `08-tst-run-tests` | test_cases | test_results | TST |
| 09 | `09-sum-summarize-task` | test_results | summary | Summary |

The numeric prefix on every skill title is what the Cascade workflow's
auto-seeder reads to lay out the default pipeline. Adding a custom
skill `10-deploy-task` with the right title prefix appends it to the
chain automatically — no code change needed. Drop the prefix on a
skill (e.g. retitling to `07-tst-write-tests` → `tst-write-tests`)
and it falls out of the default flow but stays available via the
manual picker.

Per-task chain: `task → implementation → test_cases → test_results → summary`.
The `summary` artifact is the authoritative per-task result the
stakeholder reads — it folds in the implementation, the tests, and the
test outcome.

All skills are gated (`gate: approval`) so child skills can't run on a
parent that hasn't been Approved in the artifact view. The cascade
auto-approves every produced child, so the chain runs end-to-end
without manual gating during a Play run.

## Dynamic decomposition: N derived from input

None of the decomposition skills (`01`, `02`, `03`) emit a fixed count.
Each derives **N** from its input using coverage / singularity / size
tests baked into the skill body:

- `01-ba-discover-epics` reads the Master Requirement + Requirements
  prose and picks the smallest set of demoable slices that covers
  everything without overlap.
- `02-ba-decompose-stories` reads the Epic's `## Scope` bullets and
  produces one Story per user-meaningful behavior, walking-skeleton
  first.
- `03-ba-decompose-tasks` reads the Story's `## Acceptance criteria`
  and produces one Task per imperative change (schema, util, endpoint,
  UI, fixture …) sized to <1 day.

The prioritization checkpoints (`Nb` skills) flag both
under-decomposition (gaps in Epic `## Scope` coverage) and
over-decomposition (overlapping siblings) so the human reviewer can
correct before the next tier fires.

## Per-tier prioritization checkpoints (`Nb` skills)

Five skills carry the `b` suffix and act as **prioritization
checkpoints** at different tiers:

| Checkpoint | Aggregates | Trigger point in cascade |
|---|---|---|
| `01b-pm-prioritize-epics` | every Epic under the seed | seed pop, after `01` |
| `02b-pm-prioritize-stories` | Stories under one Epic | each Epic pop, after `02` |
| `03b-pm-prioritize-tasks-coarse` | every Task under the seed | manual-only (see caveat) |
| `05b-pm-prioritize-tasks-refined` | every Task under the seed (with LLDs) | manual-only (see caveat) |
| `05c-sa-prioritize-plans` | every Plan under the seed | manual-only (see caveat) |

Three frontmatter fields drive the special behavior:

- `aggregate: <kind>` — the runner walks the source's descendants
  and inlines every artifact of `<kind>` into the prompt instead of
  just the source body.
- `cascade_stop: true` — the cascade DOES NOT auto-approve the
  produced backlog. The Play run pauses with a "review the new
  backlog and approve to continue" status; clicking Approve and
  Play again resumes from the next dirty downstream node.
- `emit_workflow: true` — after the artifact is imported, the
  runner parses its `## Priority order` list, looks up each child
  by title, and writes a `Workflow — Prioritized Backlog (…)` note
  so the dependency graph is visual, not just prose.

**Caveat: `03b` / `05b` / `05c` cascade triggering.** All three have
`input_kind: requirements`, so in cascade mode they fire on the
seed at start-of-run when no Tasks/Plans exist yet — the body comes
back with no aggregated input and the artifact is `Rejected`. To get
a useful global backlog, **invoke these manually on the seed** (right-
click the seed → Run skill…) after the prerequisite artifacts have
been decomposed. The cascade triggering of these three is a known
limitation; the `Nb` skills at higher tiers (`01b`/`02b`) work
correctly in cascade because their inputs are produced by the
immediately-preceding skill on the same artifact.

Skipping any `Nb` skill is fine — uncheck it in the StagesDropdown
checkbox UI to exclude it from the cascade run; the manual picker
still offers it.

## Per-tier dep ordering (engine-enforced)

Every BA artifact (Epic, Story, Task) has a `## Depends on`
section with sibling slugs (or `None (parallel-safe)`). The cascade
engine reads these and serializes processing per tier:
- Before running any skill on artifact A, the engine checks A's
  declared deps. If any dep hasn't been processed (or wasn't already
  Approved before the cascade started), A defers until they are.
- The optional `Nb` skills additionally produce a backlog with a
  `## Cross-tree dependencies` section using `->` or `→` arrows.
  The engine unions backlog edges with each artifact's own deps
  when computing the gate.
- If two backlogs disagree (e.g. `03b` says `T5 -> T2`, `05b` says
  `T5 -> T3`), the engine respects all listed prerequisites — the
  union is conservative.
- **Cycles or unresolvable deps deadlock the cascade**, which then
  surfaces a `Failed` outcome with the stuck items named (`title <-
  [unresolved-prereq, …]`). Fix the bodies or backlogs and re-run.
- **Stale backlog edges**: if you remove a `## Depends on` from an
  artifact body but the latest backlog still asserts that edge,
  the engine will keep blocking. Re-run the relevant `Nb` skill to
  refresh the backlog (or delete the stale backlog artifact).
- **Re-runs**: after partial completion, every artifact already
  marked `Approved` counts as "done" for dep-gate purposes. So a
  re-run picks up where it left off rather than re-blocking on
  finished work.

## Tuning

Skill bodies are prompts. Adjust the "How many to produce" sections
(Epic coverage tests, Story sizing, Task layer-coverage rules) to
match your team's norms. Each re-run reads the current skill body —
no rebuild needed.
