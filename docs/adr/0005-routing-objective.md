# ADR 0005: Apply hard constraints before confidence-bounded cost ranking

- Status: Accepted
- Date: 2026-08-07

## Context

The cheapest invocation is not the cheapest completed task if it often needs repair, review, or fallback. Conversely, selecting the strongest model for every task wastes cash and quota. A single weighted score can also trade away privacy or mandatory quality without making that failure visible.

## Decision

Routing has two explicit phases.

### Human-owned policy

The person owns the optimization policy: minimum quality and verification,
cash/quota/time ceilings, allowed providers and data locations, environmental
evidence requirements, and the ranking objective. The allocator recommends the
worker; it cannot silently change the person's thresholds or objective. A human
override is recorded as part of the decision rather than retraining history to
pretend it was the recommendation.

The v0.1 engine implements the `economy` objective described below: minimum
expected accepted-result cost after every hard gate. Planned policies may rank
by quality or latency within a fixed budget, or return a Pareto set across cost,
latency, CO2e, and water. They must remain lexicographic and inspectable; OWI
will not hide unlike units inside one universal weighted score.

### 1. Eligibility

Reject a worker when any hard constraint fails: availability, privacy/region/provider policy, required tool or context capacity, cash ceiling, latency ceiling, quota availability, or the required skill's conservative success bound. Every rejection has a machine-readable reason.

For a skill estimate with posterior mean `p` and uncertainty, compare the configured lower confidence bound `p_low` to the task's minimum quality. Do not use the mean alone. Public benchmark priors have capped weight; verified local outcomes update the local posterior.

Evidence is eligible only for the capability it actually measured. Domain,
task family, artifact/modality, tools, language or jurisdiction, benchmark
version, and exact worker configuration are part of applicability. Transfer to
a broader or neighboring skill requires an explicit versioned mapping and an
uncertainty discount. A legal factuality result cannot qualify a worker for CAD
solid generation, and a science/engineering knowledge score cannot replace a
geometry-kernel execution benchmark.

The generalized applicability key is an atomic tuple of application domain,
task class, artifact type, skill, acceptance profile, and exercised tool
context. This is extensible vocabulary, not a closed list of applications. CAD
is one example; the same rule applies to code, research, images, simulations,
translation, support, and future task families.

The v0.1 Rust record persists skill, benchmark, metric, exact release, optional
exact worker, and provenance; its engine gates declared skills/tools and
caller-supplied task estimates. The structured applicability tuple,
`ScopedAbilityEstimate`, exact-match competency query, and executed SHACL gate
are v0.2 contracts. Until that projection exists, OWI does not claim that
domain/artifact/tool applicability is persisted or automatically inferred.

### 2. Ranking

Rank eligible candidates by integer currency microunits of expected accepted cost:

```text
E[accepted cost] = run_cost
                 + required_review_cost
                 + (1 - p) * fallback_cost
                 + quota_shadow_cost
```

Provider quotas remain separately recorded resource dimensions; their shadow cost is a versioned policy input, not fabricated cash spend. Ties prefer the higher confidence lower bound, then lower latency. The result includes the Pareto frontier and a complete explanation.

The v0.1 maker/checker field reserves a distinct, policy-authorized checker ID
and records a caller-supplied review-cost assumption. It is not yet a complete
checker plan: checker availability, clearance, review-specific skills,
evidence, tariff, and context are not independently quoted. Execution under a
maker/checker policy remains disabled until v0.2 models and validates that
second worker as a full candidate plan.

## Consequences

The system may select a moderately priced worker over a superficially cheaper one, and may report that no safe candidate exists. Calibration and fallback assumptions need continuous evaluation. The policy is inspectable, testable, and can optimize personal history without exposing it publicly.

The v0.1 kernel is deterministic for identical serialized quote input. Full
historical replay is a v0.2 gate and must freeze the canonical request,
estimator, ontology, policy, public snapshot, private-state version, and seed;
recording only a snapshot and policy label is not sufficient.

## Invariants

1. Privacy, safety, and minimum quality are never compensated by a lower price.
2. Money uses integer microunits; quota, latency, and tokens remain separate quantities.
3. Every quote identifies its snapshot, policy version, assumptions, and exclusion reasons.
4. The person selects the optimization policy; the planner and worker cannot
   expand or rewrite it.
5. Evidence never grants an unmeasured skill through a global model score.
