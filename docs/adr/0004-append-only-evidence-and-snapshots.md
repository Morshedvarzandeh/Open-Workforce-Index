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

## Implementation status

v0.1 implements append-only public records, a closed snapshot member manifest
with a verified digest, and private quote audits containing candidates and
exclusions. Its evidence record has an observation time but not yet distinct
publication/retrieval/ingestion times or a typed protocol manifest. Complete
decision replay is a v0.2 release gate: the snapshot still needs an estimator
version, the private decision needs a local-state version, and the request
fingerprint must be computed from canonical request bytes rather than accepted
from an adapter. Until those fields exist, OWI can verify the public evidence
view but does not claim full quote reconstruction.

## Invariants

1. Published identifiers and snapshot digests are never reused for different bytes.
2. Price and availability changes are time-bounded revisions, not overwrites.
3. Before persisted decisions are described as replayable, their snapshot,
   estimator, ontology, private-state, policy, canonical request, and seed
   versions must all be fixed and verified.
