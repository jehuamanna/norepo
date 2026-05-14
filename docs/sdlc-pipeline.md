# SDLC Pipeline — User Guide

Operon turns a free-form charter into shippable code by running a chain
of Claude-driven skills, each producing typed Artifact notes. This
document explains the artifact tree, the placement rules, the run
lifecycle, and the conventions you need to know to use the app
effectively.

If you're looking for an end-to-end walkthrough on a fresh app, the
checklist at the bottom of this file is the quickest path.

---

## Cascade artifact tree (post-cascade structure)

```
demo (project)
├── Skill notes ────────────────────── one per imported seed; NoteKind::Skill
│   ├── 00-coherence-check
│   ├── 02-ba-discover-epics
│   ├── 02n-ba-normalize-epics
│   ├── 03-ba-decompose-features
│   ├── 03n-ba-normalize-features
│   ├── 04-ba-decompose-stories
│   ├── 04n-ba-normalize-stories
│   ├── 05-ba-decompose-tasks
│   ├── 05n-sde-normalize-tasks
│   ├── 06-sa-draft-architecture
│   ├── 07a-sde-plan-task
│   ├── 07b-sde-execute-implementation
│   ├── 08-sde-generate-tests
│   ├── 09-sde-execute-tests
│   ├── 10-sde-fix-bug
│   └── 11-sa-review-architecture
│
├── Discovery (NoteKind::Phase, phase_order: 0)
│   └── master-req-memory-match (Artifact: master_requirement)
│       │     ↑ may have user-authored Markdown / nested master-reqs /
│       │       images as direct children — those are BUNDLED into the
│       │       prompt at run time, they're NOT separate cascade outputs.
│       │
│       ├── architecture-memory-match (Artifact: architecture)
│       │   └── review-phase-1-multiplayer (Artifact: architecture_review)
│       │
│       └── epic-01-playable-memory-match (Artifact: epic)
│           │  (body rewritten in place by 02n-ba normalizer)
│           │
│           ├── feature-01-grid-of-cards (Artifact: feature)
│           │   │  (body rewritten in place by 03n)
│           │   │
│           │   ├── story-01-card-flip (Artifact: story)
│           │   │   │  (body rewritten in place by 04n)
│           │   │   │
│           │   │   ├── task-01-build-game-state (Artifact: task)
│           │   │   │   │  (body rewritten in place by 05n)
│           │   │   │   │
│           │   │   │   └── plan-task-01 (Artifact: implementation_plan)
│           │   │   │       └── impl-task-01 (Artifact: implementation)
│           │   │   │           ├── test-cases-task-01 (Artifact: test_cases)
│           │   │   │           │   └── test-results-task-01 (Artifact: test_results)
│           │   │   │           └── bug-N (Artifact: bug)
│           │   │   │               └── impl-rev (Artifact: implementation)
│           │   │   ├── task-02 …
│           │   │   └── task-03 …
│           │   ├── story-02 …
│           │   └── story-03 …
│           ├── feature-02 …
│           └── feature-03 …
│
└── Phase 1 — Multiplayer (NoteKind::Phase, phase_order: 1)
    └── master-req-multiplayer (Artifact: master_requirement)
        │  ← Phase gate: `06-sa-draft-architecture` SKIPS here
        │    (architecture is frozen after the first phase).
        │  ← When this cascade completes, `11-sa-review-architecture`
        │    auto-fires against the existing architecture and a new
        │    review note appears under it.
        │
        └── epic-… (Artifact: epic)
            └── feature-… → story-… → task-… → plan → impl → tests → results
```

---

## Skill → output map

| Skill | input_kind | output_kind | count | placement | notes |
|---|---|---|---|---|---|
| `02-ba-discover-epics` | master_requirement | epic | many | child of source | aggregates `requirements`; bundles master-req subtree |
| `02n-ba-normalize-epics` | epic | epic | one | **rewrites source body** | no new note |
| `03-ba-decompose-features` | epic | feature | many | child of source | |
| `03n-ba-normalize-features` | feature | feature | one | rewrites source | |
| `04-ba-decompose-stories` | feature | story | many | child of source | |
| `04n-ba-normalize-stories` | story | story | one | rewrites source | |
| `05-ba-decompose-tasks` | story | task | many | child of source | |
| `05n-sde-normalize-tasks` | task | task | one | rewrites source | |
| `06-sa-draft-architecture` | master_requirement | architecture | one | sibling-of-epics under MR | gated to first phase only |
| `07a-sde-plan-task` | task | implementation_plan | one | child of task | inherits architecture |
| `07b-sde-execute-implementation` | implementation_plan | implementation | one | child of plan | inherits architecture; writes code via Claude tools |
| `08-sde-generate-tests` | implementation | test_cases | one | child of impl | |
| `09-sde-execute-tests` | test_cases | test_results | one | child of test_cases | runs the tests, captures output |
| `10-sde-fix-bug` | bug | implementation | one | child of bug | revision of an earlier impl |
| `11-sa-review-architecture` | architecture | architecture_review | one | child of architecture | auto-fires for non-first-phase cascades |
| `00-coherence-check` | any | clarification | many | child of disagreeing artifact | halts cascade until each Pending is resolved |

`aggregate: <kind>` in a skill's frontmatter walks descendants of the
source and inlines every matching artifact into the prompt. `inherit:
<kind>` walks the ancestor chain instead. Neither field changes
placement — they only enrich the prompt the model sees.

---

## Three placement rules to remember

1. **Default (non-normalizer):** the new artifact is a **child of the
   note Play was clicked on**. If the skill's `input_kind` doesn't match
   the source's `artifact_kind`, the runner walks up the parent chain
   to find an ancestor of the right kind and re-parents the new
   artifact there. Click Play on the wrong note, the runner still does
   the right thing.

2. **Normalizer (`input_kind == output_kind`, `output_count: one`):**
   no new note is created. The source body is **rewritten in place**,
   preserving its parent and existing children. That's why Epic /
   Feature / Story / Task bodies "improve" without the tree growing.

3. **Aggregator (`aggregate: <kind>`):** the runner inlines every
   descendant of the source matching `<kind>` into the prompt.
   Placement is unchanged; the model just sees more context.

---

## What's NOT a child of anything

- **Skill notes** live at the project root, imported once via the
  project's right-click → "Import skills…" menu.
- **Phase notes** (`NoteKind::Phase`) live at the project root, created
  via right-click → "New phase".
- **User-authored markdown, nested master-reqs, images** under a
  `master_requirement` are **prompt inputs** (bundled into the skill
  run's context), not cascade outputs.
- **Workflow notes** (the cascade canvas) are siblings of artifacts,
  auto-created the first time you Play. Treat them as a visualization,
  not part of the lineage.

---

## On-disk vs explorer

Two structures kept in sync:

- **Explorer tree** = the `parent_id` chain stored in SQLite. Drives
  the visible hierarchy in the side panel and what the cascade walks.
- **On-disk artifact body** = `<repo>/.operon/artifacts/<slug>/<slug>/.../index.md`.
  Each artifact note carries a stable `slug` (migration 018) so
  renames don't break paths. The runner writes new artifacts to a
  per-run tempdir under `.operon/`, then `import_produced_artifacts`
  moves them to their canonical slug path.
- **Clarification artifacts** (from `00-coherence-check`) are the one
  exception: they land as `.mdx` files in the per-run tempdir mid-
  cascade and get imported as `NoteKind::Artifact` children of the
  disagreeing note. The cascade halts until each Pending clarification
  is answered.

---

## Three-tier Claude settings

Every Claude invocation needs a model and a permission mode. Operon
resolves both via a three-tier hierarchy:

- **Chat-level** override (chat header model + permission pickers).
- **Project-level** default (Tools → Project Claude Defaults).
- **Global** default (Settings → Claude defaults).

Resolution order: chat → project → global → omit the flag (let
Claude pick). The chat picker's first option labels what would be
inherited from below — e.g. `Inherit (Opus 4.7)` means the global
default is Opus 4.7 and this chat hasn't overridden.

For Vault-scope chats (no project bound), the project tier is skipped
— it's chat → global.

---

## Phase notes — multi-batch requirements

A phase is a project-root container (`NoteKind::Phase`) that groups
one batch of requirements + its decomposition. Typical use: Discovery
holds the initial charter; Phase 1, Phase 2, … hold follow-on
requirement batches.

Frontmatter fields:
- `phase_order: <int>` — sort key. Lower = earlier. Optional.
- `phase_label: <str>` — display name. Optional; falls back to the
  note title.

Ordering: explicit `phase_order` ascending, then `created_at_ms`
ascending. Unnumbered phases sort to the end.

**Architecture is frozen after the first phase.** When a master-req
in Phase 1+ is run through the cascade, the architecture skill is
gated off and the post-cascade auto-trigger fires the architecture-
review skill instead. The review surfaces as a child note under
architecture, plus a `needs_review: true` flag that drives a ⚠ badge
in the explorer.

---

## Artifact status lifecycle

Every Artifact carries a `status:` in its frontmatter. The lifecycle:

```
Pending  ──Approve──▶  Approved  ──MarkDirty──▶  Dirty  ──Approve──▶  Approved
   │                       │                                              │
   └─Reject──▶ Rejected     └──── downstream skills can run on Approved ─┘
                                                  or Dirty
   Running   while a skill is producing this artifact
   Error     a skill run failed (transient; clear by re-running)
```

- **Pending** — fresh output, awaiting human approval. Downstream
  skills cannot run on Pending sources (with one exception: the root
  artifact you Play on is exempt — it's the explicit entry point).
- **Approved** — the user has accepted the body as-is.
- **Dirty** — the user edited the body and wants downstream re-runs
  that preserve the existing subtree (revision-row append mode).
- **Rejected** — the user dismissed the artifact. Downstream skills
  won't fire on it.

For child-producing skills, re-running on a Dirty source detects
existing children and rewrites them in place (revision rows appended
to history). Re-running on Approved is a no-op if children already
exist — the cascade walks downstream from them instead.

---

## Running the cascade

- **▶ Play** on any artifact runs every kind-matching skill on that
  node, then walks downstream BFS, repeating until each leaf has no
  more matching skills.
- **Step mode** (toolbar button on the workflow canvas) pauses after
  every skill firing so you can review each artifact before the next
  step. Continuous mode (default) only pauses at `cascade_stop`
  checkpoints (the normalizer skills + architecture).
- **Cascade-stop pause** — a skill with `cascade_stop: true` halts the
  cascade after producing its output. Approve the new artifacts and
  click Play again to resume.
- **Clarification block** — if `00-coherence-check` produces a
  `clarification` artifact, the cascade refuses to start until the
  user answers it. The clarification body lists the question and
  options; pick one to flip status to Approved.

The workflow canvas's `⧉ 1 phase / All phases` toggle hides edges that
cross phase boundaries by default. Toggle to show every cross-phase
dependency when debugging cross-batch wiring.

---

## Architecture review (Phase E)

When a Phase 1+ cascade completes, `11-sa-review-architecture` fires
automatically against the project's existing architecture artifact.
The output is a Pending `architecture_review` Artifact note as a
child of architecture, plus a `needs_review: true` flag set on
architecture itself.

Surface signals:
- Explorer row for architecture renders a ⚠ glyph next to the status
  dot.
- Opening architecture in the editor shows a yellow banner listing
  pending review titles.
- Opening a review shows `## Concerns`, `## Recommended amendments`,
  `## No action needed if…`.

When you Approve or Reject the last Pending/Dirty review, the
architecture's `needs_review` flag clears automatically and the
banner / badge disappear.

Manual re-trigger: open architecture → ▶ Play → skill picker →
`11-sa-review-architecture` (kind matches `architecture`).

The review never auto-edits architecture. Applying the recommended
amendments is the user's call — typically by editing the architecture
body, clicking Mark dirty, and re-running downstream work.

---

## Master-requirement subtree bundling (Phase D)

A master_requirement note is rarely a single document. You can author
nested master-reqs, supporting Markdown notes, and images under it,
and the runner bundles them into every skill run that sources from
that master-req.

In the prompt the model sees a block like:

```
--- master-requirement subtree (N text + M image) ---
--- subtree: user-personas ---
Persona A: power user; Persona B: casual
--- /subtree: user-personas ---
--- subtree image: ui-mockup ---
path: /vault/.operon/images/abc.png
(Use the Read tool to fetch this image if visual context is needed.)
--- /subtree image: ui-mockup ---
--- /master-requirement subtree ---
```

Walking order is depth-first by `sibling_index` so the prompt mirrors
the tree you authored. Only `Markdown` notes, nested `master_requirement`
artifacts, and `Image` notes go into the bundle — Epic / Feature /
Story / etc. children are downstream outputs and are skipped.

---

## Companion chat

The chat pane to the right of the editor runs `claude --print` with
each turn, with three useful affordances:

- **Per-chat model + permission pickers** in the header. Selecting
  `Inherit (X)` clears the chat-level override and falls back to the
  project / global tier.
- **Session rail** on the left. Sessions are ordered by `last_used_ms`
  (most recent first). Clicking a row only switches the active
  session — it does NOT reorder. Double-click a row to rename it
  inline. Hover shows the full chat name; the × delete button only
  appears on hover.
- **@-mentions**: type `@` followed by a note title prefix; the
  popover suggests matches. Selecting one inserts an
  `@[<title>](note:<uuid>)` token. The transcript renders these as
  clickable chips — clicking opens the note in a new tab and reveals
  it in the explorer (expanding any collapsed parent folders).

---

## Re-running and regeneration

- **Mark dirty** on an artifact → its frontmatter status flips to
  Dirty.
- **Play** on a Dirty artifact (or any ancestor of one) → child-
  producing skills detect the dirty child, wipe its stale subtree,
  re-run with the artifact's `revision_notes` as extra context, and
  append a row to `## Revision history`.
- **Refinement notes** — the artifact view has a `revision_notes`
  textarea (when in edit mode). What you type there is inlined into
  the next regeneration prompt under `--- refinement notes from
  user ---`. Auto-cleared after a successful re-run.

For a wholly fresh re-run that discards the existing subtree: delete
the children manually, then Play on the parent.

---

## Bug fixing

The SDE chain produces test_results. If tests fail, the user files a
`bug` artifact (right-click → Add note → Artifact ▶ → Bug) pointing
at the offending implementation. Running `10-sde-fix-bug` on the bug
produces a new `implementation` revision that addresses the bug; the
test cycle (`08` → `09`) then re-runs to confirm.

---

## Quick reference: end-to-end on a fresh app

1. Pick a vault root (Settings → Vault).
2. Settings → Claude defaults → set the global model + permission.
3. Right-click the workspace → New project. Bind a repository.
4. Right-click project → Import skills… → pick the `seed-skills-updated/`
   folder.
5. Right-click project → New phase → name it `Discovery`. Set
   `phase_order: 0` in its body.
6. Under Discovery, create a `master_requirement` artifact. Write the
   charter, Approve.
7. (Optional) author Markdown / image children under the master-req
   for richer context (Phase D bundle).
8. Click ▶ Play on the master-req. Watch the cascade walk
   architecture + epics, normalize them, then go feature → story →
   task → plan → impl → tests → results.
9. (Optional) New phase → `Phase 1`. Create another master_requirement.
   Play. This time architecture is NOT re-spawned; a review note
   appears under the existing architecture.
10. Open the review, read the concerns, Approve. The ⚠ flag clears.

---

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `architecture` appears as a child of an Epic | Play clicked on Epic before Phase A.1; legacy data | Drag architecture under master_req in explorer |
| Cascade halts immediately with "unresolved clarifications" | A `00-coherence-check` clarification is Pending | Open each Pending clarification, pick an answer, retry Play |
| Skill picker shows skills greyed out as mismatched | The source's `artifact_kind` doesn't match the skill's `input_kind` | Either Play on a kind-matching parent, or trust the runner to ascend to one |
| Phase 1 cascade spawns a second architecture | Phase note has wrong `phase_order` (≤ 0) | Check the phase's body frontmatter and the parent chain — the master-req must be inside a `NoteKind::Phase` with `phase_order > min` |
| ⚠ flag stuck on architecture after approving all reviews | `LocalNoteVersion` didn't refresh in this view | Switch tabs and back; or check the architecture body for stray `needs_review: true` |
| Model picker shows "Inherit (Claude default)" even after setting global | The global value was saved but the view's memo hasn't refreshed | Restart the app, or change + change-back the global to bump `GLOBAL_SETTINGS_VERSION` |
| Mention chip click does nothing | Running in wasm preview / sandbox without `NoteLinkResolver` provided | Use the desktop build; or wait for the panel to fully mount before clicking |
| Permission shim missing notice in chat header | `operon-mcp-permission` binary not built or not on PATH | `cargo build` rebuilds it as a sibling of the main binary; or set `OPERON_MCP_PERMISSION_BIN` |
