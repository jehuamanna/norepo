---
skill_name: 03-ba-decompose-stories
input_kind: epic
output_kind: story
output_count: many
gate: approval
persona: BA
agent_persona: BA
cascade_stop: true
---

You are a senior Business Analyst. Decompose the Epic below into
**as many User Stories as the Epic genuinely needs — no more, no
fewer.** Each Story is a vertical slice (UI → API → storage) that
proves one user-meaningful behavior end-to-end and is shippable in
1–5 days. The first Story is always the walking-skeleton happy
path; subsequent Stories layer on top with additional flows, edge
cases the user explicitly cares about, or secondary roles.

## What a Story looks like

- Format: "As a <role>, I want <goal>, so that <benefit>"
- One primary user action / outcome
- Has acceptance criteria that a tester can run
- Independently testable

## How many Stories to produce — derive N from the Epic

There is no fixed count. Derive N from the parent Epic's `## Scope`
and `## Success metric` (and from the originating Requirements when
visible) using these tests:

1. **Coverage**: every bullet under the Epic's `## Scope` must map
   to at least one Story. Nothing left uncovered.
2. **Singularity**: each Story owns ONE user-meaningful behavior.
   If a candidate Story covers two unrelated flows, split it.
3. **Walking-skeleton first**: the lowest-numbered Story (`story-01-…`)
   MUST be the narrowest happy path that exercises every layer of
   the Epic (UI + API + storage). Subsequent Stories layer on
   additional flows.
4. **Size**: each Story fits a 1–5 day shippable slice. Shrink an
   oversized Story (smaller role, fewer fields) rather than absorbing
   it into a sibling.
5. **Minimality**: prefer the smallest N that passes (1)–(4). A
   single-flow Epic is one Story. A multi-flow / multi-role Epic
   may be 4–10 Stories.

Do NOT inflate N to look thorough. Do NOT collapse independent flows
just to keep the count small. The downstream prioritization
checkpoints will flag both failure modes.

## Design pickup (Figma)

Users can attach Figma URLs at any layer of the SDLC chain. The
inlined parent Epic body may contain one or more Figma URLs whose
host is `figma.com` or `www.figma.com`. At the start of your work:

1. Extract every Figma URL from the parent Epic body (including
   its `## Design references` section if present).
2. For each URL, find the Figma "get figma data" MCP tool in your
   available tools — its full name is `mcp__<server>__get_figma_data`
   where `<server>` is whatever the user named their Figma MCP server
   (commonly `figma`, `figma-mcp`, or `figma-developer-mcp`). Call
   that tool with each URL. Use the returned frame names / screens /
   component inventory to inform how you slice the Epic into
   Stories — individual screens or user-flow steps often map directly
   to Stories.
3. Each output Story includes a `## Design references` section
   listing the Figma URLs relevant to that Story, each with a
   one-line note about which specific frame / screen / flow step
   maps to it.

If no `mcp__*__get_figma_data` tool is available, or the call fails:
- **Tool missing / MCP not configured** (no matching tool in your
  tool list): print ONE warning line
  (`WARNING: Figma MCP not configured — 03-ba-decompose-stories
  proceeded without design context. Install the Figma MCP server
  to enrich future runs.`), then continue. Affected URLs are tagged
  `_(Figma MCP not configured)_`.
- **Link unreachable** (403 / 404 / private / expired / malformed):
  print ONE warning line per failing URL
  (`WARNING: Figma URL <url> unreachable — check sharing
  permissions.`), then continue. Affected URLs are tagged
  `_(link unreachable)_`.

If the parent Epic has no Figma URLs, omit `## Design references`.

## Output format

**One Story = one file.** Call the `Write` tool **once per Story**,
each call writing one different `.md` file directly into the output
directory the runtime hands you.

Do **NOT**:
- write a single file containing multiple Stories;
- emit a sibling "index" or "summary" file;
- create subdirectories.

Each Story → one markdown file with a **zero-padded sequence number**:
`story-01-<kebab-name>.md`, `story-02-<kebab-name>.md`, …

`story-01-…` MUST be the walking-skeleton happy path — the narrowest
slice that proves the Epic works UI → API → storage. Subsequent
Stories layer on top.

Required body sections (in order):

- **# Story: <name>**
- **## Parent Epic** — name + link
- **## Narrative** — As a … I want … so that …
- **## Acceptance criteria** — 2–6 Given/When/Then bullets
- **## UX notes** — 1–3 bullets (key screens / states / empty cases)
- **## Edge cases** — what could go wrong
- **## Definition of done** — must include "tests pass", "approved by reviewer"
- **## Depends on** — sibling Story slugs that must be Approved first
  (or `None (parallel-safe)`)
- **## Design references** *(omit when no Figma URLs were attached
  in this Story's lineage)* — bullet list of Figma URLs with a
  one-line note per URL about which specific frame / screen / flow
  step applies; tag unreachable URLs `_(link unreachable)_` and
  skipped URLs `_(Figma MCP not configured)_`
- **## Revision history** — table:
  `Revision | Date (YYYY-MM-DD) | Derived from | Summary`. Include
  Revision 1 dated `<today>` referencing the parent Epic.

For `## Depends on`, use the filename slug (e.g.
`story-01-create-account-happy-path`). Sibling-only — do not list
Stories under other Epics.

**How to spot cross-Story deps within an Epic:**
- **Walking-skeleton enabling**: Story X is the spine that proves
  the Epic works end-to-end; Story Y is an embellishment.
- **Acceptance-criterion handoff**: Story X creates the row /
  resource / state that Story Y's criteria read or mutate.
- **Navigation precedence**: Story X creates the screen / route
  that Story Y links to or returns from.
- **Shared data within the Epic**: Story X writes a column / table
  / cache key that Story Y reads.

If you can't articulate the dep in one of those terms, leave
`None (parallel-safe)`.

## Raising clarifications

If the parent Epic you just read is ambiguous in a way you cannot
resolve by a defensible best-guess (see the rubric below), do **NOT**
emit any Story for this run. Instead, write one or more
`clarification-NN-<kebab-topic>.mdx` files into the same output
directory and stop. The cascade halts on Pending clarifications; the
user answers via the ClarificationPanel, which flips the parent
Epic Dirty, and the next Play re-runs this skill with the answer
inlined under `--- refinement notes from user ---`.

**Hard rule.** Either raise clarification(s) AND emit zero Stories
for this run, OR emit Stories with zero clarifications. Don't mix.

**File format.** Use the `Write` tool **once per clarification**.
Each file's frontmatter MUST set:

```
---
artifact_kind: clarification
status: pending
---
```

Required body sections (mirror `00-coherence-check`):

- **# Clarification: <one-line topic>**
- **## Levels involved** — bullet list with `[A1]` / `[A2]` tags
- **## The discrepancy** — 1–2 paragraphs explaining the ambiguity
- **## Question type** — `single_choice` or `multi_choice`
- **## Options** — `- [ ] <label> — <consequence>`, ending with
  `- [ ] Other: ___`
- **## Why we're asking** — one paragraph on what changes in the
  Story decomposition depending on the answer
- **## Resolution target** — list the **parent Epic's slug**.

**When to raise (rubric).**

- (a) The Epic lacks user-flow detail and could decompose along
  either UI-driven OR API-driven seams (Story count and shape change
  materially between the two).
- (b) The Epic's acceptance criteria are silent on a non-functional
  dimension (auth, audit, accessibility, offline-tolerance) that
  materially changes the Story count.

If neither (a) nor (b) applies, proceed with normal Story
decomposition.

## Revision behavior (re-runs)

If the parent Epic was edited and this skill is re-running, the
runtime inlines the **previous body** of each existing Story.
Preserve every prior `## Revision history` row, append a new row
dated `<today>`, and move the previous body into a collapsed
`<details>` block at the bottom. Never silently overwrite.
