# Seed skills for the SDLC artifact pipeline

Ten sample skills that drive the
`Requirements → Epic → Feature → Story → Task → HLD/LLD → Code → Tests → Test Results → Summary`
pipeline on top of the artifact engine in `src/plugins/artifact/`.

## How to install

These are templates, not auto-imported. For each `.md` file:

1. Open Operon, navigate to the project's note tree.
2. Right-click the project → **Add note ▶** → **Skill** (the arrow opens a
   submenu listing all creatable kinds — Markdown, MDX, Code, Skill,
   Workflow, Artifact, etc.). You can also click the **+** button on the
   project header for the same submenu.
3. **Title the note exactly after the file's basename** — including the
   numeric prefix (e.g. `02-ba-decompose-features`). The numeric
   prefix is what wires the skill into the right slot in the cascade
   workflow's auto-seeded pipeline; without it, the skill is still
   usable from the manual picker but won't be in the default flow.
4. Paste the file's full contents (frontmatter + body).
5. Save.

Skill notes show up in the explorer with an `[sk]` badge.

Once all ten exist, the skill picker on every artifact will offer the
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
| 02 | `02-ba-decompose-features` | epic | feature | BA |
| 03 | `03-ba-decompose-stories` | feature | story | BA |
| 04 | `04-ba-decompose-tasks` | story | task | BA |
| 05 | `05-sa-design-feature-hld` | feature | plan | SA |
| 06 | `06-sa-design-story-lld` | story | plan | SA |
| 07 | `07-sde-implement-task` | task | implementation | SDE |
| 08 | `08-tst-write-tests` | implementation | test_cases | TST |
| 09 | `09-tst-run-tests` | test_cases | test_results | TST |
| 10 | `10-sum-summarize-task` | test_results | summary | Summary |

The numeric prefix on every skill title is what the Cascade workflow's
auto-seeder reads to lay out the default pipeline. Adding a custom
skill `11-deploy-task` with the right title prefix appends it to the
chain automatically — no code change needed. Drop the prefix on a
skill (e.g. retitling to `08-tst-write-tests` → `tst-write-tests`)
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

## Tuning

Skill bodies are prompts. Adjust the calibration sections (Epic count,
Story sizing, Task estimation rules) to match your team's norms. Each
re-run reads the current skill body — no rebuild needed.
