# Contributing

Thank you for helping build a transparent AI workforce allocator.

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

- exact model release, provider offering, harness and tool permissions;
- benchmark/dataset and revision;
- prompt/protocol digest, attempts or seeds, metric, unit, and sample count;
- source URL, publication/retrieval time, content digest, and license;
- whether the result is vendor-reported, independently reproduced, or signed.

Store a link and digest rather than copying a large third-party dataset unless
its license explicitly permits redistribution. Apache-2.0 covers OWI code and
project-authored data; it does not relicense imported benchmark material.

## Design rules

- Never use marketing names or mutable aliases as stable IDs.
- Never collapse unlike benchmarks into an unexplained global score.
- Never use floating-point values for cash.
- Never hide an eligibility failure inside a weighted score.
- Never add private task/repository fields to the public read/export interface.
- Prefer a transparent baseline and measured calibration over premature ML.
