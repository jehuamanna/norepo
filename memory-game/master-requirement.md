---
artifact_kind: master_requirement
status: approved
---

# Master Requirement: Memory Match

## Charter

Build a single-player memory card-matching game playable in the
browser. The player flips two cards per turn trying to find matching
pairs; the game tracks moves and elapsed time, and stores a personal
best per board size. Target audience is casual players on desktop and
mobile web; no login required.

The product must launch with three difficulty levels (4×4, 6×6, 8×8)
and at least one card-art theme. Subsequent releases add multiplayer
and theme packs.

## Inputs from CE

> "Want it to feel snappy — card flip under 200ms, no loading
> spinner between rounds." — design lead, kickoff transcript

> "Highscores need to survive a browser refresh but we are NOT
> standing up an auth system in v1." — PM, scoping doc

> "Mobile-first. Most of our pilot users are on phones." — pilot
> coordinator, intake form

> RFP snippet: "Accessible to WCAG AA. Keyboard navigation required;
> screen-reader card announcements optional in v1 but flagged for v2."

> Competitive note: a similar match-pairs game from $COMPETITOR was
> reported to feel laggy on iPhone 11; pilot users specifically
> called out wanting "instant" feedback.

## Constraints carried forward

- No backend / no auth (compliance hard line from PM).
- WCAG AA color contrast minimum.
- Bundle size budget: 150 KB gzipped (mobile-first).
