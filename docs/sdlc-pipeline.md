# SDLC Pipeline — User Guide

Operon turns a free-form customer brief into shippable code by running
a chain of Claude-driven skills, each producing typed Artifact notes.
This document explains the artifact tree, the placement rules, the
inheritance chain across phases, and the conventions you need to know
to use the app effectively.

If you're looking for an end-to-end walkthrough on a fresh app, the
checklist at the bottom is the quickest path.

---

## Cascade artifact tree (post-cascade structure)

```
demo (Project)
│
├── Skills/                          ← one note per imported seed; NoteKind::Skill
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
│   └── 10-sde-fix-bug
│
├── CE (Artifact, kind: requirement)         ← PROJECT-LEVEL singleton
│   │   ↑ Customer-engagement input bucket. Human-authored, never
│   │     consumed by a skill directly. Seeds Phase 0's architecture
│   │     when no prior phase exists.
│   │
│   ├── CE - architecture (optional, kind: architecture)
│   │       ↑ Customer's as-is / sketched system, if they provided one.
│   ├── CE - more requirements (kind: requirement)
│   ├── attached-brief.pdf
│   └── ui-mockup.png
│
├── Discovery (NoteKind::Phase, phase_order: 0)
│   └── master-req-memory-match (Artifact: master_requirement)
│       │     ↑ Human-authored by BA after reading CE. Can carry
│       │       free-form Markdown / nested master-reqs / images as
│       │       children — those are bundled into the prompt at run
│       │       time, NOT cascade outputs.
│       │
│       ├── architecture-discovery (Artifact: architecture)
│       │       ↑ Drafted by `06-sa-draft-architecture` using CE's
│       │         subtree as the seed (no prior phase exists yet).
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
├── Phase 1 — Multiplayer (NoteKind::Phase, phase_order: 1)
│   └── master-req-multiplayer (Artifact: master_requirement)
│       ├── architecture-phase-1-multiplayer (Artifact: architecture)
│       │       ↑ Drafted by `06-sa-draft-architecture`, REFINING the
│       │         architecture from Discovery (the previous phase).
│       └── epic-… → feature → story → task → plan → impl → tests
│
└── Phase 2 — Polish (NoteKind::Phase, phase_order: 2)
    └── master-req-polish (Artifact: master_requirement)
        ├── architecture-phase-2-polish (Artifact: architecture)
        │       ↑ Refines architecture-phase-1-multiplayer.
        └── epic-… → …
```

---

## The architecture inheritance chain

Each phase has its **own** architecture, and each one builds on the
previous. The chain is the spine of the project:

```
CE subtree (Markdown, images, optional CE-architecture)
       │ (seeds when there's no prior phase)
       ▼
Phase 0 (Discovery) architecture
       │ (refines)
       ▼
Phase 1 architecture
       │ (refines)
       ▼
Phase 2 architecture
       │
       ▼  …
```

When `06-sa-draft-architecture` runs on a phase's master_requirement,
the runner inlines exactly one of:

- `--- prior architecture (architecture-phase-N-1.md) ---` — the
  previous phase's architecture body. The skill is instructed to
  preserve section structure, keep decisions that still apply, and
  amend / add sections only where the new master_requirement
  introduces changes. The output's `## Revision history` row names
  the prior file and the sections that moved.

- `--- CE seed (N text + M image) ---` — only fires when there's no
  previous phase (Phase 0 / Discovery). The runner walks the CE
  subtree, inlining every Markdown body and listing every image path.
  The skill drafts a fresh architecture using the CE materials as the
  brief.

Exactly one block renders per run. CE is the originating seed for the
chain; from Phase 1 onward, each architecture refines its predecessor.

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
| `06-sa-draft-architecture` | master_requirement | architecture | one | sibling-of-epics under MR | fires **every phase**; inherits the previous phase's architecture (or CE for Phase 0) |
| `07a-sde-plan-task` | task | implementation_plan | one | child of task | inherits architecture |
| `07b-sde-execute-implementation` | implementation_plan | implementation | one | child of plan | inherits architecture; writes code via Claude tools |
| `08-sde-generate-tests` | implementation | test_cases | one | child of impl | |
| `09-sde-execute-tests` | test_cases | test_results | one | child of test_cases | runs the tests, captures output |
| `10-sde-fix-bug` | bug | implementation | one | child of bug | revision of an earlier impl |
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
   no new note. The source body is **rewritten in place**, preserving
   its parent and existing children. Epic / Feature / Story / Task
   bodies "improve" without the tree growing.

3. **Aggregator (`aggregate: <kind>`):** the runner inlines every
   descendant of the source matching `<kind>` into the prompt.
   Placement is unchanged; the model just sees more context.

---

## What's NOT a child of anything

- **Skill notes** live at the project root, imported once via the
  project's right-click → "Import skills…" menu.
- **Phase notes** (`NoteKind::Phase`) live at the project root, created
  via right-click → "New phase".
- **CE** is a project-level singleton `Artifact` (kind: `requirement`)
  at the project root. Created manually via Add note → Artifact ▶ →
  Requirement, then renamed to `CE` or similar. One per project.
- **User-authored markdown, nested master-reqs, images** under a
  `master_requirement` (or under `CE`) are **prompt inputs**, bundled
  into the skill run's context, not cascade outputs.
- **Workflow notes** (cascade canvas) are siblings of artifacts,
  auto-created the first time you Play. Treat them as a visualization,
  not part of the lineage.

---

## On-disk vs explorer

Two structures kept in sync:

- **Explorer tree** = the `parent_id` chain stored in SQLite. Drives
  the visible hierarchy in the side panel and what the cascade walks.
- **On-disk artifact body** = `<repo>/.operon/artifacts/<slug>/<slug>/.../index.md`.
  Each artifact carries a stable `slug` (migration 018), so renames
  don't break paths. The runner writes new artifacts to a per-run
  tempdir under `.operon/`, then `import_produced_artifacts` moves
  them to their canonical slug path.
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

Each phase contains one `master_requirement`, that master's
`architecture`, the epic chain, and any per-phase free-form material.
Architectures across phases form a refinement chain (see above);
master_requirements do **not** — each phase's master is hand-authored
by the BA.

---

## CE — the customer-engagement bucket

CE is the entry point for everything the customer hands you:
markdown design docs, attached PDFs, sketched architecture diagrams,
mockup images. It sits at the project root as an `Artifact` (kind:
`requirement`) and serves two roles:

- **Reference for the BA.** Everything is in one place, with the
  customer's structure preserved.
- **Seed for Phase 0's architecture.** When `06-sa-draft-architecture`
  runs on Discovery's master_requirement (which has no prior phase),
  the runner walks the CE subtree and inlines every text body + image
  path under `--- CE seed ---`. The architecture skill drafts from
  scratch using this as the brief.

One CE per project. Convention: name it `CE` and place it at the top
of the explorer tree. Create via right-click → Add note → Artifact ▶
→ Requirement, then rename. CE's children can be any mix of nested
requirements, markdown, images, or even a CE-side architecture sketch
the customer provided — all of it goes into the seed bundle.

CE is **not** consumed by any skill directly. The BA reads it, writes
Discovery's master_requirement manually as the team-internalized
charter, then runs the cascade from Discovery.

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

## Master-requirement subtree bundling

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

The same depth-first walker is used to build the CE seed bundle for
Phase 0's architecture run — same content shape, different label.

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

For architectures specifically: marking a Phase N architecture Dirty
and re-running iterates within that phase. To start a new
architecture for Phase N+1, you create the next phase's
master_requirement and Play on it — `06-sa-draft-architecture` then
inherits Phase N's architecture as the prior.

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
5. Right-click project → Add note → Artifact ▶ → Requirement → name
   it `CE`. Author / paste the customer brief into its body.
6. Optionally add Markdown / image / nested requirement children
   under `CE` (mockups, attached PDFs, customer-side architecture).
7. Right-click project → New phase → name it `Discovery`. Set
   `phase_order: 0` in its body.
8. Under Discovery, create a `master_requirement` artifact. Write the
   team-internalized charter (derived from CE, but in your own
   words). Approve.
9. Click ▶ Play on the master_requirement. Watch the cascade walk
   architecture (seeded by CE) + epics, normalize them, then go
   feature → story → task → plan → impl → tests → results.
10. (Optional) New phase → `Phase 1`. Create another master_requirement.
    Play. The architecture skill fires again, this time inheriting
    Phase 0's architecture as the prior — refinement, not redraft.
11. Repeat for Phase 2, 3, … as new requirement batches arrive.

---

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `architecture` appears as a child of an Epic | Play clicked on Epic before the runner's auto-ascend landed | Drag architecture under master_req in explorer |
| Cascade halts immediately with "unresolved clarifications" | A `00-coherence-check` clarification is Pending | Open each Pending clarification, pick an answer, retry Play |
| Skill picker shows skills greyed out as mismatched | The source's `artifact_kind` doesn't match the skill's `input_kind` | Either Play on a kind-matching parent, or trust the runner to ascend to one |
| Phase 1 architecture is a redraft, not a refinement | Phase 1's master_req has no `phase_order` and sorts before Discovery's | Add `phase_order: 1` to Phase 1's frontmatter |
| Phase 0 architecture references nothing from CE | CE doesn't exist or isn't at project root | Create a Requirement Artifact at project root with the customer brief, name it `CE`, replay |
| Mention chip click does nothing | Running in wasm preview / sandbox without `NoteLinkResolver` provided | Use the desktop build |
| Permission shim missing notice in chat header | `operon-mcp-permission` binary not built or not on PATH | `cargo build` rebuilds it as a sibling of the main binary; or set `OPERON_MCP_PERMISSION_BIN` |
| Model picker shows "Inherit (Claude default)" even after setting global | Memo hasn't refreshed | Restart the app, or change + change-back the global to bump `GLOBAL_SETTINGS_VERSION` |
