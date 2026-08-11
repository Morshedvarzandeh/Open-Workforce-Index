# OWI Office game graphics pack v2

This directory is the reusable presentation boundary for OWI's bright office,
manager scenes, and first-project training tour. It holds intrinsic artwork and
rights/provenance records. Scene placement belongs in `ui/scenes`; tour words
and progression belong in `ui/tours`; model selection, execution, prices,
receipts, and evidence remain server-owned application data.

## Contract

- `manifest.json` is the runtime catalog. It gives every semantic asset a
  stable ID, state, skin, accessible meaning, rendition metadata, checksum,
  loading class, and explicit rights state.
- `SOURCES.json` is the provenance ledger. It records the exact training-map
  prompt and the byte-identical import of the four v1 runtime images.
- `manifest.schema.json` and `sources.schema.json` are the machine-readable
  v2 contracts.
- `validate.py` verifies schemas are parseable, references are closed, paths
  cannot escape the pack, files match declared bytes/checksums/dimensions,
  critical runtime art stays within budget, tour and scene contracts resolve,
  and release rights are approved.
- `backgrounds/` and `characters/` contain runtime images. Future assets use
  `props/`, `effects/`, `ui/`, and `overlays/` rather than mixing layout into
  a bitmap.

The v2 pack deliberately does not reuse v1 files by relative path. Its four v1
runtime renditions are byte-identical copies with inherited pending rights, so
the v2 pack can be installed, cached, and validated as one self-contained
unit. Never overwrite a registered file: add a new filename and increment the
asset revision.

## Layer and state boundary

Scenes use fixed z-index bands: background `0–99`, environment `100–199`, rear
props `200–299`, characters `300–399`, front props `400–499`, effects
`500–599`, HUD `800–899`, tutorial shade `900–909`, spotlight `910–919`, coach
`920–949`, and toast `990–999`. A runtime should put each scene in an isolated
stacking context.

Character assets may declare `idle`, `assigned`, `working`, `waiting-review`,
`accepted`, `needs-work`, `blocked`, or `offline`. This development slice only
publishes the existing idle cutouts. A renderer must use the idle image as an
honest fallback until a separately registered state asset exists; it must not
fake completion or worker quality from animation.

## Mobile behavior

The scene files provide four layout profiles: phone portrait, phone landscape,
tablet, and desktop. Runtime HTML remains responsible for at least 44 px touch
targets (48 px primary actions), `env(safe-area-inset-*)`, `100dvh`, software
keyboard/visual viewport movement, focus order, and a compact phone crew
carousel. Text, worker identities, prices, checks, and outputs stay as HTML,
never embedded in artwork.

## Validation and rights

Run development checks with:

```bash
python3 ui/assets/office/v2/validate.py
```

Release mode is intentionally closed today:

```bash
python3 ui/assets/office/v2/validate.py --release
```

Every generated or copied bitmap is still marked `pending`. A maintainer must
record the rights owner, evidence, reviewer, review date, and license expression
before release mode can pass. Technical quality, generation history, or an
Apache-2.0 repository license must never be presented as an art-rights approval.
