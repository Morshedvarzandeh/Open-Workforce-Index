# Contributing

Thank you for helping build a transparent AI workforce allocator.

## Licensing contributions

OWI uses the GNU Affero General Public License, version 3 only
(`AGPL-3.0-only`); see [LICENSE](LICENSE). By intentionally submitting a
contribution for inclusion in OWI, you agree to license it under these terms
unless a separate arrangement is explicitly agreed. Only submit material you
have the right to contribute, and retain any required third-party notices.

## Development

Use Rust 1.87 or newer, then run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
cargo test --workspace --all-targets
```

Keep changes narrow and include tests for routing, money, identity, privacy, or
storage invariants. New architectural decisions belong in `docs/adr/`.

## Adding public evidence

Do not paste a leaderboard number without its protocol. A record must identify:

- the exact model release and, when the source discloses it, the provider
  offering, harness, inference configuration, and tool permissions;
- benchmark/dataset and revision;
- prompt/protocol digest, attempts or seeds, metric, unit, and sample count when
  reported;
- source URL, publication/retrieval time, content digest, and license;
- whether the result is vendor-reported, independently reproduced, or signed.

Unknown worker configuration or sample size stays unknown. Never fabricate an
exact worker merely to make release-level evidence fit the schema; the
estimator must transfer such evidence with lower confidence.

Store a link and digest rather than copying a large third-party dataset unless
its license explicitly permits redistribution. AGPL-3.0-only covers OWI code and
project-authored data unless separately licensed; it does not relicense imported
benchmark material. Keep evidence-record and test-fixture source-license values
intact: they describe the represented source, not the license of OWI itself.

Environmental factors additionally require an exact offering or deployment,
functional unit, scaling rule, lifecycle phase, measurement boundary, units,
geography/time applicability, and provenance. Do not convert a median prompt
or provider fleet average into a per-token or model-specific factor without
source-supported applicability. Unknown impact stays unknown; location- and
market-based CO2e and water withdrawal/consumption remain separate.

## Design rules

- Never use marketing names or mutable aliases as stable IDs.
- Never collapse unlike benchmarks into an unexplained global score.
- Never use floating-point values for cash.
- Never hide an eligibility failure inside a weighted score.
- Never add private task/repository fields to the public read/export interface.
- Prefer a transparent baseline and measured calibration over premature ML.
