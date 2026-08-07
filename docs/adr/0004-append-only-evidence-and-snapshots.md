# ADR 0004: Evidence, outcomes, and snapshots are append-only

- Status: Accepted
- Date: 2026-08-07

## Context

The allocator updates as prices, releases, benchmarks, and local outcomes arrive. If historical inputs can be edited in place, past recommendations cannot be reproduced and undesirable changes can be hidden. Benchmark publication time also differs from the time the system learned about it.

## Decision

Treat raw evidence observations, verified outcome events, and published index snapshots as append-only facts.

- Every observation records source URI or digest, observed/event time, ingestion time, metric definition, target identity, sample size when known, and evidence tier.
- Corrections append a new entity linked with `prov:wasRevisionOf`; retractions append a status event. Original facts remain addressable.
- Derived estimates link to all contributing observations with `prov:wasDerivedFrom` and record estimator/policy version.
- A snapshot is an immutable manifest of input identifiers plus ontology and estimator versions. Its canonical bytes receive a content digest.
- Routing decisions store snapshot digest, policy version, all candidate quotes, hard-constraint exclusion reasons, and selected worker.
- Public snapshots include only public evidence. Private outcomes update local estimates but are not silently promoted to public evidence.

## Consequences

Storage grows and consumers must select the latest valid revision rather than update rows. Rebuilding and audit are straightforward, rollbacks select an earlier snapshot, and benchmark freshness can be measured without erasing history.

## Invariants

1. Published identifiers and snapshot digests are never reused for different bytes.
2. Price and availability changes are time-bounded revisions, not overwrites.
3. A routing decision can be replayed against the exact evidence view it used.
