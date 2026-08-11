# OWI Office Art Pack v1

This directory is the reusable graphics boundary for the OWI office interface.
It keeps art independent from HTML and CSS so a browser UI, mobile shell, or a
future skin can consume the same files without copying them.

The pack starts in `development` state. Its registered artwork carries file,
provenance, dimensions, checksum, accessibility, placement, and license-state
metadata. Release validation remains closed until every pending rights review
is explicitly approved.

## Directory contract

```text
v1/
├── manifest.json          Runtime asset and skin catalog
├── manifest.schema.json   JSON Schema for the catalog
├── SOURCES.json           Human/audit provenance ledger
├── LICENSE.md             Licensing and redistribution rules
├── validate.py            Dependency-free contract validator
├── backgrounds/           Full-scene and room backgrounds
├── characters/            Transparent character cutouts and poses
├── props/                 Furniture, plants, devices, and small objects
├── skins/                 Skin-specific overlays and decorations
└── sources/               Editable source files and generation notes
```

Only `manifest.json` is needed by a runtime. `SOURCES.json`, `LICENSE.md`, and
the preferred editable files in `sources/` travel with redistributed packs.

## Asset record

Each object in `manifest.json.assets` represents one semantic asset and must
contain:

- `id` and `revision`: stable identity and SemVer revision;
- `kind`: `background`, `character`, `prop`, `effect`, `ui-decoration`, or
  `skin-overlay`;
- `skin_ids`: skins in which the asset is allowed;
- `title`, `alt_text`, and `decorative`: accessibility-facing semantics;
- `brand_status`: must be `original-unbranded` for this base pack;
- `placement`: scene, slot, layer, anchor, normalized focal point, fit, and a
  suggested z-index;
- `source_record_ids`: provenance records from `SOURCES.json`;
- `license`: SPDX expression, approval state, attribution, and evidence;
- `renditions`: one or more physical `master` or `runtime` files with media
  type, pixel dimensions, density, alpha information, byte count, and SHA-256
  digest. Consumers should prefer `runtime`; masters remain reusable inputs.

Character assets additionally use a `character` object. It records a stable
fictional character ID, character type, age class, office role, pose,
expression, state, and look direction. Human characters must use the `adult`
age class; non-human characters use `not-applicable`. Character names and
artwork must stay fictional and must not impersonate a model, vendor, product,
or real person.

Coordinates in `placement.focal_point` are normalized `[x, y]` values from
`0.0` to `1.0`, measured from the top-left corner. Runtime layout remains in
CSS; placement metadata is a portable hint, not absolute positioning.

## Naming and versions

- Use lowercase kebab-case paths and IDs, for example
  `character-bowl-dispatcher-idle`.
- Use descriptive fictional names. Do not put model, vendor, product, or real
  person names into the artwork or asset ID.
- Never replace a published file in place. Change the asset `revision`, use a
  new filename, and update its checksum.
- `pack_version` follows SemVer. Metadata fixes are patches; backward-compatible
  assets or skins are minors; removed or incompatible IDs require a new major
  pack directory.
- The top-level `v1` directory is the runtime contract major. A v2 consumer
  must not assume v1 fields or meanings.

## Adding artwork

1. Export a runtime image into the matching category directory. Keep an
   editable source or generation record in `sources/` when one exists.
2. Add a record to `SOURCES.json`. Record who or what created it, when, all
   inputs, the exact prompt or a prompt file plus digest for generated work,
   every generated/intermediate/runtime artifact, and the rights review. Do
   not invent missing provenance.
3. Add the semantic asset and rendition metadata to `manifest.json`.
4. Calculate metadata, for example:

   ```bash
   wc -c < characters/character-bowl-dispatcher-idle-v1.png
   sha256sum characters/character-bowl-dispatcher-idle-v1.png
   ```

5. Run the local validator:

   ```bash
   python3 ui/assets/office/v1/validate.py
   ```

6. Before publishing a pack, run strict release checks:

   ```bash
   python3 ui/assets/office/v1/validate.py --release
   ```

Release mode rejects an empty pack, non-approved licenses, missing source
rights reviews, missing files, byte or digest mismatches, unknown skins, and
invalid declared PNG dimensions.

## Art direction and safety

The default skin is a bright, welcoming, mobile-game-style office: readable
silhouettes, warm daylight, cheerful color, and an original blue bowl-shaped
robot dispatcher. Human characters are explicitly adults with varied
professional personalities. Keep the scene playful without casino imagery,
loot-box cues, artificial urgency, fake currency, or fabricated performance,
cost, sustainability, or savings numbers.

Artwork must be original and unbranded. Do not include provider marks, model
names, logos, copyrighted game characters, real people, or UI text that could
be mistaken for measured OWI data. Decorative artwork should set
`decorative: true` and use an empty `alt_text`; meaningful artwork should use
concise, literal alt text.

## Skins

`skins` is a registry, not a second folder hierarchy. A skin can inherit the
base layout contract and select assets by `skin_ids`. The `bright-office` skin
is the v1 default. Future skins should be added with a new skin ID and their
own assets; they must not overwrite base files.

Keep behavior, live model identity, task output, cost, and evidence in the
application layer. This directory contains presentation assets only.
