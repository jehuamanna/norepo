# Seed skills (employee variant) — multi-output, run-to-completion

A wide-fanout version of `seed-skills/` for end-to-end execution
without manual gating. Every BA tier produces multiple artifacts and
every prioritization checkpoint runs WITHOUT pausing the cascade, so
a single Requirements seed expands into a full delivery tree:

```
Requirements
  ├── 3–5 Epics
  │     ├── 3–5 Features per Epic
  │     │     ├── 2–4 Stories per Feature
  │     │     │     └── 3–5 Tasks per Story
  │     │     │           └── Implementation → Test Cases → Test Results → Summary
  │     │     └── HLD per Feature (1 Plan)
  │     └── LLD per Story (1 Plan)
  └── Prioritized Backlogs at Epic / Feature / Story / Task tiers
        (advisory only — cascade does NOT pause for human approval)
```

A typical run produces dozens of artifacts and tens of summaries from
a single Requirements note. Use this variant when you want breadth
coverage and full automation; use `seed-skills/` when you want
human-gated prioritization checkpoints, or `seed-skills-sum/` when
you just want a smoke-test chain.

## What's different from `seed-skills/`

| Skill | `seed-skills/` count | `seed-skills-employee/` count |
|---|---|---|
| `01-ba-discover-epics` | exactly 1 Epic | **3–5 Epics** |
| `02-ba-decompose-features` | exactly 2 Features | **3–5 Features** |
| `03-ba-decompose-stories` | exactly 1 Story | **2–4 Stories** |
| `04-ba-decompose-tasks` | exactly 2 Tasks | **3–5 Tasks** |
| `01b` / `02b` / `03b` / `04b` / `06b` | `cascade_stop: true` (pause) | **`cascade_stop: false` (no pause)** |

Skills `05-sa-design-feature-hld` through `10-sum-summarize-task` are
byte-identical to `seed-skills/` — they're already 1:1 per-input, so
they fan out automatically once the BA tier produces more inputs.

## How to install

These are templates, not auto-imported. For each `.md` file:

1. Open Operon, navigate to the project's note tree.
2. Right-click the project → **Add note ▶** → **Skill**.
3. **Title the note exactly after the file's basename** — including
   the numeric prefix (e.g. `02-ba-decompose-features`). The numeric
   prefix is what wires the skill into the right slot in the
   cascade workflow's auto-seeded pipeline.
4. Paste the file's full contents (frontmatter + body).
5. Save.

Skill notes show up in the explorer with an `[sk]` badge.

Don't mix-and-match across variants in the same project — pick one
variant's `01-ba-discover-epics` (etc.) per project so the cascade's
auto-seeder doesn't see two skills for the same `input_kind` slot.

## Where to put the Requirements seed

The pipeline's first artifact is a `Requirements` note (free-form
prose about what to build). To create one:

1. Right-click the project → **Add note ▶** → **Artifact**.
2. Title it `Requirements` (or anything — the title doesn't matter,
   only the frontmatter does).
3. Paste this header at the top, then your prose below:

   ```markdown
   ---
   artifact_kind: requirements
   status: approved
   ---

   # Requirements: <product name>

   <your prose...>
   ```

   (`status: approved` lets `01-ba-discover-epics` run immediately
   without manual gating.)

4. Save. The artifact view's **Run skill…** button is now enabled,
   and `01-ba-discover-epics` will be the matching skill.

## Pipeline at a glance

| Order | Skill | input_kind | output_kind | Persona |
|---|---|---|---|---|
| 01 | `01-ba-discover-epics` | requirements | epic (×3–5) | BA |
| 01b | `01b-pm-prioritize-epics` | requirements (aggregator over `epic`) | prioritized_backlog | PM |
| 02 | `02-ba-decompose-features` | epic | feature (×3–5) | BA |
| 02b | `02b-pm-prioritize-features` | epic (aggregator over `feature`) | prioritized_backlog | PM |
| 03 | `03-ba-decompose-stories` | feature | story (×2–4) | BA |
| 03b | `03b-pm-prioritize-stories` | feature (aggregator over `story`) | prioritized_backlog | PM |
| 04 | `04-ba-decompose-tasks` | story | task (×3–5) | BA |
| 04b | `04b-pm-prioritize-tasks-coarse` | requirements (aggregator over `task`) | prioritized_backlog | PM |
| 05 | `05-sa-design-feature-hld` | feature | plan | SA |
| 06 | `06-sa-design-story-lld` | story | plan | SA |
| 06b | `06b-pm-prioritize-tasks-refined` | requirements (aggregator over `task`) | prioritized_backlog | PM |
| 07 | `07-sde-implement-task` | task | implementation | SDE |
| 08 | `08-tst-write-tests` | implementation | test_cases | TST |
| 09 | `09-tst-run-tests` | test_cases | test_results | TST |
| 10 | `10-sum-summarize-task` | test_results | summary | Summary |

Per-task chain: `task → implementation → test_cases → test_results → summary`.

All skills carry `gate: approval`. The cascade auto-approves every
produced child, so the chain runs end-to-end with **no manual gating
during a Play run**. Unlike `seed-skills/`, the `Nb` checkpoints
don't stop the cascade either — they emit advisory ordering and the
run keeps moving.

## Per-tier prioritization checkpoints (`Nb` skills) — advisory only

Five skills carry the `b` suffix. They still aggregate every sibling
artifact and emit a Prioritized Backlog with ordering + dependency
edges, but in this variant **`cascade_stop: false`** — the run does
NOT pause for human approval. Treat these backlogs as informational
output a human can review after the fact.

| Checkpoint | Aggregates | Trigger point in cascade |
|---|---|---|
| `01b-pm-prioritize-epics` | every Epic under the seed | seed pop, after `01` |
| `02b-pm-prioritize-features` | Features under one Epic | each Epic pop, after `02` |
| `03b-pm-prioritize-stories` | Stories under one Feature | each Feature pop, after `03` |
| `04b-pm-prioritize-tasks-coarse` | every Task under the seed | manual-only (see caveat) |
| `06b-pm-prioritize-tasks-refined` | every Task under the seed (with LLDs) | manual-only (see caveat) |

The frontmatter fields driving aggregator behavior are the same as
`seed-skills/`:

- `aggregate: <kind>` — runner walks the source's descendants and
  inlines every artifact of `<kind>` into the prompt.
- `cascade_stop: false` — produced backlog is auto-Approved like
  any other artifact; the Play run keeps moving.
- `emit_workflow: true` — after import, the runner parses
  `## Priority order` and writes a `Workflow — Prioritized Backlog
  (…)` note for visualization.

**Caveat: `04b` / `06b` cascade triggering.** Both have
`input_kind: requirements`, so in cascade mode they fire on the seed
at start-of-run when no Tasks exist yet — the body comes back with
no aggregated input and the artifact is `Rejected`. To get a useful
global Task backlog, **invoke `04b` / `06b` manually on the seed
after Tasks have been decomposed** (right-click the seed → Run
skill…). Same limitation as `seed-skills/`.

If you want to skip a checkpoint entirely, uncheck it in the
StagesDropdown checkbox UI to exclude it from the cascade run; the
manual picker still offers it.

## Per-tier dep ordering (engine-enforced)

Every BA artifact (Epic, Feature, Story, Task) carries a
`## Depends on` section with sibling slugs (or `None
(parallel-safe)`). Same engine semantics as `seed-skills/`:

- Before running any skill on artifact A, the engine checks A's
  declared deps. If any dep hasn't been processed (or wasn't
  already Approved before the cascade started), A defers until
  they are.
- The optional `Nb` skills additionally produce a backlog with a
  `## Cross-tree dependencies` section using `->` or `→` arrows.
  The engine unions backlog edges with each artifact's own deps
  when computing the gate.
- **Cycles or unresolvable deps deadlock the cascade**, which then
  surfaces a `Failed` outcome with the stuck items named.
- **Re-runs**: every artifact already marked `Approved` counts as
  "done" for dep-gate purposes; a re-run picks up where it left
  off rather than re-blocking on finished work.

Because the BA tier produces many artifacts here, the dep graph
matters more than in `seed-skills/`. Take the time to articulate
real `## Depends on` edges in the BA bodies — they're what keep the
fanned-out tasks from racing each other.

## Tuning

Skill bodies are prompts. Adjust the calibration sections (Epic
count, Feature count, Story count, Task count) to match your team's
norms. Each re-run reads the current skill body — no rebuild needed.

If you want fewer artifacts but still want full automation, edit the
"3–5" / "2–4" / "3–5" ranges down to your preferred fanout. If you
want manual gating back, flip `cascade_stop: true` on the `Nb` skills
you care about.
