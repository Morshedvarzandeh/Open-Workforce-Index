# Subscription allowance and API costs

Open the connected ask page with `tools/owi-serve`, then expand
**Help when you need it → My plan & self-updating agents**.

Choose the billing method used by your Claude commands:

- **Not confirmed:** rank using API-equivalent estimates; the actual billing
  method and charges remain unknown.
- **API:** compare estimated API costs.
- **Subscription:** enter your plan name, the lowest remaining percentage
  shown across the relevant session/weekly limits, and the next reset time.
  Only declare included usage when your command actually uses that plan.

These settings apply to the listed Claude models. They do not change your
provider login, buy credits, or enable paid extra usage. Keep paid extra
usage off in your provider account to stay within included allowance.
Provider access and model availability still need to work independently.

## A declaration, not an account meter

OWI does not have a live connection to consumer subscription allowance.
Declarations expire after one hour or at the entered reset, whichever is
earlier. Zero, unknown, or expired allowance blocks that subscription route.
A reset does not automatically refill it: check the provider and refresh
the declaration using **Refresh these limits**.
Pausing instruction updates does not refresh the allowance.
Tokens are never converted into invented plan percentages.

An Anthropic API key, auth token, custom base URL, or cloud-provider override
in the server environment conflicts with a declared subscription route.
OWI blocks that route until its configuration is made consistent. This
check does not certify the billing hidden inside arbitrary custom commands.

## How recommendations change

The existing skill, tool, privacy, and quality gates run first. The account
policy can remove candidates or re-rank eligible ones; it cannot restore a
candidate that failed those gates.

For a declared included route, the maker's modeled incremental API cash is
zero. The monthly subscription fee remains a separate sunk/overhead cost.
The optional **Value of preserving quota per task** is an owner's preference
used in ranking, not a cash charge or a provider-measured quota unit.
The GUI uses the same entered value for the listed Claude models. Advanced
configuration can assign different per-model values.

The CLI retains its configured review/time cost in the comparison. Its
private `last-runtime-quote.json` records the account-adjusted recommendation
and settings; the engine's original API quote remains intact. The ask page
uses its simpler task estimate, without the CLI's hourly review-cost input.

Subscription maker runs use only declared included/local planners and
checkers. If no separate eligible helper exists, checks remain undecided for
human review. CLI escalation does not switch a subscription task to an API
route, including when the recommended subscription runner is unavailable.
A multi-part CLI planner also stays within declared included/local
routes when a subscription is configured. These rules do not bypass quotas.

## What is measured

Expand **Usage & agent version** under a connected result. Every command
invocation is recorded separately, including planners, makers, checkers,
failures, and timeouts. Records contain no task or output text.

| Field | Meaning |
|---|---|
| Input/output tokens | Counts reported by the runner; output includes thinking when the provider includes it |
| Cache reads/writes | Separate reported counters; not added to input again without knowing the provider's convention |
| API equivalent | Runner's estimated dollar equivalent, including its cache treatment when supported |
| Runner-reported charge | An explicit custom-adapter report; not an invoice reconciliation |
| Invoice charge | Unknown; OWI does not retrieve provider invoices |

Plain-text runners have unknown token counts and charges. Built-in Claude
commands now request JSON output and preserve the provider's text result.
Existing configured commands keep their format; add `--output-format json`
to a Claude command to expose its telemetry.

Claude's `total_cost_usd` is stored as an **estimate**, including for
subscription users. It is never treated as a subscription bill.
Unknown values in the new private `runtime.sqlite` usage ledger are null.
The older quality-outcome schema still requires numeric cash/quota fields;
its compatibility zeros now carry explicit `usage_status: unmeasured`
metadata and must not be used as evidence of free execution.

```bash
tools/owi-maintain usage                 # last 20 operations
tools/owi-maintain status                # billing declarations and agent versions
tools/owi-maintain configure --input my-runtime-settings.json
```

A custom runner can opt into `"formats": {"my-model": "owi-json"}` in runtime
settings and emit:

```json
{
  "owi_usage_version": 1,
  "output": "The task result",
  "usage": {
    "input_tokens": 100,
    "output_tokens": 30,
    "cache_read_input_tokens": 900,
    "cache_creation_input_tokens": 50,
    "api_equivalent_micros": 12000,
    "reported_charge_micros": null
  }
}
```

Cash uses integer millionths of a USD. Configurations accept `billing`,
`learning`, and `formats`. Billing is keyed by model identifier, with
`mode`, optional `plan`, `remaining_percent`, epoch-seconds `reset_at`,
and integer `quota_value_micros`. An omitted `verified_at` stamps a fresh
declaration; preserve the returned timestamp when changing unrelated settings.

## Provider references

Checked 13 September 2026:

- [Claude Code costs](https://code.claude.com/docs/en/costs):
  subscription allowances, API-equivalent figures, cache behavior, and thinking.
- [Claude Code on Pro/Max](https://support.claude.com/en/articles/11145838-use-claude-code-with-your-pro-or-max-plan):
  separate API billing and environment-key precedence.
- [Programmatic Claude Code](https://code.claude.com/docs/en/headless):
  structured output and estimated per-invocation costs.

OWI does not hardcode consumer-plan token quotas, discounts, or a promised
subscription/API conversion ratio. Cache configuration remains the runner's
responsibility; telemetry improves transparency, not the accuracy of an
unmeasured future task's estimate.
