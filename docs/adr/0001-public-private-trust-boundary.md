# ADR 0001: Separate public and private trust domains

- Status: Accepted
- Date: 2026-08-07

## Context

The public index needs model, price, benchmark, and provenance data. Personal allocation needs prompts, repository context, budgets, usage, and outcomes. Combining them makes an export bug a confidentiality incident and weakens the promise that personal learning remains local.

## Decision

Run two physically separate stores and expose narrow interfaces:

- `index.sqlite` is public, rebuildable, and may be exported as RDF or a signed snapshot.
- `local.sqlite` contains task metadata, budgets, decisions, usage, and outcomes. It is private by default and created with owner-only permissions where the platform supports them.
- Credentials and secret-class content are forbidden from both stores. The
  v0.1 private API still contains trusted-adapter, free-form metadata fields, so
  this is currently an input contract rather than a complete type-level
  guarantee. Execution cannot be enabled until those fields are restricted or
  redacted and credentials are supplied only through the operating system's
  runtime secret mechanism.
- Public export code depends only on a `PublicIndexRead` capability. It cannot accept a local-store handle or private domain types.
- SHACL privacy validation and forbidden-predicate queries are release gates, but are defense in depth rather than the security boundary.

Privacy classes are monotonic: `Public < PrivateMetadata < ConfidentialContent < Secret`. A worker is eligible only when its clearance and provider policy permit the task class. Because v0.1 does not model a verifiable local execution boundary, `Secret` tasks fail closed before routing.

## Consequences

There is deliberate duplication of database setup, backup, and migration plumbing. Cross-domain analysis must use explicit, redacted projections. In return, public publishing becomes auditable and private history can be deleted or backed up independently.

## Invariants

1. No prompt, repository identifier, tenant identifier, credential reference, or raw artifact enters a public record.
2. Named RDF graphs are not treated as an access-control mechanism.
3. A public snapshot can be rebuilt without opening `local.sqlite`.
