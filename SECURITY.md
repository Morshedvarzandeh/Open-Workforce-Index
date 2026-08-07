# Security policy

## Reporting a vulnerability

Do not open a public issue for a vulnerability involving credential exposure,
authorization bypass, private-data export, repository escape, or unbounded
spend. Use GitHub's private vulnerability reporting for this repository. If it
is unavailable, contact the maintainer privately through their GitHub profile.

Include the affected revision, reproduction steps, expected impact, and any
safe mitigation you found. Do not include real credentials, proprietary prompts,
or private repository content.

## Security invariants

Implemented at the v0.1 decision and storage boundary:

- Public index and private allocator data use physically separate databases.
- Public export code depends only on a public read interface.
- Public-export regression tests reject private task and repository markers.
- A maker and checker cannot be the same exact worker identity, and a linked
  outcome must use the checker selected by its recorded quote.
- Secret-class tasks are classified but rejected by the v0.1 allocator; they
  are not eligible for model routing.

The v0.1 CLI is a decision-kernel demonstration, not an execution or credential
boundary. Fixture/adaptor input is trusted. In particular, free-form private
outcome metadata and repository-scope fields are not secret stores and must not
contain credentials, prompt bodies, repository content, or other secret values.

Required before OWI is allowed to execute model or repository work:

- Credentials are held by the operating system's credential facility and are
  never accepted through free-form task, outcome, database, or RDF fields.
- A deterministic policy layer—not the planner—selects eligible workers,
  grants tools and repository scope, and issues bounded spend leases.
- All consequential actions require deterministic permission, human-approval,
  and budget gates.
- Prompts, repository files, web content, and tool output are untrusted input.
- Network, repository paths, tools, spend, retries, wall time, and delegation
  depth are deny-by-default bounded leases.

Telemetry is off by default. Future contribution of local outcome statistics
must be explicit, inspectable, revocable, and sanitized into a new public record
without private identifiers or provenance links.
