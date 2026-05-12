# Seed skills (slim test pipeline) — 1 epic / 1 feature / 1 story / 1 task

A minimal version of `seed-skills/` for end-to-end smoke-testing the
artifact cascade. Every BA tier produces exactly **one** artifact, so a
Requirements seed cascades into a single chain:

```
Requirements
  └── 1 Epic
        └── 1 Feature
              └── 1 Story
                    └── 1 Task
                          └── Implementation → Test Cases → Test Results → Summary
```

Plus one HLD per Feature and one LLD per Story (so 1 each).

## What's different from `seed-skills/`

- `02-ba-decompose-features` produces **1 Feature** (full version: 2).
- `04-ba-decompose-tasks` produces **1 Task** (full version: 2).
- The `Nb` / `Nc` PM/SA prioritization checkpoints are **omitted**.
  In single-artifact mode they always self-Reject (their stop-and-ask
  rule fires), which would just litter the cascade with Rejected
  notes. The full `seed-skills/` set is the right pick when you want
  to exercise prioritization.

The remaining 10 skills are byte-identical to `seed-skills/` except
where called out above.

## How to install

Same procedure as `seed-skills/README.md`: per `.md` file, create a
new Skill note under the project, title it after the file's basename
(numeric prefix matters for cascade auto-seeding), paste contents,
save. The skill picker on each artifact will offer the matching skill
based on `input_kind`.

## Where to put the Requirements seed

Same as in `seed-skills/`: create an Artifact note with this header
and your prose below it.

```markdown
---
artifact_kind: requirements
status: approved
---

# Requirements: <product name>

<your prose...>
```

`status: approved` lets `01-ba-discover-epics` run immediately
without manual gating.

## Pipeline at a glance

| Order | Skill | input_kind | output_kind | Persona |
|---|---|---|---|---|
| 01 | `01-ba-discover-epics` | requirements | epic | BA |
| 02 | `02-ba-decompose-features` | epic | feature | BA |
| 03 | `03-ba-decompose-stories` | feature | story | BA |
| 04 | `04-ba-decompose-tasks` | story | task | BA |
| 05 | `05-sa-design-feature-hld` | feature | plan | SA |
| 06 | `06-sa-design-story-lld` | story | plan | SA |
| 07 | `07-sde-implement-task` | task | implementation | SDE |
| 08 | `08-tst-write-tests` | implementation | test_cases | TST |
| 09 | `09-tst-run-tests` | test_cases | test_results | TST |
| 10 | `10-sum-summarize-task` | test_results | summary | Summary |

All skills are gated (`gate: approval`); the cascade auto-approves
each child it produces, so a Play run goes end-to-end without manual
gating.

## Per-tier dep ordering

Every BA artifact still carries a `## Depends on` section, but in
single-artifact-per-tier mode it always reads `None (parallel-safe)` —
there are no siblings to depend on. The cascade engine treats this as
a clean parallel-safe graph and does not block.

## Tuning

Skill bodies are prompts. Adjust calibration sections to taste; each
re-run reads the current skill body — no rebuild needed.
