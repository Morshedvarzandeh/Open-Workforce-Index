# ADR 0010: Make project mapping a generic OWI client

- Status: Proposed
- Date: 2026-10-04

## Context

People need one place to see how a project's parts, APIs, agents, schedules,
work and cost evidence relate, and to understand which parts a source change
can reach.  An initial project-specific prototype proved that this is useful,
but its component registry, workflow names and domain vocabulary cannot become
an Open Workforce Index product.  OWI must accept any project without importing
that prototype's domain, simulations, schedules or copied price data.

The browser is not allowed to become a repository, credential or spending
authority.  A repository is also untrusted input: a checked-in declaration may
describe a component, but must not gain the power to execute a command merely
because a person opened the map.

## Decision

OWI adds a separate, private `project map` boundary.  It does not overload the
existing `.owi/project.json` work-staffing manifest and does not enter the
public workforce index or RDF projection.

An optional `.owi/map.json` declaration supplies project-neutral lanes,
components, owned repository-relative paths and directed relationships.  The
declaration is data only.  It has no structured command, credential, local
absolute-path, provider-request or executable-action fields.  Human-readable
names, descriptions and facts remain repository-supplied display text and are
not treated as authority or secret-scanned.

The local OWI host combines that declaration with bounded adapters and emits a
versioned `owi.system-map.v1` snapshot.  The initial adapters are:

- Git working-tree changes and Git-style text-line counts;
- declared ownership and relationships;
- local OWI runtime state, agent revisions and runner-reported usage.

Lifecycle and change impact are separate fields.  `implemented`, `declared`
and `planned` describe lifecycle.  `changed`, `affected`, `unchanged` and
`unavailable` describe one comparison.  Only directly changed components own
line additions and deletions; affected components show the dependency path and
never inherit line counts.

Evidence retains its basis.  Published prices, planning estimates,
runner-reported usage and reconciled invoices are not interchangeable.  An
unknown amount is `null`, never zero.  A declared schedule does not prove that
it is active, and an active schedule does not prove a completed run.
The initial runtime adapter reports a shared local-runtime window, not spend
attributed to the selected repository.  It exposes coverage so a sum with
missing run values is `partial`, and the client labels shared runtime actions
as affecting more than the currently selected map.

The browser client renders one full-page map so the same application can be
packaged for Chromium and Firefox without making a browser-specific sidebar
the primary layout.  It is an untrusted client.  It receives opaque project,
snapshot and action IDs plus normalized repository-relative source references.
It receives no structured absolute repository root, remote URL, command,
credential, prompt or model output field.  Repository-supplied display strings
are forwarded verbatim and may themselves mention arbitrary text or embedded
paths, so users must not put secrets in the map declaration.

The browser remains untrusted for repository data, action definitions and
authority expansion.  Native-host installation nevertheless allowlists the
exact owner-installed extension identity, so that extension's confirmation
dialog is the approval surface; repository content and ordinary web pages
cannot manufacture the confirmation channel.

Management actions are supplied only by trusted OWI service adapters.  A
repository declaration cannot create one.  The first writable actions are
limited to pausing or resuming local instruction learning and rolling back an
agent revision.  Every mutation requires an explicit click, confirmation, an
expected-current-state precondition and an audited result.  The first release
does not run a model, trigger a provider, change Git, install a schedule or
spend money.

The adapter boundary is:

```text
discover(project)    -> graph fragment
observe(project)     -> evidence fragment
listActions(project) -> trusted action descriptors
invoke(action)       -> audited receipt and a new snapshot
```

Native Messaging remains the distribution transport described by the deferred
browser-advisor design.  A loopback server may be used for a source-checkout
preview, but it is not the extension's production authority and must remain
loopback-only, token-bound, same-origin and length-limited.

## Initial contract

The declaration contains `schemaVersion`, `project`, `lanes`, `components` and
`relations`.  Component ownership accepts normalized repository-relative exact
paths or directory prefixes only.  IDs are stable and unique; relationships
must reference existing components.

The generated snapshot contains:

- immutable snapshot/comparison metadata and adapter versions;
- lanes, nodes and typed edges;
- changed-file evidence with text, binary or unavailable line counts;
- evidence records with explicit claim state and measurement basis; and
- trusted action descriptors with authority, approval and availability.

Unknown declaration fields, native request fields, top-level snapshot fields
and action fields are rejected at their boundaries.  The client may ignore a
new nested display-only snapshot field, but it cannot infer authority from it.

## Acceptance gates

Before this record can become `Accepted` and the client can replace any
existing OWI surface:

1. strict fixtures reject duplicate IDs, dangling edges, unsafe paths,
   executable repository actions and unknown schema versions;
2. changed-line and dependency propagation tests prove that indirect impact
   never becomes direct churn;
3. action tests reject unknown IDs and stale expected state and prove that the
   initial actions cannot run a command or contact a provider;
4. browser tests render a repository-neutral fixture and the extension
   manifests request no page, cookie, history, clipboard or broad host access;
5. absolute roots, remotes, prompts, output and credentials are absent from
   the client snapshot; and
6. packaged-client tests pass without a developer Python or Rust installation.

## Consequences

Any project can describe its architecture without adopting another project's
vocabulary.  Useful Git and OWI runtime evidence can be shown locally while
unknowns remain honest.  The data declaration stays reviewable and portable,
but it cannot silently acquire execution authority.

The initial client is deliberately less powerful than a general repository
control plane.  Provider execution, arbitrary project commands, schedule
creation, reconciled project billing, page scraping and remote multi-user
hosting require later records and their own security gates.

## Invariants

1. No domain-specific component, worker, schedule or simulation is built into
   the generic map.
2. Repository declarations describe; trusted OWI adapters authorize.
3. Direct changes carry churn; indirect impact carries paths.
4. Unknown cost and usage are never displayed as zero.
5. The client cannot expand repository, tool, credential or spending scope.
