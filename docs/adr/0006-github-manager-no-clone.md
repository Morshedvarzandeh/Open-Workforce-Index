# ADR 0006: GitHub Manager is a read-only source adapter

- Status: accepted for the local MVP
- Date: 2026-08-11

## Context

The owner needs one Manager view across repositories, issues, pull requests,
failed Actions runs, OWI tasks, staffing, outputs, and evidence. Requiring the
owner to clone every repository defeats that experience. GitHub content is also
untrusted input: an issue body must not be able to select a model, grant tools,
approve spend, or write back to GitHub.

## Decision

The first GitHub Manager is a server-side, on-demand REST source adapter.

- It discovers repositories through GitHub and exposes opaque repository IDs
  to the browser. The browser cannot supply a URL or filesystem path.
- Public-owner mode can observe public metadata without credentials. Private
  access uses a server-only GitHub App user access token or a fine-grained
  personal token for personal testing. The local v1 adapter does not accept a
  bare installation access token. Token values never enter HTML, JSON
  responses, URLs, logs, SQLite,
  prompts, provider runners, or the public index.
- The adapter reads repository metadata, open issues, open pull requests, and
  recent failed Actions runs. It does not clone, download archives or source
  files, inspect repository contents, or write to GitHub.
- Refresh is manual and labelled as an on-demand snapshot. Responses preserve
  observed time, source update time, pagination/truncation, stale/error state,
  and rate-limit evidence. “Selected”, “imported”, “staffed”, and “completed”
  are separate states.
- Import is an explicit owner action referencing a cached stable GitHub item
  identity. It creates only an unassigned local draft and its provenance.
  Import never invokes a planner, allocator, worker, verifier, provider,
  payment, or GitHub mutation. Repeated import is idempotent.
- Private-source tasks have a minimum `confidential_content` privacy class.
  Local edits are not overwritten by later source refreshes.

## Security boundary

Outbound requests are restricted to HTTPS GitHub API hosts and validated path
segments. Redirect and pagination targets are revalidated before credentials
are forwarded. Requests have bounded time, pages, items, and response size.
Rate-limit responses stop immediately and are surfaced without automatic
retry. GitHub titles, bodies, labels, logs, and links are treated as data, not
instructions or trusted HTML.

The local OWI token/cookie and same-origin write gate remains in front of every
Manager endpoint. Private-repository access is disabled on an unencrypted
non-loopback bind unless the owner explicitly enables that risk.

## Consequences

This slice gives a real no-clone portfolio and draft-import workflow, but it is
not a webhook-synchronized hosted service and it cannot edit code or open pull
requests. A later hosted release may add GitHub App OAuth, webhook inbox and
reconciliation, ephemeral source snapshots, and a separately approved
write-back broker without weakening this read-only observer boundary.
