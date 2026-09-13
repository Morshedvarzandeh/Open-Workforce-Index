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

For ask-page interaction changes, run `node tools/check_ask_guidance.cjs`
with Node.js, Playwright, and Chromium available. The browser scenarios use
local fixtures and mocked server responses; they never call a model.
`OWI_CHROMIUM_PATH` can select an existing Chromium executable.

For billing, runner telemetry, or adaptive-instruction changes, also run
`python3 tools/test_runtime.py` and `python3 tools/owi-selftest --no-cargo`.
The runtime suite uses local fake commands and a temporary server; it makes
no provider calls.

For MCP or provider integration changes, run `python3 tools/test_integrations.py`.
With the engine built, also run
`OWI_BINARY="$PWD/target/debug/owi" python3 tools/check_bridge_engine.py`.
Both use fake model responses. The latter validates actual Rust imports,
worker identities, allocation and recorded deterministic feedback.

For desktop packaging, run `python3 tools/test_desktop.py`. Build the release
engine, install `packaging/requirements.txt` in a build virtual environment,
then run `python packaging/build.py --engine target/release/owi` (use `.exe`
on Windows). Run `python packaging/check_bundle.py` to exercise the actual
extracted download with developer tools absent from PATH. The desktop workflow
also opens the packaged setup window on all four supported platforms before
publishing a preview. Build prerequisites never become user prerequisites.

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
