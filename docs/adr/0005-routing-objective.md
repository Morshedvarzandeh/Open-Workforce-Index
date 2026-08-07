# ADR 0005: Apply hard constraints before confidence-bounded cost ranking

- Status: Accepted
- Date: 2026-08-07

## Context

The cheapest invocation is not the cheapest completed task if it often needs repair, review, or fallback. Conversely, selecting the strongest model for every task wastes cash and quota. A single weighted score can also trade away privacy or mandatory quality without making that failure visible.

## Decision

Routing has two explicit phases.

### 1. Eligibility

Reject a worker when any hard constraint fails: availability, privacy/region/provider policy, required tool or context capacity, cash ceiling, latency ceiling, quota availability, or the required skill's conservative success bound. Every rejection has a machine-readable reason.

For a skill estimate with posterior mean `p` and uncertainty, compare the configured lower confidence bound `p_low` to the task's minimum quality. Do not use the mean alone. Public benchmark priors have capped weight; verified local outcomes update the local posterior.

### 2. Ranking

Rank eligible candidates by integer currency microunits of expected accepted cost:

```text
E[accepted cost] = run_cost
                 + required_review_cost
                 + (1 - p) * fallback_cost
                 + quota_shadow_cost
```

Provider quotas remain separately recorded resource dimensions; their shadow cost is a versioned policy input, not fabricated cash spend. Ties prefer the higher confidence lower bound, then lower latency. The result includes the Pareto frontier and a complete explanation. Maker/checker policy requires distinct exact worker identities when independent verification is mandatory.

## Consequences

The system may select a moderately priced worker over a superficially cheaper one, and may report that no safe candidate exists. Calibration and fallback assumptions need continuous evaluation. The policy is inspectable, testable, and can optimize personal history without exposing it publicly.

## Invariants

1. Privacy, safety, and minimum quality are never compensated by a lower price.
2. Money uses integer microunits; quota, latency, and tokens remain separate quantities.
3. Every quote identifies its snapshot, policy version, assumptions, and exclusion reasons.
