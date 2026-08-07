# ADR 0003: Identify exact worker configurations, not model names

- Status: Accepted
- Date: 2026-08-07

## Context

A model label alone does not determine capability or cost. Provider endpoint, exact release, harness, tools, system policy, context settings, sampling parameters, and quantization can materially change outcomes. Mutable aliases such as `latest` cannot support reproducible evidence or decisions.

## Decision

`WorkerProfile` is the atomic labor-market identity. Its canonical representation includes:

- provider offering and immutable model release identifier;
- harness and harness version;
- ordered, versioned tool capabilities and permission policy;
- execution-relevant inference configuration;
- data clearance and locality constraints.

The canonical representation is hashed into `configurationDigest`. Equality for evidence, estimates, quotas, decisions, and outcomes uses this exact identity. Human-readable aliases resolve through time-bounded revision records and are never substituted for the stored identity. Uncertain cross-provider matches use `skos:closeMatch`, not `owl:sameAs`.

## Consequences

The catalog contains more worker rows and providers need careful metadata import. Evidence does not silently transfer between configurations; an estimator may pool it only through an explicit, versioned statistical policy. Decisions remain reproducible after aliases or endpoints change.

## Invariants

1. An exact model release is distinct from a family, alias, and provider offering.
2. Changing any execution-relevant setting creates a new worker identity.
3. A recorded outcome always points to the exact worker that produced it.
