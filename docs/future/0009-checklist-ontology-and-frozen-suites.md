# ADR 0009: Promote checklist items into the ontology once lived data names the categories

- Status: Proposed
- Date: 2026-08-07

## Context

Verification is now checklist-driven: a task carries acceptance criteria, each
item is labelled by kind (`contains`, `regex`, `json`, `python`, `min-words`,
`judged`), verified mechanically or by an independent judge, and every item's
verdict — pass and fail alike — persists in the append-only ledger's outcome
metadata and, for failures, in the prevention memory. Nothing is lost.

What the items are *not* yet is first-class: the ontology knows workers,
skills, offerings, and outcomes, but a checklist item is a labelled string
inside metadata. That leaves real questions unanswerable by query: which item
kinds does a given worker fail most? Does a model that passes `json` items on
one skill pass them on another? Which free-form items recur often enough to
deserve a mechanical form?

## Decision

Two-stage, deliberately.

**Stage one (done): capture everything, promote nothing.** Labelled per-item
verdicts ride in outcome metadata; failures additionally land in
`root-causes.jsonl` with their checklist; the frozen suite
(`examples/frozen-checks-v*.json` + `tools/owi-selftest`) pins the engine's
behaviour so automation growth cannot silently lose it. This is cheap,
append-only, and reversible.

**Stage two (this ADR): ontology promotion, gated on volume.** When the
ledger holds enough item verdicts for categories to be visible in the data
rather than invented, add to the ontology:

- `CheckItemKind` — the closed taxonomy of mechanical kinds plus `judged`,
  versioned like every other ontology enum;
- a `check_item` dimension on outcome evidence, so posteriors can be computed
  per worker × skill × item-kind with the same Beta machinery and the same
  root-cause gating used everywhere else;
- judge attribution: the judging worker recorded per judged item, so judge
  reliability itself becomes measurable evidence (the maker-is-not-checker
  rule already enforced by the store makes this well-defined).

The trigger is evidence, not enthusiasm: promote when recurring item
categories appear across at least two skills and two workers, because a
taxonomy frozen before the data exists would freeze today's guesses.

## Consequences

- No idea is lost meanwhile: stage one's metadata is exactly the dataset that
  designs stage two's taxonomy, and it is already being collected.
- Per-item posteriors will let the allocator gate on *specific* weaknesses
  ("this worker's `json` items fail under this skill") instead of whole-skill
  averages — finer than today, using machinery that already exists.
- The frozen suite becomes the compatibility contract: each ontology
  promotion bumps the frozen-checks version, and the old expectations remain
  runnable so a regression is a diff, not an archaeology project.
