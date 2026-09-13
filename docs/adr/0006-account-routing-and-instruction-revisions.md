# Account-aware routing and bounded instruction revisions

Status: implemented in the simple Python/HTML runtime.

## Decision

Keep the deterministic Rust eligibility result intact. Apply a separate,
recorded account policy to its eligible candidates, with cash estimates and
declared subscription allowance kept distinct. Invalid, stale, or exhausted
declarations remove a subscription execution route. Changes never buy credit
or upgrade accounts, and subscription runs cannot silently escalate to API
billing.

Persist command telemetry in a private SQLite ledger with nullable unknown
values and distinct API equivalents, adapter-reported charges, and unknown
invoice charges. The old quality ledger's numeric compatibility fields are
not authoritative for spending.

Implement self-updating agents as versioned, catalog-bound working reminders.
Deterministic failed checks can start an online probation revision. Future
independent checks govern promotion and rollback. A revision cannot edit
policies, code, tools, or billing, and cannot lower a quality/privacy gate.

## Consequences

Consumer allowance remains manually declared until a supported provider
interface is available. Price snapshots and model releases do not refresh
automatically. Current task estimates remain approximate; observed cache
counters are recorded without inventing cache usage for future tasks.

Probation changes behavior during normal user-started tasks, so all checks
remain enabled and results still need review. Successful software tests prove
the update mechanism, not that reminders improve every model's performance.
The implementation makes no model-weight training claim.

See [usage and plans](../USAGE_AND_PLANS.md) and
[adaptive agents](../ADAPTIVE_AGENTS.md) for the user-visible contracts.
