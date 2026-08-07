# ADR 0002: SQLite ledgers are authoritative; RDF is a projection

- Status: Accepted
- Date: 2026-08-07

## Context

Routing requires transactions, uniqueness constraints, migrations, and inexpensive local operation. The knowledge graph is valuable for ontology alignment, provenance traversal, and questions that span models, workers, skills, and evidence. Making both writable systems authoritative would create conflict and partial-failure modes before the product has earned that complexity.

## Decision

Use SQLite as the system of record in each trust domain. Build RDF deterministically from committed public records and immutable SQLite snapshot rows.

- SQLite uses foreign keys, strict tables where available, integer currency microunits, and explicit schema versions.
- Public source records are reviewable JSON or database inserts produced by deterministic importers; generated database files are not committed.
- The RDF projection uses the stable `https://openworkforce.dev/ns#` namespace and immutable ontology version IRIs.
- Oxigraph is initially an in-memory read model. It is populated from a snapshot, then discarded or rebuilt; application code never dual-writes SQLite and RDF.
- OWL/RDFS, SKOS, DCAT, and PROV-O express meaning. SHACL checks release-time structural and privacy constraints. Routing safety does not rely on open-world inference.

## Consequences

SPARQL results lag until projection refresh and projection bugs require regeneration. The same snapshot digest lets operators reproduce a graph exactly. A persistent graph store may be introduced only after measured competency-query workloads justify it.

## Invariants

1. Every RDF resource that affects routing is traceable to a public snapshot record.
2. Projection is idempotent for a given snapshot digest and ontology version.
3. No application transaction commits separately to SQLite and RDF.
