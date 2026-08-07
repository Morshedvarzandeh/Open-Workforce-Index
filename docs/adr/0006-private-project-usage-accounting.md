# ADR 0006: Reconcile private AI usage to Git projects before reporting savings

- Status: Accepted
- Date: 2026-08-07

## Context

Users need to know which local Git project consumed AI resources, which exact
workers caused the consumption, and how retries, verification, tools, and
subscriptions affected the total. Provider invoices and runtime telemetry can
arrive at different times and at different levels of detail. A task can also
span several attempts or projects.

A single mutable `actual_cost` field cannot represent this history. It cannot
preserve corrections and refunds, prove that project totals equal provider
totals, or distinguish observed spend from an optimizer's hypothetical
baseline. Repository URLs and paths are private metadata and must not cross the
public-index boundary.

## Decision

### Private project identity

Project accounting exists only in `local.sqlite`. A random, local `ProjectId`
is the reporting boundary. One project can contain multiple independently
registered Git repositories or clones, and one repository can contain multiple
worktrees.

Repository identity is not derived from a remote URL, path, or commit. A remote
is only a linking hint. OWI canonicalizes it after removing credentials and
stores a keyed HMAC, never the URL itself. Local common-directory and worktree
paths are also represented by keyed fingerprints. A new clone is linked to an
existing project only through an explicit user action; matching remotes is not
sufficient because forks and mirrors are ambiguous.

Each execution records its start and end Git context: repository and worktree
IDs, object format, HEAD, an explicit base commit when one exists, dirty state,
and a keyed state fingerprint. It does not persist patches, filenames, prompt
content, or repository contents. Git context supports attribution and audit;
changed lines are never used as a cost-allocation heuristic.

### Usage ledger

Every chargeable or limited action creates a private usage event linked to an
execution attempt and, when known, an exact worker and offering. Attempts have
explicit roles such as maker, retry, fallback, checker, deterministic verifier,
tool, planner, or router. Historical events without trustworthy execution
context remain in a visible `unallocated` bucket instead of being guessed from
timestamps or the current directory.

A usage event contains one or more source totals and their project
allocations. Resource dimensions remain separate. They include cash by
currency, input/cached-input/output tokens, provider quota, wall/CPU/GPU time,
and other typed quantities. Amounts are signed integers so refunds, credits,
settlements, and corrections can be appended without editing history. Cash is
stored in integer micros.

An event becomes reportable only after an atomic posting operation verifies:

1. every source total has at least one allocation;
2. allocations use the same resource, unit, and currency as their source;
3. the signed allocations sum exactly to the signed source amount;
4. task, attempt, project, worker, and provenance references are valid; and
5. the canonical event digest and previous-ledger hash are valid.

Posted events, totals, allocations, and posting records reject update and
delete. A correction is a new signed event that references the earlier event.
Provider imports use a keyed deduplication fingerprint. Reports select only
posted events and identify both an occurred-time range and a posting-sequence
watermark, so a later backfill does not silently change a reproduced report.

For every resource and currency at a reporting watermark, the portfolio
invariant is:

\[
\sum source\ totals = \sum allocations\ across\ all\ projects
\]

Project totals and every model, task, provider, component, and attempt-role
breakdown are calculated from the same allocation rows. A report is invalid if
any reconciliation delta is non-zero.

### DAGs, retries, review, and tools

A task node declares its billing project or a versioned allocation policy.
Direct work is allocated completely to one project. Shared planner, router, or
coordination work uses explicit rational weights. Integer remainders are
distributed deterministically, and a correction reverses the original
allocations rather than recomputing them.

All attempts count, including unsuccessful makers, retries, fallbacks,
independent model checkers, and paid tools. Failed tasks remain in project
spend. Human-review time remains a separate resource unless the user provides
an explicit, versioned labor valuation. Optimizer and planner overhead is part
of optimized usage; omitting it would overstate the benefit of routing.

### Tariffs, invoices, and subscriptions

Provider-reported billed cash is stronger evidence than a tariff estimate.
Token-rate accruals are marked provisional. When an invoice or credit arrives,
OWI posts the signed settlement difference so the cumulative ledger reconciles
to the bill. Gross charges, discounts, refunds, tax, and net cash remain
inspectable. Different currencies are not added without an explicit,
versioned exchange-rate policy.

A subscription fee is posted once as actual organization cash in an overhead
bucket. Per-call marginal cash may be zero, but requests, tokens, and quota are
still reported. By default the fee remains overhead. If the user chooses a
versioned attribution policy, OWI posts a zero-total reclassification: a
negative overhead allocation and positive project allocations. This changes
project attribution, not cash incurred. Reports therefore distinguish direct
cash, allocated subscription overhead, quota, and any quota shadow value.

### Counterfactual savings

OWI never describes a hypothetical baseline as observed cash savings. A
baseline must be frozen at decision time and identify the same task contract,
quality gate, evidence and tariff snapshots, currency, verification policy, and
fallback assumptions as the optimized quote.

The decision-time forecast is:

\[
estimated\ avoided\ cash = baseline\ expected\ cash
- selected\ expected\ cash
\]

Cash comparisons use the quote's cash component, not a value that includes
quota shadow prices. Quota and other resource differences are reported
separately. After execution, OWI may compare a frozen baseline estimate with
reconciled optimized spend, but the result remains a `counterfactual_estimate`
with its method, version, uncertainty interval, coverage, and exclusions. It is
not renamed to cash saved.

If both strategies actually run on a paired task, OWI reports an
`observed_cost_difference`; both costs were incurred. Randomized comparisons
may estimate a causal effect with an interval. Before/after invoice movement is
only a trend because workload, prices, and task mix may also have changed.

Environmental impact uses the same project and usage references, but its
measurement boundaries, factors, uncertainty, and labels are a separate
accounting concern. This decision does not define or equate estimated energy,
carbon, or water with provider-billed usage.

## Consequences

The private schema and provider adapters are more involved than storing one
cost on an outcome. In return, project reports can be reproduced and reconciled
exactly, corrections remain visible, subscriptions are not treated as free,
and estimates cannot masquerade as cash transactions. Some historical usage
will remain unallocated or provisional when a provider does not expose enough
telemetry; OWI reports that limitation instead of fabricating precision.

## Invariants

1. Project identity, Git metadata, usage, and reports never enter a public
   snapshot or RDF export.
2. Posted ledger history is append-only; corrections and reclassifications are
   new signed events.
3. Allocations reconcile exactly to source totals for every resource, currency,
   and posting watermark.
4. Retries, verification, tools, planner overhead, failures, credits, and
   subscription attribution remain visible.
5. Currency, quota, time, compute, and environmental quantities are not
   silently collapsed into one cash value.
6. Optimizer benefit is labeled as a counterfactual estimate unless supported
   by an explicitly identified experimental design.
