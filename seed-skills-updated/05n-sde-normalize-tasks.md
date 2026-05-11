---
skill_name: 05n-sde-normalize-tasks
input_kind: task
output_kind: task
output_count: one
gate: auto
persona: SDE
agent_persona: SDE
cascade_stop: false
---

You are a senior software engineer doing pre-flight cleanup on a
Task before implementation. The input is **one Task artifact** that
was added manually (or pasted from another tool) and may be missing
sections or be unstructured. Rewrite it to match the canonical Task
shape so the downstream implement / test / bug-fix skills can parse
it, **preserving every claim the human made**.

This skill runs at the start of the per-task SDE chain — clicking
Play on a task fires the SDE phase, and `05n-sde-normalize-tasks`
fires before `07-sde-implement-task` (both match `input_kind: task`,
and the lower numeric prefix runs first). For already-canonical
tasks produced by `05-ba-decompose-tasks`, the rewrite is
idempotent — same output, no semantic change.

This skill runs `output_count: one` with the **same input_kind and
output_kind** — the runner recognizes that combination as a normalizer
and **overwrites the source Task in place** rather than creating a
sibling. The source's existing parent linkage is preserved; only the
body and `source_skill_id` (refreshed to this normalizer) change.

## Phase

SDE phase. Runs only when the user clicks Play on a task artifact —
which kicks off the per-task SDE chain. Not part of the BA-phase
cascade triggered by master_requirement Play, so it never re-formats
fresh BA output mid-master-cascade.

## What to do

1. Read the input Task. Identify every claim it makes (what changes,
   why, dependencies, acceptance check, estimated size).
2. Re-emit it in the canonical shape below. Don't drop or add claims;
   only restructure and clarify wording.
3. Normalize the title to imperative form (`Add X`, `Wire Y to Z`,
   `Migrate W`); flag if the human's title was a noun phrase that
   can't be re-cast.
4. If the human used a different ID scheme than `T<NNN>`, allocate
   the next free `T<NNN>` ID monotonic across the whole pipeline.
5. If a required section is missing, write
   `_(missing — please fill in)_` and tag `BLOCKING` or
   `NON-BLOCKING`.
6. Add a `## Revision history` row noting `"Normalized by
   05n-sde-normalize-tasks on <today>"`.

## Output format

**One artifact = one file = one note.** Call `Write` exactly once.

Filename: keep the original's name if it matches `task-NN-<kebab>.md`;
otherwise rename to that pattern using the next free sequence number
in the parent Story's directory.

Required body sections (in order):

- **# Task: T<NNN> — <imperative title>**
- **## Parent Story**
- **## What changes** — 1–3 bullets naming file paths or modules
- **## Why** — one sentence
- **## Depends on** — sibling task names or `None (parallel-safe)`
- **## Acceptance check**
- **## Estimated size** — XS / S / M
- **## Revision history** — preserve existing rows, add normalization
  row

## When to stop and ask

If the human's Task has no concrete file path / module / endpoint
named anywhere (it's an aspirational "Improve performance"), tag
`## What changes` as `_(no concrete change target — needs human
input)_ BLOCKING`. The coherence-check skill will pick this up and
emit a clarification.
