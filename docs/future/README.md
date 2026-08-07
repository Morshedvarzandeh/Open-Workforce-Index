# Deferred designs

These documents describe products that have not been validated and are not
being built yet. They were moved out of [`docs/adr/`](../adr/) because an
architecture decision record is a commitment, and committing to an
architecture before there is a user is how a project acquires design debt it
has to litigate later.

Nothing here is wrong. Each is a reasonable design. The problem is sequencing:

- **0006 — private project usage accounting.** Closest to the core, because
  "did routing actually save money" is the question the project has to answer
  eventually. Blocked on having real decisions to account for.
- **0007 — environmental impact accounting.** Energy, CO2e, and water reporting
  is a separate product. It does not make the routing thesis more or less true.
- **0008 — browser advisor and execution boundary.** Specifies a Chrome MV3
  client down to Native Messaging versus a loopback fallback and its
  DNS-rebinding defence — a threat model for a UI with no users.

The order to bring one back is: when the thing it accounts for exists and
someone is asking for the report. Move the file back into `docs/adr/`,
renumbered to its position in the real sequence, when that happens.

The invariants worth remembering in the meantime are small enough to state
here:

- Usage postings are append-only events, never a mutable cost field on an
  outcome. Corrections append; they do not edit.
- Every report reconciles: the sum of source totals equals the sum of project
  allocations, independently per resource and currency. Unattributed usage
  stays visible as `unallocated` rather than being distributed by guesswork.
- Unknown environmental impact is `unknown`, never zero. Location-based and
  market-based CO2e are alternative views and are never summed.
- Optimizer savings are a `counterfactual_estimate` against a baseline frozen
  at decision time, never reported as observed cash.
- A convenience client is not a new trust domain. The local service stays the
  only component that resolves credentials, validates policy, leases budget, or
  posts usage, and it revalidates every field the client sends.
