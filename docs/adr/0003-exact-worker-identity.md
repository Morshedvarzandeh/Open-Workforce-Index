# ADR 0003: Identify exact worker configurations, not model names

- Status: Accepted
- Date: 2026-08-07

## Context

A model label alone does not determine capability or cost. Provider endpoint, exact release, harness, tools, system policy, context settings, sampling parameters, and quantization can materially change outcomes. Mutable aliases such as `latest` cannot support reproducible evidence or decisions.

## Decision

`WorkerProfile` is the atomic labor-market identity. Its canonical representation includes:

- provider offering, its canonical provider, and immutable model release identifier;
- harness and harness version;
- execution-relevant inference configuration;
- system-prompt digest, skill-pack version, toolset version, and execution-policy digest.

The canonical representation is hashed into `configurationDigest`. The public store does not trust that caller-supplied digest: it joins the referenced offering to recover the authoritative release and provider, reconstructs and validates the domain `WorkerIdentity`, recomputes SHA-256 over its canonical configuration key, and rejects a mismatch before insertion.

`supportedSkillIds`, the concrete `tools` set, and `privacyClearance` are profile capability and authorization assertions used for routing eligibility. They are not copied into the execution digest as mutable sets. Execution identity instead binds `skillPackVersion`, `toolsetVersion`, and `executionPolicySha256`; changing the underlying capability manifest, tool manifest, or permission policy therefore requires a new version or digest. This keeps an authorization assertion separate from the immutable configuration that actually ran while still making execution-relevant changes identity changes.

Equality for evidence, estimates, quotas, decisions, and outcomes uses this exact identity. Human-readable aliases resolve through time-bounded revision records and are never substituted for the stored identity. Uncertain cross-provider matches use `skos:closeMatch`, not `owl:sameAs`.

## Consequences

The catalog contains more worker rows and providers need careful metadata import. Evidence for a fully disclosed configuration targets the exact worker. Release-level evidence is also permitted when a source does not disclose a full worker configuration, but it enters worker estimates only through an explicit, versioned, uncertainty-increasing transfer policy. Unknown details are never fabricated. Decisions remain reproducible after aliases or endpoints change.

## Invariants

1. An exact model release is distinct from a family, alias, and provider offering.
2. Changing any execution-relevant setting creates a new worker identity.
3. A recorded outcome always points to the exact worker that produced it.
4. The provider and release bound into a worker digest come from its stored offering, not caller labels.
