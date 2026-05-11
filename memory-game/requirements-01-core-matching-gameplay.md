---
artifact_kind: requirements
status: pending
---

# Requirement: Core matching gameplay

## Source

> "Build a single-player memory card-matching game… The player flips
> two cards per turn trying to find matching pairs."
> — Master Requirement, Charter

## Capability

Player can start a new round, flip cards two at a time, see matched
pairs persist face-up, and see unmatched pairs flip back after a brief
preview. A round ends when every pair is matched; the UI surfaces a
"You won!" panel with final move count and elapsed time.

## Acceptance criteria

- Given a new 4×4 round, when the player clicks one face-down card,
  then the card flips face-up within 200ms.
- Given one card is face-up, when the player clicks a second face-down
  card and the two match, then both cards stay face-up and the move
  counter increments by one.
- Given one card is face-up, when the player clicks a second face-down
  card and the two do NOT match, then both cards stay face-up for
  ~700ms (preview window), then flip back down; the move counter
  increments by one.
- Given the last unmatched pair is solved, when the second card flips,
  then a win panel appears showing the final move count and elapsed
  time.
- Given a round in progress, when the player clicks a card that's
  already face-up, then nothing happens (no flip, no counter
  increment).

## Constraints

- Flip animation budget: 200ms.
- Preview-before-flip-back window: 600–800ms (tune per user testing).
- No network call during a round — round state is fully client-side.

## Stakeholders

- Casual player (primary)
- QA (acceptance verification)
- Design (animation tuning)

## Out of scope

- Difficulty levels other than 4×4 (covered in
  `requirements-02-difficulty-and-themes`).
- High-score persistence (covered in
  `requirements-03-personal-best-tracking`).

## Depends on

None (parallel-safe)

## Revision history

| Revision | Date       | Derived from        | Summary       |
|----------|------------|---------------------|---------------|
| 1        | 2026-05-11 | master_requirement  | Initial draft.|
