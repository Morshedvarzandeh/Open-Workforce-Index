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

- Public index and private allocator data use physically separate databases.
- Public export code depends only on a public read interface.
- Credentials and secret values are never written to either database or RDF.
- All consequential actions require deterministic permission and budget gates.
- The planner cannot select its model, grant its tools, or increase its budget.
- A critical maker and checker cannot be the same exact worker identity.
- Prompts, repository files, web content, and tool output are untrusted input.
- Network, repository paths, tools, spend, retries, wall time, and delegation
  depth are deny-by-default bounded leases.

Telemetry is off by default. Future contribution of local outcome statistics
must be explicit, inspectable, revocable, and sanitized into a new public record
without private identifiers or provenance links.
